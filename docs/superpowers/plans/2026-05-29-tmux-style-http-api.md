# Tmux-Style HTTP API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the per-tab `/debug/*` HTTP server with one global tmux-style HTTP API server at `127.0.0.1:7878` that addresses tabs by stable id and supports list/create/close/activate, screen capture, raw input, and tmux-style key tokens.

**Architecture:** A single `tiny_http` server runs on its own thread. It parses each request into an `ApiCommand` carrying an `async-channel` oneshot reply, hands it to the gpui main thread via a global `Sender<ApiCommand>`, and blocks (5s) for the reply. The gpui side drains commands inside a `cx.foreground_executor()` task and applies them via `TerminalTabs::apply_api_command`, mutating `Entity<AgentTerminal>` instances directly.

**Tech Stack:** `tiny_http` 0.12 (HTTP), `async-channel` 2 (cross-thread + oneshot), `serde`/`serde_json` (DTOs), `clap` (CLI). All already in `Cargo.toml`.

**Spec:** `docs/superpowers/specs/2026-05-29-tmux-style-http-api-design.md`

---

## File Structure

**New files**
- `src/api_protocol.rs` — `ApiCommand`, `ApiReply`, `ReplyBody`, `TabSelector`, JSON DTOs.
- `src/api_keys.rs` — pure tmux-style key parser.
- `src/api_server.rs` — HTTP listener, request routing, response serialisation.

**Modified files**
- `src/cli.rs` — add `--api-addr`.
- `src/main.rs` — start API server, pass `Sender<ApiCommand>` into `TerminalTabs::new`.
- `src/tabs.rs` — accept sender, spawn drain task, implement `apply_api_command`.
- `src/terminal.rs` — drop `start_debug_http_server` call; expose a few small accessors (`tab_screen_text`, `tab_state_snapshot`, `set_note`, `write_input_bytes`, `replace_input_line`).

**Removed files**
- `src/debug_server.rs` — replaced entirely by `api_server.rs`. `SharedDebugState` data fields stay on `AgentTerminal` but the struct moves inline in `terminal.rs` (renamed `TabRuntimeState`).

---

## Task 1: Add `api_protocol` module with types only

Pure type definitions — no behaviour. Lets every subsequent task compile against stable signatures.

**Files:**
- Create: `src/api_protocol.rs`
- Modify: `src/main.rs:5` (add `mod api_protocol;`)

- [ ] **Step 1: Create `src/api_protocol.rs`**

```rust
use async_channel::{Receiver, Sender, bounded};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TabSelector {
    Id(u64),
    Active,
}

#[derive(Debug)]
pub(crate) enum ApiCommand {
    ListTabs { reply: Sender<ApiReply> },
    CreateTab { reply: Sender<ApiReply> },
    CloseTab { id: TabSelector, reply: Sender<ApiReply> },
    ActivateTab { id: TabSelector, reply: Sender<ApiReply> },
    GetTab { id: TabSelector, reply: Sender<ApiReply> },
    GetScreen { id: TabSelector, reply: Sender<ApiReply> },
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
    Empty,
}

pub(crate) fn oneshot() -> (Sender<ApiReply>, Receiver<ApiReply>) {
    bounded(1)
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
    pub(crate) kind: &'static str,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TabDetailDto {
    pub(crate) id: u64,
    pub(crate) title: String,
    pub(crate) kind: &'static str,
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
```

- [ ] **Step 2: Register the module**

Edit `src/main.rs` immediately after the existing `mod` block (around line 5):

```rust
mod api_protocol;
```

- [ ] **Step 3: Compile to make sure types parse**

Run: `cd /Users/mono/Repos/rterminal && cargo check`
Expected: PASS (warnings about unused types are fine).

- [ ] **Step 4: Commit**

```bash
cd /Users/mono/Repos/rterminal
git add src/api_protocol.rs src/main.rs
git commit -m "feat(api): add api_protocol types"
```

---

## Task 2: Implement tmux-style key parser (TDD)

Pure function `parse_keys(body: &str) -> Result<Vec<u8>, String>`. All-or-nothing: returns error before producing partial output.

**Files:**
- Create: `src/api_keys.rs`
- Modify: `src/main.rs` (add `mod api_keys;`)

- [ ] **Step 1: Create `src/api_keys.rs` with failing tests first**

```rust
//! tmux-style key token parser. Whitespace separates tokens; quoted runs
//! (`"…"`) are literal text. See spec §5.

pub(crate) fn parse_keys(body: &str) -> Result<Vec<u8>, String> {
    Err("not yet implemented".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_emits_cr() {
        assert_eq!(parse_keys("Enter").unwrap(), b"\r");
    }

    #[test]
    fn ctrl_c_emits_etx() {
        assert_eq!(parse_keys("C-c").unwrap(), b"\x03");
    }

    #[test]
    fn meta_x_emits_esc_prefix() {
        assert_eq!(parse_keys("M-x").unwrap(), b"\x1bx");
    }

    #[test]
    fn ctrl_meta_combined_either_order() {
        let lhs = parse_keys("C-M-x").unwrap();
        let rhs = parse_keys("M-C-x").unwrap();
        assert_eq!(lhs, rhs);
        assert_eq!(lhs, b"\x1b\x18"); // ESC + Ctrl-X (0x18)
    }

    #[test]
    fn ctrl_letter_case_insensitive() {
        // C-a and C-A are both 0x01
        assert_eq!(parse_keys("C-a").unwrap(), b"\x01");
        assert_eq!(parse_keys("C-A").unwrap(), b"\x01");
    }

    #[test]
    fn shift_tab_emits_csi_z() {
        assert_eq!(parse_keys("S-Tab").unwrap(), b"\x1b[Z");
    }

    #[test]
    fn shift_with_letter_rejected() {
        assert!(parse_keys("S-a").is_err());
    }

    #[test]
    fn arrows_emit_csi() {
        assert_eq!(parse_keys("Up").unwrap(), b"\x1b[A");
        assert_eq!(parse_keys("Down").unwrap(), b"\x1b[B");
        assert_eq!(parse_keys("Right").unwrap(), b"\x1b[C");
        assert_eq!(parse_keys("Left").unwrap(), b"\x1b[D");
    }

    #[test]
    fn function_keys_use_xterm_sequences() {
        assert_eq!(parse_keys("F1").unwrap(), b"\x1bOP");
        assert_eq!(parse_keys("F5").unwrap(), b"\x1b[15~");
        assert_eq!(parse_keys("F12").unwrap(), b"\x1b[24~");
    }

    #[test]
    fn literal_string_inserted_verbatim() {
        assert_eq!(parse_keys("\"echo hi\"").unwrap(), b"echo hi");
    }

    #[test]
    fn quoted_string_escapes_only_quote_and_backslash() {
        assert_eq!(parse_keys("\"a\\\"b\\\\c\"").unwrap(), b"a\"b\\c");
    }

    #[test]
    fn whitespace_separates_tokens() {
        assert_eq!(parse_keys("C-a \"ls\" Enter").unwrap(), b"\x01ls\r");
    }

    #[test]
    fn single_char_literal_token() {
        assert_eq!(parse_keys("a").unwrap(), b"a");
        assert_eq!(parse_keys("!").unwrap(), b"!");
    }

    #[test]
    fn unknown_token_rejected_atomically() {
        let err = parse_keys("Enter Foo Enter").unwrap_err();
        assert!(err.contains("Foo"), "error should mention bad token: {err}");
    }

    #[test]
    fn unterminated_quote_rejected() {
        assert!(parse_keys("\"oops").is_err());
    }

    #[test]
    fn duplicate_modifier_rejected() {
        assert!(parse_keys("C-C-x").is_err());
    }

    #[test]
    fn page_and_home_end() {
        assert_eq!(parse_keys("Home").unwrap(), b"\x1b[H");
        assert_eq!(parse_keys("End").unwrap(), b"\x1b[F");
        assert_eq!(parse_keys("PageUp").unwrap(), b"\x1b[5~");
        assert_eq!(parse_keys("PageDown").unwrap(), b"\x1b[6~");
    }

    #[test]
    fn empty_body_is_empty_bytes() {
        assert_eq!(parse_keys("").unwrap(), Vec::<u8>::new());
        assert_eq!(parse_keys("   ").unwrap(), Vec::<u8>::new());
    }
}
```

