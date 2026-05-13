use std::collections::HashSet;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event as AlacTermEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{ClipboardType, Config, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape, NamedColor, Processor, StdSyncHandler,
};
use anyhow::{Context as _, Result, ensure};
use gpui::{
    Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, FontFallbacks, Pixels, Subscription,
    Task, Window, px,
};
use parking_lot::Mutex;
use portable_pty::{Child, MasterPty, PtySize};
use serde::Serialize;
use serde_json::json;

use crate::cli::{AmbiguousWidth, CliOptions, Theme};
use crate::color::indexed_to_rgb;
use crate::color::{ansi_bg_to_hsla, ansi_to_hsla};
use crate::debug_server::{SharedDebugState, start_debug_http_server};
use crate::history_log::HistoryLogger;
use crate::input_log::InputLogger;
use crate::keyboard::encode_keystroke;
use crate::pty::{PtySession, SharedPtyWriter, write_to_pty};
use crate::render::{
    CUSTOM_TITLE_BAR_HEIGHT, STATUS_BAR_HEIGHT, TEXT_PADDING_X, TEXT_PADDING_Y, line_height_for,
    measure_cell_width,
};
use crate::snapshot_tab::SnapshotTabData;
use crate::text_utils::summarize_text_for_trace;

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const MIN_COLS: u16 = 2;
const MIN_ROWS: u16 = 1;
pub(crate) const DEFAULT_FONT_SIZE: Pixels = px(14.0);
pub(crate) const MIN_FONT_SIZE: Pixels = px(8.0);
pub(crate) const MAX_FONT_SIZE: Pixels = px(48.0);
const INPUT_TRACE_ENV: &str = "AGENT_TUI_INPUT_TRACE";
const MAX_PTY_BATCH_CHUNKS: usize = 256;
const MAX_PTY_BATCH_BYTES: usize = 256 * 1024;
const CURSOR_SLIDE_DURATION: Duration = Duration::from_millis(80);
const CURSOR_SLIDE_MAX_COL_DELTA: f32 = 8.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct AgentTerminalOptions {
    pub(crate) show_title_bar: bool,
}

