#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod calculator;
mod clipboard;
mod config;
mod context;
mod custom_phrase;
mod hashing;
mod ipc_server;
mod models;
mod notify;
mod paste;
mod plugins;
mod recent_usage;
mod register;
mod schema_switches;
mod speech;
mod toast;
mod tray;
mod ui;
mod user_dict;
mod schema_dict;

#[cfg(test)]
mod test_support;

use crate::context::SharedInputContext;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tracing::{error, info, warn};
use windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext;
use winxime_ipc::{check_server_running, IpcClient};
use xime_config::{
    init_logging_with_console, set_app_metadata, AppMetadata, SchemaManifest,
};
use xime_rime::RimeEngine;

fn main() {
    let _ = set_app_metadata(AppMetadata {
        display_name: "曦码·曜",
        config_dir_name: "xime",
        config_file_base: "xime",
        distribution_name: "Xime Yao",
        distribution_code_name: "Xime Yao",
        app_name: "rime.xime.server",
        version: env!("CARGO_PKG_VERSION"),
    });

    unsafe {
        let _ = SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    // 日志级别：让 `xime.yaml` 的 `log_level` 真正生效（xime-config 的 init_logging
    // 只认 `RUST_LOG`，配置项没人读）。必须放在 init 之前。
    apply_log_level_from_config();

    init_logging_with_console("server");
    info!("Server starting");
    // 出现「配置改了没反应」时，先看这行：设置页写的是哪个文件、文件到底在不在。
    let config_path = xime_config::XimeConfig::config_path();
    info!(
        "Config file: {} (exists={})",
        config_path.display(),
        config_path.exists()
    );
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|arg| arg == "/q" || arg == "/quit") {
        if check_server_running() {
            IpcClient::shutdown_server();
        }
        info!("Server stopped via /q");
        return;
    }

    if check_server_running() {
        info!("Stopping existing server...");
        IpcClient::shutdown_server();
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if !check_server_running() {
                break;
            }
        }
        if check_server_running() {
            info!("Failed to stop existing server, exiting");
            return;
        }
        info!("Existing server stopped");
    }

    let (shared_data_dir, user_data_dir, install_dir) = get_data_dirs();
    info!(
        "Data dirs: shared={}, user={}",
        shared_data_dir.display(),
        user_data_dir.display()
    );
    // 内置计算器的「方案是否占用触发键」检测要读当前方案的 yaml（见 calculator.rs）。
    calculator::init_rime_dir(&user_data_dir);

    // 方案部署源：debug 直接用仓库 rime-wubi 源目录；release 用安装目录自带的 data/ + user-data/。
    #[cfg(debug_assertions)]
    let schema_sources = vec![shared_data_dir.clone()];
    #[cfg(not(debug_assertions))]
    let schema_sources = vec![install_dir.join("data"), install_dir.join("user-data")];
    ensure_rime_data(&schema_sources, &user_data_dir, builtin_removed(&user_data_dir));
    register_builtin_schema_package(&user_data_dir, &schema_sources);

    if !shared_data_dir.exists() {
        info!("Shared data not found at {:?}", shared_data_dir);
        std::process::exit(1);
    }

    register::ensure_registered();

    // 本地语音识别引擎：工作线程常驻、空闲不占麦克风；模型目录按
    // `%APPDATA%\Xime\models\<id>`（与安卓同一约定，见 models.rs）。
    // 启动日志按**设置里选中的**模型打印（不是注册表默认模型）。
    let speech = speech::init(&user_data_dir);
    info!(
        "Speech engine ready: model_id={}, model_ready={}, provider={}",
        speech.selected_profile().id,
        speech.model_ready(),
        speech::provider_label()
    );

    let config = crate::config::get();
    // 候选数量 = 每页显示几个：rime 的 `menu/page_size` 必须 ≥ 它，否则拉不到那么多候选。
    // 写在部署**之前**，本次启动的部署就会带上。
    sync_rime_page_size(&user_data_dir, config.style.candidate_count);
    let engine = match RimeEngine::new(&shared_data_dir, &user_data_dir, "Xime Yao") {
        Ok(mut e) => {
            e.set_option("_horizontal", config.style.horizontal);
            info!(
                "Rime initialized successfully with horizontal={}",
                config.style.horizontal
            );

            info!("Running rime deployment...");
            if e.deploy() {
                info!("Rime deployment completed successfully");
            } else {
                info!("Rime deployment failed (may already be deployed)");
            }

            // 恢复用户上次选中的方案（deploy 重建了会话，默认回落列表第一个）。
            // 记录来自 rime 自己的 user.yaml（var/previously_selected_schema），
            // 见 load_rime_selected_schema。
            if let Some(id) = ipc_server::load_rime_selected_schema() {
                if e.select_schema(&id) {
                    info!("Restored previously selected schema: {}", id);
                } else {
                    info!("Previously selected schema '{}' not available", id);
                }
            }

            // 部署完核对一次：方案自己写死的 `menu.page_size` 会压过我们的设置，
            // 日志里要说清楚（否则用户只看到"候选数量不生效"）。
            if let Some(id) = e.get_current_schema() {
                warn_if_schema_overrides_page_size(
                    &user_data_dir,
                    &id,
                    config.style.candidate_count.clamp(1, 10) as usize,
                );
            }

            if let Some(status) = e.get_status() {
                info!(
                    "Active schema: {} ({})",
                    status.schema_id, status.schema_name
                );
            }

            // Copy compiled dictionary files from shared to user data dir
            // Rime's deploy may skip compiling files whose sources haven't changed,
            // leaving some .table.bin / .reverse.bin files missing in user data dir.
            let shared_build = shared_data_dir.join("build");
            let user_build = user_data_dir.join("build");
            if let Ok(entries) = std::fs::read_dir(&shared_build) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.ends_with(".table.bin") || name_str.ends_with(".reverse.bin") {
                        let user_path = user_build.join(&name);
                        if !user_path.exists() {
                            info!("Copying missing dictionary: {}", name_str);
                            let _ = std::fs::copy(entry.path(), &user_path);
                        }
                    }
                }
            }

            Arc::new(std::sync::Mutex::new(e))
        }
        Err(e) => {
            info!("Rime init failed: {}", e);
            std::process::exit(1);
        }
    };

    run_server(engine, install_dir, user_data_dir);
}

