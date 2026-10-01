//! 候选栏窗口：窗口/交换链/合成设备创建与全部消息处理（含菜单面板鼠标交互）。

use tracing::{debug, info};

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
    DefWindowProcW, GetMessagePos, GetWindowLongPtrW, HTCLIENT, KillTimer, LoadCursorW,
    RegisterClassW, SetCursor, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, HWND_TOPMOST, IDC_ARROW,
    IDC_HAND, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNA,
    WINDOWPOS, WM_DESTROY, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCCREATE, WM_PAINT, WM_SETCURSOR,
    WM_TIMER, WM_WINDOWPOSCHANGING, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows_core::{HSTRING, Interface, PCWSTR};

use super::layout::{calculate_client_rect, calculate_root_rect};
use super::model::{CandidateModel, RootModel};
use super::paint::{on_paint, on_paint_root, on_paint_with_metrics};
use super::panel::{
    dispatch_action, menu_item_id, menu_item_label, panel_height, panel_hit, rect_contains,
    window_to_panel, MenuAction, PanelHit, PanelPage,
};
use super::{
    CandidateWindow, BLUR_RADIUS, WM_HIDE_CANDIDATE, WM_HIDE_ROOT, WM_SET_POSITION,
    WM_SHOW_CANDIDATE, WM_SHOW_ROOT, WM_UPDATE_CANDIDATE,
};
use crate::speech::{self, SpeechState};
use winxime_ipc::Context;

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

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_NCCREATE => {
                let cs = lparam.0 as *const CREATESTRUCTW;
                if !cs.is_null() {
                    let user_data = unsafe { (*cs).lpCreateParams };
                    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, user_data as isize) };
                }
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_SHOW_CANDIDATE => {
                debug!("WM_SHOW_CANDIDATE received, hwnd={:?}", hwnd.0);
                let result = ShowWindow(hwnd, SW_SHOWNA);
                debug!("ShowWindow(SW_SHOWNA) result: {:?}", result);
                LRESULT(0)
            }
            WM_HIDE_CANDIDATE => {
                debug!("WM_HIDE_CANDIDATE received");
                let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                if !this_ptr.is_null() {
                    let this = &*this_ptr;
                    // 宿主收起候选栏（切窗口 / 组合结束）时若还停在语音页：
                    // 关定时器 + 丢弃本次采集（不留没人管的麦克风）。
                    if this.panel_page.get() == PanelPage::VoiceInput {
                        Self::leave_voice_page(this, hwnd, true);
                    }
                    this.collapse_panel();
                    this.metrics.replace(None);
                }
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_UPDATE_CANDIDATE => {
                debug!("WM_UPDATE_CANDIDATE received");
                let ctx_ptr = wparam.0 as *mut Context;
                if !ctx_ptr.is_null() {
                    let ctx = Box::from_raw(ctx_ptr);
                    debug!("ctx.candidates: {} items", ctx.candidates.candies.len());
                    let model = CandidateModel::from(&*ctx);
                    info!("  model.items: {:?}", model.items);

                    let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                    if !this_ptr.is_null() {
                        (*this_ptr).model.replace(model.clone());
                        // 输入新内容时收起面板（与 macOS 版候选刷新行为一致）。
                        // 语音页同理：用户开始打字 = 不要这次听写，丢弃并关定时器。
                        if (*this_ptr).panel_page.get() == PanelPage::VoiceInput {
                            Self::leave_voice_page(&*this_ptr, hwnd, true);
                        }
                        (*this_ptr).collapse_panel();

                        let view = (*this_ptr).view.borrow();
                        if let Some(view) = view.as_ref() {
                            let dpi = RenderedView::get_dpi_for_window(hwnd);
                            info!("  DPI: {}", dpi);
                            if let Ok(metrics) = calculate_client_rect(&view.dwrite_factory, &model, dpi, None) {
                                (*this_ptr).metrics.replace(Some(metrics.clone()));
                                info!(
                                    "  metrics.width: {}, metrics.height: {}",
                                    metrics.width, metrics.height
                                );
                                info!(
                                    "  metrics.hw_width: {}, metrics.hw_height: {}",
                                    metrics.hw_width, metrics.hw_height
                                );
                                info!("  metrics.item_widths: {:?}", metrics.item_widths);
                                let _ = SetWindowPos(
                                    hwnd,
                                    Some(HWND_TOPMOST),
                                    0,
                                    0,
                                    metrics.hw_width as i32,
                                    metrics.hw_height as i32,
                                    SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOCOPYBITS,
                                );
                                if !model.items.is_empty() {
                                    // 面板已在本次更新开始时收起，按纯候选栏布局绘制。
                                    let _ = on_paint_with_metrics(
                                        view,
                                        &model,
                                        dpi,
                                        &metrics,
                                        None,
                                        &(*this_ptr).panel_list.borrow(),
                                        &(*this_ptr).panel_grid.borrow(),
                                        &(*this_ptr).voice.borrow(),
                                    );
                                }
                            }
                        }
                    }
                    info!("  update complete");
                }
                LRESULT(0)
            }
            WM_SET_POSITION => {
                let x = wparam.0 as i32;
                let y = lparam.0 as i32;
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    x,
                    y + 24,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                );
                LRESULT(0)
            }
            WM_SHOW_ROOT => {
                debug!("WM_SHOW_ROOT received");
                let root_ptr = wparam.0 as *mut RootModel;
                if !root_ptr.is_null() {
                    let root = unsafe { Box::from_raw(root_ptr) };
                    debug!("showing root for '{}': {}", root.letter, root.root);

                    let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                    if !this_ptr.is_null() {
                        unsafe { (*this_ptr).root_model.replace(Some(*root.clone())) };

                        let view = unsafe { (*this_ptr).view.borrow() };
                        if let Some(view) = view.as_ref() {
                            let dpi = RenderedView::get_dpi_for_window(hwnd);
                            if let Ok(metrics) = calculate_root_rect(&view.dwrite_factory, &root, dpi) {
                                let _ = SetWindowPos(
                                    hwnd,
                                    Some(HWND_TOPMOST),
                                    0,
                                    0,
                                    metrics.hw_width as i32,
                                    metrics.hw_height as i32,
                                    SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOCOPYBITS,
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
                debug!("WM_HIDE_ROOT received");
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
                            let _ = SetWindowPos(
                                hwnd,
                                Some(HWND_TOPMOST),
                                0,
                                0,
                                metrics.hw_width as i32,
                                metrics.hw_height as i32,
                                SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOCOPYBITS,
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
                info!("WM_PAINT received");
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);

                let this_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateWindow;
                if !this_ptr.is_null() {
                    let model = (*this_ptr).model.borrow();
                    info!("  painting {} items", model.items.len());
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

                    // 面板区域点击：菜单卡片导航 / 返回菜单 / 触发动作 / 剪切板条目复制。
                    let (panel_width, bar_height, panel_height) = panel_metrics(this);
                    if let Some((lx, ly)) =
                        window_to_panel(BLUR_RADIUS, bar_height, panel_width, panel_height, pt_x, pt_y)
                    {
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
                                    crate::toast::show_toast(
                                        "曦码·曜输入法",
                                        &format!("「{label}」功能暂未开放"),
                                    );
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
                                    tracing::debug!("语音页点击被忽略：当前状态不可开始/结束");
                                }
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
                                    this.collapse_panel();
                                    Self::relayout_and_repaint(this, hwnd);
                                    let chars = text.chars().count();
                                    let body = if committed {
                                        format!("已上屏（{} 字）", chars)
                                    } else if copied {
                                        "已复制到剪贴板，按 Ctrl+V 粘贴".to_string()
                                    } else {
                                        "上屏失败：剪贴板被其他程序占用，请重试".to_string()
                                    };
                                    crate::toast::show_toast("曦码·曜输入法", &body);
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
                                    this.collapse_panel();
                                    Self::relayout_and_repaint(this, hwnd);
                                    let body = if committed {
                                        format!("已上屏：{glyph}")
                                    } else if copied {
                                        "已复制到剪贴板，按 Ctrl+V 粘贴".to_string()
                                    } else {
                                        "上屏失败：剪贴板被其他程序占用，请重试".to_string()
                                    };
                                    crate::toast::show_toast("曦码·曜输入法", &body);
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
                            this.panel_visible.set(!this.panel_visible.get());
                            if this.panel_visible.get() {
                                this.panel_page.set(PanelPage::Menu);
                                this.hovered_item.set(None);
                            }
                            Self::relayout_and_repaint(this, hwnd);
                            return LRESULT(0);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
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

                    // hover 反馈：菜单卡片 / 列表条目行 / 网格格子 / 语音页主按钮
                    // （其它子页无 hover 元素）。
                    let page = this.panel_page.get();
                    let hoverable = page == PanelPage::Menu
                        || page.is_list_page()
                        || page.is_grid_page()
                        || page == PanelPage::VoiceInput;
                    if this.panel_visible.get() && hoverable {
                        let scale = Self::get_dpi_for_window(hwnd) / 96.0;
                        let pt_x = (lparam.0 & 0xFFFF) as u16 as i16 as f32 / scale;
                        let pt_y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32 / scale;
                        let (panel_width, bar_height, panel_height) = panel_metrics(this);
                        let hovered_item = match window_to_panel(
                            BLUR_RADIUS,
                            bar_height,
                            panel_width,
                            panel_height,
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
                    if this.hovered_item.get().is_some() {
                        this.hovered_item.set(None);
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
                        let (panel_width, bar_height, panel_height) = panel_metrics(this);
                        let over_interactive = menu_btn.map_or(false, |rect| {
                            rect_contains(rect, px - BLUR_RADIUS, py - BLUR_RADIUS)
                        }) || window_to_panel(
                            BLUR_RADIUS,
                            bar_height,
                            panel_width,
                            panel_height,
                            px,
                            py,
                        )
                        .map_or(false, |(lx, ly)| {
                            let list = this.panel_list.borrow();
                            let grid = this.panel_grid.borrow();
                            panel_hit(this.panel_page.get(), panel_width, &list, &grid, lx, ly)
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

    pub(crate) fn get_dpi_for_window(hwnd: HWND) -> f32 {
        unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut dpi_x = 96u32;
            let mut dpi_y = 96u32;
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            dpi_x as f32
        }
    }

    fn get_dpi_for_point(point: POINT) -> f32 {
        unsafe {
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut dpi_x = 96u32;
            let mut dpi_y = 96u32;
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            dpi_x as f32
        }
    }

    fn clamp_point_to_monitor(x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
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
    unsafe fn toggle_voice(this: &CandidateWindow, hwnd: HWND) {
        if this.voice.borrow().state == SpeechState::Listening {
            speech::SpeechEngine::global_stop();
            this.collapse_panel();
            Self::leave_voice_page(this, hwnd, false);
        } else {
            speech::SpeechEngine::global_start();
            this.refresh_voice(false);
        }
        Self::relayout_and_repaint(this, hwnd);
    }

    /// 按当前面板状态重算窗口尺寸、缓存布局结果并重绘。
    /// 供面板展开/收起与 hover 重绘复用（须在 UI 线程调用）。
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
            *this.metrics.borrow_mut() = Some(metrics.clone());
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                metrics.hw_width as i32,
                metrics.hw_height as i32,
                SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOCOPYBITS,
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
                    &list,
                    &grid,
                    &voice,
                );
            }
        }
    }
}

/// 读取缓存的布局结果中面板命中测试所需的 (面板宽, 候选栏高, 面板高)。
/// 面板高按当前页面取（「剪切板」子页比菜单页高）；无缓存（尚未绘制过候选）时
/// 面板宽度为 0，命中测试自然落空。
fn panel_metrics(this: &CandidateWindow) -> (f32, f32, f32) {
    let page = this.panel_page.get();
    let metrics = this.metrics.borrow();
    match metrics.as_ref() {
        Some(m) => (m.width, m.bar_height, panel_height(page)),
        None => (0.0, 0.0, panel_height(page)),
    }
}

