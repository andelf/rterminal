#![allow(unexpected_cfgs)]

mod cli;
mod color;
mod debug_server;
mod input;
mod keyboard;
mod macos_ax;
mod pty;
mod render;
mod terminal;
mod text_utils;

use gpui::{
    App, Bounds, Menu, MenuItem, SystemMenuType, TitlebarOptions, WindowBounds, WindowOptions,
    actions, prelude::*, px, size,
};
use gpui_platform::application;

use crate::cli::parse_cli_options;
pub(crate) use crate::input::AgentTerminalInputHandler;
#[allow(unused_imports)]
pub(crate) use crate::terminal::{
    AgentTerminal, CellSnapshot, GridSize, ScreenSnapshot, cell_display_width_cols,
    compute_grid_size, run_self_check, snapshot_to_lines, DEFAULT_FONT_SIZE,
};

actions!(agent_tui_menu, [QuitApp]);

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
        cx.set_menus(vec![Menu {
            name: "Agent Terminal".into(),
            items: vec![
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
                cx.new(|cx| AgentTerminal::new(window, cx, cli))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
