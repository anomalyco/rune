//! Window descriptions shared by the native shell and deterministic headless selection.
use crate::{App, AppSpec};

#[derive(Clone, Debug)]
pub(crate) struct PlaceRequest {
    pub display: crate::DisplayTarget,
    pub placement: crate::OverlayPlacement,
    pub pointer: Option<(i32, i32)>,
    /// The caller's target, before sampling the pointer; headless runs resolve this.
    pub requested: crate::DisplayTarget,
}

impl PlaceRequest {
    pub fn new(display: crate::DisplayTarget, placement: crate::OverlayPlacement) -> Self {
        #[allow(unused_mut)] // Only desktop hosts can sample a pointer's display.
        let mut request = Self { requested: display.clone(), display, placement, pointer: None };
        if request.display == crate::DisplayTarget::UnderPointer {
            #[cfg(target_os = "macos")]
            { request.display = crate::platform::macos::pointer_display().map_or(crate::DisplayTarget::Primary,
                |id| crate::DisplayTarget::Native(crate::NativeDisplayId::MacOS(id))); }
            #[cfg(target_os = "windows")]
            { request.pointer = crate::platform::win32::pointer_position(); }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            { request.pointer = crate::platform::x11::overlay::pointer_position(); }
        }
        request
    }

    pub fn at(size: (f32, f32)) -> crate::overlay::Placement {
        crate::overlay::Placement { size, edge: crate::ScreenEdge::Top, margin: 0.0, align: crate::Align::Center, inset: 0.0 }
    }
}

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
        if let Some(crate::NativeDisplayId::MacOS(id)) = self.native_id() { return crate::platform::macos::display_bounds(id); }
        let divisor = if matches!(self.native_id(), Some(crate::NativeDisplayId::Wayland(_))) { self.scale } else { 1.0 };
        [f64::from(self.position.0), f64::from(self.position.1), f64::from(self.size.0) / divisor, f64::from(self.size.1) / divisor]
    }
}

/// A window's minimum content size when neither `WindowOptions` nor `AppSpec` sets one.
pub(crate) const DEFAULT_MIN_SIZE: (f32, f32) = (420.0, 400.0);

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub transparent: bool,
    pub borderless: bool,
    /// Clicks, and so hover, pass to whatever is below.
    pub click_through: bool,
    pub always_on_top: bool,
    pub visible: bool,
    /// Physical desktop coordinates, not points. Use a display's position unchanged.
    pub position: Option<(i32, i32)>,
    /// Physical content size; overrides AppSpec's logical size when set.
    pub physical_size: Option<(u32, u32)>,
    /// Logical minimum; None inherits AppSpec's minimum, Some((0, 0)) removes the floor.
    /// Requested sizes (`Tx::resize_window`) stay inside the minimum and maximum.
    pub min_size: Option<(f32, f32)>,
    /// Logical maximum content size. None leaves it unconstrained.
    pub max_size: Option<(f32, f32)>,
    /// Whether the user can resize the window. Programmatic sizing is unaffected. A window
    /// that isn't resizable doesn't maximize either: its maximize (zoom) button is disabled.
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
    /// A hidden, retained launcher. On macOS, prepares its first frame at alpha zero;
    /// show/open takes keys without activating the app. Use `on_ready` to register a shortcut.
    /// Other platforms use an ordinary initially hidden window.
    pub fn launcher_panel() -> ConfiguredWindowOptions {
        let mut native = MacWindowOptions::default().nonactivating_panel(true);
        native.launcher = true;
        Self { visible: false, borderless: true, transparent: true, resizable: false,
            min_size: Some((0.0, 0.0)), ..Default::default() }.macos(native)
    }
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

    /// The window's minimum and maximum content sizes, with `AppSpec::min_size` as the
    /// fallback minimum.
    pub(crate) fn size_limits(&self, app_min: Option<(f32, f32)>) -> ((f32, f32), Option<(f32, f32)>) {
        let mut min = self.min_size.or(app_min).unwrap_or(DEFAULT_MIN_SIZE);
        if let Some(max) = self.max_size { min = (min.0.min(max.0), min.1.min(max.1)); }
        (min, self.max_size)
    }

    /// The minimum a window that fits its content height opens with: the content sets the
    /// height, so the default minimum height doesn't apply.
    pub(crate) fn fit_min_size(&self, app_min: Option<(f32, f32)>) -> Option<(f32, f32)> {
        if app_min.is_none() && self.min_size.is_none() { Some((DEFAULT_MIN_SIZE.0, 0.0)) } else { app_min }
    }

    pub(crate) fn attributes(&self, title: &str, size: (f32, f32), app_min: Option<(f32, f32)>, visible: bool) -> winit::window::WindowAttributes {
        use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
        use winit::window::{Window, WindowButtons, WindowLevel};
        let (min, _) = self.size_limits(app_min);
        let buttons = if self.resizable { WindowButtons::all() } else { WindowButtons::all() - WindowButtons::MAXIMIZE };
        let mut attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(size.0, size.1))
            .with_min_inner_size(LogicalSize::new(min.0, min.1))
            .with_resizable(self.resizable)
            .with_enabled_buttons(buttons)
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
/// Omitted properties keep winit's defaults.
#[derive(Clone, Debug, Default)]
pub struct MacWindowOptions {
    pub(crate) shadow: Option<bool>,
    pub(crate) level: Option<isize>,
    pub(crate) collection_behavior: Option<usize>,
    pub(crate) exclude_from_capture: Option<bool>,
    pub(crate) nonactivating_panel: bool,
    pub(crate) launcher: bool,
    pub(crate) corner_radius: Option<f32>,
    pub(crate) corner_curve_circular: bool,
}

