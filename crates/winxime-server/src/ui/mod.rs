//! 候选栏窗口模块入口：对外只暴露 `CandidateWindow` 与自定义消息常量。
//!
//! 内部拆分：`model` 数据模型 / `layout` 布局测量 / `paint` D2D 绘制 /
//! `view` 窗口与消息处理 / `panel` 菜单面板 / `glyph` 网格页
//! （表情 / 符号共用版式）/ `emoji`、`symbol` 两张内置字形表。

mod emoji;
mod glyph;
mod layout;
pub mod panel;
mod model;
mod paint;
mod symbol;
mod view;

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use tracing::info;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_TOPMOST, PostMessageW, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, WM_USER,
};
use winxime_ipc::Context;

use self::model::{CandidateModel, RenderedMetrics, RootModel};
use self::panel::{PanelGrid, PanelList, PanelPage};
use self::view::RenderedView;
use crate::recent_usage;

pub const WM_SHOW_CANDIDATE: u32 = WM_USER + 1;
pub const WM_HIDE_CANDIDATE: u32 = WM_USER + 2;
pub const WM_UPDATE_CANDIDATE: u32 = WM_USER + 3;
pub const WM_SET_POSITION: u32 = WM_USER + 4;
pub const WM_SHOW_ROOT: u32 = WM_USER + 5;
pub const WM_HIDE_ROOT: u32 = WM_USER + 6;

/// 共享布局常量（DIP）：layout / paint / view 共用。
pub(crate) const ROW_SPACING: f32 = 4.0;
pub(crate) const COL_SPACING: f32 = 8.0;
pub(crate) const MARGIN: f32 = 6.0;
pub(crate) const MIN_WIDTH: f32 = 120.0;
pub(crate) const BLUR_RADIUS: f32 = 8.0;

pub struct CandidateWindow {
    pub(crate) model: RefCell<CandidateModel>,
    pub(crate) root_model: RefCell<Option<RootModel>>,
    pub(crate) view: RefCell<Option<RenderedView>>,
    /// 面板展开状态（仅 UI 线程 wnd_proc 内读写）。
    pub(crate) panel_visible: Cell<bool>,
    /// 面板当前页面。
    pub(crate) panel_page: Cell<PanelPage>,
    /// 面板内 hover 的元素下标（菜单页卡片 / 列表子页条目行）。
    pub(crate) hovered_item: Cell<Option<usize>>,
    /// 列表子页数据（剪切板 / 快捷发送，进入该页时从 clipboard.db 读一次，
    /// 绘制只读内存；`PanelList::source` 记录这份数据属于哪个子页）。
    pub(crate) panel_list: RefCell<PanelList>,
    /// 网格子页状态（表情 / 符号：当前标签、当前页、最近使用记录；
    /// 进入该页时读一次 recent_usage.json，绘制只读内存）。
    pub(crate) panel_grid: RefCell<PanelGrid>,
    /// 最近一次布局结果（用于 ⋮ 按钮/面板/候选命中测试）。
    pub(crate) metrics: RefCell<Option<RenderedMetrics>>,
}

/// 面板绘制状态（面板展开时为 Some）：页面 + hover 元素下标。
///
/// 两者是同一次绘制的同一份状态，打包传递以免绘制接口的参数越加越长
/// （网格子页的标签 / 页码 / 最近使用另走 [`PanelGrid`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PanelPaintState {
    pub(crate) page: PanelPage,
    /// hover 的元素下标（菜单页卡片 / 列表子页条目行 / 网格子页格子）。
    pub(crate) hovered_item: Option<usize>,
}

unsafe impl Send for CandidateWindow {}
unsafe impl Sync for CandidateWindow {}

impl CandidateWindow {
    /// 当前面板绘制状态（展开时为 Some：页面 + hover 元素）。
    pub(crate) fn panel_paint_state(&self) -> Option<PanelPaintState> {
        if self.panel_visible.get() {
            Some(PanelPaintState {
                page: self.panel_page.get(),
                hovered_item: self.hovered_item.get(),
            })
        } else {
            None
        }
    }

    /// 当前面板页面（未展开时为 None）——布局据此决定面板区高度。
    pub(crate) fn panel_layout_page(&self) -> Option<PanelPage> {
        self.panel_visible.get().then(|| self.panel_page.get())
    }

    /// 收起面板并复位到菜单页（输入新内容/隐藏候选栏时调用）。
    pub(crate) fn collapse_panel(&self) {
        self.panel_visible.set(false);
        self.panel_page.set(PanelPage::Menu);
        self.hovered_item.set(None);
    }

