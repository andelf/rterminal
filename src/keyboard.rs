pub(crate) fn encode_keystroke(keystroke: &gpui::Keystroke) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let alt = keystroke.modifiers.alt;

    let mut bytes = match key {
        "space" => vec![b' '],
        "enter" => vec![b'\r'],
        "tab" => vec![b'\t'],
        "backspace" => vec![0x7f],
        "escape" => vec![0x1b],
        "left" => b"\x1b[D".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        _ => encode_printable_keystroke(keystroke)?,
    };

    if alt {
        bytes.insert(0, 0x1b);
    }

    Some(bytes)
}

fn encode_printable_keystroke(keystroke: &gpui::Keystroke) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let ctrl = keystroke.modifiers.control;

    // Reserve platform/function chords for app-level shortcuts such as paste.
    if keystroke.modifiers.platform || keystroke.modifiers.function {
        return None;
    }

    // IME composition in progress (e.g. pinyin typing) should not leak intermediate ASCII
    // keystrokes into PTY before key_char is committed.
    if keystroke.is_ime_in_progress() {
        return None;
    }

    if ctrl {
        if key.len() == 1 {
            let mut ch = key.as_bytes()[0];
            ch = ch.to_ascii_lowercase() & 0x1f;
            return Some(vec![ch]);
        }
        return None;
    }

    if let Some(key_char) = keystroke
        .key_char
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        return Some(key_char.as_bytes().to_vec());
    }

    if key.chars().count() == 1 {
        return Some(key.as_bytes().to_vec());
    }

    None
}

pub(crate) fn should_defer_to_text_input(keystroke: &gpui::Keystroke) -> bool {
    // On macOS, route printable text keys through NSTextInputClient callbacks
    // (insertText / setMarkedText) to preserve IME and accessibility behavior.
    if !cfg!(target_os = "macos") {
        return false;
    }

    let modifiers = keystroke.modifiers;
    if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
        return false;
    }

    if is_terminal_control_key_name(keystroke.key.as_str()) {
        return false;
    }

    keystroke
        .key_char
        .as_ref()
        .is_some_and(|ch| !ch.is_empty() && !ch.chars().any(|c| c.is_control()))
}

fn is_terminal_control_key_name(key: &str) -> bool {
    matches!(
        key,
        "enter"
            | "tab"
            | "backspace"
            | "escape"
            | "left"
            | "right"
            | "up"
            | "down"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "delete"
            | "insert"
    )
}

pub(crate) fn is_paste_shortcut(keystroke: &gpui::Keystroke) -> bool {
    let is_v = keystroke.key.eq_ignore_ascii_case("v");
    let modifiers = keystroke.modifiers;
    let mac_paste = modifiers.platform && !modifiers.control && is_v;
    let ctrl_shift_paste = modifiers.control && modifiers.shift && is_v;
    mac_paste || ctrl_shift_paste
}

pub(crate) fn is_select_all_shortcut(keystroke: &gpui::Keystroke) -> bool {
    let is_a = keystroke.key.eq_ignore_ascii_case("a");
    let modifiers = keystroke.modifiers;
    cfg!(target_os = "macos")
        && modifiers.platform
        && !modifiers.control
        && !modifiers.alt
        && !modifiers.shift
        && !modifiers.function
        && is_a
}

pub(crate) fn is_zoom_in_shortcut(keystroke: &gpui::Keystroke) -> bool {
    if !is_platform_shortcut(keystroke.modifiers) {
        return false;
    }

    if key_matches(&keystroke.key, &["=", "equal", "plus", "+"]) {
        return true;
    }

    keystroke
        .key_char
        .as_ref()
        .is_some_and(|ch| ch == "=" || ch == "+")
}

pub(crate) fn is_zoom_out_shortcut(keystroke: &gpui::Keystroke) -> bool {
    if !is_platform_shortcut(keystroke.modifiers) {
        return false;
    }

    if key_matches(&keystroke.key, &["-", "minus", "_"]) {
        return true;
    }

    keystroke
        .key_char
        .as_ref()
        .is_some_and(|ch| ch == "-" || ch == "_")
}

fn is_platform_shortcut(modifiers: gpui::Modifiers) -> bool {
    modifiers.platform && !modifiers.control && !modifiers.alt && !modifiers.function
}

fn key_matches(key: &str, accepted: &[&str]) -> bool {
    accepted
        .iter()
        .any(|candidate| key.eq_ignore_ascii_case(candidate))
}
