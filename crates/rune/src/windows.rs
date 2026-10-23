//! Window descriptions shared by the native shell and deterministic headless selection.
use crate::{App, AppSpec};

/// Process-level settings applied once, before constructing the native event loop.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunOptions {
    /// macOS: no Dock icon/default menu and no activation at launch. Ignored elsewhere.
    pub accessory: bool,
}

/// A display in desktop physical-pixel coordinates (origins may be negative).
/// Headless runs provide one deterministic virtual display.
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    /// Native identity for overlay targeting; absent for the headless virtual display.
    pub id: Option<crate::DisplayId>,
    pub name: Option<String>,
    pub position: (i32, i32),
    pub size: (u32, u32),
    pub scale: f64,
    pub primary: bool,
}

impl Display {
    pub fn native_id(&self) -> Option<crate::NativeDisplayId> { self.id.as_ref().and_then(|id| id.native_id()) }

    /// (x, y, width, height) in the native global, top-left coordinate space. macOS uses
    /// CGDisplayBounds points; Windows/X11 use desktop pixels; Wayland uses compositor
    /// logical coordinates. Keep the origin in this space; scale only local offsets.
    pub fn native_bounds(&self) -> [f64; 4] {
        #[cfg(target_os = "macos")]
        if let Some(crate::NativeDisplayId::MacOS(id)) = self.native_id() { return crate::macos::display_bounds(id); }
        let divisor = if matches!(self.native_id(), Some(crate::NativeDisplayId::Wayland(_))) { self.scale } else { 1.0 };
        [f64::from(self.position.0), f64::from(self.position.1), f64::from(self.size.0) / divisor, f64::from(self.size.1) / divisor]
    }
}

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub transparent: bool,
    pub borderless: bool,
    pub click_through: bool,
    pub always_on_top: bool,
    pub visible: bool,
    /// Physical desktop coordinates, not points. Use a display's position unchanged.
    pub position: Option<(i32, i32)>,
    /// Physical content size; overrides AppSpec's logical size when set.
    pub physical_size: Option<(u32, u32)>,
    /// Logical minimum; None inherits AppSpec's minimum, Some((0, 0)) removes the floor.
    pub min_size: Option<(f32, f32)>,
    /// Logical maximum content size. None leaves it unconstrained.
    pub max_size: Option<(f32, f32)>,
    /// Whether the user can resize the window. Programmatic sizing is unaffected.
    pub resizable: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            transparent: false, borderless: false, click_through: false,
            always_on_top: false, visible: true, position: None, physical_size: None,
            min_size: None, max_size: None, resizable: true,
        }
    }
}

impl WindowOptions {
    /// Optional AppKit settings without adding fields to existing WindowOptions literals.
    /// Ignored on other platforms.
    pub fn macos(self, native: MacWindowOptions) -> ConfiguredWindowOptions {
        ConfiguredWindowOptions { options: self, native }
    }
    /// One transparent, borderless, topmost, click-through overlay covering this display.
    pub fn overlay(display: &Display) -> Self {
        Self {
            transparent: true, borderless: true, click_through: true, always_on_top: true,
            visible: true, position: Some(display.position), physical_size: Some(display.size),
            min_size: Some((0.0, 0.0)), max_size: None, resizable: true,
        }
    }

    pub(crate) fn attributes(&self, title: &str, size: (f32, f32), app_min: Option<(f32, f32)>, visible: bool) -> winit::window::WindowAttributes {
        use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
        use winit::window::{Window, WindowLevel};
        let mut min = self.min_size.or(app_min).unwrap_or((420.0, 400.0));
        if let Some(max) = self.max_size { min = (min.0.min(max.0), min.1.min(max.1)); }
        let mut attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(size.0, size.1))
            .with_min_inner_size(LogicalSize::new(min.0, min.1))
            .with_resizable(self.resizable)
            .with_transparent(self.transparent)
            .with_decorations(!self.borderless)
            .with_visible(visible)
            .with_window_level(if self.always_on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
        if let Some((w, h)) = self.max_size { attrs = attrs.with_max_inner_size(LogicalSize::new(w, h)); }
        if let Some((x, y)) = self.position { attrs = attrs.with_position(PhysicalPosition::new(x, y)); }
        if let Some((w, h)) = self.physical_size { attrs = attrs.with_inner_size(PhysicalSize::new(w, h)); }
        attrs
    }
}

/// AppKit properties for an ordinary window, reapplied on native recreation.
/// Omitted properties keep winit's defaults. These settings do not turn a window into a panel.
#[derive(Clone, Debug, Default)]
pub struct MacWindowOptions {
    pub(crate) shadow: Option<bool>,
    pub(crate) level: Option<isize>,
    pub(crate) collection_behavior: Option<usize>,
    pub(crate) exclude_from_capture: Option<bool>,
}

impl MacWindowOptions {
    pub fn shadow(mut self, enabled: bool) -> Self { self.shadow = Some(enabled); self }
    /// AppKit NSWindow.Level value (normal 0, floating 3, status 25).
    pub fn level(mut self, level: isize) -> Self { self.level = Some(level); self }
    /// AppKit NSWindow.CollectionBehavior bit mask. Replaces the existing mask.
    pub fn collection_behavior(mut self, bits: usize) -> Self { self.collection_behavior = Some(bits); self }
    /// Requests NSWindowSharingNone; capture clients should also filter window_number().
    /// false restores NSWindowSharingReadOnly. This is not a capture guarantee.
    pub fn exclude_from_capture(mut self, exclude: bool) -> Self { self.exclude_from_capture = Some(exclude); self }
}

