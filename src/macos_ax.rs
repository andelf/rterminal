#[cfg(target_os = "macos")]
use cocoa::{
    base::{YES, id, nil},
    foundation::{NSRange, NSString, NSUInteger},
};
#[cfg(target_os = "macos")]
use objc::{msg_send, sel, sel_impl};
#[cfg(target_os = "macos")]
use raw_window_handle::RawWindowHandle;
#[cfg(target_os = "macos")]
use std::{ffi::CStr, os::raw::c_char};

use gpui::Window;

use crate::text_utils::{should_accept_ax_override, should_publish_model_to_ax};

#[derive(Clone, Debug)]
pub(crate) struct NativeAxInputState {
    pub(crate) text: String,
    pub(crate) cursor_utf16: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct NativeAxSyncResult {
    pub(crate) override_from_ax: Option<NativeAxInputState>,
    pub(crate) published_model: bool,
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn autoreleased_nsstring(value: &str) -> id {
    unsafe {
        let string = NSString::alloc(nil).init_str(value);
        msg_send![string, autorelease]
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn nsstring_to_rust(value: id) -> Option<String> {
    if value == nil {
        return None;
    }
    unsafe {
        let utf8: *const c_char = msg_send![value, UTF8String];
        if utf8.is_null() {
            return None;
        }
        Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub(crate) fn sync_native_ax_input_view(
    window: &Window,
    input_line: &str,
    cursor_utf16: usize,
    last_published_line: &str,
    last_published_cursor_utf16: usize,
) -> NativeAxSyncResult {
    let window_handle = match raw_window_handle::HasWindowHandle::window_handle(window) {
        Ok(handle) => handle,
        Err(_) => return NativeAxSyncResult::default(),
    };
    let RawWindowHandle::AppKit(handle) = window_handle.as_raw() else {
        return NativeAxSyncResult::default();
    };
    let ns_view = handle.ns_view.as_ptr() as id;
    let mut result = NativeAxSyncResult::default();

    unsafe {
        // Keep the focused native view itself text-readable for tools like voice-correct,
        // which query AXFocusedUIElement -> AXValue/AXSelectedTextRange.
        let text_role = autoreleased_nsstring("AXTextField");
        let text_label = autoreleased_nsstring("Terminal Input Line");
        let identifier = autoreleased_nsstring("agent-terminal-input-line");

        let current_value: id = msg_send![ns_view, accessibilityValue];
        let current_text = nsstring_to_rust(current_value).unwrap_or_default();
        let current_range: NSRange = msg_send![ns_view, accessibilitySelectedTextRange];
        let current_cursor_utf16 =
            (current_range.location as usize).saturating_add(current_range.length as usize);
        let current_cursor_utf16 = current_cursor_utf16.min(current_text.encode_utf16().count());

        let accepting_ax_override = should_accept_ax_override(
            &current_text,
            current_cursor_utf16,
            input_line,
            cursor_utf16,
            last_published_line,
            last_published_cursor_utf16,
        );
        let publishing_model = should_publish_model_to_ax(
            &current_text,
            current_cursor_utf16,
            input_line,
            cursor_utf16,
            accepting_ax_override,
        );

        if accepting_ax_override {
            result.override_from_ax = Some(NativeAxInputState {
                text: current_text,
                cursor_utf16: current_cursor_utf16,
            });
        }

        let _: () = msg_send![ns_view, setAccessibilityElement: YES];
        let _: () = msg_send![ns_view, setAccessibilityRole: text_role];
        let _: () = msg_send![ns_view, setAccessibilityLabel: text_label];
        let _: () = msg_send![ns_view, setAccessibilityIdentifier: identifier];
        if publishing_model {
            let value = autoreleased_nsstring(input_line);
            let cursor_range = NSRange {
                location: cursor_utf16 as NSUInteger,
                length: 0,
            };
            let _: () = msg_send![ns_view, setAccessibilityValue: value];
            let _: () = msg_send![ns_view, setAccessibilitySelectedTextRange: cursor_range];
            result.published_model = true;
        }
    }

    result
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn sync_native_ax_input_view(
    _window: &Window,
    _input_line: &str,
    _cursor_utf16: usize,
    _last_published_line: &str,
    _last_published_cursor_utf16: usize,
) -> NativeAxSyncResult {
    NativeAxSyncResult::default()
}
