use std::io::Write;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use parking_lot::Mutex;
use serde::Serialize;
use tiny_http::{Header, Response, Server, StatusCode};

use crate::GridSize;
use crate::pty::write_to_pty;

const DEBUG_HTTP_DEFAULT_ADDR: &str = "127.0.0.1:7878";

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct DebugCounters {
    bytes_from_pty: u64,
    bytes_to_pty: u64,
    key_events: u64,
    injected_events: u64,
    resize_events: u64,
    http_requests: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct DebugState {
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
pub(crate) struct SharedDebugState {
    inner: Arc<Mutex<DebugState>>,
}

impl SharedDebugState {
    pub(crate) fn new(shell: String, status: String, grid_size: GridSize) -> Self {
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

    pub(crate) fn set_listening_addr(&self, addr: String) {
        self.inner.lock().listening_addr = Some(addr);
    }

    pub(crate) fn set_error(&self, err: impl Into<String>) {
        self.inner.lock().last_error = Some(err.into());
    }

    pub(crate) fn set_note(&self, note: Option<String>) {
        self.inner.lock().note = note;
    }

    pub(crate) fn note(&self) -> Option<String> {
        self.inner.lock().note.clone()
    }

    pub(crate) fn record_http_request(&self) {
        self.inner.lock().counters.http_requests += 1;
    }

    pub(crate) fn record_bytes_from_pty(&self, bytes: usize) {
        self.inner.lock().counters.bytes_from_pty += bytes as u64;
    }

    pub(crate) fn record_bytes_to_pty(&self, bytes: usize, injected: bool) {
        let mut state = self.inner.lock();
        state.counters.bytes_to_pty += bytes as u64;
        if injected {
            state.counters.injected_events += 1;
        }
    }

    pub(crate) fn record_key_event(&self) {
        self.inner.lock().counters.key_events += 1;
    }

    pub(crate) fn record_resize(&self) {
        self.inner.lock().counters.resize_events += 1;
    }

    pub(crate) fn update_screen_snapshot(
        &self,
        grid_size: GridSize,
        cursor_row: usize,
        cursor_col: usize,
        screen_lines: Vec<String>,
    ) {
        let mut state = self.inner.lock();
        state.grid_size = grid_size;
        state.cursor_row = cursor_row;
        state.cursor_col = cursor_col;
        state.screen_lines = screen_lines;
    }

    pub(crate) fn status_summary(&self) -> String {
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

    pub(crate) fn state_json(&self) -> String {
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

    pub(crate) fn screen_text(&self) -> String {
        let state = self.inner.lock();
        if state.screen_lines.is_empty() {
            return "<empty screen>\n".to_string();
        }

        let mut out = state.screen_lines.join("\n");
        out.push('\n');
        out
    }
}

pub(crate) fn start_debug_http_server(
    debug: SharedDebugState,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
) {
    let addr = std::env::var("AGENT_TUI_DEBUG_ADDR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEBUG_HTTP_DEFAULT_ADDR.to_string());
    start_debug_http_server_at_addr(debug, writer, addr);
}

pub(crate) fn start_debug_http_server_at_addr(
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

pub(crate) fn handle_debug_request(
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
            "available endpoints:\nGET /debug/state\nGET /debug/screen\nPOST /debug/input (raw body)\nPOST /debug/replace-line (text body)\nPOST /debug/note (text body)\n",
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
        ("POST", "/debug/replace-line") => {
            let Some(writer) = writer else {
                return text_response(503, "text/plain; charset=utf-8", "pty writer unavailable\n");
            };

            let mut body = Vec::new();
            if let Err(err) = request.as_reader().read_to_end(&mut body) {
                debug.set_error(format!("failed to read replace-line body: {err}"));
                return text_response(
                    400,
                    "text/plain; charset=utf-8",
                    "invalid replace-line body\n",
                );
            }

            let mut payload = Vec::with_capacity(body.len() + 1);
            payload.push(0x15);
            payload.extend_from_slice(&body);

            match write_to_pty(writer, &payload) {
                Ok(()) => {
                    debug.record_bytes_to_pty(payload.len(), true);
                    text_response(200, "text/plain; charset=utf-8", "input line replaced\n")
                }
                Err(err) => {
                    debug.set_error(format!("debug replace-line write failed: {err:#}"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

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

    #[test]
    fn debug_http_replaces_input_line_in_writer() {
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

        let payload = "replace with this";
        let response = send_http(
            &addr,
            format!(
                "POST /debug/replace-line HTTP/1.1\r\nHost: {addr}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            ),
        );
        assert!(response.contains("input line replaced"));

        let mut expected = Vec::from([0x15]);
        expected.extend_from_slice(payload.as_bytes());
        for _ in 0..20 {
            if sink.lock().as_slice() == expected.as_slice() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("debug replace-line bytes were not forwarded to PTY writer");
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
