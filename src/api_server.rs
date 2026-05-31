use crate::api_protocol::{
    ApiCommand, ApiReply, ReplyBody, RouteOutcome, ScrollAction, ScrollRequest, TabSelector,
    oneshot,
};
use async_channel::Sender;
use std::io::Cursor;
use std::thread;
use tiny_http::{Header, Method, Response, Server, StatusCode};

#[derive(Debug)]
pub(crate) struct RouteError {
    pub(crate) status: u16,
    pub(crate) message: String,
}

pub(crate) fn parse_request(
    method: &str,
    path: &str,
    query: &str,
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
        ("GET", ["tabs", sel, "scrollback"]) => ApiCommand::GetScrollback {
            id: parse_selector(sel)?,
            lines: parse_lines_query(query)?,
            reply: tx,
        },
        ("POST", ["tabs", sel, "scroll"]) => ApiCommand::ScrollDisplay {
            id: parse_selector(sel)?,
            action: parse_scroll_body(&body)?,
            reply: tx,
        },
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

fn parse_lines_query(query: &str) -> Result<Option<usize>, RouteError> {
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let mut it = pair.splitn(2, '=');
        let key = it.next().unwrap_or("");
        let val = it.next().unwrap_or("");
        if key == "lines" {
            return val
                .parse::<usize>()
                .map(Some)
                .map_err(|_| RouteError {
                    status: 400,
                    message: format!("invalid `lines` value: {val}"),
                });
        }
    }
    Ok(None)
}

fn parse_scroll_body(body: &[u8]) -> Result<ScrollAction, RouteError> {
    let req: ScrollRequest = serde_json::from_slice(body).map_err(|err| RouteError {
        status: 400,
        message: format!("invalid scroll body (expected JSON {{action, lines?}}): {err}"),
    })?;
    let lines = req.lines.unwrap_or(1);
    match req.action.as_str() {
        "up" => Ok(ScrollAction::Up(lines)),
        "down" => Ok(ScrollAction::Down(lines)),
        "page_up" => Ok(ScrollAction::PageUp),
        "page_down" => Ok(ScrollAction::PageDown),
        "top" => Ok(ScrollAction::Top),
        "bottom" => Ok(ScrollAction::Bottom),
        other => Err(RouteError {
            status: 400,
            message: format!(
                "unknown scroll action: {other} (want up|down|page_up|page_down|top|bottom)"
            ),
        }),
    }
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
        "  GET    /tabs/:id/scrollback    (?lines=N, default all up to 10000)",
        "  POST   /tabs/:id/scroll        (JSON: {action,lines?})",
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

pub(crate) fn start_api_server(addr: &str, cmd_tx: Sender<ApiCommand>) -> std::io::Result<()> {
    let server = Server::http(addr).map_err(|err| {
        std::io::Error::new(std::io::ErrorKind::AddrInUse, format!("bind {addr}: {err}"))
    })?;
    // Resolve the OS-assigned port when the caller passed `:0`, and round-trip
    // hostnames into their concrete socket address. The user-facing log should
    // always be reachable as-is.
    let bound = server.server_addr().to_string();
    thread::Builder::new()
        .name("agent-api-http".to_string())
        .spawn(move || serve(server, cmd_tx, bound))?;
    Ok(())
}

fn serve(server: Server, cmd_tx: Sender<ApiCommand>, addr: String) {
    eprintln!("agent api listening on http://{addr}");
    for mut request in server.incoming_requests() {
        let method = method_str(request.method()).to_string();
        let url = request.url();
        let mut parts = url.splitn(2, '?');
        let path = parts.next().unwrap_or("/").to_string();
        let query = parts.next().unwrap_or("").to_string();
        let mut body = Vec::new();
        if let Err(err) = request.as_reader().read_to_end(&mut body) {
            let _ = request.respond(error_response(400, &format!("read body: {err}")));
            continue;
        }
        let response = match parse_request(&method, &path, &query, body) {
            Ok(RouteOutcome::Immediate { status, content_type, body }) => {
                text_response(status, content_type, body)
            }
            Ok(RouteOutcome::Command(cmd, rx)) => match cmd_tx.send_blocking(cmd) {
                Ok(()) => match rx.recv_blocking() {
                    Ok(reply) => render_reply(reply),
                    Err(_) => error_response(504, "command channel closed"),
                },
                Err(_) => error_response(503, "command channel closed"),
            },
            Err(err) => error_response(err.status, &err.message),
        };
        if let Err(err) = request.respond(response) {
            eprintln!("agent api: failed to send response: {err}");
        }
    }
}

fn method_str(m: &Method) -> &'static str {
    match m {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        Method::Patch => "PATCH",
        _ => "OTHER",
    }
}

fn render_reply(reply: ApiReply) -> Response<Cursor<Vec<u8>>> {
    match reply {
        ApiReply::Ok { status, body } => match body {
            ReplyBody::Json(value) => text_response(
                status,
                "application/json; charset=utf-8",
                serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
            ),
            ReplyBody::Text(text) => text_response(status, "text/plain; charset=utf-8", text),
        },
        ApiReply::Err { status, error } => error_response(status, &error),
    }
}

fn text_response(
    status: u16,
    content_type: &str,
    body: impl Into<Vec<u8>>,
) -> Response<Cursor<Vec<u8>>> {
    let mut resp = Response::from_data(body.into()).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes("Content-Type", content_type) {
        resp = resp.with_header(header);
    }
    resp
}

