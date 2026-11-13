//! 候选栏数据模型：绘制所需的输入数据（候选/字根/布局结果）。

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows_core::HSTRING;

use crate::config::get_colors;
use winxime_ipc::Context;

/// 选字键：`labels` 为空 = 服务端没指定（rime 路径），回退 1–9 + 0；
/// 非空则逐项取首字符，空串映射成 `0`（绘制层约定 0 = 不画选字键）。
fn selkeys_from_labels(labels: &[winxime_ipc::Text]) -> Vec<u16> {
    if labels.is_empty() {
        return ('1'..='9')
            .map(|c| c as u16)
            .chain(std::iter::once('0' as u16))
            .collect();
    }
    labels
        .iter()
        .map(|label| label.str.chars().next().map(|c| c as u16).unwrap_or(0))
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) struct RenderedMetrics {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) hw_width: f32,
    pub(crate) hw_height: f32,
    /// 候选栏自身高度（不含面板区）。
    pub(crate) bar_height: f32,
    pub(crate) item_height: f32,
    pub(crate) item_widths: Vec<f32>,
    pub(crate) selkey_widths: Vec<f32>,
    pub(crate) text_widths: Vec<f32>,
    /// "⋮"/"⋯" 菜单按钮矩形（窗口 DIP 坐标）。
    pub(crate) menu_button: Option<(f32, f32, f32, f32)>,
    /// 面板展开方向（`None` = 面板收起，见 `panel::panel_side`）。
    pub(crate) panel_side: Option<super::panel::PanelSide>,
    /// 面板内容宽度（DIP）；面板收起时无意义。
    ///
    /// 面板贴右侧时窗口宽 = 候选栏宽 + 间距 + 面板宽，命中测试与绘制都要用
    /// 「候选栏自身宽度」区分两者，此字段即候选栏宽之外的面板部分。
    pub(crate) panel_content_width: f32,
}

#[derive(Debug, Clone, Default)]
pub struct CandidateModel {
    pub items: Vec<String>,
    pub comments: Vec<String>,
    pub selkeys: Vec<u16>,
    /// Rime 的分页信息：候选栏本身不画页码/翻页按钮，翻页走键盘或鼠标滚轮。
    pub total_pages: u32,
    pub font_family: HSTRING,
    pub font_size: f32,
    pub cand_per_row: u32,
    /// 候选排列方向：横向时 ⋮ 在左侧、三个点竖排；纵向时 ⋯ 在顶部、三个点横排。
    pub horizontal: bool,
    pub use_cursor: bool,
    pub current_sel: usize,
    /// 鼠标悬停的候选下标（仅计算器候选使用；普通 rime 候选恒 None）。
    /// WM_MOUSEMOVE 直接改这里并重绘——候选内容不重建，省一次模型更新。
    pub hovered: Option<usize>,
    /// 候选/面板窗口圆角半径（`style.corner_radius`，DIP）。
    ///
    /// 之前绘制里写死 8.0 / 10.0，设置页的「圆角大小」存了也没人读。
    pub corner_radius: f32,
    pub selkey_color: D2D1_COLOR_F,
    pub fg_color: D2D1_COLOR_F,
    pub comment_color: D2D1_COLOR_F,
    pub bg_color: D2D1_COLOR_F,
    pub highlight_fg_color: D2D1_COLOR_F,
    pub highlight_bg_color: D2D1_COLOR_F,
    pub border_color: D2D1_COLOR_F,
}

#[derive(Debug, Clone, Default)]
pub struct RootModel {
    pub letter: char,
    pub root: String,
    pub font_family: HSTRING,
    pub font_size: f32,
    /// 字根浮层圆角（同一份 `style.corner_radius`）。
    pub corner_radius: f32,
    pub primary_color: D2D1_COLOR_F,
    pub bg_color: D2D1_COLOR_F,
    pub fg_color: D2D1_COLOR_F,
}

impl From<(char, String)> for RootModel {
    fn from((letter, root): (char, String)) -> Self {
        let config = crate::config::get();
        let font_family = if config.style.font_family.is_empty() {
            HSTRING::from("Microsoft YaHei UI")
        } else {
            HSTRING::from(config.style.font_family.as_str())
        };
        let (r, g, b) = config.get_primary_color();
        let color_u32 = (r as u32) << 16 | (g as u32) << 8 | b as u32;
        let (_, _, _, selkey_color, _, _, _) = get_colors(color_u32);

        Self {
            letter,
            root,
            font_family,
            font_size: config.style.font_size,
            corner_radius: config.style.corner_radius.clamp(0.0, 32.0),
            primary_color: selkey_color,
            bg_color: D2D1_COLOR_F {
                r: 0.98,
                g: 0.98,
                b: 0.98,
                a: 1.0,
            },
            fg_color: D2D1_COLOR_F {
                r: 0.2,
                g: 0.2,
                b: 0.2,
                a: 1.0,
            },
        }
    }
}

