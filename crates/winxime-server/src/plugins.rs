//! 插件宿主（对齐 Xime plugin-core 的宿主侧职责分工）：
//! - 宿主做重活：内置插件安装、JS 运行时管理、备份包打包/恢复、剪贴板监听与去重
//! - 插件只做协议传输（WebDAV PUT/GET/PROPFIND/DELETE，经 host.http/host.crypto/host.config）
//!
//! 线程模型（PluginRuntime 非 Send，运行时必须活在创建线程内）：
//! - backup 类操作：一操作一实例（调用线程内加载、用完即弃，onLoad 幂等）
//! - 剪贴板同步：专用工作线程独占持有运行时，宿主侧经通道投递命令
//!
//! 目录布局（对齐 Xime 的 filesDir 布局）：
//! - `%APPDATA%\Xime\rime`           rime 用户数据（单目录模型）
//! - `%APPDATA%\Xime\plugins\<id>`   已安装插件
//! - `%APPDATA%\Xime\plugins\registry.yaml`
//! - `%APPDATA%\Xime\plugins\config\<id>.yaml` 插件配置（host.config 存取）

use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use tracing::{info, trace, warn};
use xime_plugin::{PluginManager, PluginManifest, PluginRecordState, PluginRuntime, RemoteBackupEntry};
use xime_p2p::Event;

use crate::clipboard_p2p::{p2p_dir_for, P2pSync};
use crate::hashing::sha256_hex;

/// 备份恢复的硬上限（审计 F3）：备份包由远端插件提供，必须假设它可能是 zip 炸弹。
/// 预扫描看中心目录声明的大小，解压时按**实际写入字节**再复核一遍（声明值可以撒谎）。
const MAX_RESTORE_ENTRIES: usize = 20_000;
const MAX_RESTORE_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_RESTORE_FILE_BYTES: u64 = 128 * 1024 * 1024;

/// 恢复限额。默认值即线上上限；单测注入小值跑「超限」分支，不必造两万个条目。
#[derive(Clone, Copy)]
struct RestoreLimits {
    entries: usize,
    total_bytes: u64,
    file_bytes: u64,
}

impl Default for RestoreLimits {
    fn default() -> Self {
        Self {
            entries: MAX_RESTORE_ENTRIES,
            total_bytes: MAX_RESTORE_TOTAL_BYTES,
            file_bytes: MAX_RESTORE_FILE_BYTES,
        }
    }
}

/// 已启用插件的元数据快照（纯数据，可跨线程；运行时按需在调用线程内加载）。
#[derive(Clone)]
struct EnabledPlugin {
    id: String,
    plugin_type: String,
    dir: PathBuf,
    entry: String,
    config: PathBuf,
}

/// 剪贴板同步工作线程的命令。
enum ClipboardCommand {
    /// 本地剪贴板变化（已读出的文本）。
    LocalChanged(String),
    /// 定时拉取节拍。
    PollTick,
    /// 停止工作线程（插件被停用或选型变更，由 reload 触发重建）。
    Shutdown,
}

pub struct PluginHost {
    manager: PluginManager,
    /// 已启用插件的元数据快照（启动时扫描；启停变更重启 server 后生效，
    /// 与旧版"启动时加载全部运行时"行为一致）。
    enabled: Mutex<Vec<EnabledPlugin>>,
    /// rime 用户数据目录（单目录模型）：备份打包/恢复的对象。
    rime_dir: PathBuf,
    /// 剪贴板同步工作线程的信箱（未启用剪贴板同步插件时为 None）。
    clipboard_tx: Mutex<Option<mpsc::Sender<ClipboardCommand>>>,
    /// 正在运行的剪贴板工作线程所加载的同步插件 id（None = 无插件）。
    clipboard_plugin: Mutex<Option<String>>,
    /// 剪贴板工作线程是否已启动（线程常驻，历史记录不依赖同步插件）。
    worker_started: AtomicBool,
}

impl PluginHost {
    /// 创建插件宿主：安装内置插件并扫描已启用插件。
    /// `bundled_dir` 为安装目录自带的插件源（`resources/plugins`），缺失时跳过安装。
    pub fn new(rime_dir: PathBuf, bundled_dir: Option<PathBuf>) -> Arc<Self> {
        let plugins_root = rime_dir
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("plugins");
        let manager = PluginManager::new(&plugins_root);

        if let Some(bundled) = bundled_dir {
            install_bundled(&manager, &bundled);
        }

        let host = Arc::new(Self {
            manager,
            enabled: Mutex::new(Vec::new()),
            rime_dir,
            clipboard_tx: Mutex::new(None),
            clipboard_plugin: Mutex::new(None),
            worker_started: AtomicBool::new(false),
        });
        host.scan_enabled();
        host.sync_clipboard_worker();
        host
    }

    /// rime 用户数据目录（单目录模型）。IPC 处理器里做方案词表 / 快捷短语的
    /// 文件读写用它拿目录——**不要**各自重算路径**：debug 构建里引擎用的是
    /// `target/debug/user-data`，重算会指到 `%APPDATA%` 去（历史上踩过）。
    pub fn rime_dir(&self) -> &Path {
        &self.rime_dir
    }

    /// 扫描已启用插件，缓存元数据（目录 / 入口 / 配置文件路径）。
    fn scan_enabled(&self) {
        let mut list = self.enabled.lock().unwrap_or_else(|e| e.into_inner());
        list.clear();
        for record in self.manager.list() {
            if !record.enabled || record.state != PluginRecordState::Ready {
                continue;
            }
            let dir = self.manager.plugin_dir(&record.id);
            let manifest = match PluginManifest::from_dir(&dir) {
                Ok(m) => m,
                Err(e) => {
                    warn!("插件 manifest 读取失败 {}: {}", record.id, e);
                    continue;
                }
            };
            list.push(EnabledPlugin {
                id: record.id.clone(),
                plugin_type: manifest.plugin_type.clone(),
                entry: manifest.entry.clone(),
                config: self.manager.config_path(&record.id),
                dir,
            });
            info!(
                "插件已就绪: {} v{} (type={})",
                record.id, record.version, manifest.plugin_type
            );
        }
    }
}

