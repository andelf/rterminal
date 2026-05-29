//! tmux-style key token parser. Whitespace separates tokens; quoted runs
//! (`"…"`) are literal text. See spec §5.

#[derive(Clone, Copy, Default)]
struct Mods {
    ctrl: bool,
    meta: bool,
    shift: bool,
}

pub(crate) fn parse_keys(body: &str) -> Result<Vec<u8>, String> {
    let mut tokens = Vec::new();
    let mut iter = body.chars().peekable();
    while let Some(&c) = iter.peek() {
        if c.is_whitespace() {
            iter.next();
            continue;
        }
        if c == '"' {
            iter.next();
            let mut literal = String::new();
            loop {
                match iter.next() {
                    Some('\\') => match iter.next() {
                        Some('"') => literal.push('"'),
                        Some('\\') => literal.push('\\'),
                        Some(other) => return Err(format!("invalid escape: \\{other}")),
                        None => return Err("unterminated quoted string".to_string()),
                    },
                    Some('"') => break,
                    Some(ch) => literal.push(ch),
                    None => return Err("unterminated quoted string".to_string()),
                }
            }
            tokens.push(Token::Literal(literal));
            continue;
        }
        let mut token = String::new();
        while let Some(&ch) = iter.peek() {
            if ch.is_whitespace() {
                break;
            }
            token.push(ch);
            iter.next();
        }
        tokens.push(Token::Word(token));
    }

    let mut out = Vec::new();
    for token in tokens {
        match token {
            Token::Literal(text) => out.extend_from_slice(text.as_bytes()),
            Token::Word(word) => emit_word(&word, &mut out)?,
        }
    }
    Ok(out)
}

enum Token {
    Word(String),
    Literal(String),
}

fn emit_word(word: &str, out: &mut Vec<u8>) -> Result<(), String> {
    let (mods, key) = split_modifiers(word)?;
    emit_key(mods, key, word, out)
}

fn split_modifiers(word: &str) -> Result<(Mods, &str), String> {
    let mut mods = Mods::default();
    let mut rest = word;
    loop {
        let Some((prefix, tail)) = rest.split_once('-') else {
            break;
        };
        if tail.is_empty() {
            break;
        }
        match prefix {
            "C" => {
                if mods.ctrl {
                    return Err(format!("duplicate Ctrl modifier in '{word}'"));
                }
                mods.ctrl = true;
            }
            "M" => {
                if mods.meta {
                    return Err(format!("duplicate Meta modifier in '{word}'"));
                }
                mods.meta = true;
            }
            "S" => {
                if mods.shift {
                    return Err(format!("duplicate Shift modifier in '{word}'"));
                }
                mods.shift = true;
            }
            _ => break,
        }
        rest = tail;
    }
    Ok((mods, rest))
}

