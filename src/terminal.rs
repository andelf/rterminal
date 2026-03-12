use std::io::Write;
use std::ops::Range;
use std::sync::Arc;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event as AlacTermEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape, NamedColor, Processor, StdSyncHandler,
};
use anyhow::{Context as _, Result, ensure};
use gpui::{Context, FocusHandle, Pixels, Subscription, Task, Window, px};
use parking_lot::Mutex;
use portable_pty::{Child, MasterPty, PtySize};
use serde::Serialize;

use crate::cli::CliOptions;
use crate::color::{ansi_bg_to_hsla, ansi_to_hsla};
use crate::debug_server::{SharedDebugState, start_debug_http_server};
use crate::keyboard::encode_keystroke;
use crate::pty::{PtySession, write_to_pty};
use crate::render::{
    CUSTOM_TITLE_BAR_HEIGHT, STATUS_BAR_ESTIMATED_HEIGHT, TEXT_PADDING_X, TEXT_PADDING_Y,
    line_height_for, measure_cell_width,
};
use crate::color::indexed_to_rgb;

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const MIN_COLS: u16 = 2;
const MIN_ROWS: u16 = 1;
pub(crate) const DEFAULT_FONT_SIZE: Pixels = px(14.0);
pub(crate) const MIN_FONT_SIZE: Pixels = px(8.0);
pub(crate) const MAX_FONT_SIZE: Pixels = px(48.0);
const INPUT_TRACE_ENV: &str = "AGENT_TUI_INPUT_TRACE";

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
pub(crate) struct TitleTrackingListener {
    pub(crate) title: Arc<Mutex<Option<String>>>,
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
}

impl Default for CellSnapshot {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: gpui::Hsla::default(),
            bg: None,
            width_cols: 1,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct ScreenSnapshot {
    pub(crate) cells: Vec<Vec<CellSnapshot>>,
    pub(crate) cursor_row: usize,
    pub(crate) cursor_col: usize,
    pub(crate) cursor_visible: bool,
    pub(crate) alt_screen: bool,
}

pub(crate) struct AgentTerminal {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) term: Term<TitleTrackingListener>,
    pub(crate) processor: Processor<StdSyncHandler>,
    pub(crate) grid_size: GridSize,
    pub(crate) snapshot: ScreenSnapshot,
    pub(crate) shell: String,
    pub(crate) terminal_title: Arc<Mutex<Option<String>>>,
    pub(crate) show_status_bar: bool,
    pub(crate) font_family: String,
    pub(crate) font_size: Pixels,
    pub(crate) master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    pub(crate) writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    pub(crate) child: Option<Arc<Mutex<Box<dyn Child + Send>>>>,
    pub(crate) input_line: String,
    pub(crate) input_cursor_utf16: usize,
    pub(crate) marked_text_range: Option<Range<usize>>,
    pub(crate) last_ax_published_line: String,
    pub(crate) last_ax_published_cursor_utf16: usize,
    pub(crate) input_trace: bool,
    pub(crate) mouse_scroll_accum_x: f32,
    pub(crate) mouse_scroll_accum_y: f32,
    pub(crate) last_mouse_report: Option<(usize, usize, u8)>,
    pub(crate) paste_guard_prompt_open: bool,
    pub(crate) debug: SharedDebugState,
    pub(crate) _window_bounds_sub: Option<Subscription>,
    pub(crate) _pump_task: Task<Result<()>>,
}

impl AgentTerminal {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>, cli: CliOptions) -> Self {
        let focus_handle = cx.focus_handle();
        let font_size = DEFAULT_FONT_SIZE;
        let cell_width = measure_cell_width(window, &cli.font_family, font_size);
        let viewport = window.viewport_size();
        let grid_size = compute_grid_size(
            viewport,
            cell_width,
            line_height_for(font_size),
            cli.show_status_bar,
        );

        let terminal_title = Arc::new(Mutex::new(None));
        let term = Term::new(
            Config::default(),
            &grid_size,
            TitleTrackingListener {
                title: terminal_title.clone(),
            },
        );
        let processor = Processor::<StdSyncHandler>::new();
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
                    let debug = SharedDebugState::new("<none>".to_string(), message.clone(), grid_size);
                    debug.set_error(message);
                    (String::from("<none>"), None, None, None, None, debug)
                }
            };

        start_debug_http_server(debug.clone(), writer.clone());

        let mut this = Self {
            focus_handle,
            term,
            processor,
            grid_size,
            snapshot: ScreenSnapshot::default(),
            shell,
            terminal_title: terminal_title.clone(),
            show_status_bar: cli.show_status_bar,
            font_family: cli.font_family.clone(),
            font_size,
            master,
            writer,
            child,
            input_line: String::new(),
            input_cursor_utf16: 0,
            marked_text_range: None,
            last_ax_published_line: String::new(),
            last_ax_published_cursor_utf16: 0,
            input_trace: is_input_trace_enabled(),
            mouse_scroll_accum_x: 0.0,
            mouse_scroll_accum_y: 0.0,
            last_mouse_report: None,
            paste_guard_prompt_open: false,
            debug,
            _window_bounds_sub: None,
            _pump_task: Task::ready(Ok(())),
        };

        this.refresh_snapshot();
        this._window_bounds_sub = Some(cx.observe_window_bounds(window, |this, window, cx| {
            this.sync_grid_to_window(window);
            cx.notify();
        }));
        this.sync_grid_to_window(window);

        if let Some(rx) = output_rx {
            this._pump_task = cx.spawn(async move |this, cx| {
                while let Ok(bytes) = rx.recv().await {
                    this.update(cx, |this, cx| {
                        this.ingest(&bytes);
                        cx.notify();
                    })?;
                }
                let _ = this.update(cx, |this, cx| {
                    this.debug
                        .set_note(Some("shell exited, closing application".to_string()));
                    cx.quit();
                });
                Ok(())
            });
        }

        this
    }

    pub(crate) fn ingest(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
        self.debug.record_bytes_from_pty(bytes.len());
        self.refresh_snapshot();
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
                };
                cols
            ];
            rows
        ];

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

            if indexed.cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            let mut fg = indexed.cell.fg;
            let mut bg = indexed.cell.bg;
            if indexed.cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }

            cells[row][col] = CellSnapshot {
                ch: if indexed.cell.flags.contains(Flags::HIDDEN) {
                    ' '
                } else {
                    indexed.cell.c
                },
                fg: ansi_to_hsla(fg, content.colors, indexed.cell.flags, true),
                bg: ansi_bg_to_hsla(bg, content.colors),
                width_cols: cell_display_width_cols(indexed.cell.flags),
            };
        }

        let cursor = content.cursor;
        let cursor_row = (cursor.point.line.0 + content.display_offset as i32).max(0) as usize;
        let cursor_col = cursor.point.column.0.min(cols.saturating_sub(1));

        self.snapshot = ScreenSnapshot {
            cells,
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
        let cell_width = measure_cell_width(window, &self.font_family, self.font_size);
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

        if let Some(master) = &self.master {
            if let Err(err) = master.lock().resize(PtySize {
                rows: new_grid.rows,
                cols: new_grid.cols,
                pixel_width: 0,
                pixel_height: 0,
            }) {
                self.debug.set_error(format!("pty resize failed: {err:#}"));
            }
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
}

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
        STATUS_BAR_ESTIMATED_HEIGHT
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

pub(crate) fn cell_display_width_cols(flags: Flags) -> u8 {
    if flags.contains(Flags::WIDE_CHAR) {
        2
    } else {
        1
    }
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
