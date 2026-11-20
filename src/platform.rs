//! OS compatibility layer.
//!
//! Every platform-specific default lives here so the rest of the crate does not
//! accumulate scattered `cfg!` branches. The macOS branches deliberately keep
//! the original behaviour byte-for-byte; Windows gets its own defaults while
//! sharing all terminal, rendering and HTTP API code.
//!
//! Note that the Objective-C/AppKit bridge in `macos_ax.rs` is *not* a UI
//! dependency: GPUI draws the UI itself (`gpui_windows` on Windows). That
//! bridge only exposes the shadow input line as an `AXTextField` for assistive
//! tools, and already has a no-op fallback on non-macOS targets.

use std::path::PathBuf;

use gpui::{KeyBinding, Keystroke, Menu, MenuItem, Pixels, Window, div, px};
#[cfg(target_os = "macos")]
use gpui::SystemMenuType;
#[cfg(target_os = "windows")]
use gpui::{WindowControlArea, prelude::*, rgb};

use crate::{
    CaptureSnapshotTab, CloseTab, NewTab, NextTab, PrevTab, QuitApp, SwitchToTab1, SwitchToTab10,
    SwitchToTab2, SwitchToTab3, SwitchToTab4, SwitchToTab5, SwitchToTab6, SwitchToTab7,
    SwitchToTab8, SwitchToTab9,
};

#[cfg(target_os = "macos")]
const HOME_UNSET_MESSAGE: &str =
    "cannot expand '~' in --history-log-dir because HOME is unset";
#[cfg(not(target_os = "macos"))]
const HOME_UNSET_MESSAGE: &str =
    "cannot expand '~' in --history-log-dir because USERPROFILE (or HOME) is unset";

/// Shell started inside the PTY. On Windows, `RTERMINAL_SHELL` can be used to
/// pick e.g. `pwsh.exe` or a Git Bash `bash.exe`; `powershell.exe` ships with
/// every supported Windows version. Deliberately do not honour `SHELL` here:
/// MSYS/Git Bash exports a POSIX path that Windows cannot spawn.
#[cfg(target_os = "macos")]
pub(crate) fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

#[cfg(target_os = "windows")]
pub(crate) fn default_shell() -> String {
    std::env::var("RTERMINAL_SHELL").unwrap_or_else(|_| "powershell.exe".to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

/// Extra arguments used to start an interactive shell. `-i` is a POSIX shell
/// flag and must not be passed to PowerShell.
#[cfg(target_os = "windows")]
pub(crate) fn shell_args() -> &'static [&'static str] {
    &[]
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn shell_args() -> &'static [&'static str] {
    &["-i"]
}

/// Primary terminal font family.
#[cfg(target_os = "macos")]
pub(crate) fn default_font_family() -> &'static str {
    "Menlo"
}

#[cfg(target_os = "windows")]
pub(crate) fn default_font_family() -> &'static str {
    "Cascadia Mono"
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn default_font_family() -> &'static str {
    "monospace"
}

#[cfg(target_os = "windows")]
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Expand a leading `~` in a CLI path value.
pub(crate) fn expand_home(value: &str) -> Result<PathBuf, String> {
    if let Some(rest) = value.strip_prefix("~/") {
        Ok(home_dir()
            .ok_or_else(|| HOME_UNSET_MESSAGE.to_string())?
            .join(rest))
    } else if value == "~" {
        home_dir().ok_or_else(|| HOME_UNSET_MESSAGE.to_string())
    } else {
        Ok(PathBuf::from(value))
    }
}

/// Select-all chord. macOS keeps the original Cmd+A rule; other platforms use
/// Ctrl+Shift+A so the plain Ctrl+A still reaches the shell as a byte.
#[cfg(target_os = "macos")]
pub(crate) fn is_select_all_shortcut(keystroke: &Keystroke) -> bool {
    let modifiers = keystroke.modifiers;
    modifiers.platform
        && !modifiers.control
        && !modifiers.alt
        && !modifiers.shift
        && !modifiers.function
        && keystroke.key.eq_ignore_ascii_case("a")
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn is_select_all_shortcut(keystroke: &Keystroke) -> bool {
    let modifiers = keystroke.modifiers;
    modifiers.control
        && modifiers.shift
        && !modifiers.alt
        && !modifiers.platform
        && !modifiers.function
        && keystroke.key.eq_ignore_ascii_case("a")
}

/// Copy chord for snapshot tabs (the original macOS rule had no `!shift`).
#[cfg(target_os = "macos")]
pub(crate) fn is_copy_shortcut(keystroke: &Keystroke) -> bool {
    let modifiers = keystroke.modifiers;
    modifiers.platform
        && !modifiers.control
        && !modifiers.alt
        && !modifiers.function
        && keystroke.key.eq_ignore_ascii_case("c")
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn is_copy_shortcut(keystroke: &Keystroke) -> bool {
    let modifiers = keystroke.modifiers;
    modifiers.control
        && modifiers.shift
        && !modifiers.alt
        && !modifiers.platform
        && !modifiers.function
        && keystroke.key.eq_ignore_ascii_case("c")
}

/// Application key bindings. The macOS list is the original one, unchanged.
/// On Windows `cmd` maps to the Windows key (`modifiers.platform`), which the
/// shell never sees and the OS often intercepts, so we use the Ctrl+Shift
/// chords common to Windows terminals instead.
#[cfg(target_os = "macos")]
pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-q", QuitApp, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-shift-s", CaptureSnapshotTab, None),
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
    ]
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("ctrl-shift-q", QuitApp, None),
        KeyBinding::new("ctrl-shift-t", NewTab, None),
        KeyBinding::new("ctrl-shift-s", CaptureSnapshotTab, None),
        KeyBinding::new("ctrl-shift-w", CloseTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new("ctrl-shift-]", NextTab, None),
        KeyBinding::new("ctrl-shift-[", PrevTab, None),
        KeyBinding::new("ctrl-shift-1", SwitchToTab1, None),
        KeyBinding::new("ctrl-shift-2", SwitchToTab2, None),
        KeyBinding::new("ctrl-shift-3", SwitchToTab3, None),
        KeyBinding::new("ctrl-shift-4", SwitchToTab4, None),
        KeyBinding::new("ctrl-shift-5", SwitchToTab5, None),
        KeyBinding::new("ctrl-shift-6", SwitchToTab6, None),
        KeyBinding::new("ctrl-shift-7", SwitchToTab7, None),
        KeyBinding::new("ctrl-shift-8", SwitchToTab8, None),
        KeyBinding::new("ctrl-shift-9", SwitchToTab9, None),
        KeyBinding::new("ctrl-shift-0", SwitchToTab10, None),
    ]
}

