# Content Scroll Animation Plan

## Goal

Add optional pixel-level movement for terminal content when visible rows move up or down. The animation should make line insertion and scrollback movement feel continuous while keeping terminal state, cursor logic, selection, IME, and copy behavior row-based.

## Current State

The current model can repaint the latest terminal contents, and it can represent manual wheel scroll in integer rows.

- `src/terminal.rs::refresh_snapshot()` rebuilds `ScreenSnapshot` from `Term::renderable_content()`.
- `ScreenSnapshot` stores visible cells, `soft_wrapped_rows`, cursor position, cursor visibility, and `alt_screen`.
- `src/input.rs::on_scroll_wheel()` calls `term.scroll_display(Scroll::Delta(y_steps))` for normal-mode scrollback.
- `src/render.rs` paints each visible row at `origin.y + row_index * line_height`.
- Cursor slide animation already has a working frame loop: state in `AgentTerminal`, interpolation by time, and `cx.on_next_frame(... cx.notify())`.
- `src/snapshot_tab.rs` stores `top_line` as an integer and renders rows from that logical line.

The missing piece is an explicit content-motion signal. PTY output currently produces a new snapshot; the app does not store how many visible rows moved between old and new snapshots.

## Scope Contract

Initial implementation scope:

- Live terminal content animation for normal-mode wheel scroll.
- Live terminal content animation for small PTY-induced row shifts.
- Render-only pixel offset; terminal grid, cursor position, selection coordinates, IME range, and copy extraction remain logical row/column state.
- Snapshot tab animation is a follow-up using the same model after live terminal behavior is stable.

Out of initial scope:

- Smooth pixel-level scrollback storage inside alacritty_terminal.
- Animation for alternate-screen mouse-mode wheel events sent to the PTY.
- Animation for large output bursts, clear screen, resize, font-size changes, and alt-screen transitions.

## State Design

Add a render-only animation state to `AgentTerminal`.

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContentScrollReason {
    Wheel,
    Pty,
}

#[derive(Clone, Copy, Debug)]
struct ContentScrollAnimation {
    from_px: f32,
    to_px: f32,
    started_at: Instant,
    duration: Duration,
    reason: ContentScrollReason,
}
```

Suggested fields:

```rust
pub(crate) content_scroll_anim: Option<ContentScrollAnimation>,
pub(crate) last_snapshot_for_scroll: Option<ScreenSnapshot>,
pub(crate) last_scroll_delta_rows: i32,
```

The visual offset function mirrors cursor animation:

```rust
fn content_scroll_offset_px_at(&self, now: Instant) -> f32
fn content_scroll_animation_active_at(&self, now: Instant) -> bool
```

Use an ease-out curve and a short duration:

- Wheel: 80ms.
- PTY row insertion: 90-120ms.
- Clamp visual distance to a small row count so high-throughput output stays current.

Add independent CLI controls:

- `--no-content-scroll-animation`, matching the existing `--no-cursor-slide` style.
- `--content-scroll-duration-ms <N>`, so tuning does not require a rebuild.

Content scroll and cursor slide remain separate feature flags because they solve separate visual problems.

## Event Sources

### Manual Wheel Scroll

`src/input.rs::on_scroll_wheel()` already has `y_steps`.

Implementation:

1. Capture old top display state before `term.scroll_display`.
2. Call `term.scroll_display(Scroll::Delta(y_steps))`.
3. Call `refresh_snapshot()`.
4. Start animation with `delta_rows = applied_rows`, where applied rows should account for clamping at top/bottom.

The first implementation can use requested `y_steps` and skip animation when the new snapshot is unchanged. A later refinement can compute exact applied delta if alacritty exposes the display offset directly.

### PTY-Induced Scroll

`src/terminal.rs::ingest_batch()` is the correct entry point because PTY chunks are coalesced before one UI update.

Implementation:

1. Clone old `ScreenSnapshot` before processing the batch.
2. Process chunks through `processor.advance`.
3. Build the new snapshot.
4. Compute a row-shift delta from old/new snapshots.
5. Start animation for small deltas only.

Practical delta detector:

- Compare row fingerprints of old and new visible rows.
- Detect common cases:
  - old rows `k..` match new rows `0..n-k` => content moved up by `k`.
  - old rows `0..n-k` match new rows `k..` => content moved down by `k`.
- Support `k` in `1..=3` in the first implementation.
- Fingerprint the complete visual row contents using `(ch, fg, bg, width_cols, spans_next_col, expands_layout)` for each cell.
- Include trailing spaces and style data. Terminal output can use background-only cells, and text-only matching can produce false shifts.

Fallback behavior:

- If no stable shift is detected, snap.
- If the batch changes many rows, snap.
- If `alt_screen` changed, snap.
- If grid size changed, snap.

## Coalesce And Saturation Rules

The animation must never make terminal output feel delayed.

Rules:

- Maximum animated delta: 3 rows for PTY, 6 rows for wheel.
- If a new scroll event arrives while an animation is active, calculate current visual offset and combine it with the new delta, then clamp to the max distance.
- If PTY output arrives faster than animation duration, keep the latest snapshot as the source of truth and only animate the visual offset toward zero.
- If a paste or command outputs a large burst, use snap mode.

Concrete coalescing formula:

```rust
let current_visual_px = self.content_scroll_offset_px_at(now);
let delta_px = delta_rows as f32 * line_height;
let max_px = max_rows as f32 * line_height;
let from_px = (current_visual_px + delta_px).clamp(-max_px, max_px);

