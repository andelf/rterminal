# Terminal Implementation Research

## 1. libghostty / libghostty-vt

Repository cloned to `/Users/oker/Works/agent-tui/research/ghostty`.

### 1.1 High-level conclusion

- `libghostty` exists, but it is not yet a stable, general-purpose embedding SDK.
- The actually reusable public surface today is `libghostty-vt`.
- Full `libghostty` is currently closer to Ghostty's own host integration layer, mainly used by Ghostty's native Apple frontends.

### 1.2 Public surfaces

- Full embedding API: `include/ghostty.h`
- Reusable VT API: `include/ghostty/vt.h`

`ghostty.h` exposes opaque app/surface handles and APIs for:

- app lifecycle
- surface lifecycle
- keyboard/mouse/text input
- clipboard
- drawing and resize notifications
- focus and color scheme

`vt.h` exposes the smaller public VT-oriented API:

- key encoding
- OSC parsing
- SGR parsing
- paste safety checks
- allocator hooks
- Wasm helpers

### 1.3 How to use it

For external consumers, prefer `libghostty-vt`.

- C examples:
  - `example/c-vt/src/main.c`
  - `example/c-vt-key-encode/src/main.c`
  - `example/c-vt-sgr/src/main.c`
  - `example/c-vt-paste/src/main.c`
- Zig examples:
  - `example/zig-vt/src/main.zig`
  - `example/zig-vt-stream/src/main.zig`

Build entry points:

- `build.zig`
- `src/build/GhosttyLibVt.zig`
- `src/build/GhosttyZig.zig`

Design intent:

- `zig build lib-vt` builds `libghostty-vt`
- installs `include/ghostty/*.h`
- emits `libghostty-vt.pc`

### 1.4 Implementation structure

- `src/lib_vt.zig` is the public Zig module and the C export bridge.
- `src/terminal/main.zig` contains the core terminal implementation reused by Ghostty.
- `src/apprt/embedded.zig` contains the host-facing embedded runtime layer used by full `libghostty`.

Important detail: the C ABI is exported from Zig with `@export(...)`; the public C API is not a separate implementation.

### 1.5 Limitations

- `libghostty` is explicitly marked as not yet reusable as a general embedding API.
- `libghostty-vt` is usable, but its API is still marked unstable.
- Full `libghostty` is still strongly shaped around Ghostty's own app/runtime assumptions.
- Documentation for full embedding is sparse; comments point readers back into Zig implementation files.
- Full `libghostty` build/distribution is not yet a polished cross-platform SDK story.

### 1.6 Verification note

I did not complete binary verification in this environment because local C/Zig builds are blocked by antivirus. This section is based on source inspection.

## 2. How Zed Implements Its Terminal

Repository inspected: `/Users/oker/Repos/zed`

### 2.1 High-level architecture

Zed does not implement a terminal emulator from scratch.

It composes three layers:

1. `terminal` crate: terminal session model, PTY wiring, terminal state integration
2. `terminal_view` crate: GPUI rendering, input handling, panel integration
3. `project` / `remote` / `task` crates: shell selection, environment resolution, remote transport, task spawning

The key design choice is that Zed reuses `alacritty_terminal` as the terminal core.

Evidence:

- `crates/terminal/Cargo.toml` depends on `alacritty_terminal`
- `crates/terminal/src/terminal.rs` imports:
  - `alacritty_terminal::Term`
  - `alacritty_terminal::event_loop::EventLoop`
  - `alacritty_terminal::tty`

### 2.2 What each layer owns

#### `terminal` crate

Main file: `crates/terminal/src/terminal.rs`

Owns:

- `Term<ZedListener>` from `alacritty_terminal`
- PTY creation via `alacritty_terminal::tty::new(...)`
- event loop creation via `alacritty_terminal::event_loop::EventLoop`
- forwarding PTY output into terminal state
- forwarding UI input into PTY bytes
- terminal-local behaviors such as copy/paste, selection state, title changes, color requests, process exit tracking

#### `terminal_view` crate

Main files:

- `crates/terminal_view/src/terminal_view.rs`
- `crates/terminal_view/src/terminal_panel.rs`

Owns:

- GPUI view and focus handling
- mouse and keyboard event integration
- pane/tab/split/zoom behavior
- dock/center terminal panel behavior
- serialization/persistence of panel state

#### `project` / `remote` / `task`

Main file for shell creation:

- `crates/project/src/terminals.rs`

Owns:

- shell selection
- current directory choice
- environment resolution
- local vs remote terminal selection
- task terminals versus interactive shell terminals

## 3. Zed Terminal Call Chain

This is the action-to-PTY path for a normal terminal spawn.

### 3.1 User action to panel

Workspace/panel actions are registered in:

- `crates/terminal_view/src/terminal_panel.rs`
- `crates/zed/src/zed.rs`

Example flow:

1. user triggers `workspace::NewTerminal` or `workspace::OpenTerminal`
2. `TerminalPanel::new_terminal(...)` handles the action
3. panel decides whether to create a local shell or normal project shell

### 3.2 Panel to project

The panel calls one of:

- `project.create_terminal_shell(...)`
- `project.create_local_terminal(...)`
- `project.create_terminal_task(...)`

These are implemented in:

- `crates/project/src/terminals.rs`

### 3.3 Project decides local vs remote

`create_terminal_shell_internal(...)` does the important policy work:

- resolves terminal settings
- decides whether this terminal is local or via remote client
- chooses the shell
- resolves directory environment
- injects Zed terminal environment variables
- optionally prepares activation scripts such as Python venv activation
- constructs `TerminalBuilder::new(...)`

Remote terminals still use the same terminal UI/session model; only the command/shell construction path differs.

### 3.4 Builder creates PTY + terminal core

`TerminalBuilder::new(...)` in `crates/terminal/src/terminal.rs` performs:

1. normalize env
   - removes `SHLVL`
   - provides fallback `LANG` when missing
   - injects `ZED_TERM`, `TERM_PROGRAM`, `TERM=xterm-256color`, `COLORTERM=truecolor`
2. build shell parameters
3. build `alacritty_terminal::tty::Options`
4. create PTY with `tty::new(...)`
5. create `Term::new(...)`
6. create `EventLoop::new(...)`
7. get event-loop channel and spawn the IO loop
8. create Zed `Terminal` object around that machinery

### 3.5 Builder becomes a GPUI entity

Zed separates instantiation from subscription:

- `TerminalBuilder::new(...)` can fail
- `TerminalBuilder::subscribe(cx)` attaches the builder to GPUI state and starts draining Alacritty events into the model

This matches the note in:

- `crates/terminal_view/README.md`

### 3.6 PTY output to renderable content

Once the event loop runs:

1. shell/task writes bytes to PTY
2. Alacritty event loop parses and mutates `Term`
3. Zed receives `AlacTermEvent`s over a channel
4. `Terminal::process_event(...)` handles title, clipboard, color requests, wakeups, exit, bell, etc.
5. Zed snapshots render state into `last_content`
6. `TerminalView` reads that content and GPUI renders it

### 3.7 Input back to PTY

Input path is the inverse:

1. GPUI event arrives in `TerminalView`
2. key/mouse mappings translate UI input into terminal bytes
3. `Terminal::input(...)` or `Terminal::write_to_pty(...)` is called
4. bytes are sent to the PTY notifier
5. shell/app receives the input

Input mapping files:

- `crates/terminal/src/mappings/keys.rs`
- `crates/terminal/src/mappings/mouse.rs`

## 4. `terminal.rs` Detailed Implementation Notes

Main file: `crates/terminal/src/terminal.rs`

### 4.1 `TerminalBuilder`

`TerminalBuilder` exists because PTY creation can fail before a GPUI entity exists.

That split is explicit:

- `TerminalBuilder::new(...)` creates the terminal backend
- `TerminalBuilder::subscribe(cx)` binds it into GPUI async/model context

This is a practical workaround for UI model creation constraints, not just an aesthetic pattern.

### 4.2 Two terminal modes

There are two backend modes:

- `DisplayOnly`
- `Pty`

`new_display_only(...)` creates a terminal state without a real PTY. Zed uses this in some embedded or synthetic cases.

`new(...)` creates the real PTY-backed interactive terminal.

