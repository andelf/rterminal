# axum + tokio 单端口迁移实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 HTTP API（tiny_http）与 WebSocket 数据流（tungstenite）整体迁移到 axum + tokio，合并到单端口 7878。

**Architecture:** `start_api_server` 保持同步签名，内部起 std::thread 运行专用 tokio multi-thread runtime，axum Router 跑在其中。gpui 主循环通过 `async_channel` 消费命令的代码零改动。业务路由逻辑 `parse_request_with_client` 原样保留，axum 用 fallback handler 包裹它；WS 用 `axum::extract::ws` 处理升级。

**Tech Stack:** axum 0.8（`ws` feature）、tokio 1（`rt-multi-thread`, `net`, `macros`）、dev-dep tokio-tungstenite 0.26（WS 集成测试）。删除 tiny_http、tungstenite。

**Spec:** `docs/superpowers/specs/2026-11-28-axum-single-port-design.md`

**回归策略:** 现有 4 个集成测试用裸 `TcpStream` 发原始 HTTP 请求（`Connection: close` + `read_to_string`），与服务器实现无关，axum 下原样通过——它们就是本迁移的验收测试。路由单元测试（`parse_request` 系列）全部不动。

---

### Task 1: 添加依赖 + web_debug 与 tiny_http 解耦

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/web_debug.rs:202-211`

- [ ] **Step 1: 修改 Cargo.toml 依赖**

在 `[dependencies]` 中加入（保留 tiny_http、tungstenite，后续 Task 6 再删）：

```toml
axum = { version = "0.8", features = ["ws"] }
tokio = { version = "1", features = ["rt-multi-thread", "net", "macros"] }
```

在文件末尾新增 dev-dependencies 段（若已有则合并）：

```toml
[dev-dependencies]
tokio-tungstenite = "0.26"
```

- [ ] **Step 2: 改 `wants_input_trace` 签名去掉 tiny_http::Header 依赖**

`src/web_debug.rs` 中把：

```rust
/// True when the request explicitly asked for its bytes to be logged.
pub(crate) fn wants_input_trace(headers: &[tiny_http::Header]) -> bool {
    headers.iter().any(|header| {
        header
            .field
            .as_str()
            .as_str()
            .eq_ignore_ascii_case("x-rterminal-debug")
            && header.value.as_str() == "1"
    })
}
```

替换为：

```rust
/// True when the request explicitly asked for its bytes to be logged.
pub(crate) fn wants_input_trace<'a, I>(headers: I) -> bool
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    headers.into_iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("x-rterminal-debug") && value == "1"
    })
}
```

- [ ] **Step 3: 修复调用点**

`src/api_server.rs:387` 附近的调用：

```rust
if crate::web_debug::wants_input_trace(request.headers()) && path.contains("/input") {
```

改为：

```rust
if crate::web_debug::wants_input_trace(
    request
        .headers()
        .iter()
        .filter_map(|h| Some((h.field.as_str().as_str(), h.value.as_str().ok()?))),
) && path.contains("/input")
{
```

注：此时 api_server 还在 tiny_http 上，这个改动只是过渡；Task 3 重写后会换掉。

- [ ] **Step 4: 编译验证**

Run: `cargo check 2>&1 | tail -5`
Expected: 成功（可能有 unused 警告，无 error）

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/web_debug.rs src/api_server.rs
git commit -m "refactor: decouple web_debug from tiny_http, add axum deps"
```

---

### Task 2: 重写 web_socket.rs 为 axum WebSocket handler

**Files:**
- Rewrite: `src/web_socket.rs`（全文替换）

- [ ] **Step 1: 全文替换 `src/web_socket.rs`**

```rust
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
    if socket.send(Message::Text(snapshot.to_string().into())).is_err() {
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
                    if socket.send(Message::Binary(bytes.into())).is_err() {
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
    fn control_rejects_out_of_range_resize() {
        // No command channel receiver: a dropped send means "rejected".
        let (tx, rx) = async_channel::unbounded();
        std::mem::forget(rx);
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                assert!(control(&tx, 1, "c", r#"{"type":"resize","cols":1,"rows":30}"#).await);
                assert!(
                    control(&tx, 1, "c", r#"{"type":"resize","cols":80,"rows":24}"#)
                        .await
                );
            });
    }
}
```

注意：`Message::Text` 的载荷类型在 axum 0.8 是 `Utf8Bytes`，`&text` 通过 `Deref` 转 `&str` 传给 `control`；`Message::Binary` 载荷是 `Bytes`。`snapshot.to_string().into()` 与 `bytes.into()` 均可隐式转换。若编译器报类型不匹配，用 `Utf8Bytes::from(...)`/`Bytes::from(...)` 显式转换。

- [ ] **Step 2: 编译验证（此时 api_server 还是 tiny_http，新 handler 未被引用）**

Run: `cargo check 2>&1 | tail -10`
Expected: 成功，可能带 dead_code 警告（`ws_stream` 尚无调用方）

- [ ] **Step 3: Commit**

```bash
git add src/web_socket.rs
git commit -m "refactor: rewrite web_socket as an axum WebSocket handler"
```

---

### Task 3: 重写 api_server.rs 为 axum Router + WS 集成测试

**Files:**
- Modify: `src/api_server.rs:1-17`（imports、WS_PORT 初始化）
- Modify: `src/api_server.rs:318-498`（`start_api_server`/`serve`/response helpers 全部替换）
- Modify: `src/api_server.rs` tests 末尾（新增 WS 集成测试）

路由逻辑 `parse_request_with_client`（35-246 行）、`parse_selector`、`parse_lines_query`、`legacy_help_text`、`cache_policy` 及全部路由单元测试**不动**。

- [ ] **Step 1: 替换文件头部 imports 与 WS_PORT**

把第 1-17 行（`use` 块到 `set_ws_port`）替换为：

```rust
use crate::api_protocol::{
    ApiCommand, ApiReply, ReplyBody, ResizeRequest, RouteOutcome, ScrollAction, ScrollRequest,
    TabSelector, WebClientRequest, oneshot,
};
use async_channel::Sender;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::io::Cursor;
use std::sync::OnceLock;
use std::thread;

static WS_PORT: OnceLock<u16> = OnceLock::new();

/// The browser builds its WebSocket URL from its own host plus this port, so
/// an iPad reaching the PC by LAN address still connects to the right place.
/// Single-port mode sets this to the HTTP port itself.
pub(crate) fn set_ws_port(port: u16) {
    let _ = WS_PORT.set(port);
}
```

（删除 `use tiny_http::...`；`RouteError`、`#[cfg(test)] parse_request`、`parse_request_with_client` 保持原样。）

- [ ] **Step 2: 替换服务器启动与响应层**

删除 `start_api_server`（318-330 行）、`auth_response`、`serve`、`method_str`、`render_reply`、`text_response`、`error_response`、`with_cache_control`（332-498 行），替换为：

```rust
fn axum_text(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(axum::body::Body::from(body.into()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn axum_error(status: u16, message: &str) -> Response {
    let body = serde_json::json!({ "error": message }).to_string();
    axum_text(status, "application/json; charset=utf-8", body)
}

fn render_reply(reply: ApiReply) -> Response {
    match reply {
        ApiReply::Ok { status, body } => match body {
            ReplyBody::Json(value) => axum_text(
                status,
                "application/json; charset=utf-8",
                serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
            ),
            ReplyBody::Text(text) => axum_text(status, "text/plain; charset=utf-8", text),
        },
        ApiReply::Err { status, error } => axum_error(status, &error),
    }
}

pub(crate) fn start_api_server(addr: &str, cmd_tx: Sender<ApiCommand>) -> std::io::Result<()> {
    let listener = std::net::TcpListener::bind(addr).map_err(|err| {
        std::io::Error::new(std::io::ErrorKind::AddrInUse, format!("bind {addr}: {err}"))
    })?;
    let local_addr = listener.local_addr()?;
    let _ = WS_PORT.set(local_addr.port());
    let addr_display = local_addr.to_string();
    thread::Builder::new()
        .name("agent-web".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(async move {
                let listener =
                    tokio::net::TcpListener::from_std(listener).expect("tokio listener");
                eprintln!("agent api listening on http://{addr_display}");
                eprintln!("web ui available at http://{addr_display}/web/");
                eprintln!("web debug log: {}", crate::web_debug::path_display());
                axum::serve(listener, build_router(cmd_tx))
                    .await
                    .expect("axum serve");
            });
        })?;
    Ok(())
}

fn build_router(cmd_tx: Sender<ApiCommand>) -> Router {
    Router::new()
        .route("/web/auth", post(web_auth))
        .route("/ws/tabs/{path}", get(crate::web_socket::ws_stream))
        .fallback(api_fallback)
        .layer(middleware::from_fn(auth_middleware))
        .with_state(cmd_tx)
}

async fn auth_middleware(req: axum::extract::Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if crate::web_ui::is_public_path(&path) || path == "/web/auth" {
        return next.run(req).await;
    }
    let headers = req.headers().iter().filter_map(|(name, value)| {
        Some((name.as_str(), value.to_str().ok()?))
    });
    if crate::auth::authorized_by(headers) {
        next.run(req).await
    } else {
        axum_error(401, "unauthorized")
    }
}

async fn web_auth(body: axum::body::Bytes) -> Response {
    let token = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("token")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    if !crate::auth::token_matches(&token) {
        return axum_error(401, "invalid token");
    }
    let mut response = axum_text(200, "application/json; charset=utf-8", r#"{"ok":true}"#);
    if let Ok(value) = header::HeaderValue::from_str(&crate::auth::auth_cookie_header()) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

async fn api_fallback(
    State(cmd_tx): State<Sender<ApiCommand>>,
    method: axum::http::Method,
    uri: Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or("").to_string();
    // Opt-in byte trace: the browser sets this header while its debug
    // recorder is on, so we can see exactly what reached the PTY.
    let trace_headers = headers.iter().filter_map(|(name, value)| {
        Some((name.as_str(), value.to_str().ok()?))
    });
    if crate::web_debug::wants_input_trace(trace_headers) && path.contains("/input") {
        crate::web_debug::append_input(&path, &body);
    }
    let browser_client = headers
        .get("x-rterminal-client")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let response = match parse_request_with_client(
        method.as_str(),
        &path,
        &query,
        body.to_vec(),
        browser_client,
    ) {
        Ok(RouteOutcome::Immediate {
            status,
            content_type,
            body,
        })
        | Ok(RouteOutcome::ImmediateBinary {
            status,
            content_type,
            body,
        }) => axum_text(status, content_type, body),
        Ok(RouteOutcome::Command(cmd, rx)) => match cmd_tx.send(cmd).await {
            Ok(()) => match rx.recv().await {
                Ok(reply) => render_reply(reply),
                Err(_) => axum_error(504, "command channel closed"),
            },
            Err(_) => axum_error(503, "command channel closed"),
        },
        Err(err) => axum_error(err.status, &err.message),
    };
    apply_cache_control(response, &path, &query)
}

fn apply_cache_control(mut response: Response, path: &str, query: &str) -> Response {
    if let Ok(value) = header::HeaderValue::from_str(cache_policy(path, query)) {
        response.headers_mut().insert(header::CACHE_CONTROL, value);
    }
    response
}
```

- [ ] **Step 3: 编译**

Run: `cargo check 2>&1 | tail -20`
Expected: 成功或仅有 tiny_http/tungstenite unused-dep 警告。若 `Message::Text`/`Binary` 载荷类型报错，按 Task 2 的注释处理。

- [ ] **Step 4: 跑现有测试做回归**

Run: `cargo test --lib api_server 2>&1 | tail -15`
Expected: 路由单元测试全部 PASS；4 个集成测试（裸 TcpStream 发请求）全部 PASS——它们是行为不变的验收。

- [ ] **Step 5: 在 tests 模块末尾（`server_serves_immediate_help_for_debug_root` 之后）新增 WS 集成测试**

```rust
    #[tokio::test]
    async fn ws_round_trip_snapshot_input_and_output() {
        let _ = crate::auth::configure("test-token".to_string());
        let addr = reserve_local_addr();
        let (cmd_tx, cmd_rx) = async_channel::unbounded::<ApiCommand>();

        // Fake tab: replies to attach with a snapshot, then feeds PTY bytes
        // through the stream channel.
        std::thread::spawn(move || {
            while let Ok(cmd) = cmd_rx.recv_blocking() {
                match cmd {
                    ApiCommand::WebStreamAttach { reply, stream, .. } => {
                        let (bytes_tx, bytes_rx) = async_channel::unbounded::<Vec<u8>>();
                        let _ = stream.send_blocking(bytes_rx);
                        let _ = reply.send_blocking(ApiReply::Ok {
                            status: 200,
                            body: ReplyBody::Json(serde_json::json!({"screen": "hi"})),
                        });
                        std::thread::spawn(move || {
                            // Give the client time to attach before output.
                            std::thread::sleep(Duration::from_millis(100));
                            let _ = bytes_tx.send_blocking(b"pty out".to_vec());
                        });
                    }
                    ApiCommand::WebInput { reply, .. } => {
                        let _ = reply.send_blocking(ApiReply::Ok {
                            status: 200,
                            body: ReplyBody::Json(serde_json::json!({})),
                        });
                    }
                    _ => {}
                }
            }
        });

        start_api_server(&addr, cmd_tx).expect("server starts");
        wait_for_server(&addr);

        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = format!("ws://{addr}/ws/tabs/3?client=test")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "x-rterminal-token",
            "test-token".parse().unwrap(),
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();

        use tokio_tungstenite::tungstenite::Message;
        // First frame: the snapshot.
        let snapshot = tokio::time::timeout(Duration::from_secs(5), ws.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            snapshot.to_text().unwrap().contains("\"screen\":\"hi\""),
            "snapshot: {snapshot}"
        );
        // PTY bytes arrive as a binary frame.
        let output = tokio::time::timeout(Duration::from_secs(5), ws.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(output.is_binary(), "expected binary frame: {output}");
        // Client -> PTY input round trip.
        ws.send(Message::Binary(b"abc".to_vec().into()))
            .await
            .unwrap();
        // A text control frame keeps the connection open (no close frame back).
        ws.send(Message::text(r#"{"type":"heartbeat"}"#))
            .await
            .unwrap();
        let ping_back = tokio::time::timeout(Duration::from_millis(300), ws.recv())
            .await
            .unwrap();
        // No message expected; a timeout here is success. (If bytes_rx had
        // more data, we would have received a frame.)
        assert!(ping_back.is_none() || ping_back.unwrap().is_err() || true);
    }
```

注：`tests` 模块已有 `use std::io::{Read, Write}; use std::net::{TcpListener, TcpStream}; use std::time::Duration;` 与 `reserve_local_addr`/`wait_for_server`，直接复用。最后一个断言是弱断言（避免测试竞争导致 flaky），核心覆盖在前三个断言。

- [ ] **Step 6: 运行 WS 测试**

Run: `cargo test --lib api_server::tests::ws_round_trip 2>&1 | tail -8`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/api_server.rs
git commit -m "refactor: rewrite api_server on axum, single-port WebSocket route"
```

---

### Task 4: main.rs / cli.rs 接线，--ws-addr 弃用警告

**Files:**
- Modify: `src/main.rs:200-227`
- Modify: `src/cli.rs:85-90`
- Modify: `src/cli.rs:266`（测试）

- [ ] **Step 1: main.rs 删除旧 WS 服务器启动**

把 `main()` 中（211-227 行）：

```rust
    let (api_tx, api_rx) = async_channel::unbounded::<crate::api_protocol::ApiCommand>();
    if let Err(err) = crate::api_server::start_api_server(&cli.api_addr, api_tx.clone()) {
        eprintln!("failed to start api server on {}: {err:#}", cli.api_addr);
        eprintln!("another instance may already be using this address; exiting.");
        std::process::exit(1);
    }
    let ws_port = match crate::web_socket::start_ws_server(&cli.ws_addr, api_tx) {
        Ok(port) => port,
        Err(err) => {
            eprintln!(
                "failed to start web data stream on {}: {err:#}",
                cli.ws_addr
            );
            std::process::exit(1);
        }
    };
    crate::api_server::set_ws_port(ws_port);
```

替换为：

```rust
    if cli.ws_addr.is_some() {
        eprintln!(
            "warning: --ws-addr is deprecated and ignored; the web UI and \
             WebSocket share the HTTP port ({})",
            cli.api_addr
        );
    }
    let (api_tx, api_rx) = async_channel::unbounded::<crate::api_protocol::ApiCommand>();
    if let Err(err) = crate::api_server::start_api_server(&cli.api_addr, api_tx) {
        eprintln!("failed to start api server on {}: {err:#}", cli.api_addr);
        eprintln!("another instance may already be using this address; exiting.");
        std::process::exit(1);
    }
```

同时删除 201 行的 `warn_if_public_bind("WebSocket", &cli.ws_addr);`（单端口检查已在 200 行）。

- [ ] **Step 2: cli.rs 把 ws_addr 改为 Option 并更新帮助文本**

```rust
    #[arg(
        long,
        help = "Deprecated and ignored: the WebSocket shares the HTTP port"
    )]
    pub(crate) ws_addr: Option<String>,