/// Application menu. The macOS `Services` submenu is an AppKit concept and is
/// only registered on macOS.
#[cfg(target_os = "macos")]
pub(crate) fn app_menu() -> Vec<Menu> {
    vec![Menu {
        name: "Agent Terminal".into(),
        disabled: false,
        items: vec![
            MenuItem::action("New Tab", NewTab),
            MenuItem::action("Capture Snapshot Tab", CaptureSnapshotTab),
            MenuItem::action("Close Tab", CloseTab),
            MenuItem::separator(),
            MenuItem::action("Next Tab", NextTab),
            MenuItem::action("Previous Tab", PrevTab),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Agent Terminal", QuitApp),
        ],
    }]
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn app_menu() -> Vec<Menu> {
    vec![Menu {
        name: "Agent Terminal".into(),
        disabled: false,
        items: vec![
            MenuItem::action("New Tab", NewTab),
            MenuItem::action("Capture Snapshot Tab", CaptureSnapshotTab),
            MenuItem::action("Close Tab", CloseTab),
            MenuItem::separator(),
            MenuItem::action("Next Tab", NextTab),
            MenuItem::action("Previous Tab", PrevTab),
            MenuItem::separator(),
            MenuItem::action("Quit Agent Terminal", QuitApp),
        ],
    }]
}

/// Space reserved for the macOS traffic lights at the left of a self-drawn
/// title bar. Windows has no traffic lights, so the strip starts at the normal
/// padding instead.
#[cfg(target_os = "windows")]
pub(crate) fn titlebar_left_gutter(_mac_width: Pixels) -> gpui::Div {
    div().w(px(8.0))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn titlebar_left_gutter(mac_width: Pixels) -> gpui::Div {
    div().w(mac_width)
}

/// Right-hand slot of a self-drawn title bar. GPUI's Windows backend hides the
/// native title bar by keeping the top frame in the client area (`WM_NCCALCSIZE`),
/// so the OS caption buttons are not visible and the application must draw its
/// own. Three elements are mapped to `WindowControlArea`, which the backend
/// turns into `HTMINBUTTON` / `HTMAXBUTTON` / `HTCLOSE` hit tests. Segoe MDL2
/// Assets ships with Windows and contains all four glyphs used here.
#[cfg(target_os = "windows")]
pub(crate) fn titlebar_right_slot(window: &Window, _mac_width: Pixels) -> gpui::Div {
    fn caption_button(
        id: &'static str,
        glyph: &'static str,
        area: WindowControlArea,
        close: bool,
    ) -> impl gpui::IntoElement {
        let hover_bg = if close {
            rgb(0xc42b32)
        } else {
            rgb(0x2a2f3a)
        };
        let active_bg = if close {
            rgb(0x9e2228)
        } else {
            rgb(0x353b48)
        };
        div()
            .id(id)
            .w(px(38.0))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .font_family("Segoe MDL2 Assets")
            .text_size(px(10.0))
            .text_color(rgb(0xa9b1c6))
            .hover(move |style| style.bg(hover_bg).text_color(rgb(0xffffff)))
            .active(move |style| style.bg(active_bg).text_color(rgb(0xffffff)))
            .window_control_area(area)
            .child(glyph)
    }

    div()
        .flex()
        .h_full()
        .items_center()
        .child(caption_button(
            "window-minimize",
            "\u{e921}",
            WindowControlArea::Min,
            false,
        ))
        .child(caption_button(
            "window-zoom",
            if window.is_maximized() {
                "\u{e923}"
            } else {
                "\u{e922}"
            },
            WindowControlArea::Max,
            false,
        ))
        .child(caption_button(
            "window-close",
            "\u{e8bb}",
            WindowControlArea::Close,
            true,
        ))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn titlebar_right_slot(_window: &Window, mac_width: Pixels) -> gpui::Div {
    div().w(mac_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_expands_tilde() {
        let expanded = expand_home("~/.rterminal/history").expect("home directory available");
        assert!(expanded.ends_with(".rterminal/history"));
    }

    #[test]
    fn plain_paths_pass_through() {
        assert_eq!(
            expand_home("/tmp/agent-history").unwrap(),
            PathBuf::from("/tmp/agent-history")
        );
    }

    #[test]
    fn shell_args_match_shell_flavour() {
        #[cfg(target_os = "windows")]
        assert!(shell_args().is_empty(), "PowerShell must not receive -i");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(shell_args(), &["-i"]);
    }

    #[test]
    fn key_bindings_are_registered() {
        assert!(!key_bindings().is_empty());
        assert!(!app_menu().is_empty());
    }
}