self.content_scroll_anim = Some(ContentScrollAnimation {
    from_px,
    to_px: 0.0,
    started_at: now,
    duration,
    reason,
});
```

This makes repeated events hand off from the current visual position. Reusing the current position avoids a visible jump when a new PTY batch or wheel event arrives before the previous animation finishes.

Recommended constants:

```rust
const CONTENT_SCROLL_DURATION: Duration = Duration::from_millis(100);
const CONTENT_SCROLL_MAX_PTY_ROWS: i32 = 3;
const CONTENT_SCROLL_MAX_WHEEL_ROWS: i32 = 6;
```

## Render Integration

Add one y offset in `src/render.rs`.

Current row paint:

```rust
let y = origin.y + row_index as f32 * line_height;
```

Planned row paint:

```rust
let content_y_offset_px = self.content_scroll_visual_offset_px();
let origin = bounds.origin + point(TEXT_PADDING_X, dynamic_padding_y + content_y_offset_px);
```

Apply the offset at the `origin` definition, not inside individual row or cursor formulas. That single integration point automatically moves cells, backgrounds, selection highlights, cursor, and IME together.

Hit testing stays logical:

- Mouse position maps to current row/col using the existing grid model.
- Selection storage stays `SelectionPoint { row, col }`.
- Copy still uses `ScreenSnapshot.soft_wrapped_rows`, preserving soft-break and hard-break behavior.

Canvas clipping is required so offset rows do not draw outside the terminal surface. If GPUI canvas clipping is already bounded to the canvas, document that assumption; otherwise explicitly skip painting cells with row bounds outside the viewport.

Schedule animation frames using the existing cursor pattern:

```rust
if cursor_sliding || content_scroll_active {
    cx.on_next_frame(window, |_, _, cx| cx.notify());
}
```

Verify whether GPUI dedupes multiple `on_next_frame` registrations in the same render pass. If it does not, compute one `needs_next_frame = cursor_sliding || content_scroll_active` boolean and register a single callback.

## Eviction Row Painting

A sliding snapshot needs the row that just left the visible window. Without it, an upward scroll starts with a blank strip at the top, because the new row 0 is painted one line lower while the old top row has already been discarded.

Use `last_snapshot_for_scroll` while the animation is active:

- Upward content movement: paint the evicted old top rows above the new snapshot, at `origin.y - k * line_height` through `origin.y - line_height`.
- Downward content movement: paint the evicted old bottom rows below the new snapshot.
- Apply the same clipping rules as normal rows.
- Store only the snapshot needed for the active animation, then clear it when the animation finishes or a snap fallback occurs.

For the first patch, support up to the same max rows as the delta detector. This keeps memory and paint cost bounded.

## Snapshot Tab Follow-Up

`src/snapshot_tab.rs` is simpler because scrolling is driven by `top_line`.

Follow-up design:

- Add `scroll_anim: Option<ContentScrollAnimation>` to `SnapshotTab`.
- In `on_scroll_wheel`, compute `old_top_line -> next`.
- Start animation from `(old_top_line - next) * line_height` to `0`.
- Render visible rows with pixel offset.
- Keep selection and copy logical over `line_index`.

This should follow the live terminal implementation so constants and easing stay shared.

## Snap Fallback Conditions

Use immediate repaint for:

- Resize or font-size changes.
- Alt-screen enter/exit.
- Clear screen or near-full-screen redraw.
- PTY batch that changes more than the small-delta threshold.
- Scroll deltas that hit scrollback top/bottom and produce no visual row movement.
- Selection drag in progress, using `selection_mode_active && selection_button.is_some()`.

After mouse up, animation can resume because selection endpoints are committed to logical row/column coordinates.

## Validation Plan

Automated tests:

- Unit-test row-shift detector with 1-row, 2-row, down-scroll, no-match, and large-delta cases.
- Unit-test coalescing and clamping logic.
- Keep existing copy tests for soft-wrap/hard-break behavior.
- Add a regression test around selection coordinates: mouse down at one logical cell, PTY scroll during drag snaps, mouse up commits the intended logical anchor/focus.

Manual checks:

- Wheel scrollback moves smoothly in normal mode.
- Repeated `printf` or shell output with one new line at a time slides upward smoothly.
- `cat` of a large file snaps without lag.
- Resize snaps and leaves cursor/selection correct.
- Alt-screen apps such as `vim`, `less`, and `top` do not animate PTY internal redraws.
- Shift+drag selection still selects the intended logical rows.
- IME marked text and cursor remain visually aligned during a small scroll.

## Implementation Steps

1. Extract a small row-fingerprint helper and `detect_snapshot_row_shift(old, new, max_rows)`.
2. Add `ContentScrollAnimation` state and visual offset helpers to `AgentTerminal`.
3. Start wheel animation from `on_scroll_wheel`.
4. Start PTY animation from `ingest_batch` using old/new snapshot comparison.
5. Paint eviction rows from `last_snapshot_for_scroll` while animation is active.
6. Apply `content_y_offset` once at `origin` in `render.rs`.
7. Add frame scheduling while content animation is active.
8. Add tests for detection, coalescing, snap fallbacks, and selection coordinates.
9. Add snapshot tab animation as a second patch after live terminal behavior is accepted.

## Review Questions

- Initial PTY animation supports `k in 1..=3`.
- Selection drag snaps while `selection_mode_active && selection_button.is_some()`.
- Snapshot tab animation is a follow-up patch.
- Add `--no-content-scroll-animation` plus `--content-scroll-duration-ms`.