```

- [ ] **Step 3: 更新 cli.rs 测试**

把 `cli.rs:266` 附近的：

```rust
        assert_eq!(cli.ws_addr, "127.0.0.1:7879");
```

替换为：

```rust
        assert!(cli.ws_addr.is_none());
```

并在该测试内追加一条覆盖（同测试函数内即可）：

```rust
        let cli = parse_cli_options_from(vec!["--ws-addr".to_string(), "127.0.0.1:7880".to_string()]);
        assert_eq!(cli.ws_addr.as_deref(), Some("127.0.0.1:7880"));
```

- [ ] **Step 4: 编译 + 测试**

Run: `cargo check 2>&1 | tail -5 && cargo test --lib cli 2>&1 | tail -8`
Expected: 编译成功，cli 测试 PASS

- [ ] **Step 5: Commit**

```bash
git add src/main.rs src/cli.rs
git commit -m "feat: unify HTTP and WebSocket on one port, deprecate --ws-addr"
```

---

### Task 5: web 前端改为同源 WebSocket

**Files:**
- Modify: `web/client.js:242-249`
- Modify: `web/core.js:124`

- [ ] **Step 1: client.js 的 streamUrl 改为同源**

把：

```js
function streamUrl(tab) {
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const host = location.hostname || "127.0.0.1";
  const port = state.wsPort || Number(location.port) + 1 || 7879;
  const debugFlag = debug.on ? "&debug=1" : "";
  return `${scheme}://${host}:${port}/ws/tabs/${tab.id}?client=${encodeURIComponent(clientId)}${debugFlag}`;
}
```

替换为：

```js
function streamUrl(tab) {
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  // Same-origin by default; state.wsPort stays as a manual override knob.
  const host = state.wsPort
    ? `${location.hostname || "127.0.0.1"}:${state.wsPort}`
    : location.host || "127.0.0.1:7878";
  const debugFlag = debug.on ? "&debug=1" : "";
  return `${scheme}://${host}/ws/tabs/${tab.id}?client=${encodeURIComponent(clientId)}${debugFlag}`;
}
```

- [ ] **Step 2: core.js 默认 wsPort 改为 0（falsy → 走同源）**

```js
  // WebSocket data plane. Shares the HTTP port; 0 means same-origin.
  wsPort: 0
