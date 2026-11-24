//! The window's controls over the app's titlebar: the macOS traffic lights and the Windows
//! custom frame (`App::titlebar`), and what they take of it (`Cx::titlebar_inset`,
//! `Cx::chrome_insets`, the caption buttons drawn over the view).
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

#[cfg(not(target_os = "macos"))]
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::window::Window;

use crate::runtime::RuntimeCore;
use crate::tooling::Chrome;
use crate::window::surface::Win;

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(in crate::window) mod float_on_open;
#[cfg(target_os = "macos")]
pub(in crate::window) mod mac;
pub(in crate::window) mod platform_sync;
#[cfg(test)]
mod tests;

pub(in crate::window) use platform_sync::sync_platform_core;

/// Native titlebar behaviour the framework owns: traffic-light placement, drag regions and the
/// double-click action. Elsewhere only the portable parts apply.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
pub(in crate::window) fn uses_drag_regions(chrome: Chrome, borderless: bool, panel: bool) -> bool {
    chrome.drag_regions && (borderless || chrome.full_size_content) && !panel
}

/// Places the traffic lights and returns `Cx::titlebar_inset`.
pub(in crate::window) fn place_chrome(window: &Window, chrome: Chrome) -> f32 {
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

/// The frame the window draws for the app's `chrome` and `titlebar` as the view `set` it
/// (`Cx::set_titlebar`, `Cx::titlebar_height`): on macOS a titlebar is a full-size content
/// view with the traffic lights where the titlebar or the app placed them, else in the band's
/// middle.
pub(in crate::window) fn frame_chrome(chrome: Chrome, titlebar: Option<crate::Titlebar>, set: crate::titlebar::TitlebarSet) -> Chrome {
    #[cfg(target_os = "macos")]
    return titlebar.map(|t| set.over(t)).map_or(chrome, |t| t.mac_chrome(chrome));
    #[cfg(not(target_os = "macos"))]
    { let _ = (titlebar, set); chrome }
}

/// Places the traffic lights, and tells the runtime what the window's controls take of the
/// titlebar and which caption buttons to draw over it. A change re-runs the view.
pub(in crate::window) fn sync_chrome_core(rt: &mut RuntimeCore, w: &Win, chrome: Chrome, titlebar: Option<crate::Titlebar>) {
    let resolved = titlebar.map(|t| rt.titlebar_set().over(t));
    #[cfg(target_os = "macos")]
    if let Some(t) = resolved {
        crate::platform::macos::traffic_lights::set_hidden(&w.window, !t.has_controls());
    }
    let leading = place_chrome(&w.window, frame_chrome(chrome, titlebar, rt.titlebar_set()));
    rt.set_titlebar_inset(leading);
    let caption = caption(w, resolved);
    let scale = w.window.scale_factor() as f32;
    let height = resolved.filter(|_| custom_frame(w)).map_or(0.0, |t| t.height());
    let buttons = caption.map_or(0.0, |c| c.width(scale));
    // The traffic lights' room is on the right in a right-to-left app on macOS, as the caption
    // buttons' is on the left on Windows.
    let (left, right) = if lights_right(w) { (0.0, leading) } else { (leading, 0.0) };
    let (left, right) = if caption.is_some_and(|c| c.mirrored) { (left + buttons, right) } else { (left, right + buttons) };
    rt.set_chrome_insets(crate::ChromeInsets::new(left, right, height));
    rt.set_caption(caption);
    rt.set_maximized(w.is_maximized());
    rt.set_full_screen(w.is_full_screen());
}

/// The traffic lights sit at the window's right: a right-to-left app on macOS.
fn lights_right(w: &Win) -> bool {
    #[cfg(target_os = "macos")]
    return mac::traffic_lights_right(&w.window);
    #[cfg(not(target_os = "macos"))]
    { let _ = w; false }
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

/// The caption buttons a custom frame draws for `titlebar`; hidden in full screen or when
/// `Titlebar::controls(false)` is set.
fn caption(w: &Win, titlebar: Option<crate::Titlebar>) -> Option<crate::titlebar::Caption> {
    #[cfg(target_os = "windows")]
    return w.native.frame.as_ref().map(|f| {
        if let Some(t) = titlebar {
            f.set_height(t.height());
            f.set_controls(t.has_controls());
        }
        f.caption(&w.window)
    });
    #[cfg(not(target_os = "windows"))]
    { let _ = (w, titlebar); None }
}

/// Which native target carries out a titlebar action. Adopted macOS panels drag the `NSPanel`
/// directly and ignore `DoubleClick` so no titlebar action can reach winit's hidden source
/// `NSWindow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum TitlebarTarget {
    DragPanel,
    DragWindow,
    DoubleClickWindow,
    Ignore,
}

/// Selects the native target for a titlebar action based on whether the window is backed by an
/// adopted macOS `NSPanel`.
pub(in crate::window) fn titlebar_target(action: crate::runtime::TitlebarAction, panel: bool) -> TitlebarTarget {
    match (action, panel) {
        (crate::runtime::TitlebarAction::Drag, true) => TitlebarTarget::DragPanel,
        (crate::runtime::TitlebarAction::Drag, false) => TitlebarTarget::DragWindow,
        (crate::runtime::TitlebarAction::DoubleClick, true) => TitlebarTarget::Ignore,
        (crate::runtime::TitlebarAction::DoubleClick, false) => TitlebarTarget::DoubleClickWindow,
    }
}

/// A double click on a titlebar: AppKit's action on macOS, else maximize or restore, as the
/// maximize button would. Adopted macOS panels ignore it.
pub(in crate::window) fn titlebar_double_click(window: &Window) {
    #[cfg(target_os = "macos")]
    mac::titlebar_double_click(window);
    #[cfg(not(target_os = "macos"))]
    toggle_maximized(window);
}

/// Maximizes the window, or restores it when it is maximized. A window without a maximize
/// button (one that isn't resizable) only restores. Adopted macOS panels ignore it.
pub(in crate::window) fn toggle_maximized(window: &Window) {
    #[cfg(target_os = "macos")]
    if crate::platform::macos::is_panel(window) { return; }
    let maximized = window.is_maximized();
    if maximized || window.enabled_buttons().contains(winit::window::WindowButtons::MAXIMIZE) {
        window.set_maximized(!maximized);
    }
}

/// Shows a window that opens maximized or full screen that way (`App::maximized`,
/// `App::full_screen`): maximized first, so leaving full screen returns to it.
#[cfg(target_os = "windows")]
pub(in crate::window) fn open_as(window: &Window, open_as: crate::windows::OpenAs) {
    if open_as.maximizes() { window.set_maximized(true); }
    if open_as.full_screens() { window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None))); }
}

