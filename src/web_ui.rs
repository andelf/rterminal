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
    pub(crate) const APP_JS: &str = "";
    pub(crate) const XTERM_JS: &str = "";
    pub(crate) const XTERM_CSS: &str = "";
}

#[cfg(not(debug_assertions))]
mod embedded {
    pub(crate) const INDEX_HTML: &str = include_str!("../web/index.html");
    pub(crate) const STYLE_CSS: &str = include_str!("../web/style.css");
    pub(crate) const APP_JS: &str = include_str!("../web/app.js");
    pub(crate) const XTERM_JS: &str = include_str!("../web/vendor/xterm.min.js");
    pub(crate) const XTERM_CSS: &str = include_str!("../web/vendor/xterm.min.css");
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

fn load(rel: &str, embedded: &str) -> String {
    if let Some(dir) = find_web_dir() {
        let path = dir.join(rel);
        if let Ok(bytes) = fs::read(&path) {
            return String::from_utf8_lossy(&bytes).into_owned();
        }
    }
    embedded.to_string()
}

pub(crate) fn static_response(path: &str) -> Option<RouteOutcome> {
    let (content_type, body) = match path {
        "/web" | "/web/" | "/web/index.html" => (
            "text/html; charset=utf-8",
            load("index.html", embedded::INDEX_HTML),
        ),
        "/web/style.css" => (
            "text/css; charset=utf-8",
            load("style.css", embedded::STYLE_CSS),
        ),
        "/web/app.js" => (
            "text/javascript; charset=utf-8",
            load("app.js", embedded::APP_JS),
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
            "/web/app.js",
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