impl From<&Context> for CandidateModel {
    fn from(ctx: &Context) -> Self {
        let config = crate::config::get();
        let font_family = if config.style.font_family.is_empty() {
            HSTRING::from("Microsoft YaHei UI")
        } else {
            HSTRING::from(config.style.font_family.as_str())
        };
        let (r8, g8, b8) = config.get_primary_color();
        let color_u32 = (r8 as u32) << 16 | (g8 as u32) << 8 | b8 as u32;
        let (
            bg_color,
            border_color,
            fg_color,
            selkey_color,
            comment_color,
            highlight_bg_color,
            highlight_fg_color,
        ) = get_colors(color_u32);

        // 分页列数来自用户配置（`style.candidate_count`）。这里就夹住：0 或负数会让
        // 后面的行数计算变成除零（`inf` 一路变成窗口高度的 `inf`），
        // 上限 32 只是防呆（一屏几十列的候选栏没有意义，也没人这么配）。
        let cand_per_row = if config.style.horizontal {
            config.style.candidate_count.clamp(1, 32) as u32
        } else {
            1
        };

        // 候选栏一次显示几个：`style.candidate_count`（文档语义）。
        // 上限 10：选字键只有 1–9 + 0（librime 的 selector 原生支持 0 = 第 10 个），
        // 再多也只能用方向键选，显示出来纯属占地方。
        // rime 一页最多给 `menu/page_size` 个，所以这里再截断一次——
        // page_size 由服务端在部署时对齐（见 server 的 `sync_rime_page_size`）。
        let max_items = config.style.candidate_count.clamp(1, 10) as usize;

        let comments: Vec<String> = ctx
            .candidates
            .comments
            .iter()
            .take(max_items)
            .map(|c| c.str.clone())
            .collect();

        // 选字键：优先用服务端给的 labels（内置计算器的候选 label 为空串——数字是
        // 往算式里输入用的，画「1.」会让人误以为按 1 能选中）。labels 为空时回退
        // rime 的固定 1–9 + 0（librime 的 selector 原生按
        // `index = ((ch - XK_0) + 9) % 10` 映射，所以 0 就是第 10 个）。
        let selkeys = selkeys_from_labels(&ctx.candidates.labels);

        Self {
            items: ctx
                .candidates
                .candies
                .iter()
                .take(max_items)
                .map(|c| c.str.clone())
                .collect(),
            comments,
            selkeys,
            total_pages: ctx.candidates.total_pages,
            current_sel: ctx.candidates.highlighted,
            hovered: None,
            font_family,
            font_size: config.style.font_size,
            corner_radius: config.style.corner_radius.clamp(0.0, 32.0),
            cand_per_row,
            horizontal: config.style.horizontal,
            use_cursor: true,
            selkey_color,
            fg_color,
            comment_color,
            bg_color,
            highlight_fg_color,
            highlight_bg_color,
            border_color,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::selkeys_from_labels;
    use winxime_ipc::Text;

    fn label(s: &str) -> Text {
        Text { str: s.to_string() }
    }

    /// 没有 labels（rime 路径）：保持原来的 1–9 + 0，行为不变。
    #[test]
    fn selkeys_fall_back_to_rime_defaults() {
        let keys = selkeys_from_labels(&[]);
        assert_eq!(
            keys,
            vec![
                '1' as u16, '2' as u16, '3' as u16, '4' as u16, '5' as u16, '6' as u16,
                '7' as u16, '8' as u16, '9' as u16, '0' as u16,
            ]
        );
    }

    /// 有 labels（内置计算器）：空串 = 不画（0），非空取首字符。
    #[test]
    fn selkeys_follow_server_labels() {
        let keys = selkeys_from_labels(&[label(""), label(""), label("")]);
        assert_eq!(keys, vec![0, 0, 0]);

        let keys = selkeys_from_labels(&[label("1"), label(""), label("ab")]);
        assert_eq!(keys, vec!['1' as u16, 0, 'a' as u16]);
    }
}