fn error_response(status: u16, message: &str) -> Response<Cursor<Vec<u8>>> {
    let body = serde_json::json!({ "error": message }).to_string();
    text_response(status, "application/json; charset=utf-8", body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(method: &str, path: &str, body: &[u8]) -> Result<ApiCommand, RouteError> {
        route_q(method, path, "", body)
    }

    fn route_q(
        method: &str,
        path: &str,
        query: &str,
        body: &[u8],
    ) -> Result<ApiCommand, RouteError> {
        match parse_request(method, path, query, body.to_vec())? {
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
        match parse_request("GET", "/debug", "", Vec::new()).unwrap() {
            RouteOutcome::Immediate { status, body, .. } => {
                assert_eq!(status, 200);
                assert!(body.contains("/tabs"));
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn unknown_path_returns_404() {
        let err = parse_request("GET", "/nope", "", Vec::new()).unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[test]
    fn bad_id_returns_400() {
        let err = parse_request("GET", "/tabs/notanumber", "", Vec::new()).unwrap_err();
        assert_eq!(err.status, 400);
    }

    #[test]
    fn get_scrollback_routes() {
        match route("GET", "/tabs/3/scrollback", b"").unwrap() {
            ApiCommand::GetScrollback { id, lines, .. } => {
                assert_eq!(id, TabSelector::Id(3));
                assert_eq!(lines, None);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn get_scrollback_with_lines_query() {
        match route_q("GET", "/tabs/active/scrollback", "lines=200", b"").unwrap() {
            ApiCommand::GetScrollback { id, lines, .. } => {
                assert_eq!(id, TabSelector::Active);
                assert_eq!(lines, Some(200));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn get_scrollback_rejects_non_numeric_lines() {
        let err = route_q("GET", "/tabs/1/scrollback", "lines=abc", b"").unwrap_err();
        assert_eq!(err.status, 400);
    }

    #[test]
    fn post_scroll_up_with_lines() {
        match route("POST", "/tabs/2/scroll", br#"{"action":"up","lines":5}"#).unwrap() {
            ApiCommand::ScrollDisplay { id, action, .. } => {
                assert_eq!(id, TabSelector::Id(2));
                assert_eq!(action, crate::api_protocol::ScrollAction::Up(5));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn post_scroll_top_no_lines() {
        match route("POST", "/tabs/active/scroll", br#"{"action":"top"}"#).unwrap() {
            ApiCommand::ScrollDisplay { action, .. } => {
                assert_eq!(action, crate::api_protocol::ScrollAction::Top);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn post_scroll_unknown_action_rejected() {
        let err = route("POST", "/tabs/1/scroll", br#"{"action":"sideways"}"#).unwrap_err();
        assert_eq!(err.status, 400);
    }

    #[test]
    fn post_scroll_missing_action_rejected() {
        let err = route("POST", "/tabs/1/scroll", br#"{"lines":5}"#).unwrap_err();
        assert_eq!(err.status, 400);
    }

    use crate::api_protocol::{ApiReply, ReplyBody};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    fn reserve_local_addr() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        addr.to_string()
    }

    fn wait_for_server(addr: &str) {
        for _ in 0..40 {
            if TcpStream::connect(addr).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("server did not start: {addr}");
    }

    fn send_http(addr: &str, request: &str) -> String {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream.write_all(request.as_bytes()).expect("write");
        let _ = stream.shutdown(std::net::Shutdown::Write);
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read");
        response
    }

    #[test]
    fn server_round_trip_get_tabs_via_fake_handler() {
        let addr = reserve_local_addr();
        let (cmd_tx, cmd_rx) = async_channel::unbounded::<ApiCommand>();

        std::thread::spawn(move || {
            while let Ok(cmd) = cmd_rx.recv_blocking() {
                if let ApiCommand::ListTabs { reply } = cmd {
                    let body = serde_json::json!({
                        "active": 1,
                        "tabs": [{"id":1,"title":"zsh","kind":"terminal","cols":80,"rows":24}],
                    });
                    let _ = reply.send_blocking(ApiReply::Ok {
                        status: 200,
                        body: ReplyBody::Json(body),
                    });
                }
            }
        });

        start_api_server(&addr, cmd_tx).expect("server starts");
        wait_for_server(&addr);

        let response = send_http(
            &addr,
            &format!("GET /tabs HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        );
        assert!(response.contains("200 OK"), "response: {response}");
        assert!(response.contains("\"active\":1"), "response: {response}");
    }

    #[test]
    fn server_returns_404_for_unknown_route() {
        let addr = reserve_local_addr();
        let (cmd_tx, _cmd_rx) = async_channel::unbounded::<ApiCommand>();
        start_api_server(&addr, cmd_tx).expect("server starts");
        wait_for_server(&addr);

        let response = send_http(
            &addr,
            &format!("GET /nope HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        );
        assert!(response.contains("404"), "response: {response}");
    }

    #[test]
    fn server_serves_immediate_help_for_debug_root() {
        let addr = reserve_local_addr();
        let (cmd_tx, _cmd_rx) = async_channel::unbounded::<ApiCommand>();
        start_api_server(&addr, cmd_tx).expect("server starts");
        wait_for_server(&addr);

        let response = send_http(
            &addr,
            &format!("GET /debug HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        );
        assert!(response.contains("200 OK"), "response: {response}");
        assert!(response.contains("/tabs"), "response: {response}");
    }
}
