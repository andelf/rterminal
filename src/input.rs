use std::ops::Range;
use std::time::{Duration, Instant};

use alacritty_terminal::term::TermMode;
use gpui::{
    App, Bounds, ClipboardItem, Context, EntityInputHandler, InputHandler, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PromptLevel, ScrollDelta,
    ScrollWheelEvent, UTF16Selection, Window, point, px, size,
};
use serde_json::json;

use crate::AgentTerminal;
use crate::keyboard::{
    encode_keystroke, is_paste_shortcut, is_select_all_shortcut, is_zoom_in_shortcut,
    is_zoom_out_shortcut, should_defer_to_text_input,
};
use crate::macos_ax::NativeAxInputState;
use crate::render::{
    CUSTOM_TITLE_BAR_HEIGHT, STATUS_BAR_HEIGHT, TEXT_PADDING_X,
    measure_cell_width, terminal_content_padding_y,
};
use crate::terminal::{CellSnapshot, ScreenSnapshot, SelectionPoint};
use crate::text_utils::{
    delete_next_word_utf16, delete_previous_word_utf16, delete_to_end_utf16,
    summarize_text_for_trace, utf16_substring, utf16_to_byte_index,
};

const FONT_SIZE_STEP: f32 = 1.0;
const PASTE_GUARD_MIN_LINES: usize = 4;
const PASTE_GUARD_MIN_CHARS: usize = 120;
const PASTE_GUARD_NON_ASCII_RATIO: f32 = 0.35;
const AX_OVERRIDE_GUARD_WINDOW: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug)]
struct PasteRisk {
    line_count: usize,
    char_count: usize,
    non_ascii_ratio: f32,
}

pub(crate) struct AgentTerminalInputHandler {
    terminal: gpui::Entity<AgentTerminal>,
    element_bounds: Bounds<Pixels>,
}

macro_rules! define_mouse_forwarders {
    ($(($down:ident, $up:ident)),+ $(,)?) => {
        $(
            pub(crate) fn $down(
                &mut self,
                event: &MouseDownEvent,
                window: &mut Window,
                cx: &mut Context<Self>,
            ) {
                self.on_mouse_down(event, window, cx);
            }

            pub(crate) fn $up(
                &mut self,
                event: &MouseUpEvent,
                window: &mut Window,
                cx: &mut Context<Self>,
            ) {
                self.on_mouse_up(event, window, cx);
            }
        )+
    };
}

impl AgentTerminalInputHandler {
    pub(crate) fn new(
        element_bounds: Bounds<Pixels>,
        terminal: gpui::Entity<AgentTerminal>,
    ) -> Self {
        Self {
            terminal,
            element_bounds,
        }
    }
}

