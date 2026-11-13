//! 候选栏/字根提示的布局测量：用 DirectWrite 量取文本尺寸，产出 RenderedMetrics。

use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory1, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_METRICS,
};
use windows_core::{w, HSTRING};

use super::model::{CandidateModel, RenderedMetrics, RootModel};
use super::panel::{panel_height, panel_side, PanelSide, PanelPage, MENU_BUTTON_GAP, MENU_BUTTON_SIZE, MENU_STRIP_HEIGHT, PANEL_GAP, PANEL_MIN_WIDTH};
use super::{BLUR_RADIUS, COL_SPACING, MARGIN, MIN_WIDTH, ROW_SPACING};

/// 候选区相对窗口左侧的偏移：横向布局左侧要留 ⋮ 按钮。
fn candidate_offset(horizontal: bool) -> f32 {
    if horizontal {
        MENU_BUTTON_SIZE + MENU_BUTTON_GAP
    } else {
        0.0
    }
}

/// 候选栏行数。
///
/// `cand_per_row` 是**用户配置**来的（`style.candidate_count`），可能是 0 或负数。
/// 直接拿它当除数会得到 `inf`，一路传成窗口高度的 `inf`（`SetWindowPos` 收到无穷大）。
/// 这里统一按「至少 1 列」算，调用方不必再各自防。
fn row_count(items: usize, cand_per_row: usize) -> f32 {
    let columns = cand_per_row.max(1) as f32;
    (items as f32 / columns).ceil().max(1.0)
}

/// 候选栏与插入行之间的空隙（DIP）。
const CURSOR_GAP: f32 = 4.0;

/// 拿不到插入行高度时的回落值（DIP）。正常路径由 TSF 上报真实行高。
const FALLBACK_LINE_HEIGHT: f32 = 20.0;

/// 屏幕工作区（物理像素，左/上/右/下四条边）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// 单轴夹取：区间倒挂（窗口比工作区还大）时贴左/上边，不 panic。
///
/// 标准库的 `clamp` 在 `min > max` 时 panic，而多显示器、缩放切换、
/// 用户把显示器分辨率调得比候选栏还窄时，这个倒挂是真实可能发生的。
fn clamp_axis(value: i32, low: i32, high: i32) -> i32 {
    if low > high {
        low
    } else {
        value.clamp(low, high)
    }
}

/// 一次择位的结果：窗口左上角（物理像素）与「是否摆在插入行上方」。
///
/// `above` 同时是给下一次择位用的粘滞状态：候选数在 1~10 之间跳变时窗口高度
/// 跟着跳，如果每次都重新判断上下，贴着屏幕底打字就会看到候选栏「忽上忽下」
/// （打第一个键时 3 个候选放得下、整词 10 个候选放不下，于是每次都在上下之间跳）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placement {
    pub x: i32,
    pub y: i32,
    pub above: bool,
}

/// 把候选栏摆到插入行附近。
///
/// 规则（与 VISUAL 里「紧贴光标」一致）：
/// 1. 横向：窗口可视内容左边对齐插入点左边，再夹进工作区；
/// 2. 纵向：优先贴在插入行下方 `CURSOR_GAP` 处；下方放不下就翻到插入行上方；
/// 3. 上下都放不下（窗口比工作区高）时退回「下方 + 夹进工作区」，用户至少能看到顶部。
///
/// `prefer_above` 是粘滞位：传 true 表示上次摆在上方，只要上方仍放得下就继续
/// 留在上方，不因为这一次变矮了跳回下方。
///
/// 传入的 `hw_height`/`hw_width` 是**含窗口四周模糊外扩**的物理尺寸，
/// 因此可视内容的位置要各减去一个 `BLUR_RADIUS * scale`。`anchor` 是
/// `(插入点左边, 插入行下沿, 插入行高度)`，全部为物理像素；行高为 0
/// 表示上报方没带（旧版 TSF），按 `FALLBACK_LINE_HEIGHT` 处理。
pub(crate) fn place_window(
    anchor: (i32, i32, i32),
    hw_width: i32,
    hw_height: i32,
    dpi: f32,
    work: WorkArea,
    prefer_above: bool,
) -> Placement {
    let scale = if dpi > 0.0 { dpi / 96.0 } else { 1.0 };
    let gap = (CURSOR_GAP * scale).round() as i32;
    let pad = (BLUR_RADIUS * scale).round() as i32;
    let line_height = if anchor.2 > 0 {
        anchor.2
    } else {
        (FALLBACK_LINE_HEIGHT * scale).round() as i32
    };
    // 窗口里真正看得见的内容高度（扣掉上下两条透明模糊边）。
    let visible_height = (hw_height - pad * 2).max(0);

    let line_top = anchor.1 - line_height;
    let below_top = anchor.1 + gap;
    let above_top = line_top - gap - visible_height;

    let can_below = below_top + visible_height <= work.bottom;
    let can_above = above_top >= work.top;

    // 粘滞：已经在下方就留在下方，只有下方真放不下才翻上去；已经翻到上方
    // 时同理。上下都放不下时回到「下方 + 夹取」，行为与未加粘滞时一致。
    let above = if prefer_above {
        can_above || !can_below
    } else {
        !can_below && can_above
    };
    let visible_top = if above { above_top } else { below_top };

    let x = anchor.0 - pad;
    let y = visible_top - pad;
    Placement {
        x: clamp_axis(x, work.left, work.right - hw_width),
        y: clamp_axis(y, work.top, work.bottom - hw_height),
        above,
    }
}

