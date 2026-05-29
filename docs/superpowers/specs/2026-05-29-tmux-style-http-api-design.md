# Tmux-Style HTTP API for Multi-Tab Agent Control

**Date**: 2026-05-29
**Status**: Spec — ready for plan
**Replaces**: existing per-tab `/debug/*` HTTP server in `src/debug_server.rs`

## 1. Motivation

External agents need to drive rterminal like `tmux` from the CLI: list tabs, spawn new tabs, switch focus, send keystrokes, and capture screen content — all over a single stable HTTP endpoint. Today every `AgentTerminal` instance starts its own debug server on an auto-incrementing port (7878, 7879, …), each only knows about its own PTY, and there is no way for a client to enumerate tabs or address one by id. We replace that with one global HTTP server bound to `127.0.0.1` that owns the tab registry.

## 2. Goals & Non-Goals

**Goals (v1)**

- Single global HTTP server bound to `127.0.0.1`, default port `7878`.
- Stable tab addressing by numeric id (never reused after close); plus the single string alias `active` for the currently focused tab. No other aliases (no `last`, no name-based lookup) in v1.
- CRUD on tabs: list, create, close, switch active.
- Read per-tab state (`GET /tabs/:id`) and screen text (`GET /tabs/:id/screen`).
- Inject raw bytes (`POST /tabs/:id/input`) and tmux-style key tokens (`POST /tabs/:id/keys`).
- Backward-compatible aliases for current `/debug/*` endpoints routed to the active tab.

**Non-Goals (v1)**

- Scrollback / history access.
- Resize via API, rename, wait-for-text, SSE/WebSocket streaming.
- Multiple panes per tab.
- Authentication (server is bound to loopback only).
- Creating a tab with a custom command, cwd, or environment.

## 3. Architecture

```
┌────────────────────────────────────────────┐
│ gpui main thread                           │
│ ┌────────────────────────────────────────┐ │
│ │ TerminalTabs (entity)                  │ │
│ │  - tabs: Vec<TerminalTab>              │ │
│ │  - active_tab, next_tab_id             │ │
│ │  - api_inbox: Receiver<ApiCommand>     │ │
│ │  - drain task spawned in new()         │ │
│ └──────────────────▲─────────────────────┘ │
│                    │ async-channel          │
└────────────────────│───────────────────────┘
                     │
┌────────────────────│───────────────────────┐
│ http thread (tiny_http blocking accept)    │
│  parse request → ApiCommand { reply_tx }   │
│  send to api_inbox                         │
│  block on reply_rx (oneshot)               │
│  serialise reply → HTTP response           │
└────────────────────────────────────────────┘
```

### 3.1 Component changes

| File | Change |
|---|---|
| `src/debug_server.rs` | **Removed.** Move counters/note state into per-tab fields on `AgentTerminal`. |
| `src/api_server.rs` | **New.** HTTP listener + request → `ApiCommand` parser. |
| `src/api_keys.rs` | **New.** tmux-style key token parser → bytes. Pure function, fully unit-testable. |
| `src/api_protocol.rs` | **New.** `ApiCommand`, `ApiReply`, JSON DTOs (serde). |
| `src/tabs.rs` | Spawn drain task in `TerminalTabs::new`; add `apply_api_command`. |
| `src/terminal.rs` | Remove `start_debug_http_server` call. Expose helpers needed by `apply_api_command` (snapshot, write-input, current state). |
| `src/main.rs` | Start global `ApiServer` before opening the gpui window; pass `Sender<ApiCommand>` into `TerminalTabs::new`. |
| `src/cli.rs` | Add `--api-addr` (default `127.0.0.1:7878`). |

### 3.2 Cross-thread protocol

The HTTP thread is fully synchronous (tiny_http). The gpui main thread runs a drain task spawned via `cx.foreground_executor().spawn(...)` that awaits on the async-channel receiver and applies each command via `entity.update(cx, |this, cx| this.apply_api_command(cmd))`.

