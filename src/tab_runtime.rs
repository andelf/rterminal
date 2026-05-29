//! Per-tab runtime stats and note. Mutated from `terminal.rs` event handlers,
//! read by the API server when answering `/tabs/:id` and `/tabs/:id/screen`.

use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use serde::Serialize;

use crate::api_protocol::ApiCounters;
use crate::terminal::GridSize;

#[derive(Clone, Debug)]
struct Inner {
    started_at: Instant,
    shell: String,
    status: String,
    note: Option<String>,
    grid_size: GridSize,
    cursor_row: usize,
    cursor_col: usize,
    screen_lines: Vec<String>,
    counters: ApiCounters,
    last_error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct TabRuntimeState {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Serialize)]
pub(crate) struct TabRuntimeSnapshot {
    pub(crate) shell: String,
    pub(crate) status: String,
    pub(crate) note: Option<String>,
    pub(crate) grid_size: GridSize,
    pub(crate) cursor_row: usize,
    pub(crate) cursor_col: usize,
    pub(crate) screen_lines: Vec<String>,
    pub(crate) counters: ApiCounters,
    pub(crate) uptime_ms: u128,
    pub(crate) last_error: Option<String>,
}

impl TabRuntimeState {
    pub(crate) fn new(shell: String, status: String, grid_size: GridSize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                started_at: Instant::now(),
                shell,
                status,
                note: None,
                grid_size,
                cursor_row: 0,
                cursor_col: 0,
                screen_lines: Vec::new(),
                counters: ApiCounters::default(),
                last_error: None,
            })),
        }
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

    pub(crate) fn record_http_request(&self) {
        self.inner.lock().counters.http_requests += 1;
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

    pub(crate) fn snapshot(&self) -> TabRuntimeSnapshot {
        let state = self.inner.lock();
        TabRuntimeSnapshot {
            shell: state.shell.clone(),
            status: state.status.clone(),
            note: state.note.clone(),
            grid_size: state.grid_size,
            cursor_row: state.cursor_row,
            cursor_col: state.cursor_col,
            screen_lines: state.screen_lines.clone(),
            counters: state.counters.clone(),
            uptime_ms: state.started_at.elapsed().as_millis(),
            last_error: state.last_error.clone(),
        }
    }

    pub(crate) fn screen_text(&self) -> String {
        let state = self.inner.lock();
        if state.screen_lines.is_empty() {
            return "<empty screen>\n".to_string();
        }
        // Strip per-row trailing whitespace + drop any wholly-empty rows from
        // the bottom. The grid pads every row to `cols`, and the unused rows
        // below the last drawn line are layout padding with no semantic content.
        // TUI apps that draw across the whole grid (vim with `~`, htop, less)
        // leave non-empty content on every row, so this trim doesn't touch them.
        let last_non_empty = state
            .screen_lines
            .iter()
            .rposition(|line| !line.trim_end().is_empty());
        let Some(last) = last_non_empty else {
            return "<empty screen>\n".to_string();
        };
        let mut out = String::new();
        for line in &state.screen_lines[..=last] {
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}