- [ ] **Step 2: Register the module and verify tests fail**

Edit `src/main.rs`:

```rust
mod api_keys;
```

Run: `cd /Users/mono/Repos/rterminal && cargo test api_keys`
Expected: All tests FAIL with "not yet implemented".

- [ ] **Step 3: Implement the parser**

Replace the entire body of `src/api_keys.rs` above the `#[cfg(test)]` line with:

```rust
//! tmux-style key token parser. Whitespace separates tokens; quoted runs
//! (`"…"`) are literal text. See spec §5.

#[derive(Clone, Copy, Default)]
struct Mods {
    ctrl: bool,
    meta: bool,
    shift: bool,
}

pub(crate) fn parse_keys(body: &str) -> Result<Vec<u8>, String> {
    let mut tokens = Vec::new();
    let mut iter = body.chars().peekable();
    while let Some(&c) = iter.peek() {
        if c.is_whitespace() {
            iter.next();
            continue;
        }
        if c == '"' {
            iter.next();
            let mut literal = String::new();
            loop {
                match iter.next() {
                    Some('\\') => match iter.next() {
                        Some('"') => literal.push('"'),
                        Some('\\') => literal.push('\\'),
                        Some(other) => return Err(format!("invalid escape: \\{other}")),
                        None => return Err("unterminated quoted string".to_string()),
                    },
                    Some('"') => break,
                    Some(ch) => literal.push(ch),
                    None => return Err("unterminated quoted string".to_string()),
                }
            }
            tokens.push(Token::Literal(literal));
            continue;
        }
        let mut token = String::new();
        while let Some(&ch) = iter.peek() {
            if ch.is_whitespace() {
                break;
            }
            token.push(ch);
            iter.next();
        }
        tokens.push(Token::Word(token));
    }

    let mut out = Vec::new();
    for token in tokens {
        match token {
            Token::Literal(text) => out.extend_from_slice(text.as_bytes()),
            Token::Word(word) => emit_word(&word, &mut out)?,
        }
    }
    Ok(out)
}

enum Token {
    Word(String),
    Literal(String),
}

fn emit_word(word: &str, out: &mut Vec<u8>) -> Result<(), String> {
    let (mods, key) = split_modifiers(word)?;
    emit_key(mods, key, out)
}

fn split_modifiers(word: &str) -> Result<(Mods, &str), String> {
    let mut mods = Mods::default();
    let mut rest = word;
    loop {
        let (prefix, tail) = match rest.split_once('-') {
            Some(parts) => parts,
            None => break,
        };
        // tail must not be empty (e.g. "C-" alone is invalid)
        if tail.is_empty() {
            break;
        }
        match prefix {
            "C" => {
                if mods.ctrl {
                    return Err(format!("duplicate Ctrl modifier in '{word}'"));
                }
                mods.ctrl = true;
            }
            "M" => {
                if mods.meta {
                    return Err(format!("duplicate Meta modifier in '{word}'"));
                }
                mods.meta = true;
            }
            "S" => {
                if mods.shift {
                    return Err(format!("duplicate Shift modifier in '{word}'"));
                }
                mods.shift = true;
            }
            _ => break,
        }
        rest = tail;
    }
    Ok((mods, rest))
}

fn emit_key(mods: Mods, key: &str, out: &mut Vec<u8>) -> Result<(), String> {
    // Shift is only meaningful in front of named special keys.
    let named = named_key_bytes(key);

    if mods.shift {
        match key {
            "Tab" => {
                if mods.meta {
                    out.push(0x1b);
                }
                out.extend_from_slice(b"\x1b[Z");
                return Ok(());
            }
            _ => return Err(format!("Shift modifier not supported with '{key}'")),
        }
    }

    if let Some(bytes) = named {
        if mods.ctrl {
            return Err(format!("Ctrl modifier not supported with named key '{key}'"));
        }
        if mods.meta {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        return Ok(());
    }

    // Single-character literal (with optional modifiers).
    let mut chars = key.chars();
    let ch = chars.next().ok_or_else(|| "empty key token".to_string())?;
    if chars.next().is_some() {
        return Err(format!("unknown key token: {key}"));
    }

    let base = ch as u32;
    if mods.ctrl {
        let upper = ch.to_ascii_uppercase() as u32;
        if !('A' as u32..=b'_' as u32).contains(&upper) && upper != '@' as u32 {
            return Err(format!("Ctrl modifier not supported with '{ch}'"));
        }
        let ctrl_byte = (upper - '@' as u32) as u8;
        if mods.meta {
            out.push(0x1b);
        }
        out.push(ctrl_byte);
        return Ok(());
    }

    if mods.meta {
        out.push(0x1b);
    }
    // Emit as UTF-8.
    let mut buf = [0u8; 4];
    let encoded = ch.encode_utf8(&mut buf);
    out.extend_from_slice(encoded.as_bytes());
    let _ = base; // suppress unused warning if expansion shifts
    Ok(())
}

fn named_key_bytes(name: &str) -> Option<&'static [u8]> {
    match name {
        "Enter" => Some(b"\r"),
        "Tab" => Some(b"\t"),
        "Escape" => Some(b"\x1b"),
        "Space" => Some(b" "),
        "BSpace" => Some(b"\x7f"),
        "Up" => Some(b"\x1b[A"),
        "Down" => Some(b"\x1b[B"),
        "Right" => Some(b"\x1b[C"),
        "Left" => Some(b"\x1b[D"),
        "Home" => Some(b"\x1b[H"),
        "End" => Some(b"\x1b[F"),
        "PageUp" => Some(b"\x1b[5~"),
        "PageDown" => Some(b"\x1b[6~"),
        "F1" => Some(b"\x1bOP"),
        "F2" => Some(b"\x1bOQ"),
        "F3" => Some(b"\x1bOR"),
        "F4" => Some(b"\x1bOS"),
        "F5" => Some(b"\x1b[15~"),
        "F6" => Some(b"\x1b[17~"),
        "F7" => Some(b"\x1b[18~"),
        "F8" => Some(b"\x1b[19~"),
        "F9" => Some(b"\x1b[20~"),
        "F10" => Some(b"\x1b[21~"),
        "F11" => Some(b"\x1b[23~"),
        "F12" => Some(b"\x1b[24~"),
        _ => None,
    }
}
```

- [ ] **Step 4: Run tests and fix any failures**

Run: `cd /Users/mono/Repos/rterminal && cargo test api_keys`
Expected: All tests PASS.

If `unknown_token_rejected_atomically` fails because earlier tokens were already written, the issue is that `parse_keys` writes tokens before fully validating them. The current implementation collects all tokens first then emits — that satisfies the atomic guarantee.

- [ ] **Step 5: Commit**