### 4.3 Event bridge

`ZedListener` implements `EventListener` for Alacritty and forwards `AlacTermEvent` into an unbounded async channel.

Then `subscribe(...)` runs an async loop that:

- processes the first event immediately for latency
- batches subsequent events for a few milliseconds
- coalesces wakeups
- updates the `Terminal` entity on the foreground thread

This is a classic latency-vs-throughput compromise.

### 4.4 Resize handling

`Terminal::set_size(...)` queues an internal resize event.

`process_terminal_event(...)` then:

- updates cached bounds
- sends `Msg::Resize(...)` to the PTY/event loop
- resizes the in-memory `Term`
- emits a wakeup if hyperlinks/search matches need relocation

So resize is applied to both:

- PTY-side window size
- local render/state model

### 4.5 Output-side event handling

`process_event(...)` handles the main events coming back from Alacritty:

- `Title` / `ResetTitle`
- `ClipboardStore` / `ClipboardLoad`
- `PtyWrite`
- `TextAreaSizeRequest`
- `CursorBlinkingChange`
- `Bell`
- `Wakeup`
- `ColorRequest`
- `Exit` / `ChildExit`

Important detail: color requests are answered inline in event order to preserve protocol ordering. The code comment explicitly calls this out.

### 4.6 Writing to the PTY

`write_to_pty(...)` is a small but important boundary:

- no-op for display-only terminals
- sends bytes through `Notifier`
- debug-logs the payload when logging is enabled

`input(...)` adds extra terminal UX semantics before writing:

- scroll to bottom
- clear current selection
- optionally record input in tests

### 4.7 Task lifecycle

A terminal can either be:

- an interactive shell terminal
- a task terminal carrying `TaskState`

On task exit, `register_task_finished(...)` decides whether to close, reveal output, retain scrollback, and notify completion listeners.

Task reuse/spawn policy lives one layer up in `TerminalPanel`, but task completion state is stored on the terminal object itself.

### 4.8 Process tracking

`crates/terminal/src/pty_info.rs` adds process awareness on top of the PTY:

- Unix uses `tcgetpgrp` to find the foreground process group
- `sysinfo` is used to fetch current process metadata
- title/breadcrumb updates are emitted when process/cwd changes
- kill behavior targets the foreground process group on Unix

This is why Zed can show better terminal titles and kill the active job more intelligently than a naive shell-only kill.

## 5. Practical Summary

### 5.1 Ghostty

- If the goal is reusable terminal core: use `libghostty-vt`
- If the goal is full app embedding: full `libghostty` is not mature enough yet for low-risk external adoption

### 5.2 Zed

Zed's terminal strategy is:

- reuse `alacritty_terminal` for terminal emulation and PTY event loop
- wrap it in a Zed-owned `Terminal` model
- integrate that model deeply into GPUI panes, tabs, focus, tasks, and remote workflows

It is an IDE-integrated terminal architecture, not a standalone terminal emulator architecture.

## 6. Zed Render Layer: How `TerminalView` Turns `last_content` Into UI

The important detail is that `TerminalView` itself is not the low-level painter.

Render ownership is split like this:

- `TerminalView`: wires UI actions, focus, key context, scrollbars, context menu, terminal subscriptions
- `TerminalElement`: does layout, prepaint, and paint for terminal contents

Main files:

- `crates/terminal_view/src/terminal_view.rs`
- `crates/terminal_view/src/terminal_element.rs`

### 6.1 `TerminalView` is the composition layer

`TerminalView::render(...)` does not iterate terminal cells directly.

Instead it:

1. updates the scroll handle
2. applies pending display-offset changes back to the `Terminal` model
3. sets up GPUI event handlers:
   - actions like copy/paste/scroll/clear/select-all
   - raw key down handling
   - right-click menu behavior
4. creates a wrapper `div`
5. mounts `TerminalElement::new(...)`
6. optionally adds custom scrollbars

So `TerminalView` is mostly the controller/composition layer.

### 6.2 `TerminalElement` is the real renderer

`TerminalElement` implements `gpui::Element`.

That means it owns:

- `request_layout(...)`
- `prepaint(...)`
- `paint(...)`

