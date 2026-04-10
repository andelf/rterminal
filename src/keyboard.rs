pub(crate) fn encode_keystroke(keystroke: &gpui::Keystroke) -> Option<Vec<u8>> {
    // Reserve platform/function chords for app-level shortcuts such as paste.
    if keystroke.modifiers.platform || keystroke.modifiers.function {
        return None;
    }

    if let Some(bytes) = encode_special_keystroke(keystroke) {
        return Some(bytes);
    }

    encode_printable_keystroke(keystroke)
}

fn encode_special_keystroke(keystroke: &gpui::Keystroke) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let modifiers = keystroke.modifiers;
    let has_modifiers = modifiers.shift || modifiers.alt || modifiers.control;

    if !has_modifiers {
        return match key {
            "space" => Some(vec![b' ']),
            "enter" => Some(vec![b'\r']),
            "tab" => Some(vec![b'\t']),
            "backspace" => Some(vec![0x7f]),
            "escape" => Some(vec![0x1b]),
            "left" => Some(b"\x1b[D".to_vec()),
            "right" => Some(b"\x1b[C".to_vec()),
            "up" => Some(b"\x1b[A".to_vec()),
            "down" => Some(b"\x1b[B".to_vec()),
            "home" => Some(b"\x1b[H".to_vec()),
            "end" => Some(b"\x1b[F".to_vec()),
            "insert" => Some(b"\x1b[2~".to_vec()),
            "delete" => Some(b"\x1b[3~".to_vec()),
            "pageup" => Some(b"\x1b[5~".to_vec()),
            "pagedown" => Some(b"\x1b[6~".to_vec()),
            "f1" => Some(b"\x1bOP".to_vec()),
            "f2" => Some(b"\x1bOQ".to_vec()),
            "f3" => Some(b"\x1bOR".to_vec()),
            "f4" => Some(b"\x1bOS".to_vec()),
            "f5" => Some(b"\x1b[15~".to_vec()),
            "f6" => Some(b"\x1b[17~".to_vec()),
            "f7" => Some(b"\x1b[18~".to_vec()),
            "f8" => Some(b"\x1b[19~".to_vec()),
            "f9" => Some(b"\x1b[20~".to_vec()),
            "f10" => Some(b"\x1b[21~".to_vec()),
            "f11" => Some(b"\x1b[23~".to_vec()),
            "f12" => Some(b"\x1b[24~".to_vec()),
            _ => None,
        };
    }

    match (key, modifiers.shift, modifiers.alt, modifiers.control) {
        ("tab", true, false, false) => return Some(b"\x1b[Z".to_vec()),
        ("enter", true, false, false) => return Some(vec![b'\n']),
        ("enter", false, true, false) => return Some(vec![0x1b, b'\r']),
        ("backspace", false, false, true) => return Some(vec![0x08]),
        ("backspace", false, true, false) => return Some(vec![0x1b, 0x7f]),
        ("space", false, false, true) => return Some(vec![0x00]),
        _ => {}
    }

    if let Some(bytes) = encode_modified_special_key(key, modifier_code(keystroke)) {
        return Some(bytes);
    }

    // Alt-modified printable keys are sent as ESC + key bytes.
    if modifiers.alt && !modifiers.control {
        if let Some(key_char) = keystroke
            .key_char
            .as_ref()
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        {
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(key_char.as_bytes());
            return Some(bytes);
        }

        if key.chars().count() == 1 {
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(key.as_bytes());
            return Some(bytes);
        }
    }

    None
}

fn encode_modified_special_key(key: &str, modifier_code: u8) -> Option<Vec<u8>> {
    let seq = match key {
        "up" => format!("\x1b[1;{modifier_code}A"),
        "down" => format!("\x1b[1;{modifier_code}B"),
        "right" => format!("\x1b[1;{modifier_code}C"),
        "left" => format!("\x1b[1;{modifier_code}D"),
        "home" => format!("\x1b[1;{modifier_code}H"),
        "end" => format!("\x1b[1;{modifier_code}F"),
        "insert" => format!("\x1b[2;{modifier_code}~"),
        "delete" => format!("\x1b[3;{modifier_code}~"),
        "pageup" => format!("\x1b[5;{modifier_code}~"),
        "pagedown" => format!("\x1b[6;{modifier_code}~"),
        "f1" => format!("\x1b[1;{modifier_code}P"),
        "f2" => format!("\x1b[1;{modifier_code}Q"),
        "f3" => format!("\x1b[1;{modifier_code}R"),
        "f4" => format!("\x1b[1;{modifier_code}S"),
        "f5" => format!("\x1b[15;{modifier_code}~"),
        "f6" => format!("\x1b[17;{modifier_code}~"),
        "f7" => format!("\x1b[18;{modifier_code}~"),
        "f8" => format!("\x1b[19;{modifier_code}~"),
        "f9" => format!("\x1b[20;{modifier_code}~"),
        "f10" => format!("\x1b[21;{modifier_code}~"),
        "f11" => format!("\x1b[23;{modifier_code}~"),
        "f12" => format!("\x1b[24;{modifier_code}~"),
        _ => return None,
    };

    Some(seq.into_bytes())
}

fn modifier_code(keystroke: &gpui::Keystroke) -> u8 {
    let mut code = 0u8;
    if keystroke.modifiers.shift {
        code |= 1;
    }
    if keystroke.modifiers.alt {
        code |= 1 << 1;
    }
    if keystroke.modifiers.control {
        code |= 1 << 2;
    }
    code + 1
}

fn encode_printable_keystroke(keystroke: &gpui::Keystroke) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let ctrl = keystroke.modifiers.control;

    // IME composition in progress (e.g. pinyin typing) should not leak intermediate ASCII
    // keystrokes into PTY before key_char is committed.
    if keystroke.is_ime_in_progress() {
        return None;
    }

    if ctrl {
        if key.len() == 1 && key.is_ascii() {
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

pub(crate) fn should_defer_to_text_input(
    keystroke: &gpui::Keystroke,
    option_as_meta: bool,
) -> bool {
    // On macOS, route printable text keys through NSTextInputClient callbacks
    // (insertText / setMarkedText) to preserve IME and accessibility behavior.
    if !cfg!(target_os = "macos") {
        return false;
    }

    let modifiers = keystroke.modifiers;
    if modifiers.control || modifiers.platform || modifiers.function {
        return false;
    }
    // When option_as_meta is true, Alt+key is handled by encode_keystroke as
    // ESC+key.  When false, let macOS text input produce native characters
    // (e.g. Option+D → ∂).
    if modifiers.alt && option_as_meta {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_shift_tab() {
        let ks = gpui::Keystroke::parse("shift-tab").expect("parse shift-tab");
        assert_eq!(encode_keystroke(&ks), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn encodes_ctrl_left() {
        let ks = gpui::Keystroke::parse("ctrl-left").expect("parse ctrl-left");
        assert_eq!(encode_keystroke(&ks), Some(b"\x1b[1;5D".to_vec()));
    }

    #[test]
    fn encodes_alt_printable_with_escape_prefix() {
        let ks = gpui::Keystroke::parse("alt-x").expect("parse alt-x");
        assert_eq!(encode_keystroke(&ks), Some(vec![0x1b, b'x']));
    }
}