fn emit_key(mods: Mods, key: &str, original: &str, out: &mut Vec<u8>) -> Result<(), String> {
    let named = named_key_bytes(key);

    if mods.shift {
        match key {
            "Tab" => {
                if mods.meta {
                    out.push(0x1b);
                }
                out.extend_from_slice(b"\x1b[Z");
                return Ok(());
            }
            _ => return Err(format!("Shift modifier not supported with '{key}' in '{original}'")),
        }
    }

    if let Some(bytes) = named {
        if mods.ctrl {
            return Err(format!("Ctrl modifier not supported with named key '{key}' in '{original}'"));
        }
        if mods.meta {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        return Ok(());
    }

    let mut chars = key.chars();
    let ch = chars.next().ok_or_else(|| format!("unknown key token: {original}"))?;
    if chars.next().is_some() {
        return Err(format!("unknown key token: {original}"));
    }

    if mods.ctrl {
        let upper = ch.to_ascii_uppercase() as u32;
        // Ctrl maps 'A'..='_' (and '@') to 0x01..=0x1F
        if !(('A' as u32..=b'_' as u32).contains(&upper) || upper == '@' as u32) {
            return Err(format!("Ctrl modifier not supported with '{ch}' in '{original}'"));
        }
        let ctrl_byte = if upper == '@' as u32 {
            0u8
        } else {
            (upper - '@' as u32) as u8
        };
        if mods.meta {
            out.push(0x1b);
        }
        out.push(ctrl_byte);
        return Ok(());
    }

    if mods.meta {
        out.push(0x1b);
    }
    let mut buf = [0u8; 4];
    let encoded = ch.encode_utf8(&mut buf);
    out.extend_from_slice(encoded.as_bytes());
    Ok(())
}

fn named_key_bytes(name: &str) -> Option<&'static [u8]> {
    match name {
        "Enter" => Some(b"\r"),
        "Tab" => Some(b"\t"),
        "Escape" => Some(b"\x1b"),
        "Space" => Some(b" "),
        "BSpace" => Some(b"\x7f"),
        "Up" => Some(b"\x1b[A"),
        "Down" => Some(b"\x1b[B"),
        "Right" => Some(b"\x1b[C"),
        "Left" => Some(b"\x1b[D"),
        "Home" => Some(b"\x1b[H"),
        "End" => Some(b"\x1b[F"),
        "PageUp" => Some(b"\x1b[5~"),
        "PageDown" => Some(b"\x1b[6~"),
        "F1" => Some(b"\x1bOP"),
        "F2" => Some(b"\x1bOQ"),
        "F3" => Some(b"\x1bOR"),
        "F4" => Some(b"\x1bOS"),
        "F5" => Some(b"\x1b[15~"),
        "F6" => Some(b"\x1b[17~"),
        "F7" => Some(b"\x1b[18~"),
        "F8" => Some(b"\x1b[19~"),
        "F9" => Some(b"\x1b[20~"),
        "F10" => Some(b"\x1b[21~"),
        "F11" => Some(b"\x1b[23~"),
        "F12" => Some(b"\x1b[24~"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_emits_cr() {
        assert_eq!(parse_keys("Enter").unwrap(), b"\r");
    }

    #[test]
    fn ctrl_c_emits_etx() {
        assert_eq!(parse_keys("C-c").unwrap(), b"\x03");
    }

    #[test]
    fn meta_x_emits_esc_prefix() {
        assert_eq!(parse_keys("M-x").unwrap(), b"\x1bx");
    }

    #[test]
    fn ctrl_meta_combined_either_order() {
        let lhs = parse_keys("C-M-x").unwrap();
        let rhs = parse_keys("M-C-x").unwrap();
        assert_eq!(lhs, rhs);
        assert_eq!(lhs, b"\x1b\x18"); // ESC + Ctrl-X (0x18)
    }

    #[test]
    fn ctrl_letter_case_insensitive() {
        assert_eq!(parse_keys("C-a").unwrap(), b"\x01");
        assert_eq!(parse_keys("C-A").unwrap(), b"\x01");
    }

    #[test]
    fn shift_tab_emits_csi_z() {
        assert_eq!(parse_keys("S-Tab").unwrap(), b"\x1b[Z");
    }

    #[test]
    fn shift_with_letter_rejected() {
        assert!(parse_keys("S-a").is_err());
    }

    #[test]
    fn arrows_emit_csi() {
        assert_eq!(parse_keys("Up").unwrap(), b"\x1b[A");
        assert_eq!(parse_keys("Down").unwrap(), b"\x1b[B");
        assert_eq!(parse_keys("Right").unwrap(), b"\x1b[C");
        assert_eq!(parse_keys("Left").unwrap(), b"\x1b[D");
    }

    #[test]
    fn function_keys_use_xterm_sequences() {
        assert_eq!(parse_keys("F1").unwrap(), b"\x1bOP");
        assert_eq!(parse_keys("F5").unwrap(), b"\x1b[15~");
        assert_eq!(parse_keys("F12").unwrap(), b"\x1b[24~");
    }

    #[test]
    fn literal_string_inserted_verbatim() {
        assert_eq!(parse_keys("\"echo hi\"").unwrap(), b"echo hi");
    }

    #[test]
    fn quoted_string_escapes_only_quote_and_backslash() {
        assert_eq!(parse_keys("\"a\\\"b\\\\c\"").unwrap(), b"a\"b\\c");
    }

    #[test]
    fn whitespace_separates_tokens() {
        assert_eq!(parse_keys("C-a \"ls\" Enter").unwrap(), b"\x01ls\r");
    }

    #[test]
    fn single_char_literal_token() {
        assert_eq!(parse_keys("a").unwrap(), b"a");
        assert_eq!(parse_keys("!").unwrap(), b"!");
    }

    #[test]
    fn unknown_token_rejected_atomically() {
        let err = parse_keys("Enter Foo Enter").unwrap_err();
        assert!(err.contains("Foo"), "error should mention bad token: {err}");
    }

    #[test]
    fn unterminated_quote_rejected() {
        assert!(parse_keys("\"oops").is_err());
    }

    #[test]
    fn duplicate_modifier_rejected() {
        assert!(parse_keys("C-C-x").is_err());
    }

    #[test]
    fn page_and_home_end() {
        assert_eq!(parse_keys("Home").unwrap(), b"\x1b[H");
        assert_eq!(parse_keys("End").unwrap(), b"\x1b[F");
        assert_eq!(parse_keys("PageUp").unwrap(), b"\x1b[5~");
        assert_eq!(parse_keys("PageDown").unwrap(), b"\x1b[6~");
    }

    #[test]
    fn empty_body_is_empty_bytes() {
        assert_eq!(parse_keys("").unwrap(), Vec::<u8>::new());
        assert_eq!(parse_keys("   ").unwrap(), Vec::<u8>::new());
    }
}
