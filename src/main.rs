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
mod web_debug;
mod web_socket;
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

const SINGLETON_LOCK: &str = "~/.rterminal/instance.lock";

struct InstanceGuard {
    path: std::path::PathBuf,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if let Ok(contents) = std::fs::read_to_string(&self.path)
            && contents.trim().parse::<u32>().ok() == Some(std::process::id())
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn acquire_instance_lock() -> Result<InstanceGuard, String> {
    use std::io::Write;

    let path = crate::platform::expand_home(SINGLETON_LOCK)
        .map_err(|err| format!("resolve lock path: {err}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("create {}: {err}", parent.display()))?;
    }

    for _ in 0..2 {
        match std::fs::read_to_string(&path) {
            Ok(contents) => {
                if let Ok(pid) = contents.trim().parse::<u32>()
                    && pid != 0
                    && process_is_alive(pid)
                {
                    return Err(format!("another instance is already running (pid {pid})"));
                }
                let _ = std::fs::remove_file(&path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("read {}: {err}", path.display())),
        }

        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                writeln!(file, "{}", std::process::id())
                    .map_err(|err| format!("write {}: {err}", path.display()))?;
                return Ok(InstanceGuard { path });
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(format!("create {}: {err}", path.display())),
        }
    }
    Err("could not acquire the instance lock".to_string())
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    let filter = format!("PID eq {pid}");
    match std::process::Command::new("tasklist")
        .args(["/FI", &filter, "/NH"])
        .output()
    {
        Ok(output) => String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()),
        Err(_) => false,
    }
}

#[cfg(not(windows))]
fn process_is_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn bind_is_loopback(addr: &str) -> bool {
    let host = if addr.starts_with('[') {
        addr.split(']').next().unwrap_or("").trim_start_matches('[')
    } else {
        addr.rsplit_once(':').map(|(host, _)| host).unwrap_or(addr)
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

fn warn_if_public_bind(name: &str, addr: &str) {
    if !bind_is_loopback(addr) {
        eprintln!(
            "WARNING: {name} is listening on {addr} without authentication; \
             reachable devices can read and control the terminal"
        );
    }
}

fn main() {
    let cli = parse_cli_options();

    if cli.self_check {
        if let Err(err) = run_self_check() {
            eprintln!("self-check failed: {err:#}");
            std::process::exit(1);
        }
        return;
    }

    if cli.cursor_trail && cli.no_cursor_slide {
        eprintln!("warning: --cursor-trail has no effect when --no-cursor-slide is set");
    }
    warn_if_public_bind("HTTP API", &cli.api_addr);
    warn_if_public_bind("WebSocket", &cli.ws_addr);

    let _instance = match acquire_instance_lock() {
        Ok(guard) => guard,
        Err(message) => {
            eprintln!("{message}; only one rterminal instance is allowed");
            std::process::exit(1);
        }
    };

    let (api_tx, api_rx) = async_channel::unbounded::<crate::api_protocol::ApiCommand>();
    if let Err(err) = crate::api_server::start_api_server(&cli.api_addr, api_tx.clone()) {
        eprintln!("failed to start api server on {}: {err:#}", cli.api_addr);
        eprintln!("another instance may already be using this address; exiting.");
        std::process::exit(1);
    }
    let ws_port = match crate::web_socket::start_ws_server(&cli.ws_addr, api_tx) {
        Ok(port) => port,
        Err(err) => {
            eprintln!(
                "failed to start web data stream on {}: {err:#}",
                cli.ws_addr
            );
            std::process::exit(1);
        }
    };
    crate::api_server::set_ws_port(ws_port);

    application().run(move |cx: &mut App| {
        cx.on_action(|_: &QuitApp, cx: &mut App| cx.quit());
        cx.bind_keys(crate::platform::key_bindings());
        cx.set_menus(crate::platform::app_menu());

        let initial_size = cx
            .primary_display()
            .map(|display| {
                let display_size = display.bounds().size;
                size(
                    px(f32::from(display_size.width) * 0.5),
                    px(f32::from(display_size.height) * 0.5),
                )
            })
            .unwrap_or_else(|| size(px(1000.0), px(520.0)));
        let bounds = Bounds::centered(None, initial_size, cx);
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

#[cfg(test)]
mod tests {
    use super::bind_is_loopback;

    #[test]
    fn loopback_bind_detection_handles_ip_and_hostnames() {
        assert!(bind_is_loopback("127.0.0.1:7878"));
        assert!(bind_is_loopback("localhost:7878"));
        assert!(bind_is_loopback("[::1]:7878"));
        assert!(!bind_is_loopback("0.0.0.0:7878"));
        assert!(!bind_is_loopback("192.168.1.10:7878"));
        assert!(!bind_is_loopback("[::]:7878"));
    }
}
