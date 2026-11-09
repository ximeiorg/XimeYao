//! 语音模型目录与下载（本地离线模型）。
//!
//! 分工（对齐公开的跨端约定）：模型**目录布局**在 [`crate::models`]（`models/<id>/`
//! 四件套平铺），**模型清单**在 `xime-speech` 的 [`AsrModelRegistry`]（id/展示名/
//! 下载地址/文件名角色），本模块只做两件事：
//!
//! 1. `catalog`：把注册表 + 磁盘现状 + 当前选中合并成设置页要的列表；
//! 2. 下载：注册表给的是 ModelScope 的 `.tar.bz2`，流式下载（带进度）→
//!    比对内置 SHA-256 → 解压到暂存目录 → 整体替换 `models/<id>/`。
//!
//! 下载地址是 `resolve/master/...`（可变分支），所以「装到磁盘上的东西必须是
//! 注册表里那一份」由**摘要**保证，而不是由 URL 保证；校验不过就不解压、
//! 不碰已有模型目录。
//!
//! 解压时**只取文件名**铺平（包内有一层顶层目录，不留目录结构也是防目录穿越
//! 的一条硬防线）。
//!
//! 下载在**独立线程**里跑（不能占识别器工作线程：用户可能一边下载一边听写），
//! 进度写全局槽，设置页按 250ms 轮询取走；同一时刻只允许一个下载在进行。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use xime_speech::{AsrModelProfile, AsrModelRegistry};

use super::settings;
use crate::hashing::Sha256Writer;

/// 模型包体积上限（审计 F4）：注册表是编译内置的，但下载源在网络另一端，
/// `Content-Length` 和实际字节都不能当可信输入——超限立即中止，不给写满磁盘的机会。
const MAX_MODEL_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// 解压后的单文件上限（现有包里最大的 onnx 约 100 MB，留 5 倍余量）。
const MAX_MODEL_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// 解压后合计上限：正常整包在 300 MB 量级。
const MAX_MODEL_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

/// 解压限额。默认值即线上上限；单测注入小值跑「超限」分支，不必造几百 MB 的包。
#[derive(Clone, Copy)]
struct ExtractLimits {
    file_bytes: u64,
    total_bytes: u64,
}

impl Default for ExtractLimits {
    fn default() -> Self {
        Self {
            file_bytes: MAX_MODEL_FILE_BYTES,
            total_bytes: MAX_MODEL_TOTAL_BYTES,
        }
    }
}

/// 设置页要展示的一个模型条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// 模型 id（= 目录名）。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 一句话描述。
    pub description: String,
    /// 下载包大小（如实标注，仅展示）。
    pub size: String,
    /// 四件套是否已下载完整。
    pub downloaded: bool,
    /// 是否为当前选中。
    pub selected: bool,
    /// 是否为推荐模型。
    pub recommended: bool,
}

/// 正在进行的下载（进度 0.0~1.0）。
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadState {
    /// 目标模型 id。
    pub model_id: String,
    /// 进度：下载占 0.0~0.9，解压占 0.9~1.0。
    pub progress: f32,
}

/// 当前下载槽（None = 没有下载在进行）。
static DOWNLOAD: Mutex<Option<DownloadState>> = Mutex::new(None);

/// 最近一次模型操作错误（下载 / 删除；设置页读到就展示，下次操作时清空）。
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// 模型集合变化计数：下载完成 / 删除完成都 +1，设置页据此决定要不要重查列表。
static REV: AtomicU64 = AtomicU64::new(0);

