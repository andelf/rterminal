//! Static assets for the built-in xterm.js web client.
//!
//! Debug builds read the files from `web/` at runtime, so editing JS/CSS only
//! needs a browser refresh. Release builds keep the previous behaviour: the
//! assets are embedded into the binary with `include_str!` and need no extra
//! files next to the executable.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::api_protocol::RouteOutcome;

#[cfg(debug_assertions)]
mod embedded {
    pub(crate) const INDEX_HTML: &str = "";
    pub(crate) const STYLE_CSS: &str = "";
    pub(crate) const CORE_JS: &str = "";
    pub(crate) const DEBUG_JS: &str = "";
    pub(crate) const VIEWPORT_JS: &str = "";
    pub(crate) const INPUT_JS: &str = "";
    pub(crate) const KEYBOARD_JS: &str = "";
    pub(crate) const CLIENT_JS: &str = "";
    pub(crate) const APP_JS: &str = "";
    pub(crate) const XTERM_JS: &str = "";
    pub(crate) const XTERM_CSS: &str = "";
    pub(crate) const MANIFEST: &str = "";
    pub(crate) const ICON_SVG: &[u8] = b"";
    pub(crate) const ICON_192: &[u8] = b"";
    pub(crate) const ICON_512: &[u8] = b"";
    pub(crate) const APPLE_TOUCH_ICON: &[u8] = b"";
}

#[cfg(not(debug_assertions))]
mod embedded {
    pub(crate) const INDEX_HTML: &str = include_str!("../web/index.html");
    pub(crate) const STYLE_CSS: &str = include_str!("../web/style.css");
    pub(crate) const CORE_JS: &str = include_str!("../web/core.js");
    pub(crate) const DEBUG_JS: &str = include_str!("../web/debug.js");
    pub(crate) const VIEWPORT_JS: &str = include_str!("../web/viewport.js");
    pub(crate) const INPUT_JS: &str = include_str!("../web/input.js");
    pub(crate) const KEYBOARD_JS: &str = include_str!("../web/keyboard.js");
    pub(crate) const CLIENT_JS: &str = include_str!("../web/client.js");
    pub(crate) const APP_JS: &str = include_str!("../web/app.js");
    pub(crate) const XTERM_JS: &str = include_str!("../web/vendor/xterm.min.js");
    pub(crate) const XTERM_CSS: &str = include_str!("../web/vendor/xterm.min.css");
    pub(crate) const MANIFEST: &str = include_str!("../web/manifest.webmanifest");
    pub(crate) const ICON_SVG: &[u8] = include_bytes!("../web/icons/icon.svg");
    pub(crate) const ICON_192: &[u8] = include_bytes!("../web/icons/icon-192.png");
    pub(crate) const ICON_512: &[u8] = include_bytes!("../web/icons/icon-512.png");
    pub(crate) const APPLE_TOUCH_ICON: &[u8] = include_bytes!("../web/icons/apple-touch-icon.png");
}

/// Locate the source `web/` directory. The executable is usually in
/// `target/debug/`, so walk up a few parents before falling back to cwd.
fn find_web_dir() -> Option<PathBuf> {
    if let Ok(dir) = env::var("RTERMINAL_WEB_DIR") {
        let path = PathBuf::from(dir);
        if path.is_dir() {
            return Some(path);
        }
    }

    if let Ok(exe) = env::current_exe() {
        let mut base: Option<&Path> = exe.parent();
        for _ in 0..4 {
            let Some(dir) = base else { break };
            let candidate = dir.join("web");
            if candidate.is_dir() {
                return Some(candidate);
            }
            base = dir.parent();
        }
    }

    let from_cwd = PathBuf::from("web");
    if from_cwd.is_dir() {
        Some(from_cwd)
    } else {
        None
    }
}

fn load_bytes(rel: &str, embedded: &[u8]) -> Vec<u8> {
    if let Some(dir) = find_web_dir() {
        let path = dir.join(rel);
        if let Ok(bytes) = fs::read(path) {
            return bytes;
        }
    }
    embedded.to_vec()
}

fn load(rel: &str, embedded: &str) -> String {
    if let Some(dir) = find_web_dir() {
        let path = dir.join(rel);
        if let Ok(bytes) = fs::read(&path) {
            return String::from_utf8_lossy(&bytes).into_owned();
        }
    }
    embedded.to_string()
}

fn binary(content_type: &'static str, body: Vec<u8>) -> RouteOutcome {
    RouteOutcome::ImmediateBinary {
        status: 200,
        content_type,
        body,
    }
}

pub(crate) fn is_public_path(path: &str) -> bool {
    matches!(
        path,
        "/favicon.svg"
            | "/web"
            | "/web/"
            | "/web/index.html"
            | "/web/style.css"
            | "/web/core.js"
            | "/web/debug.js"
            | "/web/viewport.js"
            | "/web/input.js"
            | "/web/keyboard.js"
            | "/web/client.js"
            | "/web/app.js"
            | "/web/manifest.webmanifest"
            | "/web/vendor/xterm.min.js"
            | "/web/vendor/xterm.min.css"
            | "/web/icons/icon.svg"
            | "/web/icons/icon-192.png"
            | "/web/icons/icon-512.png"
            | "/web/icons/apple-touch-icon.png"
    )
}