impl MacWindowOptions {
    /// Use a key-taking NSPanel. Open/focus never activates the app or its other windows.
    pub fn nonactivating_panel(mut self, enabled: bool) -> Self {
        self.nonactivating_panel = enabled;
        if !enabled { self.launcher = false; }
        self
    }
    pub fn shadow(mut self, enabled: bool) -> Self { self.shadow = Some(enabled); self }
    /// AppKit NSWindow.Level value (normal 0, floating 3, status 25).
    pub fn level(mut self, level: isize) -> Self { self.level = Some(level); self }
    /// AppKit NSWindow.CollectionBehavior bit mask. Replaces the existing mask.
    pub fn collection_behavior(mut self, bits: usize) -> Self { self.collection_behavior = Some(bits); self }
    /// Requests NSWindowSharingNone; capture clients should also filter window_number().
    /// false restores NSWindowSharingReadOnly. This is not a capture guarantee. A nonactivating
    /// panel applies it only while shown (see `OverlayOptions::exclude_from_capture`).
    pub fn exclude_from_capture(mut self, exclude: bool) -> Self { self.exclude_from_capture = Some(exclude); self }
    /// Rounds a transparent, borderless window or panel to `radius` points with a continuous
    /// (squircle) curve. The window's root layer clips everything it shows, Rune's drawing and
    /// native material views alike, so the root view needn't clip itself to the radius. A
    /// border drawn inside the edge is still the app's. The system shadow, when on, follows
    /// the rounded shape. Ignored for opaque or bordered windows, where AppKit owns the frame,
    /// and on other platforms.
    pub fn corner_radius(mut self, radius: f32) -> Self { self.corner_radius = Some(radius.max(0.0)); self }
    /// Use a circular corner arc instead of the continuous curve for `corner_radius`.
    pub fn corner_curve_circular(mut self) -> Self { self.corner_curve_circular = true; self }
}

/// Ordinary options plus native builders, accepted by WindowSpec::configured_options.
#[derive(Clone, Debug)]
pub struct ConfiguredWindowOptions {
    pub(crate) options: WindowOptions,
    pub(crate) native: MacWindowOptions,
}

impl From<WindowOptions> for ConfiguredWindowOptions {
    fn from(options: WindowOptions) -> Self { Self { options, native: MacWindowOptions::default() } }
}