/// Ordinary options plus native builders, accepted by WindowSpec::configured_options.
#[derive(Clone, Debug)]
pub struct ConfiguredWindowOptions {
    options: WindowOptions,
    native: MacWindowOptions,
}

impl From<WindowOptions> for ConfiguredWindowOptions {
    fn from(options: WindowOptions) -> Self { Self { options, native: MacWindowOptions::default() } }
}

/// One independent retained tree and app state. Share Live handles or application services
/// between states; events, focus, animation clocks and redraws remain window-local.
pub struct WindowSpec<S: 'static> {
    pub key: String,
    pub app: App<S>,
    pub options: WindowOptions,
    /// Runs on the event thread after native creation. Not called by headless tooling.
    pub on_created: Option<Box<dyn FnOnce(crate::window::WindowHandle)>>,
}

impl<S> WindowSpec<S> {
    pub fn new(key: impl Into<String>, app: AppSpec<S>) -> Self {
        Self::from_app(key, App::new(app))
    }
    /// Preserve AppHandle, windowless, keep-running and GPU-release behavior for this window.
    /// Closing a default window removes only it; quitting any app exits the whole event loop.
    /// The outer run_windows command line controls tooling; App::args and the process-level
    /// App::accessory launch policy are not used here; use run_windows_with(RunOptions, factory).
    /// AppHandle::set_dock_icon remains global.
    pub fn from_app(key: impl Into<String>, app: App<S>) -> Self {
        Self { key: key.into(), app, options: WindowOptions::default(), on_created: None }
    }
    /// Configure this window's frame, including headless traffic-light placement.
    pub fn chrome(mut self, chrome: crate::Chrome) -> Self { self.app = self.app.chrome(chrome); self }
    pub fn options(mut self, options: WindowOptions) -> Self {
        self.options = options;
        self.app.macos_window = MacWindowOptions::default();
        self
    }
    pub fn configured_options(mut self, options: ConfiguredWindowOptions) -> Self {
        let ConfiguredWindowOptions { options, native } = options;
        self.options = options;
        self.app.macos_window = native;
        self
    }
    pub fn state(mut self, state: S) -> Self { self.app = self.app.init(move || state); self }
    pub fn on_created(mut self, callback: impl FnOnce(crate::window::WindowHandle) + 'static) -> Self {
        self.on_created = Some(Box::new(callback));
        self
    }
}

pub(crate) fn validate<S>(windows: &[WindowSpec<S>]) -> Result<(), String> {
    let mut keys = std::collections::HashSet::new();
    for window in windows {
        if window.key.is_empty() || !keys.insert(&window.key) { return Err(format!("empty or duplicate window key: {:?}", window.key)); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::{LogicalSize, Size};

    #[test]
    fn default_window_options_remain_inferable_at_existing_call_sites() {
        // This fails to compile if options becomes generic over Into<...>.
        let _configure: fn(WindowSpec<()>) -> WindowSpec<()> = |spec| spec.options(Default::default());
    }

    #[test]
    fn compact_window_overrides_the_legacy_minimum_without_changing_defaults() {
        let defaults = WindowOptions::default().attributes("app", (560.0, 440.0), None, true);
        assert_eq!(defaults.min_inner_size, Some(Size::Logical(LogicalSize::new(420.0, 400.0))));
        assert!(defaults.resizable && defaults.decorations && defaults.visible);
        assert_eq!(defaults.max_inner_size, None);

        let compact = WindowOptions {
            min_size: Some((280.0, 80.0)), max_size: Some((320.0, 120.0)),
            resizable: false, borderless: true, ..Default::default()
        }.attributes("compact", (300.0, 100.0), Some((860.0, 560.0)), false);
        assert_eq!(compact.inner_size, Some(Size::Logical(LogicalSize::new(300.0, 100.0))));
        assert_eq!(compact.min_inner_size, Some(Size::Logical(LogicalSize::new(280.0, 80.0))));
        assert_eq!(compact.max_inner_size, Some(Size::Logical(LogicalSize::new(320.0, 120.0))));
        assert!(!compact.resizable && !compact.decorations && !compact.visible);
    }

    #[test]
    fn maximum_alone_caps_the_inherited_minimum_per_axis() {
        let options = WindowOptions { max_size: Some((320.0, 120.0)), ..Default::default() };
        for (app_min, expected) in [
            (None, (320.0, 120.0)),
            (Some((860.0, 80.0)), (320.0, 80.0)),
        ] {
            let attrs = options.attributes("compact", (300.0, 100.0), app_min, true);
            assert_eq!(attrs.min_inner_size, Some(Size::Logical(LogicalSize::new(expected.0, expected.1))));
            assert_eq!(attrs.max_inner_size, Some(Size::Logical(LogicalSize::new(320.0, 120.0))));
        }
    }

    #[test]
    fn default_options_inherit_the_app_minimum() {
        let attrs = WindowOptions::default().attributes("app", (900.0, 600.0), Some((860.0, 560.0)), true);
        assert_eq!(attrs.min_inner_size, Some(Size::Logical(LogicalSize::new(860.0, 560.0))));
    }
}