impl InputHandler for AgentTerminalInputHandler {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.terminal.update(cx, |terminal, cx| {
            terminal.selected_text_range(ignore_disabled_input, window, cx)
        })
    }

    fn marked_text_range(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<std::ops::Range<usize>> {
        self.terminal
            .update(cx, |terminal, cx| terminal.marked_text_range(window, cx))
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        self.terminal.update(cx, |terminal, cx| {
            terminal.text_for_range(range_utf16, adjusted_range, window, cx)
        })
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.terminal.update(cx, |terminal, cx| {
            terminal.replace_text_in_range(replacement_range, text, window, cx)
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.terminal.update(cx, |terminal, cx| {
            terminal.replace_and_mark_text_in_range(
                range_utf16,
                new_text,
                new_selected_range,
                window,
                cx,
            )
        });
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.terminal
            .update(cx, |terminal, cx| terminal.unmark_text(window, cx));
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.terminal.update(cx, |terminal, cx| {
            terminal.bounds_for_range(range_utf16, self.element_bounds, window, cx)
        })
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.terminal.update(cx, |terminal, cx| {
            terminal.character_index_for_point(point, window, cx)
        })
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}

impl AgentTerminal {
    pub(crate) fn write_text_input(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.trace_input(format!(
            "text-input len={} value={}",
            text.len(),
            summarize_text_for_trace(text)
        ));
        self.log_input_event(
            "write_text_input",
            json!({
                "text": self.input_log_text_value(text),
                "len": text.len(),
            }),
        );
        self.write_bytes(text.as_bytes());
    }

    pub(crate) fn write_paste_input(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        // Convert line endings to \r before writing to PTY. Terminal input uses
        // \r for Enter; without this, \n bytes pass through raw-mode PTYs
        // unchanged, breaking multi-line paste in programs like tmux and vi.
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bracketed = self.term.mode().contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            // Strip ESC bytes so pasted content cannot inject escape sequences
            // that would break out of the bracketed paste or confuse the shell.
            let sanitized = text.replace('\x1b', "");
            self.write_bytes(b"\x1b[200~");
            self.write_text_input(&sanitized);
            self.write_bytes(b"\x1b[201~");
        } else {
            self.write_text_input(&text);
        }
    }

    pub(crate) fn input_line_len_utf16(&self) -> usize {
        self.input_line.encode_utf16().count()
    }

    pub(crate) fn clamp_input_cursor(&mut self) {
        self.input_cursor_utf16 = self.input_cursor_utf16.min(self.input_line_len_utf16());
    }

    pub(crate) fn insert_input_text_at_cursor(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        self.clamp_input_cursor();
        let cursor_byte = utf16_to_byte_index(&self.input_line, self.input_cursor_utf16);
        self.input_line.insert_str(cursor_byte, text);
        self.input_cursor_utf16 += text.encode_utf16().count();
    }

    pub(crate) fn backspace_input_char(&mut self) {
        self.clamp_input_cursor();
        if self.input_cursor_utf16 == 0 {
            return;
        }

        let cursor_byte = utf16_to_byte_index(&self.input_line, self.input_cursor_utf16);
        let Some((start_byte, removed)) = self.input_line[..cursor_byte].char_indices().last()
        else {
            return;
        };
        self.input_line.replace_range(start_byte..cursor_byte, "");
        self.input_cursor_utf16 = self.input_cursor_utf16.saturating_sub(removed.len_utf16());
    }

    pub(crate) fn delete_input_char_at_cursor(&mut self) {
        self.clamp_input_cursor();
        let cursor_byte = utf16_to_byte_index(&self.input_line, self.input_cursor_utf16);
        let Some(ch) = self.input_line[cursor_byte..].chars().next() else {
            return;
        };
        let end_byte = cursor_byte + ch.len_utf8();
        self.input_line.replace_range(cursor_byte..end_byte, "");
    }

    pub(crate) fn move_input_cursor_left(&mut self) {
        self.clamp_input_cursor();
        if self.input_cursor_utf16 == 0 {
            return;
        }

        let cursor_byte = utf16_to_byte_index(&self.input_line, self.input_cursor_utf16);
        if let Some((_, ch)) = self.input_line[..cursor_byte].char_indices().last() {
            self.input_cursor_utf16 = self.input_cursor_utf16.saturating_sub(ch.len_utf16());
        } else {
            self.input_cursor_utf16 = 0;
        }
    }

    pub(crate) fn move_input_cursor_right(&mut self) {
        self.clamp_input_cursor();
        let len = self.input_line_len_utf16();
        if self.input_cursor_utf16 >= len {
            return;
        }

        let cursor_byte = utf16_to_byte_index(&self.input_line, self.input_cursor_utf16);
        if let Some(ch) = self.input_line[cursor_byte..].chars().next() {
            self.input_cursor_utf16 += ch.len_utf16();
        } else {
            self.input_cursor_utf16 = len;
        }
    }

    pub(crate) fn clear_input_line(&mut self) {
        self.input_line.clear();
        self.input_cursor_utf16 = 0;
    }

    pub(crate) fn delete_previous_input_word(&mut self) {
        delete_previous_word_utf16(&mut self.input_line, &mut self.input_cursor_utf16);
        self.clamp_input_cursor();
    }

    pub(crate) fn delete_next_input_word(&mut self) {
        delete_next_word_utf16(&mut self.input_line, &mut self.input_cursor_utf16);
        self.clamp_input_cursor();
    }

    pub(crate) fn delete_to_end_of_input_line(&mut self) {
        delete_to_end_utf16(&mut self.input_line, self.input_cursor_utf16);
        self.clamp_input_cursor();
    }

    pub(crate) fn apply_terminal_bytes_to_input_line(&mut self, bytes: &[u8]) {
        match bytes {
            b"\r" => self.clear_input_line(),
            [0x7f] => self.backspace_input_char(),
            [0x08] => self.backspace_input_char(), // Ctrl-H
            [0x01] => self.input_cursor_utf16 = 0, // Ctrl-A
            [0x05] => self.input_cursor_utf16 = self.input_line_len_utf16(), // Ctrl-E
            [0x02] => self.move_input_cursor_left(), // Ctrl-B
            [0x06] => self.move_input_cursor_right(), // Ctrl-F
            [0x03] => self.clear_input_line(),     // Ctrl-C
            [0x04] => self.delete_input_char_at_cursor(), // Ctrl-D
            [0x0b] => self.delete_to_end_of_input_line(), // Ctrl-K
            [0x17] => self.delete_previous_input_word(), // Ctrl-W
            [0x15] => self.clear_input_line(),     // Ctrl-U clears current line in common shells.
            b"\x1b[D" => self.move_input_cursor_left(),
            b"\x1b[C" => self.move_input_cursor_right(),
            b"\x1b[H" => self.input_cursor_utf16 = 0,
            b"\x1b[F" => self.input_cursor_utf16 = self.input_line_len_utf16(),
            b"\x1b[3~" => self.delete_input_char_at_cursor(),
            b"\x1b\x7f" => self.delete_previous_input_word(), // Alt-Backspace
            b"\x1bd" => self.delete_next_input_word(),        // Alt-D
            _ => {
                if bytes.first() == Some(&0x1b) {
                    return;
                }

                if let Ok(text) = std::str::from_utf8(bytes)
                    && !text.chars().any(char::is_control)
                {
                    self.insert_input_text_at_cursor(text);
                }
            }
        }
    }

    pub(crate) fn rewrite_terminal_input_line(&mut self) {
        self.write_bytes(&[0x15]); // Ctrl-U clears shell input line.
        if !self.input_line.is_empty() {
            let line = self.input_line.clone();
            self.write_bytes(line.as_bytes());
        }

        let tail = self
            .input_line_len_utf16()
            .saturating_sub(self.input_cursor_utf16);
        for _ in 0..tail {
            self.write_bytes(b"\x1b[D");
        }
    }

    pub(crate) fn ime_cursor_bounds(
        &self,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
    ) -> Bounds<Pixels> {
        let cell_width = measure_cell_width(
            window,
            &self.font_family,
            self.font_fallbacks.as_ref(),
            self.font_size,
        );
        let line_height = self.line_height();
        let dynamic_padding_y = terminal_content_padding_y(
            element_bounds.size.height,
            line_height,
            self.grid_size.rows as usize,
        );
        let cursor_origin = point(
            element_bounds.origin.x + TEXT_PADDING_X + self.snapshot.cursor_col as f32 * cell_width,
            element_bounds.origin.y
                + dynamic_padding_y
                + self.snapshot.cursor_row as f32 * line_height,
        );
        Bounds::new(cursor_origin, size(cell_width.max(px(2.0)), line_height))
    }

    define_mouse_forwarders!(
        (on_mouse_down_left, on_mouse_up_left),
        (on_mouse_down_middle, on_mouse_up_middle),
        (on_mouse_down_right, on_mouse_up_right),
    );

    pub(crate) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selection_mode_active {
            if event.pressed_button != self.selection_button {
                return;
            }

            let (row, col) = self.mouse_grid_point(event.position, window);
            if self.update_selection_focus(row, col) {
                self.trace_input(format!(
                    "selection move button={:?} row={} col={}",
                    event.pressed_button, row, col
                ));
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }

        let mode = *self.term.mode();
        if !mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG) || event.modifiers.shift
        {
            return;
        }

        let Some(button_code) = mouse_move_button_code(event.pressed_button) else {
            return;
        };

        if mode.contains(TermMode::MOUSE_DRAG) && button_code == 35 {
            return;
        }

        let (row, col) = self.mouse_grid_point(event.position, window);
        if self.last_mouse_report == Some((row, col, button_code)) {
            return;
        }

        if let Some(bytes) = encode_mouse_report(row, col, button_code, true, event.modifiers, mode)
        {
            self.write_bytes(&bytes);
            self.last_mouse_report = Some((row, col, button_code));
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(crate) fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode = *self.term.mode();
        let (x_steps, y_steps) = self.scroll_steps(event, window);
        if x_steps == 0 && y_steps == 0 {
            return;
        }

        let mut handled = false;
        if mode.intersects(TermMode::MOUSE_MODE) && !event.modifiers.shift {
            let (row, col) = self.mouse_grid_point(event.position, window);
            handled |= self.repeat_wheel_report(row, col, x_steps, y_steps, event.modifiers, mode);
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
            && !event.modifiers.shift
        {
            let bytes = encode_alt_scroll_bytes(x_steps, y_steps);
            if !bytes.is_empty() {
                self.write_bytes(&bytes);
                handled = true;
            }
        }

        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.last_mouse_report = None;
        self.trace_input(format!(
            "mouse down button={:?} control={} shift={} alt={} platform={}",
            event.button,
            event.modifiers.control,
            event.modifiers.shift,
            event.modifiers.alt,
            event.modifiers.platform
        ));

        if event.button == MouseButton::Left && event.modifiers.shift {
            let (row, col) = self.mouse_grid_point(event.position, window);
            self.start_selection(row, col, MouseButton::Left);
            self.trace_input(format!(
                "selection start button={:?} row={} col={}",
                MouseButton::Left,
                row,
                col
            ));
            cx.stop_propagation();
            cx.notify();
            return;
        }

        let mode = *self.term.mode();
        if mode.intersects(TermMode::MOUSE_MODE)
            && !event.modifiers.shift
            && let Some(button_code) = mouse_button_code(event.button)
        {
            let (row, col) = self.mouse_grid_point(event.position, window);
            if let Some(bytes) =
                encode_mouse_report(row, col, button_code, true, event.modifiers, mode)
            {
                self.write_bytes(&bytes);
                cx.stop_propagation();
                cx.notify();
            }
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection_mode_active && self.selection_button == Some(event.button) {
            let (row, col) = self.mouse_grid_point(event.position, window);
            self.update_selection_focus(row, col);
            let copied = self.copy_current_selection_to_clipboard(cx);
            if copied {
                self.debug
                    .set_note(Some("selection copied to clipboard".to_string()));
            } else {
                self.debug
                    .set_note(Some("selection empty, nothing copied".to_string()));
            }
            self.trace_input(format!(
                "selection finish button={:?} row={} col={} copied={}",
                event.button, row, col, copied
            ));
            self.clear_selection();
            cx.stop_propagation();
            cx.notify();
            return;
        }

        let mode = *self.term.mode();
        if mode.intersects(TermMode::MOUSE_MODE)
            && !event.modifiers.shift
            && let Some(button_code) = mouse_button_code(event.button)
        {
            let (row, col) = self.mouse_grid_point(event.position, window);
            if let Some(bytes) =
                encode_mouse_report(row, col, button_code, false, event.modifiers, mode)
            {
                self.write_bytes(&bytes);
                cx.stop_propagation();
                cx.notify();
            }
        }

        self.last_mouse_report = None;
    }

    fn repeat_wheel_report(
        &mut self,
        row: usize,
        col: usize,
        x_steps: i32,
        y_steps: i32,
        modifiers: gpui::Modifiers,
        mode: TermMode,
    ) -> bool {
        let mut handled = false;

        let y_code = if y_steps > 0 { 64 } else { 65 };
        for _ in 0..y_steps.unsigned_abs() {
            if let Some(bytes) = encode_mouse_report(row, col, y_code, true, modifiers, mode) {
                self.write_bytes(&bytes);
                handled = true;
            }
        }

        let x_code = if x_steps > 0 { 66 } else { 67 };
        for _ in 0..x_steps.unsigned_abs() {
            if let Some(bytes) = encode_mouse_report(row, col, x_code, true, modifiers, mode) {
                self.write_bytes(&bytes);
                handled = true;
            }
        }

        handled
    }

    fn scroll_steps(&mut self, event: &ScrollWheelEvent, window: &mut Window) -> (i32, i32) {
        match event.delta {
            ScrollDelta::Lines(lines) => (lines.x as i32, lines.y as i32),
            ScrollDelta::Pixels(pixels) => {
                let cell_width = f32::from(
                    measure_cell_width(
                        window,
                        &self.font_family,
                        self.font_fallbacks.as_ref(),
                        self.font_size,
                    )
                    .max(px(1.0)),
                );
                let line_height = f32::from(self.line_height().max(px(1.0)));

                self.mouse_scroll_accum_x += f32::from(pixels.x);
                self.mouse_scroll_accum_y += f32::from(pixels.y);

                let x_steps = (self.mouse_scroll_accum_x / cell_width) as i32;
                let y_steps = (self.mouse_scroll_accum_y / line_height) as i32;

                self.mouse_scroll_accum_x %= cell_width;
                self.mouse_scroll_accum_y %= line_height;

                (x_steps, y_steps)
            }
        }
    }

    fn mouse_grid_point(
        &self,
        position: gpui::Point<Pixels>,
        window: &mut Window,
    ) -> (usize, usize) {
        let line_height = self.line_height().max(px(1.0));
        let cell_width = measure_cell_width(
            window,
            &self.font_family,
            self.font_fallbacks.as_ref(),
            self.font_size,
        )
        .max(px(1.0));

        let origin = if let Some(bounds) = *self.canvas_bounds.lock() {
            let dynamic_padding_y = terminal_content_padding_y(
                bounds.size.height,
                line_height,
                self.grid_size.rows as usize,
            );
            point(
                bounds.origin.x + TEXT_PADDING_X,
                bounds.origin.y + dynamic_padding_y,
            )
        } else {
            let title_bar_height = if self.show_title_bar {
                CUSTOM_TITLE_BAR_HEIGHT
            } else {
                px(0.0)
            };
            let status_height = if self.show_status_bar {
                STATUS_BAR_HEIGHT
            } else {
                px(0.0)
            };
            let surface_height = (window.viewport_size().height
                - title_bar_height
                - status_height)
                .max(line_height);
            point(
                TEXT_PADDING_X,
                title_bar_height
                    + status_height
                    + terminal_content_padding_y(
                        surface_height,
                        line_height,
                        self.grid_size.rows as usize,
                    ),
            )
        };

        let raw_col = ((position.x - origin.x) / cell_width).floor() as i32;
        let raw_row = ((position.y - origin.y) / line_height).floor() as i32;

        let max_col = self.grid_size.cols.saturating_sub(1) as i32;
        let max_row = self.grid_size.rows.saturating_sub(1) as i32;

        let col = raw_col.clamp(0, max_col) as usize;
        let row = raw_row.clamp(0, max_row) as usize;
        (row, col)
    }

    pub(crate) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.trace_input(format!(
            "keydown key={:?} key_char={:?} modifiers={:?}",
            event.keystroke.key, event.keystroke.key_char, event.keystroke.modifiers
        ));
        self.log_input_event(
            "keydown",
            json!({
                "key": event.keystroke.key.clone(),
                "key_char": event.keystroke.key_char.clone(),
                "modifiers": format!("{:?}", event.keystroke.modifiers),
                "input_line": self.input_log_text_value(&self.input_line),
                "input_cursor_utf16": self.input_cursor_utf16,
            }),
        );

        if is_zoom_in_shortcut(&event.keystroke) {
            self.debug.record_key_event();
            self.adjust_font_size(px(FONT_SIZE_STEP), window);
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if is_zoom_out_shortcut(&event.keystroke) {
            self.debug.record_key_event();
            self.adjust_font_size(px(-FONT_SIZE_STEP), window);
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if is_select_all_shortcut(&event.keystroke) {
            self.mark_local_key_activity();
            self.debug.record_key_event();
            self.clear_input_line();
            self.write_bytes(&[0x15]); // Ctrl-U clears shell input line.
            self.trace_input("keydown cmd-a clear current input line");
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if is_paste_shortcut(&event.keystroke) {
            self.mark_local_key_activity();
            self.debug.record_key_event();
            if let Some(item) = cx.read_from_clipboard()
                && let Some(text) = item.text()
            {
                if let Some(risk) = evaluate_paste_risk(&text) {
                    if self.paste_guard_prompt_open {
                        self.debug
                            .set_note(Some("paste confirmation already open".to_string()));
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }

                    self.paste_guard_prompt_open = true;
                    let detail = format!(
                        "{} lines, {} chars, {:.0}% non-ASCII text.\nPaste anyway?",
                        risk.line_count,
                        risk.char_count,
                        risk.non_ascii_ratio * 100.0
                    );
                    let answer = window.prompt(
                        PromptLevel::Warning,
                        "Large multi-line paste detected",
                        Some(&detail),
                        &["Paste", "Cancel"],
                        cx,
                    );
                    let text_to_paste = text.to_string();

                    cx.spawn(
                        async move |this: gpui::WeakEntity<AgentTerminal>,
                                    cx: &mut gpui::AsyncApp| {
                            let paste_allowed = answer.await.ok() == Some(0);
                            let _ = this.update(cx, move |this, cx| {
                                this.paste_guard_prompt_open = false;
                                if paste_allowed {
                                    this.insert_input_text_at_cursor(&text_to_paste);
                                    this.write_paste_input(&text_to_paste);
                                    this.debug
                                        .set_note(Some("large paste confirmed".to_string()));
                                } else {
                                    this.debug
                                        .set_note(Some("large paste canceled".to_string()));
                                }
                                cx.notify();
                            });
                        },
                    )
                    .detach();

                    cx.stop_propagation();
                    cx.notify();
                    return;
                }

                self.insert_input_text_at_cursor(&text);
                self.write_paste_input(&text);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if should_defer_to_text_input(&event.keystroke, self.option_as_meta) {
            self.mark_local_key_activity();
            self.trace_input("keydown deferred to text input handler");
            self.log_input_event(
                "keydown_deferred_to_text_input",
                json!({
                    "key": event.keystroke.key.clone(),
                    "key_char": event.keystroke.key_char.clone(),
                }),
            );
            return;
        }

        if let Some(bytes) = encode_keystroke(&event.keystroke) {
            self.mark_local_key_activity();
            self.debug.record_key_event();
            let before_line = self.input_line.clone();
            let before_cursor = self.input_cursor_utf16;
            let enter_probe_id = if event.keystroke.key.eq_ignore_ascii_case("enter") {
                Some(self.start_enter_latency_probe(&before_line))
            } else {
                None
            };
            let high_priority_control = is_high_priority_control_bytes(&bytes);

            if high_priority_control {
                self.write_bytes(&bytes);
                if let Some(probe_id) = enter_probe_id {
                    self.mark_enter_latency_write_done(probe_id, bytes.len());
                }
            }
            self.apply_terminal_bytes_to_input_line(&bytes);
            self.log_input_event(
                "keydown_encoded",
                json!({
                    "key": event.keystroke.key.clone(),
                    "bytes_hex": bytes_to_hex(&bytes),
                    "high_priority_control": high_priority_control,
                    "before_line": self.input_log_text_value(&before_line),
                    "before_cursor_utf16": before_cursor,
                    "after_line": self.input_log_text_value(&self.input_line),
                    "after_cursor_utf16": self.input_cursor_utf16,
                }),
            );
            if !high_priority_control {
                self.write_bytes(&bytes);
                if let Some(probe_id) = enter_probe_id {
                    self.mark_enter_latency_write_done(probe_id, bytes.len());
                }
            }
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(crate) fn trace_input(&self, message: impl AsRef<str>) {
        if self.input_trace {
            eprintln!("[input-trace] {}", message.as_ref());
        }
    }

    pub(crate) fn log_input_event(&self, event: &str, fields: serde_json::Value) {
        if let Some(logger) = &self.input_logger {
            logger.log_event(event, fields);
        }
    }

    pub(crate) fn input_log_text_value(&self, text: &str) -> serde_json::Value {
        if let Some(logger) = &self.input_logger {
            logger.text_value(text)
        } else {
            json!(summarize_text_for_trace(text))
        }
    }

    pub(crate) fn window_title(&self) -> String {
        format!("agent terminal | {}", self.tab_title())
    }

    pub(crate) fn apply_external_ax_input_state(&mut self, state: NativeAxInputState) -> bool {
        if self.snapshot.alt_screen {
            return false;
        }
        if self.ime_marked_text.is_some() {
            self.trace_input("ax override skipped during ime composition");
            self.log_input_event(
                "ax_override_skipped",
                json!({
                    "reason": "ime_composing",
                    "ax_line": self.input_log_text_value(&state.text),
                    "ax_cursor_utf16": state.cursor_utf16,
                }),
            );
            return false;
        }

        let cursor_utf16 = state.cursor_utf16.min(state.text.encode_utf16().count());
        if self.input_line == state.text && self.input_cursor_utf16 == cursor_utf16 {
            return false;
        }
        let screen_match = self.ax_text_matches_screen_context(&state.text);
        if !screen_match && self.ax_text_has_probable_prefix_noise(&state.text) {
            self.trace_input(format!(
                "ax override rejected (probable prefix noise) model={} ax={}",
                summarize_text_for_trace(&self.input_line),
                summarize_text_for_trace(&state.text)
            ));
            self.log_input_event(
                "ax_override_rejected",
                json!({
                    "reason": "probable_prefix_noise",
                    "model_line": self.input_log_text_value(&self.input_line),
                    "ax_line": self.input_log_text_value(&state.text),
                    "ax_cursor_utf16": cursor_utf16,
                }),
            );
            return false;
        }
        if !screen_match {
            self.trace_input(format!(
                "ax override accepted despite screen mismatch model={} ax={}",
                summarize_text_for_trace(&self.input_line),
                summarize_text_for_trace(&state.text)
            ));
            self.log_input_event(
                "ax_override_mismatch_accepted",
                json!({
                    "model_line": self.input_log_text_value(&self.input_line),
                    "ax_line": self.input_log_text_value(&state.text),
                    "ax_cursor_utf16": cursor_utf16,
                }),
            );
        }

        self.trace_input(format!(
            "ax override text={} cursor_utf16={}",
            summarize_text_for_trace(&state.text),
            cursor_utf16
        ));

        self.input_line = state.text;
        self.input_cursor_utf16 = cursor_utf16;
        self.ime_marked_text = None;
        self.rewrite_terminal_input_line();
        self.log_input_event(
            "ax_override_applied",
            json!({
                "input_line": self.input_log_text_value(&self.input_line),
                "input_cursor_utf16": self.input_cursor_utf16,
            }),
        );
        true
    }

    pub(crate) fn selection_bounds(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        let anchor = self.selection_anchor?;
        let focus = self.selection_focus?;
        Some(normalize_selection_bounds(anchor, focus))
    }

    fn start_selection(&mut self, row: usize, col: usize, button: MouseButton) {
        let point = self.normalize_selection_point(row, col);
        self.selection_mode_active = true;
        self.selection_button = Some(button);
        self.selection_anchor = Some(point);
        self.selection_focus = Some(point);
    }

    fn update_selection_focus(&mut self, row: usize, col: usize) -> bool {
        let point = self.normalize_selection_point(row, col);
        if self.selection_focus == Some(point) {
            return false;
        }
        self.selection_focus = Some(point);
        true
    }

    fn clear_selection(&mut self) {
        self.selection_mode_active = false;
        self.selection_button = None;
        self.selection_anchor = None;
        self.selection_focus = None;
    }

    fn current_selection_text(&self) -> Option<String> {
        let (start, end) = self.selection_bounds()?;
        let text = extract_selection_text(&self.snapshot, start, end);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn copy_current_selection_to_clipboard(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.current_selection_text() else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    fn normalize_selection_point(&self, row: usize, col: usize) -> SelectionPoint {
        let normalized_col = self
            .snapshot
            .cells
            .get(row)
            .map(|cells| normalize_selection_col(cells, col))
            .unwrap_or(col);
        SelectionPoint {
            row,
            col: normalized_col,
        }
    }

    pub(crate) fn allow_ax_override(&self) -> bool {
        if self.ime_marked_text.is_some() {
            return false;
        }
        match self.last_local_key_event_at {
            Some(last_event) => last_event.elapsed() >= AX_OVERRIDE_GUARD_WINDOW,
            None => true,
        }
    }

    fn mark_local_key_activity(&mut self) {
        self.last_local_key_event_at = Some(Instant::now());
    }

    fn ax_text_matches_screen_context(&self, ax_text: &str) -> bool {
        if ax_text.is_empty() {
            return true;
        }

        let Some(row) = self.snapshot.cells.get(self.snapshot.cursor_row) else {
            return true;
        };
        if row.is_empty() {
            return true;
        }

        let visible_row = row_text_without_wide_spacers(row);
        if visible_row.is_empty() {
            return true;
        }

        let mut row_before_cursor = String::new();
        let mut covered_until_col = 0usize;
        for (col_index, cell) in row.iter().enumerate() {
            if col_index > self.snapshot.cursor_col {
                break;
            }
            if col_index < covered_until_col {
                continue;
            }
            row_before_cursor.push(cell.ch);
            covered_until_col = col_index.saturating_add(cell_advance_cols(cell));
        }

        let row_before_cursor = row_before_cursor.trim_end();
        let visible_row = visible_row.trim_end();
        visible_row.contains(ax_text)
            || row_before_cursor.contains(ax_text)
            || ax_text.starts_with(&self.input_line)
    }

    fn ax_text_has_probable_prefix_noise(&self, ax_text: &str) -> bool {
        probable_ascii_prefix_noise(ax_text, &self.input_line)
    }
}

fn mouse_button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        // Mirror Zed's mapping for consistency with existing behavior.
        MouseButton::Right => Some(1),
        MouseButton::Middle => Some(2),
        MouseButton::Navigate(_) => None,
    }
}

fn mouse_move_button_code(button: Option<MouseButton>) -> Option<u8> {
    match button {
        Some(MouseButton::Left) => Some(32),
        Some(MouseButton::Middle) => Some(33),
        Some(MouseButton::Right) => Some(34),
        Some(MouseButton::Navigate(_)) => None,
        None => Some(35),
    }
}

fn encode_mouse_report(
    row: usize,
    col: usize,
    button: u8,
    pressed: bool,
    modifiers: gpui::Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let mods = mouse_modifiers(modifiers);
    if mode.contains(TermMode::SGR_MOUSE) {
        let effective_button = if pressed {
            button.saturating_add(mods)
        } else {
            3u8.saturating_add(mods)
        };
        Some(
            format!(
                "\x1b[<{};{};{}{}",
                effective_button,
                col + 1,
                row + 1,
                if pressed { 'M' } else { 'm' }
            )
            .into_bytes(),
        )
    } else {
        let effective_button = if pressed {
            button.saturating_add(mods)
        } else {
            3u8.saturating_add(mods)
        };
        encode_normal_mouse_report(
            row,
            col,
            effective_button,
            mode.contains(TermMode::UTF8_MOUSE),
        )
    }
}

fn mouse_modifiers(modifiers: gpui::Modifiers) -> u8 {
    let mut mods = 0u8;
    if modifiers.shift {
        mods = mods.saturating_add(4);
    }
    if modifiers.alt {
        mods = mods.saturating_add(8);
    }
    if modifiers.control {
        mods = mods.saturating_add(16);
    }
    mods
}

fn encode_normal_mouse_report(row: usize, col: usize, button: u8, utf8: bool) -> Option<Vec<u8>> {
    let max_point = if utf8 { 2015 } else { 223 };
    if row >= max_point || col >= max_point {
        return None;
    }

    let mut msg = vec![b'\x1b', b'[', b'M', 32u8.saturating_add(button)];

    if utf8 && col >= 95 {
        msg.extend(encode_normal_mouse_pos(col));
    } else {
        msg.push(32u8.saturating_add(1u8).saturating_add(col as u8));
    }

    if utf8 && row >= 95 {
        msg.extend(encode_normal_mouse_pos(row));
    } else {
        msg.push(32u8.saturating_add(1u8).saturating_add(row as u8));
    }

    Some(msg)
}

fn encode_normal_mouse_pos(pos: usize) -> [u8; 2] {
    let value = 32 + 1 + pos;
    let first = 0xC0 + value / 64;
    let second = 0x80 + (value & 63);
    [first as u8, second as u8]
}

fn encode_alt_scroll_bytes(x_steps: i32, y_steps: i32) -> Vec<u8> {
    let mut content =
        Vec::with_capacity(3 * (x_steps.unsigned_abs() + y_steps.unsigned_abs()) as usize);

    for _ in 0..y_steps.unsigned_abs() {
        content.push(0x1b);
        content.push(b'O');
        content.push(if y_steps > 0 { b'A' } else { b'B' });
    }

    for _ in 0..x_steps.unsigned_abs() {
        content.push(0x1b);
        content.push(b'O');
        content.push(if x_steps > 0 { b'D' } else { b'C' });
    }

    content
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn evaluate_paste_risk(text: &str) -> Option<PasteRisk> {
    let line_count = text.lines().count().max(1);
    let char_count = text.chars().count();
    if line_count < PASTE_GUARD_MIN_LINES || char_count < PASTE_GUARD_MIN_CHARS {
        return None;
    }

    let mut visible_chars = 0usize;
    let mut non_ascii_chars = 0usize;
    for ch in text.chars() {
        if ch.is_control() || ch.is_whitespace() {
            continue;
        }
        visible_chars += 1;
        if !ch.is_ascii() {
            non_ascii_chars += 1;
        }
    }

    if visible_chars == 0 {
        return None;
    }

    let non_ascii_ratio = non_ascii_chars as f32 / visible_chars as f32;
    if non_ascii_ratio < PASTE_GUARD_NON_ASCII_RATIO {
        return None;
    }

    Some(PasteRisk {
        line_count,
        char_count,
        non_ascii_ratio,
    })
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

pub(crate) fn selection_contains_cell(
    start: SelectionPoint,
    end: SelectionPoint,
    row: usize,
    col: usize,
) -> bool {
    if row < start.row || row > end.row {
        return false;
    }

    if start.row == end.row {
        return row == start.row && col >= start.col && col <= end.col;
    }

    if row == start.row {
        return col >= start.col;
    }

    if row == end.row {
        return col <= end.col;
    }

    true
}

fn extract_selection_text(
    snapshot: &ScreenSnapshot,
    start: SelectionPoint,
    end: SelectionPoint,
) -> String {
    let mut lines = Vec::new();

    for row in start.row..=end.row {
        let Some(cells) = snapshot.cells.get(row) else {
            break;
        };
        if cells.is_empty() {
            lines.push(String::new());
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
            lines.push(String::new());
            continue;
        }

        let clamped_end = line_end.min(cells.len().saturating_sub(1));
        if line_start > clamped_end {
            lines.push(String::new());
            continue;
        }

        let mut text = String::new();
        let mut col = line_start;
        while col <= clamped_end {
            let cell = &cells[col];
            text.push(cell.ch);
            let step = cell_advance_cols(cell);
            col = col.saturating_add(step);
        }
        while text.ends_with(' ') {
            text.pop();
        }
        lines.push(text);
    }

    lines.join("\n")
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

fn cell_advance_cols(cell: &CellSnapshot) -> usize {
    if cell.spans_next_col {
        usize::from(cell.width_cols.max(1))
    } else {
        1
    }
}

fn probable_ascii_prefix_noise(ax_text: &str, model_text: &str) -> bool {
    if model_text.is_empty() {
        return false;
    }
    let Some(prefix) = ax_text.strip_suffix(model_text) else {
        return false;
    };
    let prefix_len = prefix.chars().count();
    (1..=4).contains(&prefix_len) && prefix.chars().all(|ch| ch.is_ascii_alphanumeric())
}

fn is_high_priority_control_bytes(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }

    // Prioritize PTY dispatch for control bytes and escape sequences
    // (Enter, Backspace, Ctrl chords, arrows, function keys, etc).
    bytes[0] == 0x1b || bytes.iter().any(|byte| *byte < 0x20 || *byte == 0x7f)
}

impl EntityInputHandler for AgentTerminal {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        if let Some(marked_text) = self.ime_marked_text.as_ref() {
            let len = marked_text.encode_utf16().count();
            if len == 0 {
                *adjusted_range = Some(0..0);
                return Some(String::new());
            }

            let start = range.start.min(len);
            let end = range.end.min(len);
            if start >= end {
                *adjusted_range = Some(0..len);
                return Some(marked_text.clone());
            }

            *adjusted_range = Some(start..end);
            return Some(utf16_substring(marked_text, start..end).unwrap_or_default());
        }

        *adjusted_range = None;
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if self.snapshot.alt_screen {
            None
        } else {
            Some(UTF16Selection {
                range: 0..0,
                reversed: false,
            })
        }
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.ime_marked_text
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ime_marked_text = None;
        self.trace_input("ime unmark_text");
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_text = None;
        self.trace_input(format!(
            "ime replace_text_in_range len={} text={}",
            text.len(),
            summarize_text_for_trace(text)
        ));
        self.log_input_event(
            "ime_replace_text_in_range",
            json!({
                "replacement_range": format!("{:?}", range),
                "text": self.input_log_text_value(text),
                "before_line": self.input_log_text_value(&self.input_line),
                "before_cursor_utf16": self.input_cursor_utf16,
            }),
        );

        // IME composition always commits at cursor; range-based replacement is not
        // supported because our input model writes directly to the PTY shell.
        let _ = range;
        self.insert_input_text_at_cursor(text);
        self.write_text_input(text);
        self.log_input_event(
            "ime_replace_text_in_range_applied",
            json!({
                "after_line": self.input_log_text_value(&self.input_line),
                "after_cursor_utf16": self.input_cursor_utf16,
            }),
        );
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.trace_input(format!(
            "ime replace_and_mark_text len={}",
            new_text.len()
        ));
        self.ime_marked_text = if new_text.is_empty() {
            None
        } else {
            Some(new_text.to_string())
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let mut bounds = self.ime_cursor_bounds(element_bounds, window);
        if self.ime_marked_text.is_some() {
            // bounds.size.width is cell_width (from ime_cursor_bounds), reuse it.
            let cell_width = bounds.size.width.max(px(1.0));
            bounds.origin.x += cell_width * range_utf16.start as f32;
        }
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use crate::render::terminal_content_padding_y;
    use crate::terminal::CellSnapshot;

    use super::{
        evaluate_paste_risk, extract_selection_text, normalize_selection_bounds,
        probable_ascii_prefix_noise, selection_contains_cell,
    };
    use crate::terminal::{ScreenSnapshot, SelectionPoint};

    #[test]
    fn paste_risk_requires_multiline_and_non_ascii_heavy_content() {
        let safe = "line1\nline2\nline3\nline4";
        assert!(evaluate_paste_risk(safe).is_none());

        let risky = "中文内容一二三四五六七八九十中文内容一二三四五六七八九十\n第二行中文内容一二三四五六七八九十中文内容一二三四五六七八九十\n第三行中文内容一二三四五六七八九十中文内容一二三四五六七八九十\n第四行中文内容一二三四五六七八九十中文内容一二三四五六七八九十\n";
        assert!(evaluate_paste_risk(risky).is_some());
    }

    #[test]
    fn paste_risk_ignores_short_text_even_if_non_ascii() {
        let short = "你好\n你好\n你好\n你好\n";
        assert!(evaluate_paste_risk(short).is_none());
    }

    #[test]
    fn selection_bounds_are_normalized() {
        let a = SelectionPoint { row: 4, col: 10 };
        let b = SelectionPoint { row: 1, col: 2 };
        let (start, end) = normalize_selection_bounds(a, b);
        assert_eq!(start, b);
        assert_eq!(end, a);
    }

    #[test]
    fn selection_contains_handles_single_and_multi_line_ranges() {
        let start = SelectionPoint { row: 1, col: 3 };
        let end = SelectionPoint { row: 3, col: 2 };

        assert!(selection_contains_cell(start, end, 1, 3));
        assert!(selection_contains_cell(start, end, 2, 50));
        assert!(selection_contains_cell(start, end, 3, 2));
        assert!(!selection_contains_cell(start, end, 0, 10));
        assert!(!selection_contains_cell(start, end, 1, 2));
        assert!(!selection_contains_cell(start, end, 3, 3));
    }

    #[test]
    fn extract_selection_text_returns_expected_multiline_slice() {
        let snapshot = ScreenSnapshot {
            cells: vec![
                "012345".chars().map(cell).collect(),
                "abcdef".chars().map(cell).collect(),
                "uvwxyz".chars().map(cell).collect(),
            ],
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            alt_screen: false,
        };

        let text = extract_selection_text(
            &snapshot,
            SelectionPoint { row: 0, col: 2 },
            SelectionPoint { row: 2, col: 3 },
        );

        assert_eq!(text, "2345\nabcdef\nuvwx");
    }

    #[test]
    fn extract_selection_text_skips_wide_char_spacers() {
        let snapshot = ScreenSnapshot {
            cells: vec![vec![
                wide_cell('你'),
                cell(' '),
                wide_cell('好'),
                cell(' '),
                cell('X'),
            ]],
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            alt_screen: false,
        };

        let text = extract_selection_text(
            &snapshot,
            SelectionPoint { row: 0, col: 0 },
            SelectionPoint { row: 0, col: 4 },
        );

        assert_eq!(text, "你好X");
    }

    #[test]
    fn terminal_padding_matches_single_row_floor() {
        let padding = terminal_content_padding_y(px(100.0), px(18.0), 1);
        assert_eq!(padding, px(41.0));
    }

    #[test]
    fn probable_prefix_noise_detects_short_ascii_prefix() {
        assert!(probable_ascii_prefix_noise("3n你好世界", "你好世界"));
        assert!(probable_ascii_prefix_noise("nnn你好世界", "你好世界"));
        assert!(!probable_ascii_prefix_noise("前缀你好世界", "你好世界"));
        assert!(!probable_ascii_prefix_noise("12345你好世界", "你好世界"));
        assert!(!probable_ascii_prefix_noise("你好世界abc", "你好世界"));
    }

    fn cell(ch: char) -> CellSnapshot {
        CellSnapshot {
            ch,
            ..CellSnapshot::default()
        }
    }

    fn wide_cell(ch: char) -> CellSnapshot {
        CellSnapshot {
            ch,
            width_cols: 2,
            spans_next_col: true,
            ..CellSnapshot::default()
        }
    }
}
