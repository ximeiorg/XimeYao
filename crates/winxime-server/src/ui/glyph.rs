//! 网格页（表情 / 符号）共用的版式与数据解析。
//!
//! 两页形态完全一样：**分类标签栏 + 8 列网格 + 底部翻页条**，只有三处不同——
//! 数据来源（`emoji` / `symbol` 两张内置表）、字形字体（表情必须用 Segoe UI Emoji
//! 才有彩色字形，符号用候选字体）、标签行数（表情 7 个标签一行够，符号 18 个要两行）。
//! 所以几何、命中、绘制都共用，只在数据解析处分叉。这也是本项目
//! 「画得出来必须点得到」的延续：绘制与命中调的是同一个 [`item_at`]。
//!
//! 「最近使用」（第 0 个标签）不在两张静态表里：它是运行期的 LRU 记录
//! （见 `recent_usage`），由窗口持有、进入页面时读一次，绘制/命中时按引用传进来。
//!
//! 数据与语义对齐安卓版（`EmojiData.kt` / `SymbolData.kt` / `RecentUsageStore.kt`）：
//! 最近使用是**第一个**分类页；单类条目超过一页时在分类内翻页
//! （安卓那边是可滚动的键盘，桌面上候选栏不能滚动，改用翻页条，与剪切板子页一致）。

use super::emoji;
use super::symbol;
use crate::recent_usage::RecentKind;

/// 一个内置分类：标签 + 该分类的字形（表情或符号）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GlyphCategory {
    /// 标签栏上的文案（符号页照安卓用单个代表字符，如「中」「数」「⇌」）。
    pub(crate) label: &'static str,
    /// 该分类的字形（顺序即展示顺序）。
    pub(crate) glyphs: &'static [&'static str],
}

/// 网格每行 8 格（安卓 `EmojiData.layoutColumns = 8`）。
pub(crate) const PER_ROW: usize = 8;
/// 网格固定 4 行：一页 32 格。
pub(crate) const ROWS: usize = 4;
/// 一页容量（也是「最近使用」上限：正好一页，见 `recent_usage::MAX_COUNT`）。
pub(crate) const PER_PAGE: usize = PER_ROW * ROWS;
/// 标签栏一行放几个标签（符号页 18 个标签 → 两行 9 + 9）。
pub(crate) const TABS_PER_ROW: usize = 9;
/// 「最近使用」标签的文案（安卓里叫「最近使用」，桌面标签位窄，取前两字）。
pub(crate) const RECENT_LABEL: &str = "最近";
/// 「最近使用」为空时的提示（照安卓那句）。
pub(crate) const RECENT_EMPTY_TEXT: &str = "暂无最近使用";
/// 内置分类为空时的兜底提示（内置表有单测保证非空，这里只是绘制兜底）。
pub(crate) const EMPTY_TEXT: &str = "暂无内容";

/// 网格页的数据来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlyphKind {
    Emoji,
    Symbol,
}

impl GlyphKind {
    /// 内置分类（不含「最近使用」）。
    pub(crate) fn categories(self) -> &'static [GlyphCategory] {
        match self {
            Self::Emoji => emoji::CATEGORIES,
            Self::Symbol => symbol::CATEGORIES,
        }
    }

    /// 标签总数 = 1（最近使用）+ 内置分类数。
    pub(crate) fn tab_count(self) -> usize {
        1 + self.categories().len()
    }

    /// 标签栏需要几行。
    pub(crate) fn tab_rows(self) -> usize {
        self.tab_count().div_ceil(TABS_PER_ROW)
    }

    /// 该页是否需要「上一页 / 下一页」翻页条：只要有一个内置分类超过一页就要。
    ///
    /// 「最近使用」不用管——它的上限 [`crate::recent_usage::MAX_COUNT`] 正好是一页容量；
    /// 表情每类也恰好 32 个（一页）。所以只有符号页（「中」类 83 个 → 3 页）会有翻页条，
    /// 表情页不占这一行：底部留给标签栏。
    pub(crate) fn needs_paging(self) -> bool {
        self.categories()
            .iter()
            .any(|category| category.glyphs.len() > PER_PAGE)
    }

    /// 第 `tab` 个标签的文案（0 = 最近使用）。
    pub(crate) fn tab_label(self, tab: usize) -> Option<&'static str> {
        match tab {
            0 => Some(RECENT_LABEL),
            _ => self
                .categories()
                .get(tab - 1)
                .map(|category| category.label),
        }
    }

    /// 该页的字形是否要用系统 emoji 字体（中文字体里没有表情字形）。
    pub(crate) fn uses_emoji_font(self) -> bool {
        matches!(self, Self::Emoji)
    }

    /// 该页的「最近使用」记录种类。
    pub(crate) fn recent_kind(self) -> RecentKind {
        match self {
            Self::Emoji => RecentKind::Emoji,
            Self::Symbol => RecentKind::Symbol,
        }
    }
}

