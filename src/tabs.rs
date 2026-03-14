use gpui::{
    Context, Entity, MouseButton, Render, Subscription, Window, WindowControlArea, div, prelude::*,
    px, rgb,
};

use crate::cli::CliOptions;
use crate::render::CUSTOM_TITLE_BAR_HEIGHT;
use crate::terminal::{AgentTerminal, TerminalExitedEvent};

const TRAFFIC_LIGHT_LEFT_GUTTER: gpui::Pixels = px(68.0);

struct TerminalTab {
    id: usize,
    terminal: Entity<AgentTerminal>,
    _exit_subscription: Subscription,
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
    pending_focus_sync: bool,
}

impl TerminalTabs {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>, cli: CliOptions) -> Self {
        let mut this = Self {
            cli,
            tabs: Vec::new(),
            active_tab: 0,
            next_tab_id: 1,
            pending_focus_sync: false,
        };

        this.open_new_tab(window, cx);
        this
    }

    fn open_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            terminal,
            _exit_subscription: exit_subscription,
        });
        self.active_tab = self.tabs.len().saturating_sub(1);
        self.request_focus_active_tab(window, cx);
        cx.notify();
    }

    fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            cx.quit();
            return;
        }

        self.close_tab_at_index(self.active_tab, cx);
        if self.tabs.is_empty() {
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
            cx.quit();
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
            tab.terminal.update(cx, |terminal, cx| {
                window.focus(&terminal.focus_handle, cx);
            });
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
}

impl Render for TerminalTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_focus_sync {
            self.pending_focus_sync = false;
            self.request_focus_active_tab(window, cx);
        }

        let this = cx.entity();
        let tabs_data: Vec<(usize, String, bool)> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let title = tab.terminal.read(cx).tab_title();
                (tab.id, title, index == self.active_tab)
            })
            .collect();
        let active_terminal = self
            .tabs
            .get(self.active_tab)
            .map(|tab| tab.terminal.clone());

        let tabs_row = tabs_data.into_iter().fold(
            div()
                .w_full()
                .h(CUSTOM_TITLE_BAR_HEIGHT)
                .bg(rgb(0x171a21))
                .window_control_area(WindowControlArea::Drag)
                .flex()
                .items_center()
                .gap_1()
                .child(div().w(TRAFFIC_LIGHT_LEFT_GUTTER)),
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
            .child(div().w(px(8.0)));

        let content = if let Some(active_terminal) = active_terminal {
            div().flex_1().min_h_0().child(active_terminal)
        } else {
            div()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(rgb(0xa9b1c6))
                .child("No terminal tabs")
        };

        div()
            .id("terminal-tabs")
            .size_full()
            .bg(rgb(0x0f1115))
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_close_tab))
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

#[cfg(test)]
mod tests {
    use super::next_active_tab_index;

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
}
