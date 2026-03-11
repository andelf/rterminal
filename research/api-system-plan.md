# Agent Terminal API Plan (v1)

## 1. Goal

Design a stable HTTP API layer on top of the existing debug server so external tools can:

- read current terminal screen state (with optional `head` / `tail` / `with_control`)
- read cursor state
- read terminal title
- read process/session metadata
- write text/bytes to terminal
- send key events
- access raw debug state for diagnostics

The plan must preserve current behavior and keep existing `/debug/*` endpoints available.

## 2. Existing Baseline (Today)

Current endpoints:

- `GET /debug`
- `GET /debug/state`
- `GET /debug/screen`
- `POST /debug/input`
- `POST /debug/replace-line`
- `POST /debug/note`

Current strengths:

- local observability counters and status
- direct PTY input injection
- replace-line helper for correction workflows

Current gaps for product API:

- no versioned namespace
- no explicit cursor/title/process endpoints
- no structured request/response schema
- no key-event endpoint
- no explicit control over `head/tail/with_control`

## 3. API Design Principles

- Versioned namespace: `/api/v1/*`
- Backward compatibility: keep `/debug/*` unchanged
- Read/write separation:
  - read endpoints are idempotent `GET`
  - write endpoints are explicit `POST`
- Single JSON envelope for v1:
  - success: `{ "ok": true, "data": ..., "meta": ... }`
  - error: `{ "ok": false, "error": { "code": "...", "message": "...", "details": ... } }`
- Safe defaults:
  - loopback bind by default
  - bounded payload size
  - bounded `head/tail` parameters

## 4. Proposed Endpoints (v1)

## 4.1 Read APIs

### `GET /api/v1/screen`

Query params:

- `tail` (optional, integer >= 1): return last N lines
- `head` (optional, integer >= 1): return first N lines
- `with_control` (optional, bool, default `false`): include control-aware representation

Rules:

- `head` and `tail` are mutually exclusive (if both provided -> `400`)
- if neither is provided: return full visible snapshot lines
- `tail/head` have max bound (suggested: 2000)

Response `data`:

- `lines`: string array (human-readable, trimmed as today)
- `cursor`: `{ row, col }`
- `grid`: `{ cols, rows }`
- `alt_screen`: bool
- `with_control`: bool
- `control_lines` (optional): present only when `with_control=true`

Notes:

- Phase 1 can implement `control_lines` as escaped control chars from snapshot text.
- Phase 2 can upgrade to true PTY-byte-backed representation (see Section 8).

### `GET /api/v1/cursor`

Response `data`:

- `row`, `col`
- `visible`
- `alt_screen`
- `input_cursor_utf16`

### `GET /api/v1/title`

Response `data`:

- `terminal_title` (from terminal events)
- `window_title` (effective title shown)
- `shell`

### `GET /api/v1/process`

Response `data`:

- `shell`
- `pid` (optional/null if unavailable in backend)
- `running` (bool)
- `exit_code` (optional/null)
- `started_at_ms`
- `uptime_ms`

### `GET /api/v1/state`

Aggregated state endpoint for clients that prefer one request.

Query params:

- `include_screen` (bool, default `true`)
- `include_debug_raw` (bool, default `false`)
- `with_control` (bool, default `false`, forwarded to screen serializer)
- `tail` / `head` (same rules as `/screen`)

Response `data`:

- `screen` (optional)
- `cursor`
- `title`
- `process`
- `debug` (existing counters + error + note + listening addr)
- `debug_raw` (optional; see Section 8)

## 4.2 Write APIs

### `POST /api/v1/input/write`

Body schema:

- `text` (string, optional)
- `bytes_base64` (string, optional)
- `encoding` (optional, default `utf8` when `text` is set)

Rules:

- exactly one of `text` or `bytes_base64` must be provided

Response `data`:

- `bytes_written`
- `injected` = true

### `POST /api/v1/input/key`

Body schema:

- `key` (string): examples `enter`, `backspace`, `a`, `left`
- `modifiers` (object, optional): `control`, `alt`, `shift`, `platform`, `function`
- `key_char` (string, optional)
- `repeat` (int, optional, default `1`, max bounded)

Behavior:

- map to existing keystroke encoding path
- apply same input-model update path used by UI keydown handling
- write encoded bytes to PTY

Response `data`:

- `bytes_written`
- `encoded_bytes_hex`
- `repeat`

### `POST /api/v1/input/replace-line`

Body schema:

- `text` (string)

Behavior:

- same as existing `/debug/replace-line` (`Ctrl+U` + new content)

Response `data`:

- `bytes_written`
- `mode`: `replace_line`

## 4.3 Compatibility APIs

Keep current `/debug/*` endpoints as-is.

Add:

- `GET /api/v1/debug/state` as structured wrapper of current debug snapshot
- `GET /api/v1/debug/screen` as structured wrapper of current `/debug/screen`

This allows gradual client migration without breaking existing tools.

## 5. Response Envelope

Success:

```json
{
  "ok": true,
  "data": {},
  "meta": {
    "api_version": "v1",
    "ts_ms": 1760000000000
  }
}
```

Error:

```json
{
  "ok": false,
  "error": {
    "code": "INVALID_ARGUMENT",
    "message": "head and tail cannot be used together",
    "details": {
      "head": 10,
      "tail": 10
    }
  },
  "meta": {
    "api_version": "v1",
    "ts_ms": 1760000000000
  }
}
```

## 6. Validation Rules

- `head`, `tail`: integer, `1..2000`
- `repeat`: integer, `1..100`
- request body max size for input endpoints (suggested `256 KiB`)
- unsupported key/modifier combos return `422`
- all write endpoints return `503` when PTY writer unavailable

## 7. Security and Runtime Controls

- default bind: loopback only (`127.0.0.1`)
- optional token gate for write APIs:
  - env: `AGENT_TUI_API_TOKEN`
  - request header: `Authorization: Bearer <token>`
- read-only mode option (future):
  - env: `AGENT_TUI_API_READ_ONLY=1` disables all write APIs

## 8. Raw Debug Content Strategy

Requirement: expose status with raw diagnostics.

Plan:

- keep current structured debug state as baseline
- add an in-memory PTY raw ring buffer:
  - store last N bytes from PTY output (suggested default: `1 MiB`)
  - expose via `/api/v1/state?include_debug_raw=true`
- `debug_raw` object:
  - `pty_output_tail_base64`
  - `pty_output_tail_text_lossy`
  - `raw_truncated` bool
  - `buffer_size`

This provides practical raw diagnostics without unbounded memory growth.

## 9. Implementation Phases

### Phase 1: API Skeleton + Read Endpoints

- add `/api/v1/*` router
- implement `/screen`, `/cursor`, `/title`, `/process`, `/state`
- shared JSON response helper
- `head/tail/with_control` parameter parser

### Phase 2: Write Endpoints

- implement `/input/write`, `/input/key`, `/input/replace-line`
- reuse existing key encoding and model update path
- enforce payload/argument validation

### Phase 3: Raw Debug Extension

- add bounded PTY raw ring buffer
- expose `include_debug_raw` in `/state`
- add truncation metadata

### Phase 4: Hardening

- optional token auth for write APIs
- docs + examples + compatibility notes
- final regression and load sanity checks

## 10. Test Plan

Unit tests:

- query parsing (`head/tail` mutual exclusion, bounds)
- key request -> encoded bytes mapping
- response envelope and error schema

Integration tests (same style as current tiny_http tests):

- read endpoints return expected fields
- write endpoints mutate counters and inject bytes
- replace-line behavior preserved
- include-debug-raw behavior and truncation flags

Manual validation:

- voice correction flow: read current line -> replace-line -> verify state
- Chinese input + deletion controls + API state consistency

## 11. Open Decisions

- Should `/api/v1/screen` default to full visible lines or default `tail=200`?
- Should `with_control=true` return escaped text only in v1, or require raw PTY reconstruction immediately?
- Should write APIs be token-protected by default or opt-in?

## 12. Suggested First Delivery Scope

Deliver first:

- `/api/v1/state`
- `/api/v1/screen` (with `head/tail`, `with_control=false` first)
- `/api/v1/cursor`
- `/api/v1/title`
- `/api/v1/input/write`
- `/api/v1/input/key`

Then add:

- `/api/v1/process` PID/exit details
- raw ring buffer diagnostics
- token auth