This is where `terminal.last_content()` is actually consumed.

### 6.3 What `prepaint(...)` does

`prepaint(...)` is the main translation stage from terminal state to paintable UI data.

It performs these steps:

1. computes font metrics
   - terminal font family
   - font fallbacks
   - font features
   - line height
   - cell width using the width of `m`
2. derives `TerminalBounds`
   - visible pixel bounds
   - line height
   - cell width
3. calls:
   - `terminal.set_size(dimensions)`
   - `terminal.sync(window, cx)`

That is the key moment where the terminal model is synchronized before painting.

Then it reads `terminal.last_content`, including:

- `cells`
- `mode`
- `display_offset`
- `cursor_char`
- `selection`
- `cursor`

### 6.4 `last_content.cells` becomes batched text runs + background rects

The heavy lifting happens in:

- `TerminalElement::layout_grid(...)`

This function does not paint cell-by-cell directly.

Instead it converts the visible cells into two optimized structures:

1. `Vec<LayoutRect>`
   - background rectangles
   - merged aggressively to reduce paint calls
2. `Vec<BatchedTextRun>`
   - adjacent cells with compatible text style are coalesced into one text run

This is an important optimization choice:

- fewer background quads
- fewer text shaping/paint calls
- less per-cell overhead

It also handles:

- inverse colors
- underline / undercurl / strikeout
- hyperlinks
- wide characters
- zero-width characters
- contrast correction for normal glyphs
- special treatment for decorative powerline / box-drawing glyphs

### 6.5 Visible-region clipping

`prepaint(...)` also computes the intersection between terminal bounds and the current GPUI content mask.

If the terminal is partially clipped by a scroll container, Zed only feeds the visible rows into `layout_grid(...)`.

So rendering is viewport-aware rather than always processing the whole terminal buffer.

That matters for:

- embedded terminals
- scrollable thread views
- other parent containers with clipping

### 6.6 Cursor, selection, hyperlinks, IME

Still in `prepaint(...)`, Zed derives:

- `CursorLayout`
  - block / underline / bar / hollow cursor
  - width adapts to wide glyphs
- selection/search highlight ranges
  - stored as relative terminal ranges
- hyperlink tooltip element
  - only when modifier + hover state matches
- IME cursor bounds
  - used by the input handler for marked-text placement
- optional `block_below_cursor`
  - extra embedded UI block rendered immediately below cursor line

These are all packed into `LayoutState`.

So `LayoutState` is effectively the render cache for one frame.

### 6.7 What `paint(...)` does

`paint(...)` takes `LayoutState` and emits pixels in a strict order:

1. paint terminal background quad
2. register mouse listeners and input handler
3. paint background rects
4. paint highlighted ranges
   - search matches
   - selection
5. paint batched text runs
6. paint IME marked text overlay if present
7. paint cursor if visible
8. paint `block_below_cursor` element if present
9. paint hyperlink tooltip if present

So the actual paint order is:

- background
- highlights
- text
- transient overlays
- cursor
- extra UI overlays

### 6.8 How input is still tied to the render element

`TerminalElement` also registers:

- mouse down/up
- drag selection
- mouse move / hover
- scroll wheel
- modifier-changed events
- IME integration through `TerminalInputHandler`

This means render and interaction are intentionally colocated at the element layer, not split into separate painter/controller subsystems.

### 6.9 Short summary

The render path is:

1. `TerminalView::render(...)`
2. `TerminalElement::request_layout(...)`
3. `TerminalElement::prepaint(...)`
4. sync terminal model and read `last_content`
5. convert visible cells into:
   - merged background rects
   - batched text runs
   - cursor/highlight/tooltip/layout metadata
6. `TerminalElement::paint(...)`

So the real answer to "how does Zed render terminal UI from `last_content`" is:

- `TerminalView` mounts `TerminalElement`
- `TerminalElement` turns `last_content` into a frame-local `LayoutState`
- that `LayoutState` is painted as quads, highlights, shaped text runs, cursor, and overlays

## 7. What We Learned

This is the main architectural takeaway from the Ghostty and Zed research.

### 7.1 A terminal renderer is not a text renderer

