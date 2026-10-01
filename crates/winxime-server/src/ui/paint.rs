//! 候选栏与字根提示的 Direct2D 绘制。

use tracing::info;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D_RECT_F, D2D_SIZE_F,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1GaussianBlur, ID2D1BitmapRenderTarget, ID2D1DeviceContext,
    D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{DXGI_PRESENT, DXGI_SWAP_CHAIN_FLAG};
use windows_core::{w, HSTRING};
use windows_numerics::Vector2;

use super::layout::calculate_client_rect;
use super::model::{CandidateModel, RenderedMetrics, RootModel};
use super::panel::{draw_menu_button, draw_panel, PanelGrid, PanelList};
use super::view::RenderedView;
use super::{BLUR_RADIUS, COL_SPACING, MARGIN, PanelPaintState, ROW_SPACING};

    pub(crate) fn on_paint_root(
        view: &RenderedView,
        model: &RootModel,
        dpi: f32,
        metrics: &RenderedMetrics,
    ) -> Result<(), String> {
        unsafe {
            view.d2d_context.SetTarget(None);
            view.swapchain
                .ResizeBuffers(
                    0,
                    metrics.hw_width as u32,
                    metrics.hw_height as u32,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .map_err(|e| format!("ResizeBuffers failed: {:?}", e))?;

            view.d2d_context.SetDpi(dpi, dpi);
            RenderedView::create_swapchain_bitmap(&view.swapchain, &view.d2d_context)?;

            let text_format = view.dwrite_factory
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
            let _ = text_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);

            let text_format_centered = view.dwrite_factory
                .CreateTextFormat(
                    &model.font_family,
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    model.font_size,
                    w!("zh-CN"),
                )
                .map_err(|e| format!("CreateTextFormat centered failed: {:?}", e))?;
            let _ = text_format_centered.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = text_format_centered.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);

            view.d2d_context.BeginDraw();

            let blur_radius = BLUR_RADIUS;
            let corner_radius = 8.0;
            let key_bg_corner_radius = 4.0;

            let bg_brush = view.d2d_context
                .CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: 0.98,
                        g: 0.98,
                        b: 0.98,
                        a: 1.0,
                    },
                    None,
                )
                .map_err(|e| format!("CreateSolidColorBrush bg failed: {:?}", e))?;

            let border_brush = view.d2d_context
                .CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: 0.88,
                        g: 0.88,
                        b: 0.88,
                        a: 1.0,
                    },
                    None,
                )
                .map_err(|e| format!("CreateSolidColorBrush border failed: {:?}", e))?;

            let key_bg_brush = view.d2d_context
                .CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: model.primary_color.r,
                        g: model.primary_color.g,
                        b: model.primary_color.b,
                        a: 0.19,
                    },
                    None,
                )
                .map_err(|e| format!("CreateSolidColorBrush key_bg failed: {:?}", e))?;

            let key_border_brush = view.d2d_context
                .CreateSolidColorBrush(&model.primary_color, None)
                .map_err(|e| format!("CreateSolidColorBrush key_border failed: {:?}", e))?;

            let key_text_brush = view.d2d_context
                .CreateSolidColorBrush(&model.primary_color, None)
                .map_err(|e| format!("CreateSolidColorBrush key_text failed: {:?}", e))?;

            let root_text_brush = view.d2d_context
                .CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: 0.2,
                        g: 0.2,
                        b: 0.2,
                        a: 1.0,
                    },
                    None,
                )
                .map_err(|e| format!("CreateSolidColorBrush root_text failed: {:?}", e))?;

            draw_drop_shadow(
                &view.d2d_context,
                metrics.width,
                metrics.height,
                blur_radius,
                corner_radius,
            )?;

            let bg_rounded_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: blur_radius,
                    top: blur_radius,
                    right: metrics.width + blur_radius,
                    bottom: metrics.height + blur_radius,
                },
                radiusX: corner_radius,
                radiusY: corner_radius,
            };
            view.d2d_context
                .FillRoundedRectangle(&bg_rounded_rect, &bg_brush);
            view.d2d_context
                .DrawRoundedRectangle(&bg_rounded_rect, &border_brush, 2.0, None);

            let letter_width = metrics.selkey_widths.get(0).copied().unwrap_or(20.0);
            let key_bg_width = letter_width + 16.0;
            let key_bg_height = 24.0;
            let x_start = blur_radius + 12.0;
            let y_center = blur_radius + (metrics.height - key_bg_height) / 2.0;

            let key_bg_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: x_start,
                    top: y_center,
                    right: x_start + key_bg_width,
                    bottom: y_center + key_bg_height,
                },
                radiusX: key_bg_corner_radius,
                radiusY: key_bg_corner_radius,
            };
            view.d2d_context
                .FillRoundedRectangle(&key_bg_rect, &key_bg_brush);
            view.d2d_context
                .DrawRoundedRectangle(&key_bg_rect, &key_border_brush, 1.5, None);

            let letter_rect = D2D_RECT_F {
                left: x_start,
                top: y_center,
                right: x_start + key_bg_width,
                bottom: y_center + key_bg_height,
            };

            let letter_buf = [model.letter as u16];
            view.d2d_context.DrawText(
                &letter_buf,
                &text_format_centered,
                &letter_rect,
                &key_text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            let root_hstring = HSTRING::from(&model.root);
            let root_x_start = x_start + key_bg_width + 8.0;
            let root_rect = D2D_RECT_F {
                left: root_x_start,
                top: blur_radius + (metrics.height - key_bg_height) / 2.0,
                right: metrics.width + blur_radius - 12.0,
                bottom: blur_radius + (metrics.height + key_bg_height) / 2.0,
            };
            view.d2d_context.DrawText(
                &root_hstring,
                &text_format,
                &root_rect,
                &root_text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            view.d2d_context
                .EndDraw(None, None)
                .map_err(|e| format!("EndDraw failed: {:?}", e))?;
            let _ = view.swapchain.Present(1, DXGI_PRESENT(0)).ok();

            Ok(())
        }
    }

    pub(crate) fn on_paint(
        view: &RenderedView,
        model: &CandidateModel,
        panel: Option<PanelPaintState>,
        list: &PanelList,
        grid: &PanelGrid,
    ) -> Result<(), String> {
        let dpi = RenderedView::get_dpi_for_window(view.hwnd);
        let metrics =
            calculate_client_rect(&view.dwrite_factory, model, dpi, panel.map(|state| state.page))?;
        on_paint_with_metrics(view, model, dpi, &metrics, panel, list, grid)
    }

    pub(crate) fn on_paint_with_metrics(
        view: &RenderedView,
        model: &CandidateModel,
        dpi: f32,
        metrics: &RenderedMetrics,
        panel: Option<PanelPaintState>,
        list: &PanelList,
        grid: &PanelGrid,
    ) -> Result<(), String> {
        unsafe {
            info!(
                "on_paint: dpi={}, width={}, height={}, hw_width={}, hw_height={}",
                dpi, metrics.width, metrics.height, metrics.hw_width, metrics.hw_height
            );
            info!("on_paint: item_widths={:?}", metrics.item_widths);

            view.d2d_context.SetTarget(None);
            view.swapchain
                .ResizeBuffers(
                    0,
                    metrics.hw_width as u32,
                    metrics.hw_height as u32,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .map_err(|e| format!("ResizeBuffers failed: {:?}", e))?;

            view.d2d_context.SetDpi(dpi, dpi);
            RenderedView::create_swapchain_bitmap(&view.swapchain, &view.d2d_context)?;

            let text_format = view.dwrite_factory
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

            view.d2d_context.BeginDraw();

            let blur_radius = BLUR_RADIUS;
            let corner_radius = 10.0;

            let bg_brush = view.d2d_context
                .CreateSolidColorBrush(&model.bg_color, None)
                .map_err(|e| format!("CreateSolidColorBrush bg failed: {:?}", e))?;

            let border_brush = view.d2d_context
                .CreateSolidColorBrush(&model.border_color, None)
                .map_err(|e| format!("CreateSolidColorBrush border failed: {:?}", e))?;

            let selkey_brush = view.d2d_context
                .CreateSolidColorBrush(&model.selkey_color, None)
                .map_err(|e| format!("CreateSolidColorBrush selkey failed: {:?}", e))?;

            let text_brush = view.d2d_context
                .CreateSolidColorBrush(&model.fg_color, None)
                .map_err(|e| format!("CreateSolidColorBrush text failed: {:?}", e))?;

            let highlight_brush = view.d2d_context
                .CreateSolidColorBrush(&model.highlight_bg_color, None)
                .map_err(|e| format!("CreateSolidColorBrush highlight failed: {:?}", e))?;

            let selected_text_brush = view.d2d_context
                .CreateSolidColorBrush(&model.highlight_fg_color, None)
                .map_err(|e| format!("CreateSolidColorBrush selected_text failed: {:?}", e))?;


            let bg_rounded_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: blur_radius,
                    top: blur_radius,
                    right: metrics.width + blur_radius,
                    bottom: metrics.height + blur_radius,
                },
                radiusX: corner_radius,
                radiusY: corner_radius,
            };
            view.d2d_context
                .FillRoundedRectangle(&bg_rounded_rect, &bg_brush);

            let border_rounded_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: blur_radius + 0.5,
                    top: blur_radius + 0.5,
                    right: metrics.width + blur_radius - 0.5,
                    bottom: metrics.height + blur_radius - 0.5,
                },
                radiusX: corner_radius,
                radiusY: corner_radius,
            };
            view.d2d_context
                .DrawRoundedRectangle(&border_rounded_rect, &border_brush, 0.5, None);

            let comment_brush = view.d2d_context
                .CreateSolidColorBrush(&model.comment_color, None)
                .map_err(|e| format!("CreateSolidColorBrush comment failed: {:?}", e))?;

            let mut col = 0usize;
            let mut x = MARGIN + blur_radius;
            let mut y = MARGIN + blur_radius;
            let padding_x = 6.0;
            let padding_y = 4.0;

            for (i, item) in model.items.iter().enumerate() {
                let selkey = model.selkeys.get(i).copied().unwrap_or('?' as u16);
                let mut selkey_buf = [0u16; 3];
                selkey_buf[0] = selkey;
                selkey_buf[1] = '.' as u16;

                let item_width = metrics.item_widths.get(i).copied().unwrap_or(60.0);
                let selkey_width = metrics.selkey_widths.get(i).copied().unwrap_or(20.0);
                let text_width = metrics.text_widths.get(i).copied().unwrap_or(40.0);
                let comment_width = metrics.comment_widths.get(i).copied().unwrap_or(0.0);

                info!(
                    "  item {}: x={}, item_width={}, text='{}'",
                    i, x, item_width, item
                );
                info!(
                    "  bg_rect: left={}, right={}",
                    blur_radius,
                    metrics.width + blur_radius
                );
                info!("  item_right={}", x + item_width);

                let selkey_rect = D2D_RECT_F {
                    left: x + padding_x,
                    top: y + padding_y,
                    right: x + selkey_width + padding_x,
                    bottom: y + metrics.item_height - padding_y,
                };

                let text_rect = D2D_RECT_F {
                    left: x + selkey_width + padding_x,
                    top: y + padding_y,
                    right: x + selkey_width + text_width + padding_x,
                    bottom: y + metrics.item_height - padding_y,
                };

                let comment_rect = D2D_RECT_F {
                    left: x + selkey_width + text_width + padding_x + 4.0,
                    top: y + padding_y,
                    right: x + item_width - padding_x,
                    bottom: y + metrics.item_height - padding_y,
                };

                let item_hstring = HSTRING::from(item);
                let comment = model.comments.get(i).cloned().unwrap_or_default();
                let comment_hstring = HSTRING::from(&comment);

                if model.use_cursor && i == model.current_sel {
                    let highlight_rounded_rect = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: x,
                            top: y,
                            right: x + item_width,
                            bottom: y + metrics.item_height,
                        },
                        radiusX: 6.0,
                        radiusY: 6.0,
                    };
                    view.d2d_context
                        .FillRoundedRectangle(&highlight_rounded_rect, &highlight_brush);

                    view.d2d_context.DrawText(
                        &selkey_buf[..2],
                        &text_format,
                        &selkey_rect,
                        &selected_text_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );

                    view.d2d_context.DrawText(
                        &item_hstring,
                        &text_format,
                        &text_rect,
                        &selected_text_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );

                    if !comment.is_empty() {
                        view.d2d_context.DrawText(
                            &comment_hstring,
                            &text_format,
                            &comment_rect,
                            &selected_text_brush,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                } else {
                    view.d2d_context.DrawText(
                        &selkey_buf[..2],
                        &text_format,
                        &selkey_rect,
                        &selkey_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );

                    view.d2d_context.DrawText(
                        &item_hstring,
                        &text_format,
                        &text_rect,
                        &text_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );

                    if !comment.is_empty() {
                        view.d2d_context.DrawText(
                            &comment_hstring,
                            &text_format,
                            &comment_rect,
                            &comment_brush,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                }

                col += 1;
                if col >= model.cand_per_row as usize {
                    col = 0;
                    x = MARGIN + blur_radius;
                    y += metrics.item_height + ROW_SPACING;
                } else {
                    x += item_width + COL_SPACING;
                }
            }

            // 面板区：候选栏下方（展开时），与候选区同一窗口、间距分隔。
            if let Some(state) = panel {
                draw_panel(
                    &view.d2d_context,
                    &view.dwrite_factory,
                    model,
                    blur_radius,
                    metrics.width,
                    metrics.bar_height,
                    state,
                    list,
                    grid,
                )?;
            }

            // "⋮" 菜单按钮（候选栏最右侧，横向布局）。
            if let Some(rect) = metrics.menu_button {
                draw_menu_button(
                    &view.d2d_context,
                    &view.dwrite_factory,
                    model,
                    blur_radius,
                    rect,
                )?;
            }

            view.d2d_context
                .EndDraw(None, None)
                .map_err(|e| format!("EndDraw failed: {:?}", e))?;

            let _ = view.swapchain.Present(1, DXGI_PRESENT(0)).ok();
        }

        Ok(())
    }
