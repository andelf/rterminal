# rterminal HTTP API — endpoint reference

Default bind: `127.0.0.1:7878`. Override with `--api-addr host:port` when launching the binary.

All data/control endpoints require the token supplied at launch via `RTERMINAL_WEB_TOKEN` or `--web-token`. Send it on every request as `X-Rterminal-Token: <TOKEN>` or `Authorization: Bearer <TOKEN>`. Static web assets are public; the browser unlock form exchanges the token for an HttpOnly session cookie used by HTTP and WebSocket requests.

## Tab selectors

In every URL where `:id` appears, the selector is either:
- A positive integer (the tab's stable id — never reused after close), or
- The literal string `active` (the currently-focused tab; resolves at request time)

Invalid selector → 400 `{"error":"invalid tab selector: <value>"}`.
Unknown id → 404 `{"error":"unknown tab"}`. Same for `active` when no tabs exist.

## Tab management

### `GET /tabs`

List all tabs and the active id.

**Response 200:**
```json
{
  "active": 3,
  "tabs": [
    {"id": 1, "title": "mono@host:~", "kind": "terminal", "cols": 120, "rows": 30},
    {"id": 3, "title": "/bin/zsh",     "kind": "terminal", "cols": 120, "rows": 30}
  ]
}
```

`active` is `null` when there are no tabs. Closing the last tab leaves an empty native window and keeps the HTTP/Web service running; `POST /tabs` creates a new tab.
`kind` is `"terminal"` for live shells, `"snapshot"` for read-only captured views.

### `POST /tabs`

Create a new terminal tab. Body is currently ignored. The new tab becomes the active tab; the GUI focuses it on the next render frame.

**Response 201:**
```json
{"id": 7, "title": "/bin/zsh", "kind": "terminal"}
```

`title` reflects the actual shell as soon as the PTY spawns. If the request returns before the PTY title is known, the response carries an empty string — usually this is fine because callers fetch `GET /tabs/:id` for fresh detail.

### `DELETE /tabs/:id`

Close a tab. Works for both terminal and snapshot tabs.

**Response 200:**
```json
{"closed": 7}
```

**Error:** 404 if the id is unknown.

If the closed tab was the last one, rterminal keeps an empty window open instead of quitting, so the HTTP and browser services remain available.

### `POST /tabs/:id/activate`

Make the tab the active/focused one. The GUI focuses it on the next render frame.

**Response 200:**
```json
{"active": 7}
```

**Error:** 404 if the id is unknown.

## Observation

### `GET /tabs/:id`

Detailed state of a tab.

**Response 200:**
```json
{
  "id": 3,
  "title": "/bin/zsh",
  "kind": "terminal",
  "cols": 120,
  "rows": 30,
  "cursor_row": 0,
  "cursor_col": 17,
  "status": "connected",
  "note": "history transcript: /Users/mono/.rterminal/history/agent-terminal-...ansi",
  "counters": {
    "bytes_from_pty": 24512,
    "bytes_to_pty": 312,
    "key_events": 47,
    "injected_events": 6,
    "resize_events": 0,
    "http_requests": 12
  },
  "uptime_ms": 84231,
  "last_error": null
}
```

Fields worth knowing:
- `status` — "connected" for live tabs, "snapshot" for snapshot tabs.
- `note` — a free-form string set by code; rterminal currently uses it to record the history transcript path.
- `counters.bytes_from_pty` — cumulative output bytes from the shell. The simplest "is the shell still producing output?" probe.
- `counters.injected_events` — count of API-driven writes (vs. user keystrokes).
- `counters.http_requests` — per-tab API call count.
- `uptime_ms` — wall-clock since the tab opened.
- `last_error` — most recent PTY-level error (resize, write) or null.

Snapshot tabs return placeholder values: `cols=0`, `rows=0`, `cursor_row=0`, `cursor_col=0`, `status="snapshot"`, `counters` all zero.

**Error:** 404 if the id is unknown.

### `GET /tabs/:id/screen`

Current visible screen content as plain text.

**Response 200:** `Content-Type: text/plain; charset=utf-8`. Each row's trailing whitespace is stripped server-side — the grid pads every row to `cols`, but that padding has no semantic content. Rows are joined by `\n` with a final trailing `\n`. Empty terminals return `"<empty screen>\n"`. Snapshot tabs return `"<snapshot tab>\n"`.

Inline whitespace within a row (e.g., spaces between aligned columns of a table) is preserved verbatim. Only run-of-spaces *at the end* of each row gets trimmed.

**Error:** 404 if the id is unknown.

### `GET /tabs/:id/scrollback`

Plain text dump of the tab's grid history plus the current viewport.

**Query params:**
- `lines=N` — return the last N **content** rows (rows that have actual text, not the empty padding the grid keeps below the last drawn line). Default = all retained history. Hard cap = 10000 rows server-side to bound response size.

**Response 200:** `Content-Type: text/plain; charset=utf-8`. Rows from oldest to newest, joined by `\n` with a trailing `\n`. Per-row trailing whitespace is stripped and wide-char spacer cells are skipped, matching `/screen`'s semantics. Empty tabs return `"<empty scrollback>\n"`.

**Errors:**
- 400 if `lines` is non-numeric
- 404 unknown tab id
- 409 if the target is a snapshot tab (snapshots have no scrollback)

Reading scrollback is **read-only** — it does not move the viewport. The shell user still sees the live tail. Use this when you need to ingest output longer than the visible screen without disturbing the human.

## Viewport scroll

### `POST /tabs/:id/scroll`

Move the visible viewport up/down through history.

**Request body:** `application/json`
```json
{"action": "up|down|page_up|page_down|top|bottom", "lines": 5}
```

`lines` is only used by `up` and `down` (default 1); other actions ignore it. The six actions map to `alacritty_terminal::grid::Scroll::{Delta(+N), Delta(-N), PageUp, PageDown, Top, Bottom}`.

**Response 200:**
```json
{"display_offset": 25}
```

`display_offset` is the new viewport position: `0` means the live tail (most recent content), positive integers mean the viewport has been scrolled up by that many rows. Alacritty clamps the offset to the available history range — over-scrolling silently stops at the boundary.

**Errors:**
- 400 if the body isn't valid JSON or the `action` value is unknown
- 404 unknown tab id
- 409 if the target is a snapshot tab

After a scroll, subsequent `GET /tabs/:id/screen` returns the new viewport position; the GUI also reflects the change. Use `/scrollback` instead when you only want to *read* history without moving what the user sees.

## Grid resize

### `POST /tabs/:id/resize`

Resize the terminal grid.

**Request body:** `application/json`
```json
{"cols": 100, "rows": 30, "sync_window": false}
```

`cols` must be 20–300 and `rows` 5–200. `sync_window` is optional and defaults to `false`; when true, the native window is also resized to fit the requested grid. This is the behavior used by the browser client's **Browser size** mode.

**Response 200:**
```json
{"cols": 100, "rows": 30}
```

**Errors:**
- 400 for malformed JSON or out-of-range dimensions
- 404 for an unknown tab
- 409 for a snapshot tab
- 503 if the PTY cannot be resized

## Input

### `POST /tabs/:id/input`

Write raw bytes to the tab's PTY. The request body is forwarded verbatim — `Content-Type` is ignored.

**Response 200:**
```json
{"wrote": 14}
```

**Errors:**
- 404 unknown tab id
- 409 `{"error":"cannot write to snapshot tab"}` if the target is a snapshot tab
- 503 `{"error":"pty writer unavailable"}` if the PTY writer is gone (shell exited)

The bytes hit the PTY immediately and `injected_events` increments by 1. Newlines in the body do *not* cause line submission unless the body actually contains `\r` (most shells want CR, not LF).

### `POST /tabs/:id/keys`

Send tmux-style key tokens. See `key-tokens.md` for the full grammar.

**Request body:** `text/plain` UTF-8.

**Response 200:**
```json
{"wrote": 8}
```

`wrote` is the number of *output* bytes — `Enter` is one byte, `Up` is three, `M-x` is two, etc.

**Errors:**
- 400 parse error — body wasn't valid token syntax. Error message names the bad token.
- 404 / 409 / 503 as for `/input`.

Atomicity: if any token fails to parse, zero bytes are written. Safe to batch long key sequences in one request.

## Legacy aliases

These are kept for compatibility with the original per-tab debug server. All route to the **active tab**. Prefer the `/tabs/:id/...` form for new work.

| Legacy | Equivalent |
|---|---|
| `GET /debug` | Help text listing all endpoints (text/plain) |
| `GET /debug/state` | `GET /tabs/active` (response shape unchanged) |
| `GET /debug/screen` | `GET /tabs/active/screen` |
| `POST /debug/input` | `POST /tabs/active/input` |
| `POST /debug/replace-line` | Active tab only. Prepends a `0x15` (Ctrl-U "kill line") before the body, then writes — useful for "clear-and-type" replacements. Not exposed under `/tabs/:id/replace-line`. |
| `POST /debug/note` | Active tab only. Sets the `note` field. Empty/whitespace body clears the note. |

## Status codes summary

| Code | Where it appears |
|---|---|
| 200 | All successful reads, most successful writes |
| 201 | `POST /tabs` (resource created) |
| 400 | Invalid selector, bad UTF-8 in keys/note body, unknown key token |
| 404 | Unknown tab id, or `active` when no tabs exist |
| 409 | Write attempted on a snapshot tab |
| 503 | PTY writer unavailable (shell exited) |
| 504 | Command channel closed mid-request (rterminal shutting down) |

All error bodies have shape `{"error": "<message>"}` with `Content-Type: application/json; charset=utf-8`.

## What's NOT in v1

The spec acknowledges these gaps; if you hit one, work around it client-side or surface to the user:

- **No rename endpoint** — but the shell can emit `\e]0;new title\a` (OSC 0) which rterminal honours and surfaces via `title`.
- **No agent-facing streaming / SSE endpoint** — poll `/screen` or `counters.bytes_from_pty`. The built-in browser UI uses a separate WebSocket data plane on `127.0.0.1:7879` by default (`--ws-addr`).
- **No per-request timeout on the server side** — clients should use `curl --max-time 5` or equivalent to bound their waits.
- **No auth** — the server binds to loopback by default. Don't use `--api-addr 0.0.0.0:...` on a shared or untrusted network.
- **`POST /tabs` takes no parameters** — can't specify `cwd`, `command`, or environment. Workaround: create the tab, then `POST /input` a `cd …` and the command you want to run.
