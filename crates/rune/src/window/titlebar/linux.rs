//! A Linux window's own frame for `App::titlebar`, where the Wayland compositor draws none.
//!
//! GNOME offers no server-side decorations (`xdg-decoration`), so winit would draw its own grey
//! bar above the app's titlebar. Rune takes that bar away instead: the window opens with winit's
//! frame less its header bar (`WindowAttributesExtWayland::with_headerless_frame`), which keeps
//! the window's shadow, its one-point border, its four rounded corners (the content clears
//! outside them, `window::surface::corners`) and the resize handles outside its edges. Rune
//! draws the caption buttons the desktop's button layout asks for
//! (`org.gnome.desktop.wm.preferences` `button-layout`, through the settings portal) over the
//! app's titlebar. Where that frame can't be made (no subsurfaces, a borderless window, or a
//! compositor that insists on its own decorations), the window is square and resizes from a band
//! inside its edges (`caption::edges`). A compositor that draws the frame itself (KDE, Sway,
//! Hyprland) keeps it, as X11 does.
//!
//! The parts that decide nothing about a live window are plain data, built on every platform so
//! the tests run on all of them.

use std::sync::Mutex;

use super::CaptionButton;

/// Points along the window's edges, inside it, that resize it.
pub(crate) const EDGE: f32 = 5.0;
/// Points from a corner, along either edge, that resize the window diagonally.
pub(crate) const CORNER: f32 = 16.0;

/// The caption buttons the frame shows and the side they sit at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ButtonLayout {
    /// From the titlebar's middle towards the window's edge they sit at, as `Caption::buttons`
    /// lists them: a layout on the left is drawn mirrored (`Caption::mirrored`).
    pub buttons: [Option<CaptionButton>; 3],
    /// At the window's left (in an app that reads left to right).
    pub leading: bool,
}

impl Default for ButtonLayout {
    /// Minimize, maximize and close at the right, when the desktop doesn't say.
    fn default() -> Self {
        Self { buttons: CaptionButton::ALL.map(Some), leading: false }
    }
}

impl ButtonLayout {
    /// GNOME's `button-layout`: the left side's buttons, a colon, the right side's, each a
    /// comma-separated list (`appmenu:close`, `close,minimize,maximize:`). Names that aren't
    /// caption buttons (`appmenu`, `icon`, `spacer`) are skipped, and so is a button's second
    /// mention. The buttons sit together on the side that holds close, else the side with more
    /// of them: a layout that splits them across both sides draws that side's only.
    pub(crate) fn parse(layout: &str) -> Self {
        let side = |names: &str| -> Vec<CaptionButton> {
            names.split(',').filter_map(|name| match name.trim() {
                "minimize" => Some(CaptionButton::Minimize),
                "maximize" => Some(CaptionButton::Maximize),
                "close" => Some(CaptionButton::Close),
                _ => None,
            }).collect()
        };
        let (left, right) = layout.split_once(':').map_or((side(layout), Vec::new()), |(l, r)| (side(l), side(r)));
        let close = |side: &[CaptionButton]| side.contains(&CaptionButton::Close);
        let leading = close(&left) || (!close(&right) && left.len() > right.len());
        // The left side reads from the window's edge inwards.
        let order: Vec<CaptionButton> = if leading { left.into_iter().rev().collect() } else { right };
        let mut buttons = [None; 3];
        let mut n = 0;
        for button in order {
            if !buttons.contains(&Some(button)) {
                buttons[n] = Some(button);
                n += 1;
            }
        }
        Self { buttons, leading }
    }
}

/// Sides of the window: the ones the compositor tiled, or the ones that resize it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sides {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl Sides {
    pub(crate) const NONE: Sides = Sides { top: false, right: false, bottom: false, left: false };
    #[cfg(test)]
    pub(crate) const ALL: Sides = Sides { top: true, right: true, bottom: true, left: true };

    pub(crate) fn any(self) -> bool {
        self.top || self.right || self.bottom || self.left
    }
}

/// What the frame's caption needs of its window (`Caption::own_frame`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowState {
    pub maximized: bool,
    pub full_screen: bool,
    pub resizable: bool,
    /// The compositor lets the window minimize and maximize (`Cx::window_actions`); one that
    /// doesn't say counts as yes.
    pub minimizable: bool,
    pub maximizable: bool,
    /// The sides tiled against a screen edge or another window (`Cx::tiled_edges`).
    pub tiled: Sides,
    /// winit's frame, less its header bar, is around the window: its handles resize the window
    /// from outside its edges, so no band inside them does.
    pub outer_frame: bool,
}

/// The desktop's button layout, once the settings portal answered with one.
static DESKTOP: Mutex<Option<ButtonLayout>> = Mutex::new(None);

/// Notes the desktop's `button-layout`, as the settings portal read or signalled it. Returns
/// whether it is another layout than before: the windows then draw again.
#[cfg_attr(not(feature = "dbus"), allow(dead_code))]
pub(crate) fn set_desktop_layout(layout: &str) -> bool {
    let layout = Some(ButtonLayout::parse(layout));
    let mut desktop = DESKTOP.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::replace(&mut *desktop, layout) != layout
}

/// The layout the desktop asks for, or [`ButtonLayout::default`] where none says (no settings
/// portal, or one without GNOME's key).
pub(crate) fn desktop_layout() -> ButtonLayout {
    let desktop = *DESKTOP.lock().unwrap_or_else(|e| e.into_inner());
    desktop.unwrap_or_default()
}

/// What `RUNE_LINUX_FRAME` asks for: `0` keeps winit's frame, `1` draws Rune's under any
/// Wayland compositor; anything else leaves it to what the compositor offers.
pub(crate) fn forced(value: Option<&str>) -> Option<bool> {
    match value {
        Some("0") => Some(false),
        Some("1") => Some(true),
        _ => None,
    }
}

/// Whether this process's windows with `App::titlebar` get Rune's own frame (`choose`).
static CHOSEN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Whether windows with `App::titlebar` have Rune's own frame, as the first of them found
/// (`choose`); false before any opened.
pub(crate) fn chosen() -> bool {
    CHOSEN.get().copied().unwrap_or(false)
}

/// Whether a window with `App::titlebar` on this display gets Rune's own frame: on Wayland,
/// where the compositor has no `xdg-decoration` to draw one with. Asked once, on winit's own
/// connection, before the first such window is made. X11 and a display that doesn't answer keep
/// the native frame.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
#[allow(unsafe_code)]
pub(crate) fn choose(event_loop: &winit::event_loop::ActiveEventLoop) -> bool {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    use wayland_backend::client::Backend;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::{Connection, Dispatch, QueueHandle};

    struct Probe;
    impl Dispatch<WlRegistry, GlobalListContents> for Probe {
        fn event(_: &mut Self, _: &WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }

    *CHOSEN.get_or_init(|| {
        let Ok(RawDisplayHandle::Wayland(display)) = event_loop.display_handle().map(|d| d.as_raw()) else { return false };
        if let Some(forced) = forced(crate::env::var("RUNE_LINUX_FRAME").as_deref()) { return forced; }
        // SAFETY: winit owns this `wl_display` and keeps it alive for the event loop, which
        // outlives every window; a foreign backend never closes it.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let Ok((globals, _queue)) = registry_queue_init::<Probe>(&Connection::from_backend(backend)) else { return false };
        !globals.contents().with_list(|list| list.iter().any(|global| global.interface == "zxdg_decoration_manager_v1"))
    })
}

#[cfg(test)]
#[path = "linux_tests.rs"]
mod tests;