/// 云备份：宿主负责打包 rime 用户数据，传输交给 `backup` 类插件。
///
/// 实现完整（含单测），但 **尚未接 IPC 命令和设置页入口**，`cargo build`
/// 会把它当死代码。等接线时删掉这个 `allow`，别让它长期挂着。
#[allow(dead_code)]
impl PluginHost {
    // ---- 云备份（宿主打包，插件传输）----

    /// 打包 rime 用户数据为 zip（跳过可再生的 build/ 目录）。
    /// 条目前缀 `rime/`，与 Xime 的备份包布局一致（恢复时去掉前缀落回 rime 目录）。
    pub fn build_backup_archive(&self) -> Result<(String, Vec<u8>), String> {
        let file_name = format!("Xime备份-{}.zip", Self::local_date_string());

        let file = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        Self::add_dir_to_zip(&mut zip, &self.rime_dir, "rime", options)?;

        let cursor = zip
            .finish()
            .map_err(|e| format!("zip 收尾失败: {}", e))?;
        Ok((file_name, cursor.into_inner()))
    }

    /// 立即备份：打包 → 找 backup 类插件推送。返回远端条目 id。
    pub fn backup_now(&self) -> Result<String, String> {
        let (name, archive) = self.build_backup_archive()?;
        let result = self
            .with_typed_runtime("backup", |runtime| runtime.push_backup(&name, &archive))
            .ok_or_else(|| "未安装已启用的 backup 类插件".to_string())?;
        if result.ok {
            Ok(result.id.unwrap_or_default())
        } else {
            Err(format!(
                "插件返回失败: {}",
                result.message.unwrap_or_else(|| "未知错误".to_string())
            ))
        }
    }

    /// 列出远端备份条目。
    pub fn list_backups(&self) -> Result<Vec<RemoteBackupEntry>, String> {
        self.with_typed_runtime("backup", |runtime| runtime.list_backups())
            .ok_or_else(|| "未安装已启用的 backup 类插件".to_string())?
            .ok_or_else(|| "获取远端备份列表失败".to_string())
    }

    /// 拉取并恢复备份：`_xime_backup/` 元数据条目跳过（设置/插件配置恢复属后续功能点），
    /// 其余条目按 zip 相对路径写回 rime 目录（enclosed_name 防路径穿越）。
    /// 返回恢复的文件数。
    pub fn restore_backup(&self, id: &str) -> Result<usize, String> {
        let bytes = self
            .with_typed_runtime("backup", |runtime| runtime.pull_backup(id))
            .ok_or_else(|| "未安装已启用的 backup 类插件".to_string())?
            .ok_or_else(|| "远端备份不存在或拉取失败".to_string())?;
        self.restore_archive(&bytes, RestoreLimits::default())
    }

    /// 恢复的实质部分，与插件拉取解耦，便于单测（审计 F3）。
    ///
    /// 四步：① 预扫描条目数与声明总大小；② 解压到 rime 目录**旁边**的临时目录，
    /// 边写边复核实际字节（单文件 + 合计）；③ 全部落盘成功后才逐个 rename 覆盖目标，
    /// 被覆盖的旧文件先挪进回滚目录；④ 任一步失败就回滚并清掉临时目录。
    /// 这样坏包/超限包在碰用户数据之前就被拒绝，替换阶段出错也能把旧文件放回去。
    fn restore_archive(&self, bytes: &[u8], limits: RestoreLimits) -> Result<usize, String> {
        let staging = self.side_dir("restore");
        std::fs::create_dir_all(&staging)
            .map_err(|e| format!("创建临时目录 {} 失败: {e}", staging.display()))?;
        let staged = match self.extract_to_staging(bytes, &staging, limits) {
            Ok(staged) => staged,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        let applied = self.apply_staged(&staged);
        if let Err(e) = std::fs::remove_dir_all(&staging) {
            warn!("清理临时目录 {} 失败: {e}", staging.display());
        }
        match applied {
            Ok(restored) => {
                info!("备份恢复完成: {restored} 个文件");
                Ok(restored)
            }
            Err(e) => Err(e),
        }
    }

    /// 解压到暂存目录，返回 (暂存文件绝对路径, 相对 rime 目录的目标路径)。
    /// 只在暂存目录里动，写不进去也不影响用户数据。
    fn extract_to_staging(
        &self,
        bytes: &[u8],
        staging: &Path,
        limits: RestoreLimits,
    ) -> Result<Vec<(PathBuf, PathBuf)>, String> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| format!("zip 打开失败: {e}"))?;
        if archive.len() > limits.entries {
            return Err(format!(
                "备份包含 {} 个条目，超过上限 {}",
                archive.len(),
                limits.entries
            ));
        }
        let mut declared = 0u64;
        for i in 0..archive.len() {
            let entry = archive
                .by_index(i)
                .map_err(|e| format!("zip 条目读取失败: {e}"))?;
            declared = declared.saturating_add(entry.size());
            if declared > limits.total_bytes {
                return Err(format!(
                    "备份声明解压后超过上限 {} 字节",
                    limits.total_bytes
                ));
            }
        }