/// 取锁并忽略中毒（中毒说明别的线程 panic 过，数据本身仍可用）。
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 模型列表：注册表顺序（默认模型在首位）+ 磁盘现状 + 当前选中。
pub fn catalog(data_root: &Path) -> Vec<ModelInfo> {
    let rime_user_dir = data_root.join("rime");
    let selected_id = settings::selected_profile(data_root).id;
    let recommended_id = AsrModelRegistry::recommended_id();
    AsrModelRegistry::profiles()
        .into_iter()
        .map(|profile| ModelInfo {
            downloaded: crate::models::is_model_downloaded(
                &rime_user_dir,
                &profile.id,
                &settings::profile_files(&profile),
            ),
            selected: profile.id == selected_id,
            recommended: profile.id == recommended_id,
            id: profile.id,
            name: profile.name,
            description: profile.description,
            size: profile.size,
        })
        .collect()
}

/// 当前下载状态（设置页每拍读）。
pub fn download_state() -> Option<DownloadState> {
    lock(&DOWNLOAD).clone()
}

/// 最近一次模型操作错误。
pub fn last_error() -> Option<String> {
    lock(&LAST_ERROR).clone()
}

/// 记一次模型操作错误（删除失败等由工作线程调用）。
pub fn set_last_error(message: String) {
    *lock(&LAST_ERROR) = Some(message);
}

/// 模型集合变化计数。
pub fn models_rev() -> u64 {
    REV.load(Ordering::Relaxed)
}

/// 记一次模型集合变化（删除由工作线程做，也要让设置页知道）。
pub fn bump_rev() {
    REV.fetch_add(1, Ordering::Relaxed);
}

/// 开始下载指定模型（异步；同一时刻只允许一个）。
///
/// 只做三件同步的事：查注册表、占下载槽、起线程；真正的网络与解包在线程里，
/// 因此 IPC 侧的静默超时（100ms）不会受它影响。
pub fn start_download(data_root: PathBuf, model_id: String) -> Result<(), String> {
    let profile = AsrModelRegistry::find_by_id(&model_id)
        .ok_or_else(|| format!("未知模型：{model_id}（本端注册表里没有）"))?;
    {
        let mut slot = lock(&DOWNLOAD);
        if slot.is_some() {
            return Err("已有模型正在下载，请等它结束".to_string());
        }
        *slot = Some(DownloadState {
            model_id: model_id.clone(),
            progress: 0.0,
        });
    }
    *lock(&LAST_ERROR) = None;

    let spawned = std::thread::Builder::new()
        .name("xime-speech-dl".into())
        .spawn(move || {
            let result = download_and_extract(&data_root, &profile, set_progress);
            *lock(&DOWNLOAD) = None;
            if let Err(error) = result {
                tracing::warn!("语音模型下载失败（{}）：{error}", profile.id);
                *lock(&LAST_ERROR) = Some(error);
            } else {
                tracing::info!("语音模型下载完成：{}", profile.id);
            }
            // 成功 / 失败都 +1：设置页据此重查列表或刷新错误行。
            bump_rev();
        });
    if spawned.is_err() {
        *lock(&DOWNLOAD) = None;
        return Err("无法启动下载线程".to_string());
    }
    Ok(())
}

/// 写下载进度（只在 0.0~1.0 内递增，避免 UI 进度条倒退）。
fn set_progress(progress: f32) {
    let mut slot = lock(&DOWNLOAD);
    if let Some(state) = slot.as_mut() {
        let clamped = progress.clamp(0.0, 1.0);
        if clamped > state.progress {
            state.progress = clamped;
        }
    }
}

