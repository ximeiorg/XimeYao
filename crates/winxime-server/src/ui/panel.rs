//! 候选栏菜单面板（布局与 macOS 版 XimeYi 的 candidate_window.rs 面板对齐）。
//!
//! 本模块只包含面板自身的常量、布局几何、页面模型与绘制；
//! 面板状态（展开/页面/hover）挂在 `ui::CandidateWindow` 上，
//! 鼠标交互在 `ui::RenderedView::wnd_proc` 中借助本模块的几何函数完成，
//! 保证绘制与命中测试共用同一套布局来源。

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory1, IDWriteTextFormat, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows_core::{w, HSTRING};

use super::glyph::{self, GlyphKind};
use super::model::CandidateModel;
use super::PanelPaintState;
use crate::speech::SpeechState;

// ── 布局常量（与 macOS 版 candidate_window.rs 对齐，单位为 DIP）──
/// 候选栏右侧 "⋮" 菜单按钮。
pub(crate) const MENU_BUTTON_SIZE: f32 = 20.0;
pub(crate) const MENU_BUTTON_GAP: f32 = 4.0;
/// 面板区高：布局 = 标题栏 36 + 间距 8 + 4 行条目×32（行距 4）+ 间距 8 + 底部区 32 + 底边距 8。
// 面板高度收紧到内容实际需要：菜单页顶部 10 + 3 行卡片 104 + 间距 8
// + 品牌栏 32 + 底边距 8 = 162（菜单页无标题栏，子页面内容少、此高度足够）。
const PANEL_HEIGHT: f32 = 162.0;
/// 面板区与候选区之间的间距。
const PANEL_GAP: f32 = 4.0;
/// 面板标题栏高度。
const PANEL_HEADER_HEIGHT: f32 = 36.0;
/// 面板行统一高度：菜单卡片 / 底部入口条同高。
const PANEL_ITEM_HEIGHT: f32 = 32.0;
/// 行背景块之间的垂直间距。
const PANEL_ROW_GAP: f32 = 4.0;
/// 标题栏 / 条目行区 / 底部区之间的统一间距。
const PANEL_CONTENT_GAP: f32 = 8.0;
/// 面板底边距。
const PANEL_BOTTOM_MARGIN: f32 = 8.0;
/// 面板菜单列数。
const PANEL_MENU_COLUMNS: usize = 2;
/// 菜单两列卡片之间的水平间距。
const PANEL_MENU_COL_GAP: f32 = 8.0;
/// 面板内容区统一水平边距。
const PANEL_H_INSET: f32 = 10.0;
/// 面板展开时的窗口最小宽度：候选栏本身可能很窄，菜单卡片放不下。
pub(crate) const PANEL_MIN_WIDTH: f32 = 320.0;
/// 面板页面：返回按钮区域宽度/高度（非菜单页显示于标题栏右侧）。
const PANEL_BACK_WIDTH: f32 = 64.0;
const PANEL_BACK_HEIGHT: f32 = 24.0;

/// 「剪切板 / 快捷发送」列表子页每页可见条目数。
pub(crate) const LIST_ROWS_PER_PAGE: usize = 6;
/// 「剪切板」子页一次载入的历史条目上限（跨页浏览；完整管理仍在设置程序）。
pub(crate) const CLIPBOARD_HISTORY_LIMIT: usize = 100;
/// 「快捷发送」子页一次载入的条目上限（跨页浏览；增删仍在设置程序）。
pub(crate) const QUICK_SEND_LIMIT: usize = 100;
/// 单条目的最大显示字符数（上屏/复制的始终是全文，只有展示截断）。
const LIST_DISPLAY_MAX_CHARS: usize = 40;
/// 列表子页底部翻页条高度（与菜单卡片同高）。
const LIST_FOOTER_HEIGHT: f32 = PANEL_ITEM_HEIGHT;
/// 翻页按钮尺寸（右下角「上一页 / 下一页」）与页码文本宽度。
const LIST_PAGE_BUTTON_WIDTH: f32 = 60.0;
const LIST_PAGE_BUTTON_HEIGHT: f32 = 24.0;
const LIST_PAGE_LABEL_WIDTH: f32 = 72.0;
/// 「快捷发送」条目的触发编码列宽度（列间距另计）：仅当本页有条目带编码时才占位。
const QUICK_SEND_CODE_COL_WIDTH: f32 = 64.0;
const QUICK_SEND_CODE_COL_GAP: f32 = 8.0;
/// 列表子页面板高度：标题栏 + 间距 + 条目录 + 间距 + 翻页条 + 底边距。
const LIST_PANEL_HEIGHT: f32 = PANEL_HEADER_HEIGHT
    + PANEL_CONTENT_GAP
    + LIST_ROWS_PER_PAGE as f32 * (PANEL_ITEM_HEIGHT + PANEL_ROW_GAP)
    - PANEL_ROW_GAP
    + PANEL_CONTENT_GAP
    + LIST_FOOTER_HEIGHT
    + PANEL_BOTTOM_MARGIN;

// ── 网格页（表情 / 符号）：8 列网格 +（可选）翻页条 + 底部分类标签栏 ──
// 表情与符号共用这一套版式（见 ui::glyph），自上而下：标题栏 → 网格 →（翻页条）→ 标签栏，
// 标签栏贴面板底边（照系统表情面板的习惯：切分类时视线不用从网格上挪开）。
// 只有符号页留翻页条——表情每类恰好一页（见 GlyphKind::needs_paging），
// 不占这一行时底部整行都归标签栏。桌面候选栏不做滚动，一页放不下的分类用翻页条翻页，
// 与「剪切板 / 快捷发送」子页同一套外观与几何。
/// 标签栏每行高度与行间距。
const GRID_TAB_HEIGHT: f32 = 26.0;
const GRID_TAB_GAP: f32 = 4.0;
/// 格子行高（固定）与格间距。
const GRID_CELL_HEIGHT: f32 = 36.0;
const GRID_CELL_GAP: f32 = 4.0;
/// 格子列宽下限（纯防御：面板展开时宽度至少 PANEL_MIN_WIDTH，8 列算出来都 ≥ 34）。
const GRID_CELL_WIDTH_MIN: f32 = 26.0;
/// 网格区高度（4 行格子 + 行间距）。
const GRID_HEIGHT: f32 = glyph::ROWS as f32 * (GRID_CELL_HEIGHT + GRID_CELL_GAP) - GRID_CELL_GAP;

/// 标签栏总高（按标签行数：表情 1 行，符号 2 行）。
fn grid_tab_height(kind: GlyphKind) -> f32 {
    let rows = kind.tab_rows() as f32;
    rows * GRID_TAB_HEIGHT + (rows - 1.0).max(0.0) * GRID_TAB_GAP
}

/// 网格顶边（面板内坐标）：标题栏下空一个内容间距。
fn grid_top() -> f32 {
    PANEL_HEADER_HEIGHT + PANEL_CONTENT_GAP
}

/// 网格页翻页条的 y（面板内坐标）：只有需要翻页的页（符号页）才有这一行。
fn grid_footer_y(kind: GlyphKind) -> f32 {
    debug_assert!(
        kind.needs_paging(),
        "{kind:?} 不需要翻页条，没有它的几何"
    );
    grid_top() + GRID_HEIGHT + PANEL_CONTENT_GAP
}

/// 标签栏顶边（面板内坐标）：面板最底部的一块，翻页条（若有）之下。
fn grid_tab_top(kind: GlyphKind) -> f32 {
    let above = if kind.needs_paging() {
        grid_footer_y(kind) + LIST_FOOTER_HEIGHT
    } else {
        grid_top() + GRID_HEIGHT
    };
    above + PANEL_CONTENT_GAP
}

/// 网格页面板高度：标题栏 + 间距 + 网格 +（翻页条 + 间距）+ 标签栏 + 底边距。
fn grid_panel_height(kind: GlyphKind) -> f32 {
    grid_tab_top(kind) + grid_tab_height(kind) + PANEL_BOTTOM_MARGIN
}

/// 格子列宽：8 列 + 格间距铺满面板可用宽度（左右各留 PANEL_H_INSET）。
/// 列宽**不封顶**——封顶会让网格在宽面板下整体居中、左右各空出一大块
/// （面板宽度跟着候选栏走，宽候选栏下那两条空白特别显眼）。
fn grid_cell_width(panel_width: f32) -> f32 {
    let per_row = glyph::PER_ROW as f32;
    let usable = panel_width - 2.0 * PANEL_H_INSET;
    ((usable - (per_row - 1.0) * GRID_CELL_GAP) / per_row).max(GRID_CELL_WIDTH_MIN)
}

/// 标签栏每个栏位的宽度：单行时按标签数均分（表情页 7 个标签铺满一行），
/// 多行时固定一行 glyph::TABS_PER_ROW 个（符号页两行 9 + 9）。
fn grid_tab_width(kind: GlyphKind, panel_width: f32) -> f32 {
    let usable = (panel_width - 2.0 * PANEL_H_INSET).max(0.0);
    let columns = if kind.tab_rows() == 1 {
        kind.tab_count().max(1)
    } else {
        glyph::TABS_PER_ROW
    };
    usable / columns as f32
}

/// 分类标签栏第 `index` 栏的矩形（面板内坐标；行优先排两行，行序由上往下）。
fn grid_tab_rect(kind: GlyphKind, panel_width: f32, index: usize) -> (f32, f32, f32, f32) {
    let width = grid_tab_width(kind, panel_width);
    let row = index / glyph::TABS_PER_ROW;
    let col = index % glyph::TABS_PER_ROW;
    let left = PANEL_H_INSET + col as f32 * width;
    let top = grid_tab_top(kind) + row as f32 * (GRID_TAB_HEIGHT + GRID_TAB_GAP);
    (left, top, left + width, top + GRID_TAB_HEIGHT)
}

/// 格子矩形（面板内坐标）：当前页第 `index` 格，行优先铺满 8 列。
fn grid_cell_rect(panel_width: f32, index: usize) -> (f32, f32, f32, f32) {
    let per_row = glyph::PER_ROW;
    let cell_width = grid_cell_width(panel_width);
    let col = index % per_row;
    let row = index / per_row;
    let left = PANEL_H_INSET + col as f32 * (cell_width + GRID_CELL_GAP);
    let top = grid_top() + row as f32 * (GRID_CELL_HEIGHT + GRID_CELL_GAP);
    (left, top, left + cell_width, top + GRID_CELL_HEIGHT)
}

/// 面板页面（候选栏下方面板可承载多个页面，菜单页为默认首页）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PanelPage {
    /// 菜单（默认页，2 列功能入口）。
    #[default]
    Menu,
    /// 剪切板。
    Clipboard,
    /// 快捷发送。
    QuickSend,
    /// 表情。
    Emoji,
    /// 符号。
    Symbol,
    /// 语音输入。
    VoiceInput,
}

impl PanelPage {
    /// 面板页面。注意「设置」**不是页面**：它是菜单里的动作入口
    /// （见 [`MenuAction`]），点了就执行、不开子页。
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "clipboard" => Self::Clipboard,
            "quick_send" => Self::QuickSend,
            "emoji" => Self::Emoji,
            "symbol" => Self::Symbol,
            "voice_input" => Self::VoiceInput,
            _ => return None,
        })
    }

    fn title(&self) -> &'static str {
        match self {
            Self::Menu => "菜单",
            Self::Clipboard => "剪切板",
            Self::QuickSend => "快捷发送",
            Self::Emoji => "表情",
            Self::Symbol => "符号",
            Self::VoiceInput => "语音输入",
        }
    }

    /// 是否为「条目列表」子页（进入时读一次 clipboard.db，绘制列表 + 翻页条）。
    pub(crate) fn is_list_page(&self) -> bool {
        matches!(self, Self::Clipboard | Self::QuickSend)
    }

    /// 是否为「网格」子页（标签栏 + 8 列网格 + 翻页条：表情 / 符号）。
    pub(crate) fn is_grid_page(&self) -> bool {
        matches!(self, Self::Emoji | Self::Symbol)
    }

    /// 网格子页的数据来源（非网格页为 None）。
    pub(crate) fn grid_kind(&self) -> Option<GlyphKind> {
        match self {
            Self::Emoji => Some(GlyphKind::Emoji),
            Self::Symbol => Some(GlyphKind::Symbol),
            _ => None,
        }
    }

    /// 列表子页空态主文案。
    fn list_empty_text(&self) -> &'static str {
        match self {
            Self::Clipboard => "暂无剪贴板记录",
            Self::QuickSend => "暂无快捷发送内容",
            _ => "暂无内容",
        }
    }

    /// 列表子页空态补充说明（可选，画在主文案下一行）。
    fn list_empty_hint(&self) -> Option<&'static str> {
        match self {
            // 快捷发送的录入入口在设置程序（面板只负责消费与上屏）。
            Self::QuickSend => Some("在设置程序的「剪贴板 → 快捷发送」里添加短语"),
            _ => None,
        }
    }
}

/// 面板菜单项触发的动作（交由外部回调处理）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuAction {
    /// 打开设置程序。
    OpenSettings,
}

static PANEL_ACTION_CALLBACK: OnceLock<Arc<dyn Fn(MenuAction) + Send + Sync>> = OnceLock::new();

/// 设置面板动作回调（例如启动 winxime-setup）。
pub(crate) fn set_panel_action_callback(callback: Arc<dyn Fn(MenuAction) + Send + Sync>) {
    let _ = PANEL_ACTION_CALLBACK.set(callback);
}

/// 触发面板动作回调（未设置回调时忽略）。
pub(crate) fn dispatch_action(action: MenuAction) {
    if let Some(callback) = PANEL_ACTION_CALLBACK.get() {
        callback(action);
    }
}

/// 菜单页顶部留白（面板内坐标）：菜单页无标题栏，内容直接从顶部开始。
const PANEL_MENU_TOP: f32 = 10.0;

/// 菜单页第 i 行（从顶部数）的 y（面板内坐标，y 向下）：行高 32、行距 4。
/// 菜单卡片与底部入口条共用同一套行距节奏。
fn panel_menu_row_y(i: usize) -> f32 {
    PANEL_MENU_TOP + i as f32 * (PANEL_ITEM_HEIGHT + PANEL_ROW_GAP)
}

