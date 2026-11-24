//! Server-side sink for the web client's input debug stream.
//!
//! The IME / dictation problems only reproduce on a real phone, where there is
//! no devtools console. The web client therefore records the raw DOM event
//! stream and POSTs it to `/web/debug`; this module appends those lines to
//! `~/.rterminal/web-debug/web-debug.log` so the evidence survives a reload,
//! a tab close or a client crash.
//!
//! Two kinds of lines land in the same file, in arrival order:
//!
//! ```text
//! 1732430... c| {"s":"…","n":12,"k":"input", …}      <- browser event
//! 1732430... i| POST /tabs/1/input 3B 0xe4 bd a0 "你"  <- bytes we received
//! ```
//!
//! The `i|` lines are the ground truth for the duplication bug: if the server
//! receives the same sentence twice, the browser sent it twice; if the server
//! receives it once and the screen repeats it, the bug lives in the PTY/repaint
//! path instead.

use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use parking_lot::Mutex;

/// Rotate once the file grows past this size; a single phone session of typing
/// stays far below it, so rotation only ever trims old sessions.
const MAX_BYTES: u64 = 16 * 1024 * 1024;

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

struct Sink {
    path: PathBuf,
    file: Option<fs::File>,
    announced: bool,
}

impl Sink {
    fn open() -> Self {
        let path = log_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut sink = Sink { path, file: None, announced: false };
        sink.rotate_if_needed();
        sink
    }

    fn rotate_if_needed(&mut self) {
        let too_big = fs::metadata(&self.path)
            .map(|meta| meta.len() > MAX_BYTES)
            .unwrap_or(false);
        if !too_big {
            return;
        }
        if self.file.take().is_some() {
            // Flush by dropping first; the rename below would otherwise be a
            // no-op on Windows while the handle is still open.
        }
        let _ = fs::rename(&self.path, self.path.with_extension("log.1"));
    }

    fn ensure_open(&mut self) -> bool {
        if self.file.is_some() {
            return true;
        }
        match fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            Ok(file) => {
                self.file = Some(file);
                true
            }
            Err(err) => {
                if !self.announced {
                    eprintln!(
                        "web debug: cannot write {}: {err}",
                        self.path.display()
                    );
                    self.announced = true;
                }
                false
            }
        }
    }

    fn write_line(&mut self, text: &str) {
        self.rotate_if_needed();
        if !self.ensure_open() {
            return;
        }
        if let Some(file) = self.file.as_mut() {
            let _ = file.write_all(text.as_bytes());
            let _ = file.write_all(b"\n");
            let _ = file.flush();
        }
    }
}

fn log_path() -> PathBuf {
    if let Ok(dir) = env::var("RTERMINAL_DEBUG_DIR") {
        return PathBuf::from(dir).join("web-debug.log");
    }
    let home = env::var("USERPROFILE")
        .or_else(|_| env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".rterminal")
        .join("web-debug")
        .join("web-debug.log")
}

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// Where the current log lives, for the startup banner.
pub(crate) fn path_display() -> String {
    log_path().display().to_string()
}

/// Append one NDJSON batch from the browser. Each client line is prefixed with
/// the server's own receive time so a skewed phone clock cannot hide the order.
pub(crate) fn append_client(body: &str) {
    let sink = SINK.get_or_init(|| Mutex::new(Sink::open()));
    let mut sink = sink.lock();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        sink.write_line(&format!("{} c| {}", now_ms(), trimmed));
    }
}

/// Append the exact bytes the browser wrote to a tab's PTY. Binary data is
/// rendered as hex plus a quoted preview, because a control byte is the whole
/// point of the exercise.
pub(crate) fn append_input(path: &str, bytes: &[u8]) {
    let sink = SINK.get_or_init(|| Mutex::new(Sink::open()));
    let mut sink = sink.lock();
    let text = String::from_utf8_lossy(bytes);
    sink.write_line(&format!(
        "{} i| {path} {}B 0x{} \"{}\"",
        now_ms(),
        bytes.len(),
        hex(bytes),
        escape(&text)
    ));
}

/// Return the tail of the log so a shell can read the evidence without knowing
/// the user's home directory.
pub(crate) fn tail(lines: usize) -> String {
    let text = fs::read_to_string(log_path()).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for byte in bytes {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// True when the request explicitly asked for its bytes to be logged.
pub(crate) fn wants_input_trace(headers: &[tiny_http::Header]) -> bool {
    headers.iter().any(|header| {
        header.field.as_str().as_str().eq_ignore_ascii_case("x-rterminal-debug")
            && header.value.as_str() == "1"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_keeps_bytes_readable() {
        assert_eq!(escape("a\tb\\c\"d"), "a\\tb\\\\c\\\"d");
        assert_eq!(escape("\u{1b}[A"), "\\x1b[A");
        assert_eq!(escape("你好"), "你好");
    }

    #[test]
    fn hex_is_space_separated() {
        assert_eq!(hex("你".as_bytes()), "e4 bd a0");
        assert_eq!(hex(b"\r"), "0d");
    }
}
