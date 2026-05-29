---
name: driving-rterminal
description: Use this skill whenever you need to drive a running rterminal instance from outside — listing or creating tabs, switching focus, sending commands or keystrokes to a tab, capturing screen content, or watching for output. Trigger this whenever a task involves controlling rterminal programmatically, automating shell sessions across multiple tabs, building agent workflows that need a real PTY, or any time the user mentions rterminal, "agent terminal", or its HTTP API.
---

# Driving rterminal via the HTTP API

rterminal exposes a tmux-style HTTP control API at `127.0.0.1:7878` by default. Every running tab is addressable by a stable numeric id; the special alias `active` resolves to whichever tab currently has focus. This skill is how you list, create, close, and switch tabs, how you inject keystrokes or raw bytes, and how you read what's currently on screen.

Use this skill whenever the work involves:
- Operating rterminal from a script or agent
- Spawning a fresh shell session and driving it (cd, run a command, capture output)
- Sending a Ctrl-C, Enter, arrow keys, or other special keys
- Reading a tab's current screen content to decide what to do next
- Coordinating multiple shell sessions in parallel through tabs

## Discovering the API endpoint

The default address is `127.0.0.1:7878`. If rterminal was launched with `--api-addr`, use that. When unsure, ask the user or default to the documented port. The server only binds to loopback — no auth needed, but also no remote access.

Quick liveness check:
```bash
curl -s -o /dev/null -w "%{http_code}\n" 127.0.0.1:7878/tabs
# 200 → ready. Connection refused → rterminal isn't running.
```

## Endpoint quick reference

All responses are JSON unless noted. Errors return `{"error":"..."}` with the appropriate status code.

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/tabs` | List all tabs + the active id |
| `POST` | `/tabs` | Create a new terminal tab (201) |
| `DELETE` | `/tabs/:id` | Close a tab |
| `POST` | `/tabs/:id/activate` | Make a tab the active/focused one |
| `GET` | `/tabs/:id` | Tab detail: title, cursor, status, counters, uptime |
| `GET` | `/tabs/:id/screen` | Current visible screen as plain text |
| `POST` | `/tabs/:id/input` | Write raw bytes to PTY (any content-type) |
| `POST` | `/tabs/:id/keys` | tmux-style key tokens (`Enter`, `C-c`, `Up`, `"text"`) |

`:id` is a positive integer or the literal string `active`. Tab ids never get reused after close — id 3 closed today won't come back as id 3 tomorrow.

Legacy `/debug/state`, `/debug/screen`, `/debug/input`, `/debug/replace-line`, `/debug/note` all route to the **active tab**. Prefer `/tabs/:id/...` for new code.

For the full endpoint catalogue, body schemas, and error codes, see `references/endpoints.md`.

## tmux-style key grammar (`POST /tabs/:id/keys`)

Body is plain text. Whitespace separates tokens. Examples:

| Body | Bytes sent to PTY |
|---|---|
| `Enter` | `\r` |
| `C-c` | `\x03` |
| `C-a "echo hi" Enter` | `\x01echo hi\r` |
| `Up Up Enter` | runs the previous-previous command |
| `M-b` | Alt-B (jump word backward) |

Named keys: `Enter`, `Tab`, `Escape`, `Space`, `BSpace`, `Up`/`Down`/`Left`/`Right`, `Home`, `End`, `PageUp`, `PageDown`, `F1`-`F12`.
Modifiers: `C-` (Ctrl), `M-` (Meta/Alt — emits ESC prefix), `S-` (Shift, only meaningful in front of `Tab`).
Literal text: wrap in double quotes — `"…"` — preserves spaces and special chars. Inside the quotes only `\"` and `\\` are escaped.

For the complete grammar (including F-key byte sequences, edge cases, and atomic-validation guarantees), see `references/key-tokens.md`.

## Common workflows

### Run a command and capture its output

```bash
# Inject the command, then Enter
curl -sX POST --data 'ls -la /tmp | head -5' http://127.0.0.1:7878/tabs/active/input
curl -sX POST --data 'Enter' http://127.0.0.1:7878/tabs/active/keys

# Wait briefly for output, then capture
sleep 0.5
curl -s http://127.0.0.1:7878/tabs/active/screen
```

For long-running commands, poll `/screen` every 200-500ms until output stabilises or a known prompt pattern reappears. The API itself doesn't block on output.

### Run a command in a dedicated new tab (no contamination)

```bash
NEW=$(curl -sX POST http://127.0.0.1:7878/tabs | jq -r .id)
curl -sX POST --data "cd /Users/mono/Repos/rterminal" http://127.0.0.1:7878/tabs/$NEW/input
curl -sX POST --data "Enter" http://127.0.0.1:7878/tabs/$NEW/keys
curl -sX POST --data "cargo test" http://127.0.0.1:7878/tabs/$NEW/input
curl -sX POST --data "Enter" http://127.0.0.1:7878/tabs/$NEW/keys
# ... later, when you're done with this tab:
curl -sX DELETE http://127.0.0.1:7878/tabs/$NEW
```