/// 底部入口条（菜单页品牌栏）的 y。
fn panel_footer_y() -> f32 {
    PANEL_HEIGHT - PANEL_BOTTOM_MARGIN - PANEL_ITEM_HEIGHT
}

/// 面板高度（按页面）：菜单页为收紧后的 198；列表子页按可见条目数 + 翻页条扩展；
/// 网格子页按标签行数 + 4 行网格 + 翻页条扩展（表情 1 行标签，符号 2 行）。
pub(crate) fn panel_height(page: PanelPage) -> f32 {
    match page {
        PanelPage::Emoji => grid_panel_height(GlyphKind::Emoji),
        PanelPage::Symbol => grid_panel_height(GlyphKind::Symbol),
        page if page.is_list_page() => LIST_PANEL_HEIGHT,
        _ => PANEL_HEIGHT,
    }
}

/// 面板展开时相对候选栏增加的窗口高度（DIP）。
pub(crate) fn panel_extra_height(page: PanelPage) -> f32 {
    PANEL_GAP + panel_height(page)
}

/// 列表子页的一行：全文 + 触发编码（剪切板历史无编码，快捷发送条目可带编码）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PanelListItem {
    /// 条目全文（上屏/复制用的是它，展示时截断）。
    pub(crate) text: String,
    /// 触发编码（快捷发送专有；剪切板历史恒为空串）。
    pub(crate) code: String,
}

/// 列表子页数据：进入该页时从 clipboard.db 读一次，
/// 绘帧只读内存（面板绘制在 UI 线程，不允许逐帧触盘）。
///
/// 「剪切板」与「快捷发送」共用同一份结构与翻页几何，只换数据来源：
/// `source` 记录这份数据属于哪个子页，绘制/命中只在对应页面里使用。
#[derive(Debug, Clone, Default)]
pub(crate) struct PanelList {
    /// 数据来源页面（`reload_panel_list` 写入）。
    pub(crate) source: PanelPage,
    /// 条目（最新在前；快捷发送为置顶优先）。
    pub(crate) items: Vec<PanelListItem>,
    /// 当前页码（0 起）。
    pub(crate) page: usize,
}

impl PanelList {
    /// 总页数（无条目时按 1 页显示空态）。
    pub(crate) fn page_count(&self) -> usize {
        if self.items.is_empty() {
            1
        } else {
            self.items.len().div_ceil(LIST_ROWS_PER_PAGE)
        }
    }

    /// 夹回范围内的当前页（列表变短后仍安全）。
    pub(crate) fn clamped_page(&self) -> usize {
        self.page.min(self.page_count().saturating_sub(1))
    }

    /// 当前页可见行数。
    pub(crate) fn rows_on_page(&self) -> usize {
        let start = self.clamped_page() * LIST_ROWS_PER_PAGE;
        self.items
            .len()
            .saturating_sub(start)
            .min(LIST_ROWS_PER_PAGE)
    }

    /// 当前页第 row 行的条目。
    pub(crate) fn item_at(&self, row: usize) -> Option<&PanelListItem> {
        let index = self.clamped_page() * LIST_ROWS_PER_PAGE + row;
        self.items.get(index)
    }

    /// 是否有条目带触发编码（决定「快捷发送」页是否留出编码列）。
    pub(crate) fn has_codes(&self) -> bool {
        self.items.iter().any(|item| !item.code.is_empty())
    }

    /// 是否还有上一页 / 下一页。
    pub(crate) fn has_prev_page(&self) -> bool {
        self.clamped_page() > 0
    }

    pub(crate) fn has_next_page(&self) -> bool {
        self.clamped_page() + 1 < self.page_count()
    }

    /// 翻页；已在首/末页返回 false（调用方据此跳过重绘）。
    pub(crate) fn prev_page(&mut self) -> bool {
        if self.has_prev_page() {
            self.page = self.clamped_page() - 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn next_page(&mut self) -> bool {
        if self.has_next_page() {
            self.page = self.clamped_page() + 1;
            true
        } else {
            false
        }
    }
}

/// 网格子页（表情 / 符号）的状态：当前标签、当前页、最近使用记录。
///
/// 与 [`PanelList`] 对称：进入该页时读一次（这里读的是 recent_usage.json，
/// 内置字形表是常量不用读），绘帧只读内存。两页共用这一份状态，
/// 靠 `source` 区分当前是表情还是符号。
#[derive(Debug, Clone, Default)]
pub(crate) struct PanelGrid {
    /// 数据来源页面（`reload_panel_grid` 写入）。
    pub(crate) source: PanelPage,
    /// 当前标签（0 = 最近使用；见 `ui::glyph`）。
    pub(crate) tab: usize,
    /// 当前页（0 起；在当前标签内翻页）。
    pub(crate) page: usize,
    /// 最近使用记录（进入页面时读一次，点一次更新一次）。
    pub(crate) recent: Vec<String>,
}

impl PanelGrid {
    /// 数据来源（非网格页为 None）。
    pub(crate) fn kind(&self) -> Option<GlyphKind> {
        self.source.grid_kind()
    }

    /// 这份数据是否属于当前页面（进页即 reload，这里是兜底）。
    pub(crate) fn is_live(&self, page: PanelPage) -> bool {
        self.source == page
    }

    /// 标签总数（含「最近使用」）。
    pub(crate) fn tab_count(&self) -> usize {
        self.kind().map_or(0, |kind| kind.tab_count())
    }

    /// 夹回范围内的当前标签（数据换了之后下标仍安全）。
    pub(crate) fn clamped_tab(&self) -> usize {
        self.kind()
            .map_or(0, |kind| glyph::clamped_tab(kind, self.tab))
    }

    /// 夹回范围内的当前页。
    pub(crate) fn clamped_page(&self) -> usize {
        self.kind().map_or(0, |kind| {
            glyph::clamped_page(kind, self.clamped_tab(), self.page, &self.recent)
        })
    }

    /// 当前标签的页数（空标签也画一页空态）。
    pub(crate) fn page_count(&self) -> usize {
        self.kind()
            .map_or(1, |kind| glyph::page_count(kind, self.clamped_tab(), &self.recent))
    }

    /// 当前标签当前页内有内容的格子数。
    pub(crate) fn items_on_page(&self) -> usize {
        self.kind().map_or(0, |kind| {
            glyph::items_on_page(kind, self.clamped_tab(), self.clamped_page(), &self.recent)
        })
    }

    /// 当前标签的条目总数（翻页条上的「共 N 个」）。
    pub(crate) fn item_count(&self) -> usize {
        self.kind()
            .map_or(0, |kind| glyph::item_count(kind, self.clamped_tab(), &self.recent))
    }

    /// 当前页第 `slot` 格的字形（空槽 / 越界为 None：绘制与命中共用）。
    pub(crate) fn item_at(&self, slot: usize) -> Option<&str> {
        glyph::item_at(
            self.kind()?,
            self.clamped_tab(),
            self.clamped_page(),
            slot,
            &self.recent,
        )
    }

    /// 当前是否停在「最近使用」标签（第 0 个）。
    pub(crate) fn is_recent_tab(&self) -> bool {
        self.clamped_tab() == 0
    }

    pub(crate) fn has_prev_page(&self) -> bool {
        self.clamped_page() > 0
    }

    pub(crate) fn has_next_page(&self) -> bool {
        self.clamped_page() + 1 < self.page_count()
    }

    /// 切标签（页码回到第一页）；返回是否真的变了。
    pub(crate) fn select_tab(&mut self, tab: usize) -> bool {
        let tab = self.kind().map_or(0, |kind| glyph::clamped_tab(kind, tab));
        let changed = tab != self.clamped_tab() || self.page != 0;
        self.tab = tab;
        self.page = 0;
        changed
    }

    /// 翻页；已在首/末页返回 false（调用方据此跳过重绘）。
    pub(crate) fn prev_page(&mut self) -> bool {
        if self.has_prev_page() {
            self.page = self.clamped_page() - 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn next_page(&mut self) -> bool {
        if self.has_next_page() {
            self.page = self.clamped_page() + 1;
            true
        } else {
            false
        }
    }

    /// 记录一次使用（落盘 + 刷新内存里的那份）。
    /// 「在最近使用标签里点按不重排」这条规则在调用方（见 `ui::view` 的点击分支）：
    /// 那是 UI 语义，不属于数据层。
    pub(crate) fn record_use(&mut self, value: &str) {
        let Some(kind) = self.kind() else {
            return;
        };
        self.recent = crate::recent_usage::record_use(kind.recent_kind(), value);
    }
}

/// 语音页（🎙️）绘制数据。
///
/// 与 [`PanelList`] / [`PanelGrid`] 同一条纪律：面板绘制在 UI 线程、逐帧只读内存，
/// 这份缓存由 `ui::CandidateWindow::refresh_voice` 在「进页 / 定时器拍 / 点击后」刷新
/// （唯一的全局状态读取口在 [`VoiceView::refresh`]）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct VoiceView {
    /// 语音引擎状态。
    pub(crate) state: SpeechState,
    /// 模型四件套是否已下载（进页时查一次，定时器里不查盘）。
    pub(crate) model_ready: bool,
    /// 实时识别文本。
    pub(crate) partial: String,
    /// 最近一次错误（模型缺失 / 麦克风打不开）。
    pub(crate) error: Option<String>,
    /// 最近一次上屏文本。
    pub(crate) last_commit: Option<String>,
    /// 输入电平 0~100（聆听中的电平条；快照里的值，UI 不再平滑）。
    pub(crate) level: u8,
}

/// 语音页的「状态色」：决定状态文字、电平条、主按钮的着色（页面只按它选画刷）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VoiceTone {
    /// 就绪待命。
    Ready,
    /// 正在听（主色高亮）。
    Listening,
    /// 正在装载模型。
    Loading,
    /// 用不了（模型没下载）——不是错误，但需要用户先做一件事。
    Blocked,
    /// 出错了（红色）。
    Error,
}

/// 文本区的语义：决定字号与颜色（实时文本最大最亮，占位语最淡）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VoiceTextKind {
    /// 实时识别中的文本（跟读效果）。
    Live,
    /// 已上屏的回执。
    Committed,
    /// 占位/引导语。
    Placeholder,
    /// 错误信息。
    Alert,
}

impl VoiceView {
    /// 从语音引擎快照刷新；`check_model` 为 true 时顺带查一次模型文件
    /// （进页时用；定时器每拍只读内存快照，不触盘）。
    /// 返回「与刷新前相比有无变化」——调用方据此决定要不要重绘。
    pub(crate) fn refresh(&mut self, check_model: bool) -> bool {
        let snapshot = crate::speech::SpeechEngine::global_snapshot();
        let before = self.clone();
        self.state = snapshot.state;
        self.partial = snapshot.partial;
        self.error = snapshot.error;
        self.last_commit = snapshot.last_commit;
        self.level = snapshot.level;
        if check_model {
            self.model_ready = crate::speech::SpeechEngine::global_model_ready();
        }
        *self != before
    }

    /// 是否正在采集（电平条 / 文案分支都看它）。
    pub(crate) fn listening(&self) -> bool {
        self.state == SpeechState::Listening
    }

    /// 状态色。
    pub(crate) fn tone(&self) -> VoiceTone {
        if self.error.is_some() {
            return VoiceTone::Error;
        }
        match self.state {
            SpeechState::Listening => VoiceTone::Listening,
            SpeechState::Loading => VoiceTone::Loading,
            SpeechState::Idle => {
                if self.model_ready {
                    VoiceTone::Ready
                } else {
                    VoiceTone::Blocked
                }
            }
        }
    }

    /// 主按钮文案：**短**（116px 胶囊里放得下），长句都交给状态行与底部提示。
    pub(crate) fn button_label(&self) -> &'static str {
        if self.listening() {
            return "结束并上屏";
        }
        match self.state {
            SpeechState::Loading => "准备中…",
            _ => {
                if self.model_ready {
                    "开始说话"
                } else {
                    "未下载模型"
                }
            }
        }
    }

    /// 主按钮能不能点：装载中、以及模型没下载时点它没用，直接不给点
    /// （点了只会写一句错误到快照，用户看到的还是"没反应"）。
    pub(crate) fn button_enabled(&self) -> bool {
        if self.listening() {
            return true;
        }
        match self.state {
            SpeechState::Loading => false,
            _ => self.model_ready,
        }
    }

    /// 状态行左侧文字（≤6 字，跟主按钮同居一行）。
    pub(crate) fn state_label(&self) -> &'static str {
        match self.tone() {
            VoiceTone::Error => "出错了",
            VoiceTone::Listening => "正在听…",
            VoiceTone::Loading => "正在准备",
            VoiceTone::Blocked => "模型未下载",
            VoiceTone::Ready => "麦克风就绪",
        }
    }

    /// 文本区内容 + 语义。
    pub(crate) fn text_view(&self) -> (String, VoiceTextKind) {
        if let Some(error) = &self.error {
            return (error.clone(), VoiceTextKind::Alert);
        }
        if self.listening() {
            if self.partial.trim().is_empty() {
                return ("请对着麦克风说话…".to_string(), VoiceTextKind::Placeholder);
            }
            return (self.partial.clone(), VoiceTextKind::Live);
        }
        if let Some(text) = &self.last_commit {
            return (format!("已上屏：{text}"), VoiceTextKind::Committed);
        }
        if !self.model_ready {
            return (
                "还没有语音模型：先在设置里下载一个".to_string(),
                VoiceTextKind::Placeholder,
            );
        }
        (
            "点「开始说话」，说完停顿就会自动上屏".to_string(),
            VoiceTextKind::Placeholder,
        )
    }

    /// 底部提示行（一行小字：怎么用 / 出了问题去哪）。
    pub(crate) fn footer_label(&self) -> &'static str {
        match self.tone() {
            VoiceTone::Error => "设置 → 语音转文本：下载模型或检查麦克风",
            VoiceTone::Listening => "说完停顿自动上屏 · Esc 取消",
            VoiceTone::Loading => "首次装载模型需要几秒，之后常驻",
            VoiceTone::Blocked => "设置 → 语音转文本：下载模型",
            VoiceTone::Ready => "语音只在本机识别，不出电脑",
        }
    }
}

