use gpui::{
    Context, Entity, MouseButton, Render, Subscription, Task, Window, WindowControlArea, div,
    prelude::*, px, rgb,
};
use std::time::{Duration, Instant};

use crate::cli::CliOptions;
use crate::geometry::CUSTOM_TITLE_BAR_HEIGHT;
use crate::snapshot_tab::SnapshotTab;
use crate::terminal::{AgentTerminal, GridSize, TerminalExitedEvent};
use crate::web_ansi::encode_screen;

mod api_handlers;

const TRAFFIC_LIGHT_LEFT_GUTTER: gpui::Pixels = px(68.0);
const MAX_TAB_TITLE_CHARS: usize = 28;

enum TerminalTabKind {
    Terminal {
        terminal: Entity<AgentTerminal>,
        _exit_subscription: Subscription,
    },
    Snapshot {
        snapshot: Entity<SnapshotTab>,
    },
}

struct TerminalTab {
    id: usize,
    kind: TerminalTabKind,
}

struct WebClientInfo {
    id: String,
    last_seen: Instant,
}

const WEB_CLIENT_TIMEOUT: Duration = Duration::from_secs(5);

impl TerminalTab {
    fn api_id(&self) -> u64 {
        self.id as u64
    }

    fn title(&self, cx: &mut Context<TerminalTabs>) -> String {
        let raw_title = match &self.kind {
            TerminalTabKind::Terminal { terminal, .. } => terminal.read(cx).tab_title(),
            TerminalTabKind::Snapshot { snapshot } => snapshot.read(cx).title(),
        };
        truncate_tab_title(&raw_title, MAX_TAB_TITLE_CHARS)
    }

    fn focus(&self, window: &mut Window, cx: &mut Context<TerminalTabs>) {
        match &self.kind {
            TerminalTabKind::Terminal { terminal, .. } => {
                terminal.update(cx, |terminal, cx| {
                    window.focus(&terminal.focus_handle, cx);
                });
            }
            TerminalTabKind::Snapshot { snapshot } => {
                snapshot.update(cx, |snapshot, cx| {
                    window.focus(&snapshot.focus_handle, cx);
                });
            }
        }
    }

    fn terminal(&self) -> Option<Entity<AgentTerminal>> {
        match &self.kind {
            TerminalTabKind::Terminal { terminal, .. } => Some(terminal.clone()),
            TerminalTabKind::Snapshot { .. } => None,
        }
    }
}

macro_rules! define_tab_switch_handlers {
    ($(($method:ident, $action:ty, $index:expr)),+ $(,)?) => {
        $(
            fn $method(
                &mut self,
                _: &$action,
                window: &mut Window,
                cx: &mut Context<Self>,
            ) {
                self.activate_tab_by_index($index, window, cx);
            }
        )+
    };
}

pub(crate) struct TerminalTabs {
    cli: CliOptions,
    tabs: Vec<TerminalTab>,
    active_tab: usize,
    next_tab_id: usize,
    next_snapshot_id: usize,
    pending_focus_sync: bool,
    web_client: Option<WebClientInfo>,
    pending_sync_to_window: bool,
    /// "浏览器尺寸" mode: resize the native window to fit this grid. Consumed
    /// in render(), the only place with a `&mut Window`.
    pending_window_resize: Option<crate::terminal::GridSize>,
    default_window_size: gpui::Size<gpui::Pixels>,
    browser_size_restore: Option<gpui::Size<gpui::Pixels>>,
    browser_size_target: Option<gpui::Size<gpui::Pixels>>,
    pending_window_restore: bool,
    _api_drain: Task<()>,
}

