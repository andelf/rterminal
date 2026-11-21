//! Static assets for the built-in xterm.js web client.
//!
//! Everything is embedded into the binary with `include_str!`, so the web UI
//! needs no extra files next to the executable and works from any cwd.

use crate::api_protocol::RouteOutcome;

const INDEX_HTML: &str = include_str!("../web/index.html");
const STYLE_CSS: &str = include_str!("../web/style.css");
const APP_JS: &str = include_str!("../web/app.js");
const XTERM_JS: &str = include_str!("../web/vendor/xterm.min.js");
const XTERM_CSS: &str = include_str!("../web/vendor/xterm.min.css");

pub(crate) fn static_response(path: &str) -> Option<RouteOutcome> {
    let (content_type, body) = match path {
        "/web" | "/web/" | "/web/index.html" => ("text/html; charset=utf-8", INDEX_HTML),
        "/web/style.css" => ("text/css; charset=utf-8", STYLE_CSS),
        "/web/app.js" => ("text/javascript; charset=utf-8", APP_JS),
        "/web/vendor/xterm.min.js" => ("text/javascript; charset=utf-8", XTERM_JS),
        "/web/vendor/xterm.min.css" => ("text/css; charset=utf-8", XTERM_CSS),
        _ => return None,
    };
    Some(RouteOutcome::Immediate {
        status: 200,
        content_type,
        body: body.to_string(),
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
    fn embedded_xterm_umd_exports_terminal() {
        assert!(XTERM_JS.contains("Terminal"));
    }
}
