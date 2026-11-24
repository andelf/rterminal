//! Encode the native `ScreenSnapshot` as an ANSI repaint for xterm.js.
//!
//! The first byte is `1` when the snapshot came from the alternate screen and
//! `0` otherwise; the rest is a stream that redraws every visible cell with
//! its original foreground/background and positions the cursor. The web client
//! keeps the alt-screen flag itself so switching buffers does not have to be
//! re-sent on every poll.

use crate::terminal::{CellSnapshot, ScreenSnapshot};

const ESC: char = '\u{1b}';

fn rgb(color: gpui::Hsla) -> (u8, u8, u8) {
    let rgba = color.to_rgb();
    (
        (rgba.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba.b.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn emit_cell_style(
    out: &mut String,
    cell: &CellSnapshot,
    current_fg: &mut Option<(u8, u8, u8)>,
    current_bg: &mut Option<(u8, u8, u8)>,
) {
    let fg = rgb(cell.fg);
    if Some(fg) != *current_fg {
        out.push_str(&format!("{ESC}[38;2;{};{};{}m", fg.0, fg.1, fg.2));
        *current_fg = Some(fg);
    }

    let bg = cell.bg.map(rgb);
    if bg != *current_bg {
        match bg {
            Some((r, g, b)) => out.push_str(&format!("{ESC}[48;2;{r};{g};{b}m")),
            None => out.push_str(&format!("{ESC}[49m")),
        }
        *current_bg = bg;
    }
}

pub(crate) fn encode_screen(snapshot: &ScreenSnapshot) -> String {
    let mut out = String::with_capacity(snapshot.cells.len() * 64 + 64);
    out.push(if snapshot.alt_screen { '1' } else { '0' });

    out.push_str(&format!(
        "{ESC}[?25{}",
        if snapshot.cursor_visible { 'h' } else { 'l' }
    ));
    // Clear the screen AND the scrollback (ED 3). `term.resize()` on a grid
    // change pushes the old rows into xterm's scrollback, and ED 2 does not
    // remove them. If the viewport is scrolled up, the incremental repaint
    // then shows stale history above the freshly drawn rows, so the web view
    // diverges from the native screen. ED 3 drops that history and returns
    // the viewport to the bottom on every full repaint.
    out.push_str(&format!("{ESC}[3J"));

    for (row_index, row) in snapshot.cells.iter().enumerate() {
        // Reset SGR before the erase so `EL 2` paints with the terminal's
        // default background, then rewrite the full row. Writing every cell
        // (including spaces) preserves explicit cell backgrounds.
        out.push_str(&format!(
            "{ESC}[{};1H{ESC}[0m{ESC}[2K",
            row_index + 1
        ));
        let mut current_fg = None;
        let mut current_bg = None;
        let mut col = 0;
        while col < row.len() {
            let cell = &row[col];
            // alacritty emits a WIDE_CHAR cell followed by a spacer cell.
            // xterm.js advances two columns for the wide glyph itself.
            if cell.spans_next_col && col + 1 < row.len() {
                col += 1;
            }
            emit_cell_style(&mut out, cell, &mut current_fg, &mut current_bg);
            out.push(cell.ch);
            col += 1;
        }
    }

    out.push_str(&format!(
        "{ESC}[{};{}H",
        snapshot.cursor_row + 1,
        snapshot.cursor_col + 1
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(cells: Vec<Vec<CellSnapshot>>) -> ScreenSnapshot {
        ScreenSnapshot {
            cells,
            soft_wrapped_rows: vec![],
            cursor_row: 1,
            cursor_col: 2,
            cursor_visible: true,
            alt_screen: false,
        }
    }

    #[test]
    fn encodes_prefix_cursor_and_row_positioning() {
        let out = encode_screen(&snapshot(vec![
            vec![CellSnapshot::default()],
            vec![CellSnapshot::default()],
        ]));
        assert!(out.starts_with('0'));
        assert!(out.contains("\x1b[?25h"));
        assert!(out.contains("\x1b[3J"));
        assert!(out.contains("\x1b[1;1H\x1b[0m\x1b[2K"));
        assert!(out.contains("\x1b[2;1H\x1b[0m\x1b[2K"));
        assert!(out.ends_with("\x1b[2;3H"));
    }

    #[test]
    fn encodes_alt_screen_and_hidden_cursor() {
        let mut snap = snapshot(vec![vec![CellSnapshot::default()]]);
        snap.alt_screen = true;
        snap.cursor_visible = false;
        let out = encode_screen(&snap);
        assert!(out.starts_with('1'));
        assert!(out.contains("\x1b[?25l"));
    }

    #[test]
    fn emits_truecolor_sgr_for_cell_colors() {
        let cell = CellSnapshot {
            ch: 'A',
            fg: gpui::rgb(0x112233).into(),
            bg: Some(gpui::rgb(0x445566).into()),
            ..Default::default()
        };
        let out = encode_screen(&snapshot(vec![vec![cell]]));
        assert!(out.contains("\x1b[38;2;17;34;51m"), "output: {out:?}");
        assert!(out.contains("\x1b[48;2;68;85;102m"), "output: {out:?}");
    }

    #[test]
    fn skips_wide_char_spacer_cell() {
        let wide = CellSnapshot {
            ch: '你',
            spans_next_col: true,
            ..Default::default()
        };
        let spacer = CellSnapshot::default();
        let out = encode_screen(&snapshot(vec![vec![wide, spacer]]));
        assert!(out.contains('你'));
        // The spacer is skipped; only one glyph is written for the pair.
        assert_eq!(out.matches('你').count(), 1);
    }
}