/// 下载 → 校验 → 解压到暂存 → 整体替换；`on_progress` 收 0.0~1.0。
fn download_and_extract(
    data_root: &Path,
    profile: &AsrModelProfile,
    on_progress: impl Fn(f32),
) -> Result<(), String> {
    let rime_user_dir = data_root.join("rime");
    let models_root = crate::models::models_root(&rime_user_dir);
    std::fs::create_dir_all(&models_root).map_err(|e| format!("创建模型根目录失败: {e}"))?;
    let target = models_root.join(&profile.id);
    let archive = unique_sibling(&models_root, &format!("{}.download", profile.id));
    let staging = models_root.join(format!(
        ".xime-{}.staging-{}",
        profile.id,
        std::process::id()
    ));

    let result = (|| -> Result<(), String> {
        let (written, digest) = download_archive(&profile.download_url, &archive, &on_progress)?;
        verify_package(profile, written, &digest)?;
        on_progress(0.9);
        // 解压写暂存：中途失败只脏暂存目录，已有模型目录一个字节都不动。
        if staging.exists() {
            std::fs::remove_dir_all(&staging)
                .map_err(|e| format!("清理上次遗留的暂存目录失败: {e}"))?;
        }
        std::fs::create_dir_all(&staging).map_err(|e| format!("创建暂存目录失败: {e}"))?;
        extract_tar_bz2(&archive, &staging)?;
        on_progress(0.95);
        replace_model_dir(&staging, &target)?;
        on_progress(1.0);
        Ok(())
    })();

    // 无论成败都清掉临时包：失败时它是半截文件，成功时已经没用了。
    let _ = std::fs::remove_file(&archive);
    if result.is_err() && staging.exists() {
        if let Err(e) = std::fs::remove_dir_all(&staging) {
            tracing::warn!("清理模型暂存目录 {} 失败: {e}", staging.display());
        }
    }
    result
}

/// 流式下载归档到 `dest`，同时算 SHA-256（进度按 Content-Length 折算到 0.0~0.9）。
///
/// 返回 `(实际字节数, 摘要)`；两者都基于**真正写下去的字节**，不信服务端声明。
fn download_archive(
    url: &str,
    dest: &Path,
    on_progress: &impl Fn(f32),
) -> Result<(u64, String), String> {
    let mut response = ureq::get(url)
        .call()
        .map_err(|e| format!("连接模型下载源失败: {e}"))?;
    let total: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<u64>().ok());
    ensure_declared_size(total)?;

    let file = std::fs::File::create(dest).map_err(|e| format!("创建模型包文件失败: {e}"))?;
    let mut sink = Sha256Writer::new(file);
    let written = stream_to_file(
        &mut response.body_mut().as_reader(),
        &mut sink,
        MAX_MODEL_DOWNLOAD_BYTES,
        |written| {
            if let Some(total) = total.filter(|total| *total > 0) {
                // 下载占前 90%，留 10% 给解包。
                on_progress(0.9 * (written as f32 / total as f32));
            }
        },
    )?;
    if let Some(total) = total.filter(|total| *total > 0) {
        if written < total {
            return Err(format!("模型包不完整（{written}/{total} 字节）"));
        }
    }
    let (_file, digest) = sink
        .finish()
        .map_err(|e| format!("收尾模型包文件失败: {e}"))?;
    Ok((written, digest))
}

/// 源站声明的包大小超限就直接拒绝，一个字节都不下。
fn ensure_declared_size(total: Option<u64>) -> Result<(), String> {
    if let Some(total) = total.filter(|total| *total > MAX_MODEL_DOWNLOAD_BYTES) {
        return Err(format!(
            "模型包声明 {total} 字节，超过上限 {MAX_MODEL_DOWNLOAD_BYTES} 字节，已取消下载"
        ));
    }
    Ok(())
}

/// 下载字节与内置校验值比对：字节数先比（廉价、可读），再比 SHA-256。
///
/// 摘要不符有一种「正常」成因：`resolve/master` 是可变分支，上游重打包后
/// 同名文件内容就变了。所以文案不能说「被篡改」，只能说明不一致、建议升级。
fn verify_package(
    profile: &AsrModelProfile,
    written: u64,
    actual_sha256: &str,
) -> Result<(), String> {
    let Some(expected) = profile.checksum.as_ref() else {
        return Err(format!(
            "模型 {} 缺少内置校验值，已拒绝安装（请升级到带校验的版本）",
            profile.id
        ));
    };
    if written != expected.size_bytes {
        return Err(format!(
            "模型包大小与内置记录不一致（实际 {written} 字节，应为 {} 字节），已删除；请检查网络后重试",
            expected.size_bytes
        ));
    }
    if !actual_sha256.eq_ignore_ascii_case(&expected.sha256) {
        tracing::warn!(
            "模型包摘要不符（{}）：期望 {}，实际 {}",
            profile.id,
            expected.sha256,
            actual_sha256
        );
        return Err(
            "模型包与内置校验值不一致，已删除。下载地址是可变分支，上游可能更新了模型包；若确认官方已更新，请升级程序后再试。"
                .to_string(),
        );
    }
    Ok(())
}

