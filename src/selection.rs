use crate::terminal::{CellSnapshot, ScreenSnapshot, SelectionPoint};

pub(crate) fn normalize_selection_bounds(
    start: SelectionPoint,
    end: SelectionPoint,
) -> (SelectionPoint, SelectionPoint) {
    if (start.row, start.col) <= (end.row, end.col) {
        (start, end)
    } else {
        (end, start)
    }
}

pub(crate) fn selection_contains_cell(
    start: SelectionPoint,
    end: SelectionPoint,
    row: usize,
    col: usize,
) -> bool {
    if row < start.row || row > end.row {
        return false;
    }

    if start.row == end.row {
        return row == start.row && col >= start.col && col <= end.col;
    }

    if row == start.row {
        return col >= start.col;
    }

    if row == end.row {
        return col <= end.col;
    }

    true
}

pub(crate) fn extract_selection_text(
    snapshot: &ScreenSnapshot,
    start: SelectionPoint,
    end: SelectionPoint,
) -> String {
    let mut lines = Vec::new();

    for row in start.row..=end.row {
        let Some(cells) = snapshot.cells.get(row) else {
            break;
        };
        if cells.is_empty() {
            lines.push((row, String::new()));
            continue;
        }

        let line_start = if row == start.row {
            normalize_selection_col(cells, start.col)
        } else {
            0
        };
        let line_end = if row == end.row {
            normalize_selection_col(cells, end.col)
        } else {
            cells.len().saturating_sub(1)
        };
        if line_start >= cells.len() {
            lines.push((row, String::new()));
            continue;
        }

        let clamped_end = line_end.min(cells.len().saturating_sub(1));
        if line_start > clamped_end {
            lines.push((row, String::new()));
            continue;
        }

        let mut text = String::new();
        let mut col = line_start;
        while col <= clamped_end {
            let cell = &cells[col];
            text.push(cell.ch);
            let step = cell_advance_cols(cell);
            col = col.saturating_add(step);
        }
        let trimmed_len = text.trim_end().len();
        text.truncate(trimmed_len);
        lines.push((row, text));
    }

    join_selected_lines(lines, &snapshot.soft_wrapped_rows)
}

pub(crate) fn join_selected_lines(lines: Vec<(usize, String)>, soft_wrapped_rows: &[bool]) -> String {
    let mut text = String::new();
    for (index, (row, line)) in lines.iter().enumerate() {
        text.push_str(line);
        if index + 1 < lines.len() && !soft_wrapped_rows.get(*row).copied().unwrap_or(false) {
            text.push('\n');
        }
    }
    text
}

pub(crate) fn normalize_selection_col(cells: &[CellSnapshot], col: usize) -> usize {
    if cells.is_empty() {
        return 0;
    }

    let mut normalized = col.min(cells.len().saturating_sub(1));
    while normalized > 0 {
        let prev = normalized - 1;
        let prev_span = cell_advance_cols(&cells[prev]);
        if prev_span > 1 && prev.saturating_add(prev_span) > normalized {
            normalized = prev;
            continue;
        }
        break;
    }

    normalized
}

pub(crate) fn row_text_without_wide_spacers(cells: &[CellSnapshot]) -> String {
    let mut text = String::new();
    let mut col = 0usize;
    while col < cells.len() {
        let cell = &cells[col];
        text.push(cell.ch);
        col = col.saturating_add(cell_advance_cols(cell));
    }
    text
}

pub(crate) fn cell_advance_cols(cell: &CellSnapshot) -> usize {
    if cell.spans_next_col {
        usize::from(cell.width_cols.max(1))
    } else {
        1
    }
}
