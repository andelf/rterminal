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
    // Fresh terminals can inherit stale AX value from a previously focused tab/window.
    // When the local model and publish baseline are both empty, prefer clearing AX state
    // instead of importing unrelated text.
    let has_local_publish_baseline =
        !last_published_text.is_empty() || last_published_cursor_utf16 > 0;
    if !has_local_publish_baseline && model_text.is_empty() && model_cursor_utf16 == 0 {
        return false;
    }

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

fn byte_index_to_utf16(text: &str, byte_index: usize) -> usize {
    if byte_index == 0 {
        return 0;
    }

    let mut utf16 = 0usize;
    for (idx, ch) in text.char_indices() {
        if idx >= byte_index {
            break;
        }
        utf16 += ch.len_utf16();
    }
    utf16
}

pub(crate) fn delete_previous_word_utf16(text: &mut String, cursor_utf16: &mut usize) {
    if *cursor_utf16 == 0 || text.is_empty() {
        return;
    }

    let cursor_byte = utf16_to_byte_index(text, *cursor_utf16);
    if cursor_byte == 0 {
        *cursor_utf16 = 0;
        return;
    }

    let mut start = cursor_byte;
    while start > 0 {
        let Some((prev, ch)) = text[..start].char_indices().last() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        start = prev;
    }

    while start > 0 {
        let Some((prev, ch)) = text[..start].char_indices().last() else {
            break;
        };
        if ch.is_whitespace() {
            break;
        }
        start = prev;
    }

    text.replace_range(start..cursor_byte, "");
    *cursor_utf16 = byte_index_to_utf16(text, start);
}

pub(crate) fn delete_next_word_utf16(text: &mut String, cursor_utf16: &mut usize) {
    if text.is_empty() {
        *cursor_utf16 = 0;
        return;
    }

    let cursor_byte = utf16_to_byte_index(text, *cursor_utf16);
    if cursor_byte >= text.len() {
        *cursor_utf16 = text.encode_utf16().count();
        return;
    }

    let mut end = cursor_byte;
    while end < text.len() {
        let Some(ch) = text[end..].chars().next() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        end += ch.len_utf8();
    }

    while end < text.len() {
        let Some(ch) = text[end..].chars().next() else {
            break;
        };
        if ch.is_whitespace() {
            break;
        }
        end += ch.len_utf8();
    }

    text.replace_range(cursor_byte..end, "");
    *cursor_utf16 = (*cursor_utf16).min(text.encode_utf16().count());
}

pub(crate) fn delete_to_end_utf16(text: &mut String, cursor_utf16: usize) {
    let cursor_byte = utf16_to_byte_index(text, cursor_utf16);
    text.truncate(cursor_byte);
}

#[cfg(test)]
mod tests {
    use super::should_accept_ax_override;

    #[test]
    fn rejects_stale_ax_state_for_fresh_terminal_model() {
        assert!(!should_accept_ax_override("PNPN", 4, "", 0, "", 0,));
    }

    #[test]
    fn accepts_ax_override_after_local_baseline_exists() {
        assert!(should_accept_ax_override("abcd", 4, "abc", 3, "abc", 3,));
    }
}
