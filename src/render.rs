use gpui::{
    Bounds, Context, Font, FontFallbacks, Hsla, MouseButton, Pixels, Render, Window,
    WindowControlArea, canvas, div, fill, font, point, prelude::*, px, rgb, rgba, size,
};
use alacritty_terminal::vte::ansi::CursorShape;

use crate::cli::Theme;
use crate::input::selection_contains_cell;
use crate::{AgentTerminal, AgentTerminalInputHandler};

pub(crate) const LINE_HEIGHT_SCALE: f32 = 18.0 / 14.0;
pub(crate) const TEXT_PADDING_X: Pixels = px(12.0);
pub(crate) const TEXT_PADDING_Y: Pixels = px(12.0);
pub(crate) const CUSTOM_TITLE_BAR_HEIGHT: Pixels = px(32.0);
pub(crate) const STATUS_BAR_ESTIMATED_HEIGHT: Pixels = px(42.0);

pub(crate) fn measure_cell_width(
    window: &mut Window,
    font_family: &str,
    font_fallbacks: Option<&FontFallbacks>,
    font_size: Pixels,
) -> Pixels {
    let mono = build_terminal_font(font_family, font_fallbacks);
    let font_id = window.text_system().resolve_font(&mono);
    window
        .text_system()
        .advance(font_id, font_size, 'M')
        .map(|advance| advance.width)
        .unwrap_or(px(8.0))
}

fn build_terminal_font(font_family: &str, font_fallbacks: Option<&FontFallbacks>) -> Font {
    let mut mono = font(font_family.to_string());
    mono.fallbacks = font_fallbacks.cloned();
    mono
}

fn cell_advance_cols(cell: &crate::terminal::CellSnapshot) -> usize {
    if cell.spans_next_col {
        usize::from(cell.width_cols.max(1))
    } else {
        1
    }
}

fn visual_extra_cols_before(row: &[crate::terminal::CellSnapshot], logical_col: usize) -> f32 {
    let mut covered_until_col = 0usize;
    let mut extra_cols = 0f32;
    for (col_index, cell) in row.iter().enumerate() {
        if col_index >= logical_col {
            break;
        }
        if col_index < covered_until_col {
            continue;
        }
        if cell.expands_layout && cell.width_cols > 1 {
            extra_cols += f32::from(cell.width_cols - 1);
        }
        covered_until_col = col_index.saturating_add(cell_advance_cols(cell));
    }
    extra_cols
}

pub(crate) fn line_height_for(font_size: Pixels) -> Pixels {
    (font_size * LINE_HEIGHT_SCALE).max(font_size + px(2.0))
}

#[derive(Clone, Copy)]
struct RenderPalette {
    app_bg: Hsla,
    terminal_bg: Hsla,
    title_bg: Hsla,
    title_fg: Hsla,
    selection_bg: Hsla,
    cursor_bg: Hsla,
}

fn palette_for(theme: Theme) -> RenderPalette {
    match theme {
        Theme::Default => RenderPalette {
            app_bg: rgb(0x0f1115).into(),
            terminal_bg: rgb(0x000000).into(),
            title_bg: rgb(0x171a21).into(),
            title_fg: rgb(0xa9b1c6).into(),
            selection_bg: rgba(0x4b93ffaa).into(),
            cursor_bg: rgba(0xffea00a6).into(),
        },
        Theme::EyeCare => RenderPalette {
            app_bg: rgb(0x151b17).into(),
            terminal_bg: rgb(0x1b241e).into(),
            title_bg: rgb(0x222d26).into(),
            title_fg: rgb(0xc0cbbd).into(),
            selection_bg: rgba(0x7ca67899).into(),
            cursor_bg: rgba(0xffea00a6).into(),
        },
    }
}

