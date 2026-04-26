use gpui::{
    ClipboardItem, Context, FocusHandle, FontFallbacks, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Render, ScrollDelta, ScrollWheelEvent, Window, canvas,
    div, fill, font, point, prelude::*, px, rgb, rgba, size,
};

use crate::cli::Theme;
use crate::input::selection_contains_cell;
use crate::render::{LINE_HEIGHT_SCALE, TEXT_PADDING_X, TEXT_PADDING_Y, measure_cell_width};
use crate::terminal::{CellSnapshot, SelectionPoint};

#[derive(Clone)]
pub(crate) struct SnapshotTabData {
    pub(crate) title: String,
    pub(crate) lines: Vec<Vec<CellSnapshot>>,
    pub(crate) cols: usize,
    pub(crate) font_family: String,
    pub(crate) font_fallbacks: Option<FontFallbacks>,
    pub(crate) font_size: Pixels,
    pub(crate) theme: Theme,
}

#[derive(Clone, Copy)]
struct SnapshotPalette {
    app_bg: gpui::Hsla,
    terminal_bg: gpui::Hsla,
    selection_bg: gpui::Hsla,
}

fn palette_for(theme: Theme) -> SnapshotPalette {
    match theme {
        Theme::Default => SnapshotPalette {
            app_bg: rgb(0x0f1115).into(),
            terminal_bg: rgb(0x000000).into(),
            selection_bg: rgba(0x4b93ffaa).into(),
        },
        Theme::EyeCare => SnapshotPalette {
            app_bg: rgb(0x151b17).into(),
            terminal_bg: rgb(0x1b241e).into(),
            selection_bg: rgba(0x7ca67899).into(),
        },
    }
}

pub(crate) struct SnapshotTab {
    pub(crate) focus_handle: FocusHandle,
    title: String,
    lines: Vec<Vec<CellSnapshot>>,
    cols: usize,
    top_line: usize,
    font_family: String,
    font_fallbacks: Option<FontFallbacks>,
    font_size: Pixels,
    theme: Theme,
    selection_anchor: Option<SelectionPoint>,
    selection_focus: Option<SelectionPoint>,
}

