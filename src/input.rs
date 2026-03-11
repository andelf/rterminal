use std::ops::Range;

use gpui::{
    App, Bounds, Context, EntityInputHandler, InputHandler, KeyDownEvent, MouseDownEvent, Pixels,
    UTF16Selection, Window, point, px, size,
};

use crate::keyboard::{
    encode_keystroke, is_paste_shortcut, is_select_all_shortcut, is_zoom_in_shortcut,
    is_zoom_out_shortcut, should_defer_to_text_input,
};
use crate::macos_ax::NativeAxInputState;
use crate::render::{TEXT_PADDING_X, TEXT_PADDING_Y, measure_cell_width};
use crate::text_utils::{
    delete_next_word_utf16, delete_previous_word_utf16, delete_to_end_utf16, replace_range_utf16,
    summarize_text_for_trace, utf16_substring, utf16_to_byte_index,
};
use crate::AgentTerminal;

const FONT_SIZE_STEP: f32 = 1.0;

pub(crate) struct AgentTerminalInputHandler {
    terminal: gpui::Entity<AgentTerminal>,
    element_bounds: Bounds<Pixels>,
}

impl AgentTerminalInputHandler {
    pub(crate) fn new(element_bounds: Bounds<Pixels>, terminal: gpui::Entity<AgentTerminal>) -> Self {
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
        self.write_bytes(text.as_bytes());
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

    pub(crate) fn replace_input_range_utf16(&mut self, range: Range<usize>, text: &str) {
        let len = self.input_line_len_utf16();
        let start = range.start.min(len);
        let end = range.end.min(len);
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };

        replace_range_utf16(&mut self.input_line, start..end, text);
        self.input_cursor_utf16 = start + text.encode_utf16().count();
        self.clamp_input_cursor();
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

    pub(crate) fn current_input_line_text_for_range(
        &self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
    ) -> String {
        let len = self.input_line_len_utf16();
        if len == 0 {
            *adjusted_range = Some(0..0);
            return String::new();
        }

        if range.start >= range.end || range.start >= len {
            *adjusted_range = Some(0..len);
            return self.input_line.clone();
        }

        let start = range.start.min(len);
        let end = range.end.min(len);
        *adjusted_range = Some(start..end);
        utf16_substring(&self.input_line, start..end).unwrap_or_default()
    }

    pub(crate) fn ime_cursor_bounds(
        &self,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
    ) -> Bounds<Pixels> {
        let cell_width = measure_cell_width(window, &self.font_family, self.font_size);
        let line_height = self.line_height();
        let cursor_origin = point(
            element_bounds.origin.x + TEXT_PADDING_X + self.snapshot.cursor_col as f32 * cell_width,
            element_bounds.origin.y
                + TEXT_PADDING_Y
                + self.snapshot.cursor_row as f32 * line_height,
        );
        Bounds::new(cursor_origin, size(cell_width.max(px(2.0)), line_height))
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        _event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
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
            self.debug.record_key_event();
            self.clear_input_line();
            self.write_bytes(&[0x15]); // Ctrl-U clears shell input line.
            self.trace_input("keydown cmd-a clear current input line");
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if is_paste_shortcut(&event.keystroke) {
            self.debug.record_key_event();
            if let Some(item) = cx.read_from_clipboard()
                && let Some(text) = item.text()
            {
                self.insert_input_text_at_cursor(&text);
                self.write_text_input(&text);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if should_defer_to_text_input(&event.keystroke) {
            self.trace_input("keydown deferred to text input handler");
            return;
        }

        if let Some(bytes) = encode_keystroke(&event.keystroke) {
            self.debug.record_key_event();
            self.apply_terminal_bytes_to_input_line(&bytes);
            self.write_bytes(&bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(crate) fn trace_input(&self, message: impl AsRef<str>) {
        if self.input_trace {
            eprintln!("[input-trace] {}", message.as_ref());
        }
    }

    pub(crate) fn window_title(&self) -> String {
        if let Some(title) = self.terminal_title.lock().clone()
            && !title.trim().is_empty()
        {
            return title;
        }
        format!("agent terminal | {}", self.shell)
    }

    pub(crate) fn apply_external_ax_input_state(&mut self, state: NativeAxInputState) -> bool {
        if self.snapshot.alt_screen {
            return false;
        }

        let cursor_utf16 = state.cursor_utf16.min(state.text.encode_utf16().count());
        if self.input_line == state.text && self.input_cursor_utf16 == cursor_utf16 {
            return false;
        }

        self.trace_input(format!(
            "ax override text={} cursor_utf16={}",
            summarize_text_for_trace(&state.text),
            cursor_utf16
        ));

        self.input_line = state.text;
        self.input_cursor_utf16 = cursor_utf16;
        self.marked_text_range = None;
        self.rewrite_terminal_input_line();
        true
    }
}

impl EntityInputHandler for AgentTerminal {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        Some(self.current_input_line_text_for_range(range, adjusted_range))
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
            self.clamp_input_cursor();
            Some(UTF16Selection {
                range: self.input_cursor_utf16..self.input_cursor_utf16,
                reversed: false,
            })
        }
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_text_range.clone()
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked_text_range = None;
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
        self.marked_text_range = None;
        self.trace_input(format!(
            "ime replace_text_in_range len={} text={}",
            text.len(),
            summarize_text_for_trace(text)
        ));

        if let Some(range) = range {
            self.replace_input_range_utf16(range, text);
            self.rewrite_terminal_input_line();
        } else {
            self.insert_input_text_at_cursor(text);
            self.write_text_input(text);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.trace_input(format!(
            "ime replace_and_mark_text len={} marked={:?}",
            new_text.len(),
            new_selected_range
        ));
        self.marked_text_range = if new_text.is_empty() {
            None
        } else {
            Some(new_selected_range.unwrap_or(0..new_text.encode_utf16().count()))
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(self.ime_cursor_bounds(element_bounds, window))
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