/// 在 `parent` 下挑一个不存在的同级路径（归档临时文件、替换前的旧目录都放这里）。
fn unique_sibling(parent: &Path, stem: &str) -> PathBuf {
    for n in 0..1000 {
        let candidate = parent.join(format!(".xime-{stem}-{}-{n}", std::process::id()));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!(".xime-{stem}-{}-overflow", std::process::id()))
}

/// 用暂存目录整体替换模型目录：目标已存在时先挪开，换不进去再挪回来。
///
/// Windows 上目录替换不是原子的，也没有「交换」系统调用；这里的保证是
/// **失败不留半套**：旧目录要么原样在位，要么已成功换成新目录。
fn replace_model_dir(staging: &Path, target: &Path) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| "模型目录缺少上级目录".to_string())?;
    if !target.exists() {
        return std::fs::rename(staging, target).map_err(|e| format!("启用新模型目录失败: {e}"));
    }
    let old = unique_sibling(parent, "model-old");
    std::fs::rename(target, &old).map_err(|e| {
        format!("替换模型目录失败：旧模型可能正在使用，请先在设置页切到别的模型再试（{e}）")
    })?;
    match std::fs::rename(staging, target) {
        Ok(()) => {
            if let Err(e) = std::fs::remove_dir_all(&old) {
                tracing::warn!("清理旧模型目录 {} 失败: {e}", old.display());
            }
            Ok(())
        }
        Err(e) => {
            if let Err(back) = std::fs::rename(&old, target) {
                tracing::warn!(
                    "回滚模型目录失败（{}）：{back}；旧文件在 {}",
                    target.display(),
                    old.display()
                );
            }
            Err(format!("启用新模型目录失败，已保留原模型: {e}"))
        }
    }
}

/// 边读边写，累计字节超过 `cap` 立即中止。
///
/// 只认实际写入的字节：`Content-Length` 可以撒谎（或干脆不给），不能拿它当上限依据。
/// 中止时已写出的半截文件由调用方删除。
fn stream_to_file<R: Read, W: Write>(
    reader: &mut R,
    sink: &mut W,
    cap: u64,
    on_bytes: impl Fn(u64),
) -> Result<u64, String> {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| format!("读取模型包失败: {e}"))?;
        if read == 0 {
            break;
        }
        written = written.saturating_add(read as u64);
        if written > cap {
            return Err(format!(
                "模型包超过上限 {cap} 字节，已中止下载（下载源可能被篡改）"
            ));
        }
        sink.write_all(&buffer[..read])
            .map_err(|e| format!("写入模型包失败: {e}"))?;
        on_bytes(written);
    }
    Ok(written)
}

/// 解压 `.tar.bz2`：先解成 tar 流再走 [`extract_tar`]。
fn extract_tar_bz2(archive: &Path, dest_dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("打开模型包失败: {e}"))?;
    let decoder = bzip2_rs::DecoderReader::new(file);
    extract_tar(decoder, dest_dir, ExtractLimits::default())
}

