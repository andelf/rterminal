use gpui::{
    Bounds, Context, MouseButton, Pixels, Render, Window, WindowControlArea,
    black, canvas, div, fill, font, point, prelude::*, px, rgb, rgba, size,
};

use crate::{AgentTerminal, AgentTerminalInputHandler};

pub(crate) const LINE_HEIGHT_SCALE: f32 = 18.0 / 14.0;
pub(crate) const TEXT_PADDING_X: Pixels = px(12.0);
pub(crate) const TEXT_PADDING_Y: Pixels = px(12.0);
pub(crate) const CUSTOM_TITLE_BAR_HEIGHT: Pixels = px(32.0);
pub(crate) const STATUS_BAR_ESTIMATED_HEIGHT: Pixels = px(42.0);

pub(crate) fn measure_cell_width(window: &mut Window, font_family: &str, font_size: Pixels) -> Pixels {
    let mono = font(font_family.to_string());
    let font_id = window.text_system().resolve_font(&mono);
    window
        .text_system()
        .advance(font_id, font_size, 'M')
        .map(|advance| advance.width)
        .unwrap_or(px(8.0))
}

pub(crate) fn line_height_for(font_size: Pixels) -> Pixels {
    (font_size * LINE_HEIGHT_SCALE).max(font_size + px(2.0))
}

impl Render for AgentTerminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_title(&self.window_title());
        let model_line = self.input_line.clone();
        let model_cursor_utf16 = self.input_cursor_utf16;
        let sync_result = crate::macos_ax::sync_native_ax_input_view(
            window,
            &model_line,
            model_cursor_utf16,
            &self.last_ax_published_line,
            self.last_ax_published_cursor_utf16,
        );
        if let Some(state) = sync_result.override_from_ax
            && self.apply_external_ax_input_state(state)
        {
            cx.notify();
        }
        if sync_result.published_model {
            self.last_ax_published_line = model_line;
            self.last_ax_published_cursor_utf16 = model_cursor_utf16;
        }

        let snapshot = self.snapshot.clone();
        let focused = self.focus_handle.is_focused(window);
        let focus_handle = self.focus_handle.clone();
        let entity = cx.entity();
        let status = self.debug.status_summary();
        let shell = self.shell.clone();
        let font_family = self.font_family.clone();
        let font_size = self.font_size;
        let line_height = self.line_height();
        let note = self.debug.note();
        let terminal_title = self
            .terminal_title
            .lock()
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| shell.clone());

        let status_line = if let Some(note) = note {
            format!("agent terminal | {} | {} | note: {}", shell, status, note)
        } else {
            format!("agent terminal | {} | {}", shell, status)
        };

        let root = div()
            .id("agent-terminal")
            .size_full()
            .bg(rgb(0x0f1115))
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                div()
                    .w_full()
                    .h(CUSTOM_TITLE_BAR_HEIGHT)
                    .px_3()
                    .bg(rgb(0x171a21))
                    .window_control_area(WindowControlArea::Drag)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().w(px(52.0)))
                    .child(
                        div()
                            .flex_1()
                            .text_color(rgb(0xa9b1c6))
                            .font_family(font_family.clone())
                            .text_center()
                            .child(terminal_title),
                    )
                    .child(div().w(px(52.0))),
            );

        let root = if self.show_status_bar {
            root.child(
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .bg(rgb(0x171a21))
                    .text_color(rgb(0xa9b1c6))
                    .font_family(font_family.clone())
                    .child(status_line),
            )
        } else {
            root
        };

        root.child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, cx| {
                    window.handle_input(
                        &focus_handle,
                        AgentTerminalInputHandler::new(bounds, entity.clone()),
                        cx,
                    );
                    window.paint_quad(fill(bounds, black()));

                    let mono = font(font_family.clone());
                    let run_template = gpui::TextRun {
                        len: 0,
                        font: mono.clone(),
                        color: rgb(0xd7dae0).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };

                    let font_pixels = font_size;
                    let font_id = window.text_system().resolve_font(&mono);
                    let cell_width = window
                        .text_system()
                        .advance(font_id, font_pixels, 'M')
                        .map(|advance| advance.width)
                        .unwrap_or(px(8.0));

                    let origin = bounds.origin + point(TEXT_PADDING_X, TEXT_PADDING_Y);
                    for (row_index, row) in snapshot.cells.iter().enumerate() {
                        let y = origin.y + row_index as f32 * line_height;

                        for (col_index, cell) in row.iter().enumerate() {
                            let x = origin.x + col_index as f32 * cell_width;
                            let cell_origin = point(x, y);
                            let cell_width_px = cell_width.max(px(2.0)) * cell.width_cols as f32;

                            if let Some(bg) = cell.bg {
                                window.paint_quad(fill(
                                    Bounds::new(cell_origin, size(cell_width_px, line_height)),
                                    bg,
                                ));
                            }

                            if cell.ch != ' ' {
                                let run = gpui::TextRun {
                                    len: cell.ch.len_utf8(),
                                    color: cell.fg,
                                    ..run_template.clone()
                                };
                                let shaped = window.text_system().shape_line(
                                    cell.ch.to_string().into(),
                                    font_pixels,
                                    &[run],
                                    Some(cell_width_px),
                                );
                                let _ = shaped.paint(
                                    cell_origin,
                                    line_height,
                                    gpui::TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                            }
                        }
                    }

                    if focused && snapshot.cursor_visible {
                        let cursor_origin = point(
                            origin.x + snapshot.cursor_col as f32 * cell_width,
                            origin.y + snapshot.cursor_row as f32 * line_height,
                        );
                        window.paint_quad(fill(
                            Bounds::new(cursor_origin, size(cell_width.max(px(2.0)), line_height)),
                            rgba(0x3b82f659),
                        ));
                    }
                },
            )
            .size_full(),
        )
    }
}
