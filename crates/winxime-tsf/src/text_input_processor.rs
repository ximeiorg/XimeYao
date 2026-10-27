use librime::{get_key_modifiers, K_ALT_MASK, K_CONTROL_MASK, K_SHIFT_MASK};
use std::sync::Arc;
use tracing::{debug, info};
use windows::Win32::Foundation::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::TextServices::*;
use windows_core::{Interface, *};
use winxime_ipc::{
    IpcClient, IpcCommand, IpcRequest, IpcRequestData, IpcResponse, KeyEventData, Position,
};

const TF_INVALID_COOKIE: u32 = 0xFFFFFFFF;

const VK_X_A: u16 = 0x41;
const VK_X_Z: u16 = 0x5A;
// 移位字符映射用（vk_to_xk_shifted）；上游 librime::vk_to_xk 暂无 shift 参数
const VK_OEM_1: u16 = 0xBA;
const VK_OEM_7: u16 = 0xDE;
const VK_OEM_4: u16 = 0xDB;
const VK_OEM_6: u16 = 0xDD;
const VK_OEM_COMMA: u16 = 0xBC;
const VK_OEM_PERIOD: u16 = 0xBE;
const VK_OEM_MINUS: u16 = 0xBD;
const VK_OEM_PLUS: u16 = 0xBB;
const VK_OEM_2: u16 = 0xBF;
const VK_OEM_5: u16 = 0xDC;
const VK_OEM_3: u16 = 0xC0;

const GUID_LBI_INPUTMODE: GUID = GUID::from_u128(0x5D5D8287_5B53_4DAA_B44C_52EB4794A3E7);

struct IpcState {
    client: Option<IpcClient>,
    session_id: u32,
}

pub struct IpcClientHandle {
    state: Arc<std::sync::Mutex<IpcState>>,
}

impl IpcClientHandle {
    pub fn debug_ptr(&self) -> *const () {
        Arc::as_ptr(&self.state) as *const ()
    }

    pub fn new() -> std::result::Result<Self, winxime_ipc::IpcError> {
        let client = IpcClient::connect()?;
        Ok(Self {
            state: Arc::new(std::sync::Mutex::new(IpcState {
                client: Some(client),
                session_id: 0,
            })),
        })
    }

    pub fn empty() -> Self {
        Self {
            state: Arc::new(std::sync::Mutex::new(IpcState {
                client: None,
                session_id: 0,
            })),
        }
    }

    pub fn connect(&self) -> std::result::Result<(), winxime_ipc::IpcError> {
        debug!("IpcClientHandle::connect() called");
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                debug!("  -> lock would block, returning error");
                return Err(winxime_ipc::IpcError::ConnectionFailed(
                    "lock would block".to_string(),
                ));
            }
        };
        if guard.client.is_some() {
            debug!("  -> already connected");
            return Ok(());
        }
        debug!("  -> calling IpcClient::connect()");
        let client = IpcClient::connect()?;
        debug!("  -> IpcClient::connect() succeeded");
        guard.client = Some(client);
        Ok(())
    }

    pub fn disconnect(&self) {
        debug!("IpcClientHandle::disconnect() called");
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return,
        };
        guard.client = None;
        guard.session_id = 0;
    }

    pub fn is_connected(&self) -> bool {
        // 出过一次错（超时 / 管道断）之后 `IpcClient` 会自己作废，这里一并反映出来：
        // 下一次按键就会走重连，而不是先白丢一个键。
        match self.state.try_lock() {
            Ok(g) => g.client.as_ref().is_some_and(|client| client.is_usable()),
            Err(std::sync::TryLockError::Poisoned(e)) => {
                e.into_inner().client.as_ref().is_some_and(|c| c.is_usable())
            }
            Err(std::sync::TryLockError::WouldBlock) => false,
        }
    }

    pub fn start_session(&self) -> (u32, Option<IpcResponse>) {
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => {
                debug!("start_session: lock failed");
                return (0, None);
            }
        };
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::StartSession,
                session_id: 0,
                data: IpcRequestData::None,
            };
            debug!("start_session: sending request, session_id=0");
            match client.send_request(&request) {
                Ok(response) => {
                    debug!(
                        "start_session: got response, session_id={}, ascii_mode={}",
                        response.session_id,
                        response
                            .status
                            .as_ref()
                            .map(|s| s.ascii_mode)
                            .unwrap_or(false)
                    );
                    guard.session_id = response.session_id;
                    return (guard.session_id, Some(response));
                }
                Err(e) => {
                    debug!("start_session: send_request FAILED: {:?}", e);
                    guard.client = None;
                    guard.session_id = 0;
                }
            }
        } else {
            debug!("start_session: no client");
        }
        (guard.session_id, None)
    }

    pub fn process_key(&self, keycode: i32, modifiers: i32) -> Option<IpcResponse> {
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => {
                debug!("process_key: lock failed, returning None");
                return None;
            }
        };
        let session_id = guard.session_id;
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::ProcessKeyEvent,
                session_id,
                data: IpcRequestData::KeyEvent(KeyEventData { keycode, modifiers }),
            };
            match client.send_request(&request) {
                Ok(response) => {
                    debug!("process_key: got response, success={}", response.success);
                    return Some(response);
                }
                Err(e) => {
                    debug!("process_key: send_request FAILED: {:?}", e);
                    guard.client = None;
                    guard.session_id = 0;
                }
            }
        } else {
            debug!("process_key: no client");
        }
        None
    }

    pub fn select_candidate(&self, index: usize) -> Option<IpcResponse> {
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => return None,
        };
        let session_id = guard.session_id;
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::SelectCandidate,
                session_id,
                data: IpcRequestData::SelectIndex(index),
            };
            client.send_request(&request).ok()
        } else {
            None
        }
    }

    pub fn change_page(&self, backward: bool) -> Option<IpcResponse> {
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => return None,
        };
        let session_id = guard.session_id;
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::ChangePage,
                session_id,
                data: IpcRequestData::ChangePage(backward),
            };
            client.send_request(&request).ok()
        } else {
            None
        }
    }

    pub fn update_position(&self, x: i32, y: i32) {
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let session_id = guard.session_id;
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::UpdateInputPosition,
                session_id,
                data: IpcRequestData::Position(Position { x, y }),
            };
            let _ = client.send_no_reply(&request);
        }
    }

    pub fn toggle_ascii_mode(&self) -> Option<IpcResponse> {
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => return None,
        };
        let session_id = guard.session_id;
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::ToggleAsciiMode,
                session_id,
                data: IpcRequestData::None,
            };
            client.send_request(&request).ok()
        } else {
            None
        }
    }

    pub fn focus_in(&self) {
        debug!("IPC::focus_in() called");
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => {
                debug!("  -> lock failed, skipping focus_in");
                return;
            }
        };
        let session_id = guard.session_id;
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::FocusIn,
                session_id,
                data: IpcRequestData::None,
            };
            debug!("  -> sending FocusIn request (session_id={})", session_id);
            if client.send_oneway(&request).is_ok() {
                debug!("  -> FocusIn sent successfully");
            } else {
                debug!("  -> FocusIn send FAILED");
            }
        } else {
            debug!("  -> no client, cannot send FocusIn");
        }
    }

    pub fn focus_out(&self) -> Option<IpcResponse> {
        debug!("IPC::focus_out() called");
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => {
                debug!("  -> lock failed, skipping focus_out");
                return None;
            }
        };
        let session_id = guard.session_id;
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::FocusOut,
                session_id,
                data: IpcRequestData::None,
            };
            debug!("  -> sending FocusOut request (session_id={})", session_id);
            match client.send_request(&request) {
                Ok(response) => {
                    debug!("  -> FocusOut response received");
                    Some(response)
                }
                Err(e) => {
                    debug!("  -> FocusOut send FAILED: {:?}", e);
                    None
                }
            }
        } else {
            debug!("  -> no client, cannot send FocusOut");
            None
        }
    }

    /// 把当前 TIP 实例认领为托盘 owner（只在语言产品激活/激活时调用）。
    pub fn show_tray_icon(&self, session_id: u32) {
        debug!("IPC::show_tray_icon(session_id={session_id})");
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::ShowTrayIcon,
                session_id,
                data: IpcRequestData::None,
            };
            let _ = client.send_oneway(&request);
        }
    }

    /// 只有 owner 才会真正隐藏服务端托盘；非 owner 的调用由 server 忽略。
    pub fn hide_tray_icon(&self, session_id: u32) {
        debug!("IPC::hide_tray_icon(session_id={session_id})");
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::HideTrayIcon,
                session_id,
                data: IpcRequestData::None,
            };
            let _ = client.send_oneway(&request);
        }
    }

    pub fn hide_candidates(&self) {
        debug!("IPC::hide_candidates() called");
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let session_id = guard.session_id;
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::HideCandidates,
                session_id,
                data: IpcRequestData::None,
            };
            debug!(
                "  -> sending HideCandidates request (session_id={})",
                session_id
            );
            if client.send_oneway(&request).is_ok() {
                debug!("  -> HideCandidates sent successfully");
            } else {
                debug!("  -> HideCandidates send FAILED");
            }
        } else {
            debug!("  -> no client");
        }
    }

    pub fn show_root(&self, letter: char) {
        debug!("IPC::show_root() called, letter={}", letter);
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let session_id = guard.session_id;
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::ShowRoot,
                session_id,
                data: IpcRequestData::ShowRoot(letter),
            };
            debug!("  -> sending ShowRoot request");
            if client.send_oneway(&request).is_ok() {
                debug!("  -> ShowRoot sent successfully");
            } else {
                debug!("  -> ShowRoot send FAILED");
            }
        } else {
            debug!("  -> no client");
        }
    }

    pub fn hide_root(&self) {
        debug!("IPC::hide_root() called");
        let mut guard = match self.state.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let session_id = guard.session_id;
        if let Some(ref mut client) = guard.client {
            let request = IpcRequest {
                command: IpcCommand::HideRoot,
                session_id,
                data: IpcRequestData::None,
            };
            debug!("  -> sending HideRoot request");
            if client.send_oneway(&request).is_ok() {
                debug!("  -> HideRoot sent successfully");
            } else {
                debug!("  -> HideRoot send FAILED");
            }
        } else {
            debug!("  -> no client");
        }
    }
}

