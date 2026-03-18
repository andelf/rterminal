#![allow(unexpected_cfgs)]

mod cli;
mod color;
mod debug_server;
mod input;
mod input_log;
mod keyboard;
mod macos_ax;
mod pty;
mod render;
mod tabs;
mod terminal;
mod text_utils;

use gpui::{
    App, Bounds, KeyBinding, Menu, MenuItem, SystemMenuType, TitlebarOptions, WindowBounds,
    WindowOptions, actions, prelude::*, px, size,
};
use gpui_platform::application;

use crate::cli::parse_cli_options;
pub(crate) use crate::input::AgentTerminalInputHandler;
use crate::tabs::TerminalTabs;
#[allow(unused_imports)]
pub(crate) use crate::terminal::{
    AgentTerminal, CellSnapshot, DEFAULT_FONT_SIZE, GridSize, ScreenSnapshot,
    compute_grid_size, run_self_check, snapshot_to_lines,
};

actions!(
    agent_tui_menu,
    [
        QuitApp,
        NewTab,
        CloseTab,
        NextTab,
        PrevTab,
        SwitchToTab1,
        SwitchToTab2,
        SwitchToTab3,
        SwitchToTab4,
        SwitchToTab5,
        SwitchToTab6,
        SwitchToTab7,
        SwitchToTab8,
        SwitchToTab9,
        SwitchToTab10
    ]
);

fn main() {
    let cli = parse_cli_options();

    if cli.self_check {
        if let Err(err) = run_self_check() {
            eprintln!("self-check failed: {err:#}");
            std::process::exit(1);
        }
        return;
    }

    application().run(move |cx: &mut App| {
        cx.on_action(|_: &QuitApp, cx: &mut App| cx.quit());
        cx.bind_keys([
            KeyBinding::new("cmd-t", NewTab, None),
            KeyBinding::new("cmd-w", CloseTab, None),
            KeyBinding::new("ctrl-tab", NextTab, None),
            KeyBinding::new("ctrl-shift-tab", PrevTab, None),
            KeyBinding::new("cmd-shift-]", NextTab, None),
            KeyBinding::new("cmd-shift-[", PrevTab, None),
            KeyBinding::new("cmd-1", SwitchToTab1, None),
            KeyBinding::new("cmd-2", SwitchToTab2, None),
            KeyBinding::new("cmd-3", SwitchToTab3, None),
            KeyBinding::new("cmd-4", SwitchToTab4, None),
            KeyBinding::new("cmd-5", SwitchToTab5, None),
            KeyBinding::new("cmd-6", SwitchToTab6, None),
            KeyBinding::new("cmd-7", SwitchToTab7, None),
            KeyBinding::new("cmd-8", SwitchToTab8, None),
            KeyBinding::new("cmd-9", SwitchToTab9, None),
            KeyBinding::new("cmd-0", SwitchToTab10, None),
        ]);
        cx.set_menus(vec![Menu {
            name: "Agent Terminal".into(),
            items: vec![
                MenuItem::action("New Tab", NewTab),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::separator(),
                MenuItem::action("Next Tab", NextTab),
                MenuItem::action("Previous Tab", PrevTab),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Agent Terminal", QuitApp),
            ],
        }]);

        let bounds = Bounds::centered(None, size(px(1000.0), px(520.0)), cx);
        let cli = cli.clone();
        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("agent terminal".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            move |window, cx| {
                let cli = cli.clone();
                cx.new(|cx| TerminalTabs::new(window, cx, cli))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
