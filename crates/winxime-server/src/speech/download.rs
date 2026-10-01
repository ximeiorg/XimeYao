//! 语音模型目录与下载（本地离线模型）。
//!
//! 分工（对齐公开的跨端约定）：模型**目录布局**在 [`crate::models`]（`models/<id>/`
//! 四件套平铺），**模型清单**在 `xime-speech` 的 [`AsrModelRegistry`]（id/展示名/
//! 下载地址/文件名角色），本模块只做两件事：
//!
//! 1. [`catalog`]：把注册表 + 磁盘现状 + 当前选中合并成设置页要的列表；
//! 2. 下载：注册表给的是 ModelScope 的 `.tar.bz2`，流式下载（带进度）→
//!    解压时**只取文件名**铺平到 `models/<id>/`（包内有一层顶层目录，
//!    不留目录结构也是防目录穿越的一条硬防线）。
//!
//! 下载在**独立线程**里跑（不能占识别器工作线程：用户可能一边下载一边听写），
//! 进度写全局槽，设置页按 250ms 轮询取走；同一时刻只允许一个下载在进行。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use xime_speech::{AsrModelProfile, AsrModelRegistry};

use super::settings;

/// 下载时先落的临时文件名（解压成功后删除；失败也删，不留半截包）。
const ARCHIVE_TMP: &str = ".download.tar.bz2";

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

/// 下载并解压到 `models/<id>/`；`on_progress` 收 0.0~1.0。
fn download_and_extract(
    data_root: &Path,
    profile: &AsrModelProfile,
    on_progress: impl Fn(f32),
) -> Result<(), String> {
    let rime_user_dir = data_root.join("rime");
    let dir = crate::models::ensure_model_dir(&rime_user_dir, &profile.id)?;
    let archive = dir.join(ARCHIVE_TMP);

    let result = (|| -> Result<(), String> {
        download_archive(&profile.download_url, &archive, &on_progress)?;
        on_progress(0.9);
        extract_tar_bz2(&archive, &dir)?;
        on_progress(1.0);
        Ok(())
    })();

    // 无论成败都清掉临时包：失败时它是半截文件，成功时已经没用了。
    let _ = std::fs::remove_file(&archive);
    result
}

/// 流式下载归档到 `dest`（进度按 Content-Length 折算到 0.0~0.9）。
fn download_archive(
    url: &str,
    dest: &Path,
    on_progress: &impl Fn(f32),
) -> Result<(), String> {
    let mut response = ureq::get(url)
        .call()
        .map_err(|e| format!("连接模型下载源失败: {e}"))?;
    let total: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<u64>().ok());

    let mut reader = response.body_mut().as_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| format!("创建模型包文件失败: {e}"))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| format!("读取模型包失败: {e}"))?;
        if read == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buffer[..read])
            .map_err(|e| format!("写入模型包失败: {e}"))?;
        written += read as u64;
        if let Some(total) = total.filter(|total| *total > 0) {
            // 下载占前 90%，留 10% 给解包。
            on_progress(0.9 * (written as f32 / total as f32));
        }
    }
    if let Some(total) = total.filter(|total| *total > 0) {
        if written < total {
            return Err(format!("模型包不完整（{written}/{total} 字节）"));
        }
    }
    Ok(())
}

/// 解压 `.tar.bz2` 到 `dest_dir`：只取每个条目的**文件名**铺平（防目录穿越），
/// 目录条目与无名条目跳过。
fn extract_tar_bz2(archive: &Path, dest_dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("打开模型包失败: {e}"))?;
    let decoder = bzip2_rs::DecoderReader::new(file);
    let mut tar = tar::Archive::new(decoder);
    let entries = tar.entries().map_err(|e| format!("读取模型包目录失败: {e}"))?;
    let mut unpacked = 0usize;
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
        let target = dest_dir.join(&name);
        let mut out =
            std::fs::File::create(&target).map_err(|e| format!("创建模型文件失败: {e}"))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("写出模型文件失败: {e}"))?;
        unpacked += 1;
    }
    if unpacked == 0 {
        return Err("模型包内没有文件".to_string());
    }
    Ok(())
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

    /// 测试用数据根（见 `speech::settings` 测试同一口径：本环境不能在 %TEMP% 建目录）。
    fn temp_root(label: &str) -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!("speech_download_{label}"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::create_dir_all(root.join("rime"));
        root
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