impl TerminalTabs {
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        cli: CliOptions,
        api_rx: async_channel::Receiver<crate::api_protocol::ApiCommand>,
    ) -> Self {
        let api_drain = cx.spawn(async move |this, cx| {
            while let Ok(cmd) = api_rx.recv().await {
                // update_in supplies the Window directly from the app context.
                // Unlike deferring work to render(), this still runs while the
                // screen is locked or the native window is not being redrawn.
                let _ = this.update_in(cx, |this, window, cx| {
                    this.apply_api_command(cmd, window, cx)
                });
            }
        });

        let mut this = Self {
            cli,
            tabs: Vec::new(),
            active_tab: 0,
            next_tab_id: 1,
            next_snapshot_id: 1,
            pending_focus_sync: false,
            web_client: None,
            pending_sync_to_window: false,
            pending_window_resize: None,
            default_window_size: window.viewport_size(),
            browser_size_restore: None,
            browser_size_target: None,
            pending_window_restore: false,
            _api_drain: api_drain,
        };

        this.open_new_tab(window, cx);
        this
    }

    fn open_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> usize {
        let terminal = cx.new(|cx| AgentTerminal::new_embedded(window, cx, self.cli.clone()));
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let exit_subscription = cx.subscribe(
            &terminal,
            move |this, _terminal, _event: &TerminalExitedEvent, cx| {
                this.close_tab_by_id(tab_id, cx);
            },
        );

        self.tabs.push(TerminalTab {
            id: tab_id,
            kind: TerminalTabKind::Terminal {
                terminal,
                _exit_subscription: exit_subscription,
            },
        });
        self.active_tab = self.tabs.len().saturating_sub(1);
        self.request_focus_active_tab(window, cx);
        cx.notify();
        tab_id
    }

    fn open_snapshot_tab_from_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source_terminal) = self
            .tabs
            .get(self.active_tab)
            .and_then(TerminalTab::terminal)
        else {
            return;
        };

        let title = format!("snap {}", self.next_snapshot_id);
        self.next_snapshot_id += 1;
        let snapshot_data = source_terminal.read(cx).capture_snapshot_data(title);
        let snapshot = cx.new(|cx| SnapshotTab::new(window, cx, snapshot_data));

        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(TerminalTab {
            id: tab_id,
            kind: TerminalTabKind::Snapshot { snapshot },
        });
        self.active_tab = self.tabs.len().saturating_sub(1);
        self.request_focus_active_tab(window, cx);
        cx.notify();
    }

    fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            cx.notify();
            return;
        }

        self.close_tab_at_index(self.active_tab, cx);
        if self.tabs.is_empty() {
            cx.notify();
            return;
        }

        self.request_focus_active_tab(window, cx);
        cx.notify();
    }

    fn close_tab_by_id(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };

        self.close_tab_at_index(index, cx);
        if !self.tabs.is_empty() {
            self.pending_focus_sync = true;
            cx.notify();
        }
    }

    fn close_tab_at_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }

        self.tabs.remove(index);

        if self.tabs.is_empty() {
            // Keep the window (and therefore the HTTP/web service) alive with
            // an empty terminal surface; a later "+" or POST /tabs recreates it.
            self.active_tab = 0;
            cx.notify();
            return;
        }

        self.active_tab = next_active_tab_index(self.active_tab, index, self.tabs.len());

        cx.notify();
    }

    fn activate_tab_by_id(&mut self, tab_id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };

        if self.active_tab == index {
            self.request_focus_active_tab(window, cx);
            return;
        }

        self.active_tab = index;
        self.request_focus_active_tab(window, cx);
        cx.notify();
    }

    fn activate_relative_tab(
        &mut self,
        offset: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.len() <= 1 {
            return;
        }

        let count = self.tabs.len() as isize;
        let active = self.active_tab as isize;
        self.active_tab = (active + offset).rem_euclid(count) as usize;
        self.request_focus_active_tab(window, cx);
        cx.notify();
    }

    fn activate_tab_by_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }

        if self.active_tab == index {
            self.request_focus_active_tab(window, cx);
            return;
        }

        self.active_tab = index;
        self.request_focus_active_tab(window, cx);
        cx.notify();
    }

    fn focus_active_tab_now(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            tab.focus(window, cx);
        }
    }

    fn request_focus_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_active_tab_now(window, cx);
        cx.on_next_frame(window, |this, window, cx| {
            this.focus_active_tab_now(window, cx);
        });
    }

    fn on_new_tab(&mut self, _: &crate::NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_tab(window, cx);
    }

    fn on_close_tab(&mut self, _: &crate::CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active_tab(window, cx);
    }

    fn on_next_tab(&mut self, _: &crate::NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_relative_tab(1, window, cx);
    }

    fn on_prev_tab(&mut self, _: &crate::PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_relative_tab(-1, window, cx);
    }

    fn on_capture_snapshot_tab(
        &mut self,
        _: &crate::CaptureSnapshotTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_snapshot_tab_from_active(window, cx);
    }

    define_tab_switch_handlers!(
        (on_switch_to_tab1, crate::SwitchToTab1, 0),
        (on_switch_to_tab2, crate::SwitchToTab2, 1),
        (on_switch_to_tab3, crate::SwitchToTab3, 2),
        (on_switch_to_tab4, crate::SwitchToTab4, 3),
        (on_switch_to_tab5, crate::SwitchToTab5, 4),
        (on_switch_to_tab6, crate::SwitchToTab6, 5),
        (on_switch_to_tab7, crate::SwitchToTab7, 6),
        (on_switch_to_tab8, crate::SwitchToTab8, 7),
        (on_switch_to_tab9, crate::SwitchToTab9, 8),
        (on_switch_to_tab10, crate::SwitchToTab10, 9),
    );

    fn apply_api_command(
        &mut self,
        cmd: crate::api_protocol::ApiCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        api_handlers::apply(self, cmd, window, cx)
    }

    /// Who owns the browser data plane right now, if the registration is still
    /// fresh. A browser page that does not own it may not reshape the PTY,
    /// retarget the native window, or pull the screen.
    fn web_owner(&self) -> Option<&str> {
        self.web_client
            .as_ref()
            .filter(|client| client.last_seen.elapsed() <= WEB_CLIENT_TIMEOUT)
            .map(|client| client.id.as_str())
    }

    /// True when `client` names a page that is not the current owner. A request
    /// with no client header is an operator/agent call and is never blocked.
    fn is_blocked_web_client(&self, client: Option<&str>) -> bool {
        web_client_should_yield(client, self.web_owner())
    }

    fn resolve_tab(
        &self,
        selector: crate::api_protocol::TabSelector,
    ) -> Option<(usize, &TerminalTab)> {
        use crate::api_protocol::TabSelector;
        let index = match selector {
            TabSelector::Active => self.active_tab,
            TabSelector::Id(id) => self.tabs.iter().position(|tab| tab.api_id() == id)?,
        };
        self.tabs.get(index).map(|t| (index, t))
    }

    fn with_terminal_write<F>(
        &mut self,
        selector: crate::api_protocol::TabSelector,
        reply: &async_channel::Sender<crate::api_protocol::ApiReply>,
        cx: &mut Context<Self>,
        op: F,
    ) where
        F: FnOnce(&mut AgentTerminal) -> Result<usize, String>,
    {
        use crate::api_protocol::{ApiReply, ReplyBody};
        let Some((_, tab)) = self.resolve_tab(selector) else {
            return reply_err(reply, 404, "unknown tab");
        };
        let terminal = match &tab.kind {
            TerminalTabKind::Terminal { terminal, .. } => terminal.clone(),
            TerminalTabKind::Snapshot { .. } => {
                return reply_err(reply, 409, "cannot write to snapshot tab");
            }
        };
        let result = terminal.update(cx, |term, _cx| op(term));
        match result {
            Ok(wrote) => {
                let _ = reply.send_blocking(ApiReply::Ok {
                    status: 200,
                    body: ReplyBody::Json(serde_json::json!({ "wrote": wrote })),
                });
            }
            Err(err) => reply_err(reply, 503, &err),
        }
    }
}

