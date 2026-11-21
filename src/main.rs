#![allow(unexpected_cfgs)]

mod api_keys;
mod api_protocol;
mod api_server;
mod cli;
mod color;
mod history_log;
mod input;
mod input_log;
mod keyboard;
mod macos_ax;
mod platform;
mod pty;
mod render;
mod snapshot_tab;
mod tab_runtime;
mod tabs;
mod terminal;
mod text_utils;
mod web_ansi;
mod web_ui;

use gpui::{
    App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, actions, prelude::*, px, size,
};
use gpui_platform::application;

use crate::cli::parse_cli_options;
pub(crate) use crate::input::AgentTerminalInputHandler;
use crate::tabs::TerminalTabs;
#[allow(unused_imports)]
pub(crate) use crate::terminal::{
    AgentTerminal, CellSnapshot, DEFAULT_FONT_SIZE, GridSize, ScreenSnapshot, compute_grid_size,
    run_self_check, snapshot_to_lines,
};

actions!(
    agent_tui_menu,
    [
        QuitApp,
        NewTab,
        CaptureSnapshotTab,
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

    let (api_tx, api_rx) = async_channel::unbounded::<crate::api_protocol::ApiCommand>();
    if let Err(err) = crate::api_server::start_api_server(&cli.api_addr, api_tx) {
        eprintln!("failed to start api server on {}: {err:#}", cli.api_addr);
        std::process::exit(1);
    }

    application().run(move |cx: &mut App| {
        cx.on_action(|_: &QuitApp, cx: &mut App| cx.quit());
        cx.bind_keys(crate::platform::key_bindings());
        cx.set_menus(crate::platform::app_menu());

        let bounds = Bounds::centered(None, size(px(1000.0), px(520.0)), cx);
        let cli = cli.clone();
        let api_rx = api_rx.clone();
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
                let api_rx = api_rx.clone();
                cx.new(|cx| TerminalTabs::new(window, cx, cli, api_rx))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