impl Render for AgentTerminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_title(&self.window_title());
        let model_line = self.input_line.clone();
        let model_cursor_utf16 = self.input_cursor_utf16;
        let allow_ax_override = self.allow_ax_override();
        let sync_result = crate::macos_ax::sync_native_ax_input_view(
            window,
            &model_line,
            model_cursor_utf16,
            &self.last_ax_published_line,
            self.last_ax_published_cursor_utf16,
            allow_ax_override,
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
        let cursor_shape = self.cursor_shape;
        let (cursor_visual_row, cursor_visual_col, cursor_sliding) = self.cursor_visual_state();
        if cursor_sliding {
            cx.on_next_frame(window, |_, _, cx| {
                cx.notify();
            });
        }
        let focused = self.focus_handle.is_focused(window);
        let focus_handle = self.focus_handle.clone();
        let entity = cx.entity();
        let status = self.debug.status_summary();
        let shell = self.shell.clone();
        let font_family = self.font_family.clone();
        let font_fallbacks = self.font_fallbacks.clone();
        let font_size = self.font_size;
        let line_height = self.line_height();
        let note = self.debug.note();
        let selection = self.selection_bounds();
        let palette = palette_for(self.theme);
        let terminal_title = self
            .terminal_title
            .lock()
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| shell.clone());
        let canvas_font_family = font_family.clone();
        let canvas_font_fallbacks = font_fallbacks.clone();

        let status_line = if let Some(note) = note {
            format!("agent terminal | {} | {} | note: {}", shell, status, note)
        } else {
            format!("agent terminal | {} | {}", shell, status)
        };

        let terminal_surface = div()
            .size_full()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down_left))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down_middle))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down_right))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up_left))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up_middle))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up_right))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(
                canvas(
                    move |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &focus_handle,
                            AgentTerminalInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                        window.paint_quad(fill(bounds, palette.terminal_bg));

                        let mono =
                            build_terminal_font(&canvas_font_family, canvas_font_fallbacks.as_ref());
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
                            let mut covered_until_col = 0usize;
                            let mut extra_visual_cols = 0f32;

                            for (col_index, cell) in row.iter().enumerate() {
                                let is_spacer_col = col_index < covered_until_col;
                                let x = origin.x + (col_index as f32 + extra_visual_cols) * cell_width;
                                let cell_origin = point(x, y);
                                let cell_width_px =
                                    cell_width.max(px(2.0)) * cell.width_cols as f32;

                                if !is_spacer_col && let Some(bg) = cell.bg {
                                    window.paint_quad(fill(
                                        Bounds::new(cell_origin, size(cell_width_px, line_height)),
                                        bg,
                                    ));
                                }
                                if !is_spacer_col && selection.is_some_and(|(start, end)| {
                                    selection_contains_cell(start, end, row_index, col_index)
                                }) {
                                    window.paint_quad(fill(
                                        Bounds::new(cell_origin, size(cell_width_px, line_height)),
                                        palette.selection_bg,
                                    ));
                                }

                                if !is_spacer_col && cell.ch != ' ' {
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

                                if !is_spacer_col {
                                    covered_until_col =
                                        col_index.saturating_add(cell_advance_cols(cell));
                                    if cell.expands_layout && cell.width_cols > 1 {
                                        extra_visual_cols += f32::from(cell.width_cols - 1);
                                    }
                                }
                            }
                        }

                        if focused && snapshot.cursor_visible {
                            let cursor_logical_col_floor = cursor_visual_col.max(0.0).floor() as usize;
                            let cursor_extra_cols = snapshot
                                .cells
                                .get(cursor_visual_row)
                                .map(|row| visual_extra_cols_before(row, cursor_logical_col_floor))
                                .unwrap_or(0.0);
                            let cursor_origin = point(
                                origin.x + (cursor_visual_col + cursor_extra_cols) * cell_width,
                                origin.y + cursor_visual_row as f32 * line_height,
                            );
                            let cell_width_px = cell_width.max(px(2.0));
                            match cursor_shape {
                                CursorShape::Beam => {
                                    let beam_width = (cell_width_px * 0.14).max(px(2.0));
                                    window.paint_quad(fill(
                                        Bounds::new(cursor_origin, size(beam_width, line_height)),
                                        palette.cursor_bg,
                                    ));
                                }
                                CursorShape::Underline => {
                                    let underline_height = (line_height * 0.12).max(px(2.0));
                                    let underline_origin = point(
                                        cursor_origin.x,
                                        cursor_origin.y + line_height - underline_height,
                                    );
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            underline_origin,
                                            size(cell_width_px, underline_height),
                                        ),
                                        palette.cursor_bg,
                                    ));
                                }
                                CursorShape::HollowBlock => {
                                    let border_x = (cell_width_px * 0.08).max(px(1.0));
                                    let border_y = (line_height * 0.08).max(px(1.0));

                                    window.paint_quad(fill(
                                        Bounds::new(cursor_origin, size(cell_width_px, border_y)),
                                        palette.cursor_bg,
                                    ));
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(cursor_origin.x, cursor_origin.y + line_height - border_y),
                                            size(cell_width_px, border_y),
                                        ),
                                        palette.cursor_bg,
                                    ));
                                    window.paint_quad(fill(
                                        Bounds::new(cursor_origin, size(border_x, line_height)),
                                        palette.cursor_bg,
                                    ));
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(cursor_origin.x + cell_width_px - border_x, cursor_origin.y),
                                            size(border_x, line_height),
                                        ),
                                        palette.cursor_bg,
                                    ));
                                }
                                CursorShape::Hidden => {}
                                CursorShape::Block => {
                                    window.paint_quad(fill(
                                        Bounds::new(cursor_origin, size(cell_width_px, line_height)),
                                        palette.cursor_bg,
                                    ));
                                }
                            }
                        }
                    },
                )
                .size_full(),
            );

        let title_bar = div()
            .w_full()
            .h(CUSTOM_TITLE_BAR_HEIGHT)
            .px_3()
            .bg(palette.title_bg)
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .items_center()
            .justify_between()
            .child(div().w(px(52.0)))
            .child(
                div()
                    .flex_1()
                    .text_color(palette.title_fg)
                    .font_family(font_family.clone())
                    .text_center()
                    .child(terminal_title),
            )
            .child(div().w(px(52.0)));

        let root = div()
            .id("agent-terminal")
            .size_full()
            .bg(palette.app_bg)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down));

        let root = if self.show_title_bar {
            root.child(title_bar)
        } else {
            root
        };

        let root = if self.show_status_bar {
            root.child(
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .bg(palette.title_bg)
                    .text_color(palette.title_fg)
                    .font_family(font_family.clone())
                    .child(status_line),
            )
        } else {
            root
        };

        root.child(terminal_surface)
    }
}

#[cfg(test)]
mod tests {
    fn count_occurrences(haystack: &str, needle: &str) -> usize {
        haystack.match_indices(needle).count()
    }

    #[test]
    fn render_keeps_keydown_binding_on_root() {
        let source = include_str!("render.rs");
        let keydown_binding = [".on_key_down(", "cx.listener(Self::on_key_down)", ")"].concat();
        assert!(
            source.contains(&keydown_binding),
            "render root must bind keydown so Backspace/Ctrl combos reach terminal"
        );
    }

    #[test]
    fn render_binds_keydown_exactly_once() {
        let source = include_str!("render.rs");
        let keydown_binding = [".on_key_down(", "cx.listener(Self::on_key_down)", ")"].concat();
        assert_eq!(
            count_occurrences(source, &keydown_binding),
            1,
            "keydown listener should be bound once to avoid duplicate key handling"
        );
    }
}