/// 语音页状态行高度（🎙️ + 状态文字 + 主按钮同居一行）。
const VOICE_ROW_HEIGHT: f32 = 32.0;

/// 语音页主按钮宽度（右侧胶囊）。
const VOICE_BUTTON_WIDTH: f32 = 116.0;

/// 语音页主按钮高度。
const VOICE_BUTTON_HEIGHT: f32 = 28.0;

/// 输入电平条高度（状态行下面的一条细线）。
const VOICE_METER_HEIGHT: f32 = 4.0;

/// 文本区与电平条之间的间隙。
const VOICE_TEXT_GAP: f32 = 2.0;

/// 语音页底部提示行高度。
const VOICE_FOOTER_HEIGHT: f32 = 14.0;

/// 语音页状态行左侧的图标框宽度（🎙️ 用彩色 emoji 字体单独画）。
const VOICE_ICON_BOX: f32 = 22.0;

/// 语音页状态行矩形 (left, top, right, bottom)（面板内坐标）。
///
/// 整行都是点击目标：只让 116px 的按钮可点，鼠标要瞄，产品上没必要。
fn voice_row_rect(width: f32) -> (f32, f32, f32, f32) {
    let top = PANEL_HEADER_HEIGHT + PANEL_CONTENT_GAP;
    (
        PANEL_H_INSET,
        top,
        width - PANEL_H_INSET,
        top + VOICE_ROW_HEIGHT,
    )
}

/// 语音页主按钮矩形（状态行内右侧胶囊）。
fn voice_button_rect(width: f32) -> (f32, f32, f32, f32) {
    let row = voice_row_rect(width);
    let top = row.1 + (VOICE_ROW_HEIGHT - VOICE_BUTTON_HEIGHT) / 2.0;
    (
        row.2 - VOICE_BUTTON_WIDTH,
        top,
        row.2,
        top + VOICE_BUTTON_HEIGHT,
    )
}

/// 语音页状态文字矩形（图标右侧到按钮左侧）。
fn voice_label_rect(width: f32) -> (f32, f32, f32, f32) {
    let row = voice_row_rect(width);
    let button = voice_button_rect(width);
    (row.0 + VOICE_ICON_BOX, row.1, button.0 - PANEL_ROW_GAP, row.3)
}

/// 语音页输入电平条矩形（状态行之下、满内容宽）。
fn voice_meter_rect(width: f32) -> (f32, f32, f32, f32) {
    let row = voice_row_rect(width);
    (
        PANEL_H_INSET,
        row.3,
        width - PANEL_H_INSET,
        row.3 + VOICE_METER_HEIGHT,
    )
}

/// 语音页文本区矩形（电平条之下到提示行之上的整块）。
fn voice_text_rect(width: f32) -> (f32, f32, f32, f32) {
    let top = voice_meter_rect(width).3 + VOICE_TEXT_GAP;
    let bottom = PANEL_HEIGHT - PANEL_BOTTOM_MARGIN - VOICE_FOOTER_HEIGHT;
    (PANEL_H_INSET, top, width - PANEL_H_INSET, bottom)
}

/// 语音页底部提示行矩形（贴面板底边距）。
fn voice_footer_rect(width: f32) -> (f32, f32, f32, f32) {
    let bottom = PANEL_HEIGHT - PANEL_BOTTOM_MARGIN;
    (
        PANEL_H_INSET,
        bottom - VOICE_FOOTER_HEIGHT,
        width - PANEL_H_INSET,
        bottom,
    )
}

/// 列表子页第 row 行的 y（面板内坐标）。
fn list_row_y(row: usize) -> f32 {
    PANEL_HEADER_HEIGHT + PANEL_CONTENT_GAP + row as f32 * (PANEL_ITEM_HEIGHT + PANEL_ROW_GAP)
}

/// 列表子页第 row 行的矩形 (left, top, right, bottom)。
fn list_row_rect(width: f32, row: usize) -> (f32, f32, f32, f32) {
    let y = list_row_y(row);
    (PANEL_H_INSET, y, width - PANEL_H_INSET, y + PANEL_ITEM_HEIGHT)
}

/// 列表子页翻页条的 y。
fn list_footer_y() -> f32 {
    LIST_PANEL_HEIGHT - PANEL_BOTTOM_MARGIN - LIST_FOOTER_HEIGHT
}

/// 翻页按钮矩形（`footer_y` = 翻页条顶边）：`from_right` = 0 取最右侧（下一页），
/// 1 取其左侧（上一页）。列表子页与网格子页共用这一套外观，只有翻页条的 y 不同。
fn footer_page_button_rect(width: f32, footer_y: f32, from_right: usize) -> (f32, f32, f32, f32) {
    let y = footer_y + (LIST_FOOTER_HEIGHT - LIST_PAGE_BUTTON_HEIGHT) / 2.0;
    let right =
        width - PANEL_H_INSET - from_right as f32 * (LIST_PAGE_BUTTON_WIDTH + PANEL_MENU_COL_GAP);
    (
        right - LIST_PAGE_BUTTON_WIDTH,
        y,
        right,
        y + LIST_PAGE_BUTTON_HEIGHT,
    )
}

/// 「第 x/y 页」文本矩形（紧贴「上一页」左侧的固定宽度区）。
fn footer_page_label_rect(width: f32, footer_y: f32) -> (f32, f32, f32, f32) {
    let prev = footer_page_button_rect(width, footer_y, 1);
    let right = prev.0 - PANEL_MENU_COL_GAP;
    (right - LIST_PAGE_LABEL_WIDTH, prev.1, right, prev.3)
}

/// 「共 N 条 / 共 N 个」文本矩形（左侧，右边界不侵入页码区；只有一页时可占满整行）。
fn footer_count_rect(width: f32, footer_y: f32, page_count: usize) -> (f32, f32, f32, f32) {
    let right = if page_count > 1 {
        footer_page_label_rect(width, footer_y).0 - PANEL_MENU_COL_GAP
    } else {
        width - PANEL_H_INSET
    };
    (PANEL_H_INSET, footer_y, right, footer_y + LIST_FOOTER_HEIGHT)
}

/// 列表子页的翻页条矩形（列表页高度固定，y 由 [`list_footer_y`] 决定）。
fn list_page_button_rect(width: f32, from_right: usize) -> (f32, f32, f32, f32) {
    footer_page_button_rect(width, list_footer_y(), from_right)
}

/// 列表页翻页条的「第 x/y 页」矩形：只给单测核对三段不重叠（绘制走 `footer_*`）。
#[cfg(test)]
fn list_page_label_rect(width: f32) -> (f32, f32, f32, f32) {
    footer_page_label_rect(width, list_footer_y())
}

/// 同上：列表页翻页条的「共 N 条」矩形，只给单测用。
#[cfg(test)]
fn list_count_rect(width: f32, page_count: usize) -> (f32, f32, f32, f32) {
    footer_count_rect(width, list_footer_y(), page_count)
}

/// 「快捷发送」条目行内触发编码的矩形（行矩形左侧固定宽度区；仅在有编码时占位）。
fn list_code_rect(row_rect: (f32, f32, f32, f32), has_codes: bool) -> (f32, f32, f32, f32) {
    if !has_codes {
        return (row_rect.0, row_rect.1, row_rect.0, row_rect.3);
    }
    (
        row_rect.0 + 8.0,
        row_rect.1,
        row_rect.0 + 8.0 + QUICK_SEND_CODE_COL_WIDTH,
        row_rect.3,
    )
}

/// 条目正文的起始 x（在行矩形内；带编码列时右移，避开编码）。
fn list_text_left(row_rect: (f32, f32, f32, f32), has_codes: bool) -> f32 {
    if has_codes {
        row_rect.0 + 8.0 + QUICK_SEND_CODE_COL_WIDTH + QUICK_SEND_CODE_COL_GAP
    } else {
        row_rect.0 + 8.0
    }
}

/// 单条目的展示文本：控制字符（换行/制表符）折成空格（面板单行显示），超长截断。
pub(crate) fn list_display_text(text: &str) -> String {
    let mut out = String::new();
    let mut overflowed = false;
    for (i, ch) in text.chars().enumerate() {
        if i >= LIST_DISPLAY_MAX_CHARS {
            overflowed = true;
            break;
        }
        out.push(if ch.is_control() { ' ' } else { ch });
    }
    if overflowed {
        out.push('…');
    }
    out
}

/// 剪贴板数据库路径（剪切板历史与快捷发送共用同一张表；server 启动时注入）。
static CLIPBOARD_DB_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 注册剪贴板数据库路径（`%APPDATA%\Xime\clipboard.db`，与设置程序同一文件）。
/// 未注册时回退 xime-config 的默认路径。
pub(crate) fn set_clipboard_db_path(path: PathBuf) {
    let _ = CLIPBOARD_DB_PATH.set(path);
}

/// 剪贴板数据库路径（两个列表子页的数据源）。
fn clipboard_db_path() -> PathBuf {
    CLIPBOARD_DB_PATH
        .get()
        .cloned()
        .unwrap_or_else(xime_config::clipboard_store::default_db_path)
}

/// 读取「剪切板」子页要展示的历史（最新在前）。
/// 面板不是关键路径：读库失败按空列表处理，只记日志。
pub(crate) fn load_clipboard_items(limit: usize) -> Vec<PanelListItem> {
    match xime_config::clipboard_store::list_history(&clipboard_db_path(), limit) {
        Ok(items) => items
            .into_iter()
            .map(|item| PanelListItem {
                text: item.text,
                code: String::new(),
            })
            .collect(),
        Err(e) => {
            tracing::warn!("读取剪贴板历史失败: {e}");
            Vec::new()
        }
    }
}

/// 读取「快捷发送」子页要展示的条目（置顶优先、新在前；带触发编码）。
/// 与剪切板历史同一张表（`isQuickSend=1` 子集），增删仍在设置程序里做。
pub(crate) fn load_quick_send_items(limit: usize) -> Vec<PanelListItem> {
    match xime_config::clipboard_store::list_quick_send(&clipboard_db_path()) {
        Ok(items) => items
            .into_iter()
            .take(limit)
            .map(|item| PanelListItem {
                text: item.text,
                code: item.code,
            })
            .collect(),
        Err(e) => {
            tracing::warn!("读取快捷发送条目失败: {e}");
            Vec::new()
        }
    }
}

/// 面板页面标题栏右侧的 "← 菜单" 返回按钮矩形 (left, top, right, bottom)。
fn panel_back_rect(width: f32) -> (f32, f32, f32, f32) {
    let x = width - PANEL_BACK_WIDTH - PANEL_H_INSET;
    let y = (PANEL_HEADER_HEIGHT - PANEL_BACK_HEIGHT) / 2.0;
    (x, y, x + PANEL_BACK_WIDTH, y + PANEL_BACK_HEIGHT)
}

/// 矩形 (left, top, right, bottom) 是否包含点。
pub(crate) fn rect_contains(r: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= r.0 && x <= r.2 && y >= r.1 && y <= r.3
}

/// 面板菜单项（2 列布局）：id 供点击逻辑区分功能。
struct PanelMenuItem {
    id: &'static str,
    icon: &'static str,
    label: &'static str,
    rect: (f32, f32, f32, f32),
}

/// 面板菜单布局：2 列 × 3 行（6 个功能），图标 + 文字。
/// 菜单项与 macOS 版 MENU_DEFS 一致（后续逐个接入实际功能）：
///     📋 剪切板 / 🚀 快捷发送 / 😀 表情 / 🔣 符号 / 🎙️ 语音输入 / ⚙️ 设置
/// 「🧮 计算器」按用户要求从菜单里去掉（该功能此前已下线，留着只会点出一句「暂未开放」）。
fn panel_menu_items(width: f32) -> Vec<PanelMenuItem> {
    const MENU_DEFS: [(&str, &str, &str); 6] = [
        ("clipboard", "📋", "剪切板"),
        ("quick_send", "🚀", "快捷发送"),
        ("emoji", "😀", "表情"),
        ("symbol", "🔣", "符号"),
        ("voice_input", "🎙️", "语音输入"),
        ("settings", "⚙️", "设置"),
    ];
    let col_count = PANEL_MENU_COLUMNS as f32;
    let col_w = (width - 2.0 * PANEL_H_INSET - (col_count - 1.0) * PANEL_MENU_COL_GAP) / col_count;

    MENU_DEFS
        .iter()
        .enumerate()
        .map(|(i, (id, icon, label))| {
            let col = i as f32 % col_count;
            let row = (i as f32 / col_count).floor() as usize;
            let x = PANEL_H_INSET + col * (col_w + PANEL_MENU_COL_GAP);
            let y = panel_menu_row_y(row);
            PanelMenuItem {
                id,
                icon,
                label,
                rect: (x, y, x + col_w, y + PANEL_ITEM_HEIGHT),
            }
        })
        .collect()
}

/// 菜单页第 index 张卡片对应的功能 id。
pub(crate) fn menu_item_id(index: usize, panel_width: f32) -> Option<&'static str> {
    panel_menu_items(panel_width).get(index).map(|item| item.id)
}

/// 菜单页第 index 张卡片的文案（与 [`menu_item_id`] 同源：点击后给人看的反馈要用它）。
pub(crate) fn menu_item_label(index: usize, panel_width: f32) -> Option<&'static str> {
    panel_menu_items(panel_width)
        .get(index)
        .map(|item| item.label)
}

/// 面板内命中的交互元素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelHit {
    /// 命中菜单页第 i 张卡片。
    MenuItem(usize),
    /// 命中 "← 菜单" 返回按钮。
    Back,
    /// 命中列表子页（剪切板 / 快捷发送）当前页第 i 行（行号按当前页计）。
    ListItem(usize),
    /// 命中网格子页（表情 / 符号）第 i 个分类标签。
    GlyphTab(usize),
    /// 命中网格子页当前页第 i 格（格内下标，行优先）。
    GlyphCell(usize),
    /// 命中语音页主按钮（点一下开始 / 结束识别）。
    VoiceToggle,
    /// 命中「上一页」（列表子页与网格子页共用底部翻页条）。
    PrevPage,
    /// 命中「下一页」。
    NextPage,
}

