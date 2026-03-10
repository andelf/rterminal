use std::ops::Range;

pub(crate) fn utf16_to_byte_index(text: &str, utf16_index: usize) -> usize {
    if utf16_index == 0 {
        return 0;
    }

    let mut offset = 0usize;
    for (byte_idx, ch) in text.char_indices() {
        let next_offset = offset + ch.len_utf16();
        if next_offset > utf16_index {
            return byte_idx;
        }
        if next_offset == utf16_index {
            return byte_idx + ch.len_utf8();
        }
        offset = next_offset;
    }
    text.len()
}

pub(crate) fn utf16_substring(text: &str, range: Range<usize>) -> Option<String> {
    let start = utf16_to_byte_index(text, range.start);
    let end = utf16_to_byte_index(text, range.end);
    text.get(start..end).map(ToString::to_string)
}

pub(crate) fn replace_range_utf16(text: &mut String, range: Range<usize>, replacement: &str) {
    let start = utf16_to_byte_index(text, range.start);
    let end = utf16_to_byte_index(text, range.end);
    text.replace_range(start..end, replacement);
}

pub(crate) fn should_accept_ax_override(
    ax_text: &str,
    ax_cursor_utf16: usize,
    model_text: &str,
    model_cursor_utf16: usize,
    last_published_text: &str,
    last_published_cursor_utf16: usize,
) -> bool {
    let differs_from_model = ax_text != model_text || ax_cursor_utf16 != model_cursor_utf16;
    let is_stale_last_publish =
        ax_text == last_published_text && ax_cursor_utf16 == last_published_cursor_utf16;
    differs_from_model && !is_stale_last_publish
}

pub(crate) fn should_publish_model_to_ax(
    ax_text: &str,
    ax_cursor_utf16: usize,
    model_text: &str,
    model_cursor_utf16: usize,
    accepting_ax_override: bool,
) -> bool {
    !accepting_ax_override && (ax_text != model_text || ax_cursor_utf16 != model_cursor_utf16)
}

pub(crate) fn summarize_text_for_trace(text: &str) -> String {
    const MAX_PREVIEW_CHARS: usize = 24;
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= MAX_PREVIEW_CHARS {
            out.push_str("…");
            break;
        }
        out.push(ch);
    }
    format!("{out:?}")
}