        let mut staged: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(archive.len());
        let mut total = 0u64;
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| format!("zip 条目读取失败: {e}"))?;
            let Some(rel) = entry.enclosed_name() else {
                // 绝对路径、`..`、非法字符：跳过而不是报错，一个坏条目不该毁掉整包。
                warn!("跳过包内非法路径条目: {}", entry.name());
                continue;
            };
            // 包内路径在不同平台上可能是 `/` 或 `\`，按组件比而不是比字符串前缀。
            let is_metadata = matches!(
                rel.components().next(),
                Some(std::path::Component::Normal(part)) if part == "_xime_backup"
            );
            if is_metadata {
                continue;
            }
            // 去掉打包时的 rime/ 前缀
            let target_rel = rel.strip_prefix("rime").unwrap_or(&rel).to_path_buf();
            if target_rel.as_os_str().is_empty() {
                continue;
            }
            let staged_path = staging.join(&target_rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&staged_path)
                    .map_err(|e| format!("创建 {} 失败: {e}", staged_path.display()))?;
                continue;
            }
            if let Some(parent) = staged_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
            }
            let mut out = std::fs::File::create(&staged_path)
                .map_err(|e| format!("写入 {} 失败: {e}", staged_path.display()))?;
            copy_capped(&mut entry, &mut out, &mut total, limits)
                .map_err(|e| format!("解压 {} 失败: {e}", target_rel.display()))?;
            staged.push((staged_path, target_rel));
        }
        if staged.is_empty() {
            return Err("备份包里没有可恢复的文件".to_string());
        }
        Ok(staged)
    }

    /// 把暂存文件逐个落到 rime 目录；被覆盖的旧文件先挪进回滚目录，失败时整体倒回。
    fn apply_staged(&self, staged: &[(PathBuf, PathBuf)]) -> Result<usize, String> {
        let rollback_root = self.side_dir("restore-old");
        std::fs::create_dir_all(&rollback_root)
            .map_err(|e| format!("创建回滚目录 {} 失败: {e}", rollback_root.display()))?;
        let mut replaced: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut created: Vec<PathBuf> = Vec::new();
        for (src, rel) in staged {
            let dest = self.rime_dir.join(rel);
            if let Some(parent) = dest.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    undo_apply(&replaced, &created);
                    let _ = std::fs::remove_dir_all(&rollback_root);
                    return Err(format!("创建目录 {} 失败: {e}", parent.display()));
                }
            }
            if dest.exists() {
                let old = rollback_root.join(rel);
                if let Some(parent) = old.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        undo_apply(&replaced, &created);
                        let _ = std::fs::remove_dir_all(&rollback_root);
                        return Err(format!("创建回滚目录 {} 失败: {e}", parent.display()));
                    }
                }
                if let Err(e) = std::fs::rename(&dest, &old) {
                    undo_apply(&replaced, &created);
                    let _ = std::fs::remove_dir_all(&rollback_root);
                    return Err(format!("暂存原文件 {} 失败: {e}", dest.display()));
                }
                replaced.push((dest.clone(), old));
            } else {
                created.push(dest.clone());
            }
            if let Err(e) = std::fs::rename(src, &dest) {
                undo_apply(&replaced, &created);
                let _ = std::fs::remove_dir_all(&rollback_root);
                return Err(format!("写入 {} 失败: {e}", dest.display()));
            }
        }
        if let Err(e) = std::fs::remove_dir_all(&rollback_root) {
            // 恢复本身成功；旧文件留在磁盘上只是垃圾，不该让整次恢复失败。
            warn!("清理回滚目录 {} 失败: {e}", rollback_root.display());
        }
        Ok(staged.len())
    }

    /// rime 目录旁边的临时目录名（同一卷，rename 才可能是原子的）。
    fn side_dir(&self, tag: &str) -> PathBuf {
        let base = self
            .rime_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.rime_dir.clone());
        let pid = std::process::id();
        let mut n = 0u32;
        loop {
            let dir = base.join(format!(".xime-{tag}-{pid}-{n}"));
            if !dir.exists() {
                return dir;
            }
            n = n.saturating_add(1);
            if n >= 1000 {
                // 冲突到这个程度就交给 create_dir_all 报错，别无限转。
                return dir;
            }
        }
    }

    /// 删除远端备份条目。
    pub fn delete_backup(&self, id: &str) -> Result<bool, String> {
        self.with_typed_runtime("backup", |runtime| runtime.delete_backup(id))
            .ok_or_else(|| "未安装已启用的 backup 类插件".to_string())
    }

    /// 递归把目录写进 zip（`prefix` 为包内路径前缀）。
    fn add_dir_to_zip<W: std::io::Write + Seek>(
        zip: &mut zip::ZipWriter<W>,
        dir: &Path,
        prefix: &str,
        options: zip::write::SimpleFileOptions,
    ) -> Result<(), String> {
        let entries =
            std::fs::read_dir(dir).map_err(|e| format!("读取 {} 失败: {}", dir.display(), e))?;
        for entry in entries.flatten() {
            let name = format!("{}/{}", prefix, entry.file_name().to_string_lossy());
            match entry.file_type() {
                Ok(t) if t.is_dir() => {
                    // build/ 为部署产物，可再生，不进备份包
                    if entry.file_name() == "build" {
                        continue;
                    }
                    Self::add_dir_to_zip(zip, &entry.path(), &name, options)?;
                }
                Ok(t) if t.is_file() => {
                    let bytes = std::fs::read(entry.path())
                        .map_err(|e| format!("读取 {} 失败: {}", entry.path().display(), e))?;
                    zip.start_file(&name, options)
                        .map_err(|e| format!("zip 写入 {} 失败: {}", name, e))?;
                    zip.write_all(&bytes)
                        .map_err(|e| format!("zip 写入 {} 失败: {}", name, e))?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn unix_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// 本地日期 YYYY-MM-DD（civil_from_days，无外部时间依赖）。
    fn local_date_string() -> String {
        let days = (Self::unix_secs() / 86400) as i64;
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!("{:04}-{:02}-{:02}", y, m, d)
    }
}

impl PluginHost {
    // ---- 剪贴板同步（宿主监听+去重，插件传输）----

    /// 当前是否选中了可用的剪贴板同步插件（目前只有单测在用）。
    #[cfg(test)]
    pub fn has_clipboard_sync(&self) -> bool {
        let selection = self.clipboard_selection();
        self.resolve_clipboard_plugin(&selection).is_some()
    }

    /// 重新扫描插件并对齐剪贴板工作线程状态（设置程序启停/选择插件后经 IPC 触发）。
    pub fn reload(self: &Arc<Self>) {
        self.scan_enabled();
        self.sync_clipboard_worker();
    }

    /// 本地剪贴板变化：转交剪贴板同步线程（hash 去重后经插件推送到远端）。
    pub fn clipboard_local_changed(&self, text: &str) {
        trace!("剪贴板变化进入宿主: {} 字符", text.len());
        self.send_clipboard(ClipboardCommand::LocalChanged(text.to_string()));
    }

    /// 定时拉取节拍：远端有新内容时由剪贴板线程直接写回系统剪贴板。
    /// 顺带对齐一次选型：设置程序写 clipboard_sync.toml 后无需重启即生效
    /// （与 IPC ReloadPlugins 互为兜底，防通知丢失/跨进程时序问题）。
    pub fn clipboard_poll_remote(self: &Arc<Self>) {
        self.sync_clipboard_worker();
        self.send_clipboard(ClipboardCommand::PollTick);
    }

    // ---- 内部 ----

    /// 解析剪贴板同步应使用的插件。
    /// 选型来源 clipboard_sync.toml（设置程序写入，与 Android daemon 契约一致）：
    /// enabled=false 或无候选 → None；plugin_id 为空时回退第一个已启用的
    /// clipboard_sync 插件。
    fn clipboard_selection(&self) -> ClipboardSyncSelection {
        let root = self.rime_dir.parent().unwrap_or_else(|| Path::new("."));
        read_clipboard_sync_selection(root)
    }

    fn resolve_clipboard_plugin(
        &self,
        selection: &ClipboardSyncSelection,
    ) -> Option<EnabledPlugin> {
        if !selection.wants_plugin() {
            return None;
        }
        let enabled = self.enabled.lock().unwrap_or_else(|e| e.into_inner());
        if selection.plugin_id.is_empty() {
            enabled
                .iter()
                .find(|p| p.plugin_type == "clipboard_sync")
                .cloned()
        } else {
            enabled
                .iter()
                .find(|p| p.plugin_type == "clipboard_sync" && p.id == selection.plugin_id)
                .cloned()
        }
    }

    /// 在调用线程内加载指定类型插件的运行时并执行 `f`（一操作一实例，onLoad 幂等）。
    /// PluginRuntime 非 Send（QuickJS 裸指针），不能跨线程缓存复用。
    fn with_typed_runtime<T>(
        &self,
        plugin_type: &str,
        f: impl FnOnce(&PluginRuntime) -> T,
    ) -> Option<T> {
        let plugin = {
            let enabled = self.enabled.lock().unwrap_or_else(|e| e.into_inner());
            enabled
                .iter()
                .find(|p| p.plugin_type == plugin_type)
                .cloned()?
        };
        let runtime = match PluginRuntime::load(&plugin.dir, &plugin.entry, &plugin.config) {
            Ok(rt) => rt,
            Err(e) => {
                warn!("插件运行时加载失败 {}: {}", plugin.id, e);
                return None;
            }
        };
        runtime.call_on_load();
        Some(f(&runtime))
    }

    fn send_clipboard(&self, cmd: ClipboardCommand) {
        let tx = self.clipboard_tx.lock().unwrap_or_else(|e| e.into_inner());
        match tx.as_ref() {
            Some(tx) => {
                let _ = tx.send(cmd);
            }
            None => warn!("剪贴板工作线程未运行，事件丢弃: {:?}", match cmd {
                ClipboardCommand::LocalChanged(_) => "LocalChanged",
                ClipboardCommand::PollTick => "PollTick",
                ClipboardCommand::Shutdown => "Shutdown",
            }),
        }
    }

    /// 依据 clipboard_sync.toml 选型对齐剪贴板工作线程：
    /// 线程常驻（剪贴板历史记录不依赖同步通道）；仅在选型变更时停旧起新。
    fn sync_clipboard_worker(self: &Arc<Self>) {
        let selection = self.clipboard_selection();
        let plugin_id = self
            .resolve_clipboard_plugin(&selection)
            .map(|p| p.id)
            .unwrap_or_default();
        let desired = worker_key(&plugin_id, selection.wants_p2p());
        let started = self.worker_started.load(Ordering::Acquire);
        let current = self
            .clipboard_plugin
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if started && current.as_deref() == Some(desired.as_str()) {
            return;
        }
        if started {
            let tx = self.clipboard_tx.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(tx) = tx.as_ref() {
                let _ = tx.send(ClipboardCommand::Shutdown);
            }
        }
        self.start_clipboard_worker();
    }

    /// 启动剪贴板工作线程（常驻；按选型持有可选的同步插件运行时与 P2P 节点）。
    fn start_clipboard_worker(self: &Arc<Self>) {
        let selection = self.clipboard_selection();
        let plugin = self.resolve_clipboard_plugin(&selection);
        let plugin_id = plugin.as_ref().map(|p| p.id.clone()).unwrap_or_default();
        let p2p_dir = selection.wants_p2p().then(|| p2p_dir_for(&self.rime_dir));
        let key = worker_key(&plugin_id, p2p_dir.is_some());
        let db_path = xime_config::clipboard_store::default_db_path();
        let (tx, rx) = mpsc::channel::<ClipboardCommand>();
        std::thread::spawn(move || run_clipboard_worker(plugin, p2p_dir, db_path, rx));
        *self
            .clipboard_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(tx);
        *self
            .clipboard_plugin
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(key);
        self.worker_started.store(true, Ordering::Release);
    }
}

/// 工作线程的「选型键」：插件 id + 是否走局域网直连。
///
/// 任一项变化都要重建线程——插件运行时和 P2P 节点都归那个线程独占。
fn worker_key(plugin_id: &str, p2p: bool) -> String {
    format!("{plugin_id}|{}", if p2p { "p2p" } else { "" })
}

/// 剪贴板工作线程（常驻）：始终记录剪贴板历史；按选型持有同步插件运行时
/// 和/或局域网直连节点。
/// 三通道去重状态（当前内容 / 上次推送 / 自写回显）归本线程所有，无需加锁。
fn run_clipboard_worker(
    plugin: Option<EnabledPlugin>,
    p2p_dir: Option<PathBuf>,
    db_path: PathBuf,
    rx: mpsc::Receiver<ClipboardCommand>,
) {
    xime_config::clipboard_store::migrate_legacy(
        &db_path,
        &db_path.with_file_name("clipboard_history.json"),
        &db_path.with_file_name("quick_send.yaml"),
    );
    let plugin_id = plugin.as_ref().map(|p| p.id.clone()).unwrap_or_default();
    let runtime = plugin.and_then(|p| {
        match PluginRuntime::load(&p.dir, &p.entry, &p.config) {
            Ok(rt) => {
                rt.call_on_load();
                info!("剪贴板同步插件已加载: {}", p.id);
                Some(rt)
            }
            Err(e) => {
                warn!("剪贴板同步插件加载失败 {}: {}", p.id, e);
                None
            }
        }
    });

    // 局域网直连节点也归本线程：Node 自带后台运行时，这里只做投递与轮询。
    let p2p = p2p_dir.and_then(|dir| match P2pSync::start(&dir) {
        Ok(sync) => Some(sync),
        Err(e) => {
            warn!("局域网直连未启动（{}）：{}", dir.display(), e);
            None
        }
    });

    info!(
        "剪贴板工作线程已启动: db={} 同步插件={} 局域网直连={}",
        db_path.display(),
        if plugin_id.is_empty() { "未启用" } else { &plugin_id },
        if p2p.is_some() { "已启用" } else { "未启用" }
    );
    let mut current: Option<String> = None;
    let mut last_pushed: Option<String> = None;
    let mut self_written: Option<String> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            ClipboardCommand::LocalChanged(text) => {
                // 剪贴板变化是高频事件：worker 内部流转走 trace（U-10），
                // 只保留「启动/停止/失败」这类结果级 info。
                trace!("worker 收到剪贴板变化: {} 字符", text.len());
                let hash = sha256_hex(text.as_bytes());
                if current.as_deref() == Some(hash.as_str()) {
                    continue;
                }
                if Some(&hash) == last_pushed.as_ref() || Some(&hash) == self_written.as_ref() {
                    continue;
                }
                current = Some(hash.clone());
                // 历史持久化（SQLite，与 Android clipboard.db 同构；超长截断）
                record_history(&db_path, &text);
                if let Some(sync) = &p2p {
                    if sync.push_text(&text) {
                        trace!("局域网直连已投递 ({} 字符)", text.chars().count());
                    }
                }
                let profile = serde_json::json!({
                    "type": "text",
                    "hash": hash,
                    "text": text,
                    "has_data": false,
                    "data_name": null,
                    "size": text.len(),
                    "source": "xime-windows",
                });
                if let Some(rt) = &runtime {
                    if rt.clipboard_push(&profile) {
                        last_pushed = Some(hash);
                        trace!("剪贴板已推送 ({} 字符)", text.len());
                    } else {
                        warn!(
                            "剪贴板推送失败: 同步插件={} ({} 字符)",
                            if plugin_id.is_empty() { "未启用" } else { &plugin_id },
                            text.len()
                        );
                    }
                }
            }
            ClipboardCommand::PollTick => {
                if let Some(sync) = &p2p {
                    while let Some(event) = sync.poll() {
                        handle_p2p_event(event, &db_path, &mut current, &mut self_written);
                    }
                }
                let Some(runtime) = &runtime else {
                    continue; // 未启用同步插件，无事可拉
                };
                let Some(profile) = runtime.clipboard_pull() else {
                    continue;
                };
                if profile
                    .get("has_data")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    // 图片附件链路（落盘 + 入历史 + 写回图片）桌面端尚未实现，
                    // 显式留痕避免"拉了但什么都没发生"的无声跳过
                    info!("远端剪贴板为图片附件，桌面端暂未支持图片同步，跳过");
                    continue;
                }
                let Some(text) = profile
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                if text.is_empty() {
                    continue;
                }
                // hash 留空（旧版纯文本）时由宿主补算
                let hash = profile
                    .get("hash")
                    .and_then(|v| v.as_str())
                    .filter(|h| !h.is_empty())
                    .map(String::from)
                    .unwrap_or_else(|| sha256_hex(text.as_bytes()));
                if Some(&hash) == current.as_ref() || Some(&hash) == self_written.as_ref() {
                    continue;
                }
                self_written = Some(hash.clone());
                current = Some(sha256_hex(text.as_bytes()));
                // 拉取内容与本地复制同权进历史（Android 语义：远端内容同样出现在
                // 剪贴板面板）；写回触发的本地回声已被 self_written 抑制，不会重复
                record_history(&db_path, &text);
                if crate::clipboard::write_text(&text) {
                    trace!("远端剪贴板已写回本地 ({} 字符)", text.len());
                }
            }
            ClipboardCommand::Shutdown => {
                info!("剪贴板工作线程已停止: {}", plugin_id);
                break;
            }
        }
    }
}