The input is not "text that happens to contain control characters".

The real pipeline is:

1. byte stream
2. VT/ANSI parser
3. terminal state machine
4. screen model
5. render-oriented snapshot/state
6. UI painting

The most important practical consequence is:

- control characters must be consumed before the UI layer
- the UI should never interpret VT/ANSI directly
- the UI should only consume already-decoded screen state

### 7.2 The correct intermediate representation is a screen model

Both Zed and Ghostty confirm that the stable boundary is not raw text and not UI nodes.

The useful boundary is a screen model containing things like:

- cells
- styles
- cursor
- selection
- scrollback/viewport position
- hyperlinks/highlights
- dirty information

In Zed, that boundary is `TerminalContent`.

In Ghostty, the boundary is split more explicitly between terminal state and `RenderState`.

### 7.3 Zed's practical model

Zed's implementation teaches a pragmatic integration pattern:

- reuse an existing terminal core (`alacritty_terminal`)
- synchronize it into a renderable snapshot (`last_content`)
- convert that snapshot into renderer-friendly batches
- paint through the host UI framework

The key render trick is that Zed does not render cell-by-cell in the UI tree.

Instead it converts cells into:

- merged background rectangles
- batched text runs
- separate overlays for cursor/highlights/IME/tooltips

That is a strong pattern for GPUI-style frameworks because it keeps the terminal as a custom drawing surface rather than exploding it into thousands of UI elements.

### 7.4 Ghostty's more explicit architectural lesson

Ghostty/libghostty-vt teaches a cleaner separation of responsibilities:

- terminal state is one layer
- renderer-facing state is another layer
- the render layer is viewport-aware and dirty-aware

This is the strongest design lesson from Ghostty.

Instead of making the UI read arbitrary terminal internals, a dedicated render-state abstraction gives:

- a smaller renderer API
- clearer ownership boundaries
- easier support for multiple backends
- easier headless testing and snapshotting
- easier optimization around dirty rows/viewport changes

### 7.5 Best synthesis for our own architecture

The best combined lesson is:

1. parse bytes into a terminal state machine
2. derive a dedicated render snapshot/state from terminal state
3. make the UI framework consume only that render state
4. batch paint operations instead of creating one UI node per cell

For a GPUI-based implementation, the ideal structure would be:

- `TerminalCore`
  - owns VT parsing and terminal semantics
- `TerminalRenderState`
  - owns viewport-local, renderer-facing screen data
- `TerminalElement`
  - maps render state into quads, shaped text runs, highlights, cursor, IME overlays

### 7.6 Specific implementation lessons we should carry forward

- Use a fixed cell grid derived from font metrics, not ad hoc text layout.
- Treat wide characters, zero-width characters, underline, inverse, and hyperlink state as cell metadata, not renderer-local guesses.
- Keep viewport and scrollback separate.
- Make render preparation viewport-aware so offscreen rows are not processed.
- Batch adjacent cells with identical style into shared text runs.
- Merge background regions before painting.
- Keep cursor/selection/search/IME as overlays, not as special-case text mutations.
- Track dirty state explicitly if we care about high refresh performance.
- Keep terminal protocol handling outside the UI layer.

### 7.7 The most important conclusion

If we build a terminal integration for our own system, the worst boundary would be:

- "give the renderer a stream of text and ANSI and let it figure it out"

The better boundary is:

- "give the renderer a render-state snapshot of a terminal viewport"

That is the clearest common lesson across both implementations.

## 8. Agent Terminal: Current Project Status

Repository:

- `/Users/oker/Works/agent-tui`

Main implementation files:

- `Cargo.toml`
- `src/main.rs`
- `AGENTS.md`

### 8.1 Project goal

The current goal is intentionally narrow:

- build a GPUI-based terminal proof-of-concept
- support a local interactive shell over a PTY
- render terminal output inside a GPUI window
- keep the scope to "terminal only"
- do not pull in Zed's panel/task/workspace integration

This is not intended to be a full IDE terminal yet.

### 8.2 Current design

The current PoC follows a very small version of the architecture discussed in the research:

1. PTY session
2. terminal core
3. render snapshot
4. GPUI paint

Concretely:

- `portable-pty` starts the shell and provides PTY reader/writer handles
- `alacritty_terminal::Term` holds terminal state and parses ANSI/VT output
- `ScreenSnapshot` stores the currently visible terminal cells
- GPUI `canvas(...)` paints the snapshot

The current types are:

- `PtySession`
  - opens PTY
  - spawns the user's shell
  - exposes output channel plus writer handle
- `AgentTerminal`
  - owns focus state
  - owns `Term<NoopListener>`
  - owns the ANSI processor
  - owns the latest `ScreenSnapshot`
  - handles keyboard and mouse focus input
- `CellSnapshot`
  - `ch`
  - `fg`
  - `bg`
- `ScreenSnapshot`
  - `cells`
  - `cursor_row`
  - `cursor_col`

### 8.3 Rendering pipeline in the current PoC

The current rendering path is:

1. shell writes bytes to PTY
2. background thread reads PTY bytes
3. bytes are forwarded into `Processor<StdSyncHandler>`
4. Alacritty updates `Term`
5. `refresh_snapshot()` walks `term.renderable_content()`
6. visible cells are converted into `CellSnapshot`
7. GPUI paints those cells with a monospace font in a `canvas`

Important detail:

- the first version used `Vec<String>` and therefore lost all cell styling
- the current version uses per-cell snapshots and keeps foreground/background colors

### 8.4 What has been completed

The current implementation now has:

- a GPUI window
- local shell startup through PTY
- PTY output pump on a background thread
- ANSI/VT parsing through `alacritty_terminal`
- visible terminal snapshot generation
- basic text rendering in GPUI
- basic ANSI foreground/background color rendering
- a cursor overlay
- basic keyboard input forwarding
- focus acquisition on mouse click

Input currently includes:

- printable single characters
- `space`
- `enter`
- `tab`
- `backspace`
- `escape`
- arrow keys
- `home`
- `end`
- `ctrl+<char>`
- `alt+<key>`

### 8.5 Attempts and iteration history

#### Attempt 1: plain-text snapshot

The first implementation approach was:

- parse PTY output with `alacritty_terminal`
- flatten the visible viewport into `Vec<String>`
- paint one shaped text line per row in GPUI

Why this was useful:

- it proved the basic data path
- PTY -> ANSI parser -> visible text -> GPUI

Why it was insufficient:

- all color/style information was discarded
- terminal rendering reduced to plain text only
- this caused "no colors displayed"

#### Attempt 2: GPUI API alignment

The first draft also failed to compile because of GPUI API mismatches:

- `TextRun.color` expected `Hsla`
- `shape_line(...)` expected `SharedString`
- font size conversion was wrong for `Pixels`
- cursor fill color used the wrong opacity API

These issues were fixed first so the project could pass `cargo check`.

#### Attempt 3: cell-level snapshot

After runtime feedback showed missing color support and a non-working space key:

- `ScreenSnapshot` was changed from `Vec<String>` to `Vec<Vec<CellSnapshot>>`
- per-cell foreground/background colors were extracted from Alacritty cell data
- `space` key handling was added

This is the current implementation.

### 8.6 Process correction and working rule

One important process correction was established during this work:

- code that does not compile must not be described as completed or validated
- build success is the minimum bar before claiming a PoC exists
- tests only make sense after compilation succeeds

This rule has been written into:

- `AGENTS.md`

### 8.7 Current color implementation

The current color path is intentionally minimal but functional.

It supports:

- named ANSI colors
- bright/dim named variants
- 256-color indexed palette
- simple inverse handling
- hidden text handling
- default transparent background behavior for the terminal background color

It does not yet try to fully match Alacritty's full rendering behavior.

In particular, it is still missing:

- exact color resolution parity with Alacritty config behavior
- underline/undercurl/strike rendering
- italic/bold font variant rendering
- hyperlink styling
- zero-width glyph handling beyond basic cell skipping

### 8.8 Current limitations

This PoC is still intentionally small.

Known limitations:

- no PTY resize handling yet
- fixed initial grid size (`80x24`)
- no scrollback UI
- no selection
- no copy/paste
- no IME
- no mouse reporting into terminal apps
- no search
- no hyperlink interaction
- no batching optimization; rendering is currently cell-by-cell
- no runtime verification recorded in the research doc yet beyond user feedback