/// 第 `tab` 个标签的条目总数（`tab == 0` 时由运行期的 `recent` 提供）。
pub(crate) fn item_count(kind: GlyphKind, tab: usize, recent: &[String]) -> usize {
    match tab {
        0 => recent.len(),
        _ => kind
            .categories()
            .get(tab - 1)
            .map_or(0, |category| category.glyphs.len()),
    }
}

/// 第 `tab` 个标签的页数（空分类也画一页空态：与列表子页口径一致）。
pub(crate) fn page_count(kind: GlyphKind, tab: usize, recent: &[String]) -> usize {
    item_count(kind, tab, recent).div_ceil(PER_PAGE).max(1)
}

/// 第 `tab` 个标签第 `page` 页上有多少格是有内容的。
pub(crate) fn items_on_page(
    kind: GlyphKind,
    tab: usize,
    page: usize,
    recent: &[String],
) -> usize {
    item_count(kind, tab, recent)
        .saturating_sub(page.saturating_mul(PER_PAGE))
        .min(PER_PAGE)
}

/// 第 `tab` 个标签第 `page` 页第 `slot` 格的字形。
/// 越界（标签/页/格）返回 `None`——绘制与命中共用，空槽既不画也点不到。
///
/// `recent` 里取出的是借来的 `&str`；内置表是 `'static`，会自然收窄到同一生命周期。
pub(crate) fn item_at<'a>(
    kind: GlyphKind,
    tab: usize,
    page: usize,
    slot: usize,
    recent: &'a [String],
) -> Option<&'a str> {
    if slot >= PER_PAGE {
        return None;
    }
    let index = page.checked_mul(PER_PAGE)?.checked_add(slot)?;
    match tab {
        0 => recent.get(index).map(String::as_str),
        _ => kind
            .categories()
            .get(tab - 1)?
            .glyphs
            .get(index)
            .copied(),
    }
}

/// 夹回范围内的标签下标（下标来自 UI 状态，容错优先）。
pub(crate) fn clamped_tab(kind: GlyphKind, tab: usize) -> usize {
    tab.min(kind.tab_count().saturating_sub(1))
}

