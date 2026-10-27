use crate::context::SharedInputContext;
use crate::plugins::PluginHost;
use crate::ui::CandidateWindow;
use interprocess::os::windows::named_pipe::{pipe_mode::Bytes, PipeListenerOptions};
use interprocess::os::windows::security_descriptor::SecurityDescriptor;
use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use tracing::{debug, info};
use widestring::u16cstr;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
use winxime_ipc::{
    get_pipe_path, IpcCommand, IpcRequest, IpcRequestData, IpcResponse,
};
use xime_rime::RimeEngine;

const MAX_BUFFER_SIZE: usize = 1024 * 1024;

/// X11 F1–F24 的 keysym 区间。功能键永远归宿主：IME 不消费 F2/F5。
fn is_function_key(keycode: i32) -> bool {
    (0xFFBE..=0xFFD5).contains(&keycode)
}

/// 当前拥有托盘图标的 TIP 实例（会话/线程 id）。`u32::MAX` = 没有 owner。
///
/// 托盘只有一个，但每个宿主进程/线程都会加载一份 TIP；如果每个实例都能直接
/// Hide，后台实例的 Deactivate 就会把前台正在用的图标藏掉。只有 owner 的
/// Hide 才生效，Show 会重新认领 owner。
const TRAY_OWNER_NONE: u32 = u32::MAX;
/// Chrome 里点进/点出输入框会短暂 `Deactivate` 再 `Activate`（实测约 0.8s）；
/// 直接隐藏会闪。Hide 先等一会儿，期间任何 Show 都会取消它。
const TRAY_HIDE_DELAY: std::time::Duration = std::time::Duration::from_millis(1500);

#[derive(Clone, Copy)]
struct TrayState {
    owner: u32,
    /// 每次 Show 自增；延迟 Hide 只有在自己拿到的 epoch 仍是当前值时才执行。
    epoch: u64,
}

static TRAY_STATE: Mutex<TrayState> = Mutex::new(TrayState {
    owner: TRAY_OWNER_NONE,
    epoch: 0,
});

fn tray_claim(session_id: u32) {
    // 旧版 TSF DLL 的 session_id 固定是 0；它可能仍被宿主进程映射着。
    // 不能让它抢走新实例的 owner，否则旧进程一次 Show 就会把前台 owner 改成 0。
    if session_id == 0 {
        debug!("托盘 show: 忽略旧版 session_id=0");
        return;
    }
    let mut state = TRAY_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.epoch = state.epoch.wrapping_add(1);
    state.owner = session_id;
    debug!(
        "托盘 show: owner={session_id} epoch={}",
        state.epoch
    );
    // 先改状态再投递 Show；Hide 线程拿同一把锁，不会把新 Show 又盖掉。
    crate::tray::show_icon();
}

/// 延迟隐藏：Chrome 的短暂 Deactivate 会被后续 Show 取消；真正切到其他
/// 输入法产品时没有后续 Show，1.5s 后才隐藏。
fn tray_schedule_hide(session_id: u32) {
    if session_id == 0 {
        debug!("托盘 hide: 忽略旧版 session_id=0");
        return;
    }
    let epoch = {
        let state = TRAY_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.owner != session_id {
            return;
        }
        state.epoch
    };

    std::thread::spawn(move || {
        std::thread::sleep(TRAY_HIDE_DELAY);
        let mut state = TRAY_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.owner == session_id && state.epoch == epoch {
            state.owner = TRAY_OWNER_NONE;
            debug!("托盘 hide: owner={session_id} epoch={epoch}");
            crate::tray::hide_icon();
        }
    });
}

/// 候选栏翻页：UI 线程只做引擎锁 + 前后台切换，**不再走 F24 自注入**。
/// 翻页不需要宿主跑编辑会话，直接改 server 的 rime 状态和候选窗即可；
/// 这样连续快速点击也不会把宿主/TSF 的按键队列塞满。
static PAGE_TX: OnceLock<std::sync::mpsc::Sender<bool>> = OnceLock::new();

/// 在 server 启动时注入候选栏翻页需要的全局句柄，并起一个**单线程**翻页队列。
/// 连续快速点击只排队、不每次 spawn 线程，也不会把 F24 塞进宿主按键队列。
pub fn set_page_targets(
    engine: &Arc<std::sync::Mutex<RimeEngine>>,
    context: &Arc<SharedInputContext>,
    window: &Arc<CandidateWindow>,
) {
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    if PAGE_TX.set(tx).is_err() {
        return;
    }
    let engine = engine.clone();
    let context = context.clone();
    let window = window.clone();
    std::thread::spawn(move || {
        while let Ok(backward) = rx.recv() {
            // 引擎可能正被按键线程占着：短暂重试几次，别把这次点击丢掉。
            let mut eng = None;
            for _ in 0..40 {
                match engine.try_lock() {
                    Ok(guard) => {
                        eng = Some(guard);
                        break;
                    }
                    Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                        eng = Some(poisoned.into_inner());
                        break;
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                }
            }
            let Some(mut eng) = eng else {
                continue;
            };
            let _ = change_rime_page(&mut eng, &context, &window, 0, backward);
        }
    });
}

/// 候选栏 `‹`/`›` 点击：投递到翻页队列，UI 线程立即返回。
pub fn request_page_from_ui(backward: bool) {
    if let Some(tx) = PAGE_TX.get() {
        let _ = tx.send(backward);
    }
}

pub fn run_ipc_server(
    engine: Arc<std::sync::Mutex<RimeEngine>>,
    context: Arc<SharedInputContext>,
    window: Arc<CandidateWindow>,
    ascii_mode: Arc<AtomicBool>,
    main_thread_id: u32,
    plugin_host: Arc<PluginHost>,
) {
    let pipe_path = get_pipe_path();
    tracing::info!("Winxime Server: creating named pipe at {}", pipe_path);
    set_page_targets(&engine, &context, &window);

    let sd = match SecurityDescriptor::deserialize(u16cstr!("D:(A;;GA;;;WD)")) {
        Ok(sd) => sd,
        Err(e) => {
            tracing::error!("Failed to build security descriptor: {e}");
            return;
        }
    };

    let listener = match PipeListenerOptions::new()
        .path(pipe_path)
        .mode(interprocess::os::windows::named_pipe::PipeMode::Bytes)
        .security_descriptor(Some(sd))
        .create_duplex::<Bytes>()
    {
        Ok(l) => l,
        Err(e) => {
            tracing::info!("Failed to create pipe listener: {}", e);
            return;
        }
    };

    tracing::info!("Waiting for client connections...");

    for pipe in listener.incoming() {
        match pipe {
            Ok(p) => {
                tracing::info!("Client connected!");
                let engine_clone = engine.clone();
                let context_clone = context.clone();
                let window_clone = window.clone();
                let ascii_mode_clone = ascii_mode.clone();
                let tid = main_thread_id;
                let plugin_host_clone = plugin_host.clone();
                std::thread::spawn(move || {
                    handle_connection(
                        p,
                        engine_clone,
                        context_clone,
                        window_clone,
                        ascii_mode_clone,
                        tid,
                        plugin_host_clone,
                    );
                });
            }
            Err(e) => {
                tracing::info!("Failed to accept connection: {}", e);
            }
        }
    }
}

fn handle_connection(
    pipe: interprocess::os::windows::named_pipe::PipeStream<Bytes, Bytes>,
    engine: Arc<std::sync::Mutex<RimeEngine>>,
    context: Arc<SharedInputContext>,
    window: Arc<CandidateWindow>,
    ascii_mode: Arc<AtomicBool>,
    main_thread_id: u32,
    plugin_host: Arc<PluginHost>,
) {
    let (recv, send) = pipe.split();
    let mut reader = BufReader::new(recv);
    let mut writer = BufWriter::new(send);

    loop {
        let mut buffer = Vec::new();

        loop {
            if buffer.len() > MAX_BUFFER_SIZE {
                tracing::info!("Buffer too large, disconnecting client");
                return;
            }

            let mut byte = [0u8; 1];
            match reader.read(&mut byte) {
                Ok(0) => {
                    if buffer.is_empty() {
                        return;
                    }
                    break;
                }
                Ok(_) => {
                    if byte[0] == 0 {
                        break;
                    }
                    buffer.push(byte[0]);
                }
                Err(_) => return,
            }
        }

        if buffer.is_empty() {
            continue;
        }

        let request: IpcRequest = match serde_json::from_slice(&buffer) {
            Ok(r) => r,
            Err(_) => continue,
        };
        // 每次按键都走这里：`info!` 会把 server.log 写爆（历史 21MB 里有大半是这类行）。
        tracing::trace!("Received request: {:?}", request.command);

        // 单向命令：客户端只写不等回包（`IpcClient::send_no_reply`），下面**必须不写**。
        // 协议没有请求 id，多写一条应答就留在管道里，被下一条请求当成自己的应答
        // （候选词/上屏文本串位，且完全不报错）。
        let response = process_request(
            &request,
            &engine,
            &context,
            &window,
            &ascii_mode,
            main_thread_id,
            &plugin_host,
        );

        if request.command.is_one_way() {
            continue;
        }

        let json = match serde_json::to_vec(&response) {
            Ok(j) => j,
            Err(_) => continue,
        };
        if writer.write_all(&json).is_err() {
            break;
        }
        if writer.write_all(&[0]).is_err() {
            break;
        }
        if writer.flush().is_err() {
            break;
        }
    }
    tracing::info!("Client disconnected");
}