The lack of batching means this renderer is correct enough for validation work, but not yet the right design for performance.

### 8.9 Relationship to Zed and Ghostty research

This PoC intentionally takes only the minimum useful parts from the earlier research.

Borrowed from Zed in spirit:

- `alacritty_terminal` as terminal core
- GPUI as rendering framework
- terminal state interpreted into a UI-facing snapshot before paint

Learned from Ghostty conceptually:

- parser/state/render should be separate layers
- a UI should paint a render-oriented snapshot, not raw terminal bytes

The current PoC is still closer to Zed's "terminal core plus custom GPUI rendering" path than to Ghostty's more explicit `RenderState` architecture.

### 8.10 Suggested next steps

If work continues, the most valuable next steps are:

1. implement resize from window bounds into PTY + `Term`
2. replace per-cell text shaping with grouped runs/background rect batching
3. add selection and clipboard support
4. add scrollback and viewport control
5. split current single-file PoC into:
   - `pty_session`
   - `terminal_model`
   - `terminal_snapshot`
   - `terminal_element`

## 9. Progress Log (2026-03-10)

This section records the latest validated status. It supersedes older limitation bullets in section 8 where they conflict.

### 9.1 What is now working

- Build and verification baseline:
  - `cargo check` passes
  - `cargo test` passes (13 tests)
  - `cargo run -- --self-check` passes
- Runtime stability:
  - fixed startup panic caused by UTF-8 boundary misuse when rendering non-ASCII glyphs (e.g. `➜`)
- Terminal interaction:
  - English input works
  - resize now updates both `Term` and PTY size
- Observability / debugging:
  - HTTP debug service added (`/debug/state`, `/debug/screen`, `/debug/input`, `/debug/note`)
  - runtime counters and status summary exposed in UI and debug API
- Chinese paste:
  - pasting Chinese text works (confirmed in runtime)

### 9.2 Key implementation attempts and outcomes

1. Keystroke path update (`key_char` support)
- Goal: allow IME-committed Unicode text to reach PTY.
- Outcome: partial success.
- Added UTF-8 forwarding for `key_char`; added regression tests.

2. IME in-progress filtering
- Goal: prevent composition seed key leakage (e.g. stray ASCII during composition).
- Outcome: works for covered event shape.
- Added guards using `is_ime_in_progress()` and tests for no-leak behavior.

3. Accessibility / IME text input channel
- Goal: handle platform `insertText`/`setMarkedText` path directly.
- Outcome: integrated but still not fully solving direct accessibility input in this app.
- Added GPUI `InputHandler` registration in paint (`window.handle_input(...)`) and minimal `EntityInputHandler` implementation.

4. Paste fallback
- Goal: provide reliable Chinese text entry path independent of direct IME event behavior.
- Outcome: success.
- Added `Cmd+V` (and `Ctrl+Shift+V`) paste handling via clipboard read and PTY write.

### 9.3 Current unresolved issue

- Direct Chinese input via Accessibility API is still not working reliably in this app.
- User-observed symptom:
  - using `axcli` (accessibility-based input simulation), Chinese input still fails;
  - only a single `a` may appear, and no expected Chinese text is committed.
  - direct key simulation also appears ineffective in current app runtime, e.g.:
    - `axcli --pid 12462 press Enter`
    - observed result: terminal did not reliably consume the simulated key event as expected.
- Current judgment:
  - paste path is validated and usable;
  - direct accessibility text-injection path remains a blocker and needs deeper event-trace debugging.

### 9.4 Next debugging focus

1. capture traces with the new runtime switch:
   - `AGENT_TUI_INPUT_TRACE=1 cargo run`
   - then replay accessibility inputs (including `axcli`) and collect logs
2. instrument and log raw key/input events around:
   - `KeyDownEvent.keystroke` (`key`, `key_char`, modifiers)
   - `EntityInputHandler::replace_text_in_range`
   - `EntityInputHandler::replace_and_mark_text_in_range`