This pattern is the workhorse — isolate side-effects to a tab you own, then close it.

### Interrupt a stuck command

```bash
curl -sX POST --data 'C-c' http://127.0.0.1:7878/tabs/active/keys
```

For nuclear interrupt (kill the foreground job + clear input line):
```bash
curl -sX POST --data 'C-c C-u' http://127.0.0.1:7878/tabs/active/keys
```

### Replay a history entry

```bash
# Up arrow N times, then Enter
curl -sX POST --data 'Up Up Up Enter' http://127.0.0.1:7878/tabs/active/keys
```

### Drive any tab regardless of GUI focus

Writes through `/tabs/:id/input` and `/tabs/:id/keys` go straight to the targeted tab's PTY — they do **not** require that tab to be the active/focused one. The GUI focus only routes the user's keyboard. This means an agent can drive several tabs in parallel without ever calling `/activate`:

```bash
# tab 5 keeps running cargo test in the "background";
# meanwhile the user is interactively typing in some other tab
curl -sX POST --data 'cargo test --release' http://127.0.0.1:7878/tabs/5/input
curl -sX POST --data 'Enter' http://127.0.0.1:7878/tabs/5/keys
# later, when you want output:
curl -s http://127.0.0.1:7878/tabs/5/screen
```

Reach for `/activate` only when the *user* needs to see the tab — never as a prerequisite for writing to it.

### Drive a TUI program (vim, less, htop, lazygit, …)

TUI programs read directly from the PTY, just like a shell. The same `/input` and `/keys` work:

```bash
# Open vim on a file in tab 3
curl -sX POST --data 'vim /tmp/notes.md' http://127.0.0.1:7878/tabs/3/input
curl -sX POST --data 'Enter' http://127.0.0.1:7878/tabs/3/keys
sleep 1  # let vim paint

# Enter insert mode, type content, leave insert mode, save+quit
curl -sX POST --data 'i' http://127.0.0.1:7878/tabs/3/keys
curl -sX POST --data 'first line of text' http://127.0.0.1:7878/tabs/3/input
curl -sX POST --data 'Enter' http://127.0.0.1:7878/tabs/3/keys
curl -sX POST --data 'second line' http://127.0.0.1:7878/tabs/3/input
curl -sX POST --data 'Escape' http://127.0.0.1:7878/tabs/3/keys
curl -sX POST --data ':wq' http://127.0.0.1:7878/tabs/3/input
curl -sX POST --data 'Enter' http://127.0.0.1:7878/tabs/3/keys
```

Use `GET /screen` between steps to verify the program's state — vim shows `-- INSERT --` on the status line, `less` shows a `:` prompt at the bottom, htop has its header. Greppable enough to confirm "we're in the expected mode" before sending the next keystroke.

### Set a custom tab title

The tab `title` field updates whenever the PTY emits an OSC 0 escape (`ESC ] 0 ; TITLE BEL`). Send the sequence as output from the shell:

```bash
# Sets the title to "agent:test-run-1" for tab 3
curl -sX POST --data $'printf \'\\e]0;agent:test-run-1\\a\'' \
  http://127.0.0.1:7878/tabs/3/input
curl -sX POST --data 'Enter' http://127.0.0.1:7878/tabs/3/keys
```

**Pitfall — oh-my-zsh overrides titles on every prompt redraw.** OMZ's `precmd` hook resets the title to `user@host:cwd` after every command. Disable it first if you want your title to stick:

```bash
curl -sX POST --data 'DISABLE_AUTO_TITLE=true' http://127.0.0.1:7878/tabs/3/input
curl -sX POST --data 'Enter' http://127.0.0.1:7878/tabs/3/keys
# now subsequent OSC 0 writes are persistent
```

This is useful for marking agent-owned tabs so the human can see at a glance which tabs the agent is using.

### List + introspect tabs

```bash
curl -s http://127.0.0.1:7878/tabs | jq
# {
#   "active": 3,
#   "tabs": [
#     {"id":1,"title":"mono@host:~","kind":"terminal","cols":120,"rows":30},
#     {"id":3,"title":"/bin/zsh","kind":"terminal","cols":120,"rows":30}
#   ]
# }

curl -s http://127.0.0.1:7878/tabs/3 | jq
# title, status, note, counters (bytes_to/from_pty, http_requests), uptime_ms, last_error
```

`counters.bytes_from_pty` is a cheap progress indicator — if it stops increasing, the shell has likely finished writing output for the moment.

## Workflow patterns

### Wait-for-quiet (output stabilises)

There is no built-in `wait` endpoint. The reliable pattern is to poll `bytes_from_pty` until it stops increasing for a few iterations:

```bash
prev=-1; stable=0
while [ $stable -lt 3 ]; do
  curr=$(curl -s http://127.0.0.1:7878/tabs/active | jq .counters.bytes_from_pty)
  [ "$curr" = "$prev" ] && stable=$((stable+1)) || stable=0
  prev=$curr
  sleep 0.2
done
curl -s http://127.0.0.1:7878/tabs/active/screen
```