/// 语音命令（设置页「语音转文本」区）。
///
/// 不碰 rime 引擎 → 在引擎锁之外执行；下载只投命令立刻返回（进度靠轮询）。
/// 响应把语音快照挂在 `status.speech` 上，`IpcResponse` 现有字段一律不动。
fn handle_speech(request: &IpcRequest) -> IpcResponse {
    let model_id = match &request.data {
        winxime_ipc::IpcRequestData::SpeechModel(id) => Some(id.clone()),
        _ => None,
    };
    // 需要模型 id 的三条：缺了就报错，不猜。
    let with_model = |action: fn(&str) -> Result<(), String>| match model_id.as_deref() {
        Some(id) => action(id).err(),
        None => Some("缺少模型 id".to_string()),
    };

    let error = match request.command {
        IpcCommand::GetSpeechStatus => None,
        IpcCommand::SpeechDownload => {
            with_model(crate::speech::SpeechEngine::global_start_download)
        }
        IpcCommand::SpeechDelete => with_model(crate::speech::SpeechEngine::global_delete_model),
        IpcCommand::SpeechSelect => with_model(crate::speech::SpeechEngine::global_select_model),
        IpcCommand::SpeechTestStart => {
            crate::speech::SpeechEngine::global_start_preview();
            None
        }
        // 试听结束走 Stop（会话处于试听模式，不会上屏也不弹通知）。
        IpcCommand::SpeechTestStop => {
            crate::speech::SpeechEngine::global_stop();
            None
        }
        _ => None,
    };

    IpcResponse {
        success: error.is_none(),
        session_id: request.session_id,
        status: Some(winxime_ipc::Status {
            speech: Some(speech_status(error)),
            ..Default::default()
        }),
        ..IpcResponse::default()
    }
}

/// 组装语音状态快照（设置页每 250ms 拉一次）。
///
/// 引擎未初始化（服务刚起/初始化失败）时：模型列表为空、`state` 仍报 `idle`、
/// `provider` 仍给分包说明——设置页据「列表为空 + 拿不到状态」显示「服务未运行」，
/// 不会把「服务没起来」误报成「模型没下载」。
fn speech_status(error: Option<String>) -> winxime_ipc::SpeechStatus {
    use crate::speech::{download, SpeechEngine, SpeechState};

    let snapshot = SpeechEngine::global_snapshot();
    let profile = SpeechEngine::global_selected_profile();
    let download_state = download::download_state();
    winxime_ipc::SpeechStatus {
        state: match snapshot.state {
            SpeechState::Idle => "idle",
            SpeechState::Loading => "loading",
            SpeechState::Listening => "listening",
        }
        .to_string(),
        model_id: profile.id.clone(),
        model_name: profile.name.clone(),
        model_ready: SpeechEngine::global_model_ready(),
        provider: crate::speech::provider_label(),
        text: snapshot.partial.clone(),
        // 本次命令的错误优先，其次是会话错误，最后是模型操作（下载/删除）错误。
        error: error.or(snapshot.error).or_else(download::last_error),
        download: download_state.map(|state| winxime_ipc::SpeechDownload {
            model_id: state.model_id,
            progress: state.progress,
        }),
        models_rev: download::models_rev(),
        models: SpeechEngine::global_catalog()
            .into_iter()
            .map(|entry| winxime_ipc::SpeechModel {
                id: entry.id,
                name: entry.name,
                description: entry.description,
                size: entry.size,
                downloaded: entry.downloaded,
                selected: entry.selected,
                recommended: entry.recommended,
            })
            .collect(),
    }
}

/// 方案词表只读浏览（ListSchemaEntries）：主码表 + import_tables 递归 +
/// translator.packs，在 server 侧解析（设置进程不该自己猜 rime 目录布局）。
///
/// 不碰引擎 → 在引擎锁之外执行（见 process_request 开头的预分发注释）。
fn handle_list_schema_entries(
    request: &IpcRequest,
    plugin_host: &Arc<PluginHost>,
) -> IpcResponse {
    let query = match &request.data {
        winxime_ipc::IpcRequestData::SchemaQuery(schema_id, query) => {
            Some((schema_id.clone(), query.clone()))
        }
        _ => None,
    };
    tracing::info!("ListSchemaEntries requested: {:?}", query.as_ref().map(|q| &q.0));
    let outcome = match query {
        Some((schema_id, query)) => {
            crate::schema_dict::read_schema_dict(plugin_host.rime_dir(), &schema_id, &query)
        }
        None => Err("缺少方案 id".to_string()),
    };
    match outcome {
        Ok(read) => IpcResponse {
            schema_dict_response: Some(winxime_ipc::SchemaDictResponse {
                dict_name: read.dict_name,
                tables: read.tables,
                missing: read.missing,
                entries: read.entries,
                total: read.total,
                matched: read.matched,
            }),
            ..IpcResponse::ok(request.session_id)
        },
        Err(reason) => {
            tracing::error!("读取方案词表失败：{reason}");
            IpcResponse::fail(request.session_id)
        }
    }
}

/// 快捷短语表读取（ListCustomPhrases）：词/编码/权重 + 文件与 patch 状态。
fn handle_list_custom_phrases(
    request: &IpcRequest,
    plugin_host: &Arc<PluginHost>,
) -> IpcResponse {
    let schema_id = match &request.data {
        winxime_ipc::IpcRequestData::SchemaName(id) => Some(id.clone()),
        _ => None,
    };
    tracing::info!("ListCustomPhrases requested: {:?}", schema_id.as_deref());
    let outcome = match schema_id {
        Some(id) => crate::custom_phrase::read_custom_phrases(plugin_host.rime_dir(), &id),
        None => Err("缺少方案 id".to_string()),
    };
    match outcome {
        Ok(read) => IpcResponse {
            phrase_response: Some(winxime_ipc::CustomPhraseResponse {
                dict_name: read.dict_name,
                file_name: read.file_name,
                file_exists: read.file_exists,
                patch_applied: read.patch_applied,
                patch_added: false,
                entries: read.entries,
            }),
            ..IpcResponse::ok(request.session_id)
        },
        Err(reason) => {
            tracing::error!("读取快捷短语失败：{reason}");
            IpcResponse::fail(request.session_id)
        }
    }
}

/// 快捷短语整表覆盖保存（SaveCustomPhrases）+ 视需要注入方案 patch。
///
/// **不做部署**：patch 首次注入后要重新部署才生效，由设置页提示（或点
/// 「重新部署」）。
fn handle_save_custom_phrases(
    request: &IpcRequest,
    plugin_host: &Arc<PluginHost>,
) -> IpcResponse {
    let table = match &request.data {
        winxime_ipc::IpcRequestData::CustomPhraseTable(schema_id, entries) => {
            Some((schema_id.clone(), entries.clone()))
        }
        _ => None,
    };
    tracing::info!("SaveCustomPhrases requested");
    let outcome = match table {
        Some((schema_id, entries)) => {
            crate::custom_phrase::save_custom_phrases(plugin_host.rime_dir(), &schema_id, &entries)
        }
        None => Err("缺少方案 id".to_string()),
    };
    match outcome {
        Ok(saved) => IpcResponse {
            phrase_response: Some(winxime_ipc::CustomPhraseResponse {
                dict_name: saved.dict_name,
                file_name: saved.file_name,
                file_exists: saved.file_exists,
                patch_applied: saved.patch_applied,
                patch_added: saved.patch_added,
                entries: saved.entries,
            }),
            ..IpcResponse::ok(request.session_id)
        },
        Err(reason) => {
            tracing::error!("保存快捷短语失败：{reason}");
            IpcResponse::fail(request.session_id)
        }
    }
}