```bash
cd /Users/mono/Repos/rterminal
git add src/api_keys.rs src/main.rs
git commit -m "feat(api): tmux-style key parser with atomic validation"
```

---

## Task 3: Implement HTTP request → `ApiCommand` parser with table-driven tests

Pure function `parse_request(method, path, body) -> Result<ApiCommand, RouteError>`. No I/O. Each `ApiCommand` we generate carries a freshly-created `oneshot()` pair — callers wait on the receiver.

**Files:**
- Create: `src/api_server.rs`
- Modify: `src/main.rs` (add `mod api_server;`)

- [ ] **Step 1: Skeleton + failing routing tests**

Create `src/api_server.rs`:

```rust
use crate::api_protocol::{ApiCommand, ApiReply, TabSelector, oneshot};
use async_channel::Receiver;

#[derive(Debug)]
pub(crate) struct RouteError {
    pub(crate) status: u16,
    pub(crate) message: String,
}

pub(crate) fn parse_request(
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<(ApiCommand, Receiver<ApiReply>), RouteError> {
    Err(RouteError {
        status: 501,
        message: "not yet implemented".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(method: &str, path: &str, body: &[u8]) -> Result<ApiCommand, RouteError> {
        parse_request(method, path, body.to_vec()).map(|(cmd, _rx)| cmd)
    }

    #[test]
    fn get_tabs_routes_to_list() {
        assert!(matches!(route("GET", "/tabs", b"").unwrap(), ApiCommand::ListTabs { .. }));
    }

    #[test]
    fn post_tabs_routes_to_create() {
        assert!(matches!(route("POST", "/tabs", b"").unwrap(), ApiCommand::CreateTab { .. }));
    }

    #[test]
    fn delete_tab_by_id_routes_to_close() {
        match route("DELETE", "/tabs/7", b"").unwrap() {
            ApiCommand::CloseTab { id, .. } => assert_eq!(id, TabSelector::Id(7)),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn active_selector_resolves_to_active_variant() {
        match route("GET", "/tabs/active", b"").unwrap() {
            ApiCommand::GetTab { id, .. } => assert_eq!(id, TabSelector::Active),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn post_activate_routes() {
        assert!(matches!(
            route("POST", "/tabs/3/activate", b"").unwrap(),
            ApiCommand::ActivateTab { .. }
        ));
    }

    #[test]
    fn get_screen_routes() {
        assert!(matches!(
            route("GET", "/tabs/3/screen", b"").unwrap(),
            ApiCommand::GetScreen { .. }
        ));
    }

    #[test]
    fn post_input_carries_body_bytes() {
        match route("POST", "/tabs/3/input", b"echo hi\n").unwrap() {
            ApiCommand::WriteInput { bytes, id, .. } => {
                assert_eq!(bytes, b"echo hi\n");
                assert_eq!(id, TabSelector::Id(3));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn post_keys_carries_body_string() {
        match route("POST", "/tabs/3/keys", b"Enter").unwrap() {
            ApiCommand::SendKeys { body, .. } => assert_eq!(body, "Enter"),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn legacy_debug_state_maps_to_get_active() {
        match route("GET", "/debug/state", b"").unwrap() {
            ApiCommand::GetTab { id, .. } => assert_eq!(id, TabSelector::Active),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn legacy_debug_screen_maps_to_get_screen_active() {
        match route("GET", "/debug/screen", b"").unwrap() {
            ApiCommand::GetScreen { id, .. } => assert_eq!(id, TabSelector::Active),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn legacy_debug_input_maps_to_input_active() {
        match route("POST", "/debug/input", b"abc").unwrap() {
            ApiCommand::WriteInput { id, bytes, .. } => {
                assert_eq!(id, TabSelector::Active);
                assert_eq!(bytes, b"abc");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn legacy_debug_replace_line_maps_to_replace() {
        match route("POST", "/debug/replace-line", b"hi").unwrap() {
            ApiCommand::ReplaceLine { id, bytes, .. } => {
                assert_eq!(id, TabSelector::Active);
                assert_eq!(bytes, b"hi");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn legacy_debug_note_maps_to_set_note() {
        match route("POST", "/debug/note", b"hello").unwrap() {
            ApiCommand::SetNote { id, note, .. } => {
                assert_eq!(id, TabSelector::Active);
                assert_eq!(note.as_deref(), Some("hello"));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn empty_note_body_clears_note() {
        match route("POST", "/debug/note", b"   ").unwrap() {
            ApiCommand::SetNote { note, .. } => assert!(note.is_none()),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn unknown_path_returns_404() {
        let err = route("GET", "/nope", b"").unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[test]
    fn bad_id_returns_400() {
        let err = route("GET", "/tabs/notanumber", b"").unwrap_err();
        assert_eq!(err.status, 400);
    }
}
```

- [ ] **Step 2: Register module + run tests to confirm failure**

Edit `src/main.rs`:

```rust
mod api_server;
```

Run: `cd /Users/mono/Repos/rterminal && cargo test api_server`
Expected: All routing tests FAIL.

- [ ] **Step 3: Implement `parse_request`**

Replace the stub body in `src/api_server.rs` (keep the imports and the `RouteError` struct):

```rust
pub(crate) fn parse_request(
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<(ApiCommand, Receiver<ApiReply>), RouteError> {
    let (tx, rx) = oneshot();
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    let cmd = match (method, segments.as_slice()) {
        // /tabs management
        ("GET", ["tabs"]) => ApiCommand::ListTabs { reply: tx },
        ("POST", ["tabs"]) => ApiCommand::CreateTab { reply: tx },
        ("DELETE", ["tabs", sel]) => ApiCommand::CloseTab { id: parse_selector(sel)?, reply: tx },
        ("POST", ["tabs", sel, "activate"]) => {
            ApiCommand::ActivateTab { id: parse_selector(sel)?, reply: tx }
        }
        ("GET", ["tabs", sel]) => ApiCommand::GetTab { id: parse_selector(sel)?, reply: tx },
        ("GET", ["tabs", sel, "screen"]) => {
            ApiCommand::GetScreen { id: parse_selector(sel)?, reply: tx }
        }
        ("POST", ["tabs", sel, "input"]) => {
            ApiCommand::WriteInput { id: parse_selector(sel)?, bytes: body, reply: tx }
        }
        ("POST", ["tabs", sel, "keys"]) => ApiCommand::SendKeys {
            id: parse_selector(sel)?,
            body: String::from_utf8(body).map_err(|_| RouteError {
                status: 400,
                message: "keys body must be utf-8".to_string(),
            })?,
            reply: tx,
        },

        // legacy /debug aliases
        ("GET", ["debug"]) => {
            return Err(RouteError { status: 200, message: legacy_help_text() });
        }
        ("GET", ["debug", "state"]) => ApiCommand::GetTab { id: TabSelector::Active, reply: tx },
        ("GET", ["debug", "screen"]) => {
            ApiCommand::GetScreen { id: TabSelector::Active, reply: tx }
        }
        ("POST", ["debug", "input"]) => ApiCommand::WriteInput {
            id: TabSelector::Active,
            bytes: body,
            reply: tx,
        },
        ("POST", ["debug", "replace-line"]) => ApiCommand::ReplaceLine {
            id: TabSelector::Active,
            bytes: body,
            reply: tx,
        },
        ("POST", ["debug", "note"]) => {
            let raw = String::from_utf8(body).map_err(|_| RouteError {
                status: 400,
                message: "note body must be utf-8".to_string(),
            })?;
            let trimmed = raw.trim();
            let note = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
            ApiCommand::SetNote { id: TabSelector::Active, note, reply: tx }
        }

        _ => {
            return Err(RouteError {
                status: 404,
                message: format!("no route for {method} {path}"),
            });
        }
    };

    Ok((cmd, rx))
}

fn parse_selector(s: &str) -> Result<TabSelector, RouteError> {
    if s == "active" {
        return Ok(TabSelector::Active);
    }
    s.parse::<u64>()
        .map(TabSelector::Id)
        .map_err(|_| RouteError {
            status: 400,
            message: format!("invalid tab selector: {s}"),
        })
}

fn legacy_help_text() -> String {
    [
        "available endpoints:",
        "  GET    /tabs",
        "  POST   /tabs",
        "  DELETE /tabs/:id",
        "  POST   /tabs/:id/activate",
        "  GET    /tabs/:id",
        "  GET    /tabs/:id/screen",
        "  POST   /tabs/:id/input         (raw body)",
        "  POST   /tabs/:id/keys          (tmux key tokens)",
        "legacy:",
        "  GET    /debug                  (this page)",
        "  GET    /debug/state            → /tabs/active",
        "  GET    /debug/screen           → /tabs/active/screen",
        "  POST   /debug/input            → /tabs/active/input",
        "  POST   /debug/replace-line     active tab only",
        "  POST   /debug/note             active tab only",
        "",
    ]
    .join("\n")
}
```

