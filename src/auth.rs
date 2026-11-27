//! Shared-secret authentication for the web/API surface.
//!
//! The token is supplied at startup with `--web-token` or
//! `RTERMINAL_WEB_TOKEN`. It is never stored in the repository. Users may
//! choose any printable token; only control characters are rejected because
//! they cannot be represented safely in HTTP headers.

use std::sync::OnceLock;

const COOKIE_NAME: &str = "rterminal_token";
static TOKEN: OnceLock<String> = OnceLock::new();

pub(crate) fn configure(token: String) -> Result<(), String> {
    if token.is_empty() || token.chars().any(char::is_control) {
        return Err("web token must be non-empty and contain no control characters".to_string());
    }
    TOKEN
        .set(token)
        .map_err(|_| "web token is already configured".to_string())
}

fn token() -> &'static str {
    match TOKEN.get() {
        Some(token) => token.as_str(),
        None => "",
    }
}

pub(crate) fn token_matches(candidate: &str) -> bool {
    constant_time_eq(candidate.as_bytes(), token().as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || b.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn percent_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            out.push(*byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out
}

fn percent_decode(value: &str) -> Option<String> {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let hi = hex(bytes[index + 1])?;
            let lo = hex(bytes[index + 2])?;
            out.push((hi << 4) | lo);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn cookie_token(value: &str) -> Option<&str> {
    value.split(';').find_map(|pair| {
        let (name, token) = pair.trim().split_once('=')?;
        name.eq_ignore_ascii_case(COOKIE_NAME).then_some(token)
    })
}

pub(crate) fn authorized_by<'a, I>(headers: I) -> bool
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut authorized = false;
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("cookie")
            && let Some(encoded) = cookie_token(value)
            && let Some(candidate) = percent_decode(encoded)
        {
            authorized |= token_matches(&candidate);
        } else if name.eq_ignore_ascii_case("x-rterminal-token") {
            authorized |= token_matches(value);
        } else if name.eq_ignore_ascii_case("authorization")
            && let Some(candidate) = value.strip_prefix("Bearer ")
        {
            authorized |= token_matches(candidate);
        }
    }
    authorized
}

pub(crate) fn auth_cookie_header() -> String {
    format!(
        "{COOKIE_NAME}={}; Path=/; HttpOnly; SameSite=Lax",
        percent_encode(token())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TOKEN: &str = "test-token";

    fn configure_test_token() {
        let _ = configure(TEST_TOKEN.to_string());
    }

    #[test]
    fn accepts_cookie_header_and_bearer() {
        configure_test_token();
        assert!(authorized_by([(
            "cookie",
            "other=1; rterminal_token=test-token"
        )]));
        assert!(authorized_by([("x-rterminal-token", TEST_TOKEN)]));
        assert!(authorized_by([("authorization", "Bearer test-token")]));
    }

    #[test]
    fn rejects_missing_or_wrong_token() {
        configure_test_token();
        assert!(!authorized_by::<[(&str, &str); 0]>([]));
        assert!(!authorized_by([("cookie", "rterminal_token=wrong")]));
        assert!(!authorized_by([("x-rterminal-token", "test-toke")]));
    }

    #[test]
    fn percent_encoding_round_trips_free_form_tokens() {
        let token = "token with spaces!%/&=;中文";
        let encoded = percent_encode(token);
        assert!(!encoded.contains(' '));
        assert!(!encoded.contains('%') || encoded.contains("%25"));
        assert_eq!(percent_decode(&encoded).as_deref(), Some(token));
    }

    #[test]
    fn configure_rejects_control_characters() {
        assert!(configure("line\nbreak".to_string()).is_err());
        assert!(configure(String::new()).is_err());
    }
}