/// 处理一条局域网直连事件：远端文本入历史并写回系统剪贴板。
///
/// 去重沿用工作线程的三态：`current` 挡「本地刚复制过的同一内容」，
/// `self_written` 挡「自己刚推出去、又被对端转回来的回声」。
fn handle_p2p_event(
    event: Event,
    db_path: &Path,
    current: &mut Option<String>,
    self_written: &mut Option<String>,
) {
    match event {
        Event::Clipboard(profile) => {
            if profile.has_data {
                info!("远端剪贴板为图片附件，桌面端暂未支持图片同步，跳过");
                return;
            }
            if profile.text.is_empty() {
                return;
            }
            let text = profile.text;
            // hash 留空（旧版纯文本）时由宿主补算
            let hash = if profile.hash.is_empty() {
                sha256_hex(text.as_bytes())
            } else {
                profile.hash
            };
            if current.as_deref() == Some(hash.as_str())
                || self_written.as_deref() == Some(hash.as_str())
            {
                return;
            }
            *self_written = Some(hash);
            *current = Some(sha256_hex(text.as_bytes()));
            record_history(db_path, &text);
            if crate::clipboard::write_text(&text) {
                trace!("局域网直连收到远端剪贴板 ({} 字符)", text.chars().count());
            } else {
                warn!("局域网直连收到远端剪贴板，但写回系统剪贴板失败");
            }
        }
        Event::Peer {
            did,
            name,
            connected,
        } => info!(
            "局域网直连设备{}：{}（{}）",
            if connected { "已连接" } else { "已断开" },
            name,
            did
        ),
        Event::Warning(w) => warn!("局域网直连：{}", w),
    }
}