/// 解压 tar 流到 `dest_dir`：只取每个条目的**文件名**铺平（防目录穿越），
/// 目录条目与无名条目跳过。
///
/// 限额两道：单文件按头部声明与实际写入双查，合计按实际写入累计；
/// 中途失败把已写出的文件删掉，不留半套模型让识别器误判「已下载」。
fn extract_tar<R: Read>(reader: R, dest_dir: &Path, limits: ExtractLimits) -> Result<(), String> {
    let mut tar = tar::Archive::new(reader);
    let entries = tar
        .entries()
        .map_err(|e| format!("读取模型包目录失败: {e}"))?;
    let mut unpacked = 0usize;
    let mut total: u64 = 0;
    let mut written_files: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<(), String> {
        for entry in entries {
            let mut entry = entry.map_err(|e| format!("读取模型包条目失败: {e}"))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let path = entry
                .path()
                .map_err(|e| format!("读取模型包条目路径失败: {e}"))?;
            let name = match flattened_name(&path) {
                Some(name) => name,
                None => continue,
            };
            // 头部声明先查一遍，避免为大文件先建出空文件再半路失败。
            let declared = entry
                .header()
                .size()
                .map_err(|e| format!("读取模型包条目大小失败: {e}"))?;
            if declared > limits.file_bytes {
                return Err(format!(
                    "模型文件 {} 声明 {declared} 字节，超过单文件上限 {} 字节",
                    name.to_string_lossy(),
                    limits.file_bytes
                ));
            }
            if total.saturating_add(declared) > limits.total_bytes {
                return Err(format!(
                    "模型包解压后超过合计上限 {} 字节",
                    limits.total_bytes
                ));
            }
            let target = dest_dir.join(&name);
            let mut out =
                std::fs::File::create(&target).map_err(|e| format!("创建模型文件失败: {e}"))?;
            written_files.push(target);
            let copied = std::io::copy(&mut entry, &mut out)
                .map_err(|e| format!("写出模型文件失败: {e}"))?;
            total = total.saturating_add(copied);
            // 实际字节再查一遍：头部声明和实际流可以不一致。
            if copied > limits.file_bytes {
                return Err(format!(
                    "模型文件 {} 超过单文件上限 {} 字节",
                    name.to_string_lossy(),
                    limits.file_bytes
                ));
            }
            if total > limits.total_bytes {
                return Err(format!(
                    "模型包解压后超过合计上限 {} 字节",
                    limits.total_bytes
                ));
            }
            unpacked += 1;
        }
        if unpacked == 0 {
            return Err("模型包内没有文件".to_string());
        }
        Ok(())
    })();
    if result.is_err() {
        for path in &written_files {
            if let Err(e) = std::fs::remove_file(path) {
                tracing::warn!("清理半成品模型文件 {} 失败: {e}", path.display());
            }
        }
    }
    result
}

