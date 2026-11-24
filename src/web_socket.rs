//! Browser data plane: raw PTY bytes over a WebSocket.
//!
//! HTTP stays the control plane (tabs, resize, scroll, debug). This dedicated
//! listener carries only the live stream so the browser no longer has to poll
//! a full-screen ANSI snapshot every 100 ms. A freshly attached client first
//! receives one ANSI snapshot, then incremental PTY bytes as binary frames.

use std::cell::Cell;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use serde_json::{Value, json};
use tungstenite::Message;
use tungstenite::accept_hdr;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http;

use crate::api_protocol::{ApiCommand, ApiReply, ReplyBody, TabSelector, oneshot};

const READ_TIMEOUT: Duration = Duration::from_millis(50);
const MAX_BATCH: usize = 64 * 1024;

struct Connection {
    tab_id: u64,
    client: String,
    debug: bool,
}

pub(crate) fn start_ws_server(addr: &str, cmd_tx: Sender<ApiCommand>) -> io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    eprintln!("web data stream listening on ws://{}", listener.local_addr()?);
    thread::Builder::new()
        .name("agent-web-ws".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let cmd_tx = cmd_tx.clone();
                        let _ = thread::Builder::new()
                            .name("agent-web-ws-client".to_string())
                            .spawn(move || {
                                if let Err(err) = serve_client(stream, cmd_tx) {
                                    eprintln!("web socket: {err}");
                                }
                            });
                    }
                    Err(err) => eprintln!("web socket accept: {err}"),
                }
            }
        })?;
    Ok(())
}

fn serve_client(stream: TcpStream, cmd_tx: Sender<ApiCommand>) -> Result<(), String> {
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|err| format!("set read timeout: {err}"))?;
    stream
        .set_nodelay(true)
        .map_err(|err| format!("set nodelay: {err}"))?;

    // The handshake callback extracts the tab id and client id from the URL.
    // Using a Cell keeps the closure FnOnce-friendly while retaining the data.
    let conn = Cell::new(None);
    let callback = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        let path = request.uri().path();
        let Some(tab) = path.strip_prefix("/ws/tabs/") else {
            return Err(not_found("expected /ws/tabs/<id>"));
        };
        let tab_id = tab.parse::<u64>().map_err(|_| not_found("invalid tab id"))?;
        let mut client = String::new();
        let mut debug = false;
        if let Some(query) = request.uri().query() {
            for pair in query.split('&') {
                let mut parts = pair.splitn(2, '=');
                let key = parts.next().unwrap_or("");
                let value = parts.next().unwrap_or("");
                if key == "client" {
                    client = percent_decode(value);
                } else if key == "debug" && value == "1" {
                    debug = true;
                }
            }
        }
        if client.is_empty() {
            return Err(not_found("missing client id"));
        }
        conn.set(Some(Connection { tab_id, client, debug }));
        Ok(response)
    };
    let mut socket = accept_hdr(stream, callback).map_err(|err| format!("handshake: {err}"))?;
    let Connection { tab_id, client, debug } =
        conn.into_inner().ok_or_else(|| "handshake lost connection info".to_string())?;

    let (pty_rx, mut snapshot) = attach(&cmd_tx, tab_id, client.clone())?;
    snapshot["type"] = json!("snapshot");
    socket
        .send(Message::Text(snapshot.to_string().into()))
        .map_err(|err| format!("send snapshot: {err}"))?;

    loop {
        // Drain what the PTY has produced. The first chunk goes out immediately;
        // only an existing backlog is coalesced, so latency stays low.
        if let Ok(first) = pty_rx.try_recv() {
            let mut bytes = first;
            while bytes.len() < MAX_BATCH {
                match pty_rx.try_recv() {
                    Ok(more) => bytes.extend_from_slice(&more),
                    Err(_) => break,
                }
            }
            if socket.send(Message::Binary(bytes.into())).is_err() {
                break;
            }
        }

        match socket.read() {
            Ok(Message::Binary(data)) => {
                if debug {
                    crate::web_debug::append_input(&format!("/ws/tabs/{tab_id}"), &data);
                }
                let (reply_tx, reply_rx) = oneshot();
                let cmd = ApiCommand::WriteInput {
                    id: TabSelector::Id(tab_id),
                    bytes: data.to_vec(),
                    reply: reply_tx,
                };
                if cmd_tx.send_blocking(cmd).is_err() || reply_rx.recv_blocking().is_err() {
                    break;
                }
            }
            Ok(Message::Text(text)) => {
                if !control(&cmd_tx, tab_id, &client, &text) {
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(err))
                if matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(err) => {
                eprintln!("web socket read: {err}");
                break;
            }
        }
    }

    let (reply_tx, _) = oneshot();
    let _ = cmd_tx.send_blocking(ApiCommand::WebStreamDetach {
        client,
        reply: reply_tx,
    });
    Ok(())
}

fn attach(
    cmd_tx: &Sender<ApiCommand>,
    tab_id: u64,
    client: String,
) -> Result<(Receiver<Vec<u8>>, Value), String> {
    let (reply_tx, reply_rx) = oneshot();
    let (stream_tx, stream_rx) = async_channel::bounded(1);
    cmd_tx
        .send_blocking(ApiCommand::WebStreamAttach {
            id: TabSelector::Id(tab_id),
            client,
            reply: reply_tx,
            stream: stream_tx,
        })
        .map_err(|_| "command channel closed".to_string())?;
    let reply = reply_rx
        .recv_blocking()
        .map_err(|_| "attach reply channel closed".to_string())?;
    let snapshot = match reply {
        ApiReply::Ok { body: ReplyBody::Json(value), .. } => value,
        ApiReply::Err { error, .. } => return Err(error),
        _ => return Err("unexpected attach reply".to_string()),
    };
    let rx = stream_rx
        .recv_blocking()
        .map_err(|_| "stream channel closed".to_string())?;
    Ok((rx, snapshot))
}

/// JSON control frames. Returns false when the connection should be dropped.
fn control(cmd_tx: &Sender<ApiCommand>, tab_id: u64, client: &str, text: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return true;
    };
    match value.get("type").and_then(Value::as_str) {
        Some("resize") => {
            let (Some(cols), Some(rows)) =
                (value.get("cols").and_then(Value::as_u64), value.get("rows").and_then(Value::as_u64))
            else {
                return true;
            };
            if !(20..=300).contains(&cols) || !(5..=200).contains(&rows) {
                return true;
            }
            let (reply_tx, _) = oneshot();
            cmd_tx
                .send_blocking(ApiCommand::ResizeTab {
                    id: TabSelector::Id(tab_id),
                    cols: cols as u16,
                    rows: rows as u16,
                    sync_window: value
                        .get("sync_window")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    reply: reply_tx,
                })
                .is_ok()
        }
        Some("heartbeat") => {
            let (reply_tx, _) = oneshot();
            cmd_tx
                .send_blocking(ApiCommand::WebClient {
                    id: client.to_string(),
                    connected: true,
                    cols: value.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16,
                    rows: value.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16,
                    browser_size: value
                        .get("browser_size")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    reply: reply_tx,
                })
                .is_ok()
        }
        // A fresh screen snapshot is sent by the server on every attach; the
        // browser asks for another one by reconnecting.
        _ => true,
    }
}

fn not_found(message: &str) -> ErrorResponse {
    http::Response::builder()
        .status(http::StatusCode::NOT_FOUND)
        .body(Some(message.to_string()))
        .unwrap()
}

/// Only `client` is URL-encoded; keep this tiny instead of pulling in `url`.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