/// 历史持久化（SQLite，与 Android clipboard.db 同构；超长截断）。
/// 本地复制与远端拉取共用：拉取写回触发的本地回声已被 self_written 抑制，
/// 不会经由 LocalChanged 重复记录。
fn record_history(db_path: &Path, text: &str) {
    let mut stored = text.to_string();
    if stored.chars().count() > CLIPBOARD_HISTORY_TEXT_MAX {
        stored = stored.chars().take(CLIPBOARD_HISTORY_TEXT_MAX).collect();
    }
    if let Err(e) =
        xime_config::clipboard_store::append_history(db_path, &stored, CLIPBOARD_HISTORY_CAP)
    {
        warn!("剪贴板历史写入失败: {}", e);
    } else {
        trace!("剪贴板历史已记录");
    }
}

/// 剪贴板同步选型（设置程序写 clipboard_sync.toml，与 Android daemon 契约共享）。
#[derive(Default, serde::Deserialize)]
struct ClipboardSyncSelection {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    plugin_id: String,
    /// 通道列表，首个优先：`"p2p"` = 局域网直连，其它值是剪贴板同步插件 id。
    /// 旧配置没有这个字段，按「有插件就同步」的老语义处理。
    #[serde(default)]
    channels: Vec<String>,
}

impl ClipboardSyncSelection {
    /// 是否走局域网直连。
    fn wants_p2p(&self) -> bool {
        self.enabled && self.channels.iter().any(|c| c == "p2p")
    }

