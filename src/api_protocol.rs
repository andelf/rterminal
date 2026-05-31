use async_channel::{Receiver, Sender, bounded};
use serde::Serialize;

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

#[derive(Debug)]
pub(crate) enum ApiCommand {
    ListTabs { reply: Sender<ApiReply> },
    CreateTab { reply: Sender<ApiReply> },
    CloseTab { id: TabSelector, reply: Sender<ApiReply> },
    ActivateTab { id: TabSelector, reply: Sender<ApiReply> },
    GetTab { id: TabSelector, reply: Sender<ApiReply> },
    GetScreen { id: TabSelector, reply: Sender<ApiReply> },
    GetScrollback { id: TabSelector, lines: Option<usize>, reply: Sender<ApiReply> },
    ScrollDisplay { id: TabSelector, action: ScrollAction, reply: Sender<ApiReply> },
    WriteInput { id: TabSelector, bytes: Vec<u8>, reply: Sender<ApiReply> },
    SendKeys { id: TabSelector, body: String, reply: Sender<ApiReply> },
    SetNote { id: TabSelector, note: Option<String>, reply: Sender<ApiReply> },
    ReplaceLine { id: TabSelector, bytes: Vec<u8>, reply: Sender<ApiReply> },
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
    Immediate { status: u16, content_type: &'static str, body: String },
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