fn get_data_dirs() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    #[cfg(debug_assertions)]
    {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_dir = manifest_dir
            .ancestors()
            .nth(2)
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| manifest_dir.clone());
        (
            workspace_dir.join("rime-wubi"),
            workspace_dir.join("target").join("debug").join("user-data"),
            workspace_dir.join("rime-wubi"),
        )
    }

    #[cfg(not(debug_assertions))]
    {
        let exe_path = std::env::current_exe().ok().unwrap_or_else(|| {
            std::path::PathBuf::from("C:\\Program Files\\Xime\\winxime-server.exe")
        });
        let exe_dir = exe_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("C:\\Program Files\\Xime"));

        // 单目录模型（对齐 Xime）：user 与 shared 同指向 %APPDATA%\Xime\rime，
        // 安装目录自带的 data/ + user-data/ 在首次运行时部署进去（见 ensure_rime_data）。
        let rime_dir = std::env::var("APPDATA")
            .ok()
            .map(|p| std::path::PathBuf::from(p).join("Xime").join("rime"))
            .unwrap_or_else(|| exe_dir.join("user-data"));

        (rime_dir.clone(), rime_dir.clone(), exe_dir.to_path_buf())
    }
}

/// 把 rime 目录里不属于任何市场包的方案文件登记为「内置方案包」（builtin）并备份到
/// `market/builtin/`（对齐 Android `SchemaManifestManager` 的 `ensureBuiltinBackup`
/// + `refreshBuiltinManifest`）。
///
/// 这是方案隔离的基础：内置方案文件有了明确归属后，市场包安装时的冲突检测才知道
/// 哪些文件不得覆盖（内容不同即拒绝），卸载时也不会把内置方案文件当成第三方包的
/// 文件删掉，且随时可从备份还原。
fn register_builtin_schema_package(
    rime_dir: &std::path::Path,
    seed_dirs: &[std::path::PathBuf],
) {
    let data_root = rime_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| rime_dir.to_path_buf());
    let manifest = SchemaManifest::new(rime_dir.to_path_buf(), data_root);
    if manifest.is_builtin_removed() {
        // 用户显式清理过内置方案：不再把 seed 注入回来，只把用户自己的方案
        // 登记为 local，供设置页展示/删除。
        let _ = manifest.register_local_package();
        info!("Builtin schema package was removed by user; seed injection skipped");
        return;
    }
    match manifest.seed_builtin_from(seed_dirs) {
        Ok(0) => info!("Builtin schema package: no new seed files"),
        Ok(n) => info!("Registered {n} builtin schema file(s) from install seed"),
        Err(e) => warn!("Failed to seed builtin schema package: {}", e),
    }
}