    /// 是否走同步插件。空 channels 视为旧配置，沿用原来的插件语义。
    fn wants_plugin(&self) -> bool {
        self.enabled && (self.channels.is_empty() || self.channels.iter().any(|c| c != "p2p"))
    }
}

fn read_clipboard_sync_selection(root: &Path) -> ClipboardSyncSelection {
    std::fs::read_to_string(root.join("clipboard_sync.toml"))
        .ok()
        .and_then(|content| toml::from_str(&content).ok())
        .unwrap_or_default()
}

// ---- 剪贴板历史（SQLite：xime_config::clipboard_store，与 Android clipboard.db 同构）----

/// 历史容量与单条截断（数据库与 UI 体积保护）。
const CLIPBOARD_HISTORY_CAP: usize = 50;
const CLIPBOARD_HISTORY_TEXT_MAX: usize = 2000;

/// 插件包下载临时文件路径（对齐安卓 `cache/xime_plugin_{id}_{fileName}` 约定：
/// 下载 → sha256 校验 → `PluginManager::install_from_zip` → 临时文件即删）。
/// 插件市场下载属后续功能点，先固化路径约定。
#[allow(dead_code)]
pub fn plugin_download_temp_path(plugin_id: &str, file_name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("xime_plugin_{}_{}", plugin_id, file_name))
}

/// 安装安装目录自带的内置插件（resources/plugins/<目录>），覆盖安装但保留启用状态。
fn install_bundled(manager: &PluginManager, bundled_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(bundled_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        match manager.install_from_dir(&entry.path(), true) {
            Ok(record) => info!("内置插件已安装: {} v{}", record.id, record.version),
            Err(e) => warn!("内置插件安装失败 {}: {}", entry.path().display(), e),
        }
    }
}

/// 带双重上限的流式拷贝：单文件上限挡大文件，合计上限挡「很多中等文件」的炸弹。
///
/// `total` 失败时已计入部分字节——调用方随后整体丢弃暂存目录，不需要精确值。
fn copy_capped<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
    total: &mut u64,
    limits: RestoreLimits,
) -> Result<(), String> {
    let mut buf = [0u8; 64 * 1024];
    let mut file_bytes = 0u64;
    loop {
        let n = src.read(&mut buf).map_err(|e| format!("读取失败: {e}"))?;
        if n == 0 {
            break;
        }
        let n = n as u64;
        file_bytes = file_bytes.saturating_add(n);
        *total = total.saturating_add(n);
        if file_bytes > limits.file_bytes {
            return Err(format!("单文件超过上限 {} 字节", limits.file_bytes));
        }
        if *total > limits.total_bytes {
            return Err(format!("解压合计超过上限 {} 字节", limits.total_bytes));
        }
        let n = usize::try_from(n).map_err(|_| "块长度溢出".to_string())?;
        dst.write_all(&buf[..n])
            .map_err(|e| format!("写出失败: {e}"))?;
    }
    dst.flush().map_err(|e| format!("写出失败: {e}"))
}