/// 重载插件（扫描启用列表 + 起停剪贴板同步线程）。
///
/// 不碰 rime 引擎，所以放在拿引擎锁之前处理；`process_request` 的锁内分支也调它，
/// 保证两条路径行为一致。
fn handle_reload_plugins(plugin_host: &Arc<PluginHost>, session_id: u32) -> IpcResponse {
    tracing::info!("ReloadPlugins requested");
    plugin_host.reload();
    IpcResponse::ok(session_id)
}

/// 退出服务（给主线程投 `WM_QUIT`）。
///
/// 不碰引擎：锁被占时也必须能退（见 `process_request` 的预分发注释）。
fn handle_shutdown(main_thread_id: u32, session_id: u32) -> IpcResponse {
    tracing::info!("Shutdown requested, posting WM_QUIT to main thread");
    unsafe {
        let _ = PostThreadMessageW(main_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
    }
    IpcResponse::ok(session_id)
}

/// 插入点坐标上报（**单向命令**，客户端不等回包）。
///
/// 只写 `SharedInputContext`，不碰 rime 引擎——所以放在拿锁之前处理：引擎忙
/// （同步用户资料 / 部署）时坐标也不会丢，候选栏不会停在旧位置。
fn handle_update_position(
    request: &IpcRequest,
    context: &Arc<SharedInputContext>,
) -> IpcResponse {
    if let IpcRequestData::Position(pos) = &request.data {
        // 每次按键都来一次，`info!` 会把日志写爆；排查定位问题时再开 trace。
        tracing::trace!("Position: {},{}", pos.x, pos.y);
        context.update(|ctx| {
            ctx.caret_x = pos.x;
            ctx.caret_y = pos.y;
        });
    }
    IpcResponse::ok(request.session_id)
}

/// 系统通知（toast）。不碰引擎，锁外处理。
///
/// 系统通知由 server 弹（MSIX 包内进程，toast 可归属；设置进程直跑无包身份，
/// toast 会被系统拒绝/无从归属）。
fn handle_show_toast(request: &IpcRequest) -> IpcResponse {
    let toast = match &request.data {
        IpcRequestData::Toast(t) => Some(t.clone()),
        _ => None,
    };
    let success = match toast {
        Some(t) => {
            // 所有 toast（包括设置页转发来的）都走统一入口：去重 + 限流 + 留日志。
            crate::notify::notify_titled(&t.title, t.body);
            true
        }
        None => false,
    };
    IpcResponse {
        success,
        session_id: request.session_id,
        ..IpcResponse::default()
    }
}

/// 处理按键（`ProcessKeyEvent`）。
///
/// `process_request` 里唯一的长分支：建议词模式、英文态、正常组词三条路径在此分流。
fn handle_key_event(
    request: &IpcRequest,
    eng: &mut RimeEngine,
    context: &Arc<SharedInputContext>,
    window: &Arc<CandidateWindow>,
    ascii_mode: &Arc<AtomicBool>,
) -> IpcResponse {
    // 候选栏「剪切板」面板点击后的自注入触发键（见 crate::paste）：
    // 必须在其他分支之前识别——它不参与 rime 组词，也不受英文态影响。
    if let IpcRequestData::KeyEvent(key) = &request.data {
        if key.keycode == crate::paste::XK_PASTE_TRIGGER {
            return handle_paste_trigger(
                eng,
                context,
                window,
                request.session_id,
                key.modifiers,
            );
        }
    }

    // 内置计算器（crate::calculator）：激活期间先于建议词/英文态/rime——
    // 联想模式下数字是选字键，绝不能被别的路径抢走。Delegated 表示计算器已退出、
    // 键继续走原有流程（后续流程会收尾组词串）。
    if crate::calculator::is_active() {
        if let IpcRequestData::KeyEvent(key) = &request.data {
            match crate::calculator::handle_active_key(
                key.keycode,
                key.modifiers,
                request.session_id,
                eng,
                context,
                window,
            ) {
                crate::calculator::ActiveKeyOutcome::Handled(response) => return *response,
                crate::calculator::ActiveKeyOutcome::Delegated => {}
                crate::calculator::ActiveKeyOutcome::NotAKeyDown => {
                    // 激活期间的 key-up 绝不能落到普通路径：那里在
                    // `!eng.is_composing()` 时会 `window.hide()`，于是每按一个键的抬起
                    // 都把刚显示的候选栏收掉——实机上就是「候选只闪一下」。
                    // 这里直接回一个被拒的应答：宿主不会调度编辑会话，键抬起也无需
                    // 交给 rime（计算器不经过 rime）。
                    return IpcResponse {
                        status: Some(get_ipc_status(eng)),
                        ..IpcResponse::fail(request.session_id)
                    };
                }
            }
        }
    }

    // B2 原型：数字开头的影子缓冲（默认关闭，见 calculator::auto_detect_enabled）。
    // 激活期间与计算器一样，key-up 不能落到普通路径（否则会把影子组词串收掉）。
    if crate::calculator::shadow_active() {
        if let IpcRequestData::KeyEvent(key) = &request.data {
            match crate::calculator::handle_shadow_key(
                key.keycode,
                key.modifiers,
                request.session_id,
                eng,
                context,
                window,
            ) {
                crate::calculator::ActiveKeyOutcome::Handled(response) => return *response,
                crate::calculator::ActiveKeyOutcome::Delegated => {}
                crate::calculator::ActiveKeyOutcome::NotAKeyDown => {
                    return IpcResponse {
                        status: Some(get_ipc_status(eng)),
                        ..IpcResponse::fail(request.session_id)
                    };
                }
            }
        }
    }

    if let IpcRequestData::KeyEvent(key) = &request.data {
        if is_function_key(key.keycode) {
            // 不交给 rime：rime 会把未知 keysym 当字母/符号，F2 会变成 q。
            return IpcResponse {
                status: Some(get_ipc_status(eng)),
                ..IpcResponse::fail(request.session_id)
            };
        }
    }

    let is_ascii = ascii_mode.load(Ordering::Acquire);
    tracing::info!("Key event, ascii_mode={}", is_ascii);

    let suggestion_state = context.read(|c| c.suggestion_state.clone());

    if let Some(ref suggestion) = suggestion_state {
        tracing::info!(
            "  -> in suggestion mode, suggestions: {:?}",
            suggestion.suggestions
        );

        if let IpcRequestData::KeyEvent(key) = &request.data {
            if key.keycode == 32 {
                tracing::info!("  -> Space in suggestion mode, commit suggestion");
                if suggestion.highlighted < suggestion.suggestions.len() {
                    let selected_word = &suggestion.suggestions[suggestion.highlighted];
                    tracing::info!("  -> committing suggestion word: {}", selected_word);

                    context.update(|ctx| {
                        ctx.suggestion_state = None;
                        ctx.commit_text = selected_word.clone();
                    });
                    window.hide();

                    return IpcResponse {
                        context: Some(winxime_ipc::Context {
                            preedit: winxime_ipc::Text { str: String::new() },
                            commit: Some(selected_word.clone()),
                            candidates: winxime_ipc::CandidateInfo::default(),
                        }),
                        status: Some(get_ipc_status(eng)),
                        ..IpcResponse::ok(request.session_id)
                    };
                } else {
                    context.update(|ctx| {
                        ctx.suggestion_state = None;
                    });
                    window.hide();
                    return IpcResponse {
                        status: Some(get_ipc_status(eng)),
                        ..IpcResponse::fail(request.session_id)
                    };
                }
            } else if key.keycode >= 49 && key.keycode <= 57 {
                let index = (key.keycode - 49) as usize;
                tracing::info!("  -> Number key {} in suggestion mode", index + 1);
                if index < suggestion.suggestions.len() {
                    let selected_word = &suggestion.suggestions[index];
                    tracing::info!("  -> committing suggestion word: {}", selected_word);

                    context.update(|ctx| {
                        ctx.suggestion_state = None;
                        ctx.commit_text = selected_word.clone();
                    });
                    window.hide();

                    return IpcResponse {
                        context: Some(winxime_ipc::Context {
                            preedit: winxime_ipc::Text { str: String::new() },
                            commit: Some(selected_word.clone()),
                            candidates: winxime_ipc::CandidateInfo::default(),
                        }),
                        status: Some(get_ipc_status(eng)),
                        ..IpcResponse::ok(request.session_id)
                    };
                } else {
                    return IpcResponse {
                        status: Some(get_ipc_status(eng)),
                        ..IpcResponse::fail(request.session_id)
                    };
                }
            } else {
                tracing::info!(
                    "  -> Other key in suggestion mode, clear suggestion and continue"
                );
                context.update(|ctx| {
                    ctx.suggestion_state = None;
                });
                window.hide();
            }
        } else {
            return IpcResponse {
                status: Some(get_ipc_status(eng)),
                ..IpcResponse::fail(request.session_id)
            };
        }
    }

    if is_ascii {
        tracing::info!("  -> ASCII mode, not handling");
        return IpcResponse {
            status: Some(get_ipc_status(eng)),
            ..IpcResponse::fail(request.session_id)
        };
    }

    // 内置计算器入口：组词串为空时按 `=`。方案自带功能占用触发键时让位
    // （见 calculator::trigger_blocked 的动态检测），`=` 照常走 rime。
    // 放在英文态早退之后：英文态下输入法是透明的，不接管任何键。
    if let IpcRequestData::KeyEvent(key) = &request.data {
        if key.keycode == librime::key::XK_EQUAL
            && key.modifiers
                & (librime::key::K_CONTROL_MASK
                    | librime::key::K_ALT_MASK
                    | librime::key::K_RELEASE_MASK) as i32
                == 0
        {
            if let Some(response) =
                crate::calculator::try_enter(eng, request.session_id, context, window)
            {
                return response;
            }
        }
    }

    // B2 原型入口：组词串为空时按下数字，先把数字吞进影子缓冲；等出现运算符
    // 再切成算式。非算式键由 shadow_replay 回放（先提交数字、再注入当前键）。
    // 默认关闭（见 calculator::auto_detect_enabled）。
    if let IpcRequestData::KeyEvent(key) = &request.data {
        let plain = key.modifiers
            & (librime::key::K_CONTROL_MASK
                | librime::key::K_ALT_MASK
                | librime::key::K_RELEASE_MASK) as i32
            == 0;
        if plain && crate::calculator::is_shadow_starter(key.keycode) && !eng.is_composing() {
            let no_suggestion = context.read(|c| c.suggestion_state.is_none());
            if no_suggestion {
                if let Some(response) = crate::calculator::try_enter_shadow(
                    key.keycode,
                    key.modifiers,
                    request.session_id,
                    eng,
                    context,
                    window,
                ) {
                    return response;
                }
            }
        }
    }

    let handled = match &request.data {
        IpcRequestData::KeyEvent(key) => {
            tracing::info!("Key: {} mod: {}", key.keycode, key.modifiers);
            let result = eng.process_key(key.keycode, key.modifiers);
            tracing::info!("  handled: {}", result);
            result
        }
        _ => false,
    };

    let commit = eng.get_commit();
    tracing::info!("  commit: {:?}", commit);
    info!("  input: {:?}", eng.get_input());
    info!("  composing: {}", eng.is_composing());

    if let Some(ref commit_text) = commit {
        tracing::info!(">>> COMMIT_TO_SCREEN: '{}'", commit_text);
    }

    let ipc_ctx = get_ipc_context(eng, &commit);
    update_context(eng, context, &commit);

    if let Some(ref commit_text) = commit {
        tracing::info!(">>> COMMIT_TO_SCREEN: '{}'", commit_text);

        tracing::info!("  -> hide (commit)");
        context.update(|ctx| {
            ctx.suggestion_state = None;
        });
        window.hide();
        IpcResponse {
            success: handled,
            session_id: request.session_id,
            context: ipc_ctx,
            status: Some(get_ipc_status(eng)),
            ..IpcResponse::default()
        }
    } else if !eng.is_composing() {
        tracing::info!("  -> hide (not composing)");
        context.update(|ctx| {
            ctx.suggestion_state = None;
        });
        window.hide();
        IpcResponse {
            success: handled,
            session_id: request.session_id,
            context: ipc_ctx,
            status: Some(get_ipc_status(eng)),
            ..IpcResponse::default()
        }
    } else if let Some(ctx) = &ipc_ctx {
        tracing::info!("  candies: {:?}", ctx.candidates.candies);
        if ctx.candidates.candies.is_empty() {
            tracing::info!("  -> hide (no candidates)");
            window.hide();
        } else {
            let pos = context.read(|c| (c.caret_x, c.caret_y));
            tracing::info!("  -> show at ({}, {})", pos.0, pos.1);
            window.show(pos.0, pos.1);
            info!("  -> update {} candies", ctx.candidates.candies.len());
            window.update(ctx);
        }

        IpcResponse {
            success: handled,
            session_id: request.session_id,
            context: ipc_ctx,
            status: Some(get_ipc_status(eng)),
            ..IpcResponse::default()
        }
    } else {
        IpcResponse {
            success: handled,
            session_id: request.session_id,
            context: ipc_ctx,
            status: Some(get_ipc_status(eng)),
            ..IpcResponse::default()
        }
    }
}

/// 词典管理命令：列表 / 逐条浏览 / 导入导出 / 备份恢复。
///
/// 与键事件同理，它们都只依赖引擎和插件目录，单独成函数后
/// `process_request` 里只剩一行转发。
fn handle_dict_command(
    request: &IpcRequest,
    eng: &mut RimeEngine,
    context: &Arc<SharedInputContext>,
    window: &Arc<CandidateWindow>,
) -> IpcResponse {
    match request.command {
        IpcCommand::ListUserDicts => {
            tracing::info!("ListUserDicts requested");
            let dicts = librime::list_user_dicts();
            let sync_dir = librime::get_user_data_sync_dir();
            IpcResponse {
                dict_response: Some(winxime_ipc::DictResponse {
                    dicts,
                    count: 0,
                    sync_dir,
                    entries: Vec::new(),
                    total: 0,
                }),
                ..IpcResponse::ok(request.session_id)
            }
        }

        IpcCommand::ListDictEntries => {
            // 逐条浏览用户词典：经 librime 导出通道读成文本码表再解析
            // （librime 没有"读词条"的 C 接口，见 crate::user_dict 模块注释）。
            let query = match &request.data {
                winxime_ipc::IpcRequestData::UserDictQuery(dict, query) => {
                    Some((dict.clone(), query.clone()))
                }
                _ => None,
            };
            tracing::info!("ListDictEntries requested: {:?}", query.as_ref().map(|q| &q.0));
            // 读词条会重建 rime 会话（见下方 with_user_dict_closed），组词串随之消失。
            crate::calculator::reset("ListDictEntries");
            let outcome = match query {
                // 读词条 = 让 librime **导出一份用户词典文本**再解析；而 librime 要求
                // user dict 在 Export 前处于**关闭**状态（与 Import/Backup/Restore 同一条约束）。
                // 原来直接在会话里导出 → `ExportUserDict` 恒失败，设置页看到的是
                // 「读取词条失败（输入法服务未运行？）」，其实服务活得好好的。
                // 走 `with_user_dict_closed`：关会话 → 导出 → 重建会话
                // （路由器与 ImportDictEntry 一致；代价是当前未上屏的组词会丢）。
                Some((dict, query)) => eng
                    .with_user_dict_closed(move || crate::user_dict::read_user_dict(&dict, &query)),
                None => Err("缺少词典名".to_string()),
            };
            match outcome {
                Ok(read) => IpcResponse {
                    dict_response: Some(winxime_ipc::DictResponse {
                        dicts: Vec::new(),
                        count: read.matched,
                        sync_dir: String::new(),
                        entries: read.entries,
                        total: read.total,
                    }),
                    ..IpcResponse::ok(request.session_id)
                },
                Err(reason) => {
                    tracing::error!("读取用户词典词条失败：{reason}");
                    IpcResponse::fail(request.session_id)
                }
            }
        }

        IpcCommand::ImportDictEntry => {
            // 写入一条用户词条（频率 > 0 新增、< 0 删除标记）。librime 要求
            // user dict 在 Backup/Restore/Export/Import 前处于关闭状态——
            // 销毁会话 → 导入 → 重建会话（RimeEngine::with_user_dict_closed）。
            // 代价：正在输入的句子会丢；重建后照 FocusOut 的做法清掉残留的
            // 候选窗与输入上下文，免得宿主对着已不存在的 composition 继续画。
            let entry = match &request.data {
                winxime_ipc::IpcRequestData::UserDictEntry(dict, word, code, commits) => {
                    Some((dict.clone(), word.clone(), code.clone(), *commits))
                }
                _ => None,
            };
            tracing::info!("ImportDictEntry requested");
            // 写词条同样会重建会话，计算器缓冲一起清（见下方 with_user_dict_closed）。
            crate::calculator::reset("ImportDictEntry");
            let outcome = match entry {
                Some((dict, word, code, commits)) => {
                    let result = eng.with_user_dict_closed(move || {
                        crate::user_dict::import_entry(&dict, &word, &code, commits)
                    });
                    eng.clear_composition();
                    window.hide();
                    context.update(|ctx| {
                        ctx.suggestion_state = None;
                    });
                    result
                }
                None => Err("缺少词条参数".to_string()),
            };
            match outcome {
                Ok(count) => IpcResponse {
                    dict_response: Some(winxime_ipc::DictResponse {
                        dicts: Vec::new(),
                        count,
                        sync_dir: String::new(),
                        entries: Vec::new(),
                        total: 0,
                    }),
                    ..IpcResponse::ok(request.session_id)
                },
                Err(reason) => {
                    tracing::error!("写入用户词条失败：{reason}");
                    IpcResponse::fail(request.session_id)
                }
            }
        }

        IpcCommand::BackupUserDict => {
            tracing::info!("BackupUserDict requested");
            let dict = match &request.data {
                winxime_ipc::IpcRequestData::UserDict(name) => Some(name.clone()),
                _ => None,
            };
            let result = match dict {
                Some(name) => librime::backup_user_dict(&name).map(|_| 0),
                None => Err(librime::error::Error::InvalidUtf8),
            };
            let success = result.is_ok();
            IpcResponse {
                success,
                session_id: request.session_id,
                dict_response: result.ok().map(|count| winxime_ipc::DictResponse {
                dicts: Vec::new(),
                count,
                sync_dir: String::new(),
                entries: Vec::new(),
                total: 0,
            }),
                ..IpcResponse::default()
            }
        }

        IpcCommand::RestoreUserDict => {
            tracing::info!("RestoreUserDict requested");
            let path = match &request.data {
                winxime_ipc::IpcRequestData::UserDictPath(p) => Some(p.clone()),
                _ => None,
            };
            let success = path
                .map(|p| librime::restore_user_dict(&p).is_ok())
                .unwrap_or(false);
            IpcResponse {
                success,
                session_id: request.session_id,
                ..IpcResponse::default()
            }
        }

        IpcCommand::ExportUserDict => {
            tracing::info!("ExportUserDict requested");
            let file = match &request.data {
                winxime_ipc::IpcRequestData::UserDictFile(d, p) => Some((d.clone(), p.clone())),
                _ => None,
            };
            let result = match file {
                Some((dict, path)) => librime::export_user_dict(&dict, &path),
                None => Err(librime::error::Error::InvalidUtf8),
            };
            let success = result.is_ok();
            IpcResponse {
                success,
                session_id: request.session_id,
                dict_response: result.ok().map(|count| winxime_ipc::DictResponse {
                dicts: Vec::new(),
                count,
                sync_dir: String::new(),
                entries: Vec::new(),
                total: 0,
            }),
                ..IpcResponse::default()
            }
        }

        IpcCommand::ImportUserDict => {
            tracing::info!("ImportUserDict requested");
            let file = match &request.data {
                winxime_ipc::IpcRequestData::UserDictFile(d, p) => Some((d.clone(), p.clone())),
                _ => None,
            };
            let result = match file {
                Some((dict, path)) => librime::import_user_dict(&dict, &path),
                None => Err(librime::error::Error::InvalidUtf8),
            };
            let success = result.is_ok();
            IpcResponse {
                success,
                session_id: request.session_id,
                dict_response: result.ok().map(|count| winxime_ipc::DictResponse {
                dicts: Vec::new(),
                count,
                sync_dir: String::new(),
                entries: Vec::new(),
                total: 0,
            }),
                ..IpcResponse::default()
            }
        }
        // 只有 process_request 确认命令属于本组后才会调到这里
        _ => IpcResponse::fail(request.session_id),
    }
}

fn process_request(
    request: &IpcRequest,
    engine: &Arc<std::sync::Mutex<RimeEngine>>,
    context: &Arc<SharedInputContext>,
    window: &Arc<CandidateWindow>,
    ascii_mode: &Arc<AtomicBool>,
    main_thread_id: u32,
    plugin_host: &Arc<PluginHost>,
) -> IpcResponse {
    // 不碰引擎的命令在拿锁之前处理：方案词表首次解析大码表可能要
    // 几百毫秒（pinyin_simp ~39 万条），持锁做会挡住正在打字的键事件
    // （引擎 try_lock 失败时键事件直接失败）。这些命令只读/写 rime 目录的
    // 文件，不需要会话。
    match request.command {
        IpcCommand::ListSchemaEntries => {
            return handle_list_schema_entries(request, plugin_host);
        }
        IpcCommand::ListCustomPhrases => {
            return handle_list_custom_phrases(request, plugin_host);
        }
        IpcCommand::SaveCustomPhrases => {
            return handle_save_custom_phrases(request, plugin_host);
        }
        // 插件重载只扫插件目录 + 起停剪贴板同步线程，完全不碰 rime 引擎。
        // 插件多时目录扫描 + manifest 解析不便宜，放在锁内会挡住正在打的字。
        IpcCommand::ReloadPlugins => {
            return handle_reload_plugins(plugin_host, request.session_id);
        }
        // 位置上报：只写 `SharedInputContext`，不碰引擎；而且它是**单向命令**
        // （客户端只写不等回包，服务端在 `handle_connection` 里跳过回包）。
        // 放锁外之后，引擎忙（同步资料 / 部署）时坐标也不会丢。
        IpcCommand::UpdateInputPosition => {
            return handle_update_position(request, context);
        }
        // 退出只是 PostThreadMessageW，跟引擎无关。留在锁后面的话，引擎正忙
        // （同步用户资料、重载配置、部署）时 `try_lock` 失败 → 直接回 fail，
        // 托盘「退出」点了没反应，用户只能去任务管理器杀进程。
        IpcCommand::ShutdownServer => {
            return handle_shutdown(main_thread_id, request.session_id);
        }
        // 系统通知同理：只调 toast API，不读引擎。每次弹通知都在锁内排队没有理由。
        IpcCommand::ShowToast => {
            return handle_show_toast(request);
        }
        // 语音命令同样不碰 rime 引擎：语音引擎是独立的全局单例（自己一条工作
        // 线程 + 一个下载线程），拿 rime 引擎锁只会白白挡住打字。下载更是**不能**
        // 在这里等（132MB，快命令 1s 就判超时）——只投命令、立刻回状态，
        // 进度由设置页轮询 GetSpeechStatus 取。
        IpcCommand::GetSpeechStatus
        | IpcCommand::SpeechDownload
        | IpcCommand::SpeechDelete
        | IpcCommand::SpeechSelect
        | IpcCommand::SpeechTestStart
        | IpcCommand::SpeechTestStop => {
            return handle_speech(request);
        }
        _ => {}
    }

    let mut eng = match engine.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            tracing::info!("Engine lock would block, returning error response");
            return IpcResponse::fail(request.session_id);
        }
    };

    match request.command {
        IpcCommand::Echo => IpcResponse::ok(request.session_id),

        // 只读状态：设置页启动时用来把「当前方案」标对行（不建会话、不改状态）。
        IpcCommand::GetStatus => IpcResponse {
            status: Some(get_ipc_status(&eng)),
            ..IpcResponse::ok(request.session_id)
        },

        IpcCommand::StartSession => {
            tracing::info!("StartSession");
            // 新会话不可能还有上一次会话的组词串；缓冲留着只会在新文档里
            // 「复活」一个没人画得出的算式。
            crate::calculator::reset("StartSession");
            IpcResponse {
                status: Some(get_ipc_status(&eng)),
                ..IpcResponse::ok(request.session_id)
            }
        }

        IpcCommand::EndSession => {
            tracing::info!("EndSession");
            crate::calculator::reset("EndSession");
            IpcResponse::ok(request.session_id)
        }

        IpcCommand::FocusIn => {
            tracing::info!("FocusIn");
            IpcResponse {
                status: Some(get_ipc_status(&eng)),
                ..IpcResponse::ok(request.session_id)
            }
        }

        IpcCommand::FocusOut => {
            // B2：宿主失焦时先把影子缓冲里的数字取出来，放进回包的 commit 里，
            // 由 TSF 在**旧 context** 上提交（TSF 的 FocusOut 已改为同步请求）。
            // 计算器 B1 的算式仍按原行为丢弃（未完成算式不该悄悄上屏）。
            let pending_digits = crate::calculator::take_shadow_for_flush();
            tracing::info!("FocusOut -> hide composition");
            eng.clear_composition();
            crate::calculator::reset("FocusOut");
            window.hide();
            let context = pending_digits.map(|digits| winxime_ipc::Context {
                preedit: winxime_ipc::Text { str: String::new() },
                commit: Some(digits),
                candidates: winxime_ipc::CandidateInfo::default(),
            });
            IpcResponse {
                context,
                status: Some(get_ipc_status(&eng)),
                ..IpcResponse::ok(request.session_id)
            }
        }

        IpcCommand::ProcessKeyEvent => {
            handle_key_event(request, &mut eng, context, window, ascii_mode)
        }

        IpcCommand::UpdateInputPosition => {
            // 已在拿锁之前处理（见函数开头的预分发）；保留此分支只为让 match 穷尽。
            handle_update_position(request, context)
        }

        IpcCommand::ShutdownServer => {
            // 已在拿锁之前处理（见函数开头的预分发）；保留此分支只为让 match 穷尽，
            // 两边共用同一实现，避免以后改一处漏一处。
            handle_shutdown(main_thread_id, request.session_id)
        }

        IpcCommand::ToggleAsciiMode => {
            tracing::info!("ToggleAsciiMode requested");
            // 计算器组的是自己的串：切换中英会重建 rime 状态，缓冲必须一起清。
            crate::calculator::reset("ToggleAsciiMode");
            let current = eng.is_ascii_mode();
            let new_mode = !current;
            tracing::info!("  -> current={}, setting to {}", current, new_mode);

            // Check if we were composing before the switch
            let was_composing = eng.is_composing();
            let input_text = if was_composing {
                eng.get_input().unwrap_or_default()
            } else {
                String::new()
            };

            // Clear composition in the engine
            if was_composing {
                tracing::info!("  -> clearing composition before switch");
                eng.clear_composition();
            }

            eng.set_option("ascii_mode", new_mode);
            ascii_mode.store(new_mode, Ordering::Release);
            crate::tray::update_tray_icon(new_mode);

            window.hide();

            // Build context response
            // When switching to ASCII mode with input, commit the input code
            // When switching to Chinese mode or no input, just clear the composition
            let ctx = if new_mode && !input_text.is_empty() {
                // Switching to ASCII mode: commit the input code
                tracing::info!(
                    "  -> commit_code: committing '{}' before switch to ASCII",
                    input_text
                );
                tracing::info!(">>> COMMIT_TO_SCREEN (toggle): '{}'", input_text);
                Some(winxime_ipc::Context {
                    preedit: winxime_ipc::Text { str: String::new() },
                    commit: Some(input_text),
                    candidates: winxime_ipc::CandidateInfo::default(),
                })
            } else if was_composing {
                // Was composing but not committing: indicate composition should be cleared
                tracing::info!("  -> clearing composition in TSF (no commit)");
                Some(winxime_ipc::Context {
                    preedit: winxime_ipc::Text { str: String::new() },
                    commit: None,
                    candidates: winxime_ipc::CandidateInfo::default(),
                })
            } else {
                None
            };

            update_context(&mut eng, context, &None);

            IpcResponse {
                context: ctx,
                status: Some(get_ipc_status(&eng)),
                ..IpcResponse::ok(request.session_id)
            }
        }

        // 托盘归 server 独占；TIP 只上报「谁是当前 owner」，不再各自无限显隐。
        // 非 owner 的 Hide 直接忽略，避免后台实例把前台图标藏掉。
        IpcCommand::ShowTrayIcon => {
            tray_claim(request.session_id);
            IpcResponse::ok(request.session_id)
        }

        IpcCommand::HideTrayIcon => {
            tray_schedule_hide(request.session_id);
            IpcResponse::ok(request.session_id)
        }

        IpcCommand::HideCandidates => {
            context.update(|ctx| {
                ctx.is_composing = false;
                ctx.composition.preedit.clear();
                ctx.candidates.clear();
                ctx.commit_text.clear();
            });
            IpcResponse::ok(request.session_id)
        }

        IpcCommand::ReloadConfig => {
            tracing::info!("ReloadConfig requested");
            // 配置可能被设置程序改过：先丢缓存，redeploy 之后的状态与样式都从新配置出。
            crate::config::invalidate();
            // 方案可能被换掉：触发键让位检测按方案 id 缓存，必须一起失效。
            crate::calculator::invalidate_trigger_cache();
            // redeploy 会重建会话，计算器画在宿主里的组词串一并作废。
            crate::calculator::reset("ReloadConfig");
            // 候选数量变了要把 rime 的 `menu/page_size` 一起对齐，否则部署完还是老页大小。
            let (_, user_dir) = xime_config::get_data_dirs();
            crate::sync_rime_page_size(&user_dir, crate::config::get().style.candidate_count);
            let mut deploy_result = eng.redeploy();
            if !deploy_result {
                // 设置页自己也刚跑过一次部署，两个进程的文件/缓存状态可能差一拍；
                // 重试一次，避免把「第二次就好」的情况当成失败通知给用户。
                tracing::warn!("redeploy 第一次返回 false，200ms 后重试");
                std::thread::sleep(std::time::Duration::from_millis(200));
                deploy_result = eng.redeploy();
            }
            // 部署会重建会话：**部署后引擎实际用的是哪个方案**必须留痕。
            // 「部署完又回到 emoji」这类问题，看这一行就够了。
            tracing::info!(
                "  redeploy result: {}, active schema after redeploy: {:?}",
                deploy_result,
                eng.get_current_schema()
            );
            // 部署是用户显式动作（或候选数量改动）触发的，结果必须让用户看见。
            let status = get_ipc_status(&eng);
            if deploy_result {
                crate::notify::notify(format!("已部署「{}」", status.schema_name));
            } else if eng.get_current_schema().is_some() {
                // 有 active schema 说明引擎还在正常工作，多半是设置页那边的部署
                // 已经成功、这次 redeploy 只是重复动作。只留日志，不打扰用户。
                tracing::warn!(
                    "redeploy 返回 false 但 active schema 仍在（{}），跳过失败通知",
                    status.schema_name
                );
            } else {
                crate::notify::notify(format!(
                    "部署「{}」失败，详见 server.log",
                    status.schema_name
                ));
            }

            if let Some(id) = eng.get_current_schema() {
                crate::warn_if_schema_overrides_page_size(
                    &user_dir,
                    &id,
                    crate::config::get().style.candidate_count.clamp(1, 10) as usize,
                );
            }
            IpcResponse {
                success: deploy_result,
                session_id: request.session_id,
                status: Some(get_ipc_status(&eng)),
                ..IpcResponse::default()
            }
        }

        IpcCommand::ReloadPlugins => {
            // 已在拿锁之前处理（见函数开头的预分发）；保留此分支只为让 match 穷尽，
            // 两边共用同一实现，避免以后改一处漏一处。
            handle_reload_plugins(plugin_host, request.session_id)
        }

        IpcCommand::SyncUserData => {
            tracing::info!("SyncUserData requested");
            // 对齐 weasel Configurator::SyncUserData：sync_user_data 内部走
            // 维护线程，join 等待其完成后再应答（导出 + 合并双向完成）。
            let success = librime::sync_user_data().is_ok();
            if success {
                librime::join_maintenance_thread();
            } else {
                tracing::error!("用户资料同步失败");
            }
            IpcResponse {
                success,
                session_id: request.session_id,
                ..IpcResponse::default()
            }
        }

        IpcCommand::ListUserDicts
        | IpcCommand::ListDictEntries
        | IpcCommand::ImportDictEntry
        | IpcCommand::BackupUserDict
        | IpcCommand::RestoreUserDict
        | IpcCommand::ExportUserDict
        | IpcCommand::ImportUserDict => {
            handle_dict_command(request, &mut eng, context, window)
        }


        IpcCommand::ListSchemaEntries => {
            // 不碰引擎的文件命令（见 process_request 开头的预分发注释）。
            handle_list_schema_entries(request, plugin_host)
        }

        IpcCommand::ListCustomPhrases => handle_list_custom_phrases(request, plugin_host),

        IpcCommand::SaveCustomPhrases => handle_save_custom_phrases(request, plugin_host),





        IpcCommand::ShowToast => {
            // 已在拿锁之前处理（见函数开头的预分发）；保留此分支只为让 match 穷尽。
            handle_show_toast(request)
        }

        IpcCommand::GetSchemaList => {
            tracing::info!("GetSchemaList requested");
            let schemas = eng.get_schema_list();
            let schema_list = schemas
                .iter()
                .map(|(id, name)| winxime_ipc::SchemaInfo {
                    schema_id: id.clone(),
                    schema_name: name.clone(),
                })
                .collect();
            IpcResponse {
                status: Some(get_ipc_status(&eng)),
                schema_list: Some(schema_list),
                ..IpcResponse::ok(request.session_id)
            }
        }

        IpcCommand::SelectSchema => {
            tracing::info!("SelectSchema requested");
            // 方案变了，「`=` 是否被方案占用」的检测结果作废。
            crate::calculator::invalidate_trigger_cache();
            let schema_id = match &request.data {
                winxime_ipc::IpcRequestData::SelectSchema(id) => Some(id.clone()),
                _ => None,
            };

            match schema_id {
                Some(id) => {
                    tracing::info!("  -> selecting schema: {}", id);
                    if eng.select_schema(&id) {
                        // 选中记录由 librime 写进 rime 用户目录 user.yaml
                        // （var/previously_selected_schema），无需另行持久化。
                        // 同时打印**引擎实际报的**方案：用户报「设置里又回到别的方案」时，
                        // 这一行能直接分辨是引擎没切过去、还是设置页取错了值。
                        tracing::info!(
                            "  -> schema selected successfully: requested={}, engine={:?}",
                            id,
                            eng.get_current_schema()
                        );
                        IpcResponse {
                            status: Some(get_ipc_status(&eng)),
                            ..IpcResponse::ok(request.session_id)
                        }
                    } else {
                        // 拒绝的原因几乎总是「方案未部署」（build/ 无产物）：选进
                        // 未部署方案会得到死会话（所有按键不组词），宁拒不选。
                        // 设置端收到失败后会走「写入方案列表 + 部署」的持久化路径。
                        tracing::info!(
                            "  -> schema selection failed: {} 未部署或不存在 (engine={:?})",
                            id,
                            eng.get_current_schema()
                        );
                        // 设置页那边也会提示；这里补一条系统通知，关掉设置窗口后也看得到。
                        crate::notify::notify(format!("切换方案失败：「{id}」未部署"));
                        IpcResponse {
                            status: Some(get_ipc_status(&eng)),
                            ..IpcResponse::fail(request.session_id)
                        }
                    }
                }
                None => IpcResponse {
                    status: Some(get_ipc_status(&eng)),
                    ..IpcResponse::fail(request.session_id)
                },
            }
        }

        IpcCommand::ShowRoot => {
            tracing::info!("ShowRoot requested");
            tracing::info!("  -> request.data type: {:?}", request.data);
            let letter = match &request.data {
                winxime_ipc::IpcRequestData::ShowRoot(c) => Some(*c),
                _ => None,
            };

            tracing::info!("  -> letter: {:?}", letter);

            match letter {
                Some(c) => {
                    let config = crate::config::get();
                    let schema_id = eng.get_status().map(|s| s.schema_id).unwrap_or_default();
                    tracing::info!(
                        "  -> config loaded, schema_id={}, checking root for '{}'",
                        schema_id,
                        c
                    );
                    let root = config.get_root_for_key(&schema_id, c);
                    tracing::info!("  -> root result: {:?}", root);
                    if let Some(root) = root {
                        tracing::info!("  -> showing root for '{}': {}", c, root);
                        let result = window.show_root(c, &root);
                        tracing::info!("  -> show_root result: {:?}", result);
                        IpcResponse {
                            success: result.is_ok(),
                            session_id: request.session_id,
                            status: Some(get_ipc_status(&eng)),
                            ..IpcResponse::default()
                        }
                    } else {
                        tracing::warn!("  -> no root for key '{}' in schema '{}'", c, schema_id);
                        IpcResponse {
                            status: Some(get_ipc_status(&eng)),
                            ..IpcResponse::fail(request.session_id)
                        }
                    }
                }
                None => {
                    tracing::info!("  -> no letter provided");
                    IpcResponse {
                        status: Some(get_ipc_status(&eng)),
                        ..IpcResponse::fail(request.session_id)
                    }
                }
            }
        }

        IpcCommand::HideRoot => {
            tracing::info!("HideRoot requested");
            window.hide_root();

            let ipc_ctx = get_ipc_context(&eng, &None);
            if let Some(ctx) = &ipc_ctx {
                if !ctx.candidates.candies.is_empty() {
                    let pos = context.read(|c| (c.caret_x, c.caret_y));
                    window.show(pos.0, pos.1);
                    window.update(ctx);
                }
            }

            IpcResponse {
                status: Some(get_ipc_status(&eng)),
                ..IpcResponse::ok(request.session_id)
            }
        }

        IpcCommand::SelectCandidate => {
            tracing::info!("SelectCandidate requested");
            let index = match &request.data {
                winxime_ipc::IpcRequestData::SelectIndex(i) => *i,
                _ => 0,
            };
            select_rime_candidate(&mut eng, context, window, request.session_id, index)
        }

        IpcCommand::ChangePage => {
            tracing::info!("ChangePage requested");
            let backward = match &request.data {
                winxime_ipc::IpcRequestData::ChangePage(b) => *b,
                _ => false,
            };
            change_rime_page(&mut eng, context, window, request.session_id, backward)
        }

        _ => IpcResponse::fail(request.session_id),
    }
}