fn build_tab_summary(
    tab: &TerminalTab,
    cx: &mut Context<TerminalTabs>,
) -> crate::api_protocol::TabSummaryDto {
    use crate::api_protocol::{TabKind, TabSummaryDto};
    let (kind, cols, rows, title) = match &tab.kind {
        TerminalTabKind::Terminal { terminal, .. } => {
            let term = terminal.read(cx);
            let (cols, rows) = term.grid_dimensions();
            (TabKind::Terminal, cols, rows, term.tab_title())
        }
        TerminalTabKind::Snapshot { snapshot } => {
            (TabKind::Snapshot, 0u16, 0u16, snapshot.read(cx).title())
        }
    };
    TabSummaryDto {
        id: tab.api_id(),
        title,
        kind,
        cols,
        rows,
    }
}

fn build_tab_detail(
    tab: &TerminalTab,
    cx: &mut Context<TerminalTabs>,
) -> crate::api_protocol::TabDetailDto {
    use crate::api_protocol::{TabDetailDto, TabKind};
    match &tab.kind {
        TerminalTabKind::Terminal { terminal, .. } => {
            let term = terminal.read(cx);
            let snap = term.runtime_snapshot();
            let (cols, rows) = term.grid_dimensions();
            TabDetailDto {
                id: tab.api_id(),
                title: term.tab_title(),
                kind: TabKind::Terminal,
                cols,
                rows,
                cursor_row: snap.cursor_row,
                cursor_col: snap.cursor_col,
                status: snap.status,
                note: snap.note,
                counters: snap.counters,
                uptime_ms: snap.uptime_ms,
                last_error: snap.last_error,
            }
        }
        TerminalTabKind::Snapshot { snapshot } => {
            let snap = snapshot.read(cx);
            TabDetailDto {
                id: tab.api_id(),
                title: snap.title(),
                kind: TabKind::Snapshot,
                cols: 0,
                rows: 0,
                cursor_row: 0,
                cursor_col: 0,
                status: "snapshot".to_string(),
                note: None,
                counters: Default::default(),
                uptime_ms: 0,
                last_error: None,
            }
        }
    }
}

