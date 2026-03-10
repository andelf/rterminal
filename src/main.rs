use std::io::{Read, Write};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event as AlacTermEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Config;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Processor, StdSyncHandler};
use anyhow::{Context as _, Result, ensure};
use async_channel::Receiver;
use gpui::{
    App, Bounds, Context, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Render,
    Subscription, Task, Window, WindowBounds, WindowOptions, black, canvas, div, fill, font, point,
    prelude::*, px, rgb, rgba, size,
};
use gpui_platform::application;
use parking_lot::Mutex;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tiny_http::{Header, Response, Server, StatusCode};

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const MIN_COLS: u16 = 2;
const MIN_ROWS: u16 = 1;
const FONT_SIZE: Pixels = px(14.0);
const LINE_HEIGHT: Pixels = px(18.0);
const TEXT_PADDING_X: Pixels = px(12.0);
const TEXT_PADDING_Y: Pixels = px(12.0);
const HEADER_ESTIMATED_HEIGHT: Pixels = px(42.0);
const DEBUG_HTTP_DEFAULT_ADDR: &str = "127.0.0.1:7878";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
struct GridSize {
    cols: u16,
    rows: u16,
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
struct NoopListener;

impl EventListener for NoopListener {
    fn send_event(&self, _event: AlacTermEvent) {}
}

#[derive(Clone)]
struct CellSnapshot {
    ch: char,
    fg: gpui::Hsla,
    bg: Option<gpui::Hsla>,
}

impl Default for CellSnapshot {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: gpui::Hsla::default(),
            bg: None,
        }
    }
}

#[derive(Clone, Default)]
struct ScreenSnapshot {
    cells: Vec<Vec<CellSnapshot>>,
    cursor_row: usize,
    cursor_col: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
struct DebugCounters {
    bytes_from_pty: u64,
    bytes_to_pty: u64,
    key_events: u64,
    injected_events: u64,
    resize_events: u64,
    http_requests: u64,
}

#[derive(Clone, Debug)]
struct DebugState {
    started_at: Instant,
    listening_addr: Option<String>,
    shell: String,
    status: String,
    note: Option<String>,
    grid_size: GridSize,
    cursor_row: usize,
    cursor_col: usize,
    screen_lines: Vec<String>,
    counters: DebugCounters,
    last_error: Option<String>,
}

#[derive(Serialize)]
struct DebugStateSnapshot {
    shell: String,
    status: String,
    note: Option<String>,
    listening_addr: Option<String>,
    grid_size: GridSize,
    cursor_row: usize,
    cursor_col: usize,
    screen_lines: Vec<String>,
    counters: DebugCounters,
    uptime_ms: u128,
    last_error: Option<String>,
}

#[derive(Clone)]
struct SharedDebugState {
    inner: Arc<Mutex<DebugState>>,
}

impl SharedDebugState {
    fn new(shell: String, status: String, grid_size: GridSize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DebugState {
                started_at: Instant::now(),
                listening_addr: None,
                shell,
                status,
                note: None,
                grid_size,
                cursor_row: 0,
                cursor_col: 0,
                screen_lines: Vec::new(),
                counters: DebugCounters::default(),
                last_error: None,
            })),
        }
    }

    fn set_listening_addr(&self, addr: String) {
        self.inner.lock().listening_addr = Some(addr);
    }

    fn set_error(&self, err: impl Into<String>) {
        self.inner.lock().last_error = Some(err.into());
    }

    fn set_note(&self, note: Option<String>) {
        self.inner.lock().note = note;
    }

    fn note(&self) -> Option<String> {
        self.inner.lock().note.clone()
    }

    fn record_http_request(&self) {
        self.inner.lock().counters.http_requests += 1;
    }

    fn record_bytes_from_pty(&self, bytes: usize) {
        self.inner.lock().counters.bytes_from_pty += bytes as u64;
    }

    fn record_bytes_to_pty(&self, bytes: usize, injected: bool) {
        let mut state = self.inner.lock();
        state.counters.bytes_to_pty += bytes as u64;
        if injected {
            state.counters.injected_events += 1;
        }
    }

    fn record_key_event(&self) {
        self.inner.lock().counters.key_events += 1;
    }

    fn record_resize(&self) {
        self.inner.lock().counters.resize_events += 1;
    }

    fn update_screen_snapshot(&self, grid_size: GridSize, snapshot: &ScreenSnapshot) {
        let mut state = self.inner.lock();
        state.grid_size = grid_size;
        state.cursor_row = snapshot.cursor_row;
        state.cursor_col = snapshot.cursor_col;
        state.screen_lines = snapshot_to_lines(snapshot);
    }

    fn status_summary(&self) -> String {
        let state = self.inner.lock();
        let uptime = state.started_at.elapsed().as_secs();
        let addr = state.listening_addr.as_deref().unwrap_or("starting");
        format!(
            "{} | {}x{} | in:{} out:{} key:{} inj:{} req:{} resize:{} up:{}s dbg:{}",
            state.status,
            state.grid_size.cols,
            state.grid_size.rows,
            state.counters.bytes_from_pty,
            state.counters.bytes_to_pty,
            state.counters.key_events,
            state.counters.injected_events,
            state.counters.http_requests,
            state.counters.resize_events,
            uptime,
            addr,
        )
    }

    fn state_json(&self) -> String {
        let state = self.inner.lock();
        let snapshot = DebugStateSnapshot {
            shell: state.shell.clone(),
            status: state.status.clone(),
            note: state.note.clone(),
            listening_addr: state.listening_addr.clone(),
            grid_size: state.grid_size,
            cursor_row: state.cursor_row,
            cursor_col: state.cursor_col,
            screen_lines: state.screen_lines.clone(),
            counters: state.counters.clone(),
            uptime_ms: state.started_at.elapsed().as_millis(),
            last_error: state.last_error.clone(),
        };

        serde_json::to_string_pretty(&snapshot)
            .unwrap_or_else(|_| "{\"error\":\"serialize failed\"}".to_string())
    }

    fn screen_text(&self) -> String {
        let state = self.inner.lock();
        if state.screen_lines.is_empty() {
            return "<empty screen>\n".to_string();
        }

        let mut out = state.screen_lines.join("\n");
        out.push('\n');
        out
    }
}

struct PtySession {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send>>>,
    output_rx: Receiver<Vec<u8>>,
    shell: String,
}

struct AgentTerminal {
    focus_handle: FocusHandle,
    term: Term<NoopListener>,
    processor: Processor<StdSyncHandler>,
    grid_size: GridSize,
    snapshot: ScreenSnapshot,
    shell: String,
    master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    child: Option<Arc<Mutex<Box<dyn Child + Send>>>>,
    debug: SharedDebugState,
    _window_bounds_sub: Option<Subscription>,
    _pump_task: Task<Result<()>>,
}

impl AgentTerminal {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let cell_width = measure_cell_width(window);
        let viewport = window.viewport_size();
        let grid_size = compute_grid_size(viewport, cell_width);

        let term = Term::new(Config::default(), &grid_size, NoopListener);
        let processor = Processor::<StdSyncHandler>::new();

        match PtySession::spawn(grid_size) {
            Ok(session) => {
                let shell = session.shell.clone();
                let master = Some(session.master);
                let writer = Some(session.writer);
                let child = Some(session.child);
                let rx = session.output_rx;
                let debug =
                    SharedDebugState::new(shell.clone(), "connected".to_string(), grid_size);
                start_debug_http_server(debug.clone(), writer.clone());

                let mut this = Self {
                    focus_handle,
                    term,
                    processor,
                    grid_size,
                    snapshot: ScreenSnapshot::default(),
                    shell,
                    master,
                    writer,
                    child,
                    debug,
                    _window_bounds_sub: None,
                    _pump_task: Task::ready(Ok(())),
                };

                this.refresh_snapshot();
                this._window_bounds_sub =
                    Some(cx.observe_window_bounds(window, |this, window, cx| {
                        this.sync_grid_to_window(window);
                        cx.notify();
                    }));
                this.sync_grid_to_window(window);

                this._pump_task = cx.spawn(async move |this, cx| {
                    while let Ok(bytes) = rx.recv().await {
                        this.update(cx, |this, cx| {
                            this.ingest(&bytes);
                            cx.notify();
                        })?;
                    }
                    Ok(())
                });

                this
            }
            Err(err) => {
                let debug = SharedDebugState::new(
                    "<none>".to_string(),
                    format!("failed to start shell: {err:#}"),
                    grid_size,
                );
                debug.set_error(format!("failed to start shell: {err:#}"));
                start_debug_http_server(debug.clone(), None);

                let mut this = Self {
                    focus_handle,
                    term,
                    processor,
                    grid_size,
                    snapshot: ScreenSnapshot::default(),
                    shell: "<none>".to_string(),
                    master: None,
                    writer: None,
                    child: None,
                    debug,
                    _window_bounds_sub: None,
                    _pump_task: Task::ready(Ok(())),
                };

                this.refresh_snapshot();
                this._window_bounds_sub =
                    Some(cx.observe_window_bounds(window, |this, window, cx| {
                        this.sync_grid_to_window(window);
                        cx.notify();
                    }));
                this.sync_grid_to_window(window);
                this
            }
        }
    }

    fn ingest(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
        self.debug.record_bytes_from_pty(bytes.len());
        self.refresh_snapshot();
    }

    fn refresh_snapshot(&mut self) {
        let content = self.term.renderable_content();
        let rows = self.grid_size.rows as usize;
        let cols = self.grid_size.cols as usize;
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
            };
        }

        let cursor = content.cursor;
        let cursor_row = (cursor.point.line.0 + content.display_offset as i32).max(0) as usize;
        let cursor_col = cursor.point.column.0.min(cols.saturating_sub(1));

        self.snapshot = ScreenSnapshot {
            cells,
            cursor_row: cursor_row.min(rows.saturating_sub(1)),
            cursor_col,
        };

        self.debug
            .update_screen_snapshot(self.grid_size, &self.snapshot);
    }

    fn sync_grid_to_window(&mut self, window: &mut Window) {
        let cell_width = measure_cell_width(window);
        let viewport = window.viewport_size();
        let new_grid = compute_grid_size(viewport, cell_width);
        self.apply_grid_size(new_grid);
    }

    fn apply_grid_size(&mut self, new_grid: GridSize) {
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

    fn write_bytes(&mut self, bytes: &[u8]) {
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

    fn on_mouse_down(
        &mut self,
        _event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bytes) = encode_keystroke(&event.keystroke) {
            self.debug.record_key_event();
            self.write_bytes(&bytes);
            cx.stop_propagation();
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

impl Render for AgentTerminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.snapshot.clone();
        let focused = self.focus_handle.is_focused(window);
        let status = self.debug.status_summary();
        let shell = self.shell.clone();
        let note = self.debug.note();

        let title = if let Some(note) = note {
            format!("agent terminal | {} | {} | note: {}", shell, status, note)
        } else {
            format!("agent terminal | {} | {}", shell, status)
        };

        div()
            .id("agent-terminal")
            .size_full()
            .bg(rgb(0x0f1115))
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .bg(rgb(0x171a21))
                    .text_color(rgb(0xa9b1c6))
                    .child(title),
            )
            .child(
                canvas(
                    move |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.paint_quad(fill(bounds, black()));

                        let mono = font("Menlo");
                        let font_size = FONT_SIZE;
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
                            let y = origin.y + row_index as f32 * LINE_HEIGHT;

                            for (col_index, cell) in row.iter().enumerate() {
                                let x = origin.x + col_index as f32 * cell_width;
                                let cell_origin = point(x, y);

                                if let Some(bg) = cell.bg {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            cell_origin,
                                            size(cell_width.max(px(2.0)), LINE_HEIGHT),
                                        ),
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
                                        Some(cell_width),
                                    );
                                    let _ = shaped.paint(
                                        cell_origin,
                                        LINE_HEIGHT,
                                        gpui::TextAlign::Left,
                                        None,
                                        window,
                                        cx,
                                    );
                                }
                            }
                        }

                        if focused {
                            let cursor_origin = point(
                                origin.x + snapshot.cursor_col as f32 * cell_width,
                                origin.y + snapshot.cursor_row as f32 * LINE_HEIGHT,
                            );
                            window.paint_quad(fill(
                                Bounds::new(
                                    cursor_origin,
                                    size(cell_width.max(px(2.0)), LINE_HEIGHT),
                                ),
                                rgba(0x3b82f659),
                            ));
                        }
                    },
                )
                .size_full(),
            )
    }
}

impl PtySession {
    fn spawn(grid_size: GridSize) -> Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: grid_size.rows,
                cols: grid_size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to create PTY")?;

        let mut command = CommandBuilder::new(shell.clone());
        command.arg("-i");

        let child = pair
            .slave
            .spawn_command(command)
            .context("failed to spawn shell")?;

        let master = Arc::new(Mutex::new(pair.master));
        let writer = {
            let writer = master
                .lock()
                .take_writer()
                .context("failed to get PTY writer")?;
            Arc::new(Mutex::new(writer))
        };

        let mut reader = master
            .lock()
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let (tx, rx) = async_channel::unbounded();
        thread::spawn(move || {
            let mut buf = vec![0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(read) => {
                        if tx.send_blocking(buf[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            master,
            writer,
            child: Arc::new(Mutex::new(child)),
            output_rx: rx,
            shell,
        })
    }
}

fn write_to_pty(writer: &Arc<Mutex<Box<dyn Write + Send>>>, bytes: &[u8]) -> Result<()> {
    let mut writer = writer.lock();
    writer
        .write_all(bytes)
        .context("failed to write bytes to PTY")?;
    writer.flush().context("failed to flush PTY writer")?;
    Ok(())
}

fn measure_cell_width(window: &mut Window) -> Pixels {
    let mono = font("Menlo");
    let font_id = window.text_system().resolve_font(&mono);
    window
        .text_system()
        .advance(font_id, FONT_SIZE, 'M')
        .map(|advance| advance.width)
        .unwrap_or(px(8.0))
}

fn compute_grid_size(viewport: gpui::Size<Pixels>, cell_width: Pixels) -> GridSize {
    let mut usable_width = viewport.width - (TEXT_PADDING_X * 2.0);
    let mut usable_height = viewport.height - HEADER_ESTIMATED_HEIGHT - (TEXT_PADDING_Y * 2.0);

    if usable_width < cell_width {
        usable_width = cell_width;
    }
    if usable_height < LINE_HEIGHT {
        usable_height = LINE_HEIGHT;
    }

    let cols = ((usable_width / cell_width).floor() as u32).max(MIN_COLS as u32) as u16;
    let rows = ((usable_height / LINE_HEIGHT).floor() as u32).max(MIN_ROWS as u32) as u16;

    GridSize { cols, rows }
}

fn snapshot_to_lines(snapshot: &ScreenSnapshot) -> Vec<String> {
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

fn start_debug_http_server(
    debug: SharedDebugState,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
) {
    let addr = std::env::var("AGENT_TUI_DEBUG_ADDR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEBUG_HTTP_DEFAULT_ADDR.to_string());
    start_debug_http_server_at_addr(debug, writer, addr);
}

fn start_debug_http_server_at_addr(
    debug: SharedDebugState,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    addr: String,
) {
    let _ = thread::Builder::new()
        .name("agent-debug-http".to_string())
        .spawn(move || {
            let server = match Server::http(&addr) {
                Ok(server) => server,
                Err(err) => {
                    debug.set_error(format!("failed to start debug server on {addr}: {err}"));
                    return;
                }
            };

            debug.set_listening_addr(addr.clone());
            eprintln!("debug http listening on http://{addr}");

            for mut request in server.incoming_requests() {
                debug.record_http_request();
                let method = request.method().as_str().to_string();
                let path = request.url().split('?').next().unwrap_or("/").to_string();

                let response =
                    handle_debug_request(&mut request, &method, &path, &debug, writer.as_ref());

                if let Err(err) = request.respond(response) {
                    debug.set_error(format!("failed to send HTTP response: {err}"));
                }
            }
        });
}

fn handle_debug_request(
    request: &mut tiny_http::Request,
    method: &str,
    path: &str,
    debug: &SharedDebugState,
    writer: Option<&Arc<Mutex<Box<dyn Write + Send>>>>,
) -> Response<std::io::Cursor<Vec<u8>>> {
    match (method, path) {
        ("GET", "/debug") => text_response(
            200,
            "text/plain; charset=utf-8",
            "available endpoints:\nGET /debug/state\nGET /debug/screen\nPOST /debug/input (raw body)\nPOST /debug/note (text body)\n",
        ),
        ("GET", "/debug/state") => {
            let json = debug.state_json();
            text_response(200, "application/json; charset=utf-8", json)
        }
        ("GET", "/debug/screen") => {
            let text = debug.screen_text();
            text_response(200, "text/plain; charset=utf-8", text)
        }
        ("POST", "/debug/note") => {
            let mut body = String::new();
            if let Err(err) = request.as_reader().read_to_string(&mut body) {
                debug.set_error(format!("failed to read note body: {err}"));
                return text_response(400, "text/plain; charset=utf-8", "invalid note body\n");
            }

            let note = body.trim();
            if note.is_empty() {
                debug.set_note(None);
                text_response(200, "text/plain; charset=utf-8", "note cleared\n")
            } else {
                debug.set_note(Some(note.to_string()));
                text_response(200, "text/plain; charset=utf-8", "note set\n")
            }
        }
        ("POST", "/debug/input") => {
            let Some(writer) = writer else {
                return text_response(503, "text/plain; charset=utf-8", "pty writer unavailable\n");
            };

            let mut body = Vec::new();
            if let Err(err) = request.as_reader().read_to_end(&mut body) {
                debug.set_error(format!("failed to read input body: {err}"));
                return text_response(400, "text/plain; charset=utf-8", "invalid input body\n");
            }

            if body.is_empty() {
                return text_response(400, "text/plain; charset=utf-8", "input body is empty\n");
            }

            match write_to_pty(writer, &body) {
                Ok(()) => {
                    debug.record_bytes_to_pty(body.len(), true);
                    text_response(200, "text/plain; charset=utf-8", "input injected\n")
                }
                Err(err) => {
                    debug.set_error(format!("debug input write failed: {err:#}"));
                    text_response(500, "text/plain; charset=utf-8", "failed to write to pty\n")
                }
            }
        }
        _ => text_response(404, "text/plain; charset=utf-8", "not found\n"),
    }
}

fn text_response(
    status: u16,
    content_type: &str,
    body: impl Into<Vec<u8>>,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_data(body.into()).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes("Content-Type", content_type) {
        response = response.with_header(header);
    }
    response
}

fn encode_keystroke(keystroke: &gpui::Keystroke) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let ctrl = keystroke.modifiers.control;
    let alt = keystroke.modifiers.alt;

    let mut bytes = match key {
        "space" => vec![b' '],
        "enter" => vec![b'\r'],
        "tab" => vec![b'\t'],
        "backspace" => vec![0x7f],
        "escape" => vec![0x1b],
        "left" => b"\x1b[D".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        _ if key.len() == 1 => {
            let mut ch = key.as_bytes()[0];
            if ctrl {
                ch = ch.to_ascii_lowercase() & 0x1f;
            }
            vec![ch]
        }
        _ => return None,
    };

    if alt {
        bytes.insert(0, 0x1b);
    }

    Some(bytes)
}

fn ansi_bg_to_hsla(
    color: AnsiColor,
    colors: &alacritty_terminal::term::color::Colors,
) -> Option<gpui::Hsla> {
    match color {
        AnsiColor::Named(NamedColor::Background) => None,
        other => Some(ansi_to_hsla(other, colors, Flags::empty(), false)),
    }
}

fn ansi_to_hsla(
    color: AnsiColor,
    colors: &alacritty_terminal::term::color::Colors,
    flags: Flags,
    is_foreground: bool,
) -> gpui::Hsla {
    let resolved = ansi_to_rgb(color, colors, flags, is_foreground);
    rgb(((resolved.0 as u32) << 16) | ((resolved.1 as u32) << 8) | resolved.2 as u32).into()
}

fn ansi_to_rgb(
    color: AnsiColor,
    colors: &alacritty_terminal::term::color::Colors,
    flags: Flags,
    is_foreground: bool,
) -> (u8, u8, u8) {
    match color {
        AnsiColor::Spec(rgb) => {
            let mut value = (rgb.r, rgb.g, rgb.b);
            if is_foreground && flags.contains(Flags::DIM) && !flags.contains(Flags::BOLD) {
                value = dim_rgb(value);
            }
            value
        }
        AnsiColor::Named(named) => {
            named_to_rgb(named_color_variant(named, flags, is_foreground), colors)
        }
        AnsiColor::Indexed(index) => indexed_to_rgb(index),
    }
}

fn named_color_variant(named: NamedColor, flags: Flags, is_foreground: bool) -> NamedColor {
    if !is_foreground {
        return named;
    }

    match (
        flags.contains(Flags::BOLD),
        flags.contains(Flags::DIM),
        named,
    ) {
        (true, false, NamedColor::Foreground) => NamedColor::BrightForeground,
        (true, false, value) => value.to_bright(),
        (false, true, value) => value.to_dim(),
        _ => named,
    }
}

fn named_to_rgb(
    named: NamedColor,
    colors: &alacritty_terminal::term::color::Colors,
) -> (u8, u8, u8) {
    if let Some(rgb) = colors[named] {
        return (rgb.r, rgb.g, rgb.b);
    }

    match named {
        NamedColor::Black => (0x1d, 0x1f, 0x21),
        NamedColor::Red => (0xcc, 0x66, 0x66),
        NamedColor::Green => (0xb5, 0xbd, 0x68),
        NamedColor::Yellow => (0xf0, 0xc6, 0x74),
        NamedColor::Blue => (0x81, 0xa2, 0xbe),
        NamedColor::Magenta => (0xb2, 0x94, 0xbb),
        NamedColor::Cyan => (0x8a, 0xbe, 0xb7),
        NamedColor::White => (0xc5, 0xc8, 0xc6),
        NamedColor::BrightBlack => (0x66, 0x66, 0x66),
        NamedColor::BrightRed => (0xd5, 0x4e, 0x53),
        NamedColor::BrightGreen => (0xb9, 0xca, 0x4a),
        NamedColor::BrightYellow => (0xe7, 0xc5, 0x47),
        NamedColor::BrightBlue => (0x7a, 0xa6, 0xda),
        NamedColor::BrightMagenta => (0xc3, 0x97, 0xd8),
        NamedColor::BrightCyan => (0x70, 0xc0, 0xba),
        NamedColor::BrightWhite => (0xea, 0xea, 0xea),
        NamedColor::Foreground => (0xd7, 0xda, 0xe0),
        NamedColor::Background => (0x0f, 0x11, 0x15),
        NamedColor::Cursor => (0x3b, 0x82, 0xf6),
        NamedColor::DimBlack => dim_rgb((0x1d, 0x1f, 0x21)),
        NamedColor::DimRed => dim_rgb((0xcc, 0x66, 0x66)),
        NamedColor::DimGreen => dim_rgb((0xb5, 0xbd, 0x68)),
        NamedColor::DimYellow => dim_rgb((0xf0, 0xc6, 0x74)),
        NamedColor::DimBlue => dim_rgb((0x81, 0xa2, 0xbe)),
        NamedColor::DimMagenta => dim_rgb((0xb2, 0x94, 0xbb)),
        NamedColor::DimCyan => dim_rgb((0x8a, 0xbe, 0xb7)),
        NamedColor::DimWhite => dim_rgb((0xc5, 0xc8, 0xc6)),
        NamedColor::BrightForeground => (0xff, 0xff, 0xff),
        NamedColor::DimForeground => dim_rgb((0xd7, 0xda, 0xe0)),
    }
}

fn indexed_to_rgb(index: u8) -> (u8, u8, u8) {
    match index {
        0 => named_to_rgb(NamedColor::Black, &Default::default()),
        1 => named_to_rgb(NamedColor::Red, &Default::default()),
        2 => named_to_rgb(NamedColor::Green, &Default::default()),
        3 => named_to_rgb(NamedColor::Yellow, &Default::default()),
        4 => named_to_rgb(NamedColor::Blue, &Default::default()),
        5 => named_to_rgb(NamedColor::Magenta, &Default::default()),
        6 => named_to_rgb(NamedColor::Cyan, &Default::default()),
        7 => named_to_rgb(NamedColor::White, &Default::default()),
        8 => named_to_rgb(NamedColor::BrightBlack, &Default::default()),
        9 => named_to_rgb(NamedColor::BrightRed, &Default::default()),
        10 => named_to_rgb(NamedColor::BrightGreen, &Default::default()),
        11 => named_to_rgb(NamedColor::BrightYellow, &Default::default()),
        12 => named_to_rgb(NamedColor::BrightBlue, &Default::default()),
        13 => named_to_rgb(NamedColor::BrightMagenta, &Default::default()),
        14 => named_to_rgb(NamedColor::BrightCyan, &Default::default()),
        15 => named_to_rgb(NamedColor::BrightWhite, &Default::default()),
        16..=231 => {
            let index = index - 16;
            let r = index / 36;
            let g = (index % 36) / 6;
            let b = index % 6;
            (cube_value(r), cube_value(g), cube_value(b))
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    }
}

fn cube_value(step: u8) -> u8 {
    match step {
        0 => 0,
        n => 55 + n * 40,
    }
}

fn dim_rgb((r, g, b): (u8, u8, u8)) -> (u8, u8, u8) {
    (
        ((r as f32) * 0.66) as u8,
        ((g as f32) * 0.66) as u8,
        ((b as f32) * 0.66) as u8,
    )
}

fn run_self_check() -> Result<()> {
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

    ensure!(indexed_to_rgb(16) == (0, 0, 0), "indexed color 16 mismatch");
    ensure!(
        indexed_to_rgb(231) == (255, 255, 255),
        "indexed color 231 mismatch"
    );

    let grid = compute_grid_size(size(px(1000.0), px(520.0)), px(8.0));
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

fn main() {
    if std::env::args().skip(1).any(|arg| arg == "--self-check") {
        if let Err(err) = run_self_check() {
            eprintln!("self-check failed: {err:#}");
            std::process::exit(1);
        }
        return;
    }

    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1000.0), px(520.0)), cx);
        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| AgentTerminal::new(window, cx)),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn encode_basic_keys() {
        let enter = gpui::Keystroke::parse("enter").expect("parse enter");
        assert_eq!(encode_keystroke(&enter), Some(vec![b'\r']));

        let tab = gpui::Keystroke::parse("tab").expect("parse tab");
        assert_eq!(encode_keystroke(&tab), Some(vec![b'\t']));

        let left = gpui::Keystroke::parse("left").expect("parse left");
        assert_eq!(encode_keystroke(&left), Some(b"\x1b[D".to_vec()));
    }

    #[test]
    fn encode_modifier_keys() {
        let ctrl_c = gpui::Keystroke::parse("ctrl-c").expect("parse ctrl-c");
        assert_eq!(encode_keystroke(&ctrl_c), Some(vec![3]));

        let alt_x = gpui::Keystroke::parse("alt-x").expect("parse alt-x");
        assert_eq!(encode_keystroke(&alt_x), Some(vec![0x1b, b'x']));
    }

    #[test]
    fn indexed_color_cube_edges() {
        assert_eq!(indexed_to_rgb(16), (0, 0, 0));
        assert_eq!(indexed_to_rgb(231), (255, 255, 255));
        assert_eq!(indexed_to_rgb(232), (8, 8, 8));
    }

    #[test]
    fn compute_grid_has_sane_minimums() {
        let tiny = compute_grid_size(size(px(10.0), px(10.0)), px(8.0));
        assert_eq!(tiny.cols, MIN_COLS);
        assert_eq!(tiny.rows, MIN_ROWS);

        let normal = compute_grid_size(size(px(1000.0), px(520.0)), px(8.0));
        assert!(normal.cols >= 80);
        assert!(normal.rows >= 20);
    }

    #[test]
    fn snapshot_text_trims_trailing_spaces() {
        let snapshot = ScreenSnapshot {
            cells: vec![
                vec![
                    CellSnapshot {
                        ch: 'a',
                        ..Default::default()
                    },
                    CellSnapshot {
                        ch: 'b',
                        ..Default::default()
                    },
                    CellSnapshot::default(),
                ],
                vec![CellSnapshot::default(), CellSnapshot::default()],
            ],
            cursor_row: 0,
            cursor_col: 0,
        };

        let lines = snapshot_to_lines(&snapshot);
        assert_eq!(lines[0], "ab");
        assert_eq!(lines[1], "");
    }

    #[test]
    fn debug_http_serves_state_and_note() {
        let addr = reserve_local_addr();
        let debug = SharedDebugState::new(
            "test-shell".to_string(),
            "connected".to_string(),
            GridSize { cols: 80, rows: 24 },
        );
        start_debug_http_server_at_addr(debug.clone(), None, addr.clone());

        wait_for_server(&addr);

        let state = send_http(
            &addr,
            format!("GET /debug/state HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        );
        assert!(state.contains("\"shell\": \"test-shell\""));
        assert!(state.contains("\"status\": \"connected\""));

        let note_body = "hello from test";
        let note_response = send_http(
            &addr,
            format!(
                "POST /debug/note HTTP/1.1\r\nHost: {addr}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                note_body.len(),
                note_body
            ),
        );
        assert!(note_response.contains("note set"));

        let state_after_note = send_http(
            &addr,
            format!("GET /debug/state HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        );
        assert!(state_after_note.contains("\"note\": \"hello from test\""));
    }

    #[test]
    fn debug_http_injects_input_to_writer() {
        let addr = reserve_local_addr();
        let debug = SharedDebugState::new(
            "test-shell".to_string(),
            "connected".to_string(),
            GridSize { cols: 80, rows: 24 },
        );
        let sink = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(BufferWriter { sink: sink.clone() })));

        start_debug_http_server_at_addr(debug, Some(writer), addr.clone());
        wait_for_server(&addr);

        let payload = "echo injected\n";
        let response = send_http(
            &addr,
            format!(
                "POST /debug/input HTTP/1.1\r\nHost: {addr}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            ),
        );
        assert!(response.contains("input injected"));

        for _ in 0..20 {
            if sink.lock().as_slice() == payload.as_bytes() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("debug input bytes were not forwarded to PTY writer");
    }

    fn reserve_local_addr() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral address");
        let addr = listener.local_addr().expect("read local addr");
        drop(listener);
        addr.to_string()
    }

    fn wait_for_server(addr: &str) {
        for _ in 0..40 {
            if TcpStream::connect(addr).is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("debug HTTP server did not start in time");
    }

    fn send_http(addr: &str, request: String) -> String {
        let mut stream = TcpStream::connect(addr).expect("connect to debug server");
        stream
            .write_all(request.as_bytes())
            .expect("send request to debug server");
        let _ = stream.shutdown(std::net::Shutdown::Write);

        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read response from debug server");
        response
    }

    struct BufferWriter {
        sink: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for BufferWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.sink.lock().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
