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

use std::io::{Cursor, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use tracing::{info, warn};
use xime_plugin::{PluginManager, PluginManifest, PluginRecordState, PluginRuntime, RemoteBackupEntry};

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

    // ---- 云备份（宿主打包，插件传输）----

    /// 打包 rime 用户数据为 zip（跳过可再生的 build/ 目录）。
    /// 条目前缀 `rime/`，与 Xime 的备份包布局一致（恢复时去掉前缀落回 rime 目录）。
    pub fn build_backup_archive(&self) -> Result<(String, Vec<u8>), String> {
        let file_name = format!("Xime备份-{}.zip", local_date_string());

        let file = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        add_dir_to_zip(&mut zip, &self.rime_dir, "rime", options)?;

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

        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("zip 打开失败: {}", e))?;
        let mut restored = 0usize;
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| format!("zip 条目读取失败: {}", e))?;
            let Some(rel) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
                continue;
            };
            let rel_str = rel.to_string_lossy();
            if rel_str.starts_with("_xime_backup/") || rel_str == "_xime_backup" {
                continue;
            }
            // 去掉打包时的 rime/ 前缀
            let target_rel = rel.strip_prefix("rime").unwrap_or(&rel);
            if target_rel.as_os_str().is_empty() {
                continue;
            }
            let dest = self.rime_dir.join(target_rel);
            if entry.is_dir() {
                let _ = std::fs::create_dir_all(&dest);
                continue;
            }
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut out = std::fs::File::create(&dest)
                .map_err(|e| format!("写入 {} 失败: {}", dest.display(), e))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| format!("解压 {} 失败: {}", dest.display(), e))?;
            restored += 1;
        }
        info!("备份恢复完成: {} 个文件", restored);
        Ok(restored)
    }

    /// 删除远端备份条目。
    pub fn delete_backup(&self, id: &str) -> Result<bool, String> {
        self.with_typed_runtime("backup", |runtime| runtime.delete_backup(id))
            .ok_or_else(|| "未安装已启用的 backup 类插件".to_string())
    }

    // ---- 剪贴板同步（宿主监听+去重，插件传输）----

    pub fn has_clipboard_sync(&self) -> bool {
        self.resolve_clipboard_plugin().is_some()
    }

    /// 重新扫描插件并对齐剪贴板工作线程状态（设置程序启停/选择插件后经 IPC 触发）。
    pub fn reload(self: &Arc<Self>) {
        self.scan_enabled();
        self.sync_clipboard_worker();
    }

    /// 本地剪贴板变化：转交剪贴板同步线程（hash 去重后经插件推送到远端）。
    pub fn clipboard_local_changed(&self, text: &str) {
        info!("剪贴板变化进入宿主: {} 字符", text.len());
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

    fn has_typed_plugin(&self, plugin_type: &str) -> bool {
        self.enabled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|p| p.plugin_type == plugin_type)
    }

    /// 解析剪贴板同步应使用的插件。
    /// 选型来源 clipboard_sync.toml（设置程序写入，与 Android daemon 契约一致）：
    /// enabled=false 或无候选 → None；plugin_id 为空时回退第一个已启用的
    /// clipboard_sync 插件。
    fn resolve_clipboard_plugin(&self) -> Option<EnabledPlugin> {
        let root = self.rime_dir.parent().unwrap_or_else(|| Path::new("."));
        let selection = read_clipboard_sync_selection(root);
        if !selection.enabled {
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
    /// 线程常驻（剪贴板历史记录不依赖同步插件）；仅在选型变更时停旧起新。
    fn sync_clipboard_worker(self: &Arc<Self>) {
        let desired = self.resolve_clipboard_plugin().map(|p| p.id);
        let started = self.worker_started.load(Ordering::Acquire);
        let current = self
            .clipboard_plugin
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if started && current == desired {
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

    /// 启动剪贴板工作线程（常驻；按选型持有可选的同步插件运行时）。
    fn start_clipboard_worker(self: &Arc<Self>) {
        let plugin = self.resolve_clipboard_plugin();
        let plugin_id = plugin.as_ref().map(|p| p.id.clone());
        let db_path = xime_config::clipboard_store::default_db_path();
        let (tx, rx) = mpsc::channel::<ClipboardCommand>();
        std::thread::spawn(move || run_clipboard_worker(plugin, db_path, rx));
        *self
            .clipboard_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(tx);
        *self
            .clipboard_plugin
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = plugin_id;
        self.worker_started.store(true, Ordering::Release);
    }
}

/// 剪贴板工作线程（常驻）：始终记录剪贴板历史；持有 clipboard_sync 插件
/// 运行时（若启用）时才执行推送/拉取。
/// 三通道去重状态（当前内容 / 上次推送 / 自写回显）归本线程所有，无需加锁。
fn run_clipboard_worker(
    plugin: Option<EnabledPlugin>,
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

    info!(
        "剪贴板工作线程已启动: db={} 同步插件={}",
        db_path.display(),
        if plugin_id.is_empty() { "未启用" } else { &plugin_id }
    );
    let mut current: Option<String> = None;
    let mut last_pushed: Option<String> = None;
    let mut self_written: Option<String> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            ClipboardCommand::LocalChanged(text) => {
                info!("worker 收到剪贴板变化: {} 字符", text.len());
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
                        info!("剪贴板已推送 ({} 字符)", text.len());
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
                    info!("远端剪贴板已写回本地 ({} 字符)", text.len());
                }
            }
            ClipboardCommand::Shutdown => {
                info!("剪贴板工作线程已停止: {}", plugin_id);
                break;
            }
        }
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
        info!("剪贴板历史已记录");
    }
}

/// 剪贴板同步选型（设置程序写 clipboard_sync.toml，与 Android daemon 契约共享）。
#[derive(Default, serde::Deserialize)]
struct ClipboardSyncSelection {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    plugin_id: String,
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

fn add_dir_to_zip<W: std::io::Write + Seek>(
    zip: &mut zip::ZipWriter<W>,
    dir: &Path,
    prefix: &str,
    options: zip::write::SimpleFileOptions,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("读取 {} 失败: {}", dir.display(), e))?;
    for entry in entries.flatten() {
        let name = format!("{}/{}", prefix, entry.file_name().to_string_lossy());
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                // build/ 为部署产物，可再生，不进备份包
                if entry.file_name() == "build" {
                    continue;
                }
                add_dir_to_zip(zip, &entry.path(), &name, options)?;
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

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 本地日期 YYYY-MM-DD（civil_from_days，无外部时间依赖）。
fn local_date_string() -> String {
    let days = (unix_secs() / 86400) as i64;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("winxime_plugins_{label}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        dir
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
        std::thread::spawn(move || run_clipboard_worker(None, db_for_worker, rx));
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
            Err(e) => {
                assert!(false, "manifest 解析失败: {e}");
                return;
            }
        };
        assert_eq!(manifest.name, "WebDAV 剪贴板同步");
        let config = std::env::temp_dir().join(format!("xime_schema_test_{}.yaml", std::process::id()));
        let runtime = match PluginRuntime::load(&dir, &manifest.entry, &config) {
            Ok(rt) => rt,
            Err(e) => {
                assert!(false, "插件加载失败: {e}");
                return;
            }
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
            "schema 标签异常: {labels:?}"
        );
        assert!(labels.contains(&"密码"), "schema 标签异常: {labels:?}");
    }
}
