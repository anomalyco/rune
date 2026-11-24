//! Win32 native window configuration for ordinary windows (`WindowsWindowOptions` and
//! portable `exclude_from_capture`): extended styles (`WS_EX_NOACTIVATE`, `WS_EX_TOOLWINDOW`)
//! and capture exclusion (`SetWindowDisplayAffinity`).

#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos,
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// Computes the updated `GWL_EXSTYLE` mask for `no_activate` and `tool_window`.
///
/// Setting `tool_window` adds `WS_EX_TOOLWINDOW` and clears `WS_EX_APPWINDOW` so the window
/// stays off the taskbar and Alt-Tab list. Unset flags leave existing bits untouched.
pub(crate) fn apply_ex_style(current: u32, no_activate: bool, tool_window: bool) -> u32 {
    let mut next = current;
    if no_activate {
        next |= WS_EX_NOACTIVATE;
    }
    if tool_window {
        next = (next | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
    }
    next
}

/// Applies `WS_EX_NOACTIVATE` / `WS_EX_TOOLWINDOW` to `hwnd` when requested. Does nothing
/// when neither flag is enabled so ordinary windows incur zero Win32 calls.
pub(crate) fn apply_hwnd_styles(hwnd: HWND, no_activate: bool, tool_window: bool) {
    if !no_activate && !tool_window {
        return;
    }
    // SAFETY: `hwnd` is a live top-level window owned by this thread; `GWL_EXSTYLE` updates
    // extended styles and `SetWindowPos` flushes cached frame data without moving, sizing,
    // reordering, or activating the window.
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let next = apply_ex_style(current, no_activate, tool_window);
        if next != current {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next as isize);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

/// Re-applies configured `GWL_EXSTYLE` flags to `window` after winit calls that rewrite
/// extended styles (such as `set_window_level`, `set_cursor_hittest`, maximize, or fullscreen).
pub(crate) fn apply_window_styles(window: &winit::window::Window, options: &crate::WindowsWindowOptions) {
    if !options.no_activate && !options.tool_window {
        return;
    }
    if let Some(hwnd) = super::hwnd(window) {
        apply_hwnd_styles(hwnd, options.no_activate, options.tool_window);
    }
}

/// Sets or clears capture exclusion (`WDA_EXCLUDEFROMCAPTURE` / `WDA_NONE`) on `hwnd`.
/// Returns whether `SetWindowDisplayAffinity` succeeded.
pub(crate) fn set_hwnd_exclude_from_capture(hwnd: HWND, exclude: bool) -> bool {
    let affinity = if exclude { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    // SAFETY: `hwnd` is a live top-level window owned by this thread and `affinity` is a valid
    // `WDA_*` constant.
    unsafe { SetWindowDisplayAffinity(hwnd, affinity) != 0 }
}

/// Sets or clears capture exclusion on `window`. Returns whether `SetWindowDisplayAffinity`
/// succeeded.
pub(crate) fn set_exclude_from_capture(window: &winit::window::Window, exclude: bool) -> bool {
    super::hwnd(window).is_some_and(|hwnd| set_hwnd_exclude_from_capture(hwnd, exclude))
}

/// Applies `WindowsWindowOptions` and portable `exclude_from_capture` to a freshly created
/// `hwnd` before it is shown.
pub(crate) fn configure_hwnd(
    hwnd: HWND,
    options: &crate::WindowsWindowOptions,
    exclude_from_capture: Option<bool>,
) {
    apply_hwnd_styles(hwnd, options.no_activate, options.tool_window);
    if let Some(exclude) = options.effective_exclude_from_capture(exclude_from_capture) {
        let _ = set_hwnd_exclude_from_capture(hwnd, exclude);
    }
}

/// Applies `WindowsWindowOptions` and portable `exclude_from_capture` to `window` while it is
/// still hidden, before its first frame or reveal.
pub(crate) fn configure_window(
    window: &winit::window::Window,
    options: &crate::WindowsWindowOptions,
    exclude_from_capture: Option<bool>,
) {
    let effective_exclude = options.effective_exclude_from_capture(exclude_from_capture);
    if !options.no_activate && !options.tool_window && effective_exclude.is_none() {
        return;
    }
    if let Some(hwnd) = super::hwnd(window) {
        configure_hwnd(hwnd, options, exclude_from_capture);
    }
}

#[cfg(test)]
#[path = "window_options_tests.rs"]
mod tests;
