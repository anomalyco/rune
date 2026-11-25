//! The window's controls over the app's titlebar: the macOS traffic lights, the Windows custom
//! frame and Rune's own frame on Linux (`App::titlebar`), and what they take of it
//! (`Cx::titlebar_inset`, `Cx::chrome_insets`, the caption buttons drawn over the view).
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

#[cfg(not(target_os = "macos"))]
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::window::Window;

use crate::runtime::RuntimeCore;
use crate::tooling::Chrome;
use crate::window::surface::Win;

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(in crate::window) mod float_on_open;
pub(in crate::window) mod full_screen;
#[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
pub(in crate::window) mod linux_wm_state;
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
    { place_chrome_for(window, chrome, mac::initial_full_screen_state(window)) }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, chrome);
        0.0
    }
}

#[cfg(target_os = "macos")]
fn place_chrome_for(window: &Window, chrome: Chrome, state: full_screen::State) -> f32 {
    if state.is_full_screen() {
        return 0.0;
    }
    if let Some(p) = chrome.traffic_lights {
        crate::platform::macos::traffic_lights::place(window, p);
    }
    mac::titlebar_inset(window, state)
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
    #[cfg(target_os = "macos")]
    w.sync_full_screen();
    let full_screen = w.is_full_screen();
    let resolved = titlebar.map(|t| rt.titlebar_set().over(t));
    #[cfg(target_os = "macos")]
    if let Some(t) = resolved && (!full_screen || !t.has_controls()) {
        crate::platform::macos::traffic_lights::set_hidden(&w.window, !t.has_controls());
    }
    let frame = frame_chrome(chrome, titlebar, rt.titlebar_set());
    #[cfg(target_os = "macos")]
    let leading = place_chrome_for(&w.window, frame, w.full_screen_state());
    #[cfg(not(target_os = "macos"))]
    let leading = place_chrome(&w.window, frame);
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
    rt.set_full_screen(full_screen);
    let _ = sync_wm_state_core(rt, w);
    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    rt.set_window_menu_trigger(resolve_window_menu_trigger(false, None, cfg!(target_os = "windows") && caption.is_some()));
}

/// Reads platform window-manager state (decoration mode, tiled edges, allowed actions) and updates
/// `rt`, returning whether the view was dirtied. Wayland reads `xdg_toplevel` / `xdg-decoration`
/// configure state; X11 queries EWMH / `_GTK_EDGE_CONSTRAINTS` properties; macOS and Windows
/// honestly leave the state at its default unknown values. On Linux this also updates the titlebar
/// secondary-click window-menu trigger from the same state read.
pub(in crate::window) fn sync_wm_state_core(rt: &mut RuntimeCore, w: &Win) -> bool {
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    {
        let wayland = is_wayland_window(&w.window);
        let state = if wayland {
            linux_wm_state::read_wayland(&w.window)
        } else {
            crate::platform::x11::wm_state::read(&w.window, rt.window_manager_state().decoration_mode)
        };
        rt.set_window_menu_trigger(resolve_window_menu_trigger(wayland, state.actions.window_menu, false));
        rt.sync_window_manager_state(state)
    }
    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    {
        let _ = (rt, w);
        false
    }
}

/// The traffic lights sit at the window's right: a right-to-left app on macOS.
fn lights_right(w: &Win) -> bool {
    #[cfg(target_os = "macos")]
    return mac::traffic_lights_right(&w.window);
    #[cfg(not(target_os = "macos"))]
    { let _ = w; false }
}

/// Whether Rune replaced the native title bar: macOS draws the app's titlebar under its
/// transparent one; Windows has the custom frame, and Linux Rune's own where the Wayland
/// compositor draws none (`titlebar::linux`).
fn custom_frame(w: &Win) -> bool {
    #[cfg(target_os = "windows")]
    return w.native.frame.is_some();
    #[cfg(target_os = "macos")]
    { let _ = w; true }
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    { let _ = w; crate::titlebar::linux::chosen() }
    #[cfg(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))))]
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
    // Rune's own frame: the desktop's buttons, and resize edges while the window floats.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    return titlebar.filter(|_| crate::titlebar::linux::chosen()).map(|t| {
        // What the compositor allows and which sides it tiled, as `Cx::window_actions` and
        // `Cx::tiled_edges` report them.
        let wm = linux_wm_state::read_wayland(&w.window);
        let tiled = wm.tiled_edges.unwrap_or_default();
        let window = crate::titlebar::linux::WindowState {
            maximized: w.is_maximized(),
            full_screen: w.is_full_screen(),
            resizable: w.window.is_resizable(),
            minimizable: wm.actions.minimize != Some(false),
            maximizable: wm.actions.maximize != Some(false),
            tiled: crate::titlebar::linux::Sides { top: tiled.top, right: tiled.right, bottom: tiled.bottom, left: tiled.left },
        };
        let caption = crate::titlebar::Caption::own_frame(t.height(), crate::titlebar::linux::desktop_layout(), window);
        crate::titlebar::Caption { hidden: caption.hidden || !t.has_controls(), ..caption }
    });
    #[cfg(not(any(target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
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