/// 夹回范围内的页码（切标签后页数可能变少）。
pub(crate) fn clamped_page(kind: GlyphKind, tab: usize, page: usize, recent: &[String]) -> usize {
    page.min(page_count(kind, tab, recent).saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recent(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn recent_is_the_first_tab_and_builtin_categories_follow() {
        for kind in [GlyphKind::Emoji, GlyphKind::Symbol] {
            assert_eq!(kind.tab_count(), 1 + kind.categories().len());
            assert_eq!(kind.tab_label(0), Some(RECENT_LABEL));
            for (i, category) in kind.categories().iter().enumerate() {
                assert_eq!(kind.tab_label(i + 1), Some(category.label), "{kind:?} #{i}");
            }
            assert_eq!(kind.tab_label(kind.tab_count()), None);
        }
    }

    #[test]
    fn tab_labels_are_unique_and_short() {
        for kind in [GlyphKind::Emoji, GlyphKind::Symbol] {
            let mut labels: Vec<&str> = (0..kind.tab_count())
                .filter_map(|tab| kind.tab_label(tab))
                .collect();
            let count = labels.len();
            labels.sort_unstable();
            labels.dedup();
            assert_eq!(labels.len(), count, "{kind:?} 标签重名");
            for label in labels {
                // 标签位窄：最多两个字符（符号页照安卓用单个代表字符）。
                assert!(
                    label.chars().count() <= 2,
                    "{kind:?} 标签过长：{label}"
                );
                assert!(!label.trim().is_empty(), "{kind:?} 空标签");
            }
        }
    }

    #[test]
    fn builtin_categories_are_non_empty_with_unique_glyphs() {
        for kind in [GlyphKind::Emoji, GlyphKind::Symbol] {
            for category in kind.categories() {
                assert!(!category.glyphs.is_empty(), "{kind:?} {}", category.label);
                let mut seen: Vec<&str> = category.glyphs.to_vec();
                let count = seen.len();
                seen.sort_unstable();
                seen.dedup();
                assert_eq!(seen.len(), count, "{kind:?} {} 分类内字形重复", category.label);
                for glyph in category.glyphs {
                    assert!(!glyph.trim().is_empty(), "{kind:?} {} 空字形", category.label);
                    assert!(
                        glyph.chars().count() <= 4,
                        "{kind:?} {} 字形过长：{glyph}",
                        category.label
                    );
                    // 字形里不该混进空白（键盘/网格都是单字位）。
                    assert!(
                        !glyph.chars().any(char::is_whitespace),
                        "{kind:?} {} 字形含空白：{glyph}",
                        category.label
                    );
                }
            }
        }
    }

    #[test]
    fn symbol_tabs_fit_two_rows_and_emoji_fits_one() {
        assert_eq!(GlyphKind::Emoji.tab_rows(), 1);
        // 18 个标签（最近 + 17 个内置分类）：两行 9 + 9，一行放不下。
        assert_eq!(GlyphKind::Symbol.tab_count(), 18);
        assert_eq!(GlyphKind::Symbol.tab_rows(), 2);
    }

    #[test]
    fn only_the_symbol_page_needs_a_paging_bar() {
        // 「最近使用」上限就是一页容量，任何页都不可能靠它翻页。
        assert!(crate::recent_usage::MAX_COUNT <= PER_PAGE);
        // 表情每类恰好 32 个（正好一页）→ 表情页没有翻页条，底部整行留给标签栏。
        assert!(!GlyphKind::Emoji.needs_paging());
        for category in GlyphKind::Emoji.categories() {
            assert!(
                category.glyphs.len() <= PER_PAGE,
                "表情「{}」有 {} 个，超过一页",
                category.label,
                category.glyphs.len()
            );
        }
        // 符号页「中」类 83 个 → 3 页，必须留翻页条。
        assert!(GlyphKind::Symbol.needs_paging());
    }

    #[test]
    fn empty_recent_is_one_empty_page() {
        for kind in [GlyphKind::Emoji, GlyphKind::Symbol] {
            let empty = recent(&[]);
            assert_eq!(item_count(kind, 0, &empty), 0);
            assert_eq!(page_count(kind, 0, &empty), 1);
            assert_eq!(items_on_page(kind, 0, 0, &empty), 0);
            assert_eq!(item_at(kind, 0, 0, 0, &empty), None);
        }
    }

    #[test]
    fn pages_cover_every_item_exactly_once() {
        for kind in [GlyphKind::Emoji, GlyphKind::Symbol] {
            // 内置分类逐个验；再拿一条「人手改过、超过一页」的最近使用记录验翻页。
            let long_recent: Vec<String> = (0..PER_PAGE * 2 + 3).map(|i| format!("g{i}")).collect();
            for tab in 0..kind.tab_count() {
                let recent = if tab == 0 { long_recent.as_slice() } else { &[] };
                let total = item_count(kind, tab, recent);
                let pages = page_count(kind, tab, recent);
                assert_eq!(pages, total.div_ceil(PER_PAGE).max(1), "{kind:?} #{tab}");
                let mut seen = 0;
                for page in 0..pages {
                    let on_page = items_on_page(kind, tab, page, recent);
                    assert!(on_page <= PER_PAGE);
                    for slot in 0..on_page {
                        assert!(item_at(kind, tab, page, slot, recent).is_some());
                    }
                    // 空槽必须取不到内容。
                    assert_eq!(item_at(kind, tab, page, on_page, recent), None);
                    seen += on_page;
                }
                assert_eq!(seen, total, "{kind:?} #{tab} 分页漏项/重项");
            }
        }
    }

    #[test]
    fn item_at_is_bounds_checked() {
        let kind = GlyphKind::Symbol;
        let items = recent(&["①", "②"]);
        assert_eq!(item_at(kind, 0, 0, 0, &items), Some("①"));
        assert_eq!(item_at(kind, 0, 0, 1, &items), Some("②"));
        assert_eq!(item_at(kind, 0, 0, 2, &items), None);
        // 超出页容量 / 越过最后一页 / 不存在的标签都是 None。
        assert_eq!(item_at(kind, 0, 0, PER_PAGE, &items), None);
        assert_eq!(item_at(kind, 0, 5, 0, &items), None);
        assert_eq!(item_at(kind, kind.tab_count(), 0, 0, &items), None);
        assert_eq!(item_at(kind, usize::MAX, 0, 0, &items), None);
        assert_eq!(item_at(kind, 1, usize::MAX, 0, &items), None);
    }

    #[test]
    fn clamped_tab_and_page_stay_inside() {
        let kind = GlyphKind::Emoji;
        assert_eq!(clamped_tab(kind, 0), 0);
        assert_eq!(clamped_tab(kind, kind.tab_count() + 9), kind.tab_count() - 1);
        let empty = recent(&[]);
        assert_eq!(clamped_page(kind, kind.tab_count() + 9, 7, &empty), 0);
        // 一个内置分类（符号「中」有 83 个 → 3 页）的页码夹回。
        let symbol_tab = 1;
        let pages = page_count(GlyphKind::Symbol, symbol_tab, &empty);
        assert!(pages > 1, "符号第一个分类应该超过一页");
        assert_eq!(
            clamped_page(GlyphKind::Symbol, symbol_tab, 99, &empty),
            pages - 1
        );
    }

    #[test]
    fn only_the_emoji_page_uses_the_emoji_font_and_its_own_recent_list() {
        assert!(GlyphKind::Emoji.uses_emoji_font());
        assert!(!GlyphKind::Symbol.uses_emoji_font());
        assert_eq!(GlyphKind::Emoji.recent_kind(), RecentKind::Emoji);
        assert_eq!(GlyphKind::Symbol.recent_kind(), RecentKind::Symbol);
    }
}