/// 窗口放在 `(x, y)` 且尺寸为 `w × h` 时，是否整块落在工作区内。
fn fits_within(x: i32, y: i32, w: i32, h: i32, work: WorkArea) -> bool {
    x >= work.left && y >= work.top && x + w <= work.right && y + h <= work.bottom
}

/// 带粘性的一次择位：候选数变化导致窗口高矮跳变时，尽量不让窗口动。
///
/// 打第一个键时几个候选、打出整词时候选变多，窗口高度会跟着变。如果每次都
/// 重新择位，窗口的上边界会随高度来回跳（用户看到的「忽上忽下」）。这里的
/// 取舍是：
/// - 插入点没动，且**原位放得下新尺寸** → 原位改尺寸，上边界不动、只管向下长；
/// - 插入点动了，或原位已经放不下 → 才重新择位，并沿用上一次的上下选择。
pub(crate) fn place_window_sticky(
    anchor: (i32, i32, i32),
    hw_width: i32,
    hw_height: i32,
    dpi: f32,
    work: WorkArea,
    previous: Option<Placement>,
    anchor_changed: bool,
) -> Placement {
    if let Some(prev) = previous {
        if !anchor_changed && fits_within(prev.x, prev.y, hw_width, hw_height, work) {
            return prev;
        }
        return place_window(anchor, hw_width, hw_height, dpi, work, prev.above);
    }
    place_window(anchor, hw_width, hw_height, dpi, work, false)
}

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
            let padding_x = 6.0;
            let padding_y = 4.0;

            for (i, item) in model.items.iter().enumerate() {
                // 0 = 不画选字键（内置计算器的候选 label 为空：数字是算式输入），
                // 宽度按 0 算，文本直接从候选左边开始。
                let selkey = model.selkeys.get(i).copied().unwrap_or(0);

                let mut item_metrics = DWRITE_TEXT_METRICS::default();
                let mut comment_metrics = DWRITE_TEXT_METRICS::default();

                let mut selkey_width = 0.0f32;
                if selkey != 0 {
                    let selkey_buf = [selkey, '.' as u16];
                    let mut selkey_metrics = DWRITE_TEXT_METRICS::default();
                    dwrite
                        .CreateTextLayout(&selkey_buf, &text_format, f32::MAX, f32::MAX)
                        .map_err(|e| format!("CreateTextLayout for selkey failed: {:?}", e))?
                        .GetMetrics(&mut selkey_metrics)
                        .map_err(|e| format!("GetMetrics for selkey failed: {:?}", e))?;
                    selkey_width = selkey_metrics.widthIncludingTrailingWhitespace;
                    item_height = item_height.max(selkey_metrics.height + 2.0 * padding_y);
                }

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

                let text_width = item_metrics.widthIncludingTrailingWhitespace;
                let comment_width = if comment.is_empty() {
                    0.0
                } else {
                    comment_metrics.widthIncludingTrailingWhitespace + 4.0
                };
                selkey_widths.push(selkey_width);
                text_widths.push(text_width);

                let item_width = selkey_width + text_width + comment_width + 2.0 * padding_x;
                item_widths.push(item_width);
                item_height = item_height.max(item_metrics.height + 2.0 * padding_y);
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
                    menu_button: None,
                    panel_side: None,
                    panel_content_width: 0.0,
                });
            }

            let cand_per_row = model.cand_per_row as usize;
            let cpr = cand_per_row.max(1);
            let rows = row_count(model.items.len(), cand_per_row);
            let strip = if model.horizontal {
                0.0
            } else {
                MENU_STRIP_HEIGHT
            };
            let bar_height = strip + rows * item_height + (rows - 1.0) * ROW_SPACING + 2.0 * MARGIN;

            let offset = candidate_offset(model.horizontal);
            let max_row_width = MIN_WIDTH.max({
                let mut w = MIN_WIDTH;
                for row_start in (0..model.items.len()).step_by(cpr) {
                    let row_end = std::cmp::min(row_start + cpr, model.items.len());
                    let row_width: f32 = item_widths[row_start..row_end].iter().copied().sum::<f32>()
                        + (row_end - row_start - 1) as f32 * COL_SPACING
                        + 2.0 * MARGIN
                        + offset;
                    w = w.max(row_width);
                }
                w
            });
            // 横向：左侧 ⋮ 已算进 offset；纵向：顶部条带里居中放 ⋯，不再占右侧宽度。
            let width = if model.horizontal {
                max_row_width
            } else {
                max_row_width.max(MENU_BUTTON_SIZE + 2.0 * MARGIN)
            };

            // 面板展开方向由候选栏排列方向决定（U-14）：
            // 横向候选栏 → 面板在上方（窗口向下加高）；纵向 → 面板在右侧（窗口向右加宽）。
            let side = panel_page.map(|page| (page, panel_side(model.horizontal)));
            let panel_h = panel_page.map(panel_height);
            // 候选栏自身宽度（不含面板增量）：Right 侧面板贴在它右边。
            let bar_width = width;

            let (width, height, panel_content_width) = match (side, panel_h) {
                (Some((_, PanelSide::Above)), Some(ph)) => {
                    // 面板占窗口顶部：候选条带被推到下方，窗口总高 = 面板高 + 间距 + 候选栏高。
                    // 宽度不够容纳面板内容时按面板最小宽撑开（候选条带居左绘制）。
                    let width = width.max(PANEL_MIN_WIDTH);
                    (width, bar_height + PANEL_GAP + ph, width)
                }
                (Some((_, PanelSide::Right)), Some(ph)) => {
                    // 面板贴右侧：窗口总宽 = 候选栏宽 + 间距 + 面板内容宽；
                    // 高度取两者较大者（面板比候选栏高时向下长），候选条带仍占窗口顶部。
                    // 注意不能把候选栏高再叠上去——那会在面板下方留出一段
                    // 「候选栏高 + 间距」的空白（实机 bug：纵向展开面板后下部大片空白）。
                    let panel_width = PANEL_MIN_WIDTH.max(width);
                    let height = ph.max(bar_height);
                    (bar_width + PANEL_GAP + panel_width, height, panel_width)
                }
                _ => (width, bar_height, 0.0),
            };

            let menu_button = if model.horizontal {
                // Above 布局：候选条带被推到窗口底部，⋮ 仍相对候选条带定位。
                let bar_top = match side {
                    Some((_, PanelSide::Above)) => panel_h.unwrap_or(0.0) + PANEL_GAP,
                    _ => 0.0,
                };
                Some((
                    MARGIN,
                    bar_top + MARGIN,
                    MARGIN + MENU_BUTTON_SIZE,
                    bar_top + MARGIN + item_height,
                ))
            } else {
                // 纵向：⋮ 在候选栏自身的顶部条带内——按**候选栏宽度**（不含面板
                // 增量）水平居中。用面板展开后的总宽会把按钮居中到面板区域，
                // 与面板内容重叠（实机 bug：纵向展开面板后汉堡按钮压在面板上）。
                Some((
                    (bar_width - MENU_BUTTON_SIZE) / 2.0,
                    MARGIN,
                    (bar_width + MENU_BUTTON_SIZE) / 2.0,
                    MARGIN + strip,
                ))
            };

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
                menu_button,
                panel_side: side.map(|(_, side)| side),
                panel_content_width,
            })
        }
    }

    /// 候选命中测试：窗口客户区 DIP 坐标 → 候选下标。
    ///
    /// 必须与 `paint.rs` 的摆放递推**同源**（首项 `MARGIN`，逐项加宽度 +
    /// `COL_SPACING`，满 `cand_per_row` 换行并加 `ROW_SPACING`，窗口坐标减
    /// `BLUR_RADIUS`；面板在上方时候选条带被推到窗口底部）——
    /// 「画得出来却点不到」是这类自绘 UI 最典型的 bug。
    pub(crate) fn candidate_item_at(
        model: &CandidateModel,
        metrics: &RenderedMetrics,
        x: f32,
        y: f32,
    ) -> Option<usize> {
        if model.items.is_empty() {
            return None;
        }
        let cx = x - BLUR_RADIUS;
        // Above 布局（面板在上方）时候选条带顶 = 窗口内容高 - 候选栏高。
        let bar_top = match metrics.panel_side {
            Some(super::panel::PanelSide::Above) => metrics.height - metrics.bar_height,
            _ => 0.0,
        };
        let cy = y - BLUR_RADIUS - bar_top;
        if cx < 0.0 || cy < 0.0 || cy >= metrics.bar_height {
            return None;
        }
        let columns = (model.cand_per_row as usize).max(1);
        let start_x = MARGIN + candidate_offset(model.horizontal);
        let start_y = MARGIN
            + if model.horizontal {
                0.0
            } else {
                MENU_STRIP_HEIGHT
            };
        let mut item_x = start_x;
        let mut item_y = start_y;
        for (i, _) in model.items.iter().enumerate() {
            let width = metrics.item_widths.get(i).copied().unwrap_or(0.0);
            if cx >= item_x && cx < item_x + width && cy >= item_y && cy < item_y + metrics.item_height
            {
                return Some(i);
            }
            if (i + 1) % columns == 0 {
                item_x = start_x;
                item_y += metrics.item_height + ROW_SPACING;
            } else {
                item_x += width + COL_SPACING;
            }
        }
        None
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
                menu_button: None,
                panel_side: None,
                panel_content_width: 0.0,
            })
        }
    }


#[cfg(test)]
mod tests {
    use super::*;

    /// 行数必须永远有限：`cand_per_row` 是用户配置来的，0 会让除法变成 `inf`，
    /// 而 `bar_height` / `hw_height` 会一路跟着变成 `inf`（窗口尺寸就废了）。
    #[test]
    fn row_count_never_divides_by_zero() {
        // 0 列 = 按 1 列处理，不是崩溃、也不是 inf。
        assert_eq!(row_count(5, 0), 5.0);
        assert_eq!(row_count(1, 0), 1.0);
        assert!(row_count(5, 0).is_finite());
    }

    /// 常规分页：正好整除、有余数、单列。
    #[test]
    fn row_count_matches_page_size() {
        assert_eq!(row_count(5, 5), 1.0);
        assert_eq!(row_count(6, 5), 2.0);
        assert_eq!(row_count(10, 5), 2.0);
        assert_eq!(row_count(5, 1), 5.0);
        assert_eq!(row_count(9, 3), 3.0);
    }

    /// 极端值不能溢出成 inf / NaN（`usize::MAX` 列只是「一行全放下」）。
    #[test]
    fn row_count_is_finite_for_extreme_inputs() {
        assert!(row_count(usize::MAX, usize::MAX).is_finite());
        assert!(row_count(usize::MAX, 1).is_finite());
        assert_eq!(row_count(3, usize::MAX), 1.0);
    }

    fn hit_model() -> CandidateModel {
        CandidateModel {
            items: vec!["a".into(), "b".into(), "c".into()],
            cand_per_row: 2,
            ..CandidateModel::default()
        }
    }

    fn hit_metrics() -> RenderedMetrics {
        RenderedMetrics {
            width: 200.0,
            height: 80.0,
            hw_width: 200.0,
            hw_height: 80.0,
            bar_height: 80.0,
            item_height: 20.0,
            item_widths: vec![60.0, 70.0, 80.0],
            selkey_widths: vec![0.0, 0.0, 0.0],
            text_widths: vec![40.0, 50.0, 60.0],
            menu_button: None,
            panel_side: None,
            panel_content_width: 0.0,
        }
    }

    /// 两列三行：第 1 项、第 2 项、换行后的第 3 项都能命中。
    #[test]
    fn candidate_hit_matches_paint_layout() {
        let model = hit_model();
        let metrics = hit_metrics();
        // 窗口坐标 = 内容坐标 + BLUR_RADIUS；纵向布局顶部有 ⋯ 条带。
        let top = MARGIN + MENU_STRIP_HEIGHT;
        let at = |x: f32, y: f32| candidate_item_at(&model, &metrics, x + BLUR_RADIUS, y + BLUR_RADIUS);
        assert_eq!(at(MARGIN + 5.0, top + 5.0), Some(0));
        assert_eq!(at(MARGIN + 60.0 + COL_SPACING + 5.0, top + 5.0), Some(1));
        // 第二行：y = top + item_height + ROW_SPACING
        assert_eq!(
            at(MARGIN + 5.0, top + 20.0 + ROW_SPACING + 5.0),
            Some(2)
        );
    }

    /// 间距、窗口外、blur 边距里都不能误命中。
    #[test]
    fn candidate_hit_rejects_gaps_and_outside() {
        let model = hit_model();
        let metrics = hit_metrics();
        let at = |x: f32, y: f32| candidate_item_at(&model, &metrics, x + BLUR_RADIUS, y + BLUR_RADIUS);
        // 两项之间的 COL_SPACING 空隙
        let top = MARGIN + MENU_STRIP_HEIGHT;
        assert_eq!(at(MARGIN + 60.0 + 0.5, top + 5.0), None);
        // blur 边距（内容区之外）
        assert_eq!(candidate_item_at(&model, &metrics, 1.0, 1.0), None);
        // 候选栏下方（面板区）
        assert_eq!(at(MARGIN + 5.0, 100.0), None);
    }

    fn work_area() -> WorkArea {
        WorkArea {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1000,
        }
    }

    /// 下方空间够：紧贴插入行下方，并扣掉窗口的透明模糊边。
    ///
    /// 100% 缩放下 `BLUR_RADIUS=8`、`CURSOR_GAP=4`：窗口左上角 =
    /// (插入点左 100 - 8, 行下沿 200 + 4 - 8)。
    #[test]
    fn place_window_sits_below_caret_when_room_allows() {
        let pos = place_window((100, 200, 20), 200, 60, 96.0, work_area(), false);
        assert_eq!((pos.x, pos.y), (92, 196));
        assert!(!pos.above);
    }

    /// 下方放不下：翻到插入行上方（而不是把窗口顶到屏幕外）。
    #[test]
    fn place_window_flips_above_when_bottom_is_tight() {
        let work = WorkArea {
            bottom: 240,
            ..work_area()
        };
        let pos = place_window((100, 200, 20), 200, 60, 96.0, work, false);
        // 可视高 60-16=44；行上沿 180，上方顶 = 180-4-44=132，再扣模糊边。
        assert_eq!((pos.x, pos.y), (92, 124));
        assert!(pos.above);
    }

    /// 上下都放不下（窗口比工作区还高）：退回下方并夹进工作区，不 panic、不留负坐标。
    #[test]
    fn place_window_degrades_gracefully_when_both_sides_too_tight() {
        let work = WorkArea {
            bottom: 300,
            ..work_area()
        };
        let pos = place_window((100, 200, 20), 200, 400, 96.0, work, false);
        assert_eq!(pos.x, 92);
        assert_eq!(pos.y, 0);
    }

    /// 右侧越界时夹回工作区右边界。
    #[test]
    fn place_window_clamps_to_right_edge() {
        let pos = place_window((1900, 200, 20), 200, 60, 96.0, work_area(), false);
        assert_eq!(pos.x, 1920 - 200);
    }

    /// 行高为 0（旧版 TSF 不带这个字段）时按回落行高算，行为与带行高一致。
    #[test]
    fn place_window_falls_back_for_missing_line_height() {
        assert_eq!(
            place_window((100, 200, 0), 200, 60, 96.0, work_area(), false),
            place_window((100, 200, 20), 200, 60, 96.0, work_area(), false)
        );
    }

    /// 窗口比工作区还宽：贴左边而不是 panic（`clamp` 区间倒挂）。
    #[test]
    fn place_window_survives_window_wider_than_work_area() {
        let pos = place_window((100, 200, 20), 3000, 60, 96.0, work_area(), false);
        assert_eq!(pos.x, 0);
    }

    /// 200% 缩放：间隙与模糊边都按比例放大，不能还用 96 DPI 的像素数。
    #[test]
    fn place_window_scales_with_dpi() {
        let pos = place_window((200, 400, 40), 400, 120, 192.0, work_area(), false);
        // 可视高 120-32=88；下方顶 400+8=408；窗口左上 y = 408-16。
        assert_eq!((pos.x, pos.y), (184, 392));
    }

    /// 光标贴屏幕底边：翻上方后仍要在工作区内（这是「超出屏幕」的原始 bug）。
    #[test]
    fn place_window_never_exceeds_bottom_edge() {
        let work = WorkArea {
            bottom: 1080,
            ..work_area()
        };
        let pos = place_window((100, 1075, 24), 300, 180, 96.0, work, false);
        assert!(pos.y + 180 <= work.bottom, "窗口底边越界: y={}", pos.y);
    }

    /// 粘滞（上方）但上方已经放不下：退回下方，而不是把窗口顶到工作区外。
    #[test]
    fn place_window_falls_back_to_below_when_above_does_not_fit() {
        let work = WorkArea {
            top: 200,
            ..work_area()
        };
        // 插入行贴住工作区上边缘：上方没空间，下方有。
        let pos = place_window((100, 240, 20), 200, 60, 96.0, work, true);
        assert!(!pos.above);
        assert_eq!(pos.y, 236);
    }

    /// 粘滞（上方）：已在上面且上面仍放得下，即使这次下方也放得下，也不翻回下方。
    #[test]
    fn place_window_keeps_above_when_it_was_above() {
        let pos = place_window((100, 200, 20), 200, 60, 96.0, work_area(), true);
        assert!(pos.above);
    }

    /// `fits_within` 的边界：贴住工作区右下边缘仍算放得下，多一像素就不算。
    #[test]
    fn fits_within_is_inclusive_on_the_edges() {
        let work = work_area();
        assert!(fits_within(0, 0, 1920, 1000, work));
        assert!(fits_within(1720, 940, 200, 60, work));
        assert!(!fits_within(1721, 940, 200, 60, work));
        assert!(!fits_within(1720, 941, 200, 60, work));
    }

    /// 插入点没动、原位放得下新尺寸：高度变化（候选数跳变）时上边界不动。
    #[test]
    fn place_window_sticky_keeps_position_when_anchor_is_stable() {
        let anchor = (100, 200, 20);
        let first = place_window_sticky(anchor, 200, 60, 96.0, work_area(), None, true);
        assert_eq!((first.x, first.y), (92, 196));
        // 候选变多：窗口从 60 长到 160，原位仍放得下 → 坐标一动不动。
        let grown = place_window_sticky(anchor, 200, 160, 96.0, work_area(), Some(first), false);
        assert_eq!((grown.x, grown.y), (92, 196));
        // 变矮同理：不会因为「变矮了更该贴上去」而跳。
        let shrunk = place_window_sticky(anchor, 200, 40, 96.0, work_area(), Some(grown), false);
        assert_eq!((shrunk.x, shrunk.y), (92, 196));
    }

    /// 插入点变了：必须重新择位，否则候选栏会粘在上一处光标旁边。
    #[test]
    fn place_window_sticky_replaces_when_anchor_moves() {
        let first = place_window_sticky((100, 200, 20), 200, 60, 96.0, work_area(), None, true);
        let moved = place_window_sticky((500, 600, 20), 200, 60, 96.0, work_area(), Some(first), true);
        assert_eq!((moved.x, moved.y), (492, 596));
    }

    /// 插入点没动但原位已放不下（窗口长到超出屏幕）：重新择位并沿用上下粘滞。
    #[test]
    fn place_window_sticky_relocates_when_window_no_longer_fits() {
        let work = WorkArea {
            bottom: 800,
            ..work_area()
        };
        let anchor = (100, 700, 20);
        let first = place_window_sticky(anchor, 200, 60, 96.0, work, None, true);
        assert!(!first.above);
        // 窗口一下长到 300 高，下方（704+284=988）超出 800 → 必须翻上去。
        let grown = place_window_sticky(anchor, 200, 300, 96.0, work, Some(first), false);
        assert!(grown.above);
        assert!(grown.y + 300 <= work.bottom, "窗口底边越界: y={}", grown.y);
    }
}
