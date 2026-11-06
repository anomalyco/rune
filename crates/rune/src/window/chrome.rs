//! The window's controls over the app's titlebar: the macOS traffic lights and the Windows
//! custom frame (`App::titlebar`), and what they take of it (`Cx::titlebar_inset`,
//! `Cx::chrome_insets`, the caption buttons drawn over the view).
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use super::*;

/// Places the traffic lights and returns `Cx::titlebar_inset`.
pub(super) fn place_chrome(window: &Window, chrome: Chrome) -> f32 {
    #[cfg(target_os = "macos")]
    {
        if let Some(p) = chrome.traffic_lights {
            crate::platform::macos::traffic_lights::place(window, p);
        }
        mac::titlebar_inset(window)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, chrome);
        0.0
    }
}

/// Places the traffic lights, and tells the runtime what the window's controls take of the
/// titlebar and which caption buttons to draw over it. Returns whether the view must re-run.
pub(super) fn sync_chrome<S: 'static>(rt: &mut Runtime<S>, w: &Win, chrome: Chrome, titlebar: Option<crate::Titlebar>) -> bool {
    let leading = place_chrome(&w.window, chrome);
    rt.set_titlebar_inset(leading);
    let caption = caption(w);
    let scale = w.window.scale_factor() as f32;
    let height = titlebar.filter(|_| custom_frame(w)).map_or(0.0, |t| t.height());
    let insets = crate::ChromeInsets::new(leading, caption.map_or(0.0, |c| c.width(scale)), height);
    let before = rt.chrome_insets();
    rt.set_chrome_insets(insets);
    rt.set_caption(caption) || before != insets
}

/// Whether Rune replaced the native title bar: macOS draws the app's titlebar under its
/// transparent one; Windows has the custom frame.
fn custom_frame(w: &Win) -> bool {
    #[cfg(target_os = "windows")]
    return w.native.frame.is_some();
    #[cfg(target_os = "macos")]
    { let _ = w; true }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    { let _ = w; false }
}

/// The caption buttons a custom frame draws, out of full screen.
fn caption(w: &Win) -> Option<crate::titlebar::Caption> {
    #[cfg(target_os = "windows")]
    return w.native.frame.as_ref().filter(|_| w.window.fullscreen().is_none()).map(|f| f.caption(&w.window));
    #[cfg(not(target_os = "windows"))]
    { let _ = w; None }
}

pub(super) fn titlebar_double_click(window: &Window) {
    #[cfg(target_os = "macos")]
    mac::titlebar_double_click(window);
    #[cfg(not(target_os = "macos"))]
    window.set_maximized(!window.is_maximized());
}

/// The window menu at the pointer, where a custom titlebar has one to open.
pub(super) fn system_menu(window: &Window) {
    #[cfg(target_os = "windows")]
    crate::platform::win32::system_menu(window);
    #[cfg(not(target_os = "windows"))]
    let _ = window;
}

/// Sizes the window's content to `size` points.
pub(super) fn request_inner_size(window: &Window, size: (f32, f32)) {
    #[cfg(target_os = "windows")]
    if crate::platform::win32::request_inner_size(window, size) { return; }
    let _ = window.request_inner_size(LogicalSize::new(size.0, size.1));
}
