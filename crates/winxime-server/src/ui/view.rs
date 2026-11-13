//! 候选栏窗口：窗口/交换链/合成设备创建与全部消息处理（含菜单面板鼠标交互）。

use tracing::trace;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DeviceContext, ID2D1Factory1, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory1, DWRITE_FACTORY_TYPE_SHARED,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISwapChain1, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, ScreenToClient,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetMessagePos, GetWindowLongPtrW, GetWindowRect, HTCLIENT, IsWindowVisible,
    KillTimer, LoadCursorW, RegisterClassW, SetCursor, SetTimer, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, HWND_TOPMOST,
    IDC_ARROW, IDC_HAND, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOMOVE, SW_HIDE, SW_SHOWNA, WINDOWPOS,
    WM_DESTROY, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_PAINT, WM_SETCURSOR,
    WM_TIMER, WM_WINDOWPOSCHANGING, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};use windows_core::{HSTRING, Interface, PCWSTR};

use super::layout::{
    calculate_client_rect, calculate_root_rect, candidate_item_at, place_window_sticky,
    WorkArea,
};
use super::model::{CandidateModel, RootModel};
use super::paint::{on_paint, on_paint_root, on_paint_with_metrics};
use super::panel::{
    dispatch_action, menu_item_id, menu_item_label, panel_height, panel_hit, rect_contains,
    window_to_panel, MenuAction, PanelHit, PanelPage, PanelSide,
};
use super::{
    CandidateWindow, PanelAnim, ShowRequest, BLUR_RADIUS, WM_HIDE_CANDIDATE, WM_HIDE_ROOT,
    WM_SHOW_CANDIDATE, WM_SHOW_ROOT, WM_TOGGLE_PANEL,
};
use crate::speech::{self, SpeechState};

/// 鼠标离开窗口（windows crate 中该常量位于未启用的 Win32_UI_Controls feature，按 Win32 定义本地声明）。
const WM_MOUSELEAVE: u32 = 0x02A3;

/// 语音页轮询定时器：聆听中靠它把 partial 文本刷到面板上。
/// 只在「停在语音页」时开着（离开页面即 KillTimer），没有 partial 变化就不重绘。
const VOICE_TIMER_ID: usize = 1;
/// 语音页轮询间隔：60ms（约 16fps）。
///
/// 比原来的 120ms 快一档，是为了**电平条**——8fps 的输入电平看着是台阶。
/// 代价只有一拍读一次状态快照的锁；静默时快照没变化就 `refresh_voice` 返回
/// false、不重绘，所以提频不会变成"每拍都重画"。
const VOICE_TIMER_MS: u32 = 60;

/// 面板展开/收起动画定时器（与语音轮询定时器互斥使用不同 id）。
const PANEL_ANIM_TIMER_ID: usize = 2;
/// 动画帧间隔：16ms ≈ 60fps。
const PANEL_ANIM_FRAME_MS: u32 = 16;
/// 动画总帧数：8 帧 × 16ms ≈ 130ms，够「优雅」又不拖沓。
const PANEL_ANIM_FRAMES: u32 = 8;

const WINDOW_CLASS: &str = "WinximeCandidateWindow";

pub(crate) struct RenderedView {
    pub(crate) hwnd: HWND,
    _d2d_factory: ID2D1Factory1,
    pub(crate) dwrite_factory: IDWriteFactory1,
    pub(crate) d2d_context: ID2D1DeviceContext,
    pub(crate) swapchain: IDXGISwapChain1,
    _dcomp_target: IDCompositionTarget,
}

impl RenderedView {
    pub(crate) fn new(user_data: *const CandidateWindow) -> Result<Self, String> {
        // SAFETY: user_data 指向 Box<CandidateWindow> 内的对象，生命周期覆盖窗口；
        // CreateWindowExW 只在 WM_NCCREATE 里读一次该指针，不保留。调用发生在
        // 即将运行消息循环的 UI 线程（见 ui/mod.rs 的 ensure_view_initialized）。
        unsafe {
            let hinstance = GetModuleHandleW(None).unwrap_or_default();
            let class_name = HSTRING::from(WINDOW_CLASS);

            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(Self::wnd_proc),
                hInstance: HINSTANCE(hinstance.0),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
                ..Default::default()
            };
            RegisterClassW(&wc);

            let hwnd = windows::Win32::UI::WindowsAndMessaging::CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
                PCWSTR::from_raw(class_name.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                200,
                160,
                None,
                None,
                Some(HINSTANCE(hinstance.0)),
                Some(user_data.cast()),
            )
            .map_err(|e| format!("CreateWindowExW failed: {:?}", e))?;

            let dwrite_factory: IDWriteFactory1 =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
                    .map_err(|e| format!("DWriteCreateFactory failed: {:?}", e))?;

            let mut device = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
            .map_err(|e| format!("D3D11CreateDevice failed: {:?}", e))?;
            let device = device.ok_or("D3D11 device is None")?;

            let dxgi_device: IDXGIDevice = device
                .cast()
                .map_err(|e| format!("IDXGIDevice cast failed: {:?}", e))?;
            let adapter = dxgi_device
                .GetAdapter()
                .map_err(|e| format!("GetAdapter failed: {:?}", e))?;
            let factory: IDXGIFactory2 = adapter
                .GetParent()
                .map_err(|e| format!("GetParent failed: {:?}", e))?;

            let swapchain_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: 10,
                Height: 10,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };

            let swapchain = factory
                .CreateSwapChainForComposition(&device, &swapchain_desc, None)
                .map_err(|e| format!("CreateSwapChainForComposition failed: {:?}", e))?;

            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                    .map_err(|e| format!("D2D1CreateFactory failed: {:?}", e))?;
            let d2d_device = d2d_factory
                .CreateDevice(&dxgi_device)
                .map_err(|e| format!("CreateDevice failed: {:?}", e))?;
            let d2d_context = d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                .map_err(|e| format!("CreateDeviceContext failed: {:?}", e))?;

            Self::create_swapchain_bitmap(&swapchain, &d2d_context)?;