3. capture one full `axcli` reproduction trace and map event order.
4. verify whether `axcli` emits:
   - key events only,
   - `insertText` only,
   - or mixed composition events.
5. based on trace, decide whether to:
   - adapt InputHandler range semantics further, or
   - add a dedicated text-insert API path for accessibility automation.

### 9.5 Zed comparison notes (2026-03-10)

Source checked:

- `/Users/oker/Repos/zed/crates/terminal_view/src/terminal_element.rs`
- `/Users/oker/Repos/zed/crates/terminal_view/src/terminal_view.rs`
- `/Users/oker/Repos/zed/crates/terminal/src/terminal.rs`

Observed behavior in Zed terminal input layer:

- Terminal registers a custom `InputHandler` in paint (`window.handle_input(...)`).
- `replace_text_in_range` commits text to PTY; `replace_and_mark_text_in_range` only tracks composition text.
- `text_for_range` returns `None`.
- `character_index_for_point` returns `None`.
- `selected_text_range` returns `None` in ALT_SCREEN mode, otherwise `Some(0..0)`.
- `apple_press_and_hold_enabled` is explicitly `false`.

Applied alignment in this project:

- switched from `ElementInputHandler` default wrapper to custom input handler so `apple_press_and_hold_enabled = false` can be set explicitly.
- aligned `text_for_range` and `character_index_for_point` return behavior with Zed (`None`).
- added ALT_SCREEN-aware `selected_text_range` behavior.

Important limitation confirmed:

- If accessibility tooling only delivers plain key events like `keydown key=\"a\" key_char=Some(\"a\")` for Chinese input (without `insertText`/marked-text callbacks), app side cannot reconstruct intended Chinese commit text from that event alone.
- In that case, reliable automation should use a direct text insertion path (e.g. existing debug HTTP input endpoint) instead of key simulation.

### 9.6 Accessibility-first input routing adjustment (2026-03-10)

To prioritize IME and accessibility compatibility, keyboard handling was changed to:

- macOS printable text keys (`key_char` present, no ctrl/alt/platform/function):
  - no longer written to PTY directly in `keydown`
  - are deferred to NSTextInputClient callbacks (`insertText` / `setMarkedText`)
- control/navigation keys remain in `keydown` terminal encoding path.

Expected effect:

- avoids leaking composition seed letters directly to PTY during IME input.
- keeps terminal control key behavior unchanged.

Verification after change:

- `cargo check` pass
- `cargo test` pass (16 tests)
- `cargo run -- --self-check` pass

### 9.7 Regression root cause and fix (Enter key) (2026-03-10)

Observed regression:

- `Enter` stopped working.
- Trace showed:
  - `keydown key="enter" key_char=Some("\n")`
  - then `keydown deferred to text input handler`.

Root cause:

- The defer predicate only checked whether `key_char` was non-empty.
- `Enter` carried `key_char = "\n"` in this runtime path, so it was misclassified as printable text and skipped terminal key encoding.

Fix:

- tighten `should_defer_to_text_input(...)`:
  - do not defer for known terminal control keys (`enter`, `tab`, arrows, etc.)
  - do not defer when `key_char` contains control characters.
- keep deferring real printable text so IME/accessibility Chinese input remains supported.

Added regression tests:

- `enter_with_newline_key_char_does_not_defer`
- `chinese_text_still_defers_to_text_input_on_macos`

Validation:

- `cargo check` pass
- `cargo test` pass (18 tests)
- `cargo run -- --self-check` pass

### 9.8 Status bar visibility and window title integration (2026-03-10)

Changes:

- status bar (top line) is now hidden by default.
- add CLI flag `--show-status-bar` to explicitly show the status bar.
- when shown, status bar uses monospaced font (`Menlo`).
- window title bar now reflects current terminal title (OSC title), with shell-based fallback.

Implementation notes:

- added CLI parsing for `--show-status-bar` and existing `--self-check`.
- grid-size calculation now accounts for whether the status bar is visible.
- wired terminal title events (`Title` / `ResetTitle`) through `EventListener` and synced them to `window.set_window_title(...)`.

Validation:

- `cargo check` pass
- `cargo test` pass (18 tests)
- `cargo run -- --self-check` pass
