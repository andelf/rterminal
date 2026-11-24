# Agent Terminal

A GPU-accelerated terminal emulator built with [GPUI](https://github.com/zed-industries/zed) (Zed's UI framework) and [alacritty_terminal](https://github.com/alacritty/alacritty), designed as a standalone native desktop terminal (macOS and Windows) with first-class accessibility and input method support.

## Background

This project originated from a specific need: building a terminal emulator that treats **accessibility-driven input** as a first-class concern, rather than an afterthought. Traditional terminal emulators expose minimal accessibility semantics — most only forward raw key events to a PTY, leaving assistive technologies (Voice Control, screen readers, accessibility automation tools) unable to read or modify the current command line.

Agent Terminal takes a different approach:

- It maintains a **shadow input-line model** that mirrors what the user is typing in the shell
- It exposes this model to macOS Accessibility APIs as an `AXTextField`, allowing external tools to **read the current input**, **know the cursor position**, and **inject or replace text**
- It bridges bidirectionally between the native accessibility tree and the internal input state on every render frame
- It also exposes a **tmux-style HTTP control API** (single global server, all tabs addressable by stable id) so agents that prefer scripting over the AX bridge can list/create/close tabs, send raw input or named keystrokes (`Enter`, `C-c`, `Up`, `"text"`), capture screen and scrollback, and move the viewport — without needing accessibility permissions

The architecture draws from research into how Zed and Ghostty implement their terminal layers (documented in `research/terminal-implementation-research.md`), adopting the pattern of:

1. Reusing `alacritty_terminal` as the VT/ANSI state machine
2. Deriving a renderer-oriented `ScreenSnapshot` from terminal state
3. Painting through GPUI's canvas as a custom drawing surface

This is **not** intended to be a general-purpose terminal replacement. It is an exploration of what a terminal looks like when designed around agent-assisted and accessibility-first workflows.

## Architecture

```
┌─────────────────────────────────────────────────┐
│                   GPUI Window                    │
│  ┌─────────────────────────────────────────────┐│
│  │  TerminalTabs (tab bar + tab management)    ││
│  ├─────────────────────────────────────────────┤│
│  │  AgentTerminal (per-tab terminal instance)  ││
│  │  ┌─────────────┐  ┌──────────────────────┐ ││
│  │  │ PTY Session  │  │  alacritty_terminal  │ ││
│  │  │ (shell I/O)  │◄►│  Term<Listener>      │ ││
│  │  └─────────────┘  │  Processor            │ ││
│  │                    └──────────┬───────────┘ ││
│  │                               ▼             ││
│  │                    ┌──────────────────────┐ ││
│  │                    │   ScreenSnapshot     │ ││
│  │                    │   cells, cursor,     │ ││
│  │                    │   alt_screen, ...    │ ││
│  │                    └──────────┬───────────┘ ││
│  │                               ▼             ││
│  │  ┌─────────────┐  ┌──────────────────────┐ ││
│  │  │ input_line   │  │  GPUI canvas(...)    │ ││
│  │  │ (shadow      │  │  per-cell text       │ ││
│  │  │  model)      │  │  shaping + paint     │ ││
│  │  └──────┬───────┘  └──────────────────────┘ ││
│  │         ▼                                   ││
│  │  ┌──────────────────────┐                   ││
│  │  │  macOS AX Bridge     │                   ││
│  │  │  AXTextField on      │◄► VoiceControl /  ││
│  │  │  NSView              │   axcli / etc.    ││
│  │  └──────────────────────┘                   ││
│  └─────────────────────────────────────────────┘│
└─────────────────────────────────────────────────┘
```

### Key Components

| File | Lines | Responsibility |
|------|------:|----------------|
| `input.rs` | ~1730 | Keyboard/mouse/IME/paste handling, input-line shadow model, AX override logic, selection |
| `terminal.rs` | ~1620 | Core terminal state: PTY lifecycle, `Term` wiring, snapshot generation, cursor animation, scrollback dump |
| `tabs.rs` | ~810 | Multi-tab management, drain task for the HTTP control API, tab bar rendering, Cmd+N shortcuts |
| `api_server.rs` | ~580 | HTTP listener (tiny_http) + request routing + response rendering for the tmux-style control API |
| `snapshot_tab.rs` | ~560 | Read-only snapshot tabs with scrollback, selection, copy |
| `render.rs` | ~550 | GPUI `Render` impl, per-cell canvas painting, cursor drawing, AX sync entry point |
| `api_keys.rs` | ~300 | tmux-style key token parser (`Enter`, `C-c`, `"text"`, modifiers, F-keys) |
| `keyboard.rs` | ~300 | Keystroke-to-terminal-byte encoding (special keys, Ctrl chords, Alt, modifiers) |
| `cli.rs` | ~240 | CLI argument parsing via `clap` (including `--api-addr`) |
| `text_utils.rs` | ~180 | UTF-16 ↔ byte index conversion, word deletion, AX override heuristics |
| `color.rs` | ~150 | ANSI → HSLA color mapping (named, indexed 256, dim/bright, spec RGB) |
| `tab_runtime.rs` | ~145 | Per-tab counters, status, screen snapshot mirror shared with the HTTP API |
| `web_ui.rs` | ~60 | Embeds and serves the vendored xterm.js browser client from the binary |
| `web_ansi.rs` | ~120 | Encodes `ScreenSnapshot` as an ANSI repaint (colour + cursor + alt-screen) for the web client |
| `macos_ax.rs` | ~140 | Native Objective-C bridge: `setAccessibilityValue` / `setAccessibilitySelectedTextRange` |
| `main.rs` | ~135 | Process entry: CLI parsing, API server bootstrap, GPUI app launch |
| `history_log.rs` | ~135 | Per-tab raw PTY transcripts and metadata sidecars under `~/.rterminal/history` |
| `api_protocol.rs` | ~110 | Wire types shared by HTTP layer and command handlers (`ApiCommand`, DTOs, `ScrollAction`) |
| `pty.rs` | ~85 | PTY creation via `portable-pty`, background reader thread |
| `input_log.rs` | ~85 | Structured JSONL input event logger for debugging |

## Features

### Terminal Emulation
- Full VT/ANSI terminal emulation via `alacritty_terminal`
- ANSI color support: named, 256-color indexed palette, 24-bit true color
- Wide character rendering (CJK) with configurable ambiguous-width handling
- Cursor shapes: block, beam, underline, hidden (respects application cursor mode)
- Smooth cursor slide animation with optional trailing effect
- Alt screen buffer support (vim, less, htop, etc.)
- Scrollback history is available in normal terminal mode through mouse wheel scrolling
- Mouse reporting (click, motion, drag, scroll wheel) for terminal applications
- Bracketed paste mode
- Focus in/out events (`CSI I` / `CSI O`)
- Terminal title tracking via OSC sequences

### Input & Accessibility
- Full keyboard input: printable text, Ctrl/Alt/Shift chords, function keys, special keys
- macOS IME integration via `NSTextInputClient` (Chinese/Japanese/Korean input)
- Input-line shadow model synchronized to macOS Accessibility tree as `AXTextField`
- Bidirectional AX bridge: external tools can read and modify the current command line
- AX override guard window (250ms) to avoid conflict between local typing and external edits
- Paste support with `Cmd+V` / `Ctrl+Shift+V`
- Large paste guard: confirmation dialog for multi-line or high non-ASCII content
- `\n` → `\r` conversion in paste for correct behavior in tmux/vi

### Multi-Tab
- `Cmd+T` to open new tabs, `Cmd+W` to close
- `Ctrl+Tab` / `Cmd+Shift+]` / `Cmd+Shift+[` for tab navigation
- `Cmd+1` through `Cmd+0` for direct tab switching
- Snapshot tabs: `Cmd+Shift+S` captures a read-only, scrollable copy of the current terminal

### Appearance
- Custom transparent title bar with native traffic light controls
- Two themes: Default (dark) and Eye Care (green-tinted dark)
- Configurable font family and fallback fonts
- Font zoom with `Cmd+` / `Cmd-`
- Configurable double-width character overrides
- Option key behavior: Meta/Alt (default) or native macOS character input (`--no-option-as-meta`)

### HTTP Control API
A single global HTTP server (default `127.0.0.1:7878`, override with `--api-addr`) lets external agents drive any tab. Stable numeric tab ids, plus the alias `active` for the currently-focused tab. No auth — loopback only.

- **Tab management**: `GET /tabs` (list + active), `POST /tabs` (create, returns id+title), `DELETE /tabs/:id`, `POST /tabs/:id/activate`
- **Observation**: `GET /tabs/:id` (DTO with title, kind, cols/rows, cursor, status, counters, note, uptime), `GET /tabs/:id/screen` (current viewport as plain text), `GET /tabs/:id/screen.ansi` (full ANSI repaint with colours/cursor for the web client), `GET /tabs/:id/scrollback?lines=N` (history + viewport, trailing padding stripped)
- **Input**: `POST /tabs/:id/input` (raw bytes), `POST /tabs/:id/keys` (tmux-style tokens: `Enter`, `C-c`, `M-x`, `Up`, `F5`, `"literal text"`, ...)
- **Viewport scroll**: `POST /tabs/:id/scroll` with JSON `{"action":"up|down|page_up|page_down|top|bottom","lines":N}`
- **Size**: `POST /tabs/:id/resize` with JSON `{"cols":100,"rows":30,"sync_window":true}`. This always resizes the native PTY grid; `sync_window:true` additionally resizes the native window to fit that grid ("浏览器尺寸" mode), so neither side scrolls.
- **Legacy aliases**: `/debug/state`, `/debug/screen`, `/debug/input`, `/debug/replace-line`, `/debug/note` all route to the active tab

A consumer-side skill lives in `.claude/skills/driving-rterminal/` with the full grammar, error semantics, and common-workflow recipes.

### Browser Client (`xterm.js`)

Open **http://127.0.0.1:7878/web/** while the app is running. The page is served by the same tiny_http server and has no external assets — xterm.js 5.5.0 (MIT, see `web/vendor/xterm-LICENSE`) is vendored.

For development, debug builds read these assets from `web/` on every request, so editing JS/CSS only needs a browser refresh instead of a full rebuild. Set `RTERMINAL_WEB_DIR` to override the location; otherwise the app searches `web/` next to the executable, its parents, and finally the current directory. Release builds still embed the assets into the binary, so a single exe works from any cwd.

- The tab strip mirrors the native window's tab list (stable ids, titles, active tab). `+` / `×` / click create, close and activate real tabs.
- Each active tab polls `GET /tabs/:id/screen.ansi`, which rewrites every cell with its original ANSI foreground/background and positions the cursor. Alt-screen and cursor visibility are preserved.
- Keyboard input goes to `POST /tabs/:id/input` on the same PTY the native window drives, so both views stay in sync.
- A tiny embedded font supplies the four Powerline separators (`U+E0B0/E0B2/E0B4/E0B6`) used by `oh-my-posh`, so prompts render as solid triangles instead of boxes even on phones/clients without a Nerd Font installed.
- On touch devices one-finger panning is kept (it is how you reach the wider/taller PC grid), while pinch/double-tap zoom and page overscroll are disabled. The terminal is a fixed character grid, so zoom would desynchronise cells, selection and the soft-keyboard detection. If a browser still zooms, the script ignores the zoomed `visualViewport` instead of treating it as a keyboard.
- **The document itself cannot scroll; only `.pane` can.** `html` is `position: fixed; inset: 0; overflow: clip`, so even a programmatic or gesture scroll has no scrollport to move, and the app's height is `min(visualViewport.height, layout viewport height)`. iPad Safari reports a visual viewport *larger* than the layout viewport once its browser chrome collapses; sizing the app from that number overflowed the document, and the resulting page scroll did nothing but carry the top bar off screen. The clamped height still shrinks above the soft keyboard, and a `scrollTo(0,0)` guard covers engines where `overflow: clip` is missing.
- The view follows the live cursor on both axes after each keystroke. Once you pan/drag away it stops following until the next input, so history stays where you put it; programmatic repaints do not count as user scrolling.
- IME input is gated by real DOM flags, not string guessing: `compositionstart/update/end`, `InputEvent.isComposing` and `inputType=insertCompositionText` on xterm's hidden textarea. Pre-edit text (half-formed pinyin/CJK) is never sent to the PTY, and the full-screen ANSI repaint is deferred while a composition is active so it cannot cut off the composition view; the newest snapshot is painted once on `compositionend`. iOS often fires no composition events and reports `isComposing=false`, so the flags are tracked locally as a second gate, and tab switches clear a stale gate.
- Text input follows one invariant: **the hidden textarea's `value` is the document, and the PTY is kept as an exact projection of it.** Every route that can carry text — `onData`, the raw `input` event, `compositionend` — calls the same idempotent reconciler with the same value, so listener order cannot change the outcome and no route can double-send.
  - `data` is not trustworthy. A captured iPad session contains both shapes for the same dictation: `data="测试一下"` (the whole sentence re-inserted after select-all) and `data="内容"` (only the new fragment), while `value` was always the post-insert text. xterm forwards `data` verbatim and never reads or clears `value` (verified in the vendored 5.5.0 source), so anything trusting `data` either duplicates or loses text.
  - **Ordering, measured not assumed:** xterm registers its `input` listener inside `term.open()`, the client adds its own afterwards. On the same target element capture does *not* win — listeners run in registration order — so `onData` always fires first, after the browser has already written `value`. An earlier design used a `beforeinput` flag to tell `onData` to stand down and let `input` do the sending; because `onData` ran first, every keystroke was dropped.
  - **Keys** are still transmitted by xterm from `data` (it encodes them correctly), then the field is cleared: WebKit inserts some keys into it even after xterm cancelled the default action, and an accumulating field is what replayed old spaces. The key marker is recorded by a **capture listener on the pane**, before xterm's own keydown handler, since xterm forwards some keys into `onData` synchronously.
  - **Dictation** sends only the difference between `value` and what the PTY already holds for the utterance. Appends go out at once; a retraction or rewrite is held 600 ms and re-evaluated against the newest value on every event, so a recognition pass that catches back up costs nothing. The hold must always track the latest value — freezing it is what deleted characters the user had just re-dictated.
  - An empty field is a **buffer reset, not a retraction**. Platforms clear the textarea around commit, and treating `""` as "delete everything sent" silently ate text the user had already produced. Deleting the line itself comes through Backspace/Ctrl-U, which end the utterance explicitly.
  - `Enter`, `Backspace`, arrows and Ctrl chords never enter the document: they go straight to the PTY and end the utterance, because the cursor may no longer be at the end of the text.
  - A keydown whose `keyCode` is not 229 (not an IME placeholder) clears the composition flags. Without that, a `compositionend` that never arrives — switching IMEs, losing focus mid-composition — latches `composing` on and every later character is treated as pre-edit text and swallowed.
  - WebKit can hand spaces back as NO-BREAK SPACE (U+00A0), which is not a word separator in a shell and made commands fail silently while looking correct; the invisible variants are folded back to a plain space before anything is sent.
  - Pre-edit text (`inputType=insertCompositionText`, `isComposing`) never reaches the PTY.
- All text handling uses `Array.from()` code points, never UTF-16 units, so surrogate pairs (emoji, CJK extension chars) are never split. `tools/replay-web-input.js` replays captured scenarios (`spaces`, `keys`, `typing`, `dictation`, `rewrite`, `mixed`, `ime`, `mods`, `screen`, `screen-mac`, `screen-shift`, `screen-caps`, `screen-arrows`, `mouse`) against a headless browser and prints the bytes that reached the PTY. `spaces` and `keys` drive **real** key events through CDP (`Input.dispatchKeyEvent`); synthesised DOM events skip the editor's own key handling and hide ordering bugs — an earlier harness passed every scenario while the real keyboard was completely dead.
- The `⌨` toolbar toggle (top right, persisted in `localStorage`) reveals a row of keys phones cannot type. It sits between the terminal and the status bar, so with the soft keyboard open it floats right above it. Tapping a key goes through the same ordered input chain as typed characters; `pointerdown` uses `preventDefault()` so xterm's hidden textarea keeps focus and the keyboard stays open, and `blur()` drops the pressed state afterwards.
  - The middle strip scrolls horizontally, ordered `Esc`, `Tab`, then the sticky modifiers, then `⌫` and the Ctrl chords (`^C ^D ^Z ^A ^E ^U ^K ^L ^R`); the four arrow keys are pinned in a separate pad on the right.
  - `Shift`, `Ctrl`, `Alt` and `Win`/`Cmd` follow the same three-state model as a hardware keyboard and the platform on-screen keyboards: **one tap arms the next key** (blue), **a quick double tap locks it** (amber, inner ring) until released, and tapping a locked key releases it. A single tap on an already armed key cancels it after the double-tap window, so a mis-tap is never sticky. One-shot modifiers are spent at the single `sendInput()` choke point, after the key has been encoded: tap `Ctrl`, tap `a` → `0x01`, then plain `a` → `0x61`. Locked modifiers are not spent, so double-tap `Ctrl`, then `a a` gives `0x01 0x01`. They modify both toolbar keys and characters typed on the soft keyboard, so combinations like Ctrl+Arrow, Alt+letter, Shift+Tab and Ctrl+C work without a physical keyboard. Modifier numbering matches the native encoder (`1 + shift + 2·alt + 4·ctrl`, plus `8·meta`); hiding the toolbar clears all modifier state.
  - `⌫` and the arrow keys auto-repeat while held (400 ms initial delay, then one stroke every 45 ms). xterm only serialises input into bytes and physical-key repeat comes from the OS, so the toolbar emulates it in the browser — a mobile soft keyboard's own Backspace usually does not repeat. A plain tap still sends exactly one key. Repeat stops on release, on tab switch, and when the toolbar is hidden; Ctrl+⌫ sends BS (`0x08`) and Alt/Cmd+⌫ prefixes ESC, matching the native encoder.
- The **keyboard-mode button** (top right, persisted in `localStorage`) picks who provides the keys, so the two never fight over the same textarea:
  - **系统键盘** (default): the browser/OS keyboard is used, including IME and iOS dictation; the compact `⌨` toolbar and its one-shot/locked modifiers are available here.
  - **屏幕键盘**: a built-in full US layout (F1–F12, number row, editing keys, arrows, Ctrl/Alt/Win/Shift) becomes an extra grid row; the OS soft keyboard is deliberately not raised. A tap dispatches a native-shaped `keydown`/`keyup` at xterm's textarea with real modifier flags, so **xterm does the encoding** — there is no virtual modifier state, no `value` reconciliation and no IME in this mode. Held keys are only "down" while a finger is on them; a physical key arriving releases them immediately.
  - The arrow pad is a standard inverted T: `↑` is centred above `↓`, with `← ↓ →` on the row below. The screen keyboard's home row has **Caps Lock on the left and Enter/return on the right**, as on a physical keyboard. Caps Lock is a real toggle (amber while locked): letters stay upper case until it is pressed again, and Shift temporarily inverts it. It only affects letters; digit/symbol keys use their Shift glyphs. Lock state is cleared when switching keyboard mode or layout.
  - Letter caps are lower case by default and switch to upper case under Shift (or under Caps Lock); digit/symbol keys show their shifted glyph in the corner and swap it into the main position under Shift. The cap and the emitted byte are always the same character — an upper-case letter also sends the `keypress(charCode)` that xterm waits for before emitting it.
  - The screen keyboard has its own **Win/Mac 样式** toggle in a fixed strip above the keys (persisted in `localStorage`, defaulting to the host platform). Win uses `Ctrl Win Alt … Alt Win Ctrl`; Mac uses `⌃ ⌥ ⌘ … ⌘ ⌥ ⌃` and labels Enter as `return`. This is key-cap style only: `⌘` stays `metaKey`, `⌥` stays `altKey`, and both layouts go through the same native event dispatch — no second encoding path. Rebuilding the layout releases any held modifiers.
  - Switching to 屏幕键盘 hides the compact toolbar and clears its modifiers, so a latched Ctrl can never be applied twice. The mode switch is the only authority: no touch/pointer detection silently changes input behaviour. A hardware keyboard still works in either mode.

- Mouse reporting works in the web view too. Because `/screen.ansi` is a **full snapshot**, xterm never receives the DECSET/DECRST that switched the native terminal into mouse mode — a click was only a DOM event and produced no PTY bytes. `web_ansi::encode_screen` now mirrors the native mouse-mode bits (`?1000` / `?1002` / `?1003` / `?1006`) at the head of every repaint. xterm then generates the report from the real mouse event; both `onData` (SGR encoding) and `onBinary` (legacy X10/VT200/DRAG encoding) are forwarded through the same `sendInput()` choke point.
- The web grid **always mirrors the native PTY grid exactly** (`dto.cols × dto.rows`), so the ANSI repaint and the cursor never land on a differently sized buffer. The browser only scrolls or pads around that grid; it never picks its own cols/rows from CSS.
- The **size-mode button** (top right, persisted in `localStorage`) picks who is authoritative for the grid — the point is that the authoritative side never scrolls:
  - **PC 尺寸** (default): the native window is authoritative. The browser mirrors its grid and never resizes the host PTY; if this viewport is smaller it scrolls, if larger it pads with tmux-style `.`.
  - **浏览器尺寸**: this viewport is authoritative. `POST /tabs/:id/resize` with `sync_window:true` adopts the browser's grid **and** resizes the native window to match, so the PC shows the same cells without scrolling either. Touch devices are included on both axes here.
  - In both modes the soft keyboard only changes what is visible: no PTY or window resize is sent while it is up, the live bottom is scrolled above it, and closing it does not replay a transient resize. Switching back to PC 尺寸 drops the browser grid override and follows the window's current size again.
- The native title bar shows `● web 已连接` while a web client is attached. Heartbeats use `POST /web/client`; a stale client is dropped after 5 seconds. Only one external web client is supported, and a second one receives HTTP 409.
- Closing the last tab leaves an empty native window instead of quitting, so the HTTP/web service stays up; the web page shows a single `+` to create a new tab.
- "Scrollback" fetches `GET /tabs/:id/scrollback` into an overlay (xterm.js's own scrollback is only a local replay buffer).
- Only one instance runs at a time (tracked by `~/.rterminal/instance.lock`). A second launch prints the running PID and exits; it cannot start a second HTTP server even with a different `--api-addr`.

The server has no authentication. Keep the default loopback bind; exposing `--api-addr 0.0.0.0` exposes full terminal control to anyone who can reach the port.

### Logging & Tracing
- Input event tracing: `AGENT_TUI_INPUT_TRACE=1`
- Structured JSONL input logging: `--input-log-file <path>` (with optional `--input-log-raw`)
- Per-tab persistent PTY transcripts: raw `.ansi` output plus `.meta.json` under `~/.rterminal/history`
- **Browser input debug stream** (`◎` button in the web client's top bar): while it is on, the page records every `keydown` / `keypress` / `composition*` / `beforeinput` / `input` / `paste` / focus event, what xterm's `onData` delivered, whether the composition gate swallowed it, whether dedupe trimmed it, the exact bytes handed to the PTY, and the cursor line after each repaint. Batches go to `POST /web/debug` (every 400 ms, and via `sendBeacon` on `pagehide`), which appends them to `~/.rterminal/web-debug/web-debug.log`. Input requests made while recording also carry `X-Rterminal-Debug: 1`, so the server logs the bytes it actually received next to the client's view of them — that is what separates a browser-side duplication from a PTY/repaint one. Read it back with `GET /web/debug/log?lines=500`. The recorder is on by default in this build (`localStorage.rterminal.debug`), the button turns red while it records, and it only ever appends to a local file.

## Usage

```bash
# Basic launch
cargo run

# With options
cargo run -- \
  --font-family "JetBrains Mono" \
  --font-fallback "Symbols Nerd Font Mono,Apple Symbols" \
  --theme eye-care \
  --force-vertical-cursor \
  --cursor-trail \
  --ambiguous-width double \
  --double-width-char "↑,↓,↕"

# Self-check (verify terminal core initializes correctly)
cargo run -- --self-check

# With input debugging
AGENT_TUI_INPUT_TRACE=1 cargo run -- --input-log-file /tmp/input.jsonl --input-log-raw

# Save per-tab raw PTY output transcripts to a custom directory
cargo run -- --history-log-dir /tmp/agent-terminal-history
```

### CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `--font-family <name>` | `Menlo` (macOS) / `Cascadia Mono` (Windows) | Terminal font family |
| `--font-fallback <name,...>` | — | Comma-separated fallback font families |
| `--double-width-char <char,...>` | — | Characters forced to double-width rendering |
| `--ambiguous-width <single\|double>` | `single` | Width for Unicode ambiguous-width characters |
| `--theme <default\|eye-care>` | `default` | Color theme |
| `--force-vertical-cursor` | off | Always use beam cursor regardless of app mode |
| `--cursor-trail` | off | Enable trailing glow effect on beam cursor |
| `--no-cursor-slide` | off | Disable smooth cursor movement animation |
| `--no-option-as-meta` | off | Treat Option key as native input instead of Meta/Alt |
| `--show-status-bar` | off | Show debug status bar at bottom |
| `--api-addr <host:port>` | `127.0.0.1:7878` | Bind address for the HTTP control API (use `:0` for an OS-assigned port; the actual port is logged on startup) |
| `--input-log-file <path>` | — | Write structured input events to JSONL file |
| `--input-log-raw` | off | Include full text values in input log (not truncated) |
| `--history-log-dir <dir>` | `~/.rterminal/history` | Write per-tab raw PTY output transcripts (`.ansi`) and metadata sidecars |
| `--self-check` | — | Run startup self-check and exit |

## Tech Stack

- **UI Framework**: [GPUI](https://github.com/zed-industries/zed) — Zed's GPU-accelerated, Rust-native UI framework
- **Terminal Core**: [alacritty_terminal](https://github.com/andelf/alacritty/tree/rterminal-patches/alacritty_terminal) (fork with rterminal patches) — VT/ANSI parsing and terminal state machine
- **PTY**: [portable-pty](https://crates.io/crates/portable-pty) — Cross-platform PTY abstraction
- **macOS Interop** (macOS target only): [cocoa](https://crates.io/crates/cocoa) + [objc](https://crates.io/crates/objc) — Native Objective-C bridge for accessibility APIs
- **CLI**: [clap](https://crates.io/crates/clap) — Argument parsing
- **Debug HTTP**: [tiny_http](https://crates.io/crates/tiny_http) — Lightweight HTTP server for debug endpoints

## Building

Requires Rust 1.98.1 (pinned in `rust-toolchain.toml`, matching the upstream zed
revision) and the 2024 edition (`edition = "2024"` in `Cargo.toml`). GPUI ships
native backends for both macOS and Windows, so the project builds on either. All
platform-specific defaults (shell, font, home directory, key chords, menus) are
isolated in `src/platform.rs`; the macOS branches preserve the original behaviour.

```bash
cargo build
cargo test
cargo run -- --self-check
```

On Windows the default PTY shell is `powershell.exe`; set `RTERMINAL_SHELL` to
use `pwsh.exe`, Git Bash, or another shell. The `~/.rterminal/history` default
expands via `USERPROFILE` (falling back to `HOME`).

Powerline/Nerd Font prompts (for example `oh-my-posh`) emit Private Use Area
code points such as U+E0B0. Plain `Cascadia Mono` has no glyphs there, and the
`--font-fallback` list is empty by default, so DirectWrite falls back to a
missing-glyph box. Install a Nerd Font (or `Cascadia Code PL`) and select it
with `--font-family`, or list it via `--font-fallback`.

## Known Limitations

- **Native accessibility bridge is macOS only** — the `AXTextField` input-line bridge in `src/macos_ax.rs` needs AppKit; on Windows it is a no-op, and the tmux-style HTTP control API is the supported way for agents to read/inject input
- **Per-cell text shaping** — rendering shapes each character individually rather than batching runs per line; functional but not optimal for performance
- **Input-line model drift** — the shadow `input_line` can desynchronize from the actual shell state in complex scenarios (tmux prefix sequences, shell history navigation, tab completion)
- **No search** — no find-in-terminal functionality
- **No hyperlink interaction** — OSC 8 hyperlinks are not yet clickable
- **No bold/italic font variants** — text style flags are parsed but not rendered with distinct font faces

## License

This project is currently private and unlicensed.