impl Clone for IpcClientHandle {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct RimeOutput {
    commit: Option<String>,
    preedit: String,
    _candidates: Vec<String>,
    composing: bool,
}

impl RimeOutput {
    fn from_response(response: &IpcResponse) -> Self {
        let ctx = response.context.as_ref();
        let commit = ctx.and_then(|c| c.commit.clone());
        debug!(
            "RimeOutput::from_response: context={}, commit={:?}, preedit='{}'",
            ctx.is_some(),
            commit,
            ctx.map(|c| c.preedit.str.clone()).unwrap_or_default()
        );
        Self {
            commit,
            preedit: ctx.map(|c| c.preedit.str.clone()).unwrap_or_default(),
            _candidates: ctx
                .map(|c| c.candidates.candies.iter().map(|t| t.str.clone()).collect())
                .unwrap_or_default(),
            composing: response
                .status
                .as_ref()
                .map(|s| s.composing)
                .unwrap_or(false),
        }
    }
}

#[implement(ITfEditSession)]
struct XimeEditSession {
    output: RimeOutput,
    thread_mgr: Option<ITfThreadMgr>,
    /// 会话要写哪个 context。正常按键路径传当前 focus；失焦 flush 传
    /// `pdimprevfocus` 的旧 context——此时 `thread_mgr.GetFocus()` 已经是新窗口。
    target_context: Option<ITfContext>,
    /// 失焦 flush 专用：如果排到编辑会话时组词串已经被 TSF 终止（焦点变化时
    /// `OnCompositionTerminated` 会先跑，且文本通常留在文档里），就不要再插入
    /// 一遍 commit，否则会出现两份数字。
    flush_commit: bool,
    composition: Arc<std::sync::Mutex<Option<ITfComposition>>>,
    composition_sink: ITfCompositionSink,
    /// 编辑会话里量到的插入点坐标。**不在会话里发 IPC**：TSF 编辑会话期间拿着
    /// 文档写锁，同步等另一个进程（最坏 1 秒）会把宿主（Office / 浏览器）一起拖住。
    /// 会话结束后由调用方取走再发。
    position: Arc<std::sync::Mutex<Option<(i32, i32)>>>,
}

#[implement(ITfCompositionSink)]
struct CompositionSink {
    composition: Arc<std::sync::Mutex<Option<ITfComposition>>>,
}

impl ITfCompositionSink_Impl for CompositionSink_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _pcomposition: Ref<'_, ITfComposition>,
    ) -> Result<()> {
        debug!("OnCompositionTerminated: composition terminated");
        match self.composition.try_lock() {
            Ok(mut guard) => *guard = None,
            Err(std::sync::TryLockError::Poisoned(e)) => *e.into_inner() = None,
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        Ok(())
    }
}

impl ITfEditSession_Impl for XimeEditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        debug!(
            "DoEditSession: ec={}, commit={}, composing={}, preedit='{}'",
            ec,
            self.output.commit.is_some(),
            self.output.composing,
            self.output.preedit
        );

        let context = match self.target_context.as_ref() {
            Some(target) => target.clone(),
            None => {
                let doc_mgr = match self.thread_mgr.as_ref() {
                    Some(t) => unsafe { t.GetFocus() }?,
                    None => {
                        debug!("DoEditSession: no thread_mgr and no target context");
                        return Ok(());
                    }
                };
                unsafe { doc_mgr.GetBase() }?
            }
        };
        debug!("DoEditSession: got context");

        // Handle commit and composition (weasel pattern)
        let commit_text = self.output.commit.clone().unwrap_or_default();
        let preedit_text = self.output.preedit.clone();

        if !commit_text.is_empty() {
            debug!("DoEditSession: COMMIT '{}'", commit_text);

            // If not composing, start a composition for the commit
            let has_comp = match self.composition.try_lock() {
                Ok(g) => g.is_some(),
                Err(_) => false,
            };
            if self.flush_commit && !has_comp {
                // 失焦 flush 的组词串已经被 TSF 终止：终止时文本留在文档里，
                // 这里再插入就是第二份。直接跳过。
                debug!("DoEditSession: flush skipped (composition already terminated)");
                return Ok(());
            }
            if !has_comp {
                debug!("DoEditSession: creating composition for commit");
                self.start_composition(&context, ec, "");
            }

            // Set commit text and end composition (clear=false to keep text)
            self.update_composition_text(&context, ec, &commit_text);
            self.end_composition(ec);
            debug!("DoEditSession: commit done");
        }

        // Handle composing state changes and inline preedit update
        let has_comp = match self.composition.try_lock() {
            Ok(g) => g.is_some(),
            Err(_) => false,
        };
        if self.output.composing && !has_comp {
            debug!("DoEditSession: start composition '{}'", preedit_text);
            self.start_composition(&context, ec, &preedit_text);
        } else if self.output.composing && !preedit_text.is_empty() {
            debug!(
                "DoEditSession: update composition text '{}' (has_comp={})",
                preedit_text, has_comp
            );
            self.update_composition_text(&context, ec, &preedit_text);
        } else if !self.output.composing && has_comp {
            debug!("DoEditSession: clear text and end composition");
            self.update_composition_text(&context, ec, "");
            self.end_composition(ec);
        }

        Ok(())
    }
}

