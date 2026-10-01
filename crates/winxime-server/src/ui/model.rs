//! 候选栏数据模型：绘制所需的输入数据（候选/字根/布局结果）。

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows_core::HSTRING;

use crate::config::get_colors;
use xime_config::XimeConfig;
use winxime_ipc::Context;

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
    pub(crate) comment_widths: Vec<f32>,
    /// "⋮" 菜单按钮矩形（窗口 DIP 坐标，仅横向布局存在）。
    pub(crate) menu_button: Option<(f32, f32, f32, f32)>,
}

#[derive(Debug, Clone, Default)]
pub struct CandidateModel {
    pub items: Vec<String>,
    pub comments: Vec<String>,
    pub selkeys: Vec<u16>,
    pub total_pages: u32,
    pub current_page: u32,
    pub font_family: HSTRING,
    pub font_size: f32,
    pub cand_per_row: u32,
    pub horizontal: bool,
    pub use_cursor: bool,
    pub current_sel: usize,
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
    pub primary_color: D2D1_COLOR_F,
    pub bg_color: D2D1_COLOR_F,
    pub fg_color: D2D1_COLOR_F,
}

impl From<(char, String)> for RootModel {
    fn from((letter, root): (char, String)) -> Self {
        let config = XimeConfig::load();
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
        let config = XimeConfig::load();
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

        let cand_per_row = if config.style.horizontal {
            config.style.candidate_count as u32
        } else {
            1
        };

        let comments: Vec<String> = ctx
            .candidates
            .comments
            .iter()
            .map(|c| c.str.clone())
            .collect();

        // 选字键固定 1..5（与候选栏绘制一致）。
        let selkeys: Vec<u16> = vec![
            '1' as u16,
            '2' as u16,
            '3' as u16,
            '4' as u16,
            '5' as u16,
        ];

        Self {
            items: ctx
                .candidates
                .candies
                .iter()
                .map(|c| c.str.clone())
                .collect(),
            comments,
            selkeys,
            total_pages: ctx.candidates.total_pages,
            current_page: ctx.candidates.current_page + 1,
            current_sel: ctx.candidates.highlighted as usize,
            font_family: if config.style.font_family.is_empty() {
                HSTRING::from("Microsoft YaHei UI")
            } else {
                HSTRING::from(config.style.font_family.as_str())
            },
            font_size: config.style.font_size,
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