```rust
// api_protocol.rs (sketch)
pub(crate) enum ApiCommand {
    ListTabs { reply: oneshot::Sender<ApiReply> },
    CreateTab { reply: oneshot::Sender<ApiReply> },
    CloseTab { id: TabSelector, reply: oneshot::Sender<ApiReply> },
    ActivateTab { id: TabSelector, reply: oneshot::Sender<ApiReply> },
    GetTab { id: TabSelector, reply: oneshot::Sender<ApiReply> },
    GetScreen { id: TabSelector, reply: oneshot::Sender<ApiReply> },
    WriteInput { id: TabSelector, bytes: Vec<u8>, reply: oneshot::Sender<ApiReply> },
    SendKeys { id: TabSelector, tokens: String, reply: oneshot::Sender<ApiReply> },
    SetNote { id: TabSelector, note: Option<String>, reply: oneshot::Sender<ApiReply> },
    ReplaceLine { id: TabSelector, bytes: Vec<u8>, reply: oneshot::Sender<ApiReply> },
}

pub(crate) enum TabSelector { Id(u64), Active }

pub(crate) enum ApiReply {
    Ok { status: u16, body: ReplyBody },
    Err { status: u16, error: String },
}

pub(crate) enum ReplyBody {
    Json(serde_json::Value),
    Text(String),
    Empty,
}
```

`oneshot` is `async_channel::bounded(1)` (zero extra deps).

Timeout on the HTTP side: 5 seconds blocking on `reply_rx.recv_blocking()`; on timeout return `504 {"error":"gpui drain timed out"}` and update `last_error`.

## 4. Endpoint Catalogue

All requests/responses are `Content-Type: application/json` unless noted. Errors return `{"error":"…"}` with the documented status code.

### 4.1 Management

| Method | Path | Body | Success | Errors |
|---|---|---|---|---|
| `GET` | `/tabs` | — | 200 `{"active":3,"tabs":[{"id":1,"title":"zsh","kind":"terminal","cols":80,"rows":24}, ...]}` | — |
| `POST` | `/tabs` | empty | 201 `{"id":7,"title":"zsh","kind":"terminal"}` | 500 spawn failure |
| `DELETE` | `/tabs/:id` | — | 200 `{"closed":7}` | 404 unknown id |
| `POST` | `/tabs/:id/activate` | — | 200 `{"active":7}` | 404 unknown id |

`kind` is `"terminal"` or `"snapshot"`. Snapshot tabs are read-only (writes return 409).

### 4.2 Observation

| Method | Path | Success body | Notes |
|---|---|---|---|
| `GET` | `/tabs/:id` | `{"id","title","kind","cols","rows","cursor_row","cursor_col","status","note","counters","uptime_ms","last_error"}` | `counters` = `{bytes_from_pty,bytes_to_pty,key_events,injected_events,resize_events,http_requests}` |
| `GET` | `/tabs/:id/screen` | `text/plain; charset=utf-8`, current visible grid joined by `\n`, trailing `\n` | non-JSON for `curl \| grep` ergonomics |

### 4.3 Input

| Method | Path | Body | Success | Errors |
|---|---|---|---|---|
| `POST` | `/tabs/:id/input` | raw bytes (any content-type) | 200 `{"wrote":N}` | 400 empty body, 409 snapshot tab, 503 writer unavailable |
| `POST` | `/tabs/:id/keys` | `text/plain` tmux-style tokens | 200 `{"wrote":N}` | 400 parse error (`{"error":"unknown key token: Foo"}`), 409, 503 |

### 4.4 Legacy `/debug/*` aliases

Translated to the **currently active** tab. Returns the same body as the new endpoint.

| Legacy | New |
|---|---|
| `GET /debug` | text/plain listing of new + legacy endpoints |
| `GET /debug/state` | `GET /tabs/active` (the `shell` field is dropped — shell is per-tab now, available as `title`) |
| `GET /debug/screen` | `GET /tabs/active/screen` |
| `POST /debug/input` | `POST /tabs/active/input` |
| `POST /debug/replace-line` | active tab only via legacy path. No new `/tabs/:id/replace-line` endpoint in v1 — the existing helper is kept available for the current debug script but not promoted to the new API surface. |
| `POST /debug/note` | `POST /tabs/active/note` (note state migrates to per-tab) |

The drop of `shell` from `/debug/state` is the only intentional schema break; documented here for clients to handle.

## 5. Key Token Grammar (`/tabs/:id/keys`)

Body is plain text. Tokens separated by whitespace. Quoted runs (`"…"`) are literal text inserted verbatim.

**Tokens (case-sensitive)**:

- **Modified**: `C-x` (Ctrl), `M-x` (Alt/Meta, emitted as ESC prefix before the underlying key bytes), `S-Tab` (Shift — accepted **only** in front of named keys like `Tab`, never with letters; use `A` instead of `S-a`). Combinations: any order accepted (`C-M-x` == `M-C-x`); duplicates rejected (`C-C-x` → 400).
- **Named**: `Enter` (`\r`), `Tab` (`\t`), `Escape` (`\x1b`), `Space` (` `), `BSpace` (`\x7f`), `Up` (`\x1b[A`), `Down` (`\x1b[B`), `Right` (`\x1b[C`), `Left` (`\x1b[D`), `Home` (`\x1b[H`), `End` (`\x1b[F`), `PageUp` (`\x1b[5~`), `PageDown` (`\x1b[6~`), `F1`..`F12` (xterm sequences).
- **Literal single char**: any printable ASCII other than the reserved tokens above — `a`, `A`, `1`, `!`, `/`, etc.
- **Literal string**: `"…"`. Inside the quotes, only `\"` and `\\` are escaped; everything else (including spaces) is verbatim. No support for `\n`/`\t` escapes — use the named tokens instead.