/// 绘制投影：离屏画圆角矩形后做高斯模糊，再叠加到当前目标（须在 BeginDraw 之后调用）。
pub(crate) fn draw_drop_shadow(
    d2d: &ID2D1DeviceContext,
    width: f32,
    height: f32,
    blur_radius: f32,
    corner_radius: f32,
) -> Result<(), String> {
    unsafe {
            let shadow_render_target: ID2D1BitmapRenderTarget = d2d
                .CreateCompatibleRenderTarget(
                    Some(&D2D_SIZE_F {
                        width: width + blur_radius * 2.0,
                        height: height + blur_radius * 2.0,
                    }),
                    None,
                    None,
                    D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE,
                )
                .map_err(|e| format!("CreateCompatibleRenderTarget failed: {:?}", e))?;

            shadow_render_target.BeginDraw();
            shadow_render_target.Clear(None);

            let shadow_brush = shadow_render_target
                .CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.15,
                    },
                    None,
                )
                .map_err(|e| format!("CreateSolidColorBrush shadow failed: {:?}", e))?;

            let shadow_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: blur_radius,
                    top: blur_radius,
                    right: width + blur_radius,
                    bottom: height + blur_radius,
                },
                radiusX: corner_radius,
                radiusY: corner_radius,
            };
            shadow_render_target.FillRoundedRectangle(&shadow_rect, &shadow_brush);
            shadow_render_target
                .EndDraw(None, None)
                .map_err(|e| format!("shadow EndDraw failed: {:?}", e))?;

            let shadow_bitmap = shadow_render_target
                .GetBitmap()
                .map_err(|e| format!("GetBitmap failed: {:?}", e))?;

            let gaussian_blur_effect = d2d
                .CreateEffect(&CLSID_D2D1GaussianBlur)
                .map_err(|e| format!("CreateEffect failed: {:?}", e))?;
            gaussian_blur_effect.SetInput(0, &shadow_bitmap, false);
            let blur_output = gaussian_blur_effect
                .GetOutput()
                .map_err(|e| format!("GetOutput failed: {:?}", e))?;

            d2d.DrawImage(
                &blur_output,
                Some(&Vector2 { X: 0.0, Y: 0.0 }),
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );

            Ok(())
    }
}