impl SnapshotTab {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>, data: SnapshotTabData) -> Self {
        let focus_handle = cx.focus_handle();
        let visible_rows = estimate_visible_rows(window.viewport_size().height, data.font_size);
        let max_top = data.lines.len().saturating_sub(visible_rows);
        Self {
            focus_handle,
            title: data.title,
            lines: data.lines,
            cols: data.cols.max(1),
            top_line: max_top,
            font_family: data.font_family,
            font_fallbacks: data.font_fallbacks,
            font_size: data.font_size,
            theme: data.theme,
            selection_anchor: None,
            selection_focus: None,
        }
    }

    pub(crate) fn title(&self) -> String {
        self.title.clone()
    }

    fn line_height(&self) -> Pixels {
        (self.font_size * LINE_HEIGHT_SCALE).max(self.font_size + px(2.0))
    }

    fn visible_rows_for_height(&self, height: Pixels) -> usize {
        let line_height = self.line_height().max(px(1.0));
        let usable = (height - (TEXT_PADDING_Y * 2.0)).max(line_height);
        ((usable / line_height).floor() as usize).max(1)
    }

    fn max_top_line_for_height(&self, height: Pixels) -> usize {
        self.lines
            .len()
            .saturating_sub(self.visible_rows_for_height(height))
    }

    fn normalize_point(&self, row: usize, col: usize) -> SelectionPoint {
        let safe_row = row.min(self.lines.len().saturating_sub(1));
        let row_cells = self.lines.get(safe_row);
        let safe_col = row_cells
            .map(|cells| {
                if cells.is_empty() {
                    0
                } else {
                    normalize_selection_col(cells, col.min(self.cols.saturating_sub(1)))
                }
            })
            .unwrap_or(0);
        SelectionPoint {
            row: safe_row,
            col: safe_col,
        }
    }

    fn mouse_grid_point(&self, position: gpui::Point<Pixels>, window: &mut Window) -> SelectionPoint {
        let cell_width = measure_cell_width(
            window,
            &self.font_family,
            self.font_fallbacks.as_ref(),
            self.font_size,
        )
        .max(px(1.0));
        let line_height = self.line_height().max(px(1.0));
        let origin = point(TEXT_PADDING_X, TEXT_PADDING_Y);
        let raw_col =
            (f32::from(position.x - origin.x) / f32::from(cell_width)).floor() as i32;
        let raw_row =
            (f32::from(position.y - origin.y) / f32::from(line_height)).floor() as i32;
        let max_row = (self.visible_rows_for_height(window.viewport_size().height) as i32).max(1) - 1;
        let visible_row = raw_row.clamp(0, max_row) as usize;
        let max_col = (self.cols as i32).max(1) - 1;
        let col = raw_col.clamp(0, max_col) as usize;
        let row = self
            .top_line
            .saturating_add(visible_row)
            .min(self.lines.len().saturating_sub(1));
        self.normalize_point(row, col)
    }

    fn selection_bounds(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        let anchor = self.selection_anchor?;
        let focus = self.selection_focus?;
        Some(normalize_selection_bounds(anchor, focus))
    }

    fn selection_text(&self) -> Option<String> {
        let (start, end) = self.selection_bounds()?;
        let text = extract_selection_text(&self.lines, start, end);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn copy_selection_to_clipboard(&self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.selection_text() else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    fn copy_all_to_clipboard(&self, cx: &mut Context<Self>) {
        let text = self
            .lines
            .iter()
            .map(|row| {
                let mut line = row_text_without_wide_spacers(row);
                let trimmed_len = line.trim_end().len();
                line.truncate(trimmed_len);
                line
            })
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    pub(crate) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_copy = cfg!(target_os = "macos")
            && event.keystroke.modifiers.platform
            && !event.keystroke.modifiers.control
            && !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.function
            && event.keystroke.key.eq_ignore_ascii_case("c");
        if is_copy {
            if !self.copy_selection_to_clipboard(cx) {
                self.copy_all_to_clipboard(cx);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }

        let is_select_all = cfg!(target_os = "macos")
            && event.keystroke.modifiers.platform
            && !event.keystroke.modifiers.control
            && !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.shift
            && !event.keystroke.modifiers.function
            && event.keystroke.key.eq_ignore_ascii_case("a");
        if is_select_all && !self.lines.is_empty() {
            let last_row = self.lines.len() - 1;
            let last_col = self.cols.saturating_sub(1);
            self.selection_anchor = Some(SelectionPoint { row: 0, col: 0 });
            self.selection_focus = Some(self.normalize_point(last_row, last_col));
            cx.stop_propagation();
            cx.notify();
            return;
        }

        let is_escape = event.keystroke.key.eq_ignore_ascii_case("escape");
        if is_escape && (self.selection_anchor.is_some() || self.selection_focus.is_some()) {
            self.selection_anchor = None;
            self.selection_focus = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }

        let _ = window;
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left || self.lines.is_empty() {
            return;
        }

        let point = self.mouse_grid_point(event.position, window);
        self.selection_anchor = Some(point);
        self.selection_focus = Some(point);
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button != Some(MouseButton::Left) || self.selection_anchor.is_none() {
            return;
        }

        let point = self.mouse_grid_point(event.position, window);
        if self.selection_focus != Some(point) {
            self.selection_focus = Some(point);
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub(crate) fn on_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left || self.selection_anchor.is_none() {
            return;
        }
        self.selection_focus = Some(self.mouse_grid_point(event.position, window));
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line_height = self.line_height().max(px(1.0));
        let y_steps = match event.delta {
            ScrollDelta::Pixels(pixels) => {
                (f32::from(pixels.y) / f32::from(line_height)).round() as i32
            }
            ScrollDelta::Lines(lines) => lines.y.round() as i32,
        };
        if y_steps == 0 {
            return;
        }

        let max_top = self.max_top_line_for_height(window.viewport_size().height) as i32;
        let next = (self.top_line as i32 + y_steps).clamp(0, max_top) as usize;
        if next != self.top_line {
            self.top_line = next;
            cx.notify();
        }
        cx.stop_propagation();
    }
}

impl Render for SnapshotTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot_lines = self.lines.clone();
        let cols = self.cols;
        let top_line = self.top_line;
        let line_height = self.line_height();
        let font_size = self.font_size;
        let font_family = self.font_family.clone();
        let font_fallbacks = self.font_fallbacks.clone();
        let palette = palette_for(self.theme);
        let selection = self.selection_bounds();

        div()
            .id("snapshot-tab")
            .size_full()
            .bg(palette.app_bg)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(
                canvas(
                    move |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.paint_quad(fill(bounds, palette.terminal_bg));

                        let mono = build_terminal_font(&font_family, font_fallbacks.as_ref());
                        let run_template = gpui::TextRun {
                            len: 0,
                            font: mono.clone(),
                            color: rgb(0xd7dae0).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let font_id = window.text_system().resolve_font(&mono);
                        let cell_width = window
                            .text_system()
                            .advance(font_id, font_size, 'M')
                            .map(|advance| advance.width)
                            .unwrap_or(px(8.0))
                            .max(px(1.0));

                        let origin = bounds.origin + point(TEXT_PADDING_X, TEXT_PADDING_Y);
                        let visible_rows = {
                            let usable = (bounds.size.height - (TEXT_PADDING_Y * 2.0)).max(line_height);
                            ((usable / line_height).floor() as usize).max(1)
                        };
                        let max_top = snapshot_lines.len().saturating_sub(visible_rows);
                        let start_line = top_line.min(max_top);

                        for row_offset in 0..visible_rows {
                            let line_index = start_line.saturating_add(row_offset);
                            let y = origin.y + row_offset as f32 * line_height;
                            let mut covered_until_col = 0usize;
                            let mut extra_visual_cols = 0f32;

                            let row = snapshot_lines.get(line_index);
                            for col_index in 0..cols {
                                let cell = row
                                    .and_then(|cells| cells.get(col_index))
                                    .cloned()
                                    .unwrap_or_default();
                                let is_spacer_col = col_index < covered_until_col;
                                let x = origin.x + (col_index as f32 + extra_visual_cols) * cell_width;
                                let cell_origin = point(x, y);
                                let cell_width_px = cell_width * cell.width_cols as f32;

                                if !is_spacer_col && let Some(bg) = cell.bg {
                                    window.paint_quad(fill(
                                        gpui::Bounds::new(cell_origin, size(cell_width_px, line_height)),
                                        bg,
                                    ));
                                }
                                if !is_spacer_col
                                    && selection.is_some_and(|(start, end)| {
                                        selection_contains_cell(start, end, line_index, col_index)
                                    })
                                {
                                    window.paint_quad(fill(
                                        gpui::Bounds::new(cell_origin, size(cell_width_px, line_height)),
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
                                        font_size,
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
                                    covered_until_col = col_index.saturating_add(cell_advance_cols(&cell));
                                    if cell.expands_layout && cell.width_cols > 1 {
                                        extra_visual_cols += f32::from(cell.width_cols - 1);
                                    }
                                }
                            }
                        }
                    },
                )
                .size_full(),
            )
    }
}

fn estimate_visible_rows(height: Pixels, font_size: Pixels) -> usize {
    let line_height = (font_size * LINE_HEIGHT_SCALE).max(font_size + px(2.0));
    let usable = (height - (TEXT_PADDING_Y * 2.0)).max(line_height);
    ((usable / line_height).floor() as usize).max(1)
}

fn build_terminal_font(font_family: &str, font_fallbacks: Option<&FontFallbacks>) -> gpui::Font {
    let mut mono = font(font_family.to_string());
    mono.fallbacks = font_fallbacks.cloned();
    mono
}

fn cell_advance_cols(cell: &CellSnapshot) -> usize {
    if cell.spans_next_col {
        usize::from(cell.width_cols.max(1))
    } else {
        1
    }
}

fn normalize_selection_bounds(
    start: SelectionPoint,
    end: SelectionPoint,
) -> (SelectionPoint, SelectionPoint) {
    if (start.row, start.col) <= (end.row, end.col) {
        (start, end)
    } else {
        (end, start)
    }
}

fn normalize_selection_col(cells: &[CellSnapshot], col: usize) -> usize {
    if cells.is_empty() {
        return 0;
    }
    let mut normalized = col.min(cells.len().saturating_sub(1));
    while normalized > 0 {
        let prev = normalized - 1;
        let prev_span = cell_advance_cols(&cells[prev]);
        if prev_span > 1 && prev.saturating_add(prev_span) > normalized {
            normalized = prev;
            continue;
        }
        break;
    }
    normalized
}

fn extract_selection_text(
    lines: &[Vec<CellSnapshot>],
    start: SelectionPoint,
    end: SelectionPoint,
) -> String {
    let mut out = Vec::new();
    for row in start.row..=end.row {
        let Some(cells) = lines.get(row) else {
            break;
        };
        if cells.is_empty() {
            out.push(String::new());
            continue;
        }
        let line_start = if row == start.row {
            normalize_selection_col(cells, start.col)
        } else {
            0
        };
        let line_end = if row == end.row {
            normalize_selection_col(cells, end.col)
        } else {
            cells.len().saturating_sub(1)
        };
        if line_start >= cells.len() {
            out.push(String::new());
            continue;
        }
        let clamped_end = line_end.min(cells.len().saturating_sub(1));
        if line_start > clamped_end {
            out.push(String::new());
            continue;
        }
        let mut text = String::new();
        let mut col = line_start;
        while col <= clamped_end {
            let cell = &cells[col];
            text.push(cell.ch);
            col = col.saturating_add(cell_advance_cols(cell));
        }
        let trimmed_len = text.trim_end().len();
        text.truncate(trimmed_len);
        out.push(text);
    }
    out.join("\n")
}

fn row_text_without_wide_spacers(cells: &[CellSnapshot]) -> String {
    let mut text = String::new();
    let mut col = 0usize;
    while col < cells.len() {
        let cell = &cells[col];
        text.push(cell.ch);
        col = col.saturating_add(cell_advance_cols(cell));
    }
    text
}
