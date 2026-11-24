use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GWL_EXSTYLE, GetWindowDisplayAffinity, GetWindowLongPtrW,
    IsWindowVisible, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::{apply_ex_style, configure_hwnd, set_hwnd_exclude_from_capture};
use crate::WindowsWindowOptions;

/// A top-level Win32 window created without `WS_VISIBLE` and never shown.
struct HiddenHwnd(HWND);

#[allow(unsafe_code)]
impl HiddenHwnd {
    fn new(ex_style: u32) -> Self {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: system STATIC class with no parent, menu, or WS_VISIBLE; never shown and
        // destroyed in Drop on the same thread.
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                64,
                64,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "CreateWindowExW failed");
        // SAFETY: `hwnd` is a live window just created by `CreateWindowExW` on this thread.
        let visible = unsafe { IsWindowVisible(hwnd) };
        assert_eq!(visible, 0, "test window must remain hidden");
        Self(hwnd)
    }

    fn ex_style(&self) -> u32 {
        // SAFETY: `self.0` is a live window owned by this thread.
        unsafe { GetWindowLongPtrW(self.0, GWL_EXSTYLE) as u32 }
    }

    fn display_affinity(&self) -> u32 {
        let mut affinity = u32::MAX;
        // SAFETY: `self.0` is a live top-level window owned by this thread; `affinity` is a valid u32 out-parameter.
        let ok = unsafe { GetWindowDisplayAffinity(self.0, &mut affinity) };
        assert_ne!(ok, 0, "GetWindowDisplayAffinity failed");
        affinity
    }
}

#[allow(unsafe_code)]
impl Drop for HiddenHwnd {
    fn drop(&mut self) {
        // SAFETY: destroys the test window created on this thread.
        unsafe { DestroyWindow(self.0) };
    }
}

#[test]
fn apply_ex_style_toggles_only_requested_bits_and_clears_appwindow_for_tool_windows() {
    let base = WS_EX_APPWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP;
    assert_eq!(apply_ex_style(base, false, false), base);
    assert_eq!(apply_ex_style(base, true, false), base | WS_EX_NOACTIVATE);
    assert_eq!(
        apply_ex_style(base, false, true),
        (base | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW
    );
    assert_eq!(
        apply_ex_style(base, true, true),
        (base | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW
    );
}

#[test]
fn configure_hwnd_default_options_leave_styles_and_affinity_unchanged() {
    let win = HiddenHwnd::new(WS_EX_APPWINDOW);
    let before = win.ex_style();
    configure_hwnd(win.0, &WindowsWindowOptions::default(), None);
    assert_eq!(win.ex_style(), before);
    assert_eq!(win.display_affinity(), WDA_NONE);
}

#[test]
fn configure_hwnd_applies_no_activate_tool_window_and_capture_exclusion() {
    let win = HiddenHwnd::new(WS_EX_APPWINDOW);
    let opts = WindowsWindowOptions::default()
        .no_activate(true)
        .tool_window(true)
        .exclude_from_capture(true);
    configure_hwnd(win.0, &opts, None);

    let ex = win.ex_style();
    assert_ne!(ex & WS_EX_NOACTIVATE, 0, "WS_EX_NOACTIVATE should be set");
    assert_ne!(ex & WS_EX_TOOLWINDOW, 0, "WS_EX_TOOLWINDOW should be set");
    assert_eq!(ex & WS_EX_APPWINDOW, 0, "WS_EX_APPWINDOW should be cleared for tool_window");
    assert_eq!(win.display_affinity(), WDA_EXCLUDEFROMCAPTURE);
    // SAFETY: `win.0` is a live window owned by this thread.
    #[allow(unsafe_code)]
    let visible = unsafe { IsWindowVisible(win.0) };
    assert_eq!(visible, 0, "configuring styles must not show the hidden window");
}

#[test]
fn set_hwnd_exclude_from_capture_transitions_between_excluded_and_none() {
    let win = HiddenHwnd::new(0);
    assert_eq!(win.display_affinity(), WDA_NONE);
    assert!(set_hwnd_exclude_from_capture(win.0, true));
    assert_eq!(win.display_affinity(), WDA_EXCLUDEFROMCAPTURE);
    assert!(set_hwnd_exclude_from_capture(win.0, false));
    assert_eq!(win.display_affinity(), WDA_NONE);
    assert!(set_hwnd_exclude_from_capture(win.0, true));
    assert_eq!(win.display_affinity(), WDA_EXCLUDEFROMCAPTURE);
}

#[test]
fn configure_hwnd_portable_exclude_from_capture_overrides_windows_option() {
    let win = HiddenHwnd::new(0);
    let opts = WindowsWindowOptions::default().exclude_from_capture(false);
    configure_hwnd(win.0, &opts, Some(true));
    assert_eq!(win.display_affinity(), WDA_EXCLUDEFROMCAPTURE);

    let opts_true = WindowsWindowOptions::default().exclude_from_capture(true);
    configure_hwnd(win.0, &opts_true, Some(false));
    assert_eq!(win.display_affinity(), WDA_NONE);
}