    /// 重载列表子页数据（进入「剪切板」/「快捷发送」时调用一次：UI 线程内一次 SQLite
    /// 查询，之后绘制只读内存；面板绘制不允许逐帧触盘）。
    pub(crate) fn reload_panel_list(&self, page: PanelPage) {
        let items = match page {
            PanelPage::Clipboard => panel::load_clipboard_items(panel::CLIPBOARD_HISTORY_LIMIT),
            PanelPage::QuickSend => panel::load_quick_send_items(panel::QUICK_SEND_LIMIT),
            _ => Vec::new(),
        };
        *self.panel_list.borrow_mut() = PanelList {
            source: page,
            items,
            page: 0,
        };
    }

    /// 重载网格子页状态（进入「表情」/「符号」时调用一次）：标签回到「最近使用」、
    /// 页码回到第一页，并读一次该页的最近使用记录（内置字形表是常量，不读盘）。
    pub(crate) fn reload_panel_grid(&self, page: PanelPage) {
        let recent = page
            .grid_kind()
            .map_or_else(Vec::new, |kind| recent_usage::load(kind.recent_kind()));
        *self.panel_grid.borrow_mut() = PanelGrid {
            source: page,
            tab: 0,
            page: 0,
            recent,
        };
    }

    pub fn new() -> Arc<Self> {
        let window = Arc::new(Self {
            model: RefCell::new(CandidateModel::default()),
            root_model: RefCell::new(None),
            view: RefCell::new(None),
            panel_visible: Cell::new(false),
            panel_page: Cell::new(PanelPage::Menu),
            hovered_item: Cell::new(None),
            panel_list: RefCell::new(PanelList::default()),
            panel_grid: RefCell::new(PanelGrid::default()),
            metrics: RefCell::new(None),
        });

        // Initialize UI immediately in the thread that will run message loop
        window.ensure_view_initialized();

        window
    }

    fn ensure_view_initialized(&self) {
        if self.view.borrow().is_none() {
            let user_data_ptr = self as *const Self;
            match RenderedView::new(user_data_ptr.cast()) {
                Ok(view) => {
                    info!("UI initialized successfully");
                    *self.view.borrow_mut() = Some(view);
                }
                Err(e) => {
                    info!("Failed to initialize UI: {}", e);
                }
            }
        }
    }

    pub fn show(&self, x: i32, y: i32) {
        self.ensure_view_initialized();
        if let Some(view) = self.view.borrow().as_ref() {
            info!(
                "  show: hwnd={:?}, moving to ({}, {})",
                view.hwnd.0,
                x,
                y + 24
            );
            unsafe {
                let _ = SetWindowPos(
                    view.hwnd,
                    Some(HWND_TOPMOST),
                    x,
                    y + 24,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                );
                info!("  show: posting WM_SHOW_CANDIDATE");
                let result = PostMessageW(Some(view.hwnd), WM_SHOW_CANDIDATE, WPARAM(0), LPARAM(0));
                info!("  show: PostMessageW result: {:?}", result);
            }
        } else {
            info!("  show: view is None!");
        }
    }

    pub fn hide(&self) {
        if let Some(view) = self.view.borrow().as_ref() {
            unsafe {
                let _ = PostMessageW(Some(view.hwnd), WM_HIDE_CANDIDATE, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn update(&self, ctx: &Context) {
        if let Some(view) = self.view.borrow().as_ref() {
            info!(
                "  update: hwnd={:?}, posting WM_UPDATE_CANDIDATE",
                view.hwnd.0
            );
            unsafe {
                let ctx_ptr = Box::into_raw(Box::new(ctx.clone()));
                let result = PostMessageW(
                    Some(view.hwnd),
                    WM_UPDATE_CANDIDATE,
                    WPARAM(ctx_ptr as usize),
                    LPARAM(0),
                );
                info!("  update: PostMessageW result: {:?}", result);
                if result.is_err() {
                    let _ = Box::from_raw(ctx_ptr);
                    info!("  update: PostMessageW failed, freed memory");
                }
            }
        } else {
            info!("  update: view is None!");
        }
    }

    pub fn show_root(&self, letter: char, root: &str) -> Result<(), String> {
        self.ensure_view_initialized();
        if let Some(view) = self.view.borrow().as_ref() {
            let root_model = RootModel::from((letter, root.to_string()));
            *self.root_model.borrow_mut() = Some(root_model.clone());

            unsafe {
                let root_ptr = Box::into_raw(Box::new(root_model));
                let _ = PostMessageW(
                    Some(view.hwnd),
                    WM_SHOW_ROOT,
                    WPARAM(root_ptr as usize),
                    LPARAM(0),
                );
            }
            Ok(())
        } else {
            Err("view is None".to_string())
        }
    }

    pub fn hide_root(&self) {
        if let Some(view) = self.view.borrow().as_ref() {
            *self.root_model.borrow_mut() = None;
            unsafe {
                let _ = PostMessageW(Some(view.hwnd), WM_HIDE_ROOT, WPARAM(0), LPARAM(0));
            }
        }
    }
}