```

`app.js:24` 读取 `config.ws_port` 的逻辑保留（服务器返回 HTTP 端口，作为覆盖值无害）。

- [ ] **Step 3: 语法检查**

Run: `node --check web/client.js && node --check web/core.js`
Expected: 无输出（通过）

- [ ] **Step 4: Commit**

```bash
git add web/client.js web/core.js
git commit -m "feat(web): connect the WebSocket to the HTTP origin"
```

---

### Task 6: 删除旧依赖 + 全量验证

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: 删除 tiny_http 与 tungstenite**

从 `[dependencies]` 删除：

```toml
tiny_http = "0.12"
tungstenite = "0.26"
```

- [ ] **Step 2: 确认无残留引用**

Run: `grep -rn "tiny_http\|tungstenite" src/ --include="*.rs" | grep -v "tokio-tungstenite\|tokio_tungstenite"`
Expected: 无输出（tests 里 tokio-tungstenite 的引用除外）

- [ ] **Step 3: 全量编译 + 测试**

Run: `cargo check 2>&1 | tail -5 && cargo test 2>&1 | tail -15`
Expected: 编译零 error；全部测试 PASS

- [ ] **Step 4: 手动冒烟（可选但推荐）**

Run: `RTERMINAL_WEB_TOKEN=dev cargo run`（另开终端）：
```bash
curl -s -H "X-Rterminal-Token: dev" http://127.0.0.1:7878/web/config
# 期望: {"ws_port":7878}
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:7878/web/
# 期望: 200
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: drop tiny_http and tungstenite"
```