/// The window menu, where a custom titlebar (at `at`, the click's point in window points, else
/// the pointer) or the keyboard opens it.
pub(in crate::window) fn window_menu(window: &Window, menu: crate::runtime::WindowMenu, at: Option<(f32, f32)>) {
    #[cfg(target_os = "windows")]
    {
        let scale = window.scale_factor();
        let at = at.zip(window.inner_position().ok()).map(|((x, y), o)| (o.x + (f64::from(x) * scale).round() as i32, o.y + (f64::from(y) * scale).round() as i32));
        crate::platform::win32::window_menu(window, menu, at);
    }
    // Wayland: `xdg_toplevel.show_window_menu`, at the click or the window's top-left corner.
    // X11: `_GTK_SHOW_WINDOW_MENU` when the window manager advertises it, at the click, the
    // pointer, or the window's top-left corner.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    {
        use crate::platform::x11::window_menu::{At, show_window_menu};
        use crate::runtime::WindowMenu;
        if is_wayland_window(window) {
            let (x, y) = match menu { WindowMenu::Keyboard => (0.0, 0.0), WindowMenu::Pointer => at.unwrap_or((0.0, 0.0)) };
            window.show_window_menu(winit::dpi::LogicalPosition::new(f64::from(x.max(0.0)), f64::from(y.max(0.0))));
            return;
        }
        let scale = window.scale_factor();
        let at = match (menu, at) {
            (WindowMenu::Keyboard, _) => At::Window(0, 0),
            (WindowMenu::Pointer, Some((x, y))) => At::Window((f64::from(x) * scale).round() as i32, (f64::from(y) * scale).round() as i32),
            (WindowMenu::Pointer, None) => At::Pointer,
        };
        show_window_menu(window, at);
    }
    #[cfg(not(any(target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
    let _ = (window, menu, at);
}

/// Resolves the titlebar secondary-click window-menu trigger from platform capabilities:
/// Wayland opens on press (for `xdg_toplevel.show_window_menu`'s press serial) unless the
/// compositor explicitly reported `window_menu == Some(false)`; X11 opens on release when
/// `WindowActions::window_menu` is `Some(true)` (`_GTK_SHOW_WINDOW_MENU` in `_NET_SUPPORTED`,
/// after X11's implicit pointer grab ends); Windows custom frames open on release (`WM_NCRBUTTONUP`).
pub(in crate::window) const fn resolve_window_menu_trigger(
    wayland: bool,
    wm_window_menu: Option<bool>,
    windows_caption: bool,
) -> crate::runtime::WindowMenuTrigger {
    if wayland {
        if matches!(wm_window_menu, Some(false)) {
            crate::runtime::WindowMenuTrigger::None
        } else {
            crate::runtime::WindowMenuTrigger::OnPress
        }
    } else if matches!(wm_window_menu, Some(true)) || windows_caption {
        crate::runtime::WindowMenuTrigger::OnRelease
    } else {
        crate::runtime::WindowMenuTrigger::None
    }
}

/// Whether Alt+Space opens this window's window menu when no handler takes it: on Wayland every
/// toplevel can ask; on X11, when the window manager advertises `_GTK_SHOW_WINDOW_MENU`.
pub(in crate::window) fn has_window_menu(window: &Window) -> bool {
    #[cfg(target_os = "windows")]
    return crate::platform::win32::has_window_menu(window);
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    return is_wayland_window(window) || crate::platform::x11::window_menu::has_window_menu(window);
    #[cfg(not(any(target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
    { let _ = window; false }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
fn is_wayland_window(window: &Window) -> bool {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    matches!(window.window_handle().map(|h| h.as_raw()), Ok(RawWindowHandle::Wayland(_)))
}

/// Sizes the window's content to `size` points. Returns the applied physical size when the
/// platform resizes immediately without sending `WindowEvent::Resized` (Wayland).
#[cfg(not(target_os = "macos"))]
pub(in crate::window) fn request_inner_size(window: &Window, size: (f32, f32)) -> Option<PhysicalSize<u32>> {
    #[cfg(target_os = "windows")]
    if crate::platform::win32::request_inner_size(window, size) { return None; }
    window.request_inner_size(LogicalSize::new(size.0, size.1))
}