**Errors**: unknown token, unterminated quote, or unknown modifier combination → 400 with `{"error":"…"}` and **no bytes written** (parse fully first, then write atomically).

**Examples**:

| Input body | Bytes written |
|---|---|
| `Enter` | `\r` |
| `C-c` | `\x03` |
| `C-a "echo hi" Enter` | `\x01echo hi\r` |
| `M-b` | `\x1bb` |
| `Up Up Enter` | `\x1b[A\x1b[A\r` |

## 6. Lifecycle & Failure Modes

- **Bind failure** at startup: log to stderr, exit with non-zero status. (The HTTP API is core to the agent workflow now; failure to bind is fatal.)
- **Tab not found**: 404 `{"error":"unknown tab id: 42"}`.
- **Active alias with no tabs**: 404 `{"error":"no active tab"}`. Can only happen during the narrow window before the first tab is created — `TerminalTabs::new` creates one synchronously, so in normal runtime there is always ≥1 tab.
- **Snapshot writes**: 409 `{"error":"cannot write to snapshot tab"}`.
- **Drain timeout**: see §3.2.
- **Graceful shutdown**: when the gpui app quits, the drain task drops the receiver; the HTTP thread will then fail on the next `send_blocking` and exit. No explicit join — daemon thread.

## 7. Testing Strategy

| Layer | What | How |
|---|---|---|
| Unit | Key token parser (`api_keys.rs`) | Pure function tests covering each named key, modifier combinations, literal quote handling, malformed input. |
| Unit | URL/method router (`api_server.rs`) | Table-driven test mapping `(method, path)` to expected `ApiCommand` variant. No real network. |
| Integration | End-to-end HTTP round-trip | Spawn a fake gpui-side handler (just a thread reading from `Receiver<ApiCommand>` and replying), then drive the HTTP server with `TcpStream`. Mirrors the existing tests in `debug_server.rs`. |
| Manual | Real binary | `cargo run` + `curl` against documented endpoints. Listed in §9. |

Tests for the legacy `/debug/*` aliases assert they hit the same code path as the new endpoints (one parametrised test, both URLs).

## 8. Implementation Order

1. `api_protocol.rs` — types only, no logic. Compile-checked.
2. `api_keys.rs` — pure parser + comprehensive unit tests. TDD.
3. `api_server.rs` — router + request parsing + reply serialisation + tests using a fake handler thread.
4. Wire into `TerminalTabs`: add drain task and `apply_api_command` matcher. Remove per-`AgentTerminal` debug-server call.
5. Migrate note/counters state from `SharedDebugState` to per-tab fields on `AgentTerminal`.
6. `main.rs` + `cli.rs`: start global server, `--api-addr` flag.
7. Manual smoke (§9).

## 9. Smoke Plan

```bash
# Start
cargo run --release -- --api-addr 127.0.0.1:7878

# List
curl -s 127.0.0.1:7878/tabs

# Create + activate
curl -sX POST 127.0.0.1:7878/tabs
curl -sX POST 127.0.0.1:7878/tabs/2/activate

# Inspect
curl -s 127.0.0.1:7878/tabs/2
curl -s 127.0.0.1:7878/tabs/2/screen

# Drive
curl -sX POST -d 'ls -la' 127.0.0.1:7878/tabs/2/input
curl -sX POST -d 'Enter' 127.0.0.1:7878/tabs/2/keys
curl -sX POST -d 'C-c' 127.0.0.1:7878/tabs/active/keys

# Close
curl -sX DELETE 127.0.0.1:7878/tabs/2

# Legacy alias
curl -s 127.0.0.1:7878/debug/state   # → tabs/active
```

## 10. Open Items Deferred to Later Versions

- Tab creation with body params (`{"cwd":"…","cmd":"…"}`).
- Scrollback (`GET /tabs/:id/scrollback?lines=N`).
- Resize / rename endpoints.
- `wait` endpoint or SSE stream for line-appears / prompt-ready events.
- Optional token auth via env var.
- Plural-keys endpoint variant accepting JSON array for clients that prefer structured input.