/// 数据根里的「用户清理过内置」标记（设置页写入）。
fn builtin_removed(rime_dir: &std::path::Path) -> bool {
    rime_dir
        .parent()
        .map(|root| root.join(".builtin-removed").is_file())
        .unwrap_or(false)
}

/// 对齐 Xime 的方案部署语义（单目录模型）：
/// - 首装（rime 目录下无任何 *.schema.yaml）：把 source_dirs 依次全量复制进 rime 目录
/// - 升级：仅覆盖内容有变化且文件名不含 "custom" 的文件（保护用户定制与第三方方案）
fn ensure_rime_data(
    source_dirs: &[std::path::PathBuf],
    rime_dir: &std::path::Path,
    skip_seed: bool,
) {
    let _ = std::fs::create_dir_all(rime_dir);
    if skip_seed {
        // 用户清理过内置方案：不再把安装包 seed 复制回来。
        info!("Builtin schema seed copy skipped (removed by user)");
        return;
    }
    let has_schema = std::fs::read_dir(rime_dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".schema.yaml"))
        })
        .unwrap_or(false);

    for source in source_dirs {
        if !source.exists() {
            continue;
        }
        if has_schema {
            // 升级路径：用户已弃用的 builtin 方案文件不强更（不覆盖用户自己的
            // 同名方案/改造）。用户启用列表来自 default.custom.yaml 的 - schema: 行。
            let enabled = read_enabled_schemas(rime_dir);
            let skip_builtin = !enabled.is_empty();
            copy_changed_files(source, rime_dir, skip_builtin.then_some(&enabled));
        } else {
            copy_dir_contents(source, rime_dir);
        }
    }
}

