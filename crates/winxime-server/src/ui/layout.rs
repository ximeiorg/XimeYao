//! 候选栏/字根提示的布局测量：用 DirectWrite 量取文本尺寸，产出 RenderedMetrics。

use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory1, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_METRICS,
};
use windows_core::{w, HSTRING};

use super::model::{CandidateModel, RenderedMetrics, RootModel};
use super::panel::{panel_extra_height, PanelPage, MENU_BUTTON_GAP, MENU_BUTTON_SIZE, PANEL_MIN_WIDTH};
use super::{BLUR_RADIUS, COL_SPACING, MARGIN, MIN_WIDTH, ROW_SPACING};

    pub(crate) fn calculate_client_rect(
        dwrite: &IDWriteFactory1,
        model: &CandidateModel,
        dpi: f32,
        panel_page: Option<PanelPage>,
    ) -> Result<RenderedMetrics, String> {
        unsafe {
            let scale = dpi / 96.0;

            let text_format = dwrite
                .CreateTextFormat(
                    &model.font_family,
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    model.font_size,
                    w!("zh-CN"),
                )
                .map_err(|e| format!("CreateTextFormat failed: {:?}", e))?;

            let mut item_height = 0.0f32;
            let mut item_widths = Vec::new();
            let mut selkey_widths = Vec::new();
            let mut text_widths = Vec::new();
            let mut comment_widths = Vec::new();
            let mut selkey_buf = "?.".encode_utf16().collect::<Vec<_>>();

            for (i, item) in model.items.iter().enumerate() {
                let selkey = model.selkeys.get(i).copied().unwrap_or('?' as u16);
                selkey_buf[0] = selkey;

                let mut selkey_metrics = DWRITE_TEXT_METRICS::default();
                let mut item_metrics = DWRITE_TEXT_METRICS::default();
                let mut comment_metrics = DWRITE_TEXT_METRICS::default();

                dwrite
                    .CreateTextLayout(&selkey_buf, &text_format, f32::MAX, f32::MAX)
                    .map_err(|e| format!("CreateTextLayout for selkey failed: {:?}", e))?
                    .GetMetrics(&mut selkey_metrics)
                    .map_err(|e| format!("GetMetrics for selkey failed: {:?}", e))?;

                let item_hstring = HSTRING::from(item);
                dwrite
                    .CreateTextLayout(&item_hstring, &text_format, f32::MAX, f32::MAX)
                    .map_err(|e| format!("CreateTextLayout for item failed: {:?}", e))?
                    .GetMetrics(&mut item_metrics)
                    .map_err(|e| format!("GetMetrics for item failed: {:?}", e))?;

                let comment = model.comments.get(i).cloned().unwrap_or_default();
                let comment_hstring = HSTRING::from(&comment);
                dwrite
                    .CreateTextLayout(&comment_hstring, &text_format, f32::MAX, f32::MAX)
                    .map_err(|e| format!("CreateTextLayout for comment failed: {:?}", e))?
                    .GetMetrics(&mut comment_metrics)
                    .map_err(|e| format!("GetMetrics for comment failed: {:?}", e))?;

                let padding_x = 6.0;
                let padding_y = 4.0;
                let selkey_width = selkey_metrics.widthIncludingTrailingWhitespace;
                let text_width = item_metrics.widthIncludingTrailingWhitespace;
                let comment_width = if comment.is_empty() {
                    0.0
                } else {
                    comment_metrics.widthIncludingTrailingWhitespace + 4.0
                };
                selkey_widths.push(selkey_width);
                text_widths.push(text_width);
                comment_widths.push(comment_width);

                let item_width = selkey_width + text_width + comment_width + 2.0 * padding_x;
                item_widths.push(item_width);
                item_height = item_height
                    .max(item_metrics.height + 2.0 * padding_y)
                    .max(selkey_metrics.height + 2.0 * padding_y);
            }

            let items_len = model.items.len() as f32;
            if items_len == 0.0 {
                return Ok(RenderedMetrics {
                    width: 100.0,
                    height: 30.0,
                    hw_width: ((100.0 + BLUR_RADIUS * 2.0) * scale).ceil(),
                    hw_height: ((30.0 + BLUR_RADIUS * 2.0) * scale).ceil(),
                    bar_height: 30.0,
                    item_height: 20.0,
                    item_widths: Vec::new(),
                    selkey_widths: Vec::new(),
                    text_widths: Vec::new(),
                    comment_widths: Vec::new(),
                    menu_button: None,
                });
            }

            let cand_per_row = model.cand_per_row as usize;

            // "⋮" 菜单按钮：仅横向布局，排在最后一个候选所在行的行尾（与 macOS 一致）。
            let cpr = cand_per_row.max(1);
            let last = model.items.len() - 1;
            let row_of_last = last / cpr;
            let mut menu_button = None;
            if model.horizontal {
                let col_of_last = last % cpr;
                let row_end_x = MARGIN
                    + item_widths[row_of_last * cpr..=last].iter().copied().sum::<f32>()
                    + col_of_last as f32 * COL_SPACING;
                let btn_x = row_end_x + MENU_BUTTON_GAP;
                let btn_y = MARGIN + row_of_last as f32 * (item_height + ROW_SPACING);
                menu_button = Some((
                    btn_x,
                    btn_y,
                    btn_x + MENU_BUTTON_SIZE,
                    btn_y + item_height,
                ));
            }

            let mut max_row_width = MIN_WIDTH;
            for (row_index, row_start) in (0..model.items.len()).step_by(cpr).enumerate() {
                let row_end = std::cmp::min(row_start + cpr, model.items.len());
                let mut row_width: f32 = item_widths[row_start..row_end].iter().copied().sum::<f32>()
                    + (row_end - row_start - 1) as f32 * COL_SPACING
                    + 2.0 * MARGIN;
                if menu_button.is_some() && row_index == row_of_last {
                    // 菜单按钮占位计入其所在行的行宽。
                    row_width += MENU_BUTTON_GAP + MENU_BUTTON_SIZE;
                }
                max_row_width = max_row_width.max(row_width);
            }

            let rows = (items_len / cand_per_row as f32).ceil().max(1.0);
            let bar_height = rows * item_height + (rows - 1.0) * ROW_SPACING + 2.0 * MARGIN;

            // 面板仅在横向布局且有候选时展开（与 macOS 一致，竖排无菜单按钮入口）。
            // 面板高度按页面取（「剪切板」子页比菜单页高）。
            let panel_height = panel_page.filter(|_| model.horizontal).map(panel_extra_height);
            let width = if panel_height.is_some() {
                max_row_width.max(PANEL_MIN_WIDTH)
            } else {
                max_row_width
            };
            let height = bar_height + panel_height.unwrap_or(0.0);

            let hw_width = ((width + BLUR_RADIUS * 2.0) * scale).ceil();
            let hw_height = ((height + BLUR_RADIUS * 2.0) * scale).ceil();

            Ok(RenderedMetrics {
                width,
                height,
                hw_width,
                hw_height,
                bar_height,
                item_height,
                item_widths,
                selkey_widths,
                text_widths,
                comment_widths,
                menu_button,
            })
        }
    }

    pub(crate) fn calculate_root_rect(
        dwrite: &IDWriteFactory1,
        model: &RootModel,
        dpi: f32,
    ) -> Result<RenderedMetrics, String> {
        unsafe {
            let scale = dpi / 96.0;

            let text_format = dwrite
                .CreateTextFormat(
                    &model.font_family,
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    model.font_size,
                    w!("zh-CN"),
                )
                .map_err(|e| format!("CreateTextFormat failed: {:?}", e))?;

            let letter_buf = [model.letter as u16];

            let mut letter_metrics = DWRITE_TEXT_METRICS::default();
            dwrite
                .CreateTextLayout(&letter_buf, &text_format, f32::MAX, f32::MAX)
                .map_err(|e| format!("CreateTextLayout for letter failed: {:?}", e))?
                .GetMetrics(&mut letter_metrics)
                .map_err(|e| format!("GetMetrics for letter failed: {:?}", e))?;

            let root_hstring = HSTRING::from(&model.root);
            let mut root_metrics = DWRITE_TEXT_METRICS::default();
            dwrite
                .CreateTextLayout(&root_hstring, &text_format, f32::MAX, f32::MAX)
                .map_err(|e| format!("CreateTextLayout for root failed: {:?}", e))?
                .GetMetrics(&mut root_metrics)
                .map_err(|e| format!("GetMetrics for root failed: {:?}", e))?;

            let letter_width = letter_metrics.widthIncludingTrailingWhitespace;
            let root_width = root_metrics.widthIncludingTrailingWhitespace;
            let text_height = model.font_size;

            let key_bg_width = letter_width + 16.0;
            let key_bg_height = 24.0;
            let padding = 12.0;

            let width = (padding + key_bg_width + 8.0 + root_width + padding).max(80.0);
            let height = (key_bg_height + padding).max(36.0);

            let hw_width = ((width + BLUR_RADIUS * 2.0) * scale).ceil();
            let hw_height = ((height + BLUR_RADIUS * 2.0) * scale).ceil();

            Ok(RenderedMetrics {
                width,
                height,
                hw_width,
                hw_height,
                bar_height: height,
                item_height: text_height,
                item_widths: vec![root_width],
                selkey_widths: vec![letter_width],
                text_widths: vec![root_width],
                comment_widths: vec![],
                menu_button: None,
            })
        }
    }
