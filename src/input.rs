use std::ops::Range;

use alacritty_terminal::term::TermMode;
use gpui::{
    App, Bounds, Context, EntityInputHandler, InputHandler, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PromptLevel, ScrollDelta,
    ScrollWheelEvent, UTF16Selection, Window, point, px, size,
};

use crate::AgentTerminal;
use crate::keyboard::{
    encode_keystroke, is_paste_shortcut, is_select_all_shortcut, is_zoom_in_shortcut,
    is_zoom_out_shortcut, should_defer_to_text_input,
};
use crate::macos_ax::NativeAxInputState;
use crate::render::{
    CUSTOM_TITLE_BAR_HEIGHT, STATUS_BAR_ESTIMATED_HEIGHT, TEXT_PADDING_X, TEXT_PADDING_Y,
    measure_cell_width,
};
use crate::text_utils::{
    delete_next_word_utf16, delete_previous_word_utf16, delete_to_end_utf16, replace_range_utf16,
    summarize_text_for_trace, utf16_substring, utf16_to_byte_index,
};

const FONT_SIZE_STEP: f32 = 1.0;
const PASTE_GUARD_MIN_LINES: usize = 4;
const PASTE_GUARD_MIN_CHARS: usize = 120;
const PASTE_GUARD_NON_ASCII_RATIO: f32 = 0.35;

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
                    measure_cell_width(window, &self.font_family, self.font_size).max(px(1.0)),
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
        let status_height = if self.show_status_bar {
            STATUS_BAR_ESTIMATED_HEIGHT
        } else {
            px(0.0)
        };
        let origin = point(
            TEXT_PADDING_X,
            CUSTOM_TITLE_BAR_HEIGHT + status_height + TEXT_PADDING_Y,
        );

        let cell_width = measure_cell_width(window, &self.font_family, self.font_size).max(px(1.0));
        let line_height = self.line_height().max(px(1.0));

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
                                    this.write_text_input(&text_to_paste);
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
        format!("agent terminal | {}", self.tab_title())
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

#[cfg(test)]
mod tests {
    use super::evaluate_paste_risk;

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
}
