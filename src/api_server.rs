#[allow(unused_imports)]
use crate::api_protocol::{ApiCommand, ApiReply, ReplyBody, RouteOutcome, TabSelector, oneshot};
#[allow(unused_imports)]
use async_channel::Receiver;

#[derive(Debug)]
pub(crate) struct RouteError {
    pub(crate) status: u16,
    pub(crate) message: String,
}

pub(crate) fn parse_request(
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<RouteOutcome, RouteError> {
    let (tx, rx) = oneshot();
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    let cmd = match (method, segments.as_slice()) {
        // /tabs management
        ("GET", ["tabs"]) => ApiCommand::ListTabs { reply: tx },
        ("POST", ["tabs"]) => ApiCommand::CreateTab { reply: tx },
        ("DELETE", ["tabs", sel]) => ApiCommand::CloseTab { id: parse_selector(sel)?, reply: tx },
        ("POST", ["tabs", sel, "activate"]) => {
            ApiCommand::ActivateTab { id: parse_selector(sel)?, reply: tx }
        }
        ("GET", ["tabs", sel]) => ApiCommand::GetTab { id: parse_selector(sel)?, reply: tx },
        ("GET", ["tabs", sel, "screen"]) => {
            ApiCommand::GetScreen { id: parse_selector(sel)?, reply: tx }
        }
        ("POST", ["tabs", sel, "input"]) => {
            ApiCommand::WriteInput { id: parse_selector(sel)?, bytes: body, reply: tx }
        }
        ("POST", ["tabs", sel, "keys"]) => ApiCommand::SendKeys {
            id: parse_selector(sel)?,
            body: String::from_utf8(body).map_err(|_| RouteError {
                status: 400,
                message: "keys body must be utf-8".to_string(),
            })?,
            reply: tx,
        },

        // legacy /debug aliases
        ("GET", ["debug"]) => {
            return Ok(RouteOutcome::Immediate {
                status: 200,
                content_type: "text/plain; charset=utf-8",
                body: legacy_help_text(),
            });
        }
        ("GET", ["debug", "state"]) => ApiCommand::GetTab { id: TabSelector::Active, reply: tx },
        ("GET", ["debug", "screen"]) => {
            ApiCommand::GetScreen { id: TabSelector::Active, reply: tx }
        }
        ("POST", ["debug", "input"]) => ApiCommand::WriteInput {
            id: TabSelector::Active,
            bytes: body,
            reply: tx,
        },
        ("POST", ["debug", "replace-line"]) => ApiCommand::ReplaceLine {
            id: TabSelector::Active,
            bytes: body,
            reply: tx,
        },
        ("POST", ["debug", "note"]) => {
            let raw = String::from_utf8(body).map_err(|_| RouteError {
                status: 400,
                message: "note body must be utf-8".to_string(),
            })?;
            let trimmed = raw.trim();
            let note = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
            ApiCommand::SetNote { id: TabSelector::Active, note, reply: tx }
        }

        _ => {
            return Err(RouteError {
                status: 404,
                message: format!("no route for {method} {path}"),
            });
        }
    };

    Ok(RouteOutcome::Command(cmd, rx))
}

fn parse_selector(s: &str) -> Result<TabSelector, RouteError> {
    if s == "active" {
        return Ok(TabSelector::Active);
    }
    s.parse::<u64>()
        .map(TabSelector::Id)
        .map_err(|_| RouteError {
            status: 400,
            message: format!("invalid tab selector: {s}"),
        })
}

fn legacy_help_text() -> String {
    [
        "available endpoints:",
        "  GET    /tabs",
        "  POST   /tabs",
        "  DELETE /tabs/:id",
        "  POST   /tabs/:id/activate",
        "  GET    /tabs/:id",
        "  GET    /tabs/:id/screen",
        "  POST   /tabs/:id/input         (raw body)",
        "  POST   /tabs/:id/keys          (tmux key tokens)",
        "legacy:",
        "  GET    /debug                  (this page)",
        "  GET    /debug/state            -> /tabs/active",
        "  GET    /debug/screen           -> /tabs/active/screen",
        "  POST   /debug/input            -> /tabs/active/input",
        "  POST   /debug/replace-line     active tab only",
        "  POST   /debug/note             active tab only",
        "",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(method: &str, path: &str, body: &[u8]) -> Result<ApiCommand, RouteError> {
        match parse_request(method, path, body.to_vec())? {
            RouteOutcome::Command(cmd, _rx) => Ok(cmd),
            RouteOutcome::Immediate { .. } => panic!("expected command, got immediate response"),
        }
    }

    #[test]
    fn get_tabs_routes_to_list() {
        assert!(matches!(route("GET", "/tabs", b"").unwrap(), ApiCommand::ListTabs { .. }));
    }

    #[test]
    fn post_tabs_routes_to_create() {
        assert!(matches!(route("POST", "/tabs", b"").unwrap(), ApiCommand::CreateTab { .. }));
    }

    #[test]
    fn delete_tab_by_id_routes_to_close() {
        match route("DELETE", "/tabs/7", b"").unwrap() {
            ApiCommand::CloseTab { id, .. } => assert_eq!(id, TabSelector::Id(7)),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn active_selector_resolves_to_active_variant() {
        match route("GET", "/tabs/active", b"").unwrap() {
            ApiCommand::GetTab { id, .. } => assert_eq!(id, TabSelector::Active),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn post_activate_routes() {
        assert!(matches!(
            route("POST", "/tabs/3/activate", b"").unwrap(),
            ApiCommand::ActivateTab { .. }
        ));
    }

    #[test]
    fn get_screen_routes() {
        assert!(matches!(
            route("GET", "/tabs/3/screen", b"").unwrap(),
            ApiCommand::GetScreen { .. }
        ));
    }

    #[test]
    fn post_input_carries_body_bytes() {
        match route("POST", "/tabs/3/input", b"echo hi\n").unwrap() {
            ApiCommand::WriteInput { bytes, id, .. } => {
                assert_eq!(bytes, b"echo hi\n");
                assert_eq!(id, TabSelector::Id(3));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn post_keys_carries_body_string() {
        match route("POST", "/tabs/3/keys", b"Enter").unwrap() {
            ApiCommand::SendKeys { body, .. } => assert_eq!(body, "Enter"),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn legacy_debug_state_maps_to_get_active() {
        match route("GET", "/debug/state", b"").unwrap() {
            ApiCommand::GetTab { id, .. } => assert_eq!(id, TabSelector::Active),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn legacy_debug_screen_maps_to_get_screen_active() {
        match route("GET", "/debug/screen", b"").unwrap() {
            ApiCommand::GetScreen { id, .. } => assert_eq!(id, TabSelector::Active),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn legacy_debug_input_maps_to_input_active() {
        match route("POST", "/debug/input", b"abc").unwrap() {
            ApiCommand::WriteInput { id, bytes, .. } => {
                assert_eq!(id, TabSelector::Active);
                assert_eq!(bytes, b"abc");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn legacy_debug_replace_line_maps_to_replace() {
        match route("POST", "/debug/replace-line", b"hi").unwrap() {
            ApiCommand::ReplaceLine { id, bytes, .. } => {
                assert_eq!(id, TabSelector::Active);
                assert_eq!(bytes, b"hi");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn legacy_debug_note_maps_to_set_note() {
        match route("POST", "/debug/note", b"hello").unwrap() {
            ApiCommand::SetNote { id, note, .. } => {
                assert_eq!(id, TabSelector::Active);
                assert_eq!(note.as_deref(), Some("hello"));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn empty_note_body_clears_note() {
        match route("POST", "/debug/note", b"   ").unwrap() {
            ApiCommand::SetNote { note, .. } => assert!(note.is_none()),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn legacy_debug_root_is_immediate_text() {
        match parse_request("GET", "/debug", Vec::new()).unwrap() {
            RouteOutcome::Immediate { status, body, .. } => {
                assert_eq!(status, 200);
                assert!(body.contains("/tabs"));
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn unknown_path_returns_404() {
        let err = parse_request("GET", "/nope", Vec::new()).unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[test]
    fn bad_id_returns_400() {
        let err = parse_request("GET", "/tabs/notanumber", Vec::new()).unwrap_err();
        assert_eq!(err.status, 400);
    }
}
