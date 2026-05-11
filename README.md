# Agent Terminal

A GPU-accelerated terminal emulator built with [GPUI](https://github.com/zed-industries/zed) (Zed's UI framework) and [alacritty_terminal](https://github.com/alacritty/alacritty), designed as a standalone native macOS terminal with first-class accessibility and input method support.

## Background

This project originated from a specific need: building a terminal emulator that treats **accessibility-driven input** as a first-class concern, rather than an afterthought. Traditional terminal emulators expose minimal accessibility semantics — most only forward raw key events to a PTY, leaving assistive technologies (Voice Control, screen readers, accessibility automation tools) unable to read or modify the current command line.

Agent Terminal takes a different approach:

- It maintains a **shadow input-line model** that mirrors what the user is typing in the shell
- It exposes this model to macOS Accessibility APIs as an `AXTextField`, allowing external tools to **read the current input**, **know the cursor position**, and **inject or replace text**
- It bridges bidirectionally between the native accessibility tree and the internal input state on every render frame

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
| `terminal.rs` | ~1100 | Core terminal state: PTY lifecycle, `Term` wiring, snapshot generation, cursor animation |
| `input.rs` | ~1600 | Keyboard/mouse/IME/paste handling, input-line shadow model, AX override logic, selection |
| `render.rs` | ~510 | GPUI `Render` impl, per-cell canvas painting, cursor drawing, AX sync entry point |
| `keyboard.rs` | ~300 | Keystroke-to-terminal-byte encoding (special keys, Ctrl chords, Alt, modifiers) |
| `tabs.rs` | ~470 | Multi-tab management, tab bar rendering, Cmd+N shortcuts |
| `snapshot_tab.rs` | ~540 | Read-only snapshot tabs with scrollback, selection, copy |
| `macos_ax.rs` | ~140 | Native Objective-C bridge: `setAccessibilityValue` / `setAccessibilitySelectedTextRange` |
| `debug_server.rs` | ~520 | HTTP debug API (`/debug/state`, `/debug/screen`, `/debug/input`, `/debug/replace-line`) |
| `text_utils.rs` | ~180 | UTF-16 ↔ byte index conversion, word deletion, AX override heuristics |
| `pty.rs` | ~85 | PTY creation via `portable-pty`, background reader thread |
| `color.rs` | ~150 | ANSI → HSLA color mapping (named, indexed 256, dim/bright, spec RGB) |
| `cli.rs` | ~170 | CLI argument parsing via `clap` |
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

### Debugging & Observability
- HTTP debug server on `localhost:7878` (auto-increments port per tab)
- `GET /debug/state` — JSON snapshot of terminal state, counters, uptime
- `GET /debug/screen` — plain-text dump of visible terminal content
- `POST /debug/input` — inject raw bytes into PTY
- `POST /debug/replace-line` — replace current shell input line
- Input event tracing: `AGENT_TUI_INPUT_TRACE=1`
- Structured JSONL input logging: `--input-log-file <path>` (with optional `--input-log-raw`)
- Per-tab persistent PTY transcripts: raw `.ansi` output plus `.meta.json` under `~/.rterminal/history`

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
| `--font-family <name>` | `Menlo` | Terminal font family |
| `--font-fallback <name,...>` | — | Comma-separated fallback font families |
| `--double-width-char <char,...>` | — | Characters forced to double-width rendering |
| `--ambiguous-width <single\|double>` | `single` | Width for Unicode ambiguous-width characters |
| `--theme <default\|eye-care>` | `default` | Color theme |
| `--force-vertical-cursor` | off | Always use beam cursor regardless of app mode |
| `--cursor-trail` | off | Enable trailing glow effect on beam cursor |
| `--no-cursor-slide` | off | Disable smooth cursor movement animation |
| `--no-option-as-meta` | off | Treat Option key as native input instead of Meta/Alt |
| `--show-status-bar` | off | Show debug status bar at bottom |
| `--input-log-file <path>` | — | Write structured input events to JSONL file |
| `--input-log-raw` | off | Include full text values in input log (not truncated) |
| `--history-log-dir <dir>` | `~/.rterminal/history` | Write per-tab raw PTY output transcripts (`.ansi`) and metadata sidecars |
| `--self-check` | — | Run startup self-check and exit |

## Tech Stack

- **UI Framework**: [GPUI](https://github.com/zed-industries/zed) — Zed's GPU-accelerated, Rust-native UI framework
- **Terminal Core**: [alacritty_terminal](https://github.com/alacritty/alacritty) (vendored) — VT/ANSI parsing and terminal state machine
- **PTY**: [portable-pty](https://crates.io/crates/portable-pty) — Cross-platform PTY abstraction
- **macOS Interop**: [cocoa](https://crates.io/crates/cocoa) + [objc](https://crates.io/crates/objc) — Native Objective-C bridge for accessibility APIs
- **CLI**: [clap](https://crates.io/crates/clap) — Argument parsing
- **Debug HTTP**: [tiny_http](https://crates.io/crates/tiny_http) — Lightweight HTTP server for debug endpoints

## Building

Requires Rust 2024 edition (edition = "2024" in Cargo.toml) and macOS (GPUI currently targets macOS).

```bash
cargo build
cargo test
cargo run -- --self-check
```

## Known Limitations

- **macOS only** — GPUI's platform layer currently targets macOS; Linux/Windows support depends on upstream
- **Per-cell text shaping** — rendering shapes each character individually rather than batching runs per line; functional but not optimal for performance
- **Input-line model drift** — the shadow `input_line` can desynchronize from the actual shell state in complex scenarios (tmux prefix sequences, shell history navigation, tab completion)
- **No search** — no find-in-terminal functionality
- **No hyperlink interaction** — OSC 8 hyperlinks are not yet clickable
- **No bold/italic font variants** — text style flags are parsed but not rendered with distinct font faces

## License

This project is currently private and unlicensed.