impl XimeEditSession_Impl {
    /// 在编辑会话里量插入点坐标（`GetTextExt` 需要有效的 `ec`，只能在会话内做），
    /// 但**不在会话里发 IPC**——同步等待另一个进程期间，文档写锁一直握在我们手里。
    fn caret_position_in_session(context: &ITfContext, ec: u32) -> Option<(i32, i32)> {
        use std::mem::ManuallyDrop;
        use std::ops::Deref;

        unsafe {
            let mut selection = [TF_SELECTION::default(); 1];
            let mut selection_len = 0;
            if context
                .GetSelection(ec, TF_DEFAULT_SELECTION, &mut selection, &mut selection_len)
                .is_ok()
            {
                let position = if let Some(sel_range) = selection[0].range.deref() {
                    if let Ok(view) = context.GetActiveView() {
                        let mut rc = RECT::default();
                        let mut clipped = BOOL::default();
                        if view
                            .GetTextExt(ec, sel_range, &mut rc, &mut clipped)
                            .is_ok()
                        {
                            debug!(
                                "caret_position_in_session: left={}, bottom={}",
                                rc.left, rc.bottom
                            );
                            Some((rc.left, rc.bottom))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                let [TF_SELECTION { range, .. }] = selection;
                ManuallyDrop::into_inner(range);
                position
            } else {
                None
            }
        }
    }

    fn start_composition(&self, context: &ITfContext, ec: u32, preedit: &str) {
        debug!("start_composition: preedit='{}'", preedit);
        use windows::Win32::UI::TextServices::{ITfInsertAtSelection, TF_IAS_QUERYONLY};

        let insert_at_selection: ITfInsertAtSelection = match context.cast() {
            Ok(s) => s,
            Err(e) => {
                debug!(
                    "start_composition: cast ITfInsertAtSelection failed: {:?}",
                    e
                );
                return;
            }
        };

        let ctx_comp: ITfContextComposition = match context.cast() {
            Ok(c) => c,
            Err(e) => {
                debug!(
                    "start_composition: cast ITfContextComposition failed: {:?}",
                    e
                );
                return;
            }
        };

        unsafe {
            let range = match insert_at_selection.InsertTextAtSelection(ec, TF_IAS_QUERYONLY, &[]) {
                Ok(r) => r,
                Err(e) => {
                    debug!("start_composition: InsertTextAtSelection failed: {:?}", e);
                    return;
                }
            };
            debug!("start_composition: got empty range");

            match ctx_comp.StartComposition(ec, &range, Some(&self.composition_sink)) {
                Ok(comp) => {
                    debug!("start_composition: StartComposition succeeded");
                    let comp_range = match comp.GetRange() {
                        Ok(r) => r,
                        Err(e) => {
                            debug!("start_composition: GetRange failed: {:?}", e);
                            return;
                        }
                    };

                    let wide: Vec<u16> = preedit.encode_utf16().collect();
                    debug!("start_composition: setting text {} chars", wide.len());
                    match comp_range.SetText(ec, 0, &wide) {
                        Ok(_) => {
                            debug!("start_composition: SetText succeeded, weasel mode");
                            // Weasel: collapse to end, set selection with TF_AE_NONE
                            comp_range.Collapse(ec, TF_ANCHOR_END).ok();
                            use std::mem::ManuallyDrop;
                            let sel = TF_SELECTION {
                                range: ManuallyDrop::new(Some(comp_range)),
                                style: TF_SELECTIONSTYLE {
                                    ase: TF_AE_NONE,
                                    fInterimChar: FALSE,
                                },
                            };
                            context.SetSelection(ec, &[sel]).ok();

                            match self.composition.try_lock() {
                                Ok(mut guard) => *guard = Some(comp),
                                Err(std::sync::TryLockError::Poisoned(e)) => {
                                    *e.into_inner() = Some(comp)
                                }
                                Err(std::sync::TryLockError::WouldBlock) => {
                                    // End the composition we just created since we can't track it
                                    comp.EndComposition(ec).ok();
                                }
                            }

                            if let Some(position) =
                                Self::caret_position_in_session(context, ec)
                            {
                                // 只记下来，IPC 由 schedule_edit_session 在会话外发。
                                match self.position.lock() {
                                    Ok(mut slot) => *slot = Some(position),
                                    Err(poisoned) => *poisoned.into_inner() = Some(position),
                                }
                            }
                        }
                        Err(e) => {
                            debug!("start_composition: SetText failed: {:?}", e);
                        }
                    }
                }
                Err(e) => {
                    debug!("start_composition: StartComposition failed: {:?}", e);
                }
            }
        }
    }

    fn update_composition_text(&self, context: &ITfContext, ec: u32, preedit: &str) {
        debug!("update_composition_text: preedit='{}'", preedit);
        let comp = match self.composition.try_lock() {
            Ok(g) => g,
            Err(_) => {
                debug!("update_composition_text: lock failed");
                return;
            }
        };
        if let Some(ref composition) = *comp {
            unsafe {
                if let Ok(range) = composition.GetRange() {
                    let wide: Vec<u16> = preedit.encode_utf16().collect();
                    match range.SetText(ec, 0, &wide) {
                        Ok(_) => {
                            debug!("update_composition_text: SetText succeeded");
                            // Weasel: collapse to end, set selection with TF_AE_NONE
                            range.Collapse(ec, TF_ANCHOR_END).ok();
                            use std::mem::ManuallyDrop;
                            let sel = TF_SELECTION {
                                range: ManuallyDrop::new(Some(range)),
                                style: TF_SELECTIONSTYLE {
                                    ase: TF_AE_NONE,
                                    fInterimChar: FALSE,
                                },
                            };
                            context.SetSelection(ec, &[sel]).ok();
                        }
                        Err(e) => debug!("update_composition_text: SetText failed: {:?}", e),
                    }
                } else {
                    debug!("update_composition_text: GetRange failed");
                }
            }
        } else {
            debug!("update_composition_text: no composition");
        }
    }

    fn end_composition(&self, ec: u32) {
        match self.composition.try_lock() {
            Ok(mut guard) => {
                if let Some(comp) = guard.take() {
                    unsafe {
                        comp.EndComposition(ec).ok();
                    }
                }
            }
            Err(std::sync::TryLockError::Poisoned(e)) => {
                if let Some(comp) = e.into_inner().take() {
                    unsafe {
                        comp.EndComposition(ec).ok();
                    }
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                // Lock held by another thread - skip to avoid deadlock
            }
        }
    }
}

impl XimeTextService_Impl {
    fn is_composing(&self) -> bool {
        self.composing.get()
    }

    fn set_composing(&self, val: bool) {
        self.composing.set(val);
    }

    fn update_lang_bar(&self) {
        let sink = match self.lang_bar_sink_ref.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(ref sink) = *sink {
            unsafe {
                let _ = sink.OnUpdate(TF_LBI_STATUS | TF_LBI_ICON | TF_LBI_TEXT);
            }
        }
    }

    fn update_caret_position_sync(&self, context: &ITfContext) {
        debug!("update_caret_position_sync: getting caret rect");

        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::TextServices::{
            ITfEditSession, TF_DEFAULT_SELECTION, TF_ES_ASYNCDONTCARE, TF_ES_READ,
        };

        #[implement(ITfEditSession)]
        struct SelectionRect {
            context: ITfContext,
            rect: std::cell::Cell<RECT>,
        }

        impl SelectionRect {
            fn new(context: ITfContext) -> Self {
                Self {
                    context,
                    rect: std::cell::Cell::default(),
                }
            }
            fn rect(&self) -> RECT {
                self.rect.get()
            }
        }

        impl ITfEditSession_Impl for SelectionRect_Impl {
            fn DoEditSession(&self, ec: u32) -> Result<()> {
                use std::mem::ManuallyDrop;
                use std::ops::Deref;

                unsafe {
                    let mut selection = [TF_SELECTION::default(); 1];
                    let mut selection_len = 0;
                    self.context.GetSelection(
                        ec,
                        TF_DEFAULT_SELECTION,
                        &mut selection,
                        &mut selection_len,
                    )?;

                    if let Some(sel_range) = selection[0].range.deref() {
                        let view = self.context.GetActiveView()?;
                        let mut rc = RECT::default();
                        let mut clipped = BOOL::default();
                        view.GetTextExt(ec, sel_range, &mut rc, &mut clipped)?;
                        debug!(
                            "SelectionRect: left={}, top={}, right={}, bottom={}",
                            rc.left, rc.top, rc.right, rc.bottom
                        );
                        self.rect.set(rc);
                    }

                    let [TF_SELECTION { range, .. }] = selection;
                    ManuallyDrop::into_inner(range);
                }
                Ok(())
            }
        }

        let tid = self.client_id.get();
        let session = SelectionRect::new(context.clone()).into_object();

        let _ = unsafe {
            context.RequestEditSession(
                tid,
                session.as_interface(),
                TF_ES_ASYNCDONTCARE | TF_ES_READ,
            )
        };

        let rect = session.rect();
        debug!(
            "update_caret_position_sync: left={}, bottom={}",
            rect.left, rect.bottom
        );

        if self.ipc.is_connected() {
            self.ipc.update_position(rect.left, rect.bottom);
            debug!("update_caret_position_sync: sent to server");
        }
    }

    fn schedule_edit_session(&self, context: &ITfContext, output: RimeOutput) -> bool {
        self.schedule_edit_session_impl(context, output, false)
    }

    /// 失焦 flush：如果组词串在会话执行前已被终止，DoEditSession 会跳过提交。
    fn schedule_flush_edit_session(&self, context: &ITfContext, output: RimeOutput) -> bool {
        self.schedule_edit_session_impl(context, output, true)
    }

    fn schedule_edit_session_impl(
        &self,
        context: &ITfContext,
        output: RimeOutput,
        flush_commit: bool,
    ) -> bool {
        debug!("schedule_edit_session: called (flush={flush_commit})");
        let tid = self.client_id.get();
        let mgr = self.thread_mgr.borrow().clone();

        let composition_sink_impl = CompositionSink {
            composition: self.composition.clone(),
        };
        let composition_sink: ITfCompositionSink = composition_sink_impl.into();

        let position = Arc::new(std::sync::Mutex::new(None));
        let session = XimeEditSession {
            output,
            thread_mgr: mgr,
            target_context: Some(context.clone()),
            flush_commit,
            composition: self.composition.clone(),
            composition_sink,
            position: position.clone(),
        };
        let session_itf: ITfEditSession = session.into();
        debug!(
            "schedule_edit_session: tid={}, calling RequestEditSession",
            tid
        );
        let ok = unsafe {
            let result = context.RequestEditSession(
                tid,
                &session_itf,
                TF_ES_ASYNCDONTCARE | TF_ES_READWRITE,
            );
            debug!(
                "schedule_edit_session: RequestEditSession result: {:?}",
                result
            );
            result.is_ok()
        };

        // 会话结束后再发插入点。会话内发 IPC 会让文档写锁跟着一起等另一个进程。
        // 会话被排到异步执行时这里取不到坐标——下一位按键的
        // `update_caret_position_sync` 会在会话外补一个更新的位置，不会错位。
        self.send_pending_position(&position);
        ok
    }

    /// 把编辑会话量到的插入点发给 server（没量到就不发）。
    fn send_pending_position(&self, position: &Arc<std::sync::Mutex<Option<(i32, i32)>>>) {
        let pending = match position.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some((x, y)) = pending {
            if self.ipc.is_connected() {
                self.ipc.update_position(x, y);
            }
        }
    }

    /// 统一的按键处理（对齐 weasel KeyHandler 架构）：把键经 IPC 交给 rime
    /// 裁决，rime 吃下（response.success）才算认领。调用时机在 OnTestKeyDown /
    /// OnTestKeyUp（OnKeyDown/OnKeyUp 只重放结果，见 pending 标志），因此
    /// 不存在"认领后 rime 拒绝导致丢键"的缺口——rime 拒绝的键（Ctrl+A、
    /// 不组词的回车/空格/数字等）天然交还应用原生处理。
    /// 修饰键本身不经 rime：Shift 中英切换由本层 shift_solo 机制处理
    /// （OnKeyUp），Ctrl 单按由 show_root 逻辑处理（OnKeyDown）。
    fn process_key_event(
        &self,
        context: Option<&ITfContext>,
        vk: VIRTUAL_KEY,
        is_up: bool,
    ) -> bool {
        let code = vk.0;
        debug!("process_key_event: vk={} is_up={}", code, is_up);

        // 服务进程自身的实例不处理任何按键（见 `self_host` 字段注释）。
        if self.self_host.get() {
            return false;
        }

        if code == VK_SHIFT.0 || code == VK_CONTROL.0 || code == VK_MENU.0 {
            return false;
        }

        // F1–F23 永远不归输入法：直接交还宿主，避免 TSF 在“被 rime 拒绝”
        // 路径上顺手收 composition，把 F2 重命名/F5 刷新吞掉。
        // F24（0x87）是内部上屏触发键，不能在这里放行。
        if is_function_passthrough(code) {
            debug!("  -> function key, pass through to the app");
            return false;
        }

        // 英文态不询问 rime（对齐 weasel 的 keyboard-open 检查；服务端也会短路）
        if self
            .ascii_mode
            .load(std::sync::atomic::Ordering::Acquire)
        {
            // 这条早退原来一声不响：用户报「只有英文字母」时，日志里看不出是英文态
            // 还是 TIP 根本没被调用。现在入口有 `process_key_event` 一行、这里有原因。
            debug!("  -> ascii mode, key passed through to the app");
            return false;
        }

        if is_up {
            if !self.ipc.is_connected() {
                debug!("  -> IPC not connected, skipping key up");
                return false;
            }
        } else {
            if !self.ipc.is_connected() {
                debug!("  -> IPC not connected, attempting reconnect...");
                if self.ipc.connect().is_ok() {
                    self.ipc.start_session();
                    debug!("  -> Reconnected!");
                } else {
                    debug!("  -> Reconnect failed");
                    return false;
                }
            }
            if let Some(ctx) = context {
                self.update_caret_position_sync(ctx);
            }
        }

        let mods = get_key_modifiers(is_up);

        if !is_up && (VK_X_A..=VK_X_Z).contains(&code) && mods == 0 {
            let letter = char::from_u32(code as u32 - VK_X_A as u32 + 'a' as u32).unwrap_or('a');
            self.last_input_key.set(Some(letter));
            debug!("  -> recorded last_input_key: {}", letter);
        }

        // Shift+可打印符号键按 X11 语义上报移位后的字符 keysym（Shift+/ → '?'），
        // 中文态标点（？、（、！等）依赖此转换
        let xk = vk_to_xk_with_shift(code, mods & K_SHIFT_MASK as i32 != 0);
        debug!("  -> calling process_key({}, {})", xk, mods);
        let response = self.ipc.process_key(xk, mods);
        debug!("  -> response: {:?}", response);
        if let Some(response) = response {
            debug!("  -> success={}", response.success);
            let output = RimeOutput::from_response(&response);
            if response.success {
                self.set_composing(output.composing);
                if let Some(ctx) = context {
                    debug!("  -> calling schedule_edit_session");
                    self.schedule_edit_session(ctx, output);
                }
                return true;
            }
            // 被引擎拒绝的键不归输入法，但引擎可能已经不在组词了（内置计算器退出、
            // 会话被外部重建……）。宿主这边若还挂着 composition，就是显示与引擎
            // 脱节：收掉它，键仍然交还应用。key-up 绝不能参与——计算器激活期间
            // 每个抬起事件都会被 rime 拒绝，收 composition 会把刚画出的算式抹掉。
            if rejected_key_ends_composition(
                is_up,
                response.status.is_some(),
                output.composing,
                self.is_composing(),
            ) {
                debug!("  -> rejected key: engine not composing, end stale composition");
                self.set_composing(false);
                if let Some(ctx) = context {
                    self.schedule_edit_session(
                        ctx,
                        RimeOutput {
                            commit: None,
                            preedit: String::new(),
                            _candidates: Vec::new(),
                            composing: false,
                        },
                    );
                }
            }
        }
        debug!("  -> returning false");
        false
    }

    fn abort_composition(&self) {
        debug!("abort_composition");
        self.composing.set(false);
        // Release composition reference if held (OnCompositionTerminated will clean up TSF side)
        if let Ok(mut guard) = self.composition.try_lock() {
            *guard = None;
        }
    }
}

impl ITfKeyEventSink_Impl for XimeTextService_Impl {
    fn OnSetFocus(&self, _fforeground: BOOL) -> Result<()> {
        // Focus handling is done in ITfThreadMgrEventSink::OnSetFocus
        // to avoid redundant IPC calls from multiple TSF sinks.
        debug!("ITfKeyEventSink::OnSetFocus - ignored (handled by ThreadMgrEventSink)");
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        pic: Ref<'_, ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        let vk = VIRTUAL_KEY(wparam.0 as u16);
        debug!("OnTestKeyDown: vk={}", vk.0);
        // 对齐 weasel：TestKeyDown 即完成整个按键处理（IPC 询问 rime），
        // OnKeyDown 只重放结果。
        self.test_keyup_pending.set(false);
        if self.test_keydown_pending.get() {
            return Ok(BOOL(1));
        }
        let eaten = self.process_key_event(pic.as_ref(), vk, false);
        if eaten {
            self.test_keydown_pending.set(true);
        }
        debug!("  -> eaten: {}", eaten);
        Ok(BOOL(if eaten { 1 } else { 0 }))
    }

    fn OnKeyDown(&self, pic: Ref<'_, ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        let vk = VIRTUAL_KEY(wparam.0 as u16);
        debug!("OnKeyDown: vk={}", vk.0);

        // Shift 单按判定：Shift 按下置位；期间任何其他键按下即取消
        if vk.0 == VK_SHIFT.0 {
            self.shift_solo.set(true);
        } else {
            self.shift_solo.set(false);
        }

        if vk.0 == VK_CONTROL.0 {
            if self.is_composing() && !self.ctrl_root_visible.get() {
                let last_key = self.last_input_key.get();
                debug!("  -> Ctrl pressed while composing, last_key={:?}", last_key);
                if let Some(letter) = last_key {
                    let mods = get_key_modifiers(false);
                    debug!("  -> mods={}", mods);
                    let ctrl_only = (mods & K_CONTROL_MASK as i32) != 0
                        && (mods & K_SHIFT_MASK as i32) == 0
                        && (mods & K_ALT_MASK as i32) == 0;
                    if ctrl_only {
                        debug!("  -> sending ShowRoot for '{}'", letter);
                        self.ipc.show_root(letter);
                        self.ctrl_root_visible.set(true);
                    }
                }
            }
            return Ok(BOOL(0));
        }

        if self.test_keydown_pending.get() {
            self.test_keydown_pending.set(false);
            return Ok(BOOL(1));
        }

        // 有些应用只调 OnKeyDown 不调 OnTestKeyDown（见 weasel KeyEventSink
        // 注释）——补走完整处理
        let context = pic.as_ref();
        let handled = self.process_key_event(context, vk, false);
        debug!("  -> result: {}", handled);
        Ok(BOOL(if handled { 1 } else { 0 }))
    }

    fn OnTestKeyUp(
        &self,
        pic: Ref<'_, ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        let vk = VIRTUAL_KEY(wparam.0 as u16);
        if vk.0 == VK_SHIFT.0 {
            return Ok(BOOL(1));
        }
        if vk.0 == VK_CONTROL.0 && self.ctrl_root_visible.get() {
            return Ok(BOOL(1));
        }
        self.test_keydown_pending.set(false);
        if self.test_keyup_pending.get() {
            return Ok(BOOL(1));
        }
        let eaten = self.process_key_event(pic.as_ref(), vk, true);
        if eaten {
            self.test_keyup_pending.set(true);
        }
        debug!("OnTestKeyUp: vk={}, eaten={}", vk.0, eaten);
        Ok(BOOL(if eaten { 1 } else { 0 }))
    }

    fn OnKeyUp(&self, pic: Ref<'_, ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        let vk = VIRTUAL_KEY(wparam.0 as u16);
        debug!("OnKeyUp: vk={}", vk.0);

        if vk.0 == VK_CONTROL.0 {
            if self.ctrl_root_visible.get() {
                debug!("  -> Ctrl released, sending HideRoot");
                self.ipc.hide_root();
                self.ctrl_root_visible.set(false);
            }
            return Ok(BOOL(0));
        }

        if vk.0 != VK_SHIFT.0 {
            // 其他键活动同样取消 Shift 单按判定（按下未被 TSF 看到的兜底）
            self.shift_solo.set(false);
            if self.test_keyup_pending.get() {
                self.test_keyup_pending.set(false);
                return Ok(BOOL(1));
            }
            let context = pic.as_ref();
            let handled = self.process_key_event(context, vk, true);
            debug!("  -> key up result: {}", handled);
            return Ok(BOOL(if handled { 1 } else { 0 }));
        }

        // 仅单按 Shift（按下与抬起之间无其他键活动）才切换中英；
        // Shift+符号键（如 Shift+/ 打 ？）不触发切换
        if !self.shift_solo.replace(false) {
            debug!("  -> shift used with other key, skip ascii toggle");
            return Ok(BOOL(0));
        }

        if !self.ipc.is_connected() {
            debug!("  -> IPC not connected");
            return Ok(BOOL(0));
        }

        debug!("  -> calling toggle_ascii_mode");
        let response = self.ipc.toggle_ascii_mode();
        debug!("  -> response: {:?}", response);

        if let Some(response) = response {
            if response.success {
                if let Some(ref status) = response.status {
                    debug!(
                        "  -> server ascii_mode: {}, updating local",
                        status.ascii_mode
                    );
                    self.ascii_mode
                        .store(status.ascii_mode, std::sync::atomic::Ordering::Release);
                    let verify = self.ascii_mode.load(std::sync::atomic::Ordering::Acquire);
                    debug!("  -> verified local ascii_mode: {}", verify);
                    self.update_lang_bar();
                }

                let output = RimeOutput::from_response(&response);
                debug!(
                    "  -> output: commit={}, composing={}",
                    output.commit.is_some(),
                    output.composing
                );
                self.set_composing(output.composing);

                if output.commit.is_some() {
                    debug!(
                        "  -> has commit, scheduling edit session to commit and end composition"
                    );
                    if let Some(ctx) = pic.as_ref() {
                        self.schedule_edit_session(ctx, output);
                    }
                } else if !output.composing && self.is_composing() {
                    debug!("  -> composing changed to false, ending composition");
                    self.set_composing(false);
                    if let Some(ctx) = pic.as_ref() {
                        self.schedule_edit_session(
                            ctx,
                            RimeOutput {
                                commit: None,
                                preedit: String::new(),
                                _candidates: Vec::new(),
                                composing: false,
                            },
                        );
                    }
                }

                return Ok(BOOL(1));
            }
        }

        Ok(BOOL(0))
    }

    fn OnPreservedKey(&self, _pic: Ref<'_, ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        Ok(BOOL(0))
    }
}

#[implement(
    ITfTextInputProcessor,
    ITfActiveLanguageProfileNotifySink,
    ITfThreadFocusSink,
    ITfThreadMgrEventSink,
    ITfKeyEventSink
)]
pub struct XimeTextService {
    thread_mgr: std::cell::RefCell<Option<ITfThreadMgr>>,
    client_id: std::cell::Cell<u32>,
    ipc: IpcClientHandle,
    composing: std::cell::Cell<bool>,
    composition: Arc<std::sync::Mutex<Option<ITfComposition>>>,
    lang_bar_mgr: std::cell::RefCell<Option<ITfLangBarItemMgr>>,
    ascii_mode: crate::language_bar::SharedAsciiMode,
    lang_bar_sink_ref: crate::language_bar::LangBarSinkRef,
    lang_bar_item: std::cell::RefCell<Option<ITfLangBarItemButton>>,
    profile_sink_cookie: std::cell::Cell<u32>,
    profile_source: std::cell::RefCell<Option<ITfSource>>,
    thread_focus_sink_cookie: std::cell::Cell<u32>,
    thread_focus_source: std::cell::RefCell<Option<ITfSource>>,
    thread_mgr_event_sink_cookie: std::cell::Cell<u32>,
    last_input_key: std::cell::Cell<Option<char>>,
    ctrl_root_visible: std::cell::Cell<bool>,
    processing_focus: std::cell::Cell<bool>,
    /// Shift 单按判定：Shift 按下置位，期间任何其他键活动即清除；
    /// 抬起时仍置位才切换中英（Shift+符号键不触发切换）。
    shift_solo: std::cell::Cell<bool>,
    /// weasel 式 pending 标志：TestKeyDown/Up 已完成按键处理（IPC 询问 rime）
    /// 后置位，对应的 OnKeyDown/Up 只重放结果不重复处理；
    /// 应对应用的怪异调用序列（多次 TestKeyDown、只调 KeyDown 不调 Test）。
    test_keydown_pending: std::cell::Cell<bool>,
    test_keyup_pending: std::cell::Cell<bool>,
    /// 宿主就是输入法自己的服务进程（`winxime-server.exe`）。
    ///
    /// 服务进程为了注册 TSF profile 会碰 `ITfInputProcessorProfileMgr`，一碰 TSF 就
    /// 成了宿主，系统会把我们的 TIP 也加载进**服务自己**。那个实例连回自己发
    /// `StartSession`/`FocusIn`/`FocusOut`，而 `FocusOut` 会 `clear_composition()` +
    /// `window.hide()`——于是用户正在打的字被「自己」清掉、候选栏闪没、托盘图标反复
    /// 显示隐藏（日志里 `Client connected!` 风暴就是这么来的）。这类实例必须什么都不做。
    self_host: std::cell::Cell<bool>,
}

pub const GUID_LANG_BAR_ITEM: GUID = GUID_LBI_INPUTMODE;

/// Shift 移位字符 keysym（美式布局；ASCII 可见字符的 keysym == ASCII 码）。
/// X11 语义：Shift+可打印键上报移位后的字符（Shift+/ → '?'），librime 的
/// punctuator/key_binder 按 '?'、'(' 等字符 keysym 登记，中文态打标点
/// （？、（、！等）依赖此转换。
/// **字母也要转**：rime 的 speller 按 keysym 字符入缓冲，不会因为 modifier 里有
/// Shift 就自动大写；不转的话 Shift+V 进缓冲是 "v"，依赖大写触发器的方案功能
/// （`calculator: trigger: V`、`^N...` 农历、`^R...` 数字转换）全部失效。
fn vk_to_xk_shifted(vk: u16) -> Option<i32> {
    let shifted = match vk {
        VK_OEM_1 => 0x3A,          // ; → :
        VK_OEM_PLUS => 0x2B,       // = → +
        VK_OEM_COMMA => 0x3C,      // , → <
        VK_OEM_MINUS => 0x5F,      // - → _
        VK_OEM_PERIOD => 0x3E,     // . → >
        VK_OEM_2 => 0x3F,          // / → ?
        VK_OEM_3 => 0x7E,          // ` → ~
        VK_OEM_4 => 0x7B,          // [ → {
        VK_OEM_5 => 0x7C,          // \ → |
        VK_OEM_6 => 0x7D,          // ] → }
        VK_OEM_7 => 0x22,          // ' → "
        0x30 => 0x29,              // 0 → )
        0x31 => 0x21,              // 1 → !
        0x32 => 0x40,              // 2 → @
        0x33 => 0x23,              // 3 → #
        0x34 => 0x24,              // 4 → $
        0x35 => 0x25,              // 5 → %
        0x36 => 0x5E,              // 6 → ^
        0x37 => 0x26,              // 7 → &
        0x38 => 0x2A,              // 8 → *
        0x39 => 0x28,              // 9 → (
        // 字母也必须报**大写 keysym**：rime 的 speller 是"按 keysym 字符"入缓冲，
        // 不会因为 modifier 里有 Shift 就把小写变大写。之前这里返回 None（保持小写），
        // 于是 Shift+V 进缓冲的是 "v"——依赖大写触发器的功能（`calculator: trigger: V`、
        // `^N[0-9...]` 农历、`^R...` 数字转换、`^U...` unicode）全部失效。
        0x41..=0x5A => 0x41 + (vk - 0x41) as i32, // 'A'..='Z' → 'A'..='Z'
        _ => return None,
    };
    Some(shifted)
}

/// vk_to_xk 的移位感知包装：shift 按下且键有移位字符时返回移位 keysym。
/// F1–F23 直接交还宿主；F24（0x87）是内部上屏触发键，必须留给 paste 协议。
fn is_function_passthrough(vk: u16) -> bool {
    (0x70..=0x86).contains(&vk)
}

fn vk_to_xk_with_shift(vk: u16, shift: bool) -> i32 {    // F24 是内部「上屏触发键」的协议键（VK_F24=0x87）。它必须保持 0x87，
    // 否则 paste::request_commit() 的 F24 注入会被当成普通功能键，数字无法上屏。
    if vk == 0x87 {
        return 0x87;
    }
    // F1–F23 在 X11 里是 0xFFBE..=0xFFD4；librime 的 vk_to_xk 没有这些映射，
    // 会退回 VK 数值（F2=0x71 变成字母 q），导致 F2/F5 被当成字母吞掉。
    if (0x70..=0x86).contains(&vk) {
        return 0xFFBE + (vk - 0x70) as i32;
    }
    if shift {
        if let Some(shifted) = vk_to_xk_shifted(vk) {
            return shifted;
        }
    }
    librime::vk_to_xk(vk)
}

/// 当前进程（= 宿主应用）的可执行文件名，只给日志用。
///
/// 入参传 `None` 拿的是**主模块**路径，也就是宿主 exe（Word、记事本、浏览器……），
/// 不是我们的 DLL——这正是排查「TIP 有没有被这个程序加载」要的东西。
fn host_exe_name() -> String {
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;

    let mut buf = [0u16; 512];
    let len = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
    if len == 0 {
        return "unknown".to_string();
    }
    let full = String::from_utf16_lossy(&buf[..len.min(buf.len())]);
    full.rsplit(['\\', '/'])
        .next()
        .unwrap_or(full.as_str())
        .to_string()
}

impl Default for XimeTextService {
    fn default() -> Self {
        Self::new()
    }
}

impl XimeTextService {
    /// TSF 对象与套间线程绑定：`composition` / `lang_bar_sink_ref` 用 `Arc`
    /// 只是为了让同一个套间里的多个 COM 接口实现共享所有权，从不跨线程发送。
    /// 所以即便 `ITfLangBarItemSink` 不是 `Send`，这里也不需要它。
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new() -> Self {
        let ipc = IpcClientHandle::empty();
        debug!(
            "XimeTextService::new() called, IPC state ptr={:p}",
            ipc.state
        );
        Self {
            thread_mgr: std::cell::RefCell::new(None),
            client_id: std::cell::Cell::new(0),
            ipc,
            composing: std::cell::Cell::new(false),
            composition: Arc::new(std::sync::Mutex::new(None)),
            lang_bar_mgr: std::cell::RefCell::new(None),
            ascii_mode: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            lang_bar_sink_ref: Arc::new(std::sync::Mutex::new(None)),
            lang_bar_item: std::cell::RefCell::new(None),
            profile_sink_cookie: std::cell::Cell::new(TF_INVALID_COOKIE),
            profile_source: std::cell::RefCell::new(None),
            thread_focus_sink_cookie: std::cell::Cell::new(TF_INVALID_COOKIE),
            thread_focus_source: std::cell::RefCell::new(None),
            thread_mgr_event_sink_cookie: std::cell::Cell::new(TF_INVALID_COOKIE),
            last_input_key: std::cell::Cell::new(None),
            ctrl_root_visible: std::cell::Cell::new(false),
            processing_focus: std::cell::Cell::new(false),
            shift_solo: std::cell::Cell::new(false),
            test_keydown_pending: std::cell::Cell::new(false),
            test_keyup_pending: std::cell::Cell::new(false),
            self_host: std::cell::Cell::new(false),
        }
    }

    fn ensure_ipc(&self) -> std::result::Result<(), winxime_ipc::IpcError> {
        let connected = self.ipc.is_connected();
        debug!(
            "ensure_ipc: is_connected={}, ptr={:p}",
            connected,
            self.ipc.debug_ptr()
        );
        if connected {
            return Ok(());
        }
        debug!("ensure_ipc: attempting connect...");
        match self.ipc.connect() {
            Ok(()) => {
                let (_, response) = self.ipc.start_session();
                if let Some(response) = response {
                    if let Some(status) = response.status {
                        debug!("ensure_ipc: initial ascii_mode={}", status.ascii_mode);
                        self.ascii_mode
                            .store(status.ascii_mode, std::sync::atomic::Ordering::Release);
                    }
                }
                debug!("ensure_ipc: connect OK");
                Ok(())
            }
            Err(e) => {
                debug!("ensure_ipc: connect FAILED: {:?}", e);
                Err(e)
            }
        }
    }
}

impl XimeTextService_Impl {
    fn activate_impl(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        // 只初始化一次：重复初始化会丢掉旧的 appender guard，最坏在宿主 UI 线程
        // 上阻塞约 1.1 秒（见 dll::init_logging_once 的注释）。
        crate::dll::init_logging_once();
        // 宿主进程名：用户报「打字只有英文字母」时，先确认 TIP 到底有没有被这个程序
        // 加载——没加载就不会调 OnKeyDown，按键自然一个都到不了 server。
        let host = host_exe_name();
        info!("Activate: host={} tid={}", host, tid);

        *self.thread_mgr.borrow_mut() = ptim.cloned();
        self.client_id.set(tid);

        // 服务进程自己也会被加载 TIP（它注册 profile 时碰了 TSF）。这个实例绝不能
        // 连自己：它的 FocusOut 会把用户正在打的组合清掉、候选栏闪没，还会反复
        // Show/Hide 托盘图标。（详见 `self_host` 字段的注释。）
        if host.eq_ignore_ascii_case("winxime-server.exe") {
            self.self_host.set(true);
            info!("  -> 本进程是输入法服务自身：跳过 IPC / 按键 / 托盘处理");
            return Ok(());
        }

        match self.ensure_ipc() {
            Ok(()) => {
                debug!("IPC connected successfully");
            }
            Err(e) => debug!("IPC connection failed: {:?}", e),
        }

        if let Some(thread_mgr) = ptim {
            // Register ITfKeyEventSink directly (self implements ITfKeyEventSink)
            if let Ok(kmgr) = thread_mgr.cast::<ITfKeystrokeMgr>() {
                debug!("Got ITfKeystrokeMgr, attempting AdviseKeyEventSink...");
                use windows_core::ComObjectInterface;
                let key_sink_ref = ComObjectInterface::<ITfKeyEventSink>::as_interface_ref(self);
                let key_sink: ITfKeyEventSink = key_sink_ref.to_owned();
                unsafe {
                    if kmgr.AdviseKeyEventSink(tid, &key_sink, true).is_ok() {
                        debug!("AdviseKeyEventSink succeeded");
                    } else {
                        debug!("AdviseKeyEventSink failed");
                    }
                }
            } else {
                debug!("Failed to get ITfKeystrokeMgr");
            }

            if let Ok(lang_bar_mgr) = thread_mgr.cast::<ITfLangBarItemMgr>() {
                debug!("Got ITfLangBarItemMgr, creating LangBarItem...");
                let lang_bar = crate::language_bar::LangBarItemButton::new(
                    GUID_LANG_BAR_ITEM,
                    self.ipc.clone(),
                    self.ascii_mode.clone(),
                    self.lang_bar_sink_ref.clone(),
                );
                let lang_bar_itf: ITfLangBarItemButton = lang_bar.into();
                let add_result = unsafe { lang_bar_mgr.AddItem(&lang_bar_itf) };
                match add_result {
                    Ok(_) => {
                        debug!("LangBarItem added successfully");
                        *self.lang_bar_mgr.borrow_mut() = Some(lang_bar_mgr);
                        *self.lang_bar_item.borrow_mut() = Some(lang_bar_itf);
                    }
                    Err(e) => {
                        debug!("LangBarItem AddItem failed: {:?}", e);
                    }
                }
            } else {
                debug!("Failed to get ITfLangBarItemMgr");
            }

            // Register profile activation sink for input method switching
            if let Ok(source) = thread_mgr.cast::<ITfSource>() {
                debug!("Got ITfSource from thread_mgr, registering sinks...");
                use windows_core::ComObjectInterface;

                // Register ITfActiveLanguageProfileNotifySink
                let profile_sink_ref =
                    ComObjectInterface::<ITfActiveLanguageProfileNotifySink>::as_interface_ref(
                        self,
                    );
                let profile_sink: ITfActiveLanguageProfileNotifySink = profile_sink_ref.to_owned();
                unsafe {
                    if let Ok(cookie) =
                        source.AdviseSink(&ITfActiveLanguageProfileNotifySink::IID, &profile_sink)
                    {
                        debug!("Profile sink registered, cookie={}", cookie);
                        self.profile_sink_cookie.set(cookie);
                        *self.profile_source.borrow_mut() = Some(source.clone());
                    } else {
                        debug!("Failed to AdviseSink for profile");
                    }
                }

                // Register ITfThreadFocusSink
                let thread_focus_sink_ref =
                    ComObjectInterface::<ITfThreadFocusSink>::as_interface_ref(self);
                let thread_focus_sink: ITfThreadFocusSink = thread_focus_sink_ref.to_owned();
                unsafe {
                    if let Ok(cookie) =
                        source.AdviseSink(&ITfThreadFocusSink::IID, &thread_focus_sink)
                    {
                        debug!("Thread focus sink registered, cookie={}", cookie);
                        self.thread_focus_sink_cookie.set(cookie);
                        *self.thread_focus_source.borrow_mut() = Some(source.clone());
                    } else {
                        debug!("Failed to AdviseSink for thread focus");
                    }
                }

                // Register ITfThreadMgrEventSink (for document focus changes)
                let thread_mgr_event_sink_ref =
                    ComObjectInterface::<ITfThreadMgrEventSink>::as_interface_ref(self);
                let thread_mgr_event_sink: ITfThreadMgrEventSink =
                    thread_mgr_event_sink_ref.to_owned();
                unsafe {
                    if let Ok(cookie) =
                        source.AdviseSink(&ITfThreadMgrEventSink::IID, &thread_mgr_event_sink)
                    {
                        debug!("ThreadMgrEventSink registered, cookie={}", cookie);
                        self.thread_mgr_event_sink_cookie.set(cookie);
                    } else {
                        debug!("Failed to AdviseSink for ThreadMgrEventSink");
                    }
                }
            } else {
                debug!("Failed to get ITfSource from thread_mgr");
            }
        } else {
            debug!("No thread_mgr available");
        }

        Ok(())
    }

    fn deactivate_impl(&self) -> Result<()> {
        debug!("Deactivate called");
        self.ipc.hide_tray_icon(self.client_id.get());

        // Unregister profile sink
        if self.profile_sink_cookie.get() != TF_INVALID_COOKIE {
            if let Some(source) = self.profile_source.borrow_mut().take() {
                unsafe {
                    let _ = source.UnadviseSink(self.profile_sink_cookie.get());
                }
            }
            self.profile_sink_cookie.set(TF_INVALID_COOKIE);
        }

        // Unregister thread focus sink
        if self.thread_focus_sink_cookie.get() != TF_INVALID_COOKIE {
            if let Some(source) = self.thread_focus_source.borrow_mut().take() {
                unsafe {
                    let _ = source.UnadviseSink(self.thread_focus_sink_cookie.get());
                }
            }
            self.thread_focus_sink_cookie.set(TF_INVALID_COOKIE);
        }

        // Unregister thread mgr event sink
        if self.thread_mgr_event_sink_cookie.get() != TF_INVALID_COOKIE {
            if let Some(thread_mgr) = self.thread_mgr.borrow().as_ref() {
                if let Ok(source) = thread_mgr.cast::<ITfSource>() {
                    unsafe {
                        let _ = source.UnadviseSink(self.thread_mgr_event_sink_cookie.get());
                    }
                }
            }
            self.thread_mgr_event_sink_cookie.set(TF_INVALID_COOKIE);
        }

        // UnadviseKeyEventSink using thread_mgr
        if let Some(thread_mgr) = self.thread_mgr.borrow().as_ref() {
            if let Ok(kmgr) = thread_mgr.cast::<ITfKeystrokeMgr>() {
                unsafe {
                    let _ = kmgr.UnadviseKeyEventSink(self.client_id.get());
                }
            }
        }

        if let Some(lang_bar_mgr) = self.lang_bar_mgr.borrow_mut().take() {
            if let Some(lang_bar_item) = self.lang_bar_item.borrow_mut().take() {
                unsafe {
                    if let Err(e) = lang_bar_mgr.RemoveItem(&lang_bar_item) {
                        debug!("RemoveItem failed: {:?}", e);
                    }
                }
            }
        }

        *self.thread_mgr.borrow_mut() = None;
        self.client_id.set(0);
        self.composing.set(false);
        match self.composition.try_lock() {
            Ok(mut guard) => *guard = None,
            Err(std::sync::TryLockError::Poisoned(e)) => *e.into_inner() = None,
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        Ok(())
    }
}

impl ITfTextInputProcessor_Impl for XimeTextService_Impl {
    fn Activate(&self, ptim: Ref<'_, ITfThreadMgr>, tid: u32) -> Result<()> {
        self.activate_impl(ptim.as_ref(), tid)
    }

    fn Deactivate(&self) -> Result<()> {
        self.deactivate_impl()
    }
}

impl ITfActiveLanguageProfileNotifySink_Impl for XimeTextService_Impl {
    fn OnActivated(
        &self,
        clsid: *const GUID,
        guidprofile: *const GUID,
        factivated: BOOL,
    ) -> Result<()> {
        let clsid_ref = unsafe { clsid.as_ref() };
        let profile_ref = unsafe { guidprofile.as_ref() };

        debug!("ITfActiveLanguageProfileNotifySink::OnActivated: clsid={:?}, profile={:?}, activated={}", clsid_ref, profile_ref, factivated.0);

        let our_clsid = crate::class_factory::CLSID_XIME;
        debug!("  -> our_clsid = {:?}", our_clsid);

        if let Some(clsid_val) = clsid_ref {
            debug!("  -> clsid_val = {:?}", clsid_val);
            debug!("  -> clsid_val == our_clsid? {}", clsid_val == &our_clsid);
            if clsid_val != &our_clsid {
                debug!("  -> Not our TIP, ignoring");
                return Ok(());
            }
            debug!("  -> clsid matches our TIP!");
        } else {
            debug!("  -> clsid_ref is None");
        }

        debug!("  -> factivated.as_bool() = {}", factivated.as_bool());

        if factivated.as_bool() {
            debug!("  -> Our TIP is being activated!");

            // 服务进程自身的实例：不连 IPC（否则 FocusOut 会清掉用户正在打的字）。
            if self.self_host.get() {
                debug!("  -> self host (server process), skipping IPC");
                return Ok(());
            }

            if !self.ipc.is_connected() {
                debug!("  -> IPC not connected, attempting reconnect...");
                if self.ipc.connect().is_err() {
                    debug!("  -> Reconnect failed, skipping");
                    return Ok(());
                }
                debug!("  -> Reconnected!");
            }

            if let Some(response) = self.ipc.start_session().1 {
                if let Some(status) = response.status {
                    debug!("  -> ascii_mode from server: {}", status.ascii_mode);
                    self.ascii_mode
                        .store(status.ascii_mode, std::sync::atomic::Ordering::Release);

                    let sink = match self.lang_bar_sink_ref.try_lock() {
                        Ok(g) => g,
                        Err(_) => return Ok(()),
                    };
                    if let Some(ref sink) = *sink {
                        unsafe {
                            let _ = sink.OnUpdate(TF_LBI_STATUS | TF_LBI_ICON | TF_LBI_TEXT);
                        }
                    }
                }
            }
            self.ipc.show_tray_icon(self.client_id.get());
        } else {
            debug!("  -> Our TIP is being deactivated");
            self.ipc.hide_candidates();
            self.ipc.hide_tray_icon(self.client_id.get());
        }

        Ok(())
    }
}

impl ITfThreadFocusSink_Impl for XimeTextService_Impl {
    fn OnSetThreadFocus(&self) -> Result<()> {
        // 线程获得焦点时才认领托盘 owner；FocusOut 不隐藏，避免同一个输入法
        // 在 Chrome 输入框/页面之间点进点出时托盘闪。
        if self.self_host.get() {
            return Ok(());
        }
        self.ipc.show_tray_icon(self.client_id.get());
        Ok(())
    }

    fn OnKillThreadFocus(&self) -> Result<()> {
        // 不在这里隐藏：同一输入法在不同应用/控件间切焦点时应保持显示。
        Ok(())
    }
}

impl ITfThreadMgrEventSink_Impl for XimeTextService_Impl {
    fn OnInitDocumentMgr(&self, _pdim: Ref<'_, ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    fn OnUninitDocumentMgr(&self, _pdim: Ref<'_, ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    fn OnSetFocus(
        &self,
        pdimfocus: Ref<'_, ITfDocumentMgr>,
        pdimprevfocus: Ref<'_, ITfDocumentMgr>,
    ) -> Result<()> {
        debug!(
            "ITfThreadMgrEventSink::OnSetFocus (pdimfocus.is_null={}, processing_focus={})",
            pdimfocus.is_null(),
            self.processing_focus.get()
        );

        // 服务进程自身的实例：焦点事件一律不理（它会 reconnect 回自己、再发 FocusOut）。
        if self.self_host.get() {
            debug!("  -> self host (server process), ignoring focus event");
            return Ok(());
        }

        // Re-entrancy guard: prevent focus event nesting from STA message pumping
        if self.processing_focus.get() {
            debug!("  -> re-entrant: skipping");
            return Ok(());
        }
        self.processing_focus.set(true);

        if pdimfocus.is_null() {
            debug!("  -> Focus lost (pdimfocus is null)");

            // B2：失焦前把影子缓冲的数字从 server 取回来，在**旧 context** 上提交。
            // 注意顺序：先同步请求（响应里带 commit），再排编辑会话；如果排在
            // `abort_composition()` 之后，旧组词串已经被收掉，数字就丢了。
            let response = if self.ipc.is_connected() {
                self.ipc.focus_out()
            } else {
                None
            };
            let mut commit_scheduled = false;
            if let Some(response) = response {
                let output = RimeOutput::from_response(&response);
                if output.commit.is_some() {
                    let composition_present = match self.composition.try_lock() {
                        Ok(guard) => guard.is_some(),
                        Err(_) => false,
                    };
                    if !composition_present {
                        // 焦点变化时 TSF 往往先 OnCompositionTerminated：组词串里的
                        // 数字已经作为普通文本留在文档里，再提交就是第二份。
                        debug!(
                            "  -> flush: composition already terminated, skip commit"
                        );
                    } else if let Some(prev) = pdimprevfocus.as_ref() {
                        match unsafe { prev.GetTop() } {
                            Ok(prev_context) => {
                                commit_scheduled =
                                    self.schedule_flush_edit_session(&prev_context, output);
                                debug!(
                                    "  -> flush shadow commit scheduled={}",
                                    commit_scheduled
                                );
                            }
                            Err(e) => {
                                debug!("  -> flush: prevfocus.GetTop failed: {:?}", e);
                            }
                        }
                    } else {
                        debug!("  -> flush: no previous document manager");
                    }
                }
            }
            if !commit_scheduled {
                self.abort_composition();
            }
            self.processing_focus.set(false);
            return Ok(());
        }

        debug!("  -> Focus gained (pdimfocus is valid)");

        if !self.ipc.is_connected() {
            debug!("  -> IPC not connected, attempting reconnect...");
            if self.ipc.connect().is_err() {
                debug!("  -> Reconnect failed");
                self.processing_focus.set(false);
                return Ok(());
            }
            debug!("  -> Reconnected!");
            // start_session is already called inside connect() -> ensure_ipc
        }

        // Sync ascii_mode from server status
        if let Some(response) = self.ipc.start_session().1 {
            if let Some(status) = response.status {
                debug!("  -> ascii_mode from server: {}", status.ascii_mode);
                self.ascii_mode
                    .store(status.ascii_mode, std::sync::atomic::Ordering::Release);

                let sink = match self.lang_bar_sink_ref.try_lock() {
                    Ok(g) => g,
                    Err(_) => {
                        self.processing_focus.set(false);
                        return Ok(());
                    }
                };
                if let Some(ref sink) = *sink {
                    unsafe {
                        let _ = sink.OnUpdate(TF_LBI_STATUS | TF_LBI_ICON | TF_LBI_TEXT);
                    }
                }
            }
        }
        self.ipc.focus_in();

        self.processing_focus.set(false);
        Ok(())
    }

    fn OnPushContext(&self, _pic: Ref<'_, ITfContext>) -> Result<()> {
        Ok(())
    }

    fn OnPopContext(&self, _pic: Ref<'_, ITfContext>) -> Result<()> {
        Ok(())
    }
}

/// 被引擎拒绝的按键（`success = false`）是否应顺带收掉宿主里残留的 composition。
///
/// 判据是「显示与引擎失配」，不是「键被拒绝」本身：引擎明确说自己不在组词
/// （回包带 `status` 且 `composing = false`），而宿主这边还挂着 composition。
/// 三个门条件缺一不可：
/// - `is_up`：抬起事件不参与（计算器激活期间每个 key-up 都会被 rime 拒绝，
///   收 composition 会把刚画出的算式抹掉）；
/// - `status_present`：无 status 时 `composing` 只是默认 false，不能当证据
///   （正常组词中被拒的键就属于这种——收掉会让显示与 rime 内部状态脱节）；
/// - `local_composing`：本来没挂着就没有可收的东西。
fn rejected_key_ends_composition(
    is_up: bool,
    status_present: bool,
    engine_composing: bool,
    local_composing: bool,
) -> bool {
    !is_up && status_present && !engine_composing && local_composing
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shift+字母必须报**大写** keysym：rime 按 keysym 字符入缓冲，不看 modifier。
    ///
    /// 这是「`calculator: trigger: V` 之类的功能全部失效」的根因——之前 Shift+V
    /// 进缓冲的是 "v"。
    #[test]
    fn shift_letters_report_uppercase_keysym() {
        assert_eq!(vk_to_xk_with_shift(0x41, true), 0x41); // Shift+A → 'A'
        assert_eq!(vk_to_xk_with_shift(0x56, true), 0x56); // Shift+V → 'V'
        assert_eq!(vk_to_xk_with_shift(0x5A, true), 0x5A); // Shift+Z → 'Z'
        // 不按 Shift 时仍然是小写 keysym（正常拼音输入）
        assert_eq!(vk_to_xk_with_shift(0x56, false), 0x76); // v
    }

    /// 符号/数字的移位映射保持不变（标点功能依赖它）。
    #[test]
    fn shift_symbols_unchanged() {
        assert_eq!(vk_to_xk_with_shift(0x31, true), 0x21); // 1 → !
        assert_eq!(vk_to_xk_with_shift(0xBF, true), 0x3F); // / → ?
        assert_eq!(vk_to_xk_with_shift(0x30, true), 0x29); // 0 → )
    }

    /// F1–F24 必须映射到 X11 的 0xFFBE..=0xFFD5；否则 F2=0x71 会变成字母 q，
    /// 被 rime 当输入吃掉（宿主收不到重命名/刷新）。
    #[test]
    fn function_keys_map_to_x11_range() {
        assert_eq!(vk_to_xk_with_shift(0x70, false), 0xFFBE); // F1
        assert_eq!(vk_to_xk_with_shift(0x71, false), 0xFFBF); // F2
        assert_eq!(vk_to_xk_with_shift(0x74, false), 0xFFC2); // F5
        assert_eq!(vk_to_xk_with_shift(0x87, false), 0x87); // F24 = paste trigger
    }

    /// F1–F23 永远交还宿主；F24 是内部 paste trigger，不能放行。
    #[test]
    fn function_keys_are_passthrough_except_f24() {
        assert!(is_function_passthrough(0x70)); // F1
        assert!(is_function_passthrough(0x71)); // F2
        assert!(is_function_passthrough(0x86)); // F23
        assert!(!is_function_passthrough(0x87)); // F24 = paste trigger
    }

    /// 计算器退出后宿主残留 composition 的对账规则。
    ///
    /// 正例：key-down、回包带 status、引擎已不组词、宿主还挂着 → 收掉。
    #[test]
    fn rejected_key_ends_stale_composition() {
        assert!(rejected_key_ends_composition(
            false, // key-down
            true,  // 回包带 status
            false, // 引擎 composing=false
            true,  // 宿主还挂着
        ));
    }

    /// 三个门条件逐一反向：任一不满足都不收。
    #[test]
    fn rejected_key_never_ends_composition_in_other_cases() {
        // key-up：计算器激活期间每个抬起事件都会被 rime 拒绝，收掉会抹掉算式。
        assert!(!rejected_key_ends_composition(true, true, false, true));
        // 无 status：composing 只是默认值 false，不能当证据。
        assert!(!rejected_key_ends_composition(false, false, false, true));
        // 引擎还在组词（普通输入中被拒的 Ctrl 组合等）：显示与 rime 一致，不能动。
        assert!(!rejected_key_ends_composition(false, true, true, true));
        // 宿主本来就没挂着 composition。
        assert!(!rejected_key_ends_composition(false, true, false, false));
    }
}
