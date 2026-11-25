use async_channel::{Receiver, Sender, bounded};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TabSelector {
    Id(u64),
    Active,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScrollAction {
    Up(u32),
    Down(u32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

/// Wire shape of `POST /tabs/:id/scroll`. `action` is one of
/// `up|down|page_up|page_down|top|bottom`; `lines` is honoured only by
/// `up` and `down`. The api_server layer maps this to `ScrollAction`.
#[derive(Debug, Deserialize)]
pub(crate) struct ScrollRequest {
    pub(crate) action: String,
    #[serde(default)]
    pub(crate) lines: Option<u32>,
}

/// Wire shape of `POST /tabs/:id/resize`. Bounds-enforced in `api_server`.
/// `sync_window` means the browser is authoritative ("浏览器尺寸" mode): the
/// native window is resized to fit this grid too, so neither side scrolls.
#[derive(Debug, Deserialize)]
pub(crate) struct ResizeRequest {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    #[serde(default)]
    pub(crate) sync_window: bool,
}

/// Wire shape of `POST /web/client`. The built-in page sends this on load,
/// once a second as a heartbeat, and on unload with `connected: false`.
#[derive(Debug, Deserialize)]
pub(crate) struct WebClientRequest {
    pub(crate) id: String,
    pub(crate) connected: bool,
    #[serde(default)]
    pub(crate) cols: u16,
    #[serde(default)]
    pub(crate) rows: u16,
    #[serde(default, alias = "web_fit")]
    pub(crate) browser_size: bool,
    #[serde(default)]
    pub(crate) force: bool,
}

#[derive(Debug)]
pub(crate) enum ApiCommand {
    ListTabs {
        reply: Sender<ApiReply>,
    },
    CreateTab {
        reply: Sender<ApiReply>,
    },
    CloseTab {
        id: TabSelector,
        reply: Sender<ApiReply>,
    },
    ActivateTab {
        id: TabSelector,
        reply: Sender<ApiReply>,
    },
    GetTab {
        id: TabSelector,
        reply: Sender<ApiReply>,
    },
    GetScreen {
        id: TabSelector,
        reply: Sender<ApiReply>,
    },
    /// Whole-screen repaint encoded as ANSI (colour + cursor + alt-screen flag
    /// in the first byte) for the built-in xterm.js web client.
    GetScreenAnsi {
        id: TabSelector,
        reply: Sender<ApiReply>,
    },
    GetScrollback {
        id: TabSelector,
        lines: Option<usize>,
        reply: Sender<ApiReply>,
    },
    ScrollDisplay {
        id: TabSelector,
        action: ScrollAction,
        reply: Sender<ApiReply>,
    },
    ResizeTab {
        id: TabSelector,
        cols: u16,
        rows: u16,
        sync_window: bool,
        reply: Sender<ApiReply>,
    },
    /// `browser_size` is true in "浏览器尺寸" mode: the browser viewport is
    /// authoritative and the native window follows it. False is "PC 尺寸"
    /// mode, where the native window is authoritative.
    WebClient {
        id: String,
        connected: bool,
        cols: u16,
        rows: u16,
        browser_size: bool,
        force: bool,
        reply: Sender<ApiReply>,
    },
    /// Browser input is fenced to the current owner so a displaced client
    /// cannot write during its heartbeat/reconnect grace window.
    WebInput {
        id: TabSelector,
        client: String,
        bytes: Vec<u8>,
        reply: Sender<ApiReply>,
    },
    WriteInput {
        id: TabSelector,
        bytes: Vec<u8>,
        reply: Sender<ApiReply>,
    },
    SendKeys {
        id: TabSelector,
        body: String,
        reply: Sender<ApiReply>,
    },
    SetNote {
        id: TabSelector,
        note: Option<String>,
        reply: Sender<ApiReply>,
    },
    ReplaceLine {
        id: TabSelector,
        bytes: Vec<u8>,
        reply: Sender<ApiReply>,
    },
    /// Attach a WebSocket data stream to a tab. The reply carries the current
    /// screen as an ANSI repaint so the browser can seed xterm before any
    /// incremental PTY bytes arrive (no gap, no double-apply).
    WebStreamAttach {
        id: TabSelector,
        client: String,
        /// Unique per connection. Detach must identify the exact stream: a
        /// late detach from an old connection must not remove the subscription
        /// a newer connection just installed for the same client id.
        stream_id: u64,
        reply: Sender<ApiReply>,
        /// Private channel used to hand the PTY receiver to the socket thread.
        stream: Sender<async_channel::Receiver<Vec<u8>>>,
    },
    /// Drop one specific stream when its WebSocket closes. The heartbeat
    /// registry is deliberately untouched here — HTTP `connected:false` and
    /// the 5 s stale timeout own client lifetime.
    WebStreamDetach {
        stream_id: u64,
        reply: Sender<ApiReply>,
    },
}

#[derive(Debug)]
pub(crate) enum ApiReply {
    Ok { status: u16, body: ReplyBody },
    Err { status: u16, error: String },
}

#[derive(Debug)]
pub(crate) enum ReplyBody {
    Json(serde_json::Value),
    Text(String),
}

#[derive(Debug)]
pub(crate) enum RouteOutcome {
    Command(ApiCommand, Receiver<ApiReply>),
    Immediate {
        status: u16,
        content_type: &'static str,
        body: String,
    },
    ImmediateBinary {
        status: u16,
        content_type: &'static str,
        body: Vec<u8>,
    },
}

pub(crate) fn oneshot() -> (Sender<ApiReply>, Receiver<ApiReply>) {
    bounded(1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TabKind {
    Terminal,
    Snapshot,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct ApiCounters {
    pub(crate) bytes_from_pty: u64,
    pub(crate) bytes_to_pty: u64,
    pub(crate) key_events: u64,
    pub(crate) injected_events: u64,
    pub(crate) resize_events: u64,
    pub(crate) http_requests: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TabSummaryDto {
    pub(crate) id: u64,
    pub(crate) title: String,
    pub(crate) kind: TabKind,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TabDetailDto {
    pub(crate) id: u64,
    pub(crate) title: String,
    pub(crate) kind: TabKind,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) cursor_row: usize,
    pub(crate) cursor_col: usize,
    pub(crate) status: String,
    pub(crate) note: Option<String>,
    pub(crate) counters: ApiCounters,
    pub(crate) uptime_ms: u128,
    pub(crate) last_error: Option<String>,
}