/// Sets the window's full-screen state (`true` for borderless full screen over its display,
/// `false` to exit). Adopted macOS panels ignore it.
pub(in crate::window) fn set_full_screen(window: &Window, full_screen: bool) {
    #[cfg(target_os = "macos")]
    if crate::platform::macos::is_panel(window) { return; }
    window.set_fullscreen(full_screen.then_some(winit::window::Fullscreen::Borderless(None)));
}

/// Takes the window full screen, borderless over its display, or brings it back. Adopted
/// macOS panels ignore it.
pub(in crate::window) fn toggle_full_screen(window: &Window) {
    #[cfg(target_os = "macos")]
    if crate::platform::macos::is_panel(window) { return; }
    set_full_screen(window, window.fullscreen().is_none());
}

/// The window menu, where a custom titlebar (at `at`, the click's point in window points, else
/// the pointer) or the keyboard opens it.
pub(in crate::window) fn window_menu(window: &Window, menu: crate::runtime::WindowMenu, at: Option<(f32, f32)>) {
    #[cfg(target_os = "windows")]
    {
        let scale = window.scale_factor();
        let at = at.zip(window.inner_position().ok()).map(|((x, y), o)| (o.x + (f64::from(x) * scale).round() as i32, o.y + (f64::from(y) * scale).round() as i32));
        crate::platform::win32::window_menu(window, menu, at);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (window, menu, at);
}

/// Whether Alt+Space opens this window's window menu when no handler takes it.
pub(in crate::window) fn has_window_menu(window: &Window) -> bool {
    #[cfg(target_os = "windows")]
    return crate::platform::win32::has_window_menu(window);
    #[cfg(not(target_os = "windows"))]
    { let _ = window; false }
}

/// Sizes the window's content to `size` points. Returns the applied physical size when the
/// platform resizes immediately without sending `WindowEvent::Resized` (Wayland).
#[cfg(not(target_os = "macos"))]
pub(in crate::window) fn request_inner_size(window: &Window, size: (f32, f32)) -> Option<PhysicalSize<u32>> {
    #[cfg(target_os = "windows")]
    if crate::platform::win32::request_inner_size(window, size) { return None; }
    window.request_inner_size(LogicalSize::new(size.0, size.1))
}