fn reply_err(
    reply: &async_channel::Sender<crate::api_protocol::ApiReply>,
    status: u16,
    error: &str,
) {
    let _ = reply.send_blocking(crate::api_protocol::ApiReply::Err {
        status,
        error: error.to_string(),
    });
}

impl Render for TerminalTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_focus_sync {
            self.pending_focus_sync = false;
            self.request_focus_active_tab(window, cx);
        }

        if self.pending_window_restore {
            self.pending_window_restore = false;
            if let Some(restore) = self.browser_size_restore.take() {
                let user_resized = self
                    .browser_size_target
                    .is_some_and(|target| !window_sizes_match(window.viewport_size(), target));
                if !user_resized {
                    window.resize(restore);
                }
            }
            self.browser_size_target = None;
        }

        // Browser-size mode needs an ordinary window: fullscreen/maximized
        // bounds cannot faithfully mirror a small browser grid. Save the
        // ordinary size once, then restore it when PC-size mode resumes.
        if let Some(grid) = self.pending_window_resize.take() {
            if self.browser_size_restore.is_none() {
                let constrained = window.is_fullscreen()
                    || window.is_simple_fullscreen()
                    || window.is_maximized();
                self.browser_size_restore = Some(if constrained {
                    self.default_window_size
                } else {
                    window.viewport_size()
                });
            }
            if window.is_fullscreen() {
                window.toggle_fullscreen();
            } else if window.is_simple_fullscreen() {
                window.toggle_simple_fullscreen();
            }
            if window.is_maximized() {
                window.zoom_window();
            }
            // Resizing the window fires observe_window_bounds ->
            // sync_grid_to_window, but the grid override is still set, so the
            // browser-driven grid survives.
            if let Some((_, tab)) = self.resolve_tab(crate::api_protocol::TabSelector::Active)
                && let TerminalTabKind::Terminal { terminal, .. } = &tab.kind
            {
                let cell_width = crate::geometry::measure_cell_width(
                    window,
                    &terminal.read(cx).font_family,
                    terminal.read(cx).font_fallbacks.as_ref(),
                    terminal.read(cx).font_size,
                );
                let target = terminal.read(cx).window_size_for_grid(grid, cell_width);
                self.browser_size_target = Some(target);
                window.resize(target);
            }
        }

        if self
            .web_client
            .as_ref()
            .is_some_and(|client| client.last_seen.elapsed() > WEB_CLIENT_TIMEOUT)
        {
            self.web_client = None;
            self.pending_sync_to_window = true;
        }

        let web_connected = self.web_client.is_some();
        for tab in &self.tabs {
            if let TerminalTabKind::Terminal { terminal, .. } = &tab.kind {
                terminal.update(cx, |term, _| {
                    term.set_padding_dots(web_connected);
                    if self.pending_sync_to_window {
                        term.request_sync_to_window();
                    }
                });
            }
        }
        self.pending_sync_to_window = false;

        let this = cx.entity();
        let tabs_data: Vec<(usize, String, bool)> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let title = tab.title(cx);
                (tab.id, title, index == self.active_tab)
            })
            .collect();
        let active_content = self.tabs.get(self.active_tab).map(|tab| match &tab.kind {
            TerminalTabKind::Terminal { terminal, .. } => {
                ActiveTabContent::Terminal(terminal.clone())
            }
            TerminalTabKind::Snapshot { snapshot } => ActiveTabContent::Snapshot(snapshot.clone()),
        });

        let tabs_row = tabs_data.into_iter().fold(
            div()
                .w_full()
                .h(CUSTOM_TITLE_BAR_HEIGHT)
                .bg(rgb(0x171a21))
                .window_control_area(WindowControlArea::Drag)
                .flex()
                .items_center()
                .gap_1()
                .child(crate::platform::titlebar_left_gutter(
                    TRAFFIC_LIGHT_LEFT_GUTTER,
                )),
            |row, (tab_id, title, active)| {
                let this = this.clone();
                let bg = if active { rgb(0x252a34) } else { rgb(0x1d222b) };
                let fg = if active { rgb(0xffffff) } else { rgb(0xa9b1c6) };
                row.child(
                    div()
                        .px_3()
                        .py_1()
                        .rounded(px(6.0))
                        .bg(bg)
                        .text_color(fg)
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            this.update(cx, |this, cx| this.activate_tab_by_id(tab_id, window, cx));
                        })
                        .child(title),
                )
            },
        );

        let this = this.clone();
        let tabs_row = if web_connected {
            tabs_row.child(div().px_2().text_color(rgb(0x7ee2b8)).child("● web 已连接"))
        } else {
            tabs_row
        };
        let tabs_row = tabs_row
            .child(
                div()
                    .ml_auto()
                    .px_2()
                    .py_1()
                    .rounded(px(6.0))
                    .bg(rgb(0x1d222b))
                    .text_color(rgb(0xa9b1c6))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        this.update(cx, |this, cx| this.open_new_tab(window, cx));
                    })
                    .child("+"),
            )
            .child(crate::platform::titlebar_right_slot(window, px(8.0)));

        let content = match active_content {
            Some(ActiveTabContent::Terminal(active_terminal)) => {
                div().flex_1().min_h_0().child(active_terminal)
            }
            Some(ActiveTabContent::Snapshot(snapshot)) => div().flex_1().min_h_0().child(snapshot),
            None => div()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(rgb(0xa9b1c6))
                .child("No terminal tabs"),
        };

        div()
            .id("terminal-tabs")
            .size_full()
            .bg(rgb(0x0f1115))
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_capture_snapshot_tab))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_switch_to_tab1))
            .on_action(cx.listener(Self::on_switch_to_tab2))
            .on_action(cx.listener(Self::on_switch_to_tab3))
            .on_action(cx.listener(Self::on_switch_to_tab4))
            .on_action(cx.listener(Self::on_switch_to_tab5))
            .on_action(cx.listener(Self::on_switch_to_tab6))
            .on_action(cx.listener(Self::on_switch_to_tab7))
            .on_action(cx.listener(Self::on_switch_to_tab8))
            .on_action(cx.listener(Self::on_switch_to_tab9))
            .on_action(cx.listener(Self::on_switch_to_tab10))
            .flex()
            .flex_col()
            .child(tabs_row)
            .child(content)
    }
}