/// 铺平用文件名：只认普通文件名，`.` / `..` / 空名一律拒绝。
fn flattened_name(path: &Path) -> Option<std::ffi::OsString> {
    let name = path.file_name()?;
    let text = name.to_string_lossy();
    if text.is_empty() || text == "." || text == ".." {
        return None;
    }
    Some(name.to_os_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用数据根（统一走 `test_support`，见其模块注释）。
    fn temp_root(label: &str) -> PathBuf {
        crate::test_support::test_root(&format!("speech_download_{label}"))
    }

    #[test]
    fn catalog_marks_downloaded_and_selected() {
        let root = temp_root("catalog");
        let entries = catalog(&root);
        assert_eq!(entries.len(), AsrModelRegistry::profiles().len());
        // 空目录：都没下载；默认模型被标记为选中。
        assert!(entries.iter().all(|entry| !entry.downloaded));
        let selected: Vec<&str> = entries
            .iter()
            .filter(|entry| entry.selected)
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(selected, vec![AsrModelRegistry::default_profile().id.as_str()]);
        assert!(entries.iter().all(|entry| !entry.size.is_empty()));

        // 默认模型四件套齐了 → 只有它 downloaded。
        let profile = AsrModelRegistry::default_profile();
        let dir = crate::models::ensure_model_dir(&root.join("rime"), &profile.id).expect("建模型目录");
        for name in settings::profile_files(&profile) {
            std::fs::write(dir.join(name), b"bytes").expect("写模型文件");
        }
        let entries = catalog(&root);
        let downloaded: Vec<&str> = entries
            .iter()
            .filter(|entry| entry.downloaded)
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(downloaded, vec![profile.id.as_str()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn start_download_rejects_unknown_model() {
        let root = temp_root("unknown");
        let error = start_download(root.clone(), "no-such-model".to_string())
            .expect_err("未知模型必须被拒绝");
        assert!(error.contains("未知模型"), "{error}");
        // 拒绝时不占下载槽。
        assert!(download_state().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flattened_name_only_accepts_file_names() {
        assert_eq!(
            flattened_name(Path::new("pkg/encoder.int8.onnx")),
            Some(std::ffi::OsString::from("encoder.int8.onnx"))
        );
        assert_eq!(
            flattened_name(Path::new("pkg/sub/tokens.txt")),
            Some(std::ffi::OsString::from("tokens.txt"))
        );
        assert_eq!(flattened_name(Path::new("..")), None);
        assert_eq!(flattened_name(Path::new(".")), None);
        assert_eq!(flattened_name(Path::new("")), None);
    }

    #[test]
    fn rev_starts_stable_and_only_grows() {
        let before = models_rev();
        bump_rev();
        assert!(models_rev() > before);
    }

    /// 造一个未压缩 tar 流：列出的都是普通文件，路径按原样写入。
    fn tar_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            builder
                .append_data(&mut header, name, *data)
                .unwrap_or_default();
        }
        builder.into_inner().unwrap_or_default()
    }

    /// 源站声明的包大小超限：一个字节都不下，错误里说清上限。
    #[test]
    fn declared_size_over_cap_is_rejected() {
        let err = ensure_declared_size(Some(MAX_MODEL_DOWNLOAD_BYTES + 1))
            .expect_err("声明超限必须拒绝");
        assert!(err.contains("超过上限"), "错误要说清原因: {err}");
        assert!(ensure_declared_size(Some(1024)).is_ok());
        assert!(ensure_declared_size(None).is_ok(), "没有 Content-Length 不拦");
    }

    /// 实际下载字节超限：流式写入中途就中止，不留「先写满再说」的窗口。
    #[test]
    fn stream_to_file_aborts_when_bytes_exceed_cap() {
        let root = temp_root("stream_cap");
        let path = root.join("pkg.tar.bz2");
        let mut file = std::fs::File::create(&path).expect("创建临时包文件");
        let mut reader = std::io::Cursor::new(vec![7u8; 128]);
        let err = stream_to_file(&mut reader, &mut file, 64, |_| {})
            .expect_err("超过上限必须中止");
        assert!(err.contains("超过上限"), "错误要说清原因: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 包内单个条目的声明大小超限：连目标文件都不建。
    #[test]
    fn extract_rejects_oversized_entry_declaration() {
        let root = temp_root("extract_file");
        let dest = root.join("models");
        std::fs::create_dir_all(&dest).unwrap_or_default();
        let limits = ExtractLimits {
            file_bytes: 8,
            total_bytes: 1024,
        };
        let bytes = tar_bytes(&[("pkg/encoder.onnx", &[1u8; 64][..])]);
        let err = extract_tar(std::io::Cursor::new(bytes), &dest, limits)
            .expect_err("单文件超限必须拒绝");
        assert!(err.contains("单文件上限"), "错误要说清原因: {err}");
        assert_eq!(file_count(&dest), 0, "拒绝时不能留下空壳文件");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 合计超限：先写出的文件要清掉，避免留下半套模型被当成「已下载」。
    #[test]
    fn extract_rejects_total_over_limit_and_cleans_partial_files() {
        let root = temp_root("extract_total");
        let dest = root.join("models");
        std::fs::create_dir_all(&dest).unwrap_or_default();
        let limits = ExtractLimits {
            file_bytes: 1024,
            total_bytes: 50,
        };
        let bytes = tar_bytes(&[
            ("pkg/encoder.onnx", &[1u8; 40][..]),
            ("pkg/tokens.txt", &[2u8; 40][..]),
        ]);
        let err = extract_tar(std::io::Cursor::new(bytes), &dest, limits)
            .expect_err("合计超限必须拒绝");
        assert!(err.contains("合计上限"), "错误要说清原因: {err}");
        assert_eq!(file_count(&dest), 0, "半成品要删干净");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 只取文件名铺平：包内嵌套目录不会在目标目录里复现。
    #[test]
    fn extract_flattens_nested_paths() {
        let root = temp_root("extract_flat");
        let dest = root.join("models");
        std::fs::create_dir_all(&dest).unwrap_or_default();
        let bytes = tar_bytes(&[
            ("pkg/encoder.onnx", &[1u8; 4][..]),
            ("pkg/sub/tokens.txt", &[2u8; 3][..]),
        ]);
        extract_tar(std::io::Cursor::new(bytes), &dest, ExtractLimits::default())
            .expect("正常包应解压成功");
        assert_eq!(file_count(&dest), 2);
        assert!(dest.join("encoder.onnx").is_file());
        assert!(dest.join("tokens.txt").is_file());
        assert!(!dest.join("sub").exists(), "不应复现包内目录结构");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 造一个带指定校验值的 profile（不动内置模型的真实摘要）。
    fn profile_with_checksum(sha256: &str, size_bytes: u64) -> AsrModelProfile {
        let mut profile = AsrModelRegistry::default_profile();
        profile.checksum = Some(xime_speech::ModelChecksum {
            sha256: sha256.to_string(),
            size_bytes,
        });
        profile
    }

    #[test]
    fn verify_package_accepts_matching_digest() {
        let data = b"model-bytes";
        let digest = crate::hashing::sha256_hex(data);
        let profile = profile_with_checksum(&digest, data.len() as u64);
        verify_package(&profile, data.len() as u64, &digest).expect("摘要相符应通过");
    }

    /// 没有内置摘要就不能装：宁可装不上，也不装来路不明的东西。
    #[test]
    fn verify_package_rejects_missing_checksum() {
        let mut profile = AsrModelRegistry::default_profile();
        profile.checksum = None;
        let err = verify_package(&profile, 0, "").expect_err("无校验值必须拒绝");
        assert!(err.contains("缺少内置校验值"), "错误要说清原因: {err}");
    }

    #[test]
    fn verify_package_rejects_size_mismatch() {
        let sha = "a".repeat(64);
        let profile = profile_with_checksum(&sha, 10);
        let err = verify_package(&profile, 9, &sha).expect_err("字节数不符必须拒绝");
        assert!(err.contains("大小与内置记录不一致"), "{err}");
    }

    /// 摘要不符时只说「不一致」：下载地址是可变分支，上游真更新也会不符。
    #[test]
    fn verify_package_rejects_digest_mismatch_without_claiming_tampering() {
        let expected = "a".repeat(64);
        let actual = "b".repeat(64);
        let profile = profile_with_checksum(&expected, 10);
        let err = verify_package(&profile, 10, &actual).expect_err("摘要不符必须拒绝");
        assert!(err.contains("与内置校验值不一致"), "{err}");
        assert!(!err.contains("篡改"), "不能断言被篡改: {err}");
    }

    /// 摘要算的是真正写下去的字节（流式一遍，不二次读盘）。
    #[test]
    fn stream_to_file_hashes_exactly_what_it_writes() {
        let root = temp_root("stream_hash");
        let path = root.join("pkg.bin");
        let file = std::fs::File::create(&path).expect("创建临时文件");
        let mut sink = Sha256Writer::new(file);
        let data = vec![9u8; 300];
        let mut reader = std::io::Cursor::new(data.clone());
        let written = stream_to_file(&mut reader, &mut sink, 1024, |_| {}).expect("写入成功");
        let (_file, digest) = sink.finish().expect("取摘要");
        assert_eq!(written, 300);
        assert_eq!(digest, crate::hashing::sha256_hex(&data));
        assert_eq!(std::fs::read(&path).expect("读回文件"), data);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 替换已存在的模型目录：新内容到位、暂存与旧目录都不留。
    #[test]
    fn replace_model_dir_swaps_in_place_and_cleans_old_copy() {
        let root = temp_root("replace_swap");
        let models = root.join("models");
        let target = models.join("m");
        let staging = models.join(".staging");
        std::fs::create_dir_all(&target).expect("建旧模型目录");
        std::fs::write(target.join("encoder.onnx"), b"old").expect("写旧文件");
        std::fs::create_dir_all(&staging).expect("建暂存目录");
        std::fs::write(staging.join("encoder.onnx"), b"new").expect("写新文件");

        replace_model_dir(&staging, &target).expect("替换应成功");
        assert_eq!(
            std::fs::read(target.join("encoder.onnx")).expect("读新文件"),
            b"new"
        );
        assert!(!staging.exists(), "暂存目录应已被移走");
        let leftovers: Vec<String> = std::fs::read_dir(&models)
            .expect("读模型根目录")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("model-old"))
            .collect();
        assert!(leftovers.is_empty(), "旧目录应被清理: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 首次安装（目标不存在）：暂存目录直接改名过去。
    #[test]
    fn replace_model_dir_moves_staging_when_target_absent() {
        let root = temp_root("replace_fresh");
        let models = root.join("models");
        let target = models.join("m");
        let staging = models.join(".staging");
        std::fs::create_dir_all(&staging).expect("建暂存目录");
        std::fs::write(staging.join("tokens.txt"), b"tokens").expect("写文件");

        replace_model_dir(&staging, &target).expect("首次安装应成功");
        assert_eq!(
            std::fs::read(target.join("tokens.txt")).expect("读文件"),
            b"tokens"
        );
        assert!(!staging.exists(), "暂存目录应已被移走");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 目标目录下普通文件数量（递归不了，这里就一层）。
    fn file_count(dir: &Path) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
            .count()
    }

    /// 下载源连通性冒烟（要联网，`--ignored` 手动跑）。
    ///
    /// 只验「TLS + HTTP + 能流式读」这三段：本机 curl 当初必须加
    /// `--ssl-no-revoke` 才过吊销检查，ureq 走 rustls 是否也拦得住，只有真连一次
    /// 才知道；真下 132MB 没必要（下不下来的原因在连接阶段就暴露了）。
    #[test]
    #[ignore = "需要联网；cargo test -- --ignored 手动跑"]
    fn download_source_is_reachable() {
        for profile in AsrModelRegistry::profiles() {
            let mut response = ureq::get(&profile.download_url)
                .call()
                .unwrap_or_else(|e| panic!("{} 连接失败: {e}", profile.id));
            let total: u64 = response
                .headers()
                .get("content-length")
                .and_then(|value| value.to_str().ok())
                .and_then(|text| text.parse().ok())
                .unwrap_or(0);
            assert!(
                total > 10 * 1024 * 1024,
                "{} 的下载源报的包大小不合理: {total} 字节",
                profile.id
            );
            let mut reader = response.body_mut().as_reader();
            let mut head = [0u8; 64 * 1024];
            let read = std::io::Read::read(&mut reader, &mut head).unwrap_or(0);
            assert!(read > 0, "{} 读不到字节", profile.id);
            // bzip2 魔数 "BZh"：下到的确实是 bz2 包（不是重定向后的 HTML 错误页）。
            assert_eq!(&head[..3], b"BZh", "{} 下到的不是 bzip2 包", profile.id);
        }
    }
}