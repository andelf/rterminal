//! Browser data plane: raw PTY bytes over a WebSocket.
//!
//! HTTP stays the control plane (tabs, resize, scroll, debug). The live
//! stream shares the HTTP listener as `GET /ws/tabs/<id>`; a freshly attached
//! client first receives one ANSI snapshot, then incremental PTY bytes as
//! binary frames.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use async_channel::{Receiver, Sender, TryRecvError};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::api_protocol::{ApiCommand, ApiReply, ReplyBody, TabSelector, oneshot};

const MAX_BATCH: usize = 64 * 1024;
static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);

/// Drop exactly one subscription. Called on every exit path, including a
/// failed snapshot send.
fn detach_stream(cmd_tx: &Sender<ApiCommand>, stream_id: u64) {
    let (reply_tx, _) = oneshot();
    let _ = cmd_tx.send_blocking(ApiCommand::WebStreamDetach {
        stream_id,
        reply: reply_tx,
    });
}

/// Entry point for `GET /ws/tabs/<id>?client=<id>[&debug=1]`.
/// Authentication already happened in the auth middleware.
pub(crate) async fn ws_stream(
    ws: WebSocketUpgrade,
    Path(path): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    State(cmd_tx): State<Sender<ApiCommand>>,
) -> Response {
    let Ok(tab_id) = path.parse::<u64>() else {
        return (StatusCode::NOT_FOUND, "invalid tab id").into_response();
    };
    let client = params.get("client").cloned().unwrap_or_default();
    let debug = params.get("debug").map(|v| v == "1").unwrap_or(false);
    if client.is_empty() {
        return (StatusCode::NOT_FOUND, "missing client id").into_response();
    }
    ws.on_upgrade(move |socket| serve_client(socket, cmd_tx, tab_id, client, debug))
}

async fn serve_client(
    mut socket: WebSocket,
    cmd_tx: Sender<ApiCommand>,
    tab_id: u64,
    client: String,
    debug: bool,
) {
    let stream_id = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
    let (pty_rx, mut snapshot) = match attach(&cmd_tx, tab_id, &client, stream_id).await {
        Ok(attach) => attach,
        Err(err) => {
            eprintln!("web socket attach: {err}");
            return;
        }
    };
    snapshot["type"] = json!("snapshot");
    if socket
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
        // The subscription is already registered: unwind it here, otherwise
        // this stream stays alive until its buffer fills up.
        detach_stream(&cmd_tx, stream_id);
        return;
    }

    loop {
        // Drain PTY output and client frames concurrently. The first chunk
        // goes out immediately; only an existing backlog is coalesced, so
        // latency stays low.
        tokio::select! {
            chunk = pty_rx.recv() => match chunk {
                Ok(first) => {
                    let mut bytes = first;
                    let mut disconnected = false;
                    while bytes.len() < MAX_BATCH {
                        match pty_rx.try_recv() {
                            Ok(more) => bytes.extend_from_slice(&more),
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Closed) => {
                                disconnected = true;
                                break;
                            }
                        }
                    }
                    if socket.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                    if disconnected {
                        break;
                    }
                }
                // Channel closed: the tab is gone or the subscriber was
                // dropped. Without this the loop would spin forever on a
                // socket nobody closes.
                Err(_) => break,
            },
            frame = socket.recv() => match frame {
                Some(Ok(Message::Binary(data))) => {
                    if debug {
                        crate::web_debug::append_input(
                            &format!("/ws/tabs/{tab_id}"),
                            &data,
                        );
                    }
                    let (reply_tx, reply_rx) = oneshot();
                    let cmd = ApiCommand::WebInput {
                        id: TabSelector::Id(tab_id),
                        client: client.clone(),
                        bytes: data.to_vec(),
                        reply: reply_tx,
                    };
                    if cmd_tx.send(cmd).await.is_err() {
                        break;
                    }
                    match reply_rx.recv().await {
                        Ok(ApiReply::Ok { .. }) => {}
                        Ok(ApiReply::Err { status, error }) => {
                            eprintln!("web socket input rejected ({status}): {error}");
                            break;
                        }
                        Err(_) => break,
                    }
                }
                Some(Ok(Message::Text(text))) => {
                    if !control(&cmd_tx, tab_id, &client, &text).await {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }

    detach_stream(&cmd_tx, stream_id);
}

async fn attach(
    cmd_tx: &Sender<ApiCommand>,
    tab_id: u64,
    client: &str,
    stream_id: u64,
) -> Result<(Receiver<Vec<u8>>, Value), String> {
    let (reply_tx, reply_rx) = oneshot();
    let (stream_tx, stream_rx) = async_channel::bounded(1);
    cmd_tx
        .send(ApiCommand::WebStreamAttach {
            id: TabSelector::Id(tab_id),
            client: client.to_string(),
            stream_id,
            reply: reply_tx,
            stream: stream_tx,
        })
        .await
        .map_err(|_| "command channel closed".to_string())?;
    let reply = reply_rx
        .recv()
        .await
        .map_err(|_| "attach reply channel closed".to_string())?;
    let snapshot = match reply {
        ApiReply::Ok {
            body: ReplyBody::Json(value),
            ..
        } => value,
        ApiReply::Err { error, .. } => return Err(error),
        _ => return Err("unexpected attach reply".to_string()),
    };
    let rx = stream_rx
        .recv()
        .await
        .map_err(|_| "stream channel closed".to_string())?;
    Ok((rx, snapshot))
}

/// JSON control frames. Returns false when the connection should be dropped.
async fn control(cmd_tx: &Sender<ApiCommand>, tab_id: u64, client: &str, text: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return true;
    };
    match value.get("type").and_then(Value::as_str) {
        Some("resize") => {
            let (Some(cols), Some(rows)) = (
                value.get("cols").and_then(Value::as_u64),
                value.get("rows").and_then(Value::as_u64),
            ) else {
                return true;
            };
            if !(20..=300).contains(&cols) || !(5..=200).contains(&rows) {
                return true;
            }
            let (reply_tx, _) = oneshot();
            cmd_tx
                .send(ApiCommand::ResizeTab {
                    id: TabSelector::Id(tab_id),
                    cols: cols as u16,
                    rows: rows as u16,
                    client: Some(client.to_string()),
                    sync_window: value
                        .get("sync_window")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    reply: reply_tx,
                })
                .await
                .is_ok()
        }
        Some("heartbeat") => {
            let (reply_tx, _) = oneshot();
            cmd_tx
                .send(ApiCommand::WebClient {
                    id: client.to_string(),
                    connected: true,
                    cols: value.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16,
                    rows: value.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16,
                    browser_size: value
                        .get("browser_size")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    force: false,
                    reply: reply_tx,
                })
                .await
                .is_ok()
        }
        // A fresh screen snapshot is sent by the server on every attach; the
        // browser asks for another one by reconnecting.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_resize_range_validation() {
        // Receiver dropped: the channel is disconnected, so any send that reaches
        // it fails. A `false` return therefore means the resize passed validation
        // and was sent; a `true` return means it was rejected before any send
        // (out of range) — control() keeps the connection open in both cases.
        let (tx, rx) = async_channel::unbounded();
        drop(rx);
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                // Out of range (cols < 20): rejected before sending, kept open.
                assert!(control(&tx, 1, "c", r#"{"type":"resize","cols":1,"rows":30}"#).await);
                // In range: passes validation, send fails on the closed channel.
                assert!(
                    !control(&tx, 1, "c", r#"{"type":"resize","cols":80,"rows":24}"#)
                        .await
                );
            });
    }
}
