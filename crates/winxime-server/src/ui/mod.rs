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
use tracing::{error, info, trace, warn};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_USER};
use winxime_ipc::Context;

use self::layout::Placement;
use self::model::{CandidateModel, RenderedMetrics, RootModel};
use self::panel::{PanelGrid, PanelList, PanelPage, VoiceView};
use self::view::RenderedView;
use crate::recent_usage;

pub const WM_SHOW_CANDIDATE: u32 = WM_USER + 1;
pub const WM_HIDE_CANDIDATE: u32 = WM_USER + 2;
pub const WM_SHOW_ROOT: u32 = WM_USER + 5;
pub const WM_HIDE_ROOT: u32 = WM_USER + 6;
/// 面板快捷键触发：展开/收起面板（含动画）。IPC 线程投递，UI 线程处理。
pub const WM_TOGGLE_PANEL: u32 = WM_USER + 7;

/// 一次「显示 / 刷新候选栏」的完整载荷：定位锚点 + 要显示的内容。
///
/// 锚点和内容必须走同一条消息。分成两条（先定位、再更新内容）时，同一轮刷新
/// 会摆两次窗口：先按**旧**尺寸摆一次，再按新尺寸摆一次。候选数从 3 变成 10
/// 时窗口高度差好几百 DIP，这两次之间的位移就是用户看到的「忽上忽下 / 闪」。
pub(crate) struct ShowRequest {
    /// `(插入点左边, 插入行下沿, 插入行高度)`，物理像素。
    pub(crate) anchor: (i32, i32, i32),
    /// 本次要显示的输入上下文（候选、组词串、高亮等）。
    pub(crate) context: Context,
}

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
    /// 语音页（🎙️）缓存状态：进页 / 定时器拍 / 点击后由
    /// [`CandidateWindow::refresh_voice`] 从语音引擎快照刷新，绘制只读它。
    pub(crate) voice: RefCell<VoiceView>,
    /// 语音页的定时器是否已开（聆听中靠定时器把 partial 文本刷到面板上）。
    pub(crate) voice_timer: Cell<bool>,
    /// 最近一次布局结果（用于 ⋮ 按钮/面板/候选命中测试）。
    pub(crate) metrics: RefCell<Option<RenderedMetrics>>,
    /// 候选栏定位锚点：`(插入点左边, 插入行下沿, 插入行高度)`，物理像素。
    ///
    /// 由 TSF 每次按键上报，`show` 时转存到这里；之后所有重排（面板展开、
    /// 切页、字根窗显示/收起、DPI 变化）都拿它重新择位，因此窗口挪到哪
    /// 不再依赖「上一次 SetWindowPos 留下的坐标」。
    pub(crate) anchor: Cell<Option<(i32, i32, i32)>>,
    /// 上一次择位的结果（`None` = 还没摆过）。
    ///
    /// 用途是「高度变了但别乱动」：候选数在几个到十个之间跳，窗口高度跟着跳，
    /// 但插入点没动、原位又放得下时就不要挪窗口（见 `layout::place_window_sticky`）。
    pub(crate) placement: Cell<Option<Placement>>,
    /// 面板展开/收起动画的进行状态（`None` = 没有动画在跑）。
    pub(crate) panel_anim: Cell<Option<PanelAnim>>,
    /// 面板动画定时器是否已开（防止重复 SetTimer）。
    pub(crate) panel_anim_timer: Cell<bool>,
}

/// 面板展开/收起动画状态：方向 + 当前帧（`0..=total_frames`）。
///
/// 展开时进度 0→1（淡入 + 滑入），收起时 1→0（淡出 + 滑出），
/// 播完由定时器分支收尾（收起时才真正收窗）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PanelAnim {
    /// true = 展开动画（面板从无到有），false = 收起动画。
    pub(crate) expanding: bool,
    pub(crate) frame: u32,
    pub(crate) total_frames: u32,
}

impl PanelAnim {
    /// 线性进度 0.0~1.0。
    pub(crate) fn linear(&self) -> f32 {
        self.frame as f32 / self.total_frames.max(1) as f32
    }

    /// ease-out 进度（三次方缓出）：起步快、收尾柔。
    pub(crate) fn progress(&self) -> f32 {
        let t = self.linear();
        1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
    }
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
    ///
    /// 注意：这只改状态不清定时器——收起动画播完后的收尾也走这里（那时动画
    /// 已清）；中途打断动画（隐藏候选栏等）由调用方先 `cancel_panel_anim`。
    pub(crate) fn collapse_panel(&self) {
        self.panel_visible.set(false);
        self.panel_page.set(PanelPage::Menu);
        self.hovered_item.set(None);
    }