The `("GET", ["debug"])` branch uses `RouteError` with `status: 200` as a sentinel for "this is a successful text response, not an error". That's ugly — fix in step 4.

- [ ] **Step 4: Clean up the `GET /debug` special case**

Change `parse_request`'s return type by adding a new branch instead of misusing `RouteError`. Add to `src/api_protocol.rs`:

```rust
pub(crate) enum RouteOutcome {
    Command(ApiCommand, async_channel::Receiver<ApiReply>),
    Immediate { status: u16, content_type: &'static str, body: String },
}
```

Then change `parse_request` to return `Result<RouteOutcome, RouteError>`:

```rust
pub(crate) fn parse_request(
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<RouteOutcome, RouteError> {
    let (tx, rx) = oneshot();
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    let cmd = match (method, segments.as_slice()) {
        // ... same as before, but `("GET", ["debug"])` becomes:
        ("GET", ["debug"]) => {
            return Ok(RouteOutcome::Immediate {
                status: 200,
                content_type: "text/plain; charset=utf-8",
                body: legacy_help_text(),
            });
        }
        // ... rest unchanged ...
    };

    Ok(RouteOutcome::Command(cmd, rx))
}
```

Update the routing tests to unwrap `RouteOutcome::Command`. Add helper at the top of `tests`:

```rust
fn route(method: &str, path: &str, body: &[u8]) -> Result<ApiCommand, RouteError> {
    match parse_request(method, path, body.to_vec())? {
        RouteOutcome::Command(cmd, _rx) => Ok(cmd),
        RouteOutcome::Immediate { .. } => panic!("expected command, got immediate response"),
    }
}

#[test]
fn legacy_debug_root_is_immediate_text() {
    match parse_request("GET", "/debug", Vec::new()).unwrap() {
        RouteOutcome::Immediate { status, body, .. } => {
            assert_eq!(status, 200);
            assert!(body.contains("/tabs"));
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}
```

- [ ] **Step 5: Run tests and verify**

Run: `cd /Users/mono/Repos/rterminal && cargo test api_server`
Expected: All routing tests PASS.

- [ ] **Step 6: Commit**

```bash
cd /Users/mono/Repos/rterminal
git add src/api_server.rs src/api_protocol.rs src/main.rs
git commit -m "feat(api): route HTTP requests to ApiCommand variants"
```

---

## Task 4: HTTP listener thread with fake-handler integration test

Wraps `parse_request` with a `tiny_http` server. Handler thread reads `Receiver<ApiCommand>` on the other end. Test uses a fake handler.

**Files:**
- Modify: `src/api_server.rs` (add `start_api_server` + integration test)

- [ ] **Step 1: Add the listener function (skeleton)**

Append to `src/api_server.rs`:

```rust
use async_channel::Sender;
use std::io::{Cursor, Read};
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server, StatusCode};

const RECV_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn start_api_server(addr: &str, cmd_tx: Sender<ApiCommand>) -> std::io::Result<()> {
    let server = Server::http(addr).map_err(|err| {
        std::io::Error::new(std::io::ErrorKind::AddrInUse, format!("bind {addr}: {err}"))
    })?;
    let addr = addr.to_string();
    thread::Builder::new()
        .name("agent-api-http".to_string())
        .spawn(move || serve(server, cmd_tx, addr))?;
    Ok(())
}

fn serve(server: Server, cmd_tx: Sender<ApiCommand>, addr: String) {
    eprintln!("agent api listening on http://{addr}");
    for mut request in server.incoming_requests() {
        let method = method_str(request.method()).to_string();
        let path = request.url().split('?').next().unwrap_or("/").to_string();
        let mut body = Vec::new();
        if let Err(err) = request.as_reader().read_to_end(&mut body) {
            let _ = request.respond(error_response(400, &format!("read body: {err}")));
            continue;
        }
        let response = match parse_request(&method, &path, body) {
            Ok(RouteOutcome::Immediate { status, content_type, body }) => {
                text_response(status, content_type, body)
            }
            Ok(RouteOutcome::Command(cmd, rx)) => match cmd_tx.send_blocking(cmd) {
                Ok(()) => match rx.recv_blocking() {
                    Ok(reply) => render_reply(reply),
                    Err(_) => error_response(504, "command channel closed"),
                },
                Err(_) => error_response(503, "command channel closed"),
            },
            Err(err) => error_response(err.status, &err.message),
        };
        if let Err(err) = request.respond(response) {
            eprintln!("agent api: failed to send response: {err}");
        }
    }
}

fn method_str(m: &Method) -> &'static str {
    match m {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        Method::Patch => "PATCH",
        _ => "OTHER",
    }
}

fn render_reply(reply: ApiReply) -> Response<Cursor<Vec<u8>>> {
    match reply {
        ApiReply::Ok { status, body } => match body {
            ReplyBody::Json(value) => text_response(
                status,
                "application/json; charset=utf-8",
                serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
            ),
            ReplyBody::Text(text) => text_response(status, "text/plain; charset=utf-8", text),
            ReplyBody::Empty => Response::from_data(Vec::<u8>::new()).with_status_code(StatusCode(status)),
        },
        ApiReply::Err { status, error } => error_response(status, &error),
    }
}

fn text_response(
    status: u16,
    content_type: &str,
    body: impl Into<Vec<u8>>,
) -> Response<Cursor<Vec<u8>>> {
    let mut resp = Response::from_data(body.into()).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes("Content-Type", content_type) {
        resp = resp.with_header(header);
    }
    resp
}

fn error_response(status: u16, message: &str) -> Response<Cursor<Vec<u8>>> {
    let body = serde_json::json!({ "error": message }).to_string();
    text_response(status, "application/json; charset=utf-8", body)
}

// imports the existing module uses
use crate::api_protocol::{ReplyBody, RouteOutcome};
```

(Note: `ReplyBody` and `RouteOutcome` should already be imported at the top via `use crate::api_protocol::*` — clean up duplicate `use` lines.)

- [ ] **Step 2: Add end-to-end integration test using a fake handler**

Append inside the `#[cfg(test)] mod tests` block:

```rust
use crate::api_protocol::{ApiReply, ReplyBody, oneshot as protocol_oneshot};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

fn reserve_local_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    addr.to_string()
}

fn wait_for_server(addr: &str) {
    for _ in 0..40 {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("server did not start: {addr}");
}

fn send_http(addr: &str, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream.write_all(request.as_bytes()).expect("write");
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    response
}

#[test]
fn server_round_trip_get_tabs_via_fake_handler() {
    let addr = reserve_local_addr();
    let (cmd_tx, cmd_rx) = async_channel::unbounded::<ApiCommand>();

    std::thread::spawn(move || {
        while let Ok(cmd) = cmd_rx.recv_blocking() {
            if let ApiCommand::ListTabs { reply } = cmd {
                let body = serde_json::json!({
                    "active": 1,
                    "tabs": [{"id":1,"title":"zsh","kind":"terminal","cols":80,"rows":24}],
                });
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Json(body),
                });
            }
        }
    });

    start_api_server(&addr, cmd_tx).expect("server starts");
    wait_for_server(&addr);

    let response = send_http(
        &addr,
        &format!("GET /tabs HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    );
    assert!(response.contains("200 OK"), "response: {response}");
    assert!(response.contains("\"active\":1"), "response: {response}");
}

#[test]
fn server_returns_404_for_unknown_route() {
    let addr = reserve_local_addr();
    let (cmd_tx, _cmd_rx) = async_channel::unbounded::<ApiCommand>();
    start_api_server(&addr, cmd_tx).expect("server starts");
    wait_for_server(&addr);

    let response = send_http(
        &addr,
        &format!("GET /nope HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    );
    assert!(response.contains("404"), "response: {response}");
}
```

- [ ] **Step 3: Run + commit**

Run: `cd /Users/mono/Repos/rterminal && cargo test api_server`
Expected: PASS (including the new integration tests).

```bash
cd /Users/mono/Repos/rterminal
git add src/api_server.rs
git commit -m "feat(api): HTTP listener thread with end-to-end round-trip test"
```

---

## Task 5: Add `--api-addr` CLI flag

**Files:**
- Modify: `src/cli.rs`

- [ ] **Step 1: Add the flag**

Add a field inside `CliOptions` (before the closing `}` of the struct, near the other args):

```rust
#[arg(
    long,
    default_value = "127.0.0.1:7878",
    help = "Address to bind the agent control HTTP API on (host:port)"
)]
pub(crate) api_addr: String,
```

- [ ] **Step 2: Add a test confirming the default**

Inside the `tests` module of `src/cli.rs`:

```rust
#[test]
fn api_addr_defaults_to_loopback() {
    let cli = parse_cli_options_from(Vec::<String>::new());
    assert_eq!(cli.api_addr, "127.0.0.1:7878");
}

#[test]
fn api_addr_accepts_override() {
    let cli = parse_cli_options_from(vec!["--api-addr".to_string(), "127.0.0.1:9000".to_string()]);
    assert_eq!(cli.api_addr, "127.0.0.1:9000");
}
```

- [ ] **Step 3: Run + commit**

Run: `cd /Users/mono/Repos/rterminal && cargo test cli::tests::api_addr`
Expected: PASS.

```bash
cd /Users/mono/Repos/rterminal
git add src/cli.rs
git commit -m "feat(cli): add --api-addr flag for the HTTP control surface"
```

---

## Task 6: Migrate `SharedDebugState` to a new `TabRuntimeState` inside `terminal.rs`

We're going to delete `debug_server.rs` at the end. The data inside `SharedDebugState` needs to live on the tab as `TabRuntimeState` so other modules can read it via `&AgentTerminal`. **No behavioural change in this task** — just rename the type and move it inline. This keeps Task 7 focused on wiring.

**Files:**
- Modify: `src/debug_server.rs` (export only `SharedDebugState` data fields)
- Modify: `src/terminal.rs` (use the moved type)

- [ ] **Step 1: Move the type body**

Create a new sibling file `src/tab_runtime.rs`:

```rust
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
        let mut out = state.screen_lines.join("\n");
        out.push('\n');
        out
    }
}
```

- [ ] **Step 2: Register module + switch terminal.rs to it**

Edit `src/main.rs` (add `mod tab_runtime;`).

In `src/terminal.rs`:

```rust
// Replace:
use crate::debug_server::{SharedDebugState, start_debug_http_server};
// With:
use crate::tab_runtime::TabRuntimeState;
```

Then replace every `SharedDebugState` token in `terminal.rs` with `TabRuntimeState` (the API matches 1:1 except `state_json` → `snapshot` which is only used by the HTTP layer, and `status_summary` which is unused in terminal.rs). Also remove the `start_debug_http_server(debug.clone(), writer.clone());` line (around `terminal.rs:329`) — that was the per-tab server startup.

Use Edit with `replace_all: true` for the rename:

```
Edit src/terminal.rs:
  old_string: "SharedDebugState"
  new_string: "TabRuntimeState"
  replace_all: true
```

Then locate and remove the `start_debug_http_server(debug.clone(), writer.clone());` line.

- [ ] **Step 3: Compile**