/// 面板命中测试（绘制几何与点击/hover 共用的唯一布局来源）。
/// `x`/`y` 为面板内坐标（相对面板左上角，见 [`window_to_panel`]）。
/// `list` 提供列表子页的条目数与页码（首/末页的翻页按钮不可命中）。
/// `grid` 提供网格子页的标签 / 页码 / 最近使用记录（空槽既不画也不可命中）。
pub(crate) fn panel_hit(
    page: PanelPage,
    panel_width: f32,
    list: &PanelList,
    grid: &PanelGrid,
    x: f32,
    y: f32,
) -> Option<PanelHit> {
    match page {
        PanelPage::Menu => panel_menu_items(panel_width)
            .iter()
            .enumerate()
            .find(|(_, item)| rect_contains(item.rect, x, y))
            .map(|(i, _)| PanelHit::MenuItem(i)),
        page if page.is_grid_page() => {
            // 返回按钮先判：它和网格在纵向不重叠，但先判更省事、意图也更清楚。
            if rect_contains(panel_back_rect(panel_width), x, y) {
                return Some(PanelHit::Back);
            }
            let kind = page.grid_kind()?;
            // 数据与页面对不上时（进页即 reload，这里是兜底）标签仍可点，
            // 但格子数按 0 算——不画出内容的格子也不该点得中。
            let live = grid.is_live(page);
            let tabs = if live { grid.tab_count() } else { kind.tab_count() };
            let tab = (0..tabs)
                .find(|i| rect_contains(grid_tab_rect(kind, panel_width, *i), x, y));
            if let Some(i) = tab {
                return Some(PanelHit::GlyphTab(i));
            }
            let items = if live { grid.items_on_page() } else { 0 };
            let cell = (0..items).find(|i| rect_contains(grid_cell_rect(panel_width, *i), x, y));
            if let Some(i) = cell {
                return Some(PanelHit::GlyphCell(i));
            }
            // 翻页按钮只在真的有这一行（符号页）且不止一页时可命中。
            if live && kind.needs_paging() && grid.page_count() > 1 {
                let footer_y = grid_footer_y(kind);
                if grid.has_prev_page()
                    && rect_contains(footer_page_button_rect(panel_width, footer_y, 1), x, y)
                {
                    return Some(PanelHit::PrevPage);
                }
                if grid.has_next_page()
                    && rect_contains(footer_page_button_rect(panel_width, footer_y, 0), x, y)
                {
                    return Some(PanelHit::NextPage);
                }
            }
            None
        }
        page if page.is_list_page() => {
            // 数据与页面对不上时按空态处理（进页即 reload，这里是兜底）。
            let rows = if list.source == page {
                list.rows_on_page()
            } else {
                0
            };
            let row =
                (0..rows).find(|row| rect_contains(list_row_rect(panel_width, *row), x, y));
            if let Some(row) = row {
                return Some(PanelHit::ListItem(row));
            }
            if list.source == page && list.page_count() > 1 {
                if list.has_prev_page() && rect_contains(list_page_button_rect(panel_width, 1), x, y)
                {
                    return Some(PanelHit::PrevPage);
                }
                if list.has_next_page() && rect_contains(list_page_button_rect(panel_width, 0), x, y)
                {
                    return Some(PanelHit::NextPage);
                }
            }
            if rect_contains(panel_back_rect(panel_width), x, y) {
                Some(PanelHit::Back)
            } else {
                None
            }
        }
        PanelPage::VoiceInput => {
            // 返回按钮先判（它在标题栏里，与状态行纵向不重叠）。
            if rect_contains(panel_back_rect(panel_width), x, y) {
                return Some(PanelHit::Back);
            }
            // 整条状态行都是点击目标（绘制侧的 hover 底色与它同源）。
            if rect_contains(voice_row_rect(panel_width), x, y) {
                return Some(PanelHit::VoiceToggle);
            }
            None
        }
        _ => {
            if rect_contains(panel_back_rect(panel_width), x, y) {
                Some(PanelHit::Back)
            } else {
                None
            }
        }
    }
}

/// 把窗口客户区 DIP 坐标换算为面板内坐标；点不在面板区域内时返回 None。
/// `blur_radius` 为窗口四周阴影留白，`bar_height` 为候选栏自身高度，
/// `panel_height` 为当前页面板高度（见 [`panel_height`]）。
pub(crate) fn window_to_panel(
    blur_radius: f32,
    bar_height: f32,
    panel_width: f32,
    panel_height: f32,
    x: f32,
    y: f32,
) -> Option<(f32, f32)> {
    if panel_width <= 0.0 {
        return None;
    }
    let lx = x - blur_radius;
    let ly = y - (blur_radius + bar_height + PANEL_GAP);
    if lx >= 0.0 && lx <= panel_width && ly >= 0.0 && ly <= panel_height {
        Some((lx, ly))
    } else {
        None
    }
}