    /// 面板当前可见度（0.0~1.0）：没有动画时恒为 1.0；收起动画按 1→0 倒放。
    pub(crate) fn panel_progress(&self) -> f32 {
        match self.panel_anim.get() {
            None => 1.0,
            Some(anim) if anim.expanding => anim.progress(),
            Some(anim) => 1.0 - anim.progress(),
        }
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

    /// 刷新语音页缓存；返回「有无变化」（调用方据此决定是否重绘）。
    ///
    /// `check_model` 只在进页时为 true：模型是否下载要看磁盘，定时器每拍不查盘。
    pub(crate) fn refresh_voice(&self, check_model: bool) -> bool {
        self.voice.borrow_mut().refresh(check_model)
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
            voice: RefCell::new(VoiceView::default()),
            voice_timer: Cell::new(false),
            metrics: RefCell::new(None),
            anchor: Cell::new(None),
            placement: Cell::new(None),
            panel_anim: Cell::new(None),
            panel_anim_timer: Cell::new(false),
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
                    error!("候选栏 Direct2D 初始化失败: {}", e);
                }
            }
        }
    }

    /// 显示（或刷新）候选栏：定位锚点与要显示的内容一起交给 UI 线程。
    ///
    /// `anchor` 是 `(插入点左边, 插入行下沿, 插入行高度)`，物理像素。这里不再
    /// 自己做任何偏移：真正的择位（贴下方 / 下方不够翻上方 / 夹进工作区）由
    /// UI 线程在拿到最新布局尺寸后统一算，见 `view::RenderedView::set_size_and_place`。
    ///
    /// 锚点与上下文合并在一条消息里，是因为分开投递会让同一轮刷新摆两次窗口：
    /// 先按旧尺寸摆、再按新尺寸摆，候选数跳变时这段位移就是用户看到的闪烁。
    pub fn show(&self, anchor: (i32, i32, i32), ctx: &Context) {
        self.ensure_view_initialized();
        if let Some(view) = self.view.borrow().as_ref() {
            trace!(
                "show: hwnd={:?}, anchor=({}, {}, h={}), {} 个候选",
                view.hwnd.0,
                anchor.0,
                anchor.1,
                anchor.2,
                ctx.candidates.candies.len()
            );
            unsafe {
                let req_ptr = Box::into_raw(Box::new(ShowRequest {
                    anchor,
                    context: ctx.clone(),
                }));
                let result = PostMessageW(
                    Some(view.hwnd),
                    WM_SHOW_CANDIDATE,
                    WPARAM(req_ptr as usize),
                    LPARAM(0),
                );
                if result.is_err() {
                    let _ = Box::from_raw(req_ptr);
                    warn!("show: WM_SHOW_CANDIDATE 投递失败，已释放请求内存");
                }
                trace!("show: PostMessageW result: {:?}", result);
            }
        } else {
            warn!("show: 候选栏视图尚未初始化");
        }
    }

    pub fn hide(&self) {
        if let Some(view) = self.view.borrow().as_ref() {
            unsafe {
                let _ = PostMessageW(Some(view.hwnd), WM_HIDE_CANDIDATE, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// 面板快捷键入口：把「切换面板」投到候选栏窗口（UI 线程处理，含动画）。
    ///
    /// 从 IPC 线程调用（`ipc_server::handle_toggle_panel`）。
    pub fn post_toggle_panel(&self) {
        if let Some(view) = self.view.borrow().as_ref() {
            unsafe {
                let _ = PostMessageW(Some(view.hwnd), WM_TOGGLE_PANEL, WPARAM(0), LPARAM(0));
            }
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

#[cfg(test)]
mod tests {
    use super::PanelAnim;

    #[test]
    fn panel_anim_progress_is_ease_out_and_bounded() {
        let anim = PanelAnim {
            expanding: true,
            frame: 0,
            total_frames: 8,
        };
        assert!((anim.progress() - 0.0).abs() < 1e-4, "首帧进度应为 0");

        let mid = PanelAnim { frame: 4, ..anim };
        let half = mid.progress();
        assert!(half > 0.5, "ease-out 前半段应过半（起步快）：{half}");

        let done = PanelAnim { frame: 8, ..anim };
        assert!((done.progress() - 1.0).abs() < 1e-4, "末帧进度应为 1");
        // 单调不减：进度随帧递增。
        let mut prev = 0.0;
        for frame in 0..=8 {
            let a = PanelAnim { frame, ..anim };
            let p = a.progress();
            assert!(p >= prev - 1e-4, "进度应单调不减：frame={frame} p={p}");
            prev = p;
        }
    }

    #[test]
    fn panel_anim_total_frames_zero_does_not_divide_by_zero() {
        let anim = PanelAnim {
            expanding: false,
            frame: 0,
            total_frames: 0,
        };
        // total_frames 为 0 时 max(1) 兜底，不得 panic、进度应落在 0~1。
        let p = anim.progress();
        assert!((0.0..=1.0).contains(&p), "进度应落在 0~1：{p}");
    }
}
