use super::*;

pub(super) fn apply(
    this: &mut TerminalTabs,
    cmd: crate::api_protocol::ApiCommand,
    window: &mut Window,
    cx: &mut Context<TerminalTabs>,
) {
        use crate::api_protocol::{ApiCommand, ApiReply, ReplyBody, TabSelector, TabSummaryDto};

        // Count this request against the targeted tab so that `http_requests`
        // in the tab detail reflects API traffic. Commands without a specific
        // target are counted against the active tab if one exists.
        let selector = match &cmd {
            ApiCommand::CloseTab { id, .. }
            | ApiCommand::ActivateTab { id, .. }
            | ApiCommand::GetTab { id, .. }
            | ApiCommand::GetScreen { id, .. }
            | ApiCommand::GetScreenAnsi { id, .. }
            | ApiCommand::GetScrollback { id, .. }
            | ApiCommand::ScrollDisplay { id, .. }
            | ApiCommand::ResizeTab { id, .. }
            | ApiCommand::WriteInput { id, .. }
            | ApiCommand::WebInput { id, .. }
            | ApiCommand::SendKeys { id, .. }
            | ApiCommand::SetNote { id, .. }
            | ApiCommand::ReplaceLine { id, .. }
            | ApiCommand::WebStreamAttach { id, .. } => Some(*id),
            ApiCommand::ListTabs { .. }
            | ApiCommand::CreateTab { .. }
            | ApiCommand::WebClient { .. }
            | ApiCommand::WebStreamDetach { .. } => Some(TabSelector::Active),
        };
        if let Some(sel) = selector
            && let Some((_, tab)) = this.resolve_tab(sel)
            && let TerminalTabKind::Terminal { terminal, .. } = &tab.kind
        {
            terminal.read(cx).record_http_request();
        }

        match cmd {
            ApiCommand::ListTabs { reply } => {
                let tabs: Vec<TabSummaryDto> = this
                    .tabs
                    .iter()
                    .map(|tab| build_tab_summary(tab, cx))
                    .collect();
                let active = this.tabs.get(this.active_tab).map(|t| t.api_id());
                let value = serde_json::json!({ "active": active, "tabs": tabs });
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Json(value),
                });
            }

            ApiCommand::CreateTab { reply } => {
                this.open_new_tab(window, cx);
                let summary = this
                    .tabs
                    .last()
                    .map(|tab| build_tab_summary(tab, cx))
                    .expect("just-created tab must be present");
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 201,
                    body: ReplyBody::Json(
                        serde_json::to_value(summary).unwrap_or_else(|_| serde_json::json!({})),
                    ),
                });
            }

            ApiCommand::CloseTab { id, reply } => {
                let Some((index, tab)) = this.resolve_tab(id) else {
                    return reply_err(&reply, 404, "unknown tab");
                };
                let closed_id = tab.api_id();
                let _ = tab;
                this.close_tab_at_index(index, cx);
                this.pending_focus_sync = true;
                cx.notify();
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Json(serde_json::json!({ "closed": closed_id })),
                });
            }

            ApiCommand::ActivateTab { id, reply } => {
                let Some((index, tab)) = this.resolve_tab(id) else {
                    return reply_err(&reply, 404, "unknown tab");
                };
                let new_id = tab.api_id();
                let _ = tab;
                this.active_tab = index;
                this.pending_focus_sync = true;
                cx.notify();
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Json(serde_json::json!({ "active": new_id })),
                });
            }

            ApiCommand::GetTab { id, reply } => {
                let Some((_, tab)) = this.resolve_tab(id) else {
                    return reply_err(&reply, 404, "unknown tab");
                };
                let detail = build_tab_detail(tab, cx);
                let value = serde_json::to_value(detail).unwrap_or_else(|_| serde_json::json!({}));
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Json(value),
                });
            }

            ApiCommand::GetScreen { id, reply } => {
                let Some((_, tab)) = this.resolve_tab(id) else {
                    return reply_err(&reply, 404, "unknown tab");
                };
                let text = match &tab.kind {
                    TerminalTabKind::Terminal { terminal, .. } => terminal.read(cx).screen_text(),
                    TerminalTabKind::Snapshot { .. } => "<snapshot tab>\n".to_string(),
                };
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Text(text),
                });
            }

            ApiCommand::GetScreenAnsi { id, reply } => {
                let Some((_, tab)) = this.resolve_tab(id) else {
                    return reply_err(&reply, 404, "unknown tab");
                };
                let text = match &tab.kind {
                    TerminalTabKind::Terminal { terminal, .. } => {
                        let term = terminal.read(cx);
                        encode_screen(&term.snapshot, term.term.mode().clone())
                    }
                    TerminalTabKind::Snapshot { .. } => {
                        // A snapshot tab has no PTY modes; reset xterm's
                        // mouse service so a previous tab's mouse mode cannot
                        // swallow clicks here.
                        "0\u{1b}[?1000l\u{1b}[?1002l\u{1b}[?1003l\u{1b}[?1006l\u{1b}[2J\u{1b}[H[snapshot tab]\r\n".to_string()
                    }
                };
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Text(text),
                });
            }

            ApiCommand::ResizeTab {
                id,
                cols,
                rows,
                sync_window,
                reply,
            } => {
                let Some((_, tab)) = this.resolve_tab(id) else {
                    return reply_err(&reply, 404, "unknown tab");
                };
                let terminal = match &tab.kind {
                    TerminalTabKind::Terminal { terminal, .. } => terminal.clone(),
                    TerminalTabKind::Snapshot { .. } => {
                        return reply_err(&reply, 409, "cannot resize a snapshot tab");
                    }
                };
                let grid = GridSize { cols, rows };
                terminal.update(cx, |term, _| {
                    term.set_web_grid_override(grid);
                });
                if sync_window {
                    // Browser is authoritative: make the native window match
                    // the grid so PC shows the same cells without scrolling.
                    this.pending_window_resize = Some(grid);
                }
                cx.notify();
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Json(serde_json::json!({ "cols": cols, "rows": rows })),
                });
            }

            ApiCommand::WebClient {
                id,
                connected,
                cols: _cols,
                rows: _rows,
                browser_size,
                force,
                reply,
            } => {
                let now = Instant::now();
                let stale = this.web_client.as_ref().is_some_and(|client| {
                    now.duration_since(client.last_seen) > WEB_CLIENT_TIMEOUT
                });
                if stale {
                    this.web_client = None;
                    this.pending_sync_to_window = true;
                }

                if connected {
                    let replacing = this
                        .web_client
                        .as_ref()
                        .is_some_and(|client| client.id != id);
                    if replacing && !force {
                        let message = format!(
                            "another web client is already connected: {}",
                            this.web_client.as_ref().expect("checked above").id
                        );
                        return reply_err(&reply, 409, &message);
                    }
                    if replacing {
                        // Closing every old sender wakes its socket thread even
                        // when no PTY output is arriving. The old client's late
                        // detach remains harmless because it uses a stream id.
                        for tab in &this.tabs {
                            if let TerminalTabKind::Terminal { terminal, .. } = &tab.kind {
                                terminal.update(cx, |term, _| term.detach_other_web_streams(&id));
                            }
                        }
                    }
                    this.web_client = Some(WebClientInfo { id, last_seen: now });
                    if !browser_size {
                        // "PC 尺寸": the native window is authoritative again.
                        // Drop any browser-driven grid override and let the
                        // grid follow the window. The window keeps its current
                        // size (that IS the PC authority now).
                        for tab in &this.tabs {
                            if let TerminalTabKind::Terminal { terminal, .. } = &tab.kind {
                                terminal.update(cx, |term, _| term.clear_web_grid_override());
                            }
                        }
                        this.pending_sync_to_window = true;
                        this.pending_window_resize = None;
                        this.pending_window_restore = this.browser_size_restore.is_some();
                    }
                } else if this
                    .web_client
                    .as_ref()
                    .is_some_and(|client| client.id == id)
                {
                    this.web_client = None;
                    this.pending_sync_to_window = true;
                }

                // Mode changes update pending window/grid work for render().
                // Explicitly schedule that render even when the native window
                // is otherwise static; without this notification switching back
                // to PC-size mode could wait for an unrelated future repaint.
                cx.notify();
                let connected_now = this.web_client.is_some();
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Json(serde_json::json!({
                        "connected": connected_now,
                        "clients": if connected_now { 1 } else { 0 }
                    })),
                });
            }

            ApiCommand::GetScrollback { id, lines, reply } => {
                let Some((_, tab)) = this.resolve_tab(id) else {
                    return reply_err(&reply, 404, "unknown tab");
                };
                let text = match &tab.kind {
                    TerminalTabKind::Terminal { terminal, .. } => {
                        terminal.read(cx).scrollback_text(lines)
                    }
                    TerminalTabKind::Snapshot { .. } => {
                        return reply_err(&reply, 409, "cannot read scrollback of snapshot tab");
                    }
                };
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Text(text),
                });
            }

            ApiCommand::ScrollDisplay { id, action, reply } => {
                let Some((_, tab)) = this.resolve_tab(id) else {
                    return reply_err(&reply, 404, "unknown tab");
                };
                let terminal = match &tab.kind {
                    TerminalTabKind::Terminal { terminal, .. } => terminal.clone(),
                    TerminalTabKind::Snapshot { .. } => {
                        return reply_err(&reply, 409, "cannot scroll a snapshot tab");
                    }
                };
                let offset = terminal.update(cx, |term, _| term.scroll_viewport(action));
                cx.notify();
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Json(serde_json::json!({ "display_offset": offset })),
                });
            }

            ApiCommand::WebInput {
                id,
                client,
                bytes,
                reply,
            } => {
                if !this
                    .web_client
                    .as_ref()
                    .is_some_and(|owner| owner.id == client)
                {
                    return reply_err(&reply, 409, "web client ownership changed");
                }
                this.with_terminal_write(id, &reply, cx, move |term| term.write_injected(&bytes));
            }
            ApiCommand::WriteInput { id, bytes, reply } => {
                this.with_terminal_write(id, &reply, cx, move |term| term.write_injected(&bytes));
            }

            ApiCommand::SendKeys { id, body, reply } => match crate::api_keys::parse_keys(&body) {
                Ok(bytes) => {
                    this.with_terminal_write(id, &reply, cx, move |term| {
                        term.write_injected(&bytes)
                    });
                }
                Err(err) => reply_err(&reply, 400, &err),
            },

            ApiCommand::SetNote { id, note, reply } => {
                let Some((_, tab)) = this.resolve_tab(id) else {
                    return reply_err(&reply, 404, "unknown tab");
                };
                match &tab.kind {
                    TerminalTabKind::Terminal { terminal, .. } => {
                        terminal.read(cx).set_note(note.clone());
                        let _ = reply.send_blocking(ApiReply::Ok {
                            status: 200,
                            body: ReplyBody::Json(serde_json::json!({ "note": note })),
                        });
                    }
                    TerminalTabKind::Snapshot { .. } => {
                        reply_err(&reply, 409, "cannot set note on snapshot tab");
                    }
                }
            }

            ApiCommand::ReplaceLine { id, bytes, reply } => {
                this.with_terminal_write(id, &reply, cx, move |term| {
                    term.replace_line_injected(&bytes)
                });
            }

            ApiCommand::WebStreamAttach {
                id,
                client,
                stream_id,
                reply,
                stream,
            } => {
                let Some((_, tab)) = this.resolve_tab(id) else {
                    return reply_err(&reply, 404, "unknown tab");
                };
                let terminal = match &tab.kind {
                    TerminalTabKind::Terminal { terminal, .. } => terminal.clone(),
                    TerminalTabKind::Snapshot { .. } => {
                        return reply_err(&reply, 409, "cannot stream a snapshot tab");
                    }
                };

                // Reuse the heartbeat registry so the WebSocket data plane and
                // the HTTP control plane agree on who owns the browser side.
                let now = Instant::now();
                let stale = this
                    .web_client
                    .as_ref()
                    .is_some_and(|c| now.duration_since(c.last_seen) > WEB_CLIENT_TIMEOUT);
                if stale {
                    this.web_client = None;
                }
                if let Some(existing) = &this.web_client
                    && existing.id != client
                {
                    return reply_err(&reply, 409, "another web client is already connected");
                }
                this.web_client = Some(WebClientInfo {
                    id: client.clone(),
                    last_seen: now,
                });

                // Register the reader and capture the snapshot in the same UI
                // turn: PTY batches are also applied on this thread, so no
                // output can slip between the seed and the live stream.
                let (rx, alt, ansi, cols, rows) = terminal.update(cx, |term, _| {
                    let rx = term.attach_web_stream(stream_id, client);
                    let encoded = encode_screen(&term.snapshot, term.term.mode().clone());
                    let alt = encoded.starts_with('1');
                    (
                        rx,
                        alt,
                        encoded[1..].to_string(),
                        term.grid_size.cols,
                        term.grid_size.rows,
                    )
                });
                let _ = stream.send_blocking(rx);
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Json(serde_json::json!({
                        "alt": alt,
                        "ansi": ansi,
                        "cols": cols,
                        "rows": rows
                    })),
                });
            }

            ApiCommand::WebStreamDetach { stream_id, reply } => {
                // Remove exactly this connection's subscription. Clearing the
                // client registry here let an old socket's late detach log out
                // the browser's newer connection.
                for tab in &this.tabs {
                    if let TerminalTabKind::Terminal { terminal, .. } = &tab.kind {
                        terminal.update(cx, |term, _| term.detach_web_stream(stream_id));
                    }
                }
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Json(serde_json::json!({ "detached": true })),
                });
            }
        }
    }