impl Default for AgentTerminalOptions {
    fn default() -> Self {
        Self {
            show_title_bar: true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TerminalExitedEvent;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct GridSize {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

impl Default for GridSize {
    fn default() -> Self {
        Self {
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

#[derive(Clone)]
enum PendingTerminalEvent {
    ClipboardStore(ClipboardType, String),
    ClipboardLoad(ClipboardType, Arc<dyn Fn(&str) -> String + Sync + Send + 'static>),
}

#[derive(Clone)]
pub(crate) struct TitleTrackingListener {
    pub(crate) title: Arc<Mutex<Option<String>>>,
    pub(crate) writer: Option<SharedPtyWriter>,
    pending_events: Arc<Mutex<Vec<PendingTerminalEvent>>>,
}

impl EventListener for TitleTrackingListener {
    fn send_event(&self, event: AlacTermEvent) {
        match event {
            AlacTermEvent::Title(title) => {
                *self.title.lock() = Some(title);
            }
            AlacTermEvent::ResetTitle => {
                *self.title.lock() = None;
            }
            AlacTermEvent::PtyWrite(text) => {
                if let Some(writer) = &self.writer {
                    let _ = write_to_pty(writer, text.as_bytes());
                }
            }
            AlacTermEvent::ClipboardStore(clipboard, text) => {
                self.pending_events
                    .lock()
                    .push(PendingTerminalEvent::ClipboardStore(clipboard, text));
            }
            AlacTermEvent::ClipboardLoad(clipboard, format) => {
                self.pending_events
                    .lock()
                    .push(PendingTerminalEvent::ClipboardLoad(clipboard, format));
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
pub(crate) struct CellSnapshot {
    pub(crate) ch: char,
    pub(crate) fg: gpui::Hsla,
    pub(crate) bg: Option<gpui::Hsla>,
    pub(crate) width_cols: u8,
    pub(crate) spans_next_col: bool,
    pub(crate) expands_layout: bool,
}

impl Default for CellSnapshot {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: gpui::Hsla::default(),
            bg: None,
            width_cols: 1,
            spans_next_col: false,
            expands_layout: false,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct ScreenSnapshot {
    pub(crate) cells: Vec<Vec<CellSnapshot>>,
    pub(crate) soft_wrapped_rows: Vec<bool>,
    pub(crate) cursor_row: usize,
    pub(crate) cursor_col: usize,
    pub(crate) cursor_visible: bool,
    pub(crate) alt_screen: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SelectionPoint {
    pub(crate) row: usize,
    pub(crate) col: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct EnterLatencyProbe {
    pub(crate) id: u64,
    pub(crate) keydown_at: Instant,
    pub(crate) write_done_at: Option<Instant>,
    pub(crate) first_pty_at: Option<Instant>,
}

pub(crate) struct AgentTerminal {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) term: Term<TitleTrackingListener>,
    pub(crate) processor: Processor<StdSyncHandler>,
    pub(crate) grid_size: GridSize,
    pub(crate) snapshot: ScreenSnapshot,
    pub(crate) cursor_shape: CursorShape,
    pub(crate) force_vertical_cursor: bool,
    pub(crate) cursor_slide_enabled: bool,
    pub(crate) cursor_trail_enabled: bool,
    pub(crate) option_as_meta: bool,
    pub(crate) cursor_visual_initialized: bool,
    pub(crate) cursor_visual_row: usize,
    pub(crate) cursor_anim_from_col: f32,
    pub(crate) cursor_anim_to_col: f32,
    pub(crate) cursor_anim_started_at: Option<Instant>,
    pub(crate) shell: String,
    pub(crate) terminal_title: Arc<Mutex<Option<String>>>,
    pub(crate) show_title_bar: bool,
    pub(crate) show_status_bar: bool,
    pub(crate) theme: Theme,
    pub(crate) font_family: String,
    pub(crate) font_fallbacks: Option<FontFallbacks>,
    pub(crate) forced_double_width_chars: HashSet<char>,
    pub(crate) font_size: Pixels,
    pub(crate) master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    pub(crate) writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    pub(crate) child: Option<Arc<Mutex<Box<dyn Child + Send>>>>,
    pub(crate) input_line: String,
    pub(crate) input_cursor_utf16: usize,
    pub(crate) ime_marked_text: Option<String>,
    pub(crate) last_ax_published_line: String,
    pub(crate) last_ax_published_cursor_utf16: usize,
    pub(crate) input_trace: bool,
    pub(crate) input_logger: Option<InputLogger>,
    pub(crate) history_logger: Option<HistoryLogger>,
    pub(crate) last_local_key_event_at: Option<Instant>,
    pub(crate) pty_sample_started_at: Instant,
    pub(crate) pty_sample_bytes: usize,
    pub(crate) pty_sample_chunks: usize,
    pub(crate) last_pty_chunk_at: Option<Instant>,
    pub(crate) enter_latency_seq: u64,
    pub(crate) enter_latency_probe: Option<EnterLatencyProbe>,
    pub(crate) mouse_scroll_accum_x: f32,
    pub(crate) mouse_scroll_accum_y: f32,
    pub(crate) last_mouse_report: Option<(usize, usize, u8)>,
    pub(crate) selection_mode_active: bool,
    pub(crate) selection_button: Option<gpui::MouseButton>,
    pub(crate) selection_anchor: Option<SelectionPoint>,
    pub(crate) selection_focus: Option<SelectionPoint>,
    pub(crate) canvas_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    pub(crate) paste_guard_prompt_open: bool,
    pub(crate) shell_exited: bool,
    pub(crate) debug: SharedDebugState,
    pending_term_events: Arc<Mutex<Vec<PendingTerminalEvent>>>,
    pub(crate) _window_bounds_sub: Option<Subscription>,
    pub(crate) _focus_in_sub: Option<Subscription>,
    pub(crate) _focus_out_sub: Option<Subscription>,
    pub(crate) _pump_task: Task<Result<()>>,
}

impl AgentTerminal {
    #[allow(dead_code)]
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>, cli: CliOptions) -> Self {
        Self::new_with_options(window, cx, cli, AgentTerminalOptions::default())
    }

    pub(crate) fn new_embedded(
        window: &mut Window,
        cx: &mut Context<Self>,
        cli: CliOptions,
    ) -> Self {
        Self::new_with_options(
            window,
            cx,
            cli,
            AgentTerminalOptions {
                show_title_bar: false,
            },
        )
    }

    pub(crate) fn new_with_options(
        window: &mut Window,
        cx: &mut Context<Self>,
        cli: CliOptions,
        options: AgentTerminalOptions,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let font_size = DEFAULT_FONT_SIZE;
        let font_fallbacks = parse_font_fallbacks(&cli.font_fallbacks);
        let forced_double_width_chars = parse_double_width_chars(&cli.double_width_chars);
        let cell_width =
            measure_cell_width(window, &cli.font_family, font_fallbacks.as_ref(), font_size);
        let viewport = window.viewport_size();
        let grid_size = compute_grid_size(
            viewport,
            cell_width,
            line_height_for(font_size),
            cli.show_status_bar,
        );

        let terminal_title = Arc::new(Mutex::new(None));
        let pending_term_events = Arc::new(Mutex::new(Vec::new()));
        let (shell, master, writer, child, output_rx, debug) =
            match PtySession::spawn(grid_size.rows, grid_size.cols) {
                Ok(session) => {
                    let shell = session.shell.clone();
                    let debug =
                        SharedDebugState::new(shell.clone(), "connected".to_string(), grid_size);
                    (
                        shell,
                        Some(session.master),
                        Some(session.writer),
                        Some(session.child),
                        Some(session.output_rx),
                        debug,
                    )
                }
                Err(err) => {
                    let message = format!("failed to start shell: {err:#}");
                    let debug =
                        SharedDebugState::new("<none>".to_string(), message.clone(), grid_size);
                    debug.set_error(message);
                    (String::from("<none>"), None, None, None, None, debug)
                }
            };
        let term_config = Config {
            ambiguous_wide: matches!(cli.ambiguous_width, AmbiguousWidth::Double),
            ..Config::default()
        };
        let term = Term::new(
            term_config,
            &grid_size,
            TitleTrackingListener {
                title: terminal_title.clone(),
                writer: writer.clone(),
                pending_events: pending_term_events.clone(),
            },
        );
        let processor = Processor::<StdSyncHandler>::new();

        start_debug_http_server(debug.clone(), writer.clone());
        let input_logger = match cli.input_log_file.as_ref() {
            Some(path) => match InputLogger::new(path, cli.input_log_raw) {
                Ok(logger) => {
                    logger.log_event(
                        "logger_started",
                        json!({
                            "path": path.to_string_lossy().to_string(),
                            "raw": cli.input_log_raw,
                            "shell": shell.clone(),
                        }),
                    );
                    Some(logger)
                }
                Err(err) => {
                    debug.set_error(format!(
                        "failed to open input log file {}: {err}",
                        path.to_string_lossy()
                    ));
                    None
                }
            },
            None => None,
        };
        let history_logger = match HistoryLogger::new(&cli.history_log_dir, &shell) {
            Ok(logger) => {
                debug.set_note(Some(format!(
                    "history transcript: {}",
                    logger.path().to_string_lossy()
                )));
                Some(logger)
            }
            Err(err) => {
                debug.set_error(format!(
                    "failed to open history log dir {}: {err}",
                    cli.history_log_dir.to_string_lossy()
                ));
                None
            }
        };

        let mut this = Self {
            focus_handle,
            term,
            processor,
            grid_size,
            snapshot: ScreenSnapshot::default(),
            cursor_shape: CursorShape::Block,
            force_vertical_cursor: cli.force_vertical_cursor,
            cursor_slide_enabled: !cli.no_cursor_slide,
            cursor_trail_enabled: cli.cursor_trail,
            option_as_meta: !cli.no_option_as_meta,
            cursor_visual_initialized: false,
            cursor_visual_row: 0,
            cursor_anim_from_col: 0.0,
            cursor_anim_to_col: 0.0,
            cursor_anim_started_at: None,
            shell,
            terminal_title: terminal_title.clone(),
            show_title_bar: options.show_title_bar,
            show_status_bar: cli.show_status_bar,
            theme: cli.theme,
            font_family: cli.font_family.clone(),
            font_fallbacks,
            forced_double_width_chars,
            font_size,
            master,
            writer,
            child,
            input_line: String::new(),
            input_cursor_utf16: 0,
            ime_marked_text: None,
            last_ax_published_line: String::new(),
            last_ax_published_cursor_utf16: 0,
            input_trace: is_input_trace_enabled(),
            input_logger,
            history_logger,
            last_local_key_event_at: None,
            pty_sample_started_at: Instant::now(),
            pty_sample_bytes: 0,
            pty_sample_chunks: 0,
            last_pty_chunk_at: None,
            enter_latency_seq: 0,
            enter_latency_probe: None,
            mouse_scroll_accum_x: 0.0,
            mouse_scroll_accum_y: 0.0,
            last_mouse_report: None,
            selection_mode_active: false,
            selection_button: None,
            selection_anchor: None,
            selection_focus: None,
            canvas_bounds: Arc::new(Mutex::new(None)),
            paste_guard_prompt_open: false,
            shell_exited: false,
            debug,
            pending_term_events,
            _window_bounds_sub: None,
            _focus_in_sub: None,
            _focus_out_sub: None,
            _pump_task: Task::ready(Ok(())),
        };

        this.refresh_snapshot();
        this._window_bounds_sub = Some(cx.observe_window_bounds(window, |this, window, cx| {
            this.sync_grid_to_window(window);
            cx.notify();
        }));
        this._focus_in_sub = Some(
            cx.on_focus(&this.focus_handle, window, |this, _window, _cx| {
                if this.term.mode().contains(TermMode::FOCUS_IN_OUT) {
                    this.write_bytes(b"\x1b[I");
                }
            }),
        );
        this._focus_out_sub =
            Some(
                cx.on_focus_out(&this.focus_handle, window, |this, _event, _window, _cx| {
                    if this.term.mode().contains(TermMode::FOCUS_IN_OUT) {
                        this.write_bytes(b"\x1b[O");
                    }
                }),
            );
        this.sync_grid_to_window(window);

        if let Some(rx) = output_rx {
            this._pump_task = cx.spawn(async move |this, cx| {
                while let Ok(bytes) = rx.recv().await {
                    let mut batch = vec![bytes];
                    let mut batch_bytes = batch[0].len();

                    // Low-latency batching: process first PTY chunk immediately,
                    // then drain currently queued chunks in one UI update.
                    while batch.len() < MAX_PTY_BATCH_CHUNKS && batch_bytes < MAX_PTY_BATCH_BYTES {
                        match rx.try_recv() {
                            Ok(next) => {
                                batch_bytes = batch_bytes.saturating_add(next.len());
                                batch.push(next);
                            }
                            Err(async_channel::TryRecvError::Empty) => break,
                            Err(async_channel::TryRecvError::Closed) => break,
                        }
                    }

                    this.update(cx, |this, cx| {
                        this.ingest_batch(cx, &batch);
                        this.mark_enter_latency_first_paint();
                        cx.notify();
                    })?;
                }
                let _ = this.update(cx, |this, cx| {
                    this.shell_exited = true;
                    this.debug.set_note(Some("shell exited".to_string()));
                    cx.emit(TerminalExitedEvent);
                    cx.notify();
                });
                Ok(())
            });
        }

        this
    }

    pub(crate) fn ingest_batch(&mut self, cx: &mut Context<Self>, chunks: &[Vec<u8>]) {
        if chunks.is_empty() {
            return;
        }

        self.mark_enter_latency_first_pty();
        for chunk in chunks {
            if let Some(logger) = &self.history_logger {
                logger.record_pty_output(chunk);
            }
            self.processor.advance(&mut self.term, chunk);
            self.debug.record_bytes_from_pty(chunk.len());
            self.record_pty_ingest_diagnostics(chunk.len());
        }
        self.process_pending_terminal_events(cx);
        self.refresh_snapshot();
    }

    fn process_pending_terminal_events(&mut self, cx: &mut Context<Self>) {
        let pending_events = {
            let mut pending = self.pending_term_events.lock();
            std::mem::take(&mut *pending)
        };

        for event in pending_events {
            match event {
                PendingTerminalEvent::ClipboardStore(clipboard, text) => {
                    self.store_osc52_clipboard(cx, clipboard, text);
                }
                PendingTerminalEvent::ClipboardLoad(clipboard, format) => {
                    let text = self.load_osc52_clipboard(cx, clipboard);
                    self.write_bytes(format(&text).as_bytes());
                }
            }
        }
    }

    fn store_osc52_clipboard(
        &mut self,
        cx: &mut Context<Self>,
        clipboard: ClipboardType,
        text: String,
    ) {
        match clipboard {
            ClipboardType::Clipboard => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            ClipboardType::Selection => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
        self.debug
            .set_note(Some(format!("osc52 copied to {clipboard:?}")));
    }

    fn load_osc52_clipboard(
        &mut self,
        cx: &mut Context<Self>,
        clipboard: ClipboardType,
    ) -> String {
        let text = match clipboard {
            ClipboardType::Clipboard => cx.read_from_clipboard().and_then(|item| item.text()),
            ClipboardType::Selection => cx.read_from_clipboard().and_then(|item| item.text()),
        }
        .unwrap_or_default();

        self.debug
            .set_note(Some(format!("osc52 loaded from {clipboard:?}")));

        text
    }

    pub(crate) fn refresh_snapshot(&mut self) {
        let content = self.term.renderable_content();
        let rows = self.grid_size.rows as usize;
        let cols = self.grid_size.cols as usize;
        let alt_screen = content.mode.contains(TermMode::ALT_SCREEN);
        let mut cells = vec![
            vec![
                CellSnapshot {
                    ch: ' ',
                    fg: ansi_to_hsla(
                        AnsiColor::Named(NamedColor::Foreground),
                        content.colors,
                        Flags::empty(),
                        true,
                    ),
                    bg: None,
                    width_cols: 1,
                    spans_next_col: false,
                    expands_layout: false,
                };
                cols
            ];
            rows
        ];
        let mut soft_wrapped_rows = vec![false; rows];

        for indexed in content.display_iter {
            let row = indexed.point.line.0;
            let col = indexed.point.column.0;
            if row < 0 || col >= cols {
                continue;
            }

            let row = row as usize;
            if row >= rows {
                continue;
            }

            if indexed.cell.flags.contains(Flags::WRAPLINE) {
                soft_wrapped_rows[row] = true;
            }

            if indexed.cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            let mut fg = indexed.cell.fg;
            let mut bg = indexed.cell.bg;
            if indexed.cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }

            let ch = if indexed.cell.flags.contains(Flags::HIDDEN) {
                ' '
            } else {
                indexed.cell.c
            };
            let spans_next_col = indexed.cell.flags.contains(Flags::WIDE_CHAR);
            let expands_layout = !spans_next_col && self.forced_double_width_chars.contains(&ch);
            let width_cols = if spans_next_col || expands_layout {
                2
            } else {
                1
            };

            cells[row][col] = CellSnapshot {
                ch,
                fg: ansi_to_hsla(fg, content.colors, indexed.cell.flags, true),
                bg: ansi_bg_to_hsla(bg, content.colors),
                width_cols,
                spans_next_col,
                expands_layout,
            };
        }

        let cursor = content.cursor;
        let cursor_row = (cursor.point.line.0 + content.display_offset as i32).max(0) as usize;
        let cursor_col = cursor.point.column.0.min(cols.saturating_sub(1));
        let effective_cursor_shape =
            if self.force_vertical_cursor && cursor.shape != CursorShape::Hidden {
                CursorShape::Beam
            } else {
                cursor.shape
            };
        self.cursor_shape = effective_cursor_shape;
        self.update_cursor_visual_target(cursor_row.min(rows.saturating_sub(1)), cursor_col);

        self.snapshot = ScreenSnapshot {
            cells,
            soft_wrapped_rows,
            cursor_row: cursor_row.min(rows.saturating_sub(1)),
            cursor_col,
            cursor_visible: cursor.shape != CursorShape::Hidden,
            alt_screen,
        };

        self.debug.update_screen_snapshot(
            self.grid_size,
            self.snapshot.cursor_row,
            self.snapshot.cursor_col,
            snapshot_to_lines(&self.snapshot),
        );
    }

    pub(crate) fn sync_grid_to_window(&mut self, window: &mut Window) {
        let cell_width = measure_cell_width(
            window,
            &self.font_family,
            self.font_fallbacks.as_ref(),
            self.font_size,
        );
        let viewport = window.viewport_size();
        let new_grid = compute_grid_size(
            viewport,
            cell_width,
            self.line_height(),
            self.show_status_bar,
        );
        self.apply_grid_size(new_grid);
    }

    pub(crate) fn line_height(&self) -> Pixels {
        line_height_for(self.font_size)
    }

    pub(crate) fn adjust_font_size(&mut self, delta: Pixels, window: &mut Window) {
        let next_size = (self.font_size + delta)
            .max(MIN_FONT_SIZE)
            .min(MAX_FONT_SIZE);
        if next_size == self.font_size {
            return;
        }

        self.font_size = next_size;
        self.sync_grid_to_window(window);
    }

    pub(crate) fn apply_grid_size(&mut self, new_grid: GridSize) {
        if new_grid == self.grid_size {
            return;
        }

        self.grid_size = new_grid;
        self.term.resize(new_grid);

        if let Some(master) = &self.master
            && let Err(err) = master.lock().resize(PtySize {
                rows: new_grid.rows,
                cols: new_grid.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
        {
            self.debug.set_error(format!("pty resize failed: {err:#}"));
        }

        self.debug.record_resize();
        self.refresh_snapshot();
    }

    pub(crate) fn write_bytes(&mut self, bytes: &[u8]) {
        let Some(writer) = &self.writer else {
            self.debug
                .set_error("write skipped because PTY writer is unavailable");
            return;
        };

        match write_to_pty(writer, bytes) {
            Ok(()) => {
                self.debug.record_bytes_to_pty(bytes.len(), false);
            }
            Err(err) => {
                self.debug
                    .set_error(format!("failed to write input: {err:#}"));
            }
        }
    }

    pub(crate) fn tab_title(&self) -> String {
        self.terminal_title
            .lock()
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| self.shell.clone())
    }

    pub(crate) fn capture_snapshot_data(&self, title: String) -> SnapshotTabData {
        let grid = self.term.grid();
        let colors = self.term.colors();
        let cols = grid.columns();
        let top = grid.topmost_line().0;
        let bottom = grid.bottommost_line().0;
        let default_fg = ansi_to_hsla(
            AnsiColor::Named(NamedColor::Foreground),
            colors,
            Flags::empty(),
            true,
        );

        let mut lines = Vec::with_capacity((bottom - top + 1).max(0) as usize);
        let mut soft_wrapped_rows = Vec::with_capacity((bottom - top + 1).max(0) as usize);
        for line_index in top..=bottom {
            let mut row = vec![
                CellSnapshot {
                    ch: ' ',
                    fg: default_fg,
                    bg: None,
                    width_cols: 1,
                    spans_next_col: false,
                    expands_layout: false,
                };
                cols
            ];
            let mut soft_wrapped = false;

            for col in 0..cols {
                let cell = &grid[Line(line_index)][Column(col)];
                if cell.flags.contains(Flags::WRAPLINE) {
                    soft_wrapped = true;
                }
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }

                let mut fg = cell.fg;
                let mut bg = cell.bg;
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }

                let ch = if cell.flags.contains(Flags::HIDDEN) {
                    ' '
                } else {
                    cell.c
                };
                let spans_next_col = cell.flags.contains(Flags::WIDE_CHAR);
                let expands_layout =
                    !spans_next_col && self.forced_double_width_chars.contains(&ch);
                let width_cols = if spans_next_col || expands_layout {
                    2
                } else {
                    1
                };

                row[col] = CellSnapshot {
                    ch,
                    fg: ansi_to_hsla(fg, colors, cell.flags, true),
                    bg: ansi_bg_to_hsla(bg, colors),
                    width_cols,
                    spans_next_col,
                    expands_layout,
                };
            }

            lines.push(row);
            soft_wrapped_rows.push(soft_wrapped);
        }

        SnapshotTabData {
            title,
            lines,
            soft_wrapped_rows,
            cols,
            font_family: self.font_family.clone(),
            font_fallbacks: self.font_fallbacks.clone(),
            font_size: self.font_size,
            theme: self.theme,
        }
    }

    pub(crate) fn cursor_visual_state(&self) -> (usize, f32, bool) {
        if !self.cursor_slide_enabled {
            return (
                self.snapshot.cursor_row,
                self.snapshot.cursor_col as f32,
                false,
            );
        }

        if !self.cursor_visual_initialized {
            return (
                self.snapshot.cursor_row,
                self.snapshot.cursor_col as f32,
                false,
            );
        }

        let now = Instant::now();
        (
            self.cursor_visual_row,
            self.cursor_visual_col_at(now),
            self.cursor_animation_active_at(now),
        )
    }

    fn update_cursor_visual_target(&mut self, row: usize, col: usize) {
        let target_col = col as f32;
        let now = Instant::now();

        if !self.cursor_slide_enabled {
            self.cursor_visual_initialized = true;
            self.cursor_visual_row = row;
            self.cursor_anim_from_col = target_col;
            self.cursor_anim_to_col = target_col;
            self.cursor_anim_started_at = None;
            return;
        }

        if !self.cursor_visual_initialized {
            self.cursor_visual_initialized = true;
            self.cursor_visual_row = row;
            self.cursor_anim_from_col = target_col;
            self.cursor_anim_to_col = target_col;
            self.cursor_anim_started_at = None;
            return;
        }

        let current_col = self.cursor_visual_col_at(now);
        let row_changed = self.cursor_visual_row != row;
        let large_delta = (target_col - current_col).abs() > CURSOR_SLIDE_MAX_COL_DELTA;
        if row_changed || large_delta {
            self.cursor_visual_row = row;
            self.cursor_anim_from_col = target_col;
            self.cursor_anim_to_col = target_col;
            self.cursor_anim_started_at = None;
            return;
        }

        if (target_col - self.cursor_anim_to_col).abs() < f32::EPSILON {
            if !self.cursor_animation_active_at(now) {
                self.cursor_anim_from_col = target_col;
                self.cursor_anim_to_col = target_col;
                self.cursor_anim_started_at = None;
            }
            self.cursor_visual_row = row;
            return;
        }

        self.cursor_visual_row = row;
        self.cursor_anim_from_col = current_col;
        self.cursor_anim_to_col = target_col;
        self.cursor_anim_started_at = Some(now);
    }

    fn cursor_visual_col_at(&self, now: Instant) -> f32 {
        let Some(started_at) = self.cursor_anim_started_at else {
            return self.cursor_anim_to_col;
        };

        let elapsed = now.saturating_duration_since(started_at);
        let duration_ms = CURSOR_SLIDE_DURATION.as_millis().max(1) as f32;
        let progress = (elapsed.as_millis() as f32 / duration_ms).clamp(0.0, 1.0);
        let eased = progress * (2.0 - progress); // ease-out quad
        self.cursor_anim_from_col + (self.cursor_anim_to_col - self.cursor_anim_from_col) * eased
    }

    fn cursor_animation_active_at(&self, now: Instant) -> bool {
        let Some(started_at) = self.cursor_anim_started_at else {
            return false;
        };
        now.saturating_duration_since(started_at) < CURSOR_SLIDE_DURATION
            && (self.cursor_anim_to_col - self.cursor_anim_from_col).abs() >= f32::EPSILON
    }

    pub(crate) fn start_enter_latency_probe(&mut self, input_line: &str) -> u64 {
        if let Some(previous) = self.enter_latency_probe.take() {
            self.log_enter_latency_event(
                "enter_latency_abandoned",
                previous.id,
                &previous,
                Instant::now(),
                json!({ "reason": "superseded_by_new_enter" }),
            );
        }

        self.enter_latency_seq = self.enter_latency_seq.saturating_add(1);
        let probe = EnterLatencyProbe {
            id: self.enter_latency_seq,
            keydown_at: Instant::now(),
            write_done_at: None,
            first_pty_at: None,
        };

        self.log_enter_latency_event(
            "enter_latency_start",
            probe.id,
            &probe,
            probe.keydown_at,
            json!({
                "input_line": summarize_text_for_trace(input_line),
            }),
        );
        self.enter_latency_probe = Some(probe);
        self.enter_latency_seq
    }

    pub(crate) fn mark_enter_latency_write_done(&mut self, probe_id: u64, bytes_len: usize) {
        let now = Instant::now();
        let Some(probe) = self.enter_latency_probe.as_mut() else {
            return;
        };
        if probe.id != probe_id || probe.write_done_at.is_some() {
            return;
        }

        probe.write_done_at = Some(now);
        let snapshot = probe.clone();
        self.log_enter_latency_event(
            "enter_latency_write_done",
            probe_id,
            &snapshot,
            now,
            json!({
                "bytes_len": bytes_len,
            }),
        );
    }

    pub(crate) fn mark_enter_latency_first_pty(&mut self) {
        let now = Instant::now();
        let Some(probe) = self.enter_latency_probe.as_mut() else {
            return;
        };
        if probe.write_done_at.is_none() || probe.first_pty_at.is_some() {
            return;
        }

        probe.first_pty_at = Some(now);
        let snapshot = probe.clone();
        self.log_enter_latency_event(
            "enter_latency_first_pty",
            snapshot.id,
            &snapshot,
            now,
            json!({}),
        );
    }

    pub(crate) fn mark_enter_latency_first_paint(&mut self) {
        let now = Instant::now();
        let Some(probe) = self.enter_latency_probe.take() else {
            return;
        };
        if probe.first_pty_at.is_none() {
            self.enter_latency_probe = Some(probe);
            return;
        }

        self.log_enter_latency_event(
            "enter_latency_first_paint",
            probe.id,
            &probe,
            now,
            json!({}),
        );
    }

    fn log_enter_latency_event(
        &self,
        event: &str,
        probe_id: u64,
        probe: &EnterLatencyProbe,
        at: Instant,
        extra: serde_json::Value,
    ) {
        let Some(logger) = &self.input_logger else {
            return;
        };

        let keydown_to_now_ms = at.saturating_duration_since(probe.keydown_at).as_millis();
        let keydown_to_write_ms = probe
            .write_done_at
            .map(|t| t.saturating_duration_since(probe.keydown_at).as_millis());
        let write_to_now_ms = probe
            .write_done_at
            .map(|t| at.saturating_duration_since(t).as_millis());
        let keydown_to_first_pty_ms = probe
            .first_pty_at
            .map(|t| t.saturating_duration_since(probe.keydown_at).as_millis());
        let first_pty_to_now_ms = probe
            .first_pty_at
            .map(|t| at.saturating_duration_since(t).as_millis());

        logger.log_event(
            event,
            json!({
                "probe_id": probe_id,
                "keydown_to_now_ms": keydown_to_now_ms,
                "keydown_to_write_ms": keydown_to_write_ms,
                "write_to_now_ms": write_to_now_ms,
                "keydown_to_first_pty_ms": keydown_to_first_pty_ms,
                "first_pty_to_now_ms": first_pty_to_now_ms,
                "extra": extra,
            }),
        );
    }

    fn record_pty_ingest_diagnostics(&mut self, chunk_len: usize) {
        let Some(logger) = &self.input_logger else {
            return;
        };

        let now = Instant::now();
        if let Some(last_chunk_at) = self.last_pty_chunk_at {
            let gap = now.saturating_duration_since(last_chunk_at);
            if gap >= Duration::from_millis(800) {
                logger.log_event(
                    "pty_ingest_gap",
                    json!({
                        "gap_ms": gap.as_millis(),
                        "chunk_len": chunk_len,
                    }),
                );
            }
        }
        self.last_pty_chunk_at = Some(now);

        self.pty_sample_bytes += chunk_len;
        self.pty_sample_chunks += 1;

        let window = now.saturating_duration_since(self.pty_sample_started_at);
        if window < Duration::from_millis(500) {
            return;
        }

        let window_ms = window.as_millis().max(1);
        let bytes = self.pty_sample_bytes as u128;
        let chunks = self.pty_sample_chunks as u128;

        logger.log_event(
            "pty_ingest_sample",
            json!({
                "window_ms": window_ms,
                "bytes": bytes,
                "chunks": chunks,
                "bytes_per_sec": bytes.saturating_mul(1000) / window_ms,
                "chunks_per_sec": chunks.saturating_mul(1000) / window_ms,
            }),
        );

        self.pty_sample_started_at = now;
        self.pty_sample_bytes = 0;
        self.pty_sample_chunks = 0;
    }
}

impl EventEmitter<TerminalExitedEvent> for AgentTerminal {}

impl Drop for AgentTerminal {
    fn drop(&mut self) {
        if let Some(child) = &self.child {
            let _ = child.lock().kill();
        }
    }
}

pub(crate) fn compute_grid_size(
    viewport: gpui::Size<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    show_status_bar: bool,
) -> GridSize {
    let mut usable_width = viewport.width - (TEXT_PADDING_X * 2.0);
    let status_height = if show_status_bar {
        STATUS_BAR_HEIGHT
    } else {
        px(0.0)
    };
    let mut usable_height =
        viewport.height - CUSTOM_TITLE_BAR_HEIGHT - status_height - (TEXT_PADDING_Y * 2.0);

    if usable_width < cell_width {
        usable_width = cell_width;
    }
    if usable_height < line_height {
        usable_height = line_height;
    }

    let cols = ((usable_width / cell_width).floor() as u32).max(MIN_COLS as u32) as u16;
    let rows = ((usable_height / line_height).floor() as u32).max(MIN_ROWS as u32) as u16;

    GridSize { cols, rows }
}

pub(crate) fn snapshot_to_lines(snapshot: &ScreenSnapshot) -> Vec<String> {
    snapshot
        .cells
        .iter()
        .map(|row| {
            let mut line: String = row.iter().map(|cell| cell.ch).collect();
            while line.ends_with(' ') {
                line.pop();
            }
            line
        })
        .collect()
}

fn is_input_trace_enabled() -> bool {
    std::env::var(INPUT_TRACE_ENV)
        .ok()
        .map(|value| {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

fn parse_font_fallbacks(raw: &[String]) -> Option<FontFallbacks> {
    let fallbacks: Vec<String> = raw
        .iter()
        .map(|font| font.trim())
        .filter(|font| !font.is_empty())
        .map(|font| font.to_string())
        .collect();
    if fallbacks.is_empty() {
        None
    } else {
        Some(FontFallbacks::from_fonts(fallbacks))
    }
}

fn parse_double_width_chars(raw: &[String]) -> HashSet<char> {
    raw.iter()
        .flat_map(|entry| entry.chars())
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::Osc52;
    use std::io::{Result as IoResult, Write};

    struct RecordingWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
            self.bytes.lock().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> IoResult<()> {
            Ok(())
        }
    }

    fn recording_writer() -> (SharedPtyWriter, Arc<Mutex<Vec<u8>>>) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer: SharedPtyWriter = Arc::new(Mutex::new(Box::new(RecordingWriter {
            bytes: bytes.clone(),
        })));
        (writer, bytes)
    }

    fn make_listener(
        writer: Option<SharedPtyWriter>,
    ) -> (TitleTrackingListener, Arc<Mutex<Vec<PendingTerminalEvent>>>) {
        let title = Arc::new(Mutex::new(None));
        let pending_events = Arc::new(Mutex::new(Vec::new()));
        let listener = TitleTrackingListener {
            title,
            writer,
            pending_events: pending_events.clone(),
        };
        (listener, pending_events)
    }

    #[test]
    fn device_status_report_writes_cursor_position_to_pty() {
        let (writer, bytes) = recording_writer();
        let (listener, _) = make_listener(Some(writer));
        let mut term = Term::new(
            Config::default(),
            &GridSize { cols: 80, rows: 24 },
            listener,
        );
        let mut processor = Processor::<StdSyncHandler>::new();

        processor.advance(&mut term, b"abc");
        processor.advance(&mut term, b"\x1b[6n");

        assert_eq!(&*bytes.lock(), b"\x1b[1;4R");
    }

    #[test]
    fn osc52_copy_sequence_is_queued_for_clipboard_store() {
        let (listener, pending_events) = make_listener(None);
        let mut term = Term::new(
            Config::default(),
            &GridSize { cols: 80, rows: 24 },
            listener,
        );
        let mut processor = Processor::<StdSyncHandler>::new();

        processor.advance(&mut term, b"\x1b]52;c;SGVsbG8=\x07");

        let events = pending_events.lock().clone();
        assert_eq!(events.len(), 1);
        match &events[0] {
            PendingTerminalEvent::ClipboardStore(ClipboardType::Clipboard, text) => {
                assert_eq!(text, "Hello");
            }
            _ => panic!("expected clipboard store event"),
        }
    }

    #[test]
    fn osc52_paste_query_is_queued_for_clipboard_load() {
        let config = Config {
            osc52: Osc52::CopyPaste,
            ..Config::default()
        };
        let (listener, pending_events) = make_listener(None);
        let mut term = Term::new(config, &GridSize { cols: 80, rows: 24 }, listener);
        let mut processor = Processor::<StdSyncHandler>::new();

        processor.advance(&mut term, b"\x1b]52;c;?\x07");

        let events = pending_events.lock().clone();
        assert_eq!(events.len(), 1);
        match &events[0] {
            PendingTerminalEvent::ClipboardLoad(ClipboardType::Clipboard, _) => {}
            _ => panic!("expected clipboard load event"),
        }
    }
}
pub(crate) fn run_self_check() -> Result<()> {
    let enter = gpui::Keystroke::parse("enter").context("parse enter")?;
    ensure!(
        encode_keystroke(&enter) == Some(vec![b'\r']),
        "enter keystroke encoding mismatch"
    );

    let alt_x = gpui::Keystroke::parse("alt-x").context("parse alt-x")?;
    ensure!(
        encode_keystroke(&alt_x) == Some(vec![0x1b, b'x']),
        "alt-x keystroke encoding mismatch"
    );

    let ctrl_c = gpui::Keystroke::parse("ctrl-c").context("parse ctrl-c")?;
    ensure!(
        encode_keystroke(&ctrl_c) == Some(vec![3]),
        "ctrl-c keystroke encoding mismatch"
    );

    let chinese = gpui::Keystroke {
        modifiers: gpui::Modifiers::none(),
        key: "x".to_string(),
        key_char: Some("你".to_string()),
    };
    ensure!(
        encode_keystroke(&chinese) == Some("你".as_bytes().to_vec()),
        "chinese ime keystroke encoding mismatch"
    );

    let ime_in_progress = gpui::Keystroke {
        modifiers: gpui::Modifiers::none(),
        key: "a".to_string(),
        key_char: None,
    };
    ensure!(
        encode_keystroke(&ime_in_progress).is_none(),
        "ime in-progress keystroke should not emit bytes"
    );

    let cmd_v = gpui::Keystroke {
        modifiers: gpui::Modifiers {
            platform: true,
            ..gpui::Modifiers::none()
        },
        key: "v".to_string(),
        key_char: Some("v".to_string()),
    };
    ensure!(
        encode_keystroke(&cmd_v).is_none(),
        "cmd-v should not be forwarded as raw character"
    );

    let default_colors = alacritty_terminal::term::color::Colors::default();
    ensure!(
        indexed_to_rgb(16, &default_colors) == (0, 0, 0),
        "indexed color 16 mismatch"
    );
    ensure!(
        indexed_to_rgb(231, &default_colors) == (255, 255, 255),
        "indexed color 231 mismatch"
    );

    let grid = compute_grid_size(
        gpui::size(gpui::px(1000.0), gpui::px(520.0)),
        gpui::px(8.0),
        line_height_for(DEFAULT_FONT_SIZE),
        false,
    );
    ensure!(
        grid.cols >= 80,
        "computed columns too small for 1000px viewport"
    );
    ensure!(
        grid.rows >= 20,
        "computed rows too small for 520px viewport"
    );

    println!(
        "self-check passed: keyboard/color/grid invariants OK (cols={}, rows={})",
        grid.cols, grid.rows
    );

    Ok(())
}