            let dcomp_device: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)
                .map_err(|e| format!("DCompositionCreateDevice failed: {:?}", e))?;
            let dcomp_target = dcomp_device
                .CreateTargetForHwnd(hwnd, true)
                .map_err(|e| format!("CreateTargetForHwnd failed: {:?}", e))?;
            let visual = dcomp_device
                .CreateVisual()
                .map_err(|e| format!("CreateVisual failed: {:?}", e))?;
            visual
                .SetContent(&swapchain)
                .map_err(|e| format!("SetContent failed: {:?}", e))?;
            dcomp_target
                .SetRoot(&visual)
                .map_err(|e| format!("SetRoot failed: {:?}", e))?;
            dcomp_device
                .Commit()
                .map_err(|e| format!("Commit failed: {:?}", e))?;

            Ok(Self {
                hwnd,
                _d2d_factory: d2d_factory,
                dwrite_factory,
                d2d_context,
                swapchain,
                _dcomp_target: dcomp_target,
            })
        }
    }

    /// # Safety
    ///
    /// 必须在 UI 线程调用，且 `swapchain` 与 `target` 属于同一个 `RenderedView`
    /// （D2D 工厂是单线程的）；调用前该 target 上不应有未完成的绘制。
    pub(crate) unsafe fn create_swapchain_bitmap(
        swapchain: &IDXGISwapChain1,
        target: &ID2D1DeviceContext,
    ) -> Result<(), String> {
        let surface: windows::Win32::Graphics::Dxgi::IDXGISurface = swapchain
            .GetBuffer(0)
            .map_err(|e| format!("GetBuffer failed: {:?}", e))?;

        let bitmap_props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            ..Default::default()
        };

        let bitmap = target
            .CreateBitmapFromDxgiSurface(&surface, Some(&bitmap_props))
            .map_err(|e| format!("CreateBitmapFromDxgiSurface failed: {:?}", e))?;
        target.SetTarget(&bitmap);
        Ok(())
    }

    /// # Safety
    ///
    /// 只由系统以本窗口类的窗口过程回调；`hwnd` 是已注册的候选栏窗口，
    /// `wparam` 的裸指针按各消息的约定转移所有权（见对应分支）。
    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_NCCREATE => {
                // SAFETY: WM_NCCREATE 的 lparam 是系统提供的 CREATESTRUCTW，回调期间有效；已判空。
                let cs = lparam.0 as *const CREATESTRUCTW;
                if !cs.is_null() {
                    // SAFETY: 同一 CREATESTRUCTW，上一行已判空。
                    let user_data = unsafe { (*cs).lpCreateParams };
                    // SAFETY: 本窗口句柄在窗口过程内有效；GWLP_USERDATA 只由 UI 线程写。
                    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, user_data as isize) };
                }
                // SAFETY: 参数由系统提供，原样转交默认过程。
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_SHOW_CANDIDATE => {
                // 每键一次：入口信息并入下方布局 trace（U-10）。
                // 锚点与内容随同一条消息传入（由 IPC 线程 `show` 装箱投递）：
                // 先建模型、算新尺寸，再择位、绘制，最后才显示窗口。一次摆到位，
                // 不再「先按旧尺寸摆、再按新尺寸摆」——那两次之间的位移就是用户
                // 看到的候选栏闪烁（候选数 3 → 10 时高度差好几百 DIP）。
                let req_ptr = wparam.0 as *mut ShowRequest;
                let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                if !req_ptr.is_null() && !this_ptr.is_null() {
                    // SAFETY: 发送方用 Box::into_raw 转移 ShowRequest 所有权，消息即移交。
                    let req = unsafe { *Box::from_raw(req_ptr) };
                    let this = unsafe { &*this_ptr };
                    let model = CandidateModel::from(&req.context);
                    this.model.replace(model.clone());
                    this.anchor.set(Some(req.anchor));

                    // 输入新内容时收起面板（与 macOS 版候选刷新行为一致）。
                    // 语音页同理：用户开始打字 = 不要这次听写，丢弃并关定时器。
                    if this.panel_page.get() == PanelPage::VoiceInput {
                        Self::leave_voice_page(this, hwnd, true);
                    }
                    Self::cancel_panel_anim(this, hwnd);
                    this.collapse_panel();

                    let view = this.view.borrow();
                    if let Some(view) = view.as_ref() {
                        let dpi = RenderedView::get_dpi_for_window(hwnd);
                        if let Ok(metrics) =
                            calculate_client_rect(&view.dwrite_factory, &model, dpi, None)
                        {
                            this.metrics.replace(Some(metrics.clone()));
                            trace!(
                                "WM_SHOW_CANDIDATE: dpi={}, dip={}x{}, hw={}x{}",
                                dpi,
                                metrics.width,
                                metrics.height,
                                metrics.hw_width,
                                metrics.hw_height
                            );
                            // 插入点变了：必须重新择位（粘滞只对「插入点没动」生效）。
                            // SAFETY: 同一 UI 线程，this 与 hwnd 均有效。
                            unsafe {
                                Self::set_size_and_place(
                                    this,
                                    hwnd,
                                    metrics.hw_width as i32,
                                    metrics.hw_height as i32,
                                    true,
                                )
                            };
                            if !model.items.is_empty() {
                                // 面板已在本次更新开始时收起，按纯候选栏布局绘制。
                                let _ = on_paint_with_metrics(
                                    view,
                                    &model,
                                    dpi,
                                    &metrics,
                                    None,
                                    1.0,
                                    &this.panel_list.borrow(),
                                    &this.panel_grid.borrow(),
                                    &this.voice.borrow(),
                                );
                            }
                        }
                    }
                }
                let _ = ShowWindow(hwnd, SW_SHOWNA);
                LRESULT(0)
            }
            WM_HIDE_CANDIDATE => {
                // 每键可能多次（每次提交/失焦）：低价值，降 trace（U-10）。
                trace!("WM_HIDE_CANDIDATE received");
                let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                if !this_ptr.is_null() {
                    let this = &*this_ptr;
                    // 宿主收起候选栏（切窗口 / 组合结束）时若还停在语音页：
                    // 关定时器 + 丢弃本次采集（不留没人管的麦克风）。
                    if this.panel_page.get() == PanelPage::VoiceInput {
                        Self::leave_voice_page(this, hwnd, true);
                    }
                    Self::cancel_panel_anim(this, hwnd);
                    this.collapse_panel();
                    this.metrics.replace(None);
                    // 下次显示是一次全新的会话：清掉上一次的择位结果，
                    // 否则窗口会先粘在旧位置。
                    this.placement.set(None);
                    this.anchor.set(None);
                }
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_TOGGLE_PANEL => {
                trace!("WM_TOGGLE_PANEL received");
                let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                if !this_ptr.is_null() {
                    let this = &*this_ptr;
                    // 面板快捷键（默认 Ctrl+`）：只有候选栏可见时才响应，
                    // 避免在没打字的窗口里凭空弹出一个悬空面板。
                    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
                    if visible {
                        Self::toggle_panel(this, hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_SHOW_ROOT => {
                trace!("WM_SHOW_ROOT received");
                let root_ptr = wparam.0 as *mut RootModel;
                if !root_ptr.is_null() {
                    // SAFETY: 发送方用 Box::into_raw 转移 RootModel 的所有权，消息即移交，
                    // 这里收回后立即 clone，不把指针留到下一轮消息。
                    let root = unsafe { Box::from_raw(root_ptr) };
                    trace!("showing root for '{}': {}", root.letter, root.root);

                    let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                    if !this_ptr.is_null() {
                        // SAFETY: this_ptr 是 WM_NCCREATE 存入的窗口用户数据，窗口销毁前有效。
                        unsafe { (*this_ptr).root_model.replace(Some(*root.clone())) };

                        // SAFETY: 同上，this_ptr 有效；borrow 只在 UI 线程内借用，不跨消息。
                        let view = unsafe { (*this_ptr).view.borrow() };
                        if let Some(view) = view.as_ref() {
                            let dpi = RenderedView::get_dpi_for_window(hwnd);
                            if let Ok(metrics) = calculate_root_rect(&view.dwrite_factory, &root, dpi) {
                                // 字根窗比候选栏矮胖，原位改尺寸会露出上一屏的残影区，
                                // 所以同样重新择位（锚点不变，只是窗口变小）。
                                Self::set_size_and_place(
                                    &*this_ptr,
                                    hwnd,
                                    metrics.hw_width as i32,
                                    metrics.hw_height as i32,
                                    false,
                                );
                                let _ = on_paint_root(view, &root, dpi, &metrics);
                            }
                        }
                    }
                }
                let _ = ShowWindow(hwnd, SW_SHOWNA);
                LRESULT(0)
            }
            WM_HIDE_ROOT => {
                trace!("WM_HIDE_ROOT received");
                let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                if !this_ptr.is_null() {
                    (*this_ptr).root_model.replace(None);

                    let model = (*this_ptr).model.borrow();
                    let view = (*this_ptr).view.borrow();
                    if let Some(view) = view.as_ref() {
                        let dpi = RenderedView::get_dpi_for_window(hwnd);
                        if let Ok(metrics) =
                            calculate_client_rect(&view.dwrite_factory, &model, dpi, (*this_ptr).panel_layout_page())
                        {
                            Self::set_size_and_place(
                                &*this_ptr,
                                hwnd,
                                metrics.hw_width as i32,
                                metrics.hw_height as i32,
                                false,
                            );
                            if !model.items.is_empty() {
                                let list = (*this_ptr).panel_list.borrow();
                                let grid = (*this_ptr).panel_grid.borrow();
                                let voice = (*this_ptr).voice.borrow();
                                let _ = on_paint_with_metrics(
                                    view,
                                    &model,
                                    dpi,
                                    &metrics,
                                    (*this_ptr).panel_paint_state(),
                                    (*this_ptr).panel_progress(),
                                    &list,
                                    &grid,
                                    &voice,
                                );
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            WM_PAINT => {
                trace!("WM_PAINT received");
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);

                let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                if !this_ptr.is_null() {
                    let model = (*this_ptr).model.borrow();
                    trace!("  painting {} items", model.items.len());
                    if !model.items.is_empty() {
                        let view = (*this_ptr).view.borrow();
                        if let Some(view) = view.as_ref() {
                            let list = (*this_ptr).panel_list.borrow();
                            let grid = (*this_ptr).panel_grid.borrow();
                            let voice = (*this_ptr).voice.borrow();
                            let _ = on_paint(
                                view,
                                &model,
                                (*this_ptr).panel_paint_state(),
                                (*this_ptr).panel_progress(),
                                &list,
                                &grid,
                                &voice,
                            );
                        }
                    }
                }

                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_WINDOWPOSCHANGING => {
                let pos = lparam.0 as *mut WINDOWPOS;
                if let Some(pos) = pos.as_mut() {
                    let dpi = RenderedView::get_dpi_for_point(POINT { x: pos.x, y: pos.y });

                    let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                    if !this_ptr.is_null() {
                        let model = (*this_ptr).model.borrow();
                        let view = (*this_ptr).view.borrow();
                        if let Some(view) = view.as_ref() {
                            if let Ok(metrics) = calculate_client_rect(&view.dwrite_factory, 
                                &model,
                                dpi,
                                (*this_ptr).panel_layout_page(),
                            ) {
                                pos.cx = metrics.hw_width as i32;
                                pos.cy = metrics.hw_height as i32;
                                (pos.x, pos.y) = RenderedView::clamp_point_to_monitor(
                                    pos.x, pos.y, pos.cx, pos.cy,
                                );
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                if !this_ptr.is_null() {
                    let this = &*this_ptr;
                    let scale = Self::get_dpi_for_window(hwnd) / 96.0;
                    // lParam 为客户区物理像素坐标，换算为 DIP。
                    let pt_x = (lparam.0 & 0xFFFF) as u16 as i16 as f32 / scale;
                    let pt_y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32 / scale;

                    // 候选点击：计算器候选提交对应文本，普通 rime 候选走「选中」
                    // （保留词频/学习语义）。两条都复用粘贴面板的 F24 自注入通道。
                    let clicked_candidate = {
                        let model = this.model.borrow();
                        let metrics = this.metrics.borrow();
                        metrics
                            .as_ref()
                            .and_then(|m| candidate_item_at(&model, m, pt_x, pt_y))
                    };
                    if let Some(index) = clicked_candidate {
                        if crate::calculator::is_active() {
                            // 计算器结果/原式：点哪条提交哪条的文本。
                            if let Some(text) = crate::calculator::candidate_commit_text(index) {
                                if !crate::paste::request_commit(&text) {
                                    crate::notify::notify("点击上屏失败，请用空格");
                                }
                            }
                            // 错误提示等不可上屏的候选：吞掉点击。
                        } else if !crate::paste::request_select(index) {
                            crate::notify::notify("点击选词失败，请用数字键/空格");
                        }
                        return LRESULT(0);
                    }

                    // 面板区域点击：菜单卡片导航 / 返回菜单 / 触发动作 / 剪切板条目复制。
                    let (bar_width, _bar_height, panel_width, panel_height, panel_side) =
                        panel_metrics(this);
                    if let Some((lx, ly)) = window_to_panel(
                        bar_width,
                        panel_width,
                        panel_height,
                        panel_side,
                        pt_x,
                        pt_y,
                    ) {
                        let page = this.panel_page.get();
                        let hit = {
                            let list = this.panel_list.borrow();
                            let grid = this.panel_grid.borrow();
                            panel_hit(page, panel_width, &list, &grid, lx, ly)
                        };
                        match hit {
                            Some(PanelHit::MenuItem(i)) => {
                                let clicked_id = menu_item_id(i, panel_width);
                                if clicked_id == Some("settings") {
                                    // 设置项：回调启动设置程序，并收起面板。
                                    dispatch_action(MenuAction::OpenSettings);
                                    Self::cancel_panel_anim(this, hwnd);
                                    this.collapse_panel();
                                    Self::relayout_and_repaint(this, hwnd);
                                } else if let Some(next) = clicked_id.and_then(PanelPage::from_id)
                                {
                                    this.panel_page.set(next);
                                    this.hovered_item.set(None);
                                    if next.is_list_page() {
                                        // 进入列表子页（剪切板 / 快捷发送）：读一次
                                        // clipboard.db（UI 线程内一次查询，之后绘制只读内存）。
                                        this.reload_panel_list(next);
                                    } else if next.is_grid_page() {
                                        // 进入网格子页（表情 / 符号）：标签回到「最近使用」、
                                        // 页码回到第一页，并读一次 recent_usage.json。
                                        this.reload_panel_grid(next);
                                    } else if next == PanelPage::VoiceInput {
                                        // 进入语音页：查一次模型文件 + 预装载 + 开轮询定时器。
                                        Self::enter_voice_page(this, hwnd);
                                    }
                                    Self::relayout_and_repaint(this, hwnd);
                                } else if let Some(label) = menu_item_label(i, panel_width) {
                                    // 既没有子页也没有动作的卡片：现在 6 张卡片都能落到
                                    // 「开子页」或「设置」上，这里是给以后新增卡片的兜底——
                                    // 给一句明确反馈并收起面板，不做「点了没反应」的死卡片。
                                    crate::notify::notify(format!("「{label}」功能暂未开放"));
                                    Self::cancel_panel_anim(this, hwnd);
                                    this.collapse_panel();
                                    Self::relayout_and_repaint(this, hwnd);
                                }
                            }
                            Some(PanelHit::Back) => {
                                if page == PanelPage::VoiceInput {
                                    // 从语音页返回：还在采集就丢弃本次（「← 菜单」= 放弃）。
                                    Self::leave_voice_page(this, hwnd, true);
                                }
                                this.panel_page.set(PanelPage::Menu);
                                this.hovered_item.set(None);
                                Self::relayout_and_repaint(this, hwnd);
                            }
                            Some(PanelHit::VoiceToggle) => {
                                // 装载中 / 模型没下载时按钮是灰的：点了不该有任何动作
                                // （否则用户看到的是"闪一下又没反应"）。
                                if this.voice.borrow().button_enabled() {
                                    Self::toggle_voice(this, hwnd);
                                } else {
                                    tracing::debug!("语音页点击被忽略：当前状态不可开始/结束");                                }
                            }
                            Some(PanelHit::ListItem(row)) => {
                                let item = this.panel_list.borrow().item_at(row).cloned();
                                if let Some(item) = item {
                                    let text = item.text;
                                    // 1) 剪切板条目先放回系统剪贴板（它本来就是剪贴板内容，
                                    //    也顺带留一条 Ctrl+V 兜底路径）；快捷发送条目不污染
                                    //    剪贴板，只有上屏失败时才退化成复制。
                                    let mut copied = false;
                                    if this.panel_page.get() == PanelPage::Clipboard {
                                        copied = crate::clipboard::write_text(&text);
                                    }
                                    // 2) 直接上屏：server 自己注入一个触发键，宿主 TSF
                                    //    照常把它上报回来，server 再把文本当 commit 回包，
                                    //    文本于是经宿主正常的编辑会话落到光标处。
                                    let committed = crate::paste::request_commit(&text);
                                    if !committed && !copied {
                                        copied = crate::clipboard::write_text(&text);
                                    }
                                    Self::cancel_panel_anim(this, hwnd);
                                    this.collapse_panel();
                                    Self::relayout_and_repaint(this, hwnd);
                                    // 上屏成功不再弹通知（U-28）：条目已在光标处，
                                    // 「已上屏（N 字）」的系统通知是多余打扰。只保留
                                    // 复制兜底 / 失败重试这两条用户需要行动的提示。
                                    if !committed && copied {
                                        crate::notify::notify(
                                            "已复制到剪贴板，按 Ctrl+V 粘贴",
                                        );
                                    } else if !committed && !copied {
                                        crate::notify::notify(
                                            "上屏失败：剪贴板被其他程序占用，请重试",
                                        );
                                    }
                                }
                            }
                            Some(PanelHit::PrevPage) => {
                                // 列表子页与网格子页共用底部翻页条：按当前页面翻对应那份数据。
                                // 注意：翻页结果必须先落到局部变量——`if borrow_mut()...`
                                // 会把 RefMut 临时值延续到整个 if 体内，重绘时再次借用即 panic。
                                let turned = if page.is_grid_page() {
                                    this.panel_grid.borrow_mut().prev_page()
                                } else {
                                    this.panel_list.borrow_mut().prev_page()
                                };
                                if turned {
                                    this.hovered_item.set(None);
                                    Self::relayout_and_repaint(this, hwnd);
                                }
                            }
                            Some(PanelHit::NextPage) => {
                                let turned = if page.is_grid_page() {
                                    this.panel_grid.borrow_mut().next_page()
                                } else {
                                    this.panel_list.borrow_mut().next_page()
                                };
                                if turned {
                                    this.hovered_item.set(None);
                                    Self::relayout_and_repaint(this, hwnd);
                                }
                            }
                            Some(PanelHit::GlyphTab(index)) => {
                                // 切分类标签：页码回到第一页；内置字形表是常量，不用重载。
                                // hover 清空——格子下标是按当前标签算的，留着会指向别的字形。
                                this.panel_grid.borrow_mut().select_tab(index);
                                this.hovered_item.set(None);
                                Self::relayout_and_repaint(this, hwnd);
                            }
                            Some(PanelHit::GlyphCell(index)) => {
                                // 点字形 = 上屏（与剪切板条目同一条通道：server 注入触发键，
                                // 宿主把它报回来，server 再把字形当 commit 回包）。
                                // 「在最近使用标签里点按不重排」——安卓同款语义：那一页
                                // 保持位置稳定，便于在同一个位置连点同一个字形。
                                let hit_glyph = this
                                    .panel_grid
                                    .borrow()
                                    .item_at(index)
                                    .map(str::to_string);
                                if let Some(glyph) = hit_glyph {
                                    {
                                        let mut grid = this.panel_grid.borrow_mut();
                                        if !grid.is_recent_tab() {
                                            grid.record_use(&glyph);
                                        }
                                    }
                                    let committed = crate::paste::request_commit(&glyph);
                                    let copied =
                                        !committed && crate::clipboard::write_text(&glyph);
                                    Self::cancel_panel_anim(this, hwnd);
                                    this.collapse_panel();
                                    Self::relayout_and_repaint(this, hwnd);
                                    // 上屏成功不再弹通知：字已经出现在光标处，再把
                                    // 同一个符号（常是 emoji）用系统通知展示一遍纯属
                                    // 打扰（U-28）。只在上屏失败时才打扰用户。
                                    if !committed && copied {
                                        crate::notify::notify(
                                            "已复制到剪贴板，按 Ctrl+V 粘贴",
                                        );
                                    } else if !committed && !copied {
                                        crate::notify::notify(
                                            "上屏失败：剪贴板被其他程序占用，请重试",
                                        );
                                    }
                                }
                            }
                            None => {}
                        }
                        return LRESULT(0);
                    }

                    // "⋮" 菜单按钮点击：切换面板展开状态。
                    let menu_btn = {
                        let metrics = this.metrics.borrow();
                        metrics.as_ref().and_then(|m| m.menu_button)
                    };
                    if let Some(rect) = menu_btn {
                        if rect_contains(rect, pt_x - BLUR_RADIUS, pt_y - BLUR_RADIUS) {
                            Self::toggle_panel(this, hwnd);
                            return LRESULT(0);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
                // 面板展开/收起动画：逐帧推进，播完按方向收尾
                // （收起动画要播完才真正收窗，展开动画播完即停在终态）。
                if wparam.0 == PANEL_ANIM_TIMER_ID {
                    let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                    if !this_ptr.is_null() {
                        let this = &*this_ptr;
                        if let Some(anim) = this.panel_anim.get() {
                            let frame = anim.frame + 1;
                            if frame >= anim.total_frames {
                                // 最后一帧播完：清动画并落到终态。
                                let _ = KillTimer(Some(hwnd), PANEL_ANIM_TIMER_ID);
                                this.panel_anim_timer.set(false);
                                this.panel_anim.set(None);
                                if !anim.expanding {
                                    // 收起动画结束：此刻才把面板收掉、窗口缩回候选栏。
                                    this.collapse_panel();
                                }
                                Self::relayout_and_repaint(this, hwnd);
                            } else {
                                this.panel_anim.set(Some(PanelAnim {
                                    expanding: anim.expanding,
                                    frame,
                                    total_frames: anim.total_frames,
                                }));
                                Self::relayout_and_repaint(this, hwnd);
                            }
                        } else {
                            // 兜底：没有动画状态就不该有定时器在跑。
                            let _ = KillTimer(Some(hwnd), PANEL_ANIM_TIMER_ID);
                            this.panel_anim_timer.set(false);
                        }
                    }
                }
                // 语音页轮询：聆听中把 partial 文本刷到面板上（没变化就不重绘）。
                if wparam.0 == VOICE_TIMER_ID {
                    let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                    if !this_ptr.is_null() {
                        let this = &*this_ptr;
                        let on_voice_page = this.panel_visible.get()
                            && this.panel_page.get() == PanelPage::VoiceInput;
                        if on_voice_page {
                            if this.refresh_voice(false) {
                                Self::relayout_and_repaint(this, hwnd);
                            }
                        } else {
                            // 兜底：不在语音页就不该有定时器在跑。
                            let _ = KillTimer(Some(hwnd), VOICE_TIMER_ID);
                            this.voice_timer.set(false);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                if !this_ptr.is_null() {
                    let this = &*this_ptr;
                    let total_pages = this.model.borrow().total_pages;
                    let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16;
                    if total_pages > 1 && delta != 0 {
                        // 候选窗是 NOACTIVATE，滚轮消息不一定到得了这里；能收到就翻页。
                        crate::ipc_server::request_page_from_ui(delta > 0);
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                if !this_ptr.is_null() {
                    let this = &*this_ptr;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);

                    let scale = Self::get_dpi_for_window(hwnd) / 96.0;
                    let pt_x = (lparam.0 & 0xFFFF) as u16 as i16 as f32 / scale;
                    let pt_y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32 / scale;

                    // 候选悬停：普通 rime 候选与计算器候选都可点，统一给反馈。
                    // 直接改模型里的 hovered 后重绘。
                    let candidate_hover = {
                        let model = this.model.borrow();
                        let metrics = this.metrics.borrow();
                        metrics
                            .as_ref()
                            .and_then(|m| candidate_item_at(&model, m, pt_x, pt_y))
                    };
                    let candidate_hover_changed = {
                        let mut model = this.model.borrow_mut();
                        if model.hovered != candidate_hover {
                            model.hovered = candidate_hover;
                            true
                        } else {
                            false
                        }
                    };
                    if candidate_hover_changed {
                        Self::relayout_and_repaint(this, hwnd);
                    }

                    // hover 反馈：菜单卡片 / 列表条目行 / 网格格子 / 语音页主按钮
                    // （其它子页无 hover 元素）。
                    let page = this.panel_page.get();
                    let hoverable = page == PanelPage::Menu
                        || page.is_list_page()
                        || page.is_grid_page()
                        || page == PanelPage::VoiceInput;
                    if this.panel_visible.get() && hoverable {
                        let (bar_width, _bar_height, panel_width, panel_height, panel_side) =
                            panel_metrics(this);
                        let hovered_item = match window_to_panel(
                            bar_width,
                            panel_width,
                            panel_height,
                            panel_side,
                            pt_x,
                            pt_y,
                        ) {
                            Some((lx, ly)) => {
                                let list = this.panel_list.borrow();
                                let grid = this.panel_grid.borrow();
                                match panel_hit(page, panel_width, &list, &grid, lx, ly) {
                                    Some(PanelHit::MenuItem(i))
                                    | Some(PanelHit::ListItem(i))
                                    | Some(PanelHit::GlyphCell(i)) => Some(i),
                                    // 语音页只有一个可点元素：下标固定 0（绘制侧同源）。
                                    Some(PanelHit::VoiceToggle) => Some(0),
                                    _ => None,
                                }
                            }
                            None => None,
                        };
                        if this.hovered_item.get() != hovered_item {
                            this.hovered_item.set(hovered_item);
                            Self::relayout_and_repaint(this, hwnd);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                if !this_ptr.is_null() {
                    let this = &*this_ptr;
                    let candidate_hover_cleared = {
                        let mut model = this.model.borrow_mut();
                        model.hovered.take().is_some()
                    };
                    if this.hovered_item.get().is_some() {
                        this.hovered_item.set(None);
                    }
                    if candidate_hover_cleared {
                        Self::relayout_and_repaint(this, hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                if (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                    let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                    if !this_ptr.is_null() {
                        let this = &*this_ptr;
                        let scale = Self::get_dpi_for_window(hwnd) / 96.0;
                        // GetMessagePos 为屏幕物理像素坐标，先转客户区再换算 DIP。
                        let pos = GetMessagePos();
                        let mut pt = POINT {
                            x: (pos & 0xFFFF) as u16 as i16 as i32,
                            y: ((pos >> 16) & 0xFFFF) as u16 as i16 as i32,
                        };
                        let _ = ScreenToClient(hwnd, &mut pt);
                        let px = pt.x as f32 / scale;
                        let py = pt.y as f32 / scale;

                        let menu_btn = {
                            let metrics = this.metrics.borrow();
                            metrics.as_ref().and_then(|m| m.menu_button)
                        };
                        // 候选可点（计算器与普通 rime 候选都接）：悬停给手型光标。
                        let over_candidate = {
                            let model = this.model.borrow();
                            let metrics = this.metrics.borrow();
                            metrics
                                .as_ref()
                                .is_some_and(|m| candidate_item_at(&model, m, px, py).is_some())
                        };
                        let (bar_width, _bar_height, panel_width, panel_height, panel_side) =
                            panel_metrics(this);
                        let over_interactive = over_candidate
                            || menu_btn.is_some_and(|rect| {
                                rect_contains(rect, px - BLUR_RADIUS, py - BLUR_RADIUS)
                            })
                            || window_to_panel(
                                bar_width,
                                panel_width,
                                panel_height,
                                panel_side,
                                px,
                                py,
                            )
                            .is_some_and(|(lx, ly)| {
                                let list = this.panel_list.borrow();
                                let grid = this.panel_grid.borrow();
                                panel_hit(
                                    this.panel_page.get(),
                                    panel_width,
                                    &list,
                                    &grid,
                                    lx,
                                    ly,
                                )
                                .is_some()
                            });
                        let cursor = if over_interactive {
                            LoadCursorW(None, IDC_HAND)
                        } else {
                            LoadCursorW(None, IDC_ARROW)
                        };
                        if let Ok(cursor) = cursor {
                            SetCursor(Some(cursor));
                        }
                        return LRESULT(1);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    /// 取包含指定屏幕点的显示器工作区（物理像素）；失败返回 None。
    fn work_area_at(point: POINT) -> Option<WorkArea> {
        // SAFETY: 坐标按值传入；MONITORINFO 在栈上且 cbSize 已填。
        unsafe {
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(monitor, &mut mi).as_bool() {
                let rc = mi.rcWork;
                Some(WorkArea {
                    left: rc.left,
                    top: rc.top,
                    right: rc.right,
                    bottom: rc.bottom,
                })
            } else {
                None
            }
        }
    }

    /// 改尺寸 + 择位，一次 SetWindowPos 完成。
    ///
    /// 候选栏的尺寸由内容决定（候选行数、面板是否展开），位置由插入点决定；
    /// 两者一改就得一起重算——否则面板展开后窗口只是「原地变大」，底部与右侧
    /// 会直接顶出屏幕（U-11）。`w`/`h` 是含模糊外扩的物理像素尺寸。
    ///
    /// `anchor_changed` 表示插入点是否变了。没变且原位放得下时**不挪窗口**：
    /// 候选数在几个到十个之间跳、窗口高度跟着跳，但用户看到的光标没动，窗口
    /// 上边界就不该跟着跳（那就是「忽上忽下」，见 `layout::place_window_sticky`）。
    ///
    /// 没有锚点（窗口还没收到过 WM_SHOW_CANDIDATE）时退回「只改尺寸、交给
    /// WM_WINDOWPOSCHANGING 的 clamp 兜底」。
    ///
    /// # Safety
    ///
    /// 只能在 UI 线程调用，`hwnd` 必须是本候选栏窗口。
    unsafe fn set_size_and_place(
        this: &CandidateWindow,
        hwnd: HWND,
        w: i32,
        h: i32,
        anchor_changed: bool,
    ) {
        let placed = match this.anchor.get() {
            Some(anchor) => Self::work_area_at(POINT {
                x: anchor.0,
                y: anchor.1,
            })
            .map(|work| {
                place_window_sticky(
                    anchor,
                    w,
                    h,
                    Self::get_dpi_for_window(hwnd),
                    work,
                    this.placement.get(),
                    anchor_changed,
                )
            }),
            None => None,
        };

        match placed {
            Some(placement) => {
                this.placement.set(Some(placement));
                // 尺寸与位置都没变就什么都不做：每次按键都 SetWindowPos 会让
                // DWM 重新合成一遍，是打字「闪」的另一个来源。
                if Self::window_matches(hwnd, placement.x, placement.y, w, h) {
                    return;
                }
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    placement.x,
                    placement.y,
                    w,
                    h,
                    SWP_NOACTIVATE | SWP_NOCOPYBITS,
                );
            }
            None => {
                this.placement.set(None);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    w,
                    h,
                    SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOCOPYBITS,
                );
            }
        }
    }

    /// 窗口当前的位置与尺寸是否已经是 `(x, y, w, h)`。
    fn window_matches(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) -> bool {
        let mut rc = RECT::default();
        // SAFETY: hwnd 在窗口过程内有效；rc 在栈上。
        if unsafe { GetWindowRect(hwnd, &mut rc) }.is_err() {
            return false;
        }
        rc.left == x && rc.top == y && rc.right - rc.left == w && rc.bottom - rc.top == h
    }

    pub(crate) fn get_dpi_for_window(hwnd: HWND) -> f32 {
        // SAFETY: hwnd 是调用方持有的窗口句柄；输出参数都在栈上，API 失败时保持 96 DPI 初值。
        unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut dpi_x = 96u32;
            let mut dpi_y = 96u32;
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            dpi_x as f32
        }
    }

    fn get_dpi_for_point(point: POINT) -> f32 {
        // SAFETY: 坐标按值传入；输出参数在栈上，API 失败时保持 96 DPI 初值。
        unsafe {
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut dpi_x = 96u32;
            let mut dpi_y = 96u32;
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            dpi_x as f32
        }
    }

    fn clamp_point_to_monitor(x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
        // SAFETY: 坐标按值传入；MONITORINFO 在栈上且 cbSize 已填，API 失败时回落原坐标。
        unsafe {
            let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };

            if GetMonitorInfoW(monitor, &mut mi).as_bool() {
                let rc = mi.rcWork;
                (
                    x.clamp(rc.left, rc.right - w),
                    y.clamp(rc.top, rc.bottom - h),
                )
            } else {
                (x, y)
            }
        }
    }

    /// 进入语音页：查一次模型文件 + 预装载识别器（摊掉 ~3s 装载延迟）+ 开轮询定时器。
    ///
    /// 模型没下载时预装载只写一句错误到快照里，不阻塞、不崩。
    ///
    /// # Safety
    ///
    /// 只能在 UI 线程调用，`hwnd` 必须是本候选栏窗口；它会操作候选栏的定时器与面板状态。
    unsafe fn enter_voice_page(this: &CandidateWindow, hwnd: HWND) {
        this.refresh_voice(true);
        speech::SpeechEngine::global_warmup();
        if !this.voice_timer.get()
            && SetTimer(Some(hwnd), VOICE_TIMER_ID, VOICE_TIMER_MS, None) != 0
        {
            this.voice_timer.set(true);
        }
        // 预装载立刻把状态置成 Loading 又置回 Idle：刷一遍，让面板显示「正在准备」。
        this.refresh_voice(false);
    }

    /// 离开语音页：关轮询定时器；`cancel_if_listening` 为 true 且仍在采集时丢弃本次
    /// （不让一个没人管的麦克风留在后台）。已发过 Stop 的路径传 false——那条路的
    /// 收尾由上屏负责。
    ///
    /// # Safety
    ///
    /// 只能在 UI 线程调用，`hwnd` 必须是本候选栏窗口；与 `enter_voice_page` 成对使用。
    unsafe fn leave_voice_page(this: &CandidateWindow, hwnd: HWND, cancel_if_listening: bool) {
        if this.voice_timer.get() {
            let _ = KillTimer(Some(hwnd), VOICE_TIMER_ID);
            this.voice_timer.set(false);
        }
        if cancel_if_listening
            && speech::SpeechEngine::global_snapshot().state == SpeechState::Listening
        {
            speech::SpeechEngine::global_cancel();
        }
        this.refresh_voice(false);
    }

    /// 语音页主按钮：待命 / 装载中 → 开始识别；聆听中 → 结束并上屏。
    ///
    /// 「结束并上屏」的收尾（finalize → F24 上屏 → toast）在工作线程里做，
    /// 面板这边随即收起：上屏后宿主会结束组合，候选栏本就该消失。
    ///
    /// # Safety
    ///
    /// 只能在 UI 线程调用；会改动识别器全局状态与面板布局。
    unsafe fn toggle_voice(this: &CandidateWindow, hwnd: HWND) {
        if this.voice.borrow().state == SpeechState::Listening {
            speech::SpeechEngine::global_stop();
            Self::cancel_panel_anim(this, hwnd);
            this.collapse_panel();
            Self::leave_voice_page(this, hwnd, false);
        } else {
            speech::SpeechEngine::global_start();
            this.refresh_voice(false);
        }
        Self::relayout_and_repaint(this, hwnd);
    }

    /// 切换面板展开/收起（菜单按钮与面板快捷键共用，含动画与防抖）。
    ///
    /// 动画进行中再触发：直接落到目标态，不叠加动画（防抖）。
    fn toggle_panel(this: &CandidateWindow, hwnd: HWND) {
        // 动画进行中再点：直接落到目标态，不叠加动画（防抖）。
        if this.panel_anim.get().is_some() {
            unsafe { Self::cancel_panel_anim(this, hwnd) };
            if this.panel_visible.get() {
                // 收起动画中触发 → 留在展开态。
                unsafe { Self::relayout_and_repaint(this, hwnd) };
            } else {
                // 展开动画中触发 → 直接收起。
                this.collapse_panel();
                unsafe { Self::relayout_and_repaint(this, hwnd) };
            }
            return;
        }
        this.panel_visible.set(!this.panel_visible.get());
        if this.panel_visible.get() {
            this.panel_page.set(PanelPage::Menu);
            this.hovered_item.set(None);
        }
        // 展开/收起都播一段淡入淡出 + 滑入滑出动画；
        // 收起要等动画播完才真正收窗（见 WM_TIMER 分支）。
        this.panel_anim.set(Some(PanelAnim {
            expanding: this.panel_visible.get(),
            frame: 0,
            total_frames: PANEL_ANIM_FRAMES,
        }));
        unsafe {
            if SetTimer(Some(hwnd), PANEL_ANIM_TIMER_ID, PANEL_ANIM_FRAME_MS, None) != 0 {
                this.panel_anim_timer.set(true);
            }
        }
        unsafe { Self::relayout_and_repaint(this, hwnd) };
    }

    /// 打断进行中的面板动画（若在跑）：停定时器、清状态。
    /// 面板随之直接落到当前状态（不打动画）；在收起动画中打断 = 留在展开态。
    ///
    /// # Safety
    ///
    /// 只能在 UI 线程调用，`hwnd` 必须是本候选栏窗口。
    unsafe fn cancel_panel_anim(this: &CandidateWindow, hwnd: HWND) {
        if this.panel_anim.get().is_some() {
            let _ = KillTimer(Some(hwnd), PANEL_ANIM_TIMER_ID);
            this.panel_anim.set(None);
            this.panel_anim_timer.set(false);
        }
    }

    /// 按当前面板状态重算窗口尺寸、缓存布局结果并重绘。
    /// 供面板展开/收起与 hover 重绘复用（须在 UI 线程调用）。
    ///
    /// # Safety
    ///
    /// 只能在 UI 线程调用，`hwnd` 必须是本候选栏窗口；内部会借用 `this` 的多个 `RefCell`。
    unsafe fn relayout_and_repaint(this: &CandidateWindow, hwnd: HWND) {
        let model = this.model.borrow();
        let view = this.view.borrow();
        let view = match view.as_ref() {
            Some(v) => v,
            None => return,
        };
        let dpi = Self::get_dpi_for_window(hwnd);
        if let Ok(metrics) =
            calculate_client_rect(&view.dwrite_factory, &model, dpi, this.panel_layout_page())
        {
            // 面板从展开收回到收起时窗口会明显缩小：粘滞择位会认为「原位放得下」
            // 而原地缩窗，候选栏就停在面板展开时窗口的顶部位置、浮在半空（实机
            // bug：横向收起后面板不回光标）。此时必须当作「插入点变了」重新择位，
            // 让候选栏贴回插入行。
            let had_panel = this
                .metrics
                .borrow()
                .as_ref()
                .is_some_and(|m| m.panel_side.is_some());
            let panel_collapsed = had_panel && metrics.panel_side.is_none();
            // 面板处于展开状态时（含菜单→二级页切换、语音页状态刷新），窗口尺寸
            // 随页面内容变化：必须始终以插入点为锚重新择位，让窗口朝「远离光标」
            // 的方向生长——上方布局窗口底边钉在光标上沿、面板向上长；下方布局
            // 窗口顶边钉在光标下沿。若走粘滞「原位改尺寸」，窗口会向下/向右生长：
            // 面板盖住正在输入的文字，或把候选条带推离光标（实机 bug：二级菜单
            // 切换后位置错乱）。重新择位是确定性的（同一锚点+尺寸+上下偏好得到
            // 同一位置），hover/翻页等尺寸不变的 relayout 不会引起窗口挪动。
            // 面板全关时保持粘滞，候选数跳变仍不挪窗。
            let panel_open = metrics.panel_side.is_some();
            *this.metrics.borrow_mut() = Some(metrics.clone());
            Self::set_size_and_place(
                this,
                hwnd,
                metrics.hw_width as i32,
                metrics.hw_height as i32,
                panel_collapsed || panel_open,
            );
            if !model.items.is_empty() {
                let list = this.panel_list.borrow();
                let grid = this.panel_grid.borrow();
                let voice = this.voice.borrow();
                let _ = on_paint_with_metrics(
                    view,
                    &model,
                    dpi,
                    &metrics,
                    this.panel_paint_state(),
                    this.panel_progress(),
                    &list,
                    &grid,
                    &voice,
                );
            }
        }
    }
}

/// 读取缓存的布局结果中面板命中测试所需的
/// (候选栏宽, 候选栏高, 面板内容宽, 面板高, 面板方向)。
/// 面板高按当前页面取（「剪切板」子页比菜单页高）；无缓存（尚未绘制过候选）时
/// 面板宽度为 0，命中测试自然落空。
fn panel_metrics(this: &CandidateWindow) -> (f32, f32, f32, f32, PanelSide) {
    let page = this.panel_page.get();
    let metrics = this.metrics.borrow();
    match metrics.as_ref() {
        Some(m) => (
            m.width - m.panel_content_width,
            m.bar_height,
            m.panel_content_width,
            panel_height(page),
            m.panel_side.unwrap_or(PanelSide::Right),
        ),
        None => (0.0, 0.0, 0.0, panel_height(page), PanelSide::Right),
    }
}