enum ActiveTabContent {
    Terminal(Entity<AgentTerminal>),
    Snapshot(Entity<SnapshotTab>),
}

fn next_active_tab_index(active: usize, removed: usize, remaining_len: usize) -> usize {
    debug_assert!(remaining_len > 0);

    if active > removed {
        active - 1
    } else if active >= remaining_len {
        remaining_len - 1
    } else {
        active
    }
}

fn window_sizes_match(a: gpui::Size<gpui::Pixels>, b: gpui::Size<gpui::Pixels>) -> bool {
    // Platform window callbacks can round logical pixels differently. Treat
    // subpixel/one-pixel rounding as the same size, not a user resize.
    (f32::from(a.width) - f32::from(b.width)).abs() <= 1.0
        && (f32::from(a.height) - f32::from(b.height)).abs() <= 1.0
}

fn truncate_tab_title(title: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let char_count = title.chars().count();
    if char_count <= max_chars {
        return title.to_string();
    }

    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }

    let keep_chars = max_chars - 3;
    let truncated: String = title.chars().take(keep_chars).collect();
    format!("{truncated}...")
}

/// Whether a request from `client` must be refused because a different page
/// owns the terminal. A missing client header (agent, curl) is never refused.
fn web_client_should_yield(client: Option<&str>, owner: Option<&str>) -> bool {
    match (client, owner) {
        (Some(candidate), Some(owner)) => candidate != owner,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{next_active_tab_index, truncate_tab_title, web_client_should_yield, window_sizes_match};

    #[test]
    fn nobody_owns_the_terminal_so_every_client_may_drive_it() {
        assert!(!web_client_should_yield(Some("page-a"), None));
        assert!(!web_client_should_yield(None, None));
    }

    #[test]
    fn the_owner_may_drive_the_terminal() {
        assert!(!web_client_should_yield(Some("page-a"), Some("page-a")));
    }

    #[test]
    fn a_second_page_must_yield_to_the_owner() {
        assert!(web_client_should_yield(Some("page-b"), Some("page-a")));
    }

    #[test]
    fn agents_and_curl_without_a_client_header_are_never_gated() {
        assert!(!web_client_should_yield(None, Some("page-a")));
    }

    #[test]
    fn closing_tab_before_active_shifts_active_left() {
        assert_eq!(next_active_tab_index(3, 1, 4), 2);
    }

    #[test]
    fn closing_active_last_tab_selects_previous() {
        assert_eq!(next_active_tab_index(2, 2, 2), 1);
    }

    #[test]
    fn closing_tab_after_active_keeps_active() {
        assert_eq!(next_active_tab_index(1, 3, 4), 1);
    }

    #[test]
    fn window_restore_tolerates_pixel_rounding_but_not_user_resize() {
        let target = gpui::size(gpui::px(800.0), gpui::px(500.0));
        assert!(window_sizes_match(
            target,
            gpui::size(gpui::px(800.5), gpui::px(499.5))
        ));
        assert!(!window_sizes_match(
            target,
            gpui::size(gpui::px(900.0), gpui::px(500.0))
        ));
    }

    #[test]
    fn truncate_tab_title_keeps_short_title() {
        assert_eq!(truncate_tab_title("short", 10), "short");
    }

    #[test]
    fn truncate_tab_title_adds_ellipsis_for_long_title() {
        assert_eq!(
            truncate_tab_title("abcdefghijklmnopqrstuvwxyz", 10),
            "abcdefg..."
        );
    }

    #[test]
    fn truncate_tab_title_handles_small_limits() {
        assert_eq!(truncate_tab_title("abcdef", 3), "...");
        assert_eq!(truncate_tab_title("abcdef", 2), "..");
        assert_eq!(truncate_tab_title("abcdef", 1), ".");
    }
}