Run: `cd /Users/mono/Repos/rterminal && cargo build`
Expected: PASS. (debug_server.rs still exists and has its now-orphaned types, but it's no longer referenced — should compile with unused-code warnings.)

- [ ] **Step 4: Verify existing tests still pass**

Run: `cd /Users/mono/Repos/rterminal && cargo test`
Expected: All pass except the three tests inside `debug_server.rs` which exercise the now-removed per-tab HTTP server. Comment them out (`#[ignore]`) for now — we delete the whole file in Task 9.

```rust
// In src/debug_server.rs, prepend each #[test] with #[ignore = "replaced by api_server, removed in plan task 9"]
```

- [ ] **Step 5: Commit**

```bash
cd /Users/mono/Repos/rterminal
git add src/tab_runtime.rs src/terminal.rs src/main.rs src/debug_server.rs
git commit -m "refactor: extract TabRuntimeState into its own module"
```

---

## Task 7: Wire the API server into `main.rs` and `TerminalTabs`

**Files:**
- Modify: `src/main.rs`
- Modify: `src/tabs.rs`

- [ ] **Step 1: Start the server before opening the window**

Replace the body of `fn main()` in `src/main.rs` so that it starts the API server before `application().run(...)` and threads the sender into `TerminalTabs`:

```rust
fn main() {
    let cli = parse_cli_options();

    if cli.self_check {
        if let Err(err) = run_self_check() {
            eprintln!("self-check failed: {err:#}");
            std::process::exit(1);
        }
        return;
    }

    let (api_tx, api_rx) = async_channel::unbounded::<crate::api_protocol::ApiCommand>();
    if let Err(err) = crate::api_server::start_api_server(&cli.api_addr, api_tx) {
        eprintln!("failed to start api server on {}: {err:#}", cli.api_addr);
        std::process::exit(1);
    }

    application().run(move |cx: &mut App| {
        // ... existing key bindings, menus ...

        let bounds = Bounds::centered(None, size(px(1000.0), px(520.0)), cx);
        let cli = cli.clone();
        let api_rx = api_rx.clone();
        cx.open_window(
            WindowOptions { /* unchanged */ ..Default::default() },
            move |window, cx| {
                let cli = cli.clone();
                let api_rx = api_rx.clone();
                cx.new(|cx| TerminalTabs::new(window, cx, cli, api_rx))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
```

Add the `async_channel` import at the top if it isn't there, and ensure `mod api_server;` and `mod tab_runtime;` are registered.

- [ ] **Step 2: Update `TerminalTabs::new` to accept the receiver**

In `src/tabs.rs`, change the signature:

```rust
pub(crate) fn new(
    window: &mut Window,
    cx: &mut Context<Self>,
    cli: CliOptions,
    api_rx: async_channel::Receiver<crate::api_protocol::ApiCommand>,
) -> Self {
```

Store the drain task on the struct so it stays alive:

```rust
pub(crate) struct TerminalTabs {
    cli: CliOptions,
    tabs: Vec<TerminalTab>,
    active_tab: usize,
    next_tab_id: usize,
    next_snapshot_id: usize,
    pending_focus_sync: bool,
    _api_drain: gpui::Task<()>,
}
```

At the bottom of `new`, before returning:

```rust
let api_drain = cx.spawn(async move |this, cx| {
    while let Ok(cmd) = api_rx.recv().await {
        let _ = this.update(cx, |this, cx| this.apply_api_command(cmd, cx));
    }
});

let mut this = Self {
    cli,
    tabs: Vec::new(),
    active_tab: 0,
    next_tab_id: 1,
    next_snapshot_id: 1,
    pending_focus_sync: false,
    _api_drain: api_drain,
};
this.open_new_tab(window, cx);
this
```

(The existing code creates `this` first and then calls `open_new_tab`. Replace that pattern with the version above which assembles the struct including the task before the first `open_new_tab` call. Make sure to drop the previous `Self { … }` initialiser to avoid double-construction.)

- [ ] **Step 3: Add a stub `apply_api_command` returning 501 for everything**

In `src/tabs.rs`:

```rust
use crate::api_protocol::{ApiCommand, ApiReply, ReplyBody};

impl TerminalTabs {
    fn apply_api_command(&mut self, cmd: ApiCommand, _cx: &mut Context<Self>) {
        let reply = ApiReply::Err { status: 501, error: "not yet implemented".to_string() };
        match cmd {
            ApiCommand::ListTabs { reply: tx }
            | ApiCommand::CreateTab { reply: tx }
            | ApiCommand::CloseTab { reply: tx, .. }
            | ApiCommand::ActivateTab { reply: tx, .. }
            | ApiCommand::GetTab { reply: tx, .. }
            | ApiCommand::GetScreen { reply: tx, .. }
            | ApiCommand::WriteInput { reply: tx, .. }
            | ApiCommand::SendKeys { reply: tx, .. }
            | ApiCommand::SetNote { reply: tx, .. }
            | ApiCommand::ReplaceLine { reply: tx, .. } => {
                let _ = tx.send_blocking(reply);
            }
        }
    }
}
```

- [ ] **Step 4: Build + smoke**

Run: `cd /Users/mono/Repos/rterminal && cargo build`
Expected: PASS.

Manual quick smoke: `cargo run --release` in one terminal, then in another:

```bash
curl -s 127.0.0.1:7878/tabs
# {"error":"not yet implemented"}
```

It should respond with the 501 error, confirming the channel and HTTP server are end-to-end wired.

- [ ] **Step 5: Commit**

```bash
cd /Users/mono/Repos/rterminal
git add src/main.rs src/tabs.rs
git commit -m "feat(api): wire global server into main.rs and TerminalTabs drain"
```

---

## Task 8: Implement `apply_api_command` for all variants

This is the biggest task. Tabs already track `active_tab`, `tabs: Vec<TerminalTab>`, and each `TerminalTab::Terminal` carries an `Entity<AgentTerminal>` that exposes `tab_title()`, the snapshot, etc. We add the missing accessors on `AgentTerminal` first, then implement each match arm.

**Files:**
- Modify: `src/terminal.rs`
- Modify: `src/tabs.rs`

- [ ] **Step 1: Add per-tab accessors on `AgentTerminal`**

Append inside `impl AgentTerminal` in `src/terminal.rs` (near `tab_title`):

```rust
pub(crate) fn runtime_snapshot(&self) -> crate::tab_runtime::TabRuntimeSnapshot {
    self.debug.snapshot()
}

pub(crate) fn screen_text(&self) -> String {
    self.debug.screen_text()
}

pub(crate) fn set_note(&self, note: Option<String>) {
    self.debug.set_note(note);
}

pub(crate) fn write_injected(&mut self, bytes: &[u8]) -> Result<usize, String> {
    let Some(writer) = &self.writer else {
        return Err("pty writer unavailable".to_string());
    };
    crate::pty::write_to_pty(writer, bytes).map_err(|e| format!("write failed: {e:#}"))?;
    self.debug.record_bytes_to_pty(bytes.len(), true);
    Ok(bytes.len())
}

pub(crate) fn replace_line_injected(&mut self, suffix: &[u8]) -> Result<usize, String> {
    let mut payload = Vec::with_capacity(suffix.len() + 1);
    payload.push(0x15);
    payload.extend_from_slice(suffix);
    self.write_injected(&payload)
}

pub(crate) fn grid_dimensions(&self) -> (u16, u16) {
    (self.grid_size.cols, self.grid_size.rows)
}
```

- [ ] **Step 2: Helpers on `TerminalTabs` for selector resolution**

In `src/tabs.rs`, inside `impl TerminalTabs`:

```rust
fn resolve_tab(&self, selector: crate::api_protocol::TabSelector) -> Option<(usize, &TerminalTab)> {
    use crate::api_protocol::TabSelector;
    let index = match selector {
        TabSelector::Active => self.active_tab,
        TabSelector::Id(id) => self.tabs.iter().position(|tab| tab.id as u64 == id)?,
    };
    self.tabs.get(index).map(|t| (index, t))
}
```

Note: `TerminalTab::id` is `usize`; cast it on access. To allow indexing by `u64`, add a getter:

```rust
impl TerminalTab {
    fn api_id(&self) -> u64 {
        self.id as u64
    }
}
```

- [ ] **Step 3: Implement `apply_api_command` fully**

Replace the stub from Task 7 with:

```rust
fn apply_api_command(&mut self, cmd: ApiCommand, cx: &mut Context<Self>) {
    use crate::api_protocol::{ApiCommand, ReplyBody, TabDetailDto, TabSummaryDto, TabSelector};

    match cmd {
        ApiCommand::ListTabs { reply } => {
            let tabs: Vec<TabSummaryDto> = self
                .tabs
                .iter()
                .map(|tab| build_summary(tab, cx))
                .collect();
            let active = self.tabs.get(self.active_tab).map(|t| t.api_id());
            let value = serde_json::json!({ "active": active, "tabs": tabs });
            let _ = reply.send_blocking(ApiReply::Ok {
                status: 200,
                body: ReplyBody::Json(value),
            });
        }

        ApiCommand::CreateTab { reply } => {
            // open_new_tab takes &mut Window; we don't have one here. Defer
            // to the next render via pending_focus_sync — but that doesn't open
            // a tab. Instead, open via a foreground spawn.
            // SIMPLEST: call open_new_tab_headless that doesn't need Window.
            // For v1, we skip window-side focus and only spawn the entity.
            let new_id = self.open_new_tab_headless(cx);
            let value = serde_json::json!({
                "id": new_id,
                "title": "zsh",
                "kind": "terminal",
            });
            let _ = reply.send_blocking(ApiReply::Ok {
                status: 201,
                body: ReplyBody::Json(value),
            });
            cx.notify();
        }

        ApiCommand::CloseTab { id, reply } => {
            let Some((index, tab)) = self.resolve_tab(id) else {
                return reply_err(&reply, 404, "unknown tab");
            };
            let closed_id = tab.api_id();
            self.close_tab_at_index(index, cx);
            self.pending_focus_sync = true;
            cx.notify();
            let _ = reply.send_blocking(ApiReply::Ok {
                status: 200,
                body: ReplyBody::Json(serde_json::json!({ "closed": closed_id })),
            });
        }

        ApiCommand::ActivateTab { id, reply } => {
            let Some((index, tab)) = self.resolve_tab(id) else {
                return reply_err(&reply, 404, "unknown tab");
            };
            let new_id = tab.api_id();
            self.active_tab = index;
            self.pending_focus_sync = true;
            cx.notify();
            let _ = reply.send_blocking(ApiReply::Ok {
                status: 200,
                body: ReplyBody::Json(serde_json::json!({ "active": new_id })),
            });
        }

        ApiCommand::GetTab { id, reply } => {
            let Some((_, tab)) = self.resolve_tab(id) else {
                return reply_err(&reply, 404, "unknown tab");
            };
            let detail = build_detail(tab, cx);
            let value = serde_json::to_value(detail).unwrap_or_else(|_| serde_json::json!({}));
            let _ = reply.send_blocking(ApiReply::Ok {
                status: 200,
                body: ReplyBody::Json(value),
            });
        }

        ApiCommand::GetScreen { id, reply } => {
            let Some((_, tab)) = self.resolve_tab(id) else {
                return reply_err(&reply, 404, "unknown tab");
            };
            let text = match &tab.kind {
                TerminalTabKind::Terminal { terminal, .. } => terminal.read(cx).screen_text(),
                TerminalTabKind::Snapshot { .. } => "<snapshot tab>\n".to_string(),
            };
            let _ = reply.send_blocking(ApiReply::Ok {
                status: 200,
                body: ReplyBody::Text(text),
            });
        }

        ApiCommand::WriteInput { id, bytes, reply } => {
            self.with_terminal_write(id, &reply, |term| term.write_injected(&bytes));
        }

        ApiCommand::SendKeys { id, body, reply } => match crate::api_keys::parse_keys(&body) {
            Ok(bytes) => self.with_terminal_write(id, &reply, |term| term.write_injected(&bytes)),
            Err(err) => reply_err(&reply, 400, &err),
        },

        ApiCommand::SetNote { id, note, reply } => {
            let Some((_, tab)) = self.resolve_tab(id) else {
                return reply_err(&reply, 404, "unknown tab");
            };
            match &tab.kind {
                TerminalTabKind::Terminal { terminal, .. } => {
                    terminal.read(cx).set_note(note.clone());
                    let _ = reply.send_blocking(ApiReply::Ok {
                        status: 200,
                        body: ReplyBody::Json(serde_json::json!({ "note": note })),
                    });
                }
                TerminalTabKind::Snapshot { .. } => {
                    reply_err(&reply, 409, "cannot set note on snapshot tab");
                }
            }
        }

        ApiCommand::ReplaceLine { id, bytes, reply } => {
            self.with_terminal_write(id, &reply, |term| term.replace_line_injected(&bytes));
        }
    }
}

fn with_terminal_write<F>(
    &mut self,
    selector: crate::api_protocol::TabSelector,
    reply: &async_channel::Sender<ApiReply>,
    op: F,
) where
    F: FnOnce(&mut AgentTerminal) -> Result<usize, String>,
{
    let Some((_, tab)) = self.resolve_tab(selector) else {
        return reply_err(reply, 404, "unknown tab");
    };
    let terminal = match &tab.kind {
        TerminalTabKind::Terminal { terminal, .. } => terminal.clone(),
        TerminalTabKind::Snapshot { .. } => return reply_err(reply, 409, "cannot write to snapshot tab"),
    };
    // No &mut self access for the gpui Entity here would be required —
    // use update().
    let result = terminal.update(self_cx_placeholder(), |term, _cx| op(term));
    match result {
        Ok(wrote) => {
            let _ = reply.send_blocking(ApiReply::Ok {
                status: 200,
                body: ReplyBody::Json(serde_json::json!({ "wrote": wrote })),
            });
        }
        Err(err) => reply_err(reply, 503, &err),
    }
}

fn open_new_tab_headless(&mut self, cx: &mut Context<Self>) -> u64 {
    // Mirrors open_new_tab without the focus calls. The user opening the
    // window will focus the new tab on next render via pending_focus_sync.
    let terminal = cx.new(|cx| AgentTerminal::new(cx, self.cli.clone()));
    let tab_id = self.next_tab_id;
    self.next_tab_id += 1;
    let exit_subscription = cx.subscribe(
        &terminal,
        move |this, _terminal, _event: &TerminalExitedEvent, cx| {
            this.close_tab_by_id(tab_id, cx);
        },
    );
    self.tabs.push(TerminalTab {
        id: tab_id,
        kind: TerminalTabKind::Terminal { terminal, _exit_subscription: exit_subscription },
    });
    self.active_tab = self.tabs.len().saturating_sub(1);
    self.pending_focus_sync = true;
    tab_id as u64
}

fn build_summary(tab: &TerminalTab, cx: &mut Context<TerminalTabs>) -> TabSummaryDto {
    let (kind, cols, rows, title) = match &tab.kind {
        TerminalTabKind::Terminal { terminal, .. } => {
            let term = terminal.read(cx);
            let (cols, rows) = term.grid_dimensions();
            ("terminal", cols, rows, term.tab_title())
        }
        TerminalTabKind::Snapshot { snapshot } => {
            ("snapshot", 0, 0, snapshot.read(cx).title())
        }
    };
    TabSummaryDto { id: tab.api_id(), title, kind, cols, rows }
}

fn build_detail(tab: &TerminalTab, cx: &mut Context<TerminalTabs>) -> TabDetailDto {
    match &tab.kind {
        TerminalTabKind::Terminal { terminal, .. } => {
            let term = terminal.read(cx);
            let snap = term.runtime_snapshot();
            let (cols, rows) = term.grid_dimensions();
            TabDetailDto {
                id: tab.api_id(),
                title: term.tab_title(),
                kind: "terminal",
                cols,
                rows,
                cursor_row: snap.cursor_row,
                cursor_col: snap.cursor_col,
                status: snap.status,
                note: snap.note,
                counters: snap.counters,
                uptime_ms: snap.uptime_ms,
                last_error: snap.last_error,
            }
        }
        TerminalTabKind::Snapshot { snapshot } => {
            let snap = snapshot.read(cx);
            TabDetailDto {
                id: tab.api_id(),
                title: snap.title(),
                kind: "snapshot",
                cols: 0,
                rows: 0,
                cursor_row: 0,
                cursor_col: 0,
                status: "snapshot".to_string(),
                note: None,
                counters: Default::default(),
                uptime_ms: 0,
                last_error: None,
            }
        }
    }
}

fn reply_err(reply: &async_channel::Sender<ApiReply>, status: u16, error: &str) {
    let _ = reply.send_blocking(ApiReply::Err { status, error: error.to_string() });
}
```

**Important note:** the `with_terminal_write` helper above uses `self_cx_placeholder()` which doesn't exist. The real fix: `with_terminal_write` needs `cx: &mut Context<Self>`. Change its signature to:

```rust
fn with_terminal_write<F>(
    &mut self,
    selector: crate::api_protocol::TabSelector,
    reply: &async_channel::Sender<ApiReply>,
    cx: &mut Context<Self>,
    op: F,
) where
    F: FnOnce(&mut AgentTerminal) -> Result<usize, String>,
{
    let Some((_, tab)) = self.resolve_tab(selector) else {
        return reply_err(reply, 404, "unknown tab");
    };
    let terminal = match &tab.kind {
        TerminalTabKind::Terminal { terminal, .. } => terminal.clone(),
        TerminalTabKind::Snapshot { .. } => return reply_err(reply, 409, "cannot write to snapshot tab"),
    };
    let result = terminal.update(cx, |term, _cx| op(term));
    match result {
        Ok(wrote) => {
            let _ = reply.send_blocking(ApiReply::Ok {
                status: 200,
                body: ReplyBody::Json(serde_json::json!({ "wrote": wrote })),
            });
        }
        Err(err) => reply_err(reply, 503, &err),
    }
}
```

…and update both call sites in `apply_api_command` to pass `cx`.

**Also:** `AgentTerminal::new(cx, cli)` may not exist — there is `new_embedded(window, cx, cli)` and `new_with_options(window, cx, cli, opts)` which both take `&mut Window`. To create a tab headlessly we need a window. Either:

(a) Cache the most recent `&mut Window` (not feasible cleanly).
(b) Defer `CreateTab` to the next render: queue the create request and apply it inside `render()` where `window` is available.

Use (b). Add to `TerminalTabs`:

```rust
pending_create_requests: Vec<async_channel::Sender<ApiReply>>,
```

Initialise to `Vec::new()` in `new`. In `ApiCommand::CreateTab` arm, do:

```rust
ApiCommand::CreateTab { reply } => {
    self.pending_create_requests.push(reply);
    cx.notify();
}
```

In `render(&mut self, window, cx)`, at the top:

```rust
if !self.pending_create_requests.is_empty() {
    let replies = std::mem::take(&mut self.pending_create_requests);
    for reply in replies {
        let new_id = self.open_new_tab(window, cx) as u64;
        let value = serde_json::json!({ "id": new_id, "title": "zsh", "kind": "terminal" });
        let _ = reply.send_blocking(ApiReply::Ok { status: 201, body: ReplyBody::Json(value) });
    }
}
```

Change `open_new_tab` to return the new `tab_id` (currently returns `()`). Update its existing callers (`on_new_tab`) to ignore the return value.

- [ ] **Step 4: Build + manual smoke**

```bash
cd /Users/mono/Repos/rterminal && cargo build
```

In one terminal: `cargo run --release`. In another:

```bash
curl -s 127.0.0.1:7878/tabs | jq
curl -sX POST 127.0.0.1:7878/tabs | jq
curl -s 127.0.0.1:7878/tabs/active | jq
curl -s 127.0.0.1:7878/tabs/active/screen
curl -sX POST --data 'echo hello' 127.0.0.1:7878/tabs/active/input
curl -sX POST --data 'Enter' 127.0.0.1:7878/tabs/active/keys
curl -sX POST --data 'C-c' 127.0.0.1:7878/tabs/active/keys
```

Verify in the GUI that the new tab opens, the input lands at the prompt, Enter runs it, and Ctrl-C interrupts.

- [ ] **Step 5: Commit**

```bash
cd /Users/mono/Repos/rterminal
git add src/terminal.rs src/tabs.rs
git commit -m "feat(api): implement all ApiCommand handlers in TerminalTabs"
```

---

## Task 9: Remove `debug_server.rs`

After Task 8 the file is unreferenced (its tests were `#[ignore]`'d in Task 6). Delete it.

**Files:**
- Delete: `src/debug_server.rs`
- Modify: `src/main.rs` (remove `mod debug_server;`)

- [ ] **Step 1: Remove the file and module declaration**

```bash
cd /Users/mono/Repos/rterminal
rm src/debug_server.rs
```

Edit `src/main.rs` and delete the `mod debug_server;` line.

- [ ] **Step 2: Confirm clean compile + clean tests**

```bash
cargo build
cargo test
```

Expected: PASS, no `debug_server` references remain anywhere.

If anything still references `crate::debug_server::*`, search and delete:

```bash
grep -rn debug_server src/
```

- [ ] **Step 3: Commit**

```bash
git add -u src/
git commit -m "chore: remove obsolete debug_server module"
```

---

## Task 10: Manual end-to-end smoke + record results

Create a smoke script that exercises every documented endpoint. Not a test — a manual checklist runnable against a live binary.

**Files:**
- Create: `docs/superpowers/specs/2026-05-29-api-smoke.sh`

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# Manual smoke test for the tmux-style HTTP API.
# Usage:
#   1. Launch the binary in another terminal:  cargo run --release
#   2. Run this script:                         bash docs/superpowers/specs/2026-05-29-api-smoke.sh
set -euo pipefail
API=${API:-127.0.0.1:7878}

echo "→ list tabs"
curl -s "$API/tabs" | jq

echo "→ create tab"
NEW=$(curl -sX POST "$API/tabs" | tee /dev/tty | jq -r .id)
echo "   created id=$NEW"

echo "→ get active tab"
curl -s "$API/tabs/active" | jq

echo "→ activate the new tab"
curl -sX POST "$API/tabs/$NEW/activate" | jq

echo "→ capture screen"
curl -s "$API/tabs/$NEW/screen"

echo "→ inject 'echo hi' then Enter"
curl -sX POST --data 'echo hi' "$API/tabs/$NEW/input" | jq
curl -sX POST --data 'Enter' "$API/tabs/$NEW/keys" | jq

echo "→ Ctrl-C"
curl -sX POST --data 'C-c' "$API/tabs/$NEW/keys" | jq

echo "→ legacy /debug/screen alias"
curl -s "$API/debug/screen"

echo "→ close the tab"
curl -sX DELETE "$API/tabs/$NEW" | jq

echo "→ list tabs (should not include $NEW)"
curl -s "$API/tabs" | jq
```

- [ ] **Step 2: Run the smoke script**

```bash
cd /Users/mono/Repos/rterminal
cargo run --release &
sleep 2
bash docs/superpowers/specs/2026-05-29-api-smoke.sh
```

Confirm in the GUI that:
- the new tab appears and is the active tab after activate;
- `echo hi` is printed in that tab after the Enter key;
- Ctrl-C clears the line;
- the tab closes when DELETE is called.

- [ ] **Step 3: Stop the binary and commit the script**

```bash
git add docs/superpowers/specs/2026-05-29-api-smoke.sh
git commit -m "docs: add manual API smoke script"
```

---

## Self-Review Checklist (run before handoff)

- Every endpoint in spec §4 has a routing test (Task 3) and a real handler (Task 8).
- Every key class in spec §5 has a parser test (Task 2).
- Spec §3.2 timeout: not yet wired — the HTTP server uses `recv_blocking()` (Task 4) which blocks indefinitely. Add a 5-second deadline in Task 4 step 1 by switching to `rx.recv_blocking_timeout(Duration::from_secs(5))` if the API exists, otherwise spawn a watcher thread. (`async-channel` 2 does not expose blocking-timeout; an acceptable simplification for v1 is to drop the timeout and document the deferred work.)
- Spec §6 "Active alias with no tabs" — covered by `resolve_tab` returning `None` → 404.
- Spec §6 bind failure exits non-zero — implemented in Task 7 step 1.
- Legacy `/debug/*` aliases — Task 3 routes + Task 8 handlers + Task 10 smoke.
- `replace-line` endpoint kept reachable via legacy `/debug/replace-line` only — Task 3 + Task 8 cover it; no new `/tabs/:id/replace-line` route added.