fn update_context(
    eng: &mut RimeEngine,
    context: &Arc<SharedInputContext>,
    commit: &Option<String>,
) {
    use crate::context::CandidateInfo;
    context.update(|ctx| {
        ctx.is_composing = eng.is_composing();
        ctx.composition.preedit = eng.get_input().unwrap_or_default();
        ctx.commit_text = commit.clone().unwrap_or_default();

        let cand_list = eng.get_candidates();
        ctx.candidates = cand_list
            .candidates
            .iter()
            .map(|c| CandidateInfo {
                text: c.text.clone(),
                comment: c.comment.clone().unwrap_or_default(),
            })
            .collect();
    });
}

/// 读取 rime 自己记录的「上次选中的方案」：用户目录 `user.yaml` 的
/// `var/previously_selected_schema`。
///
/// 这个字段由 librime 的 `Switcher::SetActiveSchema` 在每次选方案时写入
/// （`RimeSelectSchema` → `Engine::ApplySchema`），并在 `Switcher::CreateSchema`
/// 建会话时读回——**选中方案的记录归 rime 自己管**，不再自建数据根
/// `selected_schema.txt`（同一份信息两处记录必然不同步）。
///
/// 注意：rime 建会话只在 **schema_list 之内**按此字段恢复，而设置程序允许选中
/// 未启用（不在 schema_list）的方案，所以启动时仍按 id 显式恢复一次。
/// 无记录/解析失败返回 None。
pub fn load_rime_selected_schema() -> Option<String> {
    let user_yaml = xime_config::get_data_dirs().1.join("user.yaml");
    let content = std::fs::read_to_string(user_yaml).ok()?;
    parse_rime_selected_schema(&content)
}