Use this when you've issued a command and need to grab its full output before proceeding.

### Wait-for-prompt (text appears)

When you know the prompt string the shell shows when idle (e.g. `$` or `➜`), poll `/screen` and grep for it:

```bash
while ! curl -s http://127.0.0.1:7878/tabs/active/screen | tail -1 | grep -q '\$ $'; do
  sleep 0.2
done
```

Adjust the regex to whatever your user's prompt actually looks like — check it first with one `GET /screen` call. Some prompts include ANSI escapes, color codes, or git branch names; `tail -1` + `grep -q` on a simple suffix is usually robust enough.

### Idempotent setup

If you might be re-running a script against the same tab, prefix input with a `C-c C-u` to clear any leftover state before writing:

```bash
curl -sX POST --data 'C-c C-u' http://127.0.0.1:7878/tabs/active/keys
curl -sX POST --data 'your command' http://127.0.0.1:7878/tabs/active/input
curl -sX POST --data 'Enter' http://127.0.0.1:7878/tabs/active/keys
```

## Choosing input vs keys

Both endpoints write to the PTY. The difference:
- `POST /input` is **raw bytes**. Whatever you send goes into the PTY verbatim. Use for literal text content — including text with quotes or spaces. **No interpretation.**
- `POST /keys` is **tokenised**. It understands `Enter`, `C-c`, `Up`, etc. and produces the matching byte sequences. Use for keystrokes and special keys.

You can mix them — typical pattern is `/input` for the command body, `/keys` for the terminating `Enter`:

```bash
curl -sX POST --data 'echo "hello world"' http://127.0.0.1:7878/tabs/active/input
curl -sX POST --data 'Enter' http://127.0.0.1:7878/tabs/active/keys
```

Or do it all through `/keys` if the text has no special chars:

```bash
curl -sX POST --data '"echo hi" Enter' http://127.0.0.1:7878/tabs/active/keys
```

### Unicode / UTF-8

Both endpoints are UTF-8 transparent — any well-formed UTF-8 (CJK characters, emoji, BMP-extension code points like 𗀀) passes through verbatim. The `/keys` parser only interprets `\"` and `\\` inside quoted literals; everything else is byte-for-byte.

```bash
# All four of these work and produce the same bytes in the file:
curl -sX POST --data '你好，世界！🚀'                  $API/tabs/3/input
curl -sX POST --data '"你好，世界！🚀" Enter'         $API/tabs/3/keys
curl -sX POST --data 'Hello 你好 World 世界 1234 ５６' $API/tabs/3/input
```

**Display caveat — CJK characters render as double-width.** A character that's 3 UTF-8 bytes on disk takes 2 *columns* on screen. When you `GET /screen`, what looks like spaces between Chinese characters is rterminal's visual padding for the second column — the actual file content has no spaces there. Don't use the screen output as the source of truth for what was written; read the file (or use a `cat`-and-capture pattern) for byte-exact verification.

## Error handling

| Status | Meaning | What to do |
|---|---|---|
| 200 | OK | Continue. |
| 201 | Created (POST /tabs) | Read `id` from body. |
| 400 | Bad request (malformed selector, invalid key token) | Read `error` field — usually a typo in a key name or a non-numeric id. |
| 404 | Unknown tab id (or no active tab when called with `active`) | The id has been closed or no tab exists. Re-list with `GET /tabs`. |
| 409 | Can't write to a snapshot tab | Snapshot tabs are read-only views. Switch target. |
| 503 / 504 | PTY writer unavailable / command channel closed | rterminal is shutting down or unhealthy. |

The server has **no per-request timeout** today — clients should add their own via `curl --max-time 5`.

## Limits to be aware of

- **Screen only shows the visible viewport.** Anything that scrolled off is gone — no scrollback endpoint in v1. For long outputs, capture progressively or redirect to a file inside the shell.
- **No resize endpoint.** The tab's grid follows the GUI window — adjust the window size if you need more rows/cols.
- **Snapshot tabs are read-only.** Writes return 409. You can identify them via `kind: "snapshot"` in `GET /tabs`.
- **No streaming.** All reads are pull-based. Implement waits client-side.
- **`active` is mutable.** Between calls the user may have switched tabs in the GUI. If you need a stable target, pin to a numeric id from the start.

## When NOT to use this skill

- If the user wants to execute a shell command for their own purposes and you have a regular `Bash` tool available, use that — it's simpler and gives you direct output. This skill is for driving an *interactive* terminal that lives in the user's GUI, often to demonstrate something visually or to operate alongside their own work in the same window.
- If rterminal isn't running. A `curl: (7) Failed to connect` from `127.0.0.1:7878` means there's no server — surface that to the user rather than trying to start one yourself.

## Further reading

- `references/endpoints.md` — Complete endpoint catalogue with request/response schemas, all status codes, and snapshot-tab semantics.
- `references/key-tokens.md` — Full tmux-style key grammar: every named key, all modifier combinations, F-key byte sequences, escape rules inside quoted literals, error semantics.