impl ConfiguredWindowOptions {
    /// Amends the AppKit settings in place, keeping what a preset already set:
    /// `WindowOptions::launcher_panel().macos(|native| native.corner_radius(16.0))` stays a
    /// launcher. Ignored on other platforms.
    pub fn macos(mut self, amend: impl FnOnce(MacWindowOptions) -> MacWindowOptions) -> Self {
        self.native = amend(self.native);
        self
    }
}

/// Forward per-window `App<S>` builder methods onto `WindowSpec<S>` so new builders don't drift.
macro_rules! forward_app_builders {
    ($(
        $(#[$meta:meta])*
        $name:ident $(=> $target:ident)? ($($arg:ident : $ty:ty),* $(,)?)
    );* $(;)?) => {
        $(
            $(#[$meta])*
            pub fn $name(mut self, $($arg: $ty),*) -> Self {
                self.app = forward_app_builders!(@call self.app, $name $(, $target)?, ($($arg),*));
                self
            }
        )*
    };
    (@call $app:expr, $name:ident, ($($arg:ident),*)) => { $app.$name($($arg),*) };
    (@call $app:expr, $_name:ident, $target:ident, ($($arg:ident),*)) => { $app.$target($($arg),*) };
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

impl<S: 'static> WindowSpec<S> {
    pub fn new(key: impl Into<String>, app: AppSpec<S>) -> Self {
        Self::from_app(key, App::new(app))
    }
    /// Preserve AppHandle, windowless, keep-running and GPU-release behavior for this window.
    /// Closing a default window removes only it; quitting any app exits the whole event loop.
    /// RunOptions controls factory display discovery and tooling/window selection. App::args
    /// replaces the selected window's arguments (including native/headless mode). Isolate a
    /// host CLI before factory discovery with run_windows_with_args(options, [], factory).
    /// App::accessory is process-level: use run_windows_with(RunOptions, factory) instead.
    /// AppHandle::set_dock_icon remains global.
    pub fn from_app(key: impl Into<String>, app: App<S>) -> Self {
        Self { key: key.into(), options: app.window_options.clone(), app, on_created: None }
    }
    forward_app_builders! {
        /// Configure this window's frame, including headless traffic-light placement.
        chrome(chrome: crate::Chrome);
        /// This window draws its own titlebar, like `App::titlebar`.
        titlebar(titlebar: crate::Titlebar);
        /// Composite this window's translucent fills, blurs, shadows, images and video in sRGB
        /// (gamma) space instead of linear light, matching CSS and Core Animation. With a
        /// `Compositor`, `Frame::srgb_blend` says which encoding its passes must match.
        srgb_blend(enabled: bool);
        /// This window's font smoothing (`App::font_smoothing`).
        font_smoothing(mode: crate::FontSmoothing);
        /// Size this window's height to its content, like `App::fit_content_height`.
        fit_content_height();
        /// Ask a tiling window manager to float this window when it opens, like `App::float_on_open`.
        float_on_open();
        /// Open this window without taking focus from the window that has it, such as a window a
        /// tab is dragged into while the press stays with its strip. A later `open` or `focus`
        /// activates it.
        inactive => open_inactive();
        /// Open this window maximized, like `App::maximized`.
        maximized(maximized: bool);
        /// Open this window full screen, like `App::full_screen`.
        full_screen(full_screen: bool);
        /// Send `Key::Modifier` presses and releases to this window, like `App::modifier_keys`.
        modifier_keys();
        /// Tab visits every focusable control in this window, like `App::tab_visits_controls`.
        tab_visits_controls(on: bool);
        /// Release this window's GPU device and renderer when it closes, like `App::release_gpu_when_closed`.
        release_gpu_when_closed();
        on_focus(f: fn(&mut S, bool, &mut crate::Tx));
        on_modifiers(f: fn(&mut S, crate::Mods, &mut crate::Tx));
        /// This window going off screen and back, like `App::on_visibility`.
        on_visibility(f: fn(&mut S, bool, &mut crate::Tx));
        /// Runs when this window rests, like `App::on_rest`.
        on_rest(f: fn(&mut S, &mut crate::Tx) -> bool);
        /// This window's editor's input method events, like `App::on_ime`.
        on_ime(f: fn(&mut S, &crate::Ime, &mut crate::Tx) -> bool);
        /// Runs whenever this window closes, like `App::on_close`.
        on_close(f: fn(&mut S, &mut crate::Tx));
        /// Runs in this window when the app starts to quit, like `App::on_quit`.
        on_quit(f: fn(&mut S, &mut crate::Tx));
        /// Runs `enter` before and `leave` after each event-handling turn of this window, like `App::turn_scope`.
        turn_scope(enter: fn(&mut S), leave: fn(&mut S));
        /// Runs when the app is reopened, like `App::on_reopen`.
        on_reopen(f: fn(&mut S, bool, &mut crate::Tx) -> bool);
        /// Receives links opened with the app, like `App::on_open_url`.
        on_open_url(f: fn(&mut S, &str, &mut crate::Tx));
        /// Runs on the UI thread each time the native window is created, like `App::on_window`.
        on_window(f: impl FnMut(&crate::WindowHandle) + 'static);
        /// Called on the event thread after the first frame is ready, once per native creation.
        on_ready(f: impl FnMut(&crate::WindowHandle) + 'static);
        /// Reports each of this window's frames, like `App::on_frame_encoded`. A spec made with
        /// `WindowSpec::from_app` keeps its app's hook unless this replaces it; windows `App::window`
        /// adds have their own.
        on_frame_encoded(f: impl FnMut(&crate::FrameEncoded) + 'static);
        /// Runs on the UI thread after each drawn frame of this window with its runtime, like `App::on_frame_drawn`.
        on_frame_drawn(f: impl FnMut(&crate::Runtime<S>) + 'static);
        /// Draw every frame of this window through `compositor`, like `App::compositor`.
        compositor(compositor: impl crate::compose::Compositor<S>);
        /// This window's own counters for native trace summaries, like `App::dev_stats`.
        dev_stats(stats: fn() -> String);
        /// This window's playground, like `App::playground`.
        playground(playground: crate::Playground<S>);
    }
    /// Configure the underlying `App` directly for any per-window builder not forwarded here.
    pub fn map_app(mut self, f: impl FnOnce(App<S>) -> App<S>) -> Self {
        self.app = f(self.app);
        self
    }
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
    /// Erase the state type, so one factory can return windows of different state types.
    pub fn boxed(self) -> AnyWindowSpec { self.into() }
}

thread_local! {
    /// Windows [`open_window`] asked for, which the event loop starts at the end of its turn.
    static OPENED: std::cell::RefCell<Vec<AnyWindowSpec>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Opens another window in the running app's event loop, with its own state, retained tree,
/// focus and clocks, as the windows a `run_windows` factory returns do: a second document
/// window, or a tab dragged out of its strip. Call it on the UI thread (a handler, an update,
/// `on_ready`); the window starts at the end of the turn. It keeps the app running as the
/// factory's windows do, so the app ends when the last of them closes. Its key must differ
/// from every open window's; a window with a key already open isn't opened. The handle places,
/// focuses and closes it, and its requests wait for it to start. Headless runs open nothing.
pub fn open_window<S: 'static>(spec: WindowSpec<S>) -> crate::WindowHandle {
    let handle = crate::WindowHandle::new(&spec.key);
    if !crate::is_headless() {
        let opened = AnyWindowSpec(Box::new(Erased { spec, handle: Some(handle.clone()), secondary: false }));
        OPENED.with(|o| o.borrow_mut().push(opened));
    }
    handle
}

/// The windows `open_window` queued since the last call.
pub(crate) fn take_opened() -> Vec<AnyWindowSpec> {
    OPENED.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

/// A window's place on the desktop, in physical pixels: what to save to reopen it there.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowPlacement {
    /// The frame's top-left, the space `WindowOptions::position` takes.
    pub position: (i32, i32),
    /// The content's top-left.
    pub content_position: (i32, i32),
    /// The content's size, the space `WindowOptions::physical_size` takes.
    pub content_size: (u32, u32),
    /// Physical pixels per point on the window's display.
    pub scale: f64,
    pub maximized: bool,
    pub minimized: bool,
    pub full_screen: bool,
    /// While the window is maximized or minimized, the frame restoring it returns to, as
    /// `position` and `content_size` give a frame: the system's own record on Windows
    /// (`GetWindowPlacement`), which follows the window to another display (Win+Shift+Arrow).
    /// None otherwise, and where the system keeps no such record (macOS, Linux).
    pub restore: Option<((i32, i32), (u32, u32))>,
    /// The display the window is on (the one with the most of it): its position and size, as
    /// `Display::position` and `Display::size` give them.
    pub display: Option<((i32, i32), (u32, u32))>,
}

/// How a window's first open shows it (`App::maximized`, `App::full_screen`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OpenAs {
    #[default]
    Normal,
    Maximized,
    /// Full screen; leaving it returns to the frame, `maximized` or not.
    FullScreen { maximized: bool },
}

impl OpenAs {
    pub(crate) fn maximized(self, on: bool) -> OpenAs {
        match (self, on) {
            (OpenAs::FullScreen { .. }, maximized) => OpenAs::FullScreen { maximized },
            (_, true) => OpenAs::Maximized,
            (_, false) => OpenAs::Normal,
        }
    }

    pub(crate) fn full_screen(self, on: bool) -> OpenAs {
        match (self, on) {
            (OpenAs::FullScreen { maximized: true }, false) => OpenAs::Maximized,
            (OpenAs::FullScreen { maximized: false }, false) => OpenAs::Normal,
            (OpenAs::Normal, true) => OpenAs::FullScreen { maximized: false },
            (OpenAs::Maximized, true) => OpenAs::FullScreen { maximized: true },
            (state, _) => state,
        }
    }

    pub(crate) fn maximizes(self) -> bool {
        matches!(self, OpenAs::Maximized | OpenAs::FullScreen { maximized: true })
    }

    pub(crate) fn full_screens(self) -> bool {
        matches!(self, OpenAs::FullScreen { .. })
    }
}

/// A `WindowSpec` of any state type, for factories that mix them:
/// `vec![review.boxed(), settings.into()]`.
pub struct AnyWindowSpec(Box<dyn ErasedWindow>);

impl AnyWindowSpec {
    pub fn key(&self) -> &str { self.0.key() }

    /// A window `App::window` added: it closes by hiding and never keeps the loop running alone.
    pub(crate) fn secondary<T: 'static>(mut spec: WindowSpec<T>, handle: crate::WindowHandle) -> Self {
        spec.app.keep_running = true;
        Self(Box::new(Erased { spec, handle: Some(handle), secondary: true }))
    }

    pub(crate) fn resolve_args(&mut self, inherited: crate::tooling::Args) -> crate::tooling::Args { self.0.resolve_args(inherited) }

    /// Render this window headless; `scale` converts a physical size option to points.
    pub(crate) fn headless(self, args: crate::tooling::Args, scale: f32) { self.0.headless(args, scale) }

    /// This window as one member of a `--desktop` composite; `selected` takes `--scene`.
    pub(crate) fn desktop(self, args: &crate::tooling::Args, selected: bool, scale: f32) -> Box<dyn crate::desktop::Member> {
        self.0.desktop(args, selected, scale)
    }

    pub(crate) fn start(self, inherited: crate::tooling::Args, cx: crate::window::Start<'_>) -> Option<Box<dyn crate::window::AnyHost>> {
        self.0.start(inherited, cx)
    }
}

impl<S: 'static> From<WindowSpec<S>> for AnyWindowSpec {
    fn from(spec: WindowSpec<S>) -> Self { Self(Box::new(Erased { spec, handle: None, secondary: false })) }
}

trait ErasedWindow {
    fn key(&self) -> &str;
    fn take_windows(&mut self) -> Vec<AnyWindowSpec>;
    fn resolve_args(&mut self, inherited: crate::tooling::Args) -> crate::tooling::Args;
    fn headless(self: Box<Self>, args: crate::tooling::Args, scale: f32);
    fn desktop(self: Box<Self>, args: &crate::tooling::Args, selected: bool, scale: f32) -> Box<dyn crate::desktop::Member>;
    fn start(self: Box<Self>, inherited: crate::tooling::Args, cx: crate::window::Start<'_>) -> Option<Box<dyn crate::window::AnyHost>>;
}

struct Erased<S: 'static> {
    spec: WindowSpec<S>,
    handle: Option<crate::WindowHandle>,
    secondary: bool,
}

impl<S: 'static> ErasedWindow for Erased<S> {
    fn key(&self) -> &str { &self.spec.key }
    fn take_windows(&mut self) -> Vec<AnyWindowSpec> { std::mem::take(&mut self.spec.app.windows) }
    fn resolve_args(&mut self, inherited: crate::tooling::Args) -> crate::tooling::Args { self.spec.app.resolve_args(inherited) }
    // A trait object's methods are all linked with it: without the `tooling` feature these
    // bodies (never called then: `run` refuses a headless run first) keep headless rendering
    // out of a release app.
    #[cfg(not(any(feature = "tooling", debug_assertions)))]
    fn headless(self: Box<Self>, _: crate::tooling::Args, _: f32) { unreachable!("{}", crate::tooling::WITHOUT_TOOLING) }
    #[cfg(not(any(feature = "tooling", debug_assertions)))]
    fn desktop(self: Box<Self>, _: &crate::tooling::Args, _: bool, _: f32) -> Box<dyn crate::desktop::Member> { unreachable!("{}", crate::tooling::WITHOUT_TOOLING) }
    #[cfg(any(feature = "tooling", debug_assertions))]
    fn headless(self: Box<Self>, args: crate::tooling::Args, scale: f32) {
        crate::tooling::headless_window_spec(self.spec.app, self.spec.options, args, scale);
    }
    #[cfg(any(feature = "tooling", debug_assertions))]
    fn desktop(self: Box<Self>, args: &crate::tooling::Args, selected: bool, scale: f32) -> Box<dyn crate::desktop::Member> {
        let Erased { spec, handle, secondary } = *self;
        Box::new(crate::desktop::Window::new(spec, handle, args, selected, !secondary, scale))
    }
    fn start(self: Box<Self>, inherited: crate::tooling::Args, cx: crate::window::Start<'_>) -> Option<Box<dyn crate::window::AnyHost>> {
        let inherited = secondary_args(self.secondary, &self.spec.key, inherited);
        crate::window::start(self.spec, self.handle, !self.secondary, inherited, cx)
    }
}

fn secondary_args(secondary: bool, key: &str, mut inherited: crate::tooling::Args) -> crate::tooling::Args {
    // A secondary window is not what --scene or --size describe unless --window selects it.
    if secondary && inherited.window.as_deref() != Some(key) {
        inherited.scene = None;
        inherited.size = None;
    }
    inherited
}

/// Each window followed by those its app added with `App::window`, depth first.
pub(crate) fn flatten(windows: impl IntoIterator<Item = AnyWindowSpec>) -> Vec<AnyWindowSpec> {
    let mut out = Vec::new();
    for mut window in windows {
        let children = window.0.take_windows();
        out.push(window);
        out.extend(flatten(children));
    }
    out
}

pub(crate) fn validate(windows: &[AnyWindowSpec]) -> Result<(), String> {
    let mut keys = std::collections::HashSet::new();
    for window in windows {
        if window.key().is_empty() || !keys.insert(window.key()) { return Err(format!("empty or duplicate window key: {:?}", window.key())); }
    }
    Ok(())
}

#[cfg(test)]
#[path = "spec_tests.rs"]
mod tests;