pub(crate) fn static_response(path: &str) -> Option<RouteOutcome> {
    match path {
        "/favicon.svg" | "/web/icons/icon.svg" => {
            return Some(binary(
                "image/svg+xml",
                load_bytes("icons/icon.svg", embedded::ICON_SVG),
            ));
        }
        "/web/icons/icon-192.png" => {
            return Some(binary(
                "image/png",
                load_bytes("icons/icon-192.png", embedded::ICON_192),
            ));
        }
        "/web/icons/icon-512.png" => {
            return Some(binary(
                "image/png",
                load_bytes("icons/icon-512.png", embedded::ICON_512),
            ));
        }
        "/web/icons/apple-touch-icon.png" => {
            return Some(binary(
                "image/png",
                load_bytes("icons/apple-touch-icon.png", embedded::APPLE_TOUCH_ICON),
            ));
        }
        _ => {}
    }

    let (content_type, body) = match path {
        "/web" | "/web/" | "/web/index.html" => (
            "text/html; charset=utf-8",
            load("index.html", embedded::INDEX_HTML),
        ),
        "/web/style.css" => (
            "text/css; charset=utf-8",
            load("style.css", embedded::STYLE_CSS),
        ),
        "/web/core.js" => (
            "text/javascript; charset=utf-8",
            load("core.js", embedded::CORE_JS),
        ),
        "/web/debug.js" => (
            "text/javascript; charset=utf-8",
            load("debug.js", embedded::DEBUG_JS),
        ),
        "/web/viewport.js" => (
            "text/javascript; charset=utf-8",
            load("viewport.js", embedded::VIEWPORT_JS),
        ),
        "/web/input.js" => (
            "text/javascript; charset=utf-8",
            load("input.js", embedded::INPUT_JS),
        ),
        "/web/keyboard.js" => (
            "text/javascript; charset=utf-8",
            load("keyboard.js", embedded::KEYBOARD_JS),
        ),
        "/web/client.js" => (
            "text/javascript; charset=utf-8",
            load("client.js", embedded::CLIENT_JS),
        ),
        "/web/app.js" => (
            "text/javascript; charset=utf-8",
            load("app.js", embedded::APP_JS),
        ),
        "/web/manifest.webmanifest" => (
            "application/manifest+json; charset=utf-8",
            load("manifest.webmanifest", embedded::MANIFEST),
        ),
        "/web/vendor/xterm.min.js" => (
            "text/javascript; charset=utf-8",
            load("vendor/xterm.min.js", embedded::XTERM_JS),
        ),
        "/web/vendor/xterm.min.css" => (
            "text/css; charset=utf-8",
            load("vendor/xterm.min.css", embedded::XTERM_CSS),
        ),
        _ => return None,
    };
    Some(RouteOutcome::Immediate {
        status: 200,
        content_type,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_web_root_and_assets() {
        for path in [
            "/web",
            "/web/",
            "/web/index.html",
            "/web/style.css",
            "/web/core.js",
            "/web/debug.js",
            "/web/viewport.js",
            "/web/input.js",
            "/web/keyboard.js",
            "/web/client.js",
            "/web/app.js",
            "/web/manifest.webmanifest",
            "/web/vendor/xterm.min.js",
            "/web/vendor/xterm.min.css",
        ] {
            match static_response(path).expect("known path") {
                RouteOutcome::Immediate { status, body, .. } => {
                    assert_eq!(status, 200);
                    assert!(!body.is_empty());
                }
                other => panic!("unexpected outcome for {path}: {other:?}"),
            }
        }
    }

    #[test]
    fn serves_binary_icons() {
        for (path, signature) in [
            ("/web/icons/icon-192.png", &[0x89, 0x50, 0x4e, 0x47][..]),
            ("/web/icons/icon-512.png", &[0x89, 0x50, 0x4e, 0x47][..]),
            (
                "/web/icons/apple-touch-icon.png",
                &[0x89, 0x50, 0x4e, 0x47][..],
            ),
            ("/web/icons/icon.svg", b"<svg"),
            ("/favicon.svg", b"<svg"),
        ] {
            match static_response(path).expect("known icon") {
                RouteOutcome::ImmediateBinary { status, body, .. } => {
                    assert_eq!(status, 200);
                    assert!(body.starts_with(signature), "unexpected asset at {path}");
                }
                other => panic!("unexpected outcome for {path}: {other:?}"),
            }
        }
    }

    #[test]
    fn only_assets_are_public() {
        assert!(is_public_path("/web/app.js"));
        assert!(is_public_path("/web/icons/icon-192.png"));
        assert!(!is_public_path("/web/config"));
        assert!(!is_public_path("/web/debug"));
        assert!(!is_public_path("/tabs"));
    }

    #[test]
    fn unknown_paths_are_not_static() {
        assert!(static_response("/web/nope").is_none());
        assert!(static_response("/tabs").is_none());
    }

    #[test]
    fn xterm_umd_exports_terminal() {
        match static_response("/web/vendor/xterm.min.js").expect("known path") {
            RouteOutcome::Immediate { body, .. } => assert!(body.contains("Terminal")),
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
}
