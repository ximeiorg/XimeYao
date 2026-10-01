use crate::context::SharedInputContext;
use crate::plugins::PluginHost;
use crate::schema_manager::SchemaManager;
use crate::ui::CandidateWindow;
use interprocess::os::windows::named_pipe::{pipe_mode::Bytes, PipeListenerOptions};
use interprocess::os::windows::security_descriptor::SecurityDescriptor;
use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tracing::info;
use widestring::u16cstr;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
use winxime_ipc::{
    get_pipe_path, IpcCommand, IpcRequest, IpcRequestData, IpcResponse, SchemaMarketResponse,
};
use xime_config::XimeConfig;
use xime_rime::RimeEngine;

const MAX_BUFFER_SIZE: usize = 1024 * 1024;

pub fn run_ipc_server(
    engine: Arc<std::sync::Mutex<RimeEngine>>,
    context: Arc<SharedInputContext>,
    window: Arc<CandidateWindow>,
    ascii_mode: Arc<AtomicBool>,
    main_thread_id: u32,
    schema_mgr: Arc<SchemaManager>,
    plugin_host: Arc<PluginHost>,
) {
    let pipe_path = get_pipe_path();
    tracing::info!("Winxime Server: creating named pipe at {}", pipe_path);

    let sd = SecurityDescriptor::deserialize(u16cstr!("D:(A;;GA;;;WD)"))
        .expect("Failed to create security descriptor");

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
                let schema_mgr_clone = schema_mgr.clone();
                let plugin_host_clone = plugin_host.clone();
                std::thread::spawn(move || {
                    handle_connection(
                        p,
                        engine_clone,
                        context_clone,
                        window_clone,
                        ascii_mode_clone,
                        tid,
                        schema_mgr_clone,
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
    schema_mgr: Arc<SchemaManager>,
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
        tracing::info!("Received request: {:?}", request.command);

        let response = process_request(
            &request,
            &engine,
            &context,
            &window,
            &ascii_mode,
            main_thread_id,
            &schema_mgr,
            &plugin_host,
        );

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
            success: true,
            session_id: request.session_id,
            context: None,
            status: None,
            schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: Some(winxime_ipc::SchemaDictResponse {
                dict_name: read.dict_name,
                tables: read.tables,
                missing: read.missing,
                entries: read.entries,
                total: read.total,
                matched: read.matched,
            }),
            phrase_response: None,
        },
        Err(reason) => {
            tracing::error!("读取方案词表失败：{reason}");
            IpcResponse {
                success: false,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
                market_response: None,
                dict_response: None,
                schema_dict_response: None,
                phrase_response: None,
            }
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
            success: true,
            session_id: request.session_id,
            context: None,
            status: None,
            schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: Some(winxime_ipc::CustomPhraseResponse {
                dict_name: read.dict_name,
                file_name: read.file_name,
                file_exists: read.file_exists,
                patch_applied: read.patch_applied,
                patch_added: false,
                entries: read.entries,
            }),
        },
        Err(reason) => {
            tracing::error!("读取快捷短语失败：{reason}");
            IpcResponse {
                success: false,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
                market_response: None,
                dict_response: None,
                schema_dict_response: None,
                phrase_response: None,
            }
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
            success: true,
            session_id: request.session_id,
            context: None,
            status: None,
            schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: Some(winxime_ipc::CustomPhraseResponse {
                dict_name: saved.dict_name,
                file_name: saved.file_name,
                file_exists: saved.file_exists,
                patch_applied: saved.patch_applied,
                patch_added: saved.patch_added,
                entries: saved.entries,
            }),
        },
        Err(reason) => {
            tracing::error!("保存快捷短语失败：{reason}");
            IpcResponse {
                success: false,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
                market_response: None,
                dict_response: None,
                schema_dict_response: None,
                phrase_response: None,
            }
        }
    }
}

fn process_request(
    request: &IpcRequest,
    engine: &Arc<std::sync::Mutex<RimeEngine>>,
    context: &Arc<SharedInputContext>,
    window: &Arc<CandidateWindow>,
    ascii_mode: &Arc<AtomicBool>,
    main_thread_id: u32,
    schema_mgr: &Arc<SchemaManager>,
    plugin_host: &Arc<PluginHost>,
) -> IpcResponse {
    // 不碰引擎的词典文件命令在拿锁之前处理：方案词表首次解析大码表可能要
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
        // 语音命令同样不碰 rime 引擎：语音引擎是独立的全局单例（自己一条工作
        // 线程 + 一个下载线程），拿 rime 引擎锁只会白白挡住打字。下载更是**不能**
        // 在这里等（132MB，IPC 客户端 100ms 静默即超时）——只投命令、立刻回状态，
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
            return IpcResponse {
                success: false,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            };
        }
    };

    match request.command {
        IpcCommand::Echo => IpcResponse {
            success: true,
            session_id: request.session_id,
            context: None,
            status: None,
            schema_list: None,
        market_response: None,
        dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
        },

        IpcCommand::StartSession => {
            tracing::info!("StartSession");
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: Some(get_ipc_status(&eng)),
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::EndSession => {
            tracing::info!("EndSession");
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::FocusIn => {
            tracing::info!("FocusIn");
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: Some(get_ipc_status(&eng)),
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::FocusOut => {
            tracing::info!("FocusOut -> hide composition");
            eng.clear_composition();
            window.hide();
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::ProcessKeyEvent => {
            // 候选栏「剪切板」面板点击后的自注入触发键（见 crate::paste）：
            // 必须在其他分支之前识别——它不参与 rime 组词，也不受英文态影响。
            if let IpcRequestData::KeyEvent(key) = &request.data {
                if key.keycode == crate::paste::XK_PASTE_TRIGGER {
                    return handle_paste_trigger(
                        &mut eng,
                        context,
                        &window,
                        request.session_id,
                        key.modifiers,
                    );
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
                                success: true,
                                session_id: request.session_id,
                                context: Some(winxime_ipc::Context {
                                    preedit: winxime_ipc::Text { str: String::new() },
                                    commit: Some(selected_word.clone()),
                                    candidates: winxime_ipc::CandidateInfo::default(),
                                }),
                                status: Some(get_ipc_status(&eng)),
                                schema_list: None,
                            market_response: None,
                            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                            };
                        } else {
                            context.update(|ctx| {
                                ctx.suggestion_state = None;
                            });
                            window.hide();
                            return IpcResponse {
                                success: false,
                                session_id: request.session_id,
                                context: None,
                                status: Some(get_ipc_status(&eng)),
                                schema_list: None,
                            market_response: None,
                            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
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
                                success: true,
                                session_id: request.session_id,
                                context: Some(winxime_ipc::Context {
                                    preedit: winxime_ipc::Text { str: String::new() },
                                    commit: Some(selected_word.clone()),
                                    candidates: winxime_ipc::CandidateInfo::default(),
                                }),
                                status: Some(get_ipc_status(&eng)),
                                schema_list: None,
                            market_response: None,
                            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                            };
                        } else {
                            return IpcResponse {
                                success: false,
                                session_id: request.session_id,
                                context: None,
                                status: Some(get_ipc_status(&eng)),
                                schema_list: None,
                            market_response: None,
                            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
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
                        success: false,
                        session_id: request.session_id,
                        context: None,
                        status: Some(get_ipc_status(&eng)),
                        schema_list: None,
                    market_response: None,
                    dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                    };
                }
            }

            if is_ascii {
                tracing::info!("  -> ASCII mode, not handling");
                return IpcResponse {
                    success: false,
                    session_id: request.session_id,
                    context: None,
                    status: Some(get_ipc_status(&eng)),
                    schema_list: None,
                market_response: None,
                dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                };
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

            let ipc_ctx = get_ipc_context(&eng, &commit);
            update_context(&mut eng, context, &commit);

            if let Some(ref commit_text) = commit {
                tracing::info!(">>> COMMIT_TO_SCREEN: '{}'", commit_text);

                tracing::info!("  -> hide (commit)");
                context.update(|ctx| {
                    ctx.suggestion_state = None;
                });
                window.hide();
                return IpcResponse {
                    success: handled,
                    session_id: request.session_id,
                    context: ipc_ctx,
                    status: Some(get_ipc_status(&eng)),
                    schema_list: None,
                market_response: None,
                dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                };
            } else if !eng.is_composing() {
                tracing::info!("  -> hide (not composing)");
                context.update(|ctx| {
                    ctx.suggestion_state = None;
                });
                window.hide();
                return IpcResponse {
                    success: handled,
                    session_id: request.session_id,
                    context: ipc_ctx,
                    status: Some(get_ipc_status(&eng)),
                    schema_list: None,
                market_response: None,
                dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                };
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

                return IpcResponse {
                    success: handled,
                    session_id: request.session_id,
                    context: ipc_ctx,
                    status: Some(get_ipc_status(&eng)),
                    schema_list: None,
                market_response: None,
                dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                };
            } else {
                return IpcResponse {
                    success: handled,
                    session_id: request.session_id,
                    context: ipc_ctx,
                    status: Some(get_ipc_status(&eng)),
                    schema_list: None,
                market_response: None,
                dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                };
            }
        }

        IpcCommand::UpdateInputPosition => {
            match &request.data {
                IpcRequestData::Position(pos) => {
                    tracing::info!("Position: {},{}", pos.x, pos.y);
                    context.update(|ctx| {
                        ctx.caret_x = pos.x;
                        ctx.caret_y = pos.y;
                    });
                }
                _ => {}
            }

            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::ShutdownServer => {
            tracing::info!("Shutdown requested, posting WM_QUIT to main thread");
            unsafe {
                let _ = PostThreadMessageW(main_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::ToggleAsciiMode => {
            tracing::info!("ToggleAsciiMode requested");
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

            update_context(&mut eng, &context, &None);

            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: ctx,
                status: Some(get_ipc_status(&eng)),
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::ShowTrayIcon => {
            crate::tray::show_icon();
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::HideTrayIcon => {
            crate::tray::hide_icon();
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::HideCandidates => {
            context.update(|ctx| {
                ctx.is_composing = false;
                ctx.composition.preedit.clear();
                ctx.candidates.clear();
                ctx.commit_text.clear();
            });
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::ReloadConfig => {
            tracing::info!("ReloadConfig requested");
            let deploy_result = eng.redeploy();
            tracing::info!("  redeploy result: {}", deploy_result);
            IpcResponse {
                success: deploy_result,
                session_id: request.session_id,
                context: None,
                status: Some(get_ipc_status(&eng)),
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::ReloadPlugins => {
            tracing::info!("ReloadPlugins requested");
            plugin_host.reload();
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
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
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::ListUserDicts => {
            tracing::info!("ListUserDicts requested");
            let dicts = librime::list_user_dicts();
            let sync_dir = librime::get_user_data_sync_dir();
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: Some(winxime_ipc::DictResponse {
                dicts,
                count: 0,
                sync_dir,
                entries: Vec::new(),
                total: 0,
            }),
            schema_dict_response: None,
            phrase_response: None,
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
            let outcome = match query {
                Some((dict, query)) => crate::user_dict::read_user_dict(&dict, &query),
                None => Err("缺少词典名".to_string()),
            };
            match outcome {
                Ok(read) => IpcResponse {
                    success: true,
                    session_id: request.session_id,
                    context: None,
                    status: None,
                    schema_list: None,
                    market_response: None,
                    dict_response: Some(winxime_ipc::DictResponse {
                        dicts: Vec::new(),
                        count: read.matched,
                        sync_dir: String::new(),
                        entries: read.entries,
                        total: read.total,
                    }),
                    schema_dict_response: None,
                    phrase_response: None,
                },
                Err(reason) => {
                    tracing::error!("读取用户词典词条失败：{reason}");
                    IpcResponse {
                        success: false,
                        session_id: request.session_id,
                        context: None,
                        status: None,
                        schema_list: None,
                        market_response: None,
                        dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                    }
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
                    success: true,
                    session_id: request.session_id,
                    context: None,
                    status: None,
                    schema_list: None,
                    market_response: None,
                    dict_response: Some(winxime_ipc::DictResponse {
                        dicts: Vec::new(),
                        count,
                        sync_dir: String::new(),
                        entries: Vec::new(),
                        total: 0,
                    }),
                    schema_dict_response: None,
                    phrase_response: None,
                },
                Err(reason) => {
                    tracing::error!("写入用户词条失败：{reason}");
                    IpcResponse {
                        success: false,
                        session_id: request.session_id,
                        context: None,
                        status: None,
                        schema_list: None,
                        market_response: None,
                        dict_response: None,
                        schema_dict_response: None,
                        phrase_response: None,
                    }
                }
            }
        }

        IpcCommand::ListSchemaEntries => {
            // 不碰引擎的文件命令（见 process_request 开头的预分发注释）。
            handle_list_schema_entries(request, plugin_host)
        }

        IpcCommand::ListCustomPhrases => handle_list_custom_phrases(request, plugin_host),

        IpcCommand::SaveCustomPhrases => handle_save_custom_phrases(request, plugin_host),

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
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: result.ok().map(|count| winxime_ipc::DictResponse {
                dicts: Vec::new(),
                count,
                sync_dir: String::new(),
                entries: Vec::new(),
                total: 0,
            }),
            schema_dict_response: None,
            phrase_response: None,
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
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
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
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: result.ok().map(|count| winxime_ipc::DictResponse {
                dicts: Vec::new(),
                count,
                sync_dir: String::new(),
                entries: Vec::new(),
                total: 0,
            }),
            schema_dict_response: None,
            phrase_response: None,
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
                context: None,
                status: None,
                schema_list: None,
            market_response: None,
            dict_response: result.ok().map(|count| winxime_ipc::DictResponse {
                dicts: Vec::new(),
                count,
                sync_dir: String::new(),
                entries: Vec::new(),
                total: 0,
            }),
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::ShowToast => {
            // 系统通知由 server 弹（MSIX 包内进程，toast 可归属；
            // 设置进程直跑无包身份，toast 会被系统拒绝/无从归属）。
            let toast = match &request.data {
                winxime_ipc::IpcRequestData::Toast(t) => Some(t.clone()),
                _ => None,
            };
            let success = match toast {
                Some(t) => {
                    crate::toast::show_toast(&t.title, &t.body);
                    true
                }
                None => false,
            };
            IpcResponse {
                success,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
                market_response: None,
                dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
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
                success: true,
                session_id: request.session_id,
                context: None,
                status: Some(get_ipc_status(&eng)),
                schema_list: Some(schema_list),
                market_response: None,
                dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::SelectSchema => {
            tracing::info!("SelectSchema requested");
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
                        tracing::info!("  -> schema selected successfully");
                        IpcResponse {
                            success: true,
                            session_id: request.session_id,
                            context: None,
                            status: Some(get_ipc_status(&eng)),
                            schema_list: None,
                        market_response: None,
                        dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                        }
                    } else {
                        // 拒绝的原因几乎总是「方案未部署」（build/ 无产物）：选进
                        // 未部署方案会得到死会话（所有按键不组词），宁拒不选。
                        // 设置端收到失败后会走「写入方案列表 + 部署」的持久化路径。
                        tracing::info!("  -> schema selection failed: {} 未部署或不存在", id);
                        IpcResponse {
                            success: false,
                            session_id: request.session_id,
                            context: None,
                            status: Some(get_ipc_status(&eng)),
                            schema_list: None,
                        market_response: None,
                        dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                        }
                    }
                }
                None => IpcResponse {
                    success: false,
                    session_id: request.session_id,
                    context: None,
                    status: Some(get_ipc_status(&eng)),
                    schema_list: None,
                market_response: None,
                dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
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
                    let config = XimeConfig::load();
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
                            context: None,
                            status: Some(get_ipc_status(&eng)),
                            schema_list: None,
                        market_response: None,
                        dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                        }
                    } else {
                        tracing::warn!("  -> no root for key '{}' in schema '{}'", c, schema_id);
                        IpcResponse {
                            success: false,
                            session_id: request.session_id,
                            context: None,
                            status: Some(get_ipc_status(&eng)),
                            schema_list: None,
                        market_response: None,
                        dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                        }
                    }
                }
                None => {
                    tracing::info!("  -> no letter provided");
                    IpcResponse {
                        success: false,
                        session_id: request.session_id,
                        context: None,
                        status: Some(get_ipc_status(&eng)),
                        schema_list: None,
                    market_response: None,
                    dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
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
                success: true,
                session_id: request.session_id,
                context: None,
                status: Some(get_ipc_status(&eng)),
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::SelectCandidate => {
            tracing::info!("SelectCandidate requested");
            let index = match &request.data {
                winxime_ipc::IpcRequestData::SelectIndex(i) => *i,
                _ => 0,
            };

            tracing::info!("  -> selecting candidate at index {}", index);
            let selected = eng.select_candidate(index);
            tracing::info!("  -> select result: {}", selected);

            let commit = eng.get_commit();
            tracing::info!("  -> commit: {:?}", commit);

            let ipc_ctx = get_ipc_context(&eng, &commit);
            update_context(&mut eng, context, &commit);

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
                session_id: request.session_id,
                context: ipc_ctx,
                status: Some(get_ipc_status(&eng)),
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::ChangePage => {
            tracing::info!("ChangePage requested");
            let backward = match &request.data {
                winxime_ipc::IpcRequestData::ChangePage(b) => *b,
                _ => false,
            };

            tracing::info!("  -> backward: {}", backward);
            let changed = eng.change_page(backward);
            tracing::info!("  -> change result: {}", changed);

            let ipc_ctx = get_ipc_context(&eng, &None);
            if let Some(ctx) = &ipc_ctx {
                let pos = context.read(|c| (c.caret_x, c.caret_y));
                window.show(pos.0, pos.1);
                window.update(ctx);
            }

            IpcResponse {
                success: changed,
                session_id: request.session_id,
                context: ipc_ctx,
                status: Some(get_ipc_status(&eng)),
                schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::FetchSchemaIndex => {
            tracing::info!("FetchSchemaIndex requested");
            match schema_mgr.fetch_index() {
                Ok(text) => IpcResponse {
                    success: true,
                    session_id: request.session_id,
                    context: None,
                    status: None,
                    schema_list: None,
                    market_response: Some(SchemaMarketResponse::Index(text)),
                    dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                },
                Err(e) => IpcResponse {
                    success: false,
                    session_id: request.session_id,
                    context: None,
                    status: None,
                    schema_list: None,
                    market_response: Some(SchemaMarketResponse::Error(e)),
                    dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                },
            }
        }

        IpcCommand::DownloadSchema => {
            tracing::info!("DownloadSchema requested");
            let dl = match &request.data {
                winxime_ipc::IpcRequestData::SchemaDownload(d) => d,
                _ => {
                    return IpcResponse {
                        success: false,
                        session_id: request.session_id,
                        context: None,
                        status: None,
                        schema_list: None,
                        market_response: Some(SchemaMarketResponse::Error(
                            "无效的请求数据".to_string(),
                        )),
                        dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                    }
                }
            };
            let result = schema_mgr.download_schema(
                &dl.schema_id,
                &dl.url,
                dl.sha256.as_deref(),
                &dl.filename,
            );
            match result {
                Ok(()) => IpcResponse {
                    success: true,
                    session_id: request.session_id,
                    context: None,
                    status: None,
                    schema_list: None,
                    market_response: Some(SchemaMarketResponse::DownloadDone(
                        dl.schema_id.clone(),
                    )),
                    dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                },
                Err(e) => IpcResponse {
                    success: false,
                    session_id: request.session_id,
                    context: None,
                    status: None,
                    schema_list: None,
                    market_response: Some(SchemaMarketResponse::Error(e)),
                    dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                },
            }
        }

        IpcCommand::InstallSchema => {
            tracing::info!("InstallSchema requested");
            let sid = match &request.data {
                winxime_ipc::IpcRequestData::SchemaInstall(d) => &d.schema_id,
                _ => {
                    return IpcResponse {
                        success: false,
                        session_id: request.session_id,
                        context: None,
                        status: None,
                        schema_list: None,
                        market_response: Some(SchemaMarketResponse::Error(
                            "无效的请求数据".to_string(),
                        )),
                        dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                    }
                }
            };
            match schema_mgr.install_schema(sid) {
                Ok(()) => IpcResponse {
                    success: true,
                    session_id: request.session_id,
                    context: None,
                    status: None,
                    schema_list: None,
                    market_response: Some(SchemaMarketResponse::InstallDone(sid.clone())),
                    dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                },
                Err(e) => IpcResponse {
                    success: false,
                    session_id: request.session_id,
                    context: None,
                    status: None,
                    schema_list: None,
                    market_response: Some(SchemaMarketResponse::Error(e)),
                    dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                },
            }
        }

        IpcCommand::UninstallSchema => {
            tracing::info!("UninstallSchema requested");
            let sid = match &request.data {
                winxime_ipc::IpcRequestData::SchemaUninstall(d) => &d.schema_id,
                _ => {
                    return IpcResponse {
                        success: false,
                        session_id: request.session_id,
                        context: None,
                        status: None,
                        schema_list: None,
                        market_response: Some(SchemaMarketResponse::Error(
                            "无效的请求数据".to_string(),
                        )),
                        dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                    }
                }
            };
            match schema_mgr.uninstall_schema(sid) {
                Ok(()) => IpcResponse {
                    success: true,
                    session_id: request.session_id,
                    context: None,
                    status: None,
                    schema_list: None,
                    market_response: Some(SchemaMarketResponse::UninstallDone(sid.clone())),
                    dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                },
                Err(e) => IpcResponse {
                    success: false,
                    session_id: request.session_id,
                    context: None,
                    status: None,
                    schema_list: None,
                    market_response: Some(SchemaMarketResponse::Error(e)),
                    dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
                },
            }
        }

        IpcCommand::ListMarketSchemas => {
            tracing::info!("ListMarketSchemas requested");
            let packages = schema_mgr.list_market_schemas();
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
                market_response: Some(SchemaMarketResponse::PackageList(packages)),
                dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        IpcCommand::ListInstalledPackages => {
            tracing::info!("ListInstalledPackages requested");
            let packages = schema_mgr.list_installed_packages();
            IpcResponse {
                success: true,
                session_id: request.session_id,
                context: None,
                status: None,
                schema_list: None,
                market_response: Some(SchemaMarketResponse::InstalledList(packages)),
                dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
            }
        }

        _ => IpcResponse {
            success: false,
            session_id: request.session_id,
            context: None,
            status: None,
            schema_list: None,
            market_response: None,
            dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
        },
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
        assert!(status.models_rev >= 0);
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

/// 处理候选栏面板的「点击上屏」触发键（server 自己 `SendInput` 注入的 VK_F24，
/// 见 [`crate::paste`]）。
///
/// - key down：取走待上屏文本，清掉当前编码串，把文本作为 `commit` 回包——
///   宿主拿到 commit 会用提交文本替换掉当前 composition 并结束它，于是文本
///   经正常的 TSF 编辑会话落到光标处（`success: true` 同时让宿主吃掉这个键）。
/// - key up：照样回 `success: true`（不漏给前台应用），但不带 commit。
fn handle_paste_trigger(
    eng: &mut RimeEngine,
    context: &SharedInputContext,
    window: &CandidateWindow,
    session_id: u32,
    modifiers: i32,
) -> IpcResponse {
    let released = modifiers & librime::K_RELEASE_MASK as i32 != 0;
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
        success: true,
        session_id,
        context: Some(winxime_ipc::Context {
            preedit: winxime_ipc::Text {
                str: String::new(),
            },
            commit: text,
            candidates: winxime_ipc::CandidateInfo::default(),
        }),
        status: Some(get_ipc_status(eng)),
        schema_list: None,
        market_response: None,
        dict_response: None,
            schema_dict_response: None,
            phrase_response: None,
    }
}

fn get_ipc_status(eng: &RimeEngine) -> winxime_ipc::Status {
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