/// 从 user.yaml 内容解析 `var/previously_selected_schema`（纯函数，便于单测）。
fn parse_rime_selected_schema(content: &str) -> Option<String> {
    let config: serde_yaml::Value = serde_yaml::from_str(content).ok()?;
    config
        .get("var")?
        .get("previously_selected_schema")?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 选中当前页第 `index` 个候选（IPC `SelectCandidate` 与「点击候选」自注入共用）。
fn select_rime_candidate(
    eng: &mut RimeEngine,
    context: &std::sync::Arc<SharedInputContext>,
    window: &CandidateWindow,
    session_id: u32,
    index: usize,
) -> IpcResponse {
    tracing::info!("  -> selecting candidate at index {}", index);
    let selected = eng.select_candidate(index);
    tracing::info!("  -> select result: {}", selected);

    let commit = eng.get_commit();
    tracing::info!("  -> commit: {:?}", commit);

    let ipc_ctx = get_ipc_context(eng, &commit);
    update_context(eng, context, &commit);

    if commit.is_some() {
        tracing::info!("  -> hide (commit after select)");
        window.hide();
    } else if !eng.is_composing() {
        tracing::info!("  -> hide (not composing after select)");
        window.hide();
    } else if let Some(ctx) = &ipc_ctx {
        let pos = context.read(|c| (c.caret_x, c.caret_y));
        window.show(pos.0, pos.1);
        window.update(ctx);
    }

    IpcResponse {
        success: selected,
        session_id,
        context: ipc_ctx,
        status: Some(get_ipc_status(eng)),
        ..IpcResponse::default()
    }
}

/// 处理候选栏/面板的「点击」触发键（server 自己 `SendInput` 注入的 VK_F24，
/// 见 [`crate::paste`]）。
///
/// - key down：先看有没有「待选中候选」载荷，有就让 rime 选中那一条；否则取走
///   待上屏文本，清掉当前编码串，把文本作为 `commit` 回包。宿主拿到 commit 会用
///   提交文本替换掉当前 composition 并结束它，于是内容经正常的 TSF 编辑会话落到
///   光标处（`success: true` 同时让宿主吃掉这个键）。
/// - key up：照样回 `success: true`（不漏给前台应用），但不带 commit。
fn change_rime_page(
    eng: &mut RimeEngine,
    context: &std::sync::Arc<SharedInputContext>,
    window: &CandidateWindow,
    session_id: u32,
    backward: bool,
) -> IpcResponse {
    tracing::info!("  -> backward: {}", backward);
    let changed = eng.change_page(backward);
    tracing::info!("  -> change result: {}", changed);

    let ipc_ctx = get_ipc_context(eng, &None);
    if let Some(ctx) = &ipc_ctx {
        let pos = context.read(|c| (c.caret_x, c.caret_y));
        window.show(pos.0, pos.1);
        window.update(ctx);
    }

    IpcResponse {
        success: changed,
        session_id,
        context: ipc_ctx,
        status: Some(get_ipc_status(eng)),
        ..IpcResponse::default()
    }
}

fn handle_paste_trigger(
    eng: &mut RimeEngine,
    context: &std::sync::Arc<SharedInputContext>,
    window: &CandidateWindow,
    session_id: u32,
    modifiers: i32,
) -> IpcResponse {
    let released = modifiers & librime::K_RELEASE_MASK as i32 != 0;
    // 面板上屏/候选点选会替换掉当前组词串：计算器缓冲若还开着，必须先退。
    crate::calculator::reset("paste trigger");

    // 普通 rime 候选点选：让 rime 自己选中（保留词频/学习语义），
    // 与「粘贴面板直接上屏文本」是同一个触发键、不同载荷。
    if released {
        let _ = crate::paste::take_pending_select();
    } else if let Some(index) = crate::paste::take_pending_select() {
        return select_rime_candidate(eng, context, window, session_id, index);
    }

    let text = if released {
        None
    } else {
        crate::paste::take_pending().filter(|t| !t.is_empty())
    };

    if let Some(ref body) = text {
        // 半成品编码串不保留：面板里点一条 = 「这条现在就上屏」，
        // 留着旧编码串会让它跟着文本一起留在文档里。
        eng.clear_composition();
        context.update(|ctx| {
            ctx.suggestion_state = None;
            ctx.commit_text = body.clone();
        });
        window.hide();
        tracing::info!("上屏触发键：提交 {} 字", body.chars().count());
    } else if !released {
        tracing::warn!("上屏触发键到达，但没有待上屏文本（可能已过期或被上一次取走）");
    }

    IpcResponse {
        context: Some(winxime_ipc::Context {
            preedit: winxime_ipc::Text {
                str: String::new(),
            },
            commit: text,
            candidates: winxime_ipc::CandidateInfo::default(),
        }),
        status: Some(get_ipc_status(eng)),
        ..IpcResponse::ok(session_id)
    }
}

pub(crate) fn get_ipc_status(eng: &RimeEngine) -> winxime_ipc::Status {
    let status = eng.get_status();
    winxime_ipc::Status {
        // 按键热路径：这里**不**填 speech（它要查模型目录），留 None；
        // 语音快照只在语音命令的响应里带（见 handle_speech）。
        speech: None,
        composing: eng.is_composing(),
        ascii_mode: status.as_ref().map(|s| s.is_ascii_mode).unwrap_or(false),
        schema_id: status
            .as_ref()
            .map(|s| s.schema_id.clone())
            .unwrap_or_default(),
        schema_name: status
            .as_ref()
            .map(|s| s.schema_name.clone())
            .unwrap_or_default(),
    }
}

fn get_ipc_context(eng: &RimeEngine, commit: &Option<String>) -> Option<winxime_ipc::Context> {
    let composing = eng.is_composing();

    if !composing && commit.is_none() {
        return None;
    }

    let cand_list = eng.get_candidates();

    Some(winxime_ipc::Context {
        preedit: winxime_ipc::Text {
            str: eng.get_input().unwrap_or_default(),
        },
        commit: commit.clone(),
        candidates: winxime_ipc::CandidateInfo {
            current_page: cand_list.page_no as u32,
            total_pages: (if cand_list.is_last_page {
                cand_list.page_no + 1
            } else {
                cand_list.page_no + 2
            }) as u32,
            highlighted: cand_list.highlighted,
            is_last_page: cand_list.is_last_page,
            candies: cand_list
                .candidates
                .iter()
                .map(|c| winxime_ipc::Text {
                    str: c.text.clone(),
                })
                .collect(),
            comments: cand_list
                .candidates
                .iter()
                .map(|c| winxime_ipc::Text {
                    str: c.comment.clone().unwrap_or_default(),
                })
                .collect(),
            labels: Vec::new(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rime_selected_schema_reads_rime_record() {
        // 真实 user.yaml 形态（librime 的 Switcher::SetActiveSchema + SchemaUpdate 写入）。
        let sample = "var:\n  last_build_time: 1790754400\n  previously_selected_schema: wubi86\n  schema_access_time:\n    wubi86: 1790754400\n";
        assert_eq!(
            parse_rime_selected_schema(sample),
            Some("wubi86".to_string())
        );
    }

    #[test]
    fn parse_rime_selected_schema_without_record_is_none() {
        // 老 user.yaml 只有 build 时间：无选中记录。
        assert_eq!(
            parse_rime_selected_schema("var:\n  last_build_time: 1790754400\n"),
            None
        );
        // 空文件 / 非 YAML / 空值一律 None，避免把空方案 id 塞给 librime。
        assert_eq!(parse_rime_selected_schema(""), None);
        assert_eq!(parse_rime_selected_schema("var: ["), None);
        assert_eq!(
            parse_rime_selected_schema("var:\n  previously_selected_schema: '  '\n"),
            None
        );
    }

    /// 语音状态装配：测试进程里语音引擎**没初始化**（OnceLock 空），
    /// 此时也必须给出「结构完整」的快照——设置页据此渲染列表与按钮，
    /// 返回空列表会让页面显示成「没有模型」而不是「服务未就绪」。
    #[test]
    fn speech_status_reports_catalog_even_without_engine() {
        let status = speech_status(None);
        assert_eq!(status.state, "idle");
        assert_eq!(
            status.model_id,
            xime_speech::AsrModelRegistry::default_profile().id,
            "引擎未初始化时按默认模型回答（设置页要能显示名字）"
        );
        assert_eq!(
            status.models_rev,
            crate::speech::download::models_rev(),
            "快照里的模型版本号必须取自下载模块的同一个计数"
        );
        assert!(
            !status.provider.is_empty(),
            "后端说明不能是空串：页面那一行会变成一个空白"
        );
        // 模型列表要读盘（数据根在引擎上），引擎没起来时为空——设置页据
        // 「服务未运行」显示，**不假装有模型可下**（点了只会失败）。
        assert!(
            status.models.is_empty(),
            "引擎未初始化时不该凭空造出模型列表"
        );
        assert!(status.download.is_none(), "没有下载在进行");
    }

    /// 语音命令的错误也要带快照回去（一次往返：错误 + 最新状态）。
    #[test]
    fn speech_command_without_model_id_fails_but_carries_status() {
        let request = winxime_ipc::IpcRequest {
            command: IpcCommand::SpeechDownload,
            session_id: 7,
            data: winxime_ipc::IpcRequestData::None,
        };
        let response = handle_speech(&request);
        assert!(!response.success, "缺模型 id 必须失败");
        assert_eq!(response.session_id, 7);
        let speech = response
            .status
            .and_then(|status| status.speech)
            .expect("语音响应必须带快照");
        assert!(
            speech.error.is_some(),
            "拒绝的原因要能显示给用户"
        );
    }
}