/// 创建文本格式（可指定字号/字重/对齐）。
fn make_text_format(
    dwrite: &IDWriteFactory1,
    family: &HSTRING,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    align: DWRITE_TEXT_ALIGNMENT,
    vcenter: bool,
) -> Result<IDWriteTextFormat, String> {
    unsafe {
        let fmt = dwrite
            .CreateTextFormat(
                family,
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("zh-CN"),
            )
            .map_err(|e| format!("CreateTextFormat failed: {:?}", e))?;
        let _ = fmt.SetTextAlignment(align);
        if vcenter {
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
        Ok(fmt)
    }
}

/// 绘制候选栏下方的菜单面板（布局与 macOS 版一致：标题栏 + 2 列卡片 + 底部品牌栏）。
/// 面板内坐标均为 panel-local，绘制时加上面板原点偏移。
/// 列表子页（剪切板 / 快捷发送）读 `list`（内存快照，见 [`PanelList`]）；
/// 网格子页（表情 / 符号）读 `grid`（当前标签 / 页 / 最近使用，见 [`PanelGrid`]）
/// 加内置字形表（见 `ui::glyph`）。
pub(crate) fn draw_panel(
    d2d: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory1,
    model: &CandidateModel,
    blur_radius: f32,
    panel_width: f32,
    bar_height: f32,
    state: PanelPaintState,
    list: &PanelList,
    grid: &PanelGrid,
    voice: &VoiceView,
) -> Result<(), String> {
    let page = state.page;
    let hovered_item = state.hovered_item;
    unsafe {
        let panel_y = blur_radius + bar_height + PANEL_GAP;
        let panel_left = blur_radius;
        let panel_right = blur_radius + panel_width;
        let panel_height = panel_height(page);
        let radius = 8.0;

        // 面板背景（比候选栏略深的灰调，与 macOS 版一致）。
        let panel_bg_brush = d2d
            .CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.92,
                    g: 0.92,
                    b: 0.94,
                    a: 0.96,
                },
                None,
            )
            .map_err(|e| format!("CreateSolidColorBrush panel_bg failed: {:?}", e))?;
        let panel_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: panel_left,
                top: panel_y,
                right: panel_right,
                bottom: panel_y + panel_height,
            },
            radiusX: radius,
            radiusY: radius,
        };
        d2d.FillRoundedRectangle(&panel_rect, &panel_bg_brush);

        let fg = model.fg_color;
        let text_brush = d2d
            .CreateSolidColorBrush(&fg, None)
            .map_err(|e| format!("CreateSolidColorBrush panel_text failed: {:?}", e))?;
        let secondary_brush = d2d
            .CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: fg.r,
                    g: fg.g,
                    b: fg.b,
                    a: 0.55,
                },
                None,
            )
            .map_err(|e| format!("CreateSolidColorBrush panel_secondary failed: {:?}", e))?;
        let card_brush = d2d
            .CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: fg.r,
                    g: fg.g,
                    b: fg.b,
                    a: 0.06,
                },
                None,
            )
            .map_err(|e| format!("CreateSolidColorBrush panel_card failed: {:?}", e))?;
        let hover_brush = d2d
            .CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: model.highlight_bg_color.r,
                    g: model.highlight_bg_color.g,
                    b: model.highlight_bg_color.b,
                    a: 0.15,
                },
                None,
            )
            .map_err(|e| format!("CreateSolidColorBrush panel_hover failed: {:?}", e))?;
        let line_brush = d2d
            .CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: fg.r,
                    g: fg.g,
                    b: fg.b,
                    a: 0.06,
                },
                None,
            )
            .map_err(|e| format!("CreateSolidColorBrush panel_line failed: {:?}", e))?;
        // 语音页要的四支笔：主色（状态点/电平条/主按钮）、主色上的文字、
        // 错误红、以及最淡的提示行。面板底色是浅色硬编码（见上），
        // 所以错误红也用固定的高对比红，不跟主题走。
        let accent_brush = d2d
            .CreateSolidColorBrush(&model.highlight_bg_color, None)
            .map_err(|e| format!("CreateSolidColorBrush panel_accent failed: {:?}", e))?;
        let on_accent_brush = d2d
            .CreateSolidColorBrush(&model.highlight_fg_color, None)
            .map_err(|e| format!("CreateSolidColorBrush panel_on_accent failed: {:?}", e))?;
        let error_brush = d2d
            .CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.90,
                    g: 0.28,
                    b: 0.23,
                    a: 1.0,
                },
                None,
            )
            .map_err(|e| format!("CreateSolidColorBrush panel_error failed: {:?}", e))?;
        let faint_brush = d2d
            .CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: fg.r,
                    g: fg.g,
                    b: fg.b,
                    a: 0.40,
                },
                None,
            )
            .map_err(|e| format!("CreateSolidColorBrush panel_faint failed: {:?}", e))?;
        let meter_track_brush = d2d
            .CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: fg.r,
                    g: fg.g,
                    b: fg.b,
                    a: 0.14,
                },
                None,
            )
            .map_err(|e| format!("CreateSolidColorBrush panel_meter failed: {:?}", e))?;

        let center_format = make_text_format(
            dwrite,
            &model.font_family,
            model.font_size,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            true,
        )?;
        let left_format = make_text_format(
            dwrite,
            &model.font_family,
            model.font_size,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            true,
        )?;
        // 剪贴板条目是任意长文本：必须禁用换行，否则会折成多行溢出条目高度
        // （超出条目宽度的部分由布局直接裁掉）。
        let row_format = make_text_format(
            dwrite,
            &model.font_family,
            model.font_size - 1.0,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            true,
        )?;
        let _ = row_format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        // 翻页按钮文字（3 字）比候选字号小一档，保证 60px 按钮内不挤。
        let small_center_format = make_text_format(
            dwrite,
            &model.font_family,
            model.font_size - 2.0,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            true,
        )?;
        // 语音页的 🎙️ 用彩色 emoji 字体画（与菜单卡片图标同一套做法）；
        // 它画在 32px 的状态行里，所以比正文大不了多少。
        let voice_icon_format = make_text_format(
            dwrite,
            &HSTRING::from("Segoe UI Emoji"),
            (model.font_size + 1.0).max(12.0),
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            true,
        )?;
        // 语音页状态文字：半粗、一档正文大小（状态行里只有它和按钮）。
        let voice_label_format = make_text_format(
            dwrite,
            &model.font_family,
            model.font_size.max(12.0),
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            true,
        )?;
        // 语音页主按钮文字：居中、半粗、比正文小一档（116px 胶囊里放得下）。
        let voice_button_format = make_text_format(
            dwrite,
            &model.font_family,
            (model.font_size - 1.0).max(12.0),
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            true,
        )?;
        // 语音页识别文本：**比候选字大两档**——它是这一页唯一的主角。
        let voice_text_format = make_text_format(
            dwrite,
            &model.font_family,
            model.font_size + 2.0,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            true,
        )?;
        // 语音页底部提示行：最小的一档（只读信息，不抢视线）。
        let voice_footer_format = make_text_format(
            dwrite,
            &model.font_family,
            (model.font_size - 3.0).max(11.0),
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            true,
        )?;

        // 底部翻页条：列表子页与网格子页共用同一套外观——左边「共 N 条 / 共 N 个」，
        // 中间「第 x/y 页」，右边「上一页 / 下一页」按钮；只有翻页条的 y 不同，
        // 所以抽成闭包（捕获上面的画刷与文本格式，两处调用各传自己的 y 与文案）。
        let draw_footer = |footer_y: f32,
                           count_text: &str,
                           page_now: usize,
                           pages: usize,
                           has_prev: bool,
                           has_next: bool| {
            let footer_top = panel_y + footer_y;
            let footer_bottom = footer_top + LIST_FOOTER_HEIGHT;
            let count_rect = footer_count_rect(panel_width, footer_y, pages);
            let count_hstring = HSTRING::from(count_text);
            d2d.DrawText(
                &count_hstring,
                &left_format,
                &D2D_RECT_F {
                    left: panel_left + count_rect.0,
                    top: footer_top,
                    right: panel_left + count_rect.2,
                    bottom: footer_bottom,
                },
                &secondary_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            if pages > 1 {
                let label_rect = footer_page_label_rect(panel_width, footer_y);
                let page_hstring = HSTRING::from(format!("第 {}/{} 页", page_now + 1, pages));
                d2d.DrawText(
                    &page_hstring,
                    &left_format,
                    &D2D_RECT_F {
                        left: panel_left + label_rect.0,
                        top: footer_top,
                        right: panel_left + label_rect.2,
                        bottom: footer_bottom,
                    },
                    &secondary_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                // 上一页在右起第 2 位（from_right = 1），下一页在最右。
                let buttons = [
                    (1usize, "上一页", has_prev),
                    (0usize, "下一页", has_next),
                ];
                for (from_right, label, enabled) in buttons {
                    let rect = footer_page_button_rect(panel_width, footer_y, from_right);
                    if enabled {
                        let button_rect = D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: panel_left + rect.0,
                                top: panel_y + rect.1,
                                right: panel_left + rect.2,
                                bottom: panel_y + rect.3,
                            },
                            radiusX: radius,
                            radiusY: radius,
                        };
                        d2d.FillRoundedRectangle(&button_rect, &card_brush);
                    }
                    let label_hstring = HSTRING::from(label);
                    d2d.DrawText(
                        &label_hstring,
                        &small_center_format,
                        &D2D_RECT_F {
                            left: panel_left + rect.0,
                            top: panel_y + rect.1,
                            right: panel_left + rect.2,
                            bottom: panel_y + rect.3,
                        },
                        if enabled {
                            &text_brush
                        } else {
                            &secondary_brush
                        },
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }
        };

        match page {
            PanelPage::Menu => {
                // 菜单页：2 列功能入口卡片（行高/行距与 macOS 版一致），底部品牌栏补齐版面。
                // 卡片图标为 emoji 字符：候选字体（中文）没有 emoji 字形，会渲染成方框；
                // 必须用系统 emoji 字体 + 彩色字形选项（Win10+）。
                let emoji_family = HSTRING::from("Segoe UI Emoji");
                let icon_format = make_text_format(
                    dwrite,
                    &emoji_family,
                    model.font_size + 2.0,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                    true,
                )?;
                for (i, item) in panel_menu_items(panel_width).iter().enumerate() {
                    let rect = (
                        panel_left + item.rect.0,
                        panel_y + item.rect.1,
                        panel_left + item.rect.2,
                        panel_y + item.rect.3,
                    );
                    let card_rect = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: rect.0,
                            top: rect.1,
                            right: rect.2,
                            bottom: rect.3,
                        },
                        radiusX: radius,
                        radiusY: radius,
                    };
                    let card_brush = if hovered_item == Some(i) {
                        &hover_brush
                    } else {
                        &card_brush
                    };
                    d2d.FillRoundedRectangle(&card_rect, card_brush);

                    let icon_hstring = HSTRING::from(item.icon);
                    let mut icon_metrics = DWRITE_TEXT_METRICS::default();
                    dwrite
                        .CreateTextLayout(&icon_hstring, &icon_format, f32::MAX, f32::MAX)
                        .map_err(|e| format!("CreateTextLayout for menu icon failed: {:?}", e))?
                        .GetMetrics(&mut icon_metrics)
                        .map_err(|e| format!("GetMetrics for menu icon failed: {:?}", e))?;

                    let icon_x = rect.0 + 10.0;
                    let icon_w = icon_metrics.widthIncludingTrailingWhitespace;
                    d2d.DrawText(
                        &icon_hstring,
                        &icon_format,
                        &D2D_RECT_F {
                            left: icon_x,
                            top: rect.1,
                            right: icon_x + icon_w,
                            bottom: rect.3,
                        },
                        &text_brush,
                        D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );

                    let label_hstring = HSTRING::from(item.label);
                    d2d.DrawText(
                        &label_hstring,
                        &left_format,
                        &D2D_RECT_F {
                            left: icon_x + icon_w + 8.0,
                            top: rect.1,
                            right: rect.2,
                            bottom: rect.3,
                        },
                        &text_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }

                // 品牌栏：占据底部入口条分区（非交互），与 macOS 版一致。
                let brand_hstring = HSTRING::from("曦码·曜输入法");
                let brand_format = make_text_format(
                    dwrite,
                    &model.font_family,
                    model.font_size - 2.0,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                    true,
                )?;
                d2d.DrawText(
                    &brand_hstring,
                    &brand_format,
                    &D2D_RECT_F {
                        left: panel_left,
                        top: panel_y + panel_footer_y(),
                        right: panel_right,
                        bottom: panel_y + panel_footer_y() + PANEL_ITEM_HEIGHT,
                    },
                    &secondary_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            page => {
                // 子页面：标题（粗体）+ 返回按钮；内容按页面分支（剪切板为历史列表，其余占位）。
                let title_format = make_text_format(
                    dwrite,
                    &model.font_family,
                    model.font_size + 1.0,
                    DWRITE_FONT_WEIGHT_BOLD,
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                    true,
                )?;
                let title_hstring = HSTRING::from(page.title());
                d2d.DrawText(
                    &title_hstring,
                    &title_format,
                    &D2D_RECT_F {
                        left: panel_left + PANEL_H_INSET + 8.0,
                        top: panel_y,
                        right: panel_right,
                        bottom: panel_y + PANEL_HEADER_HEIGHT,
                    },
                    &text_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );

                // 标题栏细分隔线。
                d2d.FillRectangle(
                    &D2D_RECT_F {
                        left: panel_left + PANEL_H_INSET,
                        top: panel_y + PANEL_HEADER_HEIGHT - 0.5,
                        right: panel_right - PANEL_H_INSET,
                        bottom: panel_y + PANEL_HEADER_HEIGHT + 0.5,
                    },
                    &line_brush,
                );

                // "← 菜单" 返回按钮。
                let back = panel_back_rect(panel_width);
                let back_hstring = HSTRING::from("← 菜单");
                d2d.DrawText(
                    &back_hstring,
                    &left_format,
                    &D2D_RECT_F {
                        left: panel_left + back.0,
                        top: panel_y + back.1,
                        right: panel_left + back.2,
                        bottom: panel_y + back.3,
                    },
                    &secondary_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );

                match page {
                    PanelPage::Clipboard | PanelPage::QuickSend => {
                        // 列表子页（剪切板 / 快捷发送）：条目单行显示（超出裁掉）+ 底部翻页条。
                        // 点击条目 → 直接上屏（见 ui::view 点击分支）。
                        // 数据与页面对不上时按空态画（进页即 reload，这里是兜底）。
                        let rows = if list.source == page {
                            list.rows_on_page()
                        } else {
                            0
                        };
                        if rows == 0 {
                            // 空态：主文案居中；有补充说明时（快捷发送）再画一行指路。
                            let empty_cy = (list_row_y(0) + list_footer_y()) / 2.0;
                            let main_top = if page.list_empty_hint().is_some() {
                                empty_cy - PANEL_ITEM_HEIGHT / 2.0 - 10.0
                            } else {
                                empty_cy - PANEL_ITEM_HEIGHT / 2.0
                            };
                            let empty_hstring = HSTRING::from(page.list_empty_text());
                            d2d.DrawText(
                                &empty_hstring,
                                &center_format,
                                &D2D_RECT_F {
                                    left: panel_left,
                                    top: panel_y + main_top,
                                    right: panel_right,
                                    bottom: panel_y + main_top + PANEL_ITEM_HEIGHT,
                                },
                                &secondary_brush,
                                D2D1_DRAW_TEXT_OPTIONS_NONE,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                            if let Some(hint) = page.list_empty_hint() {
                                let hint_hstring = HSTRING::from(hint);
                                d2d.DrawText(
                                    &hint_hstring,
                                    &small_center_format,
                                    &D2D_RECT_F {
                                        left: panel_left,
                                        top: panel_y + main_top + PANEL_ITEM_HEIGHT + 4.0,
                                        right: panel_right,
                                        bottom: panel_y
                                            + main_top
                                            + PANEL_ITEM_HEIGHT
                                            + 4.0
                                            + PANEL_ITEM_HEIGHT,
                                    },
                                    &secondary_brush,
                                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                                    DWRITE_MEASURING_MODE_NATURAL,
                                );
                            }
                        }
                        // 「快捷发送」条目的触发编码列：仅当本列表确实有编码时占位
                        // （不按页判断，避免翻页时正文左右跳动）。
                        let code_col = page == PanelPage::QuickSend && list.has_codes();
                        for row in 0..rows {
                            let Some(item) = list.item_at(row) else {
                                continue;
                            };
                            let rect = list_row_rect(panel_width, row);
                            let card_rect = D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: panel_left + rect.0,
                                    top: panel_y + rect.1,
                                    right: panel_left + rect.2,
                                    bottom: panel_y + rect.3,
                                },
                                radiusX: radius,
                                radiusY: radius,
                            };
                            let brush = if hovered_item == Some(row) {
                                &hover_brush
                            } else {
                                &card_brush
                            };
                            d2d.FillRoundedRectangle(&card_rect, brush);

                            if code_col {
                                let code_rect = list_code_rect(rect, true);
                                let code_hstring = HSTRING::from(list_display_text(&item.code));
                                d2d.DrawText(
                                    &code_hstring,
                                    &row_format,
                                    &D2D_RECT_F {
                                        left: panel_left + code_rect.0,
                                        top: panel_y + code_rect.1,
                                        right: panel_left + code_rect.2,
                                        bottom: panel_y + code_rect.3,
                                    },
                                    &secondary_brush,
                                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                                    DWRITE_MEASURING_MODE_NATURAL,
                                );
                            }

                            let text_hstring = HSTRING::from(list_display_text(&item.text));
                            d2d.DrawText(
                                &text_hstring,
                                &row_format,
                                &D2D_RECT_F {
                                    left: panel_left + list_text_left(rect, code_col),
                                    top: panel_y + rect.1,
                                    right: panel_left + rect.2 - 8.0,
                                    bottom: panel_y + rect.3,
                                },
                                &text_brush,
                                D2D1_DRAW_TEXT_OPTIONS_NONE,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }

                        // 翻页条：左「共 N 条」，中「第 x/y 页」，右侧两个翻页按钮。
                        let pages = list.page_count();
                        draw_footer(
                            list_footer_y(),
                            &format!("共 {} 条", list.items.len()),
                            list.clamped_page(),
                            pages,
                            list.has_prev_page(),
                            list.has_next_page(),
                        );
                    }
                    page if page.is_grid_page() => {
                        // 网格页（表情 / 符号）：8 列网格 +（符号页的）翻页条 + 底部分类标签栏，
                        // 两页共用这段绘制；差别只有三处——字形字体（表情必须用系统 emoji
                        // 字体 + 彩色字形选项，中文字体里没有表情字形；符号用候选字体加大一档）、
                        // 标签栏行数（表情 1 行、符号 2 行）与是否要翻页条
                        // （见 GlyphKind::needs_paging：表情每类恰好一页，不占这一行）。
                        if let Some(kind) = page.grid_kind() {
                            let tab = grid.clamped_tab();
                            let tab_page = grid.clamped_page();
                            let pages = grid.page_count();
                            let tab_format = make_text_format(
                                dwrite,
                                &model.font_family,
                                model.font_size - 2.0,
                                DWRITE_FONT_WEIGHT_NORMAL,
                                DWRITE_TEXT_ALIGNMENT_CENTER,
                                true,
                            )?;
                            let glyph_family = if kind.uses_emoji_font() {
                                HSTRING::from("Segoe UI Emoji")
                            } else {
                                model.font_family.clone()
                            };
                            let glyph_size = if kind.uses_emoji_font() {
                                model.font_size + 4.0
                            } else {
                                model.font_size + 2.0
                            };
                            let glyph_format = make_text_format(
                                dwrite,
                                &glyph_family,
                                glyph_size,
                                DWRITE_FONT_WEIGHT_NORMAL,
                                DWRITE_TEXT_ALIGNMENT_CENTER,
                                true,
                            )?;

                            // 网格：只画当前页有内容的格子（空槽不画、也点不到）。
                            // 空标签（最近使用还没有记录）在网格区中央给一句提示。
                            let items = grid.items_on_page();
                            if items == 0 {
                                let empty = if grid.is_recent_tab() {
                                    glyph::RECENT_EMPTY_TEXT
                                } else {
                                    glyph::EMPTY_TEXT
                                };
                                let grid_top_y = panel_y + grid_top();
                                let empty_cy = grid_top_y + GRID_HEIGHT / 2.0;
                                let empty_hstring = HSTRING::from(empty);
                                d2d.DrawText(
                                    &empty_hstring,
                                    &small_center_format,
                                    &D2D_RECT_F {
                                        left: panel_left,
                                        top: empty_cy - PANEL_ITEM_HEIGHT / 2.0,
                                        right: panel_right,
                                        bottom: empty_cy + PANEL_ITEM_HEIGHT / 2.0,
                                    },
                                    &secondary_brush,
                                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                                    DWRITE_MEASURING_MODE_NATURAL,
                                );
                            }
                            for slot in 0..items {
                                let Some(glyph_text) = grid.item_at(slot) else {
                                    continue;
                                };
                                let rect = grid_cell_rect(panel_width, slot);
                                let cell_rect = D2D1_ROUNDED_RECT {
                                    rect: D2D_RECT_F {
                                        left: panel_left + rect.0,
                                        top: panel_y + rect.1,
                                        right: panel_left + rect.2,
                                        bottom: panel_y + rect.3,
                                    },
                                    radiusX: radius,
                                    radiusY: radius,
                                };
                                d2d.FillRoundedRectangle(
                                    &cell_rect,
                                    if hovered_item == Some(slot) {
                                        &hover_brush
                                    } else {
                                        &card_brush
                                    },
                                );
                                let glyph_hstring = HSTRING::from(glyph_text);
                                d2d.DrawText(
                                    &glyph_hstring,
                                    &glyph_format,
                                    &D2D_RECT_F {
                                        left: panel_left + rect.0,
                                        top: panel_y + rect.1,
                                        right: panel_left + rect.2,
                                        bottom: panel_y + rect.3,
                                    },
                                    &text_brush,
                                    if kind.uses_emoji_font() {
                                        D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT
                                    } else {
                                        D2D1_DRAW_TEXT_OPTIONS_NONE
                                    },
                                    DWRITE_MEASURING_MODE_NATURAL,
                                );
                            }

                            // 翻页条（只有符号页有这一行）：与列表子页同一套
                            // （只有一页时只画「共 N 个」）。
                            if kind.needs_paging() {
                                draw_footer(
                                    grid_footer_y(kind),
                                    &format!("共 {} 个", grid.item_count()),
                                    tab_page,
                                    pages,
                                    grid.has_prev_page(),
                                    grid.has_next_page(),
                                );
                            }

                            // 分类标签栏（面板最底部一块，翻页条之下）：
                            // 选中标签用 hover 底色 + 主文字色，其余用卡片底色。
                            for i in 0..grid.tab_count() {
                                let Some(label) = kind.tab_label(i) else {
                                    continue;
                                };
                                let rect = grid_tab_rect(kind, panel_width, i);
                                let active = i == tab;
                                let tab_rect = D2D1_ROUNDED_RECT {
                                    rect: D2D_RECT_F {
                                        left: panel_left + rect.0,
                                        top: panel_y + rect.1,
                                        right: panel_left + rect.2,
                                        bottom: panel_y + rect.3,
                                    },
                                    radiusX: radius,
                                    radiusY: radius,
                                };
                                d2d.FillRoundedRectangle(
                                    &tab_rect,
                                    if active { &hover_brush } else { &card_brush },
                                );
                                let label_hstring = HSTRING::from(label);
                                d2d.DrawText(
                                    &label_hstring,
                                    &tab_format,
                                    &D2D_RECT_F {
                                        left: panel_left + rect.0,
                                        top: panel_y + rect.1,
                                        right: panel_left + rect.2,
                                        bottom: panel_y + rect.3,
                                    },
                                    if active {
                                        &text_brush
                                    } else {
                                        &secondary_brush
                                    },
                                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                                    DWRITE_MEASURING_MODE_NATURAL,
                                );
                            }
                        }
                    }
                    PanelPage::VoiceInput => {
                        // 版式：状态行（🎙️ + 状态文字 + 主按钮）/ 电平条 / 识别文本 / 提示行。
                        // 顺序就是「我在哪 → 它在听吗 → 说了什么 → 接下来怎么用」。
                        let row = voice_row_rect(panel_width);
                        let button = voice_button_rect(panel_width);
                        let hovered = hovered_item == Some(0);
                        let tone = voice.tone();
                        let rounded = |rect: (f32, f32, f32, f32), radius: f32| D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: panel_left + rect.0,
                                top: panel_y + rect.1,
                                right: panel_left + rect.2,
                                bottom: panel_y + rect.3,
                            },
                            radiusX: radius,
                            radiusY: radius,
                        };
                        let rect_of = |rect: (f32, f32, f32, f32)| D2D_RECT_F {
                            left: panel_left + rect.0,
                            top: panel_y + rect.1,
                            right: panel_left + rect.2,
                            bottom: panel_y + rect.3,
                        };

                        // 整行是点击目标：悬停时铺一层浅底，让"这一整行都能点"看得见。
                        if hovered {
                            d2d.FillRoundedRectangle(&rounded(row, radius), &card_brush);
                        }

                        // 🎙️ 页面图标（彩色 emoji 字体，与菜单卡片图标同一套做法）。
                        let icon_hstring = HSTRING::from("🎙️");
                        d2d.DrawText(
                            &icon_hstring,
                            &voice_icon_format,
                            &rect_of((
                                row.0,
                                row.1,
                                row.0 + VOICE_ICON_BOX,
                                row.3,
                            )),
                            &text_brush,
                            D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );

                        // 状态文字：颜色带语义（聆听=主色、出错=红、不可用=次级）。
                        let label_hstring = HSTRING::from(voice.state_label());
                        let label_brush = match tone {
                            VoiceTone::Listening => &accent_brush,
                            VoiceTone::Error => &error_brush,
                            VoiceTone::Blocked | VoiceTone::Loading => &secondary_brush,
                            VoiceTone::Ready => &text_brush,
                        };
                        d2d.DrawText(
                            &label_hstring,
                            &voice_label_format,
                            &rect_of(voice_label_rect(panel_width)),
                            label_brush,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );

                        // 主按钮：可点 → 主色胶囊；不可点（装载中 / 未下载）→ 灰底次要文字。
                        let enabled = voice.button_enabled();
                        d2d.FillRoundedRectangle(
                            &rounded(button, VOICE_BUTTON_HEIGHT / 2.0),
                            if enabled { &accent_brush } else { &card_brush },
                        );
                        let button_hstring = HSTRING::from(voice.button_label());
                        d2d.DrawText(
                            &button_hstring,
                            &voice_button_format,
                            &rect_of(button),
                            if enabled {
                                &on_accent_brush
                            } else {
                                &secondary_brush
                            },
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );

                        // 电平条：只在聆听中画。用户要看到"它在听我说话"，
                        // 否则对着面板说话时完全不知道麦克风有没有进声音。
                        if voice.listening() {
                            let meter = voice_meter_rect(panel_width);
                            d2d.FillRoundedRectangle(
                                &rounded(meter, VOICE_METER_HEIGHT / 2.0),
                                &meter_track_brush,
                            );
                            let filled = (meter.2 - meter.0) * (voice.level.min(100) as f32 / 100.0);
                            if filled > 1.0 {
                                d2d.FillRoundedRectangle(
                                    &rounded(
                                        (meter.0, meter.1, meter.0 + filled, meter.3),
                                        VOICE_METER_HEIGHT / 2.0,
                                    ),
                                    &accent_brush,
                                );
                            }
                        }

                        // 识别文本 / 回执 / 引导语 / 错误：语义决定字号与颜色。
                        let (text_value, kind) = voice.text_view();
                        let (format, brush) = match kind {
                            VoiceTextKind::Live => (&voice_text_format, &text_brush),
                            VoiceTextKind::Committed => (&left_format, &text_brush),
                            VoiceTextKind::Placeholder => (&left_format, &secondary_brush),
                            VoiceTextKind::Alert => (&left_format, &error_brush),
                        };
                        let text_hstring = HSTRING::from(text_value);
                        d2d.DrawText(
                            &text_hstring,
                            format,
                            &rect_of(voice_text_rect(panel_width)),
                            brush,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );

                        // 底部提示行：怎么用（聆听中）/ 去哪解决（出错或没模型）。
                        let footer_hstring = HSTRING::from(voice.footer_label());
                        d2d.DrawText(
                            &footer_hstring,
                            &voice_footer_format,
                            &rect_of(voice_footer_rect(panel_width)),
                            &faint_brush,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                    _ => {
                        // 其它功能页面（v1 占位）：占位内容居中于标题栏与品牌栏之间。
                        let placeholder_hstring = HSTRING::from("功能开发中");
                        let rows_top = PANEL_HEADER_HEIGHT + PANEL_CONTENT_GAP;
                        let rows_bottom = panel_footer_y() - PANEL_ROW_GAP;
                        let placeholder_cy = (rows_top + rows_bottom) / 2.0;
                        d2d.DrawText(
                            &placeholder_hstring,
                            &center_format,
                            &D2D_RECT_F {
                                left: panel_left,
                                top: panel_y + placeholder_cy - PANEL_ITEM_HEIGHT / 2.0,
                                right: panel_right,
                                bottom: panel_y + placeholder_cy + PANEL_ITEM_HEIGHT / 2.0,
                            },
                            &secondary_brush,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                }
            }
        }

        Ok(())
    }
}

/// 绘制候选栏最右侧的 "⋮" 菜单按钮（仅横向布局）。
/// `rect` 为按钮在候选栏内容坐标系（原点=候选栏内容左上角）中的矩形。
pub(crate) fn draw_menu_button(
    d2d: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory1,
    model: &CandidateModel,
    blur_radius: f32,
    rect: (f32, f32, f32, f32),
) -> Result<(), String> {
    unsafe {
        let format = make_text_format(
            dwrite,
            &model.font_family,
            model.font_size + 2.0,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            true,
        )?;
        let brush = d2d
            .CreateSolidColorBrush(&model.selkey_color, None)
            .map_err(|e| format!("CreateSolidColorBrush menu_btn failed: {:?}", e))?;
        let buf: Vec<u16> = "⋮".encode_utf16().collect();
        d2d.DrawText(
            &buf,
            &format,
            &D2D_RECT_F {
                left: blur_radius + rect.0,
                top: blur_radius + rect.1,
                right: blur_radius + rect.2,
                bottom: blur_radius + rect.3,
            },
            &brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

/// 面板命中测试的薄封装：列表子页不读网格状态，这里固定给一份空网格
    /// （网格页自己的命中测试直接调 `panel_hit` 并传真正的 `PanelGrid`）。
    fn hit(page: PanelPage, w: f32, list: &PanelList, x: f32, y: f32) -> Option<PanelHit> {
        panel_hit(page, w, list, &PanelGrid::default(), x, y)
    }

    /// 一份「已进入该页」的网格状态（含最近使用记录），模拟 `reload_panel_grid` 的结果。
    fn grid_state(page: PanelPage, recent: &[&str]) -> PanelGrid {
        PanelGrid {
            source: page,
            tab: 0,
            page: 0,
            recent: recent.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn menu_items_layout_two_columns_three_rows() {
        let items = panel_menu_items(PANEL_MIN_WIDTH);
        assert_eq!(items.len(), 6);
        let col_w =
            (PANEL_MIN_WIDTH - 2.0 * PANEL_H_INSET - PANEL_MENU_COL_GAP) / 2.0;
        for (i, item) in items.iter().enumerate() {
            let col = i % PANEL_MENU_COLUMNS;
            let row = i / PANEL_MENU_COLUMNS;
            let expect_x = PANEL_H_INSET + col as f32 * (col_w + PANEL_MENU_COL_GAP);
            assert!((item.rect.0 - expect_x).abs() < 1e-4, "item {i} x");
            assert!((item.rect.1 - panel_menu_row_y(row)).abs() < 1e-4, "item {i} y");
            assert!((item.rect.2 - item.rect.0 - col_w).abs() < 1e-4, "item {i} w");
            assert!(
                (item.rect.3 - item.rect.1 - PANEL_ITEM_HEIGHT).abs() < 1e-4,
                "item {i} h"
            );
        }
        // 「计算器」已从菜单里去掉：id 里不能再出现它。
        assert!(items.iter().all(|item| item.id != "calculator"));
    }

    #[test]
    fn menu_rows_fit_above_footer() {
        // 最后一行菜单卡片不得与底部品牌栏重叠。
        let last_row_bottom = panel_menu_row_y(2) + PANEL_ITEM_HEIGHT;
        assert!(last_row_bottom <= panel_footer_y() - PANEL_ROW_GAP);
        // 顶部不留大空白：首行紧贴菜单页顶部留白之下。
        assert!(panel_menu_row_y(0) >= PANEL_MENU_TOP);
        assert!(panel_menu_row_y(0) < PANEL_HEADER_HEIGHT);
    }

    #[test]
    fn back_button_inside_header() {
        let back = panel_back_rect(PANEL_MIN_WIDTH);
        // (left, top, right, bottom)：右/底边不得越界面板，尺寸恰为常量值。
        assert!(back.1 >= 0.0 && back.3 <= PANEL_HEADER_HEIGHT);
        assert!(back.0 >= 0.0 && back.2 <= PANEL_MIN_WIDTH);
        assert!((back.2 - back.0 - PANEL_BACK_WIDTH).abs() < 1e-4);
        assert!((back.3 - back.1 - PANEL_BACK_HEIGHT).abs() < 1e-4);
    }

    #[test]
    fn panel_hit_routes_menu_and_back() {
        let w = PANEL_MIN_WIDTH;
        let items = panel_menu_items(w);
        let empty = PanelList::default();
        // 第 6 项（settings）在第 3 行第 2 列，命中其中心。
        let settings = &items[5];
        assert_eq!(
            hit(
                PanelPage::Menu,
                w,
                &empty,
                (settings.rect.0 + settings.rect.2) / 2.0,
                (settings.rect.1 + settings.rect.3) / 2.0,
            ),
            Some(PanelHit::MenuItem(5))
        );
        assert_eq!(menu_item_id(5, w), Some("settings"));
        // 子页面只有返回按钮可命中。
        let back = panel_back_rect(w);
        assert_eq!(
            hit(
                PanelPage::Emoji,
                w,
                &empty,
                (back.0 + back.2) / 2.0,
                (back.1 + back.3) / 2.0,
            ),
            Some(PanelHit::Back)
        );
        // 空白处不命中。
        assert_eq!(hit(PanelPage::Menu, w, &empty, 0.5, 0.5), None);
    }

    #[test]
    fn window_to_panel_maps_inside_and_outside() {
        let blur = 8.0;
        let bar_h = 30.0;
        let menu_h = panel_height(PanelPage::Menu);
        // 候选栏区域内不算面板。
        assert!(
            window_to_panel(blur, bar_h, 320.0, menu_h, 100.0, blur + bar_h / 2.0).is_none(),
            "bar area must not map to panel"
        );
        // 面板首行内可命中，返回面板内坐标。
        let (lx, ly) = window_to_panel(
            blur,
            bar_h,
            320.0,
            menu_h,
            blur + 10.0,
            blur + bar_h + PANEL_GAP + 5.0,
        )
        .unwrap();
        assert!((lx - 10.0).abs() < 1e-4);
        assert!((ly - 5.0).abs() < 1e-4);
        // 面板高度按页面：菜单页之下、剪切板页之内的点只在剪切板页命中。
        let low_y = blur + bar_h + PANEL_GAP + menu_h + 1.0;
        assert!(window_to_panel(blur, bar_h, 320.0, menu_h, blur + 10.0, low_y).is_none());
        assert!(window_to_panel(
            blur,
            bar_h,
            320.0,
            panel_height(PanelPage::Clipboard),
            blur + 10.0,
            low_y,
        )
        .is_some());
        // 面板宽度无效时不命中。
        assert!(window_to_panel(blur, bar_h, 0.0, menu_h, 100.0, 100.0).is_none());
    }

    #[test]
    fn page_from_id_covers_all_menu_defs() {
        // 会开子页的卡片：id 必须能解析成页面。
        for id in [
            "clipboard",
            "quick_send",
            "emoji",
            "symbol",
            "voice_input",
        ] {
            assert!(PanelPage::from_id(id).is_some(), "id {id}");
        }
        // 「设置」是动作入口，不是页面：点了直接执行（见 MenuAction），
        // 绝不能解析成页面——否则会开出一个空子页。
        assert!(PanelPage::from_id("settings").is_none());
        assert!(PanelPage::from_id("unknown").is_none());
    }

    #[test]
    fn menu_cards_expose_action_ids() {
        // 菜单 6 张卡片的 id 顺序与 MENU_DEFS 一致（点击逻辑按 id 分派）。
        let w = PANEL_MIN_WIDTH;
        let ids: Vec<Option<&str>> = (0..6).map(|i| menu_item_id(i, w)).collect();
        assert_eq!(
            ids,
            vec![
                Some("clipboard"),
                Some("quick_send"),
                Some("emoji"),
                Some("symbol"),
                Some("voice_input"),
                Some("settings"),
            ]
        );
        assert_eq!(menu_item_id(6, w), None);
        // 卡片文案与 id 同源（点击无页面无动作的卡片时，反馈文案取自这里）。
        let labels: Vec<Option<&str>> = (0..6).map(|i| menu_item_label(i, w)).collect();
        assert_eq!(
            labels,
            vec![
                Some("剪切板"),
                Some("快捷发送"),
                Some("表情"),
                Some("符号"),
                Some("语音输入"),
                Some("设置"),
            ]
        );
        assert_eq!(menu_item_label(6, w), None);
    }

    /// 构造 n 条历史的面板数据（最新在前，无编码）。
    fn list_with(source: PanelPage, count: usize, page: usize) -> PanelList {
        PanelList {
            source,
            items: (0..count)
                .map(|i| PanelListItem {
                    text: format!("条目{i}"),
                    code: String::new(),
                })
                .collect(),
            page,
        }
    }

    /// 构造快捷发送数据（文本 + 触发编码）。
    fn quick_send_with(items: &[(&str, &str)]) -> PanelList {
        PanelList {
            source: PanelPage::QuickSend,
            items: items
                .iter()
                .map(|(text, code)| PanelListItem {
                    text: (*text).to_string(),
                    code: (*code).to_string(),
                })
                .collect(),
            page: 0,
        }
    }

    /// 矩形中心点。
    fn center(rect: (f32, f32, f32, f32)) -> (f32, f32) {
        ((rect.0 + rect.2) / 2.0, (rect.1 + rect.3) / 2.0)
    }

    #[test]
    fn list_page_height_fits_rows_and_footer() {
        // 菜单页/其它未接入子页高度 = 菜单页自己的版面（去掉计算器后 3 行卡片）。
        assert!((panel_height(PanelPage::Menu) - PANEL_HEIGHT).abs() < 1e-4);
        // 网格页（表情 / 符号）的高度另算，见 grid_pages_height_fits_grid_footer_and_tabs。
        assert!((panel_height(PanelPage::VoiceInput) - PANEL_HEIGHT).abs() < 1e-4);
        // 「剪切板」与「快捷发送」共用列表版式：同高。
        let height = panel_height(PanelPage::Clipboard);
        assert!((height - panel_height(PanelPage::QuickSend)).abs() < 1e-4);
        // 首行在标题栏之下；最后一行不与翻页条重叠。
        assert!(list_row_y(0) >= PANEL_HEADER_HEIGHT);
        let last_row_bottom = list_row_y(LIST_ROWS_PER_PAGE - 1) + PANEL_ITEM_HEIGHT;
        assert!(
            last_row_bottom <= list_footer_y() - PANEL_ROW_GAP + 1e-4,
            "最后一行 {last_row_bottom} 与翻页条 {} 重叠",
            list_footer_y()
        );
        // 翻页条 + 底边距恰好占满面板高度。
        let tail = list_footer_y() + LIST_FOOTER_HEIGHT + PANEL_BOTTOM_MARGIN;
        assert!((height - tail).abs() < 1e-4, "面板高度 {height} != {tail}");
        // 翻页条三段不重叠：共 N 条 ｜ 第 x/y 页 ｜ 上一页 下一页。
        let w = PANEL_MIN_WIDTH;
        let count = list_count_rect(w, 3);
        let label = list_page_label_rect(w);
        let prev = list_page_button_rect(w, 1);
        let next = list_page_button_rect(w, 0);
        assert!(count.2 <= label.0, "「共 N 条」与页码重叠");
        assert!(label.2 <= prev.0, "页码与「上一页」重叠");
        assert!(prev.2 <= next.0, "「上一页」与「下一页」重叠");
        assert!(next.2 <= w - PANEL_H_INSET + 1e-4, "「下一页」越界面板右边距");
        assert!(count.0 >= PANEL_H_INSET - 1e-4, "「共 N 条」越界左边距");
        assert!(prev.2 - prev.0 <= LIST_PAGE_BUTTON_WIDTH + 1e-4);
    }

#[test]
    fn grid_pages_height_fits_grid_footer_and_tabs() {
        // 网格页（表情 / 符号）自上而下：标题栏 → 网格 →（翻页条）→ 底部分类标签栏，
        // 不走列表页版式。
        for (page, kind) in [
            (PanelPage::Emoji, GlyphKind::Emoji),
            (PanelPage::Symbol, GlyphKind::Symbol),
        ] {
            assert!(!page.is_list_page());
            assert!(page.is_grid_page());
            assert_eq!(page.grid_kind(), Some(kind));
            // 网格紧接标题栏；标签栏在网格（或翻页条）之下，贴面板底边。
            assert!(grid_top() >= PANEL_HEADER_HEIGHT);
            let tab_top = grid_tab_top(kind);
            if kind.needs_paging() {
                // 翻页条在网格与标签栏之间，三段依次向下不重叠。
                assert!(grid_footer_y(kind) >= grid_top() + GRID_HEIGHT);
                assert!(tab_top >= grid_footer_y(kind) + LIST_FOOTER_HEIGHT);
            } else {
                assert!(tab_top >= grid_top() + GRID_HEIGHT);
            }
            // 标签栏 + 底边距恰好占满面板高度。
            let tail = tab_top + grid_tab_height(kind) + PANEL_BOTTOM_MARGIN;
            let height = panel_height(page);
            assert!(
                (height - tail).abs() < 1e-4,
                "{page:?}: 面板高度 {height} != {tail}"
            );
        }
        // 符号页标签要排两行、还多一条翻页条，所以比表情页高；表情页比列表页矮。
        assert!(panel_height(PanelPage::Symbol) > panel_height(PanelPage::Emoji));
        assert!(panel_height(PanelPage::Emoji) < panel_height(PanelPage::Clipboard));
    }

    #[test]
    fn grid_tabs_fill_one_row_or_use_fixed_columns() {
        // 表情页 1 + 6 = 7 个标签铺满一行（等宽均分可用宽度）；
        // 符号页 1 + 17 = 18 个标签排两行，每行固定 glyph::TABS_PER_ROW 个。
        let w = PANEL_MIN_WIDTH;
        assert_eq!(GlyphKind::Emoji.tab_rows(), 1);
        assert_eq!(GlyphKind::Symbol.tab_rows(), 2);
        let single = (w - 2.0 * PANEL_H_INSET) / GlyphKind::Emoji.tab_count() as f32;
        for i in 0..GlyphKind::Emoji.tab_count() {
            let rect = grid_tab_rect(GlyphKind::Emoji, w, i);
            assert!((rect.2 - rect.0 - single).abs() < 1e-4, "标签 {i} 宽度");
            assert!(rect.0 >= PANEL_H_INSET - 1e-4 && rect.2 <= w - PANEL_H_INSET + 1e-4);
        }
        let multi = (w - 2.0 * PANEL_H_INSET) / glyph::TABS_PER_ROW as f32;
        let tab_bottom = grid_tab_top(GlyphKind::Symbol) + grid_tab_height(GlyphKind::Symbol);
        for i in 0..GlyphKind::Symbol.tab_count() {
            let rect = grid_tab_rect(GlyphKind::Symbol, w, i);
            assert!((rect.2 - rect.0 - multi).abs() < 1e-4, "标签 {i} 宽度");
            assert!(rect.0 >= PANEL_H_INSET - 1e-4 && rect.2 <= w - PANEL_H_INSET + 1e-4);
            // 两行标签都落在标签栏高度内（第一行在上），不越出面板。
            assert!(rect.3 <= tab_bottom + 1e-4, "标签 {i} 越过标签栏");
        }
        // 标签栏贴底：最后一行之下只剩底边距；表情页（无翻页条）标签栏更靠近网格。
        let emoji_tab = grid_tab_rect(GlyphKind::Emoji, w, 0);
        assert!(
            (emoji_tab.3 + PANEL_BOTTOM_MARGIN - panel_height(PanelPage::Emoji)).abs() < 1e-4,
            "表情页标签栏没有贴到面板底边"
        );
        assert!(
            grid_tab_top(GlyphKind::Symbol) > grid_tab_top(GlyphKind::Emoji),
            "符号页多一条翻页条，标签栏该更低"
        );
    }

    #[test]
    fn grid_fills_the_panel_width_without_side_padding() {
        // 每页每格都必须落在网格区里：一行 8 个、固定 4 行。面板宽度从最窄（320）
        // 到很宽都要成立——列宽铺满可用宽度，两边只留 PANEL_H_INSET
        // （面板宽度跟着候选栏走，宽候选栏下也不能在网格两边空出一大块）。
        for width in [PANEL_MIN_WIDTH, 520.0, 900.0] {
            let grid_bottom = grid_top() + GRID_HEIGHT;
            for slot in 0..glyph::PER_PAGE {
                assert!(slot / glyph::PER_ROW < glyph::ROWS);
                let rect = grid_cell_rect(width, slot);
                assert!(rect.3 <= grid_bottom + 1e-4, "格子越出网格区：{rect:?}");
                assert!(rect.0 >= 0.0 && rect.2 <= width + 1e-4, "格子越界：{rect:?}");
            }
            // 第 0 格左边 = 第 7 格右边 = 面板左右边距（网格铺满，没有居中留白）。
            let first = grid_cell_rect(width, 0);
            let last = grid_cell_rect(width, glyph::PER_ROW - 1);
            assert!((first.0 - PANEL_H_INSET).abs() < 1e-4, "左边距：{first:?}");
            assert!(
                (last.2 - (width - PANEL_H_INSET)).abs() < 1e-4,
                "右边距：{last:?}"
            );
            let cell_width = grid_cell_width(width);
            assert!(cell_width >= GRID_CELL_WIDTH_MIN - 1e-4);
        }
    }

    #[test]
    fn grid_hit_routes_tabs_cells_paging_and_back() {
        let w = PANEL_MIN_WIDTH;
        let list = PanelList::default();
        // 数据与页面对不上时（进页即 reload，这里是兜底）：标签仍可点，格子不响应。
        let stale = PanelGrid::default();
        let (x, y) = center(grid_tab_rect(GlyphKind::Emoji, w, 1));
        assert_eq!(
            panel_hit(PanelPage::Emoji, w, &list, &stale, x, y),
            Some(PanelHit::GlyphTab(1))
        );
        let (x, y) = center(grid_cell_rect(w, 0));
        assert_eq!(panel_hit(PanelPage::Emoji, w, &list, &stale, x, y), None);

        // 表情页 + 空「最近使用」：标签与返回按钮可用，网格里没有可点的格子。
        let kind = GlyphKind::Emoji;
        let empty = grid_state(PanelPage::Emoji, &[]);
        assert_eq!(empty.tab_count(), kind.tab_count());
        assert_eq!(empty.page_count(), 1);
        assert_eq!(empty.items_on_page(), 0);
        assert!(empty.is_recent_tab());
        for i in [0, 1, kind.tab_count() - 1] {
            let (x, y) = center(grid_tab_rect(kind, w, i));
            assert_eq!(
                panel_hit(PanelPage::Emoji, w, &list, &empty, x, y),
                Some(PanelHit::GlyphTab(i))
            );
        }
        let (x, y) = center(grid_cell_rect(w, 0));
        assert_eq!(
            panel_hit(PanelPage::Emoji, w, &list, &empty, x, y),
            None,
            "空标签不该有可点格子"
        );
        let (back_x, back_y) = center(panel_back_rect(w));
        assert_eq!(
            panel_hit(PanelPage::Emoji, w, &list, &empty, back_x, back_y),
            Some(PanelHit::Back)
        );

        // 「最近使用」有 3 条：前 3 格可点，第 4 格（空槽）不命中。
        let recent = grid_state(PanelPage::Emoji, &["😀", "😂", "🥰"]);
        assert_eq!(recent.item_count(), 3);
        for slot in 0..3 {
            let (x, y) = center(grid_cell_rect(w, slot));
            assert_eq!(
                panel_hit(PanelPage::Emoji, w, &list, &recent, x, y),
                Some(PanelHit::GlyphCell(slot))
            );
        }
        let (x, y) = center(grid_cell_rect(w, 3));
        assert_eq!(panel_hit(PanelPage::Emoji, w, &list, &recent, x, y), None);
        // 表情页没有翻页条（needs_paging == false）：网格与底部标签栏之间的那条空隙
        // 也不该命中任何元素（别把翻页按钮的几何留在那儿）。
        let gap_y = grid_top() + GRID_HEIGHT + PANEL_CONTENT_GAP / 2.0;
        assert_eq!(
            panel_hit(PanelPage::Emoji, w, &list, &recent, w / 2.0, gap_y),
            None
        );

        // 符号页第 1 个分类（83 个）有 3 页：翻页按钮只在能翻的方向命中。
        let kind = GlyphKind::Symbol;
        let mut grid = grid_state(PanelPage::Symbol, &[]);
        assert!(grid.select_tab(1));
        assert!(!grid.is_recent_tab());
        assert_eq!(grid.page_count(), 3);
        assert_eq!(grid.items_on_page(), glyph::PER_PAGE);
        let (x, y) = center(grid_cell_rect(w, glyph::PER_PAGE - 1));
        assert_eq!(
            panel_hit(PanelPage::Symbol, w, &list, &grid, x, y),
            Some(PanelHit::GlyphCell(glyph::PER_PAGE - 1))
        );
        let footer_y = grid_footer_y(kind);
        let (prev_x, prev_y) = center(footer_page_button_rect(w, footer_y, 1));
        let (next_x, next_y) = center(footer_page_button_rect(w, footer_y, 0));
        assert_eq!(
            panel_hit(PanelPage::Symbol, w, &list, &grid, prev_x, prev_y),
            None
        );
        assert_eq!(
            panel_hit(PanelPage::Symbol, w, &list, &grid, next_x, next_y),
            Some(PanelHit::NextPage)
        );
        assert!(grid.next_page());
        assert_eq!(grid.clamped_page(), 1);
        assert_eq!(
            panel_hit(PanelPage::Symbol, w, &list, &grid, prev_x, prev_y),
            Some(PanelHit::PrevPage)
        );
        while grid.next_page() {}
        assert!(!grid.has_next_page());
        assert_eq!(
            panel_hit(PanelPage::Symbol, w, &list, &grid, next_x, next_y),
            None
        );
        // 切标签页码回到第一页；切回「最近使用」后仍是一页。
        assert!(grid.select_tab(0));
        assert_eq!(grid.clamped_page(), 0);
        assert_eq!(grid.page_count(), 1);
    }

    #[test]
    fn list_hit_routes_rows_and_paging() {
        let w = PANEL_MIN_WIDTH;
        // 两个列表子页共用同一套命中几何：逐页验证。
        for page in [PanelPage::Clipboard, PanelPage::QuickSend] {
            let single_page = list_with(page, LIST_ROWS_PER_PAGE + 2, 0);
            let (row_x, row_y) = center(list_row_rect(w, 1));
            assert_eq!(
                hit(page, w, &single_page, row_x, row_y),
                Some(PanelHit::ListItem(1)),
                "{page:?}"
            );

            let (prev_x, prev_y) = center(list_page_button_rect(w, 1));
            let (next_x, next_y) = center(list_page_button_rect(w, 0));
            // 首页：上一页不可命中，下一页可命中。
            assert_eq!(hit(page, w, &single_page, prev_x, prev_y), None);
            assert_eq!(
                hit(page, w, &single_page, next_x, next_y),
                Some(PanelHit::NextPage)
            );
            // 末页：下一页不可命中，上一页可命中；该页只有 2 条，空行不命中。
            let last_page = list_with(page, LIST_ROWS_PER_PAGE + 2, 1);
            assert_eq!(hit(page, w, &last_page, next_x, next_y), None);
            assert_eq!(
                hit(page, w, &last_page, prev_x, prev_y),
                Some(PanelHit::PrevPage)
            );
            let (empty_x, empty_y) = center(list_row_rect(w, 2));
            assert_eq!(
                hit(page, w, &last_page, empty_x, empty_y),
                None,
                "末页空行不得命中条目"
            );
            // 只有一页时翻页按钮不显示也不可命中。
            let short = list_with(page, 2, 0);
            assert_eq!(hit(page, w, &short, next_x, next_y), None);
            // 返回按钮仍然可命中。
            let (back_x, back_y) = center(panel_back_rect(w));
            assert_eq!(
                hit(page, w, &short, back_x, back_y),
                Some(PanelHit::Back)
            );
        }
    }

    #[test]
    fn list_source_guard_rejects_cross_page_data() {
        // 数据来源与页面不一致（进页忘记 reload）时不能命中任何条目/翻页按钮，
        // 只剩返回按钮——避免把上一个子页的条目画出来还点得动。
        let w = PANEL_MIN_WIDTH;
        let clipboard_data = list_with(PanelPage::Clipboard, LIST_ROWS_PER_PAGE, 0);
        let (row_x, row_y) = center(list_row_rect(w, 0));
        assert_eq!(
            hit(PanelPage::QuickSend, w, &clipboard_data, row_x, row_y),
            None
        );
        // 同一份数据在自己的页面里正常可命中。
        assert_eq!(
            hit(PanelPage::Clipboard, w, &clipboard_data, row_x, row_y),
            Some(PanelHit::ListItem(0))
        );
    }

    #[test]
    fn quick_send_code_column_keeps_text_clear() {
        let quick = quick_send_with(&[("您好，请问在吗", "dh"), ("收到", "")]);
        assert!(quick.has_codes(), "有条目带编码时应留出编码列");
        // 无编码的列表（剪切板历史）不留编码列。
        assert!(!list_with(PanelPage::Clipboard, 2, 0).has_codes());

        let w = PANEL_MIN_WIDTH;
        let rect = list_row_rect(w, 0);
        let code = list_code_rect(rect, true);
        let text_left = list_text_left(rect, true);
        // 编码列在行内、正文起点不侵入编码列，也不越过行右边距。
        assert!(code.0 >= rect.0 && code.2 <= rect.2);
        assert!(text_left >= code.2, "正文起点 {text_left} 侵入编码列 {code:?}");
        assert!(text_left < rect.2, "正文起点越界");
        // 无编码列时正文从左边距开始。
        let no_code = list_code_rect(rect, false);
        assert!((no_code.2 - no_code.0).abs() < 1e-4, "无编码时编码列应为空");
        assert!((list_text_left(rect, false) - (rect.0 + 8.0)).abs() < 1e-4);
    }

    #[test]
    fn list_paging_clamps_and_reports() {
        let mut data = list_with(PanelPage::Clipboard, LIST_ROWS_PER_PAGE * 2 + 1, 0);
        assert_eq!(data.page_count(), 3);
        assert!(!data.has_prev_page());
        assert!(data.has_next_page());
        assert_eq!(data.rows_on_page(), LIST_ROWS_PER_PAGE);
        assert!(data.next_page());
        assert_eq!(data.clamped_page(), 1);
        assert!(data.has_prev_page() && data.has_next_page());
        assert!(data.next_page());
        assert_eq!(data.clamped_page(), 2);
        assert_eq!(data.rows_on_page(), 1);
        assert!(!data.next_page(), "末页再翻页应无效");
        assert_eq!(data.item_at(0).map(|i| i.text.as_str()), Some("条目12"));
        assert_eq!(data.item_at(1), None, "末页只有一条");

        // 列表缩短后页码夹回，不越界取条目。
        data.items.truncate(LIST_ROWS_PER_PAGE + 1);
        assert_eq!(data.clamped_page(), 1);
        assert_eq!(data.rows_on_page(), 1);

        // 空列表：1 页 0 行、无翻页。
        let empty = PanelList::default();
        assert_eq!(empty.page_count(), 1);
        assert_eq!(empty.rows_on_page(), 0);
        assert!(!empty.has_prev_page() && !empty.has_next_page());
        assert_eq!(empty.item_at(0), None);
        assert!(!empty.has_codes());
    }

    #[test]
    fn list_display_text_flattens_and_truncates() {
        // 控制字符（换行/制表符）折成空格，面板单行显示。
        assert_eq!(list_display_text("第一行\r\n第二行"), "第一行  第二行");
        assert_eq!(list_display_text("a\tb"), "a b");
        // 超长截断并加省略号（上屏/复制的仍是全文）。
        let long = "字".repeat(LIST_DISPLAY_MAX_CHARS * 3);
        let shown = list_display_text(&long);
        assert_eq!(shown.chars().count(), LIST_DISPLAY_MAX_CHARS + 1);
        assert!(shown.ends_with('…'));
        // 恰好等于上限时不截断。
        let exact = "字".repeat(LIST_DISPLAY_MAX_CHARS);
        assert_eq!(list_display_text(&exact), exact);
        // 触发编码走同一个折行/截断规则显示。
        assert_eq!(list_display_text("dh"), "dh");
    }

    #[test]
    fn voice_page_layout_fits_between_header_and_bottom() {
        let w = PANEL_MIN_WIDTH;
        let row = voice_row_rect(w);
        let button = voice_button_rect(w);
        let text = voice_text_rect(w);
        let meter = voice_meter_rect(w);
        let footer = voice_footer_rect(w);
        // 状态行在标题栏之下、左右留白之内。
        assert!(row.1 >= PANEL_HEADER_HEIGHT);
        assert!(row.0 >= 0.0 && row.2 <= w);
        assert!((row.3 - row.1 - VOICE_ROW_HEIGHT).abs() < 1e-4);
        // 主按钮在状态行里、贴右侧，高度是常量且比行矮（胶囊不撑满行）。
        assert!(button.0 >= row.0 && button.2 <= row.2 + 1e-4);
        assert!((button.2 - row.2).abs() < 1e-4, "主按钮应贴右");
        assert!((button.3 - button.1 - VOICE_BUTTON_HEIGHT).abs() < 1e-4);
        assert!(button.1 >= row.1 && button.3 <= row.3);
        // 状态文字在图标右侧、按钮左侧（不与按钮重叠）。
        let label = voice_label_rect(w);
        assert!(label.0 >= row.0 + VOICE_ICON_BOX || label.0 <= row.2);
        assert!(label.2 <= button.0 + 1e-4);
        // 电平条紧贴状态行下方，满内容宽，且不压到文本区。
        assert!((meter.1 - row.3).abs() < 1e-4);
        assert!((meter.3 - meter.1 - VOICE_METER_HEIGHT).abs() < 1e-4);
        assert!((meter.0 - row.0).abs() < 1e-4 && (meter.2 - row.2).abs() < 1e-4);
        // 文本区在电平条之下、提示行之上；提示行贴面板底边距。
        assert!(text.1 >= meter.3);
        assert!(text.3 <= footer.1 + 1e-4);
        assert!(text.2 <= w && text.0 >= 0.0);
        assert!((footer.3 - (PANEL_HEIGHT - PANEL_BOTTOM_MARGIN)).abs() < 1e-4);
        assert!((footer.3 - footer.1 - VOICE_FOOTER_HEIGHT).abs() < 1e-4);
        // 语音页与菜单页一样高（无翻页条 / 标签栏要撑高）。
        assert!((panel_height(PanelPage::VoiceInput) - PANEL_HEIGHT).abs() < 1e-4);
    }

    #[test]
    fn voice_page_hit_routes_whole_status_row_and_back() {
        let w = PANEL_MIN_WIDTH;
        let empty = PanelList::default();
        // 主按钮中心可点。
        let (bx, by) = center(voice_button_rect(w));
        assert_eq!(
            hit(PanelPage::VoiceInput, w, &empty, bx, by),
            Some(PanelHit::VoiceToggle)
        );
        // 状态行左侧（图标/状态文字那一段）也可点——整行都是目标，不用瞄准按钮。
        let row = voice_row_rect(w);
        assert_eq!(
            hit(PanelPage::VoiceInput, w, &empty, row.0 + 4.0, row.1 + 4.0),
            Some(PanelHit::VoiceToggle)
        );
        // 返回按钮优先于状态行（都在面板内，位置不同）。
        let (back_x, back_y) = center(panel_back_rect(w));
        assert_eq!(
            hit(PanelPage::VoiceInput, w, &empty, back_x, back_y),
            Some(PanelHit::Back)
        );
        // 文本区与底部提示行不可点（点它不该开始识别）。
        let (tx, ty) = center(voice_text_rect(w));
        assert_eq!(hit(PanelPage::VoiceInput, w, &empty, tx, ty), None);
        let (fx, fy) = center(voice_footer_rect(w));
        assert_eq!(hit(PanelPage::VoiceInput, w, &empty, fx, fy), None);
    }

    /// 语音页文案：状态 → 状态色 / 状态文字 / 主按钮 / 底部提示。
    #[test]
    fn voice_labels_follow_state_and_model() {
        // 模型没下载：按钮直接说明白、且不可点，底部给下载路径。
        let missing = VoiceView::default();
        assert_eq!(missing.tone(), VoiceTone::Blocked);
        assert_eq!(missing.state_label(), "模型未下载");
        assert_eq!(missing.button_label(), "未下载模型");
        assert!(!missing.button_enabled(), "没模型时按钮不该可点");
        assert_eq!(missing.footer_label(), "设置 → 语音转文本：下载模型");
        assert_eq!(
            missing.text_view().0,
            "还没有语音模型：先在设置里下载一个"
        );

        // 就绪待命：能点、给引导语。
        let idle = VoiceView {
            model_ready: true,
            ..VoiceView::default()
        };
        assert_eq!(idle.tone(), VoiceTone::Ready);
        assert_eq!(idle.state_label(), "麦克风就绪");
        assert_eq!(idle.button_label(), "开始说话");
        assert!(idle.button_enabled());
        assert_eq!(idle.footer_label(), "语音只在本机识别，不出电脑");
        assert_eq!(
            idle.text_view(),
            (
                "点「开始说话」，说完停顿就会自动上屏".to_string(),
                VoiceTextKind::Placeholder
            )
        );

        // 装载中：按钮变灰且不可点（点了只会白等）。
        let loading = VoiceView {
            model_ready: true,
            state: SpeechState::Loading,
            ..VoiceView::default()
        };
        assert_eq!(loading.tone(), VoiceTone::Loading);
        assert_eq!(loading.state_label(), "正在准备");
        assert_eq!(loading.button_label(), "准备中…");
        assert!(!loading.button_enabled());

        // 聆听中：能点（结束并上屏）、提示 Esc、空文本给引导语。
        let listening = VoiceView {
            model_ready: true,
            state: SpeechState::Listening,
            ..VoiceView::default()
        };
        assert_eq!(listening.tone(), VoiceTone::Listening);
        assert_eq!(listening.state_label(), "正在听…");
        assert_eq!(listening.button_label(), "结束并上屏");
        assert!(listening.button_enabled());
        assert_eq!(listening.footer_label(), "说完停顿自动上屏 · Esc 取消");
        assert_eq!(
            listening.text_view(),
            (
                "请对着麦克风说话…".to_string(),
                VoiceTextKind::Placeholder
            )
        );
        // 有 partial 就显示 partial（实时文本是最亮的一档）。
        let partial = VoiceView {
            partial: "你好世界".to_string(),
            ..listening.clone()
        };
        assert_eq!(
            partial.text_view(),
            ("你好世界".to_string(), VoiceTextKind::Live)
        );

        // 错误优先于其它状态（模型缺失 / 麦克风打不开都走这里）。
        let failed = VoiceView {
            error: Some("麦克风打不开".to_string()),
            ..listening
        };
        assert_eq!(failed.tone(), VoiceTone::Error);
        assert_eq!(failed.state_label(), "出错了");
        assert_eq!(
            failed.text_view(),
            ("麦克风打不开".to_string(), VoiceTextKind::Alert)
        );
        // 出错时提示行要把"去哪修"说清楚，而不是重复错误本身（错误已在文本区）。
        assert!(failed.footer_label().contains("语音转文本"));

        // 上屏过：待命时显示最近一次上屏内容（回执态）。
        let committed = VoiceView {
            model_ready: true,
            last_commit: Some("已经上屏了".to_string()),
            ..VoiceView::default()
        };
        assert_eq!(
            committed.text_view(),
            ("已上屏：已经上屏了".to_string(), VoiceTextKind::Committed)
        );
    }

    /// 电平条只在聆听中画，且电平值原样来自快照（UI 不做平滑）。
    #[test]
    fn voice_level_is_carried_from_snapshot_and_only_drawn_while_listening() {
        let mut view = VoiceView::default();
        assert!(!view.listening());
        view.state = SpeechState::Listening;
        view.level = 37;
        assert!(view.listening());
        assert_eq!(view.level, 37);
        // 电平参与「有没有变化」的比较：变了就得重绘（否则电平条是死的）。
        let same = VoiceView {
            level: 37,
            ..view.clone()
        };
        assert_eq!(view, same);
        let louder = VoiceView {
            level: 60,
            ..view.clone()
        };
        assert_ne!(view, louder);
    }
}