/// 读取用户启用的方案 id 列表（default.custom.yaml 里的 `- schema: xxx` 行）。
fn read_enabled_schemas(rime_dir: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(rime_dir.join("default.custom.yaml"))
        .map(|content| {
            content
                .lines()
                .filter_map(|line| {
                    let line = line.trim();
                    let rest = line.strip_prefix('-')?.trim();
                    let rest = rest.strip_prefix("schema:")?.trim();
                    let id = rest.trim_matches('"').trim_matches('\'');
                    (!id.is_empty()).then(|| id.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 判断文件是否属于用户未启用的 builtin 方案（`<id>.schema.yaml` /
/// `<id>.dict.yaml`，id 在 builtin 方案 id 集合内但不在用户启用列表中）。
fn is_unused_builtin_file(
    name: &str,
    builtin_ids: &std::collections::HashSet<String>,
    enabled: &[String],
) -> bool {
    let stem = name
        .strip_suffix(".schema.yaml")
        .or_else(|| name.strip_suffix(".dict.yaml"));
    let Some(id) = stem else {
        return false;
    };
    builtin_ids.contains(id) && !enabled.iter().any(|e| e == id)
}

/// 升级复制：仅当目标缺失或内容不同，且文件名不含 "custom"（保护用户定制）。
/// `skip` 传入用户启用列表时，未启用的 builtin 方案文件（*.schema.yaml /
/// *.dict.yaml）不再强更——不覆盖用户自己的方案。
fn copy_changed_files(
    src: &std::path::Path,
    dst: &std::path::Path,
    skip: Option<&[String]>,
) {
    let builtin_ids: std::collections::HashSet<String> = match skip {
        Some(_) => std::fs::read_dir(src)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| {
                        let n = e.file_name().to_string_lossy().to_string();
                        n.strip_suffix(".schema.yaml")
                            .filter(|_| e.path().is_file())
                            .map(String::from)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        None => Default::default(),
    };
    if let Ok(entries) = std::fs::read_dir(src) {
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let dest = dst.join(entry.file_name());
            if ft.is_dir() {
                let _ = std::fs::create_dir_all(&dest);
                copy_changed_files(&entry.path(), &dest, skip);
            } else {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name.contains("custom") {
                    continue;
                }
                if let Some(enabled) = skip {
                    if is_unused_builtin_file(&name, &builtin_ids, enabled) {
                        continue;
                    }
                }
                let needs_copy = match std::fs::read(&dest) {
                    Ok(existing) => existing != std::fs::read(entry.path()).unwrap_or_default(),
                    Err(_) => true,
                };
                if needs_copy {
                    let _ = std::fs::copy(entry.path(), &dest);
                }
            }
        }
    }
}

fn copy_dir_contents(src: &std::path::Path, dst: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(src) {
        for entry in entries.flatten() {
            let ft = entry.file_type().ok();
            let dest = dst.join(entry.file_name());
            if ft.is_some_and(|t| t.is_dir()) {
                let _ = std::fs::create_dir_all(&dest);
                copy_dir_recursive(&entry.path(), &dest);
            } else {
                let _ = std::fs::copy(entry.path(), &dest);
            }
        }
    }
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(src) {
        for entry in entries.flatten() {
            let ft = entry.file_type().ok();
            let dest = dst.join(entry.file_name());
            if ft.is_some_and(|t| t.is_dir()) {
                let _ = std::fs::create_dir_all(&dest);
                copy_dir_recursive(&entry.path(), &dest);
            } else {
                let _ = std::fs::copy(entry.path(), &dest);
            }
        }
    }
}

/// 启动设置程序（可选附加参数，如 --about）。
/// 设置程序与 server 同目录，托盘菜单与候选栏菜单面板共用此入口。
fn launch_setup(extra_arg: Option<&str>) {
    let exe_path = std::env::current_exe().ok().unwrap_or_else(|| {
        std::path::PathBuf::from("C:\\Program Files\\winxime-server\\winxime-server.exe")
    });
    let exe_dir = exe_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("C:\\Program Files\\winxime-server"));
    let setup_path = exe_dir.join("winxime-setup.exe");
    // 冷启动计时的外部锚点：setup.log 的第一行（logging 就绪）与这条之差
    // 就是进程创建/装载耗时，用来判断「打开设置慢」是不是花在进程启动上。
    info!("启动设置程序: {}", setup_path.display());
    let mut command = std::process::Command::new(&setup_path);
    if let Some(arg) = extra_arg {
        command.arg(arg);
    }
    let _ = command.spawn();
}

fn run_server(
    engine: Arc<std::sync::Mutex<RimeEngine>>,
    install_dir: std::path::PathBuf,
    user_data_dir: std::path::PathBuf,
) {
    info!("run_server: starting");
    let context = Arc::new(SharedInputContext::new());
    let ascii_mode = Arc::new(AtomicBool::new(false));
    let main_thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };

    info!("Creating UI window...");
    let window = ui::CandidateWindow::new();
    info!("UI window created");

    // 候选栏菜单面板动作（菜单页点击「设置」→ 启动设置程序）。
    ui::panel::set_panel_action_callback(Arc::new(|action| match action {
        ui::panel::MenuAction::OpenSettings => launch_setup(None),
    }));

    // 「剪切板」子页的数据源：与设置程序/剪贴板工作线程同一个 clipboard.db
    // （rime 用户目录的同级文件）。
    let clipboard_db = user_data_dir
        .parent()
        .map(|dir| dir.join("clipboard.db"))
        .unwrap_or_else(|| user_data_dir.join("clipboard.db"));
    ui::panel::set_clipboard_db_path(clipboard_db.clone());
    // 面板「最近使用」记录（表情 / 符号）：与 clipboard.db 同目录的
    // recent_usage.json（安卓版存在 SharedPreferences 里，等价的一份 JSON）。
    recent_usage::set_store_path(clipboard_db.with_file_name("recent_usage.json"));

    info!("Creating plugin host...");
    // 插件宿主：内置插件安装（resources/plugins）+ 已启用插件 JS 运行时管理。
    // 插件根为 rime 用户目录同级（release: %APPDATA%\Xime\plugins）。
    let plugin_host = plugins::PluginHost::new(
        user_data_dir.clone(),
        Some(install_dir.join("resources").join("plugins")),
    );

    info!("Starting IPC thread...");
    let engine_clone = engine.clone();
    let context_clone = context.clone();
    let window_clone = window.clone();
    let ascii_mode_clone = ascii_mode.clone();
    let plugin_host_for_ipc = plugin_host.clone();
    std::thread::spawn(move || {
        ipc_server::run_ipc_server(
            engine_clone,
            context_clone,
            window_clone,
            ascii_mode_clone,
            main_thread_id,
            plugin_host_for_ipc,
        );
    });
    info!("IPC thread started");

    info!("Creating tray icon...");
    let on_action = {
        let engine = engine.clone();
        // 切到英文态时收起候选栏：与 IPC 侧的 ToggleAsciiMode 保持同一行为
        // （英文态下宿主不再把按键送进来，候选栏留着既没用、也会挡住输入法的
        // 「已关闭」语义）。托盘与按键两条路做同一件事，就不能只有一条收栏。
        let window_for_tray = window.clone();
        Arc::new(move |action: tray::TrayAction| match action {
            tray::TrayAction::ToggleAsciiMode => {
                if let Ok(mut eng) = engine.try_lock() {
                    let current = eng.is_ascii_mode();
                    // 英文态下宿主不再送按键，计算器必须一起收掉（否则切回中文后
                    // 会带着旧的算式继续，像「模式没关」）。
                    crate::calculator::reset("tray ToggleAsciiMode");
                    eng.set_option("ascii_mode", !current);
                    tray::update_tray_icon(!current);
                    window_for_tray.hide();
                }
            }
            tray::TrayAction::OpenSettings => launch_setup(None),
            tray::TrayAction::SyncUserData => {
                // 走 IPC 回环（与设置程序同一路径），同步在 ipc 线程完成。
                std::thread::spawn(|| {
                    if winxime_ipc::IpcClient::sync_user_data() {
                        info!("用户资料同步完成（托盘触发）");
                        crate::notify::notify("用户资料同步完成");
                    } else {
                        error!("用户资料同步失败（托盘触发）");
                        crate::notify::notify("用户资料同步失败，详见 server.log");
                    }
                });
            }
            tray::TrayAction::About => launch_setup(Some("--about")),
            tray::TrayAction::Feedback => {
                let _ = std::process::Command::new("cmd")
                    .args([
                        "/C",
                        "start",
                        "https://github.com/kingzcheung/winxime/issues",
                    ])
                    .spawn();
            }
            tray::TrayAction::ToggleSwitch { name, options } => {
                // 布尔开关取反；多选一轮转到下一项（对齐 Android toggleSchemaSwitch）。
                if let Ok(mut eng) = engine.try_lock() {
                    if !name.is_empty() {
                        let current = eng.get_option(&name).unwrap_or(false);
                        eng.set_option(&name, !current);
                    } else if options.len() > 1 {
                        let active = options
                            .iter()
                            .position(|o| eng.get_option(o) == Some(true))
                            .unwrap_or(0);
                        let next = (active + 1) % options.len();
                        for (i, opt) in options.iter().enumerate() {
                            eng.set_option(opt, i == next);
                        }
                    }
                }
            }
            tray::TrayAction::Quit => {
                IpcClient::shutdown_server();
            }
        })
    };

    // 方案 switches 提供者：每次弹出菜单求值（当前方案 + 各开关实时取值）。
    let engine_for_switches = engine.clone();
    let rime_dir_for_switches = user_data_dir.clone();
    let switch_provider: tray::SwitchProvider = Arc::new(move || {
        // 第一次拿锁只问「当前是哪个方案」，问完立刻放开：`read_schema_switches` 是
        // 整份 `<id>.schema.yaml` 的 read + `serde_yaml` 解析，持锁做的话，菜单弹出
        // 期间所有键事件 `try_lock` 都失败（键被丢，而且看起来像输入法卡死）。
        let schema_id = {
            let Ok(eng) = engine_for_switches.try_lock() else {
                return Vec::new();
            };
            let Some(schema_id) = eng.get_current_schema() else {
                return Vec::new();
            };
            schema_id
        };

        let switches =
            crate::schema_switches::read_schema_switches(&rime_dir_for_switches, &schema_id);

        // 第二次拿锁读各开关的当前取值：只有几次 `get_option`，不碰 IO。
        // 两次拿锁之间用户理论上可能切了方案；菜单是瞬时的，用旧方案的开关列表
        // 不影响正确性（点下去是按名字设值）。
        let Ok(eng) = engine_for_switches.try_lock() else {
            return Vec::new();
        };
        switches
            .into_iter()
            // ascii_mode 与顶部「切换中/英」重复，跳过。
            .filter(|sw| sw.name != "ascii_mode")
            .map(|sw| {
                if !sw.name.is_empty() {
                    let on = eng.get_option(&sw.name).unwrap_or(false);
                    let idx = if on { 1 } else { 0 };
                    let label = sw
                        .states
                        .get(idx)
                        .or_else(|| sw.states.last())
                        .cloned()
                        .unwrap_or_else(|| sw.name.clone());
                    tray::SwitchMenuItem {
                        label,
                        checked: on,
                        name: sw.name,
                        options: Vec::new(),
                    }
                } else {
                    let active = sw
                        .options
                        .iter()
                        .position(|o| eng.get_option(o) == Some(true))
                        .unwrap_or(0);
                    let label = sw.states.get(active).cloned().unwrap_or_default();
                    tray::SwitchMenuItem {
                        label,
                        checked: false,
                        name: String::new(),
                        options: sw.options,
                    }
                }
            })
            .collect()
    });

    tray::TrayIcon::install(on_action, switch_provider);
    info!("Tray icon created");

    info!("Starting clipboard listener...");
    // 本地变化 → 插件推送；定时节拍 → 拉取远端并写回系统剪贴板。
    // 插件运行时由剪贴板同步专用线程独占持有，回调只投递命令（非阻塞）。
    let host_for_clipboard = plugin_host.clone();
    clipboard::start_listener(Arc::new(move |event| match event {
        clipboard::ClipboardEvent::Changed => {
            if let Some(text) = clipboard::read_text() {
                host_for_clipboard.clipboard_local_changed(&text);
            }
        }
        clipboard::ClipboardEvent::Tick => {
            host_for_clipboard.clipboard_poll_remote();
        }
    }));

    info!("Server ready, entering message loop");

    unsafe {
        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        while windows::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }
    }
    info!("Message loop exited, cleaning up");

    tray::cleanup();
    speech::SpeechEngine::global_shutdown();
    info!("Server shutdown complete");
}

/// 把候选栏显示数量同步成 rime 的 `menu/page_size`。
///
/// 候选是 librime 一页给的：`style.candidate_count` 调到 7，而 `menu/page_size` 还是
/// 5，候选栏最多只能显示 5 个——用户看到的就是「候选数量设置不生效」。
/// 只改这一个键（`default.custom.yaml` 里其它 patch 原样保留），**部署后生效**。
fn sync_rime_page_size(user_dir: &std::path::Path, candidate_count: i32) {
    // 上限 10：选字键是 1–9 + 0，再多也没键可选。方案包里如果写死了
    // `menu.page_size`，它会压过这里的 patch——那种方案需要自己删掉那行
    // （约定见 xime-docs：候选数量归 xime 管）。
    let page_size = candidate_count.clamp(1, 10) as usize;
    match xime_config::set_page_size_in(user_dir, page_size) {
        Ok(true) => info!("menu/page_size 已对齐到 {page_size}（部署后生效）"),
        Ok(false) => {}
        Err(e) => tracing::warn!("同步 menu/page_size 失败: {e}"),
    }
}

/// 部署后核对：方案的 `menu.page_size` 是否压过了 xime 的候选数量设置。
///
/// `menu/page_size` 是**方案级**配置：方案包在自己的 `*.schema.yaml` 里写了 page_size，
/// 就压过 `default.custom.yaml`（xime 只改默认层）。这时日志必须说清楚是哪一行，
/// 否则用户只会看到「候选数量设置不生效」。
fn warn_if_schema_overrides_page_size(
    user_dir: &std::path::Path,
    schema_id: &str,
    wanted: usize,
) {
    let path = user_dir.join("build").join(format!("{schema_id}.schema.yaml"));
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(tree) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return;
    };
    let effective = tree
        .get("menu")
        .and_then(|menu| menu.get("page_size"))
        .and_then(|value| value.as_i64());
    if let Some(size) = effective {
        if size as usize != wanted {
            tracing::warn!(
                "方案 {schema_id} 自己定义了 menu.page_size={size}（期望 {wanted}）：\
                 候选数量受它限制。删掉 {schema_id}.schema.yaml 里的 menu.page_size 即可\
                 让 xime 的设置生效。"
            );
        }
    }
}

/// 把配置里的 `log_level` 翻译成 `RUST_LOG`（环境变量已设置时不动）。
///
/// `xime_config::init_logging` 的过滤器写死了「全局 info + 本项目各 crate debug」，
/// `XimeConfig::log_level` 这个配置项从来没被读过——用户以为改成 trace 有用，其实没用。
/// 这里在初始化前写进 `RUST_LOG`，`EnvFilter::try_from_default_env()` 就会优先采用它。
///
/// 第三方 crate 一律留在 `info`：`trace` 全开会把 librime 的逐键日志也倒进文件，
/// 反倒把要找的那几行埋掉。级别改动**重启服务后生效**（InitLogger 只能初始化一次）。
fn apply_log_level_from_config() {
    if std::env::var_os("RUST_LOG").is_some() {
        return;
    }
    let config = xime_config::XimeConfig::load();
    // 没配就**什么都不做**：`xime_config` 的默认过滤器是「全局 info + 本项目各 crate
    // debug」，比裸 `info` 信息量大得多，不能因为配置项缺席就把它调低。
    if config.log_level.is_none() {
        return;
    }
    let level = config.effective_log_level().to_string();
    let filter = match level.as_str() {
        "trace" | "debug" => format!(
            "info,winxime_server={level},winxime_tsf={level},winxime_ipc={level},\
             xime_config={level},xime_rime={level},xime_setup_lib={level},xime_plugin={level}"
        ),
        // error / warn / info 直接当全局指令用。
        other => other.to_string(),
    };
    std::env::set_var("RUST_LOG", filter);
}
