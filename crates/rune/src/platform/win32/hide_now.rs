//! Hides a window at once, without DWM's fade, and returns once DWM has composed a frame without
//! it. A captioned window hidden the ordinary way fades out over several frames, so a screen grab
//! taken right after `ShowWindow(SW_HIDE)` still sees it, whole or half faded. Turning off its
//! transitions (`DWMWA_TRANSITIONS_FORCEDISABLED`) for the hide drops it in one frame, and
//! `DwmFlush` waits for that frame.

#![allow(unsafe_code)]

use windows_sys::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmFlush, DwmSetWindowAttribute};
use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};

/// Hides `window` with no transition and waits for DWM to compose without it. False when it has
/// no HWND.
pub(crate) fn hide_now(window: &winit::window::Window) -> bool {
    let Some(hwnd) = super::hwnd(window) else { return false };
    let set = |disabled: bool| {
        let value = i32::from(disabled);
        // SAFETY: DWMWA_TRANSITIONS_FORCEDISABLED reads one BOOL from `value` for a live window.
        unsafe { DwmSetWindowAttribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED as u32, (&raw const value).cast(), 4) };
    };
    set(true);
    // SAFETY: plain Win32 calls on a live window; DwmFlush takes no arguments.
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
        DwmFlush();
    }
    // Rune sets no transition override elsewhere, so the default comes back for later shows.
    set(false);
    true
}