/// 把 `apply_staged` 已做的改动倒回去：新写入的删掉、被挪走的旧文件挪回原位。
///
/// 尽力而为：回滚时的错误只能记日志，真正要报给用户的是最初那个失败原因。
fn undo_apply(replaced: &[(PathBuf, PathBuf)], created: &[PathBuf]) {
    for dest in created.iter().rev() {
        if let Err(e) = std::fs::remove_file(dest) {
            warn!("回滚删除 {} 失败: {e}", dest.display());
        }
    }
    for (dest, old) in replaced.iter().rev() {
        let _ = std::fs::remove_file(dest);
        if let Err(e) = std::fs::rename(old, dest) {
            warn!(
                "回滚还原 {} 失败: {e}（原文件在 {}）",
                dest.display(),
                old.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_selection_channels_decide_transport() {
        let p2p: ClipboardSyncSelection =
            toml::from_str("enabled = true\nchannels = [\"p2p\"]").expect("解析 p2p 选型");
        assert!(p2p.wants_p2p());
        assert!(!p2p.wants_plugin());

        let auto: ClipboardSyncSelection =
            toml::from_str("enabled = true\nchannels = [\"p2p\", \"webdav\"]")
                .expect("解析自动选型");
        assert!(auto.wants_p2p());
        assert!(auto.wants_plugin());

        let plugin: ClipboardSyncSelection =
            toml::from_str("enabled = true\nchannels = [\"webdav\"]").expect("解析插件选型");
        assert!(!plugin.wants_p2p());
        assert!(plugin.wants_plugin());

        // 旧配置没有 channels：沿用「有插件就同步」
        let legacy: ClipboardSyncSelection =
            toml::from_str("enabled = true\nplugin_id = \"webdav\"").expect("解析旧配置");
        assert!(!legacy.wants_p2p());
        assert!(legacy.wants_plugin());

        let off: ClipboardSyncSelection =
            toml::from_str("enabled = false\nchannels = [\"p2p\"]").expect("解析关闭配置");
        assert!(!off.wants_p2p());
        assert!(!off.wants_plugin());
    }

    #[test]
    fn worker_key_changes_when_transport_changes() {
        assert_eq!(worker_key("webdav", false), "webdav|");
        assert_eq!(worker_key("webdav", true), "webdav|p2p");
        assert_ne!(worker_key("webdav", false), worker_key("webdav", true));
    }

    fn temp_root(label: &str) -> PathBuf {
        crate::test_support::test_root(&format!("plugins_{label}"))
    }

    fn write_minimal_plugin(root: &Path, id: &str, plugin_type: &str) {
        let dir = root.join("plugins").join(id);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        std::fs::write(
            dir.join("manifest.yaml"),
            format!("id: {id}\nname: {id}\nversion: 1.0.0\ntype: {plugin_type}\n"),
        )
        .unwrap_or_default();
        std::fs::write(
            dir.join("main.js"),
            r#"(function () {
  globalThis.plugin = {
    onLoad: function () { return true; },
    backup: {
      push: function (args) { return { ok: true, id: "test-id" }; },
      list: function () { return []; },
      pull: function (id) { return null; },
      remove: function (id) { return true; },
    },
  };
})();
"#,
        )
        .unwrap_or_default();
        // registry 由 PluginManager 维护，这里手工写一份（与安装结果同构）
        let registry = format!(
            "plugins:\n  - id: {id}\n    name: {id}\n    version: 1.0.0\n    type: {plugin_type}\n    enabled: true\n    installedAt: \"2026-01-01\"\n"
        );
        std::fs::write(root.join("plugins").join("registry.yaml"), registry).unwrap_or_default();
    }

    /// clipboard_sync.toml 选型：默认不启用；enabled+plugin_id 选中；enabled=false 停用。
    #[test]
    fn clipboard_selection_follows_clipboard_sync_toml() {
        let root = temp_root("clip_toml");
        write_minimal_plugin(&root, "test.clip", "clipboard_sync");
        std::fs::create_dir_all(root.join("rime")).unwrap_or_default();
        let host = PluginHost::new(root.join("rime"), None);
        assert!(!host.has_clipboard_sync(), "默认（无 toml）不启用");
        std::fs::write(
            root.join("clipboard_sync.toml"),
            "enabled = true
plugin_id = \"test.clip\"
",
        )
        .unwrap_or_default();
        assert!(host.has_clipboard_sync());
        std::fs::write(root.join("clipboard_sync.toml"), "enabled = false
")
            .unwrap_or_default();
        assert!(!host.has_clipboard_sync());
    }

    /// 历史记录不依赖同步插件：worker 常驻，无插件时 LocalChanged 仍落库。
    #[test]
    fn clipboard_worker_records_history_without_sync_plugin() {
        let dir = temp_root("worker_history");
        let db = dir.join("clipboard.db");
        let (tx, rx) = mpsc::channel::<ClipboardCommand>();
        let db_for_worker = db.clone();
        std::thread::spawn(move || run_clipboard_worker(None, None, db_for_worker, rx));
        tx.send(ClipboardCommand::LocalChanged("无插件也记录历史".to_string()))
            .unwrap_or_default();
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if !xime_config::clipboard_store::list_history(&db, 10)
                .unwrap_or_default()
                .is_empty()
            {
                break;
            }
        }
        let items = xime_config::clipboard_store::list_history(&db, 10).unwrap_or_default();
        assert_eq!(items.len(), 1, "无同步插件时历史仍应记录");
        assert_eq!(items[0].text, "无插件也记录历史");
    }

    #[test]
    fn backup_now_uses_typed_plugin_runtime() {
        let root = temp_root("backup_now");
        write_minimal_plugin(&root, "test.backup", "backup");
        std::fs::create_dir_all(root.join("rime")).unwrap_or_default();
        let host = PluginHost::new(root.join("rime"), None);
        let id = host
            .backup_now()
            .unwrap_or_else(|e| format!("ERR:{e}"));
        assert_eq!(id, "test-id");
    }

    #[test]
    fn backup_now_without_plugin_errors() {
        let root = temp_root("backup_empty");
        let host = PluginHost::new(root.join("rime"), None);
        assert!(host.backup_now().is_err());
    }

    #[test]
    fn clipboard_worker_only_for_clipboard_sync_plugin() {
        let root = temp_root("clip_gate");
        write_minimal_plugin(&root, "test.backup", "backup");
        let host = PluginHost::new(root.join("rime"), None);
        assert!(!host.has_clipboard_sync());
    }

    #[test]
    fn bundled_plugin_schema_labels_survive_js_bridge() {
        let dir = std::path::PathBuf::from("../../resources/plugins/webdav-clipboard-sync");
        if !dir.exists() {
            return;
        }
        let manifest = match PluginManifest::from_dir(&dir) {
            Ok(m) => m,
            Err(e) => panic!("manifest 解析失败: {e}"),
        };
        assert_eq!(manifest.name, "WebDAV 剪贴板同步");
        let config = crate::test_support::test_root("plugins_schema").join("plugin_config.yaml");
        let runtime = match PluginRuntime::load(&dir, &manifest.entry, &config) {
            Ok(rt) => rt,
            Err(e) => panic!("插件加载失败: {e}"),
        };
        runtime.call_on_load();
        let fields = runtime.get_settings_schema();
        assert!(fields.len() >= 4, "schema 字段数: {}", fields.len());
        // 乱码排查：完整打印每个字段到达表单的原始内容
        for f in &fields {
            eprintln!(
                "field key={} ftype={} label={:?} placeholder={:?} help={:?} default={:?}",
                f.key, f.ftype, f.label, f.placeholder, f.help_text, f.default_value
            );
        }
        let labels: Vec<&str> = fields.iter().map(|f| f.label.as_str()).collect();
        assert!(
            labels.contains(&"WebDAV 服务器"),
            "schema 标签异常: {labels:?}"        );
        assert!(labels.contains(&"密码"), "schema 标签异常: {labels:?}");
    }

    /// 造一个内存 zip：条目按给定顺序写入（恢复顺序 = 包内顺序）。
    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, options).unwrap_or_default();
            zip.write_all(data).unwrap_or_default();
        }
        zip.finish()
            .map(|c| c.into_inner())
            .unwrap_or_default()
    }

    /// 恢复后：正常文件落位、`_xime_backup/` 元数据跳过、无关文件不动、临时目录清干净。
    #[test]
    fn restore_archive_writes_files_and_skips_metadata() {
        let root = temp_root("restore_ok");
        let rime = root.join("rime");
        std::fs::write(rime.join("untouched.yaml"), "keep").unwrap_or_default();
        let host = PluginHost::new(rime.clone(), None);
        let bytes = zip_bytes(&[
            ("rime/default.yaml", b"name: new"),
            ("_xime_backup/meta.json", b"{}"),
        ]);
        let restored = host
            .restore_archive(&bytes, RestoreLimits::default())
            .unwrap_or_else(|e| panic!("恢复应成功: {e}"));
        assert_eq!(restored, 1, "元数据条目不计入文件数");
        assert_eq!(
            std::fs::read_to_string(rime.join("default.yaml")).unwrap_or_default(),
            "name: new"
        );
        assert_eq!(
            std::fs::read_to_string(rime.join("untouched.yaml")).unwrap_or_default(),
            "keep",
            "包里没有的文件必须原样保留"
        );
        assert!(!root.join("_xime_backup").exists(), "元数据不能落到 rime 目录");
        assert!(leftover_side_dirs(&root).is_empty(), "临时目录要清掉");
    }

    /// 条目数超限：整包拒绝，且一条都不落盘。
    #[test]
    fn restore_rejects_archive_over_entry_limit() {
        let root = temp_root("restore_entries");
        let rime = root.join("rime");
        let host = PluginHost::new(rime.clone(), None);
        let bytes = zip_bytes(&[("rime/a.yaml", b"a"), ("rime/b.yaml", b"b")]);
        let limits = RestoreLimits {
            entries: 1,
            ..RestoreLimits::default()
        };
        let err = host
            .restore_archive(&bytes, limits)
            .expect_err("超限必须拒绝");
        assert!(err.contains("超过上限"), "错误要说清原因: {err}");
        assert!(!rime.join("a.yaml").exists(), "拒绝时不能落盘");
        assert!(leftover_side_dirs(&root).is_empty(), "临时目录要清掉");
    }

    /// 声明总大小超限：预扫描就拒绝，不进解压阶段。
    #[test]
    fn restore_rejects_archive_over_total_bytes() {
        let root = temp_root("restore_total");
        let rime = root.join("rime");
        let host = PluginHost::new(rime.clone(), None);
        let bytes = zip_bytes(&[("rime/big.yaml", &[b'x'; 64][..])]);
        let limits = RestoreLimits {
            total_bytes: 8,
            ..RestoreLimits::default()
        };
        let err = host
            .restore_archive(&bytes, limits)
            .expect_err("超限必须拒绝");
        assert!(err.contains("超过上限"), "错误要说清原因: {err}");
        assert!(!rime.join("big.yaml").exists(), "拒绝时不能落盘");
    }

    /// 单文件超限：解压阶段按实际字节拦下（中心目录的声明值可以撒谎）。
    #[test]
    fn restore_rejects_single_file_over_limit() {
        let root = temp_root("restore_file");
        let rime = root.join("rime");
        let host = PluginHost::new(rime.clone(), None);
        let bytes = zip_bytes(&[("rime/big.yaml", &[b'x'; 64][..])]);
        let limits = RestoreLimits {
            file_bytes: 8,
            total_bytes: 1024,
            ..RestoreLimits::default()
        };
        let err = host
            .restore_archive(&bytes, limits)
            .expect_err("超限必须拒绝");
        assert!(err.contains("上限"), "错误要说清原因: {err}");
        assert!(!rime.join("big.yaml").exists(), "拒绝时不能落盘");
        assert!(leftover_side_dirs(&root).is_empty(), "临时目录要清掉");
    }

    /// 只有元数据、没有可恢复文件的包：报错而不是「成功恢复 0 个」。
    #[test]
    fn restore_rejects_archive_without_files() {
        let root = temp_root("restore_empty");
        let host = PluginHost::new(root.join("rime"), None);
        let bytes = zip_bytes(&[("_xime_backup/meta.json", b"{}")]);
        let err = host
            .restore_archive(&bytes, RestoreLimits::default())
            .expect_err("空包必须报错");
        assert!(err.contains("没有可恢复的文件"), "错误要说清原因: {err}");
    }

    /// 路径穿越条目要跳过，不能写到 rime 目录外面。
    #[test]
    fn restore_skips_path_traversal_entries() {
        let root = temp_root("restore_traversal");
        let rime = root.join("rime");
        let host = PluginHost::new(rime.clone(), None);
        let bytes = zip_bytes(&[("rime/ok.yaml", b"ok"), ("../evil.txt", b"bad")]);
        let restored = host
            .restore_archive(&bytes, RestoreLimits::default())
            .unwrap_or_else(|e| panic!("合法条目应恢复: {e}"));
        assert_eq!(restored, 1);
        assert!(rime.join("ok.yaml").exists());
        assert!(!root.join("evil.txt").exists(), "越界条目不能落盘");
    }

    /// 替换阶段失败要回滚：先换上的文件必须恢复成旧内容。
    ///
    /// 构造：`rime/x` 是个文件，而包里要求写 `rime/x/y.txt`，`create_dir_all` 必然失败；
    /// 包内顺序让 `z.txt` 先被替换，正好考验回滚。
    #[test]
    fn restore_failure_rolls_back_replaced_files() {
        let root = temp_root("restore_rollback");
        let rime = root.join("rime");
        std::fs::write(rime.join("z.txt"), "old-z").unwrap_or_default();
        std::fs::write(rime.join("x"), "blocker").unwrap_or_default();
        let host = PluginHost::new(rime.clone(), None);
        let bytes = zip_bytes(&[("rime/z.txt", b"new-z"), ("rime/x/y.txt", b"child")]);
        let err = host
            .restore_archive(&bytes, RestoreLimits::default())
            .expect_err("目标目录被占必须失败");
        assert!(err.contains("创建目录"), "错误要说清原因: {err}");
        assert_eq!(
            std::fs::read_to_string(rime.join("z.txt")).unwrap_or_default(),
            "old-z",
            "已替换的文件要回滚成旧内容"
        );
        assert_eq!(
            std::fs::read_to_string(rime.join("x")).unwrap_or_default(),
            "blocker",
            "占位文件不能被改动"
        );
        assert!(!rime.join("x").is_dir());
        assert!(leftover_side_dirs(&root).is_empty(), "临时目录要清掉");
    }

    /// 暂存/回滚目录都建在 rime 目录同级，测试里按前缀找残留。
    fn leftover_side_dirs(root: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(root) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".xime-restore"))
            .collect()
    }
}

