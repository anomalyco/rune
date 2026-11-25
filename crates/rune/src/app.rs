//! The app's lifetime beside its window. `App` runs an `AppSpec` with or without a window and
//! decides what closing it means; `AppHandle` opens, focuses, closes and feeds the running app
//! from any thread.
//!
//! The state, scene and text live with the app, not the window: closing the window keeps them,
//! and reopening shows the same UI, settled. Closing hides the window where the platform can
//! (macOS, Windows, X11) and destroys it on Wayland, where a hidden toplevel doesn't exist;
//! the GPU device and renderer stay either way.
//!
//! ```no_run
//! # use rune::{AppSpec, el};
//! # struct State { snapshot: String }
//! # enum Request { Activate(String), Snapshot(String), Quit }
//! # fn requests() -> Vec<Request> { Vec::new() }
//! # let spec = AppSpec::new("App", (640.0, 480.0), || State { snapshot: String::new() }, |_, _| el());
//! let app = rune::App::new(spec).windowless().keep_running();
//! let handle = app.handle();
//! std::thread::spawn(move || {
//!     for request in requests() {
//!         match request {
//!             Request::Activate(token) => handle.activate(token),
//!             Request::Snapshot(s) => handle.update(move |ui, _| ui.snapshot = s),
//!             Request::Quit => handle.quit(),
//!         }
//!     }
//! });
//! app.run();
//! ```

use std::sync::Arc;

use crate::el::Tx;
use crate::tooling::{AppSpec, Args};

#[path = "app/quit_request.rs"]
mod quit_request;
pub use quit_request::QuitResponse;
#[path = "app/web.rs"]
mod web;
#[cfg(target_arch = "wasm32")]
pub(crate) use web::with_font;

pub(crate) type Update<S> = Box<dyn FnOnce(&mut S, &mut Tx) -> bool + Send>;
/// `App::on_window`'s hook, run after each native window creation.
pub(crate) type OnWindow = Box<dyn FnMut(&crate::WindowHandle)>;
/// `App::on_display_close`'s hook, run once on the UI thread before the event loop drops windows
/// and closes the native display connection.
pub(crate) type OnDisplayClose = Box<dyn FnOnce()>;

/// Decision returned by [`App::on_close_request`] and
/// [`WindowSpec::on_close_request`](crate::WindowSpec::on_close_request) when a native window
/// receives a close request (`WindowEvent::CloseRequested`).
///
/// ```no_run
/// use rune::{App, AppSpec, CloseResponse, el};
///
/// let spec = AppSpec::new("Draft", (480.0, 320.0), || true, |_, _| el());
/// App::new(spec)
///     .on_close_request(|has_unsaved, _| {
///         if *has_unsaved { CloseResponse::Defer } else { CloseResponse::Close }
///     })
///     .run();
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseResponse {
    /// Proceed with closing the window as usual (running [`App::on_close`], and finishing or
    /// hiding the window according to [`App::keep_running`]).
    Close,
    /// Keep the window open and commit any state or [`Tx`] changes made in the hook. Close the
    /// window later with [`Tx::close_window`], [`AppHandle::close`], or
    /// [`WindowHandle::close`](crate::WindowHandle::close).
    Defer,
}

/// `App::turn_scope`'s hooks.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct TurnScope<S> {
    pub(crate) enter: fn(&mut S),
    pub(crate) leave: fn(&mut S),
}

#[cfg(not(target_arch = "wasm32"))]
impl<S> Clone for TurnScope<S> {
    fn clone(&self) -> Self { *self }
}

#[cfg(not(target_arch = "wasm32"))]
impl<S> Copy for TurnScope<S> {}

/// Normalizes an opt-in frame-rate cap (Hz): positive finite values clamp to at least `1.0` Hz,
/// while zero, negative, NaN and infinite values normalize to `0.0` (uncapped display rate).
#[inline]
pub(crate) fn normalize_fps_cap(fps: f32) -> f32 {
    if fps.is_finite() && fps > 0.0 {
        fps.max(1.0)
    } else {
        0.0
    }
}

/// Converts an opt-in `ambient_fps` rate (Hz) into its target frame period, or `None` when
/// uncapped (zero, negative, non-finite, or so large that its period rounds to zero).
#[inline]
pub(crate) fn ambient_period_from_fps(fps: f32) -> Option<std::time::Duration> {
    let hz = normalize_fps_cap(fps);
    (hz > 0.0)
        .then(|| std::time::Duration::from_secs_f64(1.0 / f64::from(hz)))
        .filter(|period| !period.is_zero())
}

/// An `AppSpec` with its lifetime: by default it opens one window and quits when it closes,
/// like `rune::run`.
pub struct App<S: 'static> {
    pub(crate) spec: AppSpec<S>,
    pub(crate) chrome: crate::Chrome,
    pub(crate) titlebar: Option<crate::Titlebar>,
    init: Option<Box<dyn FnOnce() -> S>>,
    argv: Option<Vec<String>>,
    pub(crate) open: bool,
    pub(crate) release_gpu: bool,
    pub(crate) cpu_first: crate::CpuFirst,
    pub(crate) resize_sync: crate::ResizeSync,
    pub(crate) gpu_loss_policy: crate::GpuLossPolicy,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) gpu_loss_latch: Option<Arc<crate::render::loss_latch::GpuLossLatch>>,
    pub(crate) prewarm_effects: crate::PrewarmEffects,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) accessory: bool,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) standard_menus: bool,
    pub(crate) keep_running: bool,
    pub(crate) on_close_request: Option<fn(&mut S, &mut Tx) -> CloseResponse>,
    pub(crate) on_quit_request: Option<fn(&mut S, &mut Tx) -> QuitResponse>,
    pub(crate) on_close: Option<fn(&mut S, &mut Tx)>,
    pub(crate) on_quit: Option<fn(&mut S, &mut Tx)>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) turn_scope: Option<TurnScope<S>>,
    pub(crate) on_rest: Option<fn(&mut S, &mut Tx) -> bool>,
    pub(crate) on_reopen: Option<fn(&mut S, bool, &mut Tx) -> bool>,
    pub(crate) on_open_url: Option<crate::open_url::incoming::OnOpenUrl<S>>,
    pub(crate) on_open_files: Option<crate::open_url::incoming::OnOpenFiles<S>>,
    pub(crate) shared: Arc<Shared<S>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) overlays: Vec<Box<dyn crate::overlay::PendingOverlay>>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) has_overlays: bool,
    pub(crate) windows: Vec<crate::AnyWindowSpec>,
    pub(crate) on_window: Option<OnWindow>,
    pub(crate) on_ready: Option<OnWindow>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) on_window_geometry: Option<crate::window::geometry::OnWindowGeometry<S>>,
    pub(crate) on_display_close: Option<OnDisplayClose>,
    pub(crate) on_frame_encoded: Option<crate::frame::encoded::OnFrameEncoded>,
    pub(crate) on_frame_presented: Option<crate::frame::presented::OnFramePresented>,
    pub(crate) on_frame_drawn: Option<crate::frame::encoded::OnFrameDrawn<S>>,
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub(crate) ambient_period: Option<std::time::Duration>,
    /// `App::layout_seed`'s seed and keeper; only desktop windows use them.
    #[cfg_attr(any(target_arch = "wasm32", target_os = "ios", target_os = "android"), allow(dead_code))]
    pub(crate) layout_seed: Option<crate::LayoutSeed>,
    #[cfg_attr(any(target_arch = "wasm32", target_os = "ios", target_os = "android"), allow(dead_code))]
    pub(crate) keep_layout: Option<Box<dyn FnMut(crate::LayoutSeed)>>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) same_tab_router: Option<Box<dyn Fn(&str) -> bool>>,
    pub(crate) macos_window: crate::MacWindowOptions,
    pub(crate) windows_window: crate::WindowsWindowOptions,
    pub(crate) exclude_from_capture: Option<bool>,
    pub(crate) color_scheme: crate::ColorScheme,
    pub(crate) window_backdrop: crate::WindowBackdrop,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) frame_latency: Option<u32>,
    pub(crate) compositor: Option<Box<dyn crate::compose::Compositor<S>>>,
    pub(crate) window_options: crate::WindowOptions,
    pub(crate) canvas_id: Option<String>,
    pub(crate) full_viewport: bool,
    pub(crate) fit_content_height: bool,
    pub(crate) web_document: bool,
    /// `App::font_url`'s fonts, which a web app fetches before its first frame.
    #[cfg(target_arch = "wasm32")]
    pub(crate) font_urls: Vec<String>,
    /// `App::font_url_later`'s fonts, which a web app fetches after its first frame.
    #[cfg(target_arch = "wasm32")]
    pub(crate) late_font_urls: Vec<String>,
    pub(crate) float_on_open: bool,
    /// `App::open_inactive`: the first open leaves focus where it is.
    pub(crate) open_inactive: bool,
    /// `App::maximized`, `App::full_screen`: how the window's first open shows it.
    pub(crate) open_as: crate::windows::OpenAs,
    pub(crate) runtime: RuntimeConfig<S>,
    pub(crate) playground: Option<crate::Playground<S>>,
    /// `playground::attach`, set by `App::playground`: a window calls it through this, so an app
    /// without a playground doesn't link it.
    pub(crate) attach_playground: Option<crate::playground::Attach<S>>,
}

impl<S: 'static> App<S> {
    pub fn new(spec: AppSpec<S>) -> Self {
        let shared: Arc<Shared<S>> = Arc::default();
        shared.shortcuts.named(spec.title);
        App {
            spec,
            chrome: crate::Chrome::DEFAULT,
            titlebar: None,
            init: None,
            argv: None,
            open: true,
            release_gpu: false,
            cpu_first: crate::CpuFirst::Auto,
            resize_sync: crate::ResizeSync::Auto,
            gpu_loss_policy: crate::GpuLossPolicy::Recover,
            #[cfg(not(target_arch = "wasm32"))]
            gpu_loss_latch: None,
            prewarm_effects: crate::PrewarmEffects::NONE,
            accessory: false,
            standard_menus: false,
            keep_running: false,
            on_close_request: None,
            on_quit_request: None,
            on_close: None,
            on_quit: None,
            #[cfg(not(target_arch = "wasm32"))]
            turn_scope: None,
            on_rest: None,
            on_reopen: None,
            on_open_url: None,
            on_open_files: None,
            shared,
            #[cfg(not(target_arch = "wasm32"))]
            overlays: Vec::new(),
            #[cfg(target_arch = "wasm32")]
            has_overlays: false,
            windows: Vec::new(),
            on_window: None,
            on_ready: None,
            #[cfg(not(target_arch = "wasm32"))]
            on_window_geometry: None,
            on_display_close: None,
            on_frame_encoded: None,
            on_frame_presented: None,
            on_frame_drawn: None,
            ambient_period: None,
            layout_seed: None,
            keep_layout: None,
            same_tab_router: None,
            macos_window: crate::MacWindowOptions::default(),
            windows_window: crate::WindowsWindowOptions::default(),
            exclude_from_capture: None,
            color_scheme: crate::ColorScheme::System,
            window_backdrop: crate::WindowBackdrop::None,
            frame_latency: None,
            compositor: None,
            window_options: crate::WindowOptions::default(),
            canvas_id: None,
            full_viewport: false,
            fit_content_height: false,
            web_document: false,
            #[cfg(target_arch = "wasm32")]
            font_urls: Vec::new(),
            #[cfg(target_arch = "wasm32")]
            late_font_urls: Vec::new(),
            float_on_open: false,
            open_inactive: false,
            open_as: crate::windows::OpenAs::Normal,
            runtime: RuntimeConfig::default(),
            playground: None,
            attach_playground: None,
        }
    }

    /// Composite this window's translucent fills, blurs, shadows, images, video and text in
    /// sRGB (gamma) space instead of linear light, matching CSS and Core Animation over dark
    /// backgrounds, as CoreText does. On by default. With a `Compositor`, `Frame::srgb_blend`
    /// says which encoding its passes must match.
    pub fn srgb_blend(mut self, enabled: bool) -> Self {
        self.runtime.srgb_blend = enabled;
        self
    }

    /// Maximum number of native `El::system_material` views this window may host at once
    /// (default [`crate::MAX_NATIVE_MATERIALS`], clamped to 64).
    ///
    /// Visible material elements past this limit draw their fallback fill (or neutral tint),
    /// report [`ElementInfo::material_fallback`](crate::ElementInfo::material_fallback), emit
    /// `"materialFallback": true` in `--tree` JSON, and log a one-time notice to stderr when
    /// native materials are active. Passing `0` disables native material views for this window.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// App::new(AppSpec::new("Notifications", (360.0, 520.0), || (), |_, _| el()))
    ///     .max_native_materials(8)
    ///     .run();
    /// ```
    pub fn max_native_materials(mut self, max: usize) -> Self {
        self.runtime.max_native_materials = crate::system_material::clamp_max_native_materials(max);
        self
    }

    /// Cap the wake rate (Hz) used when `El::reveal` word fades are the only active animation
    /// (`0.0`, default = full display rate; positive finite values clamp to at least `1.0` Hz,
    /// non-positive or non-finite values normalize to `0.0`).
    ///
    /// `30.0` Hz is recommended because it divides both 60 Hz and 120 Hz displays cleanly without
    /// beat judder (Apple WWDC21 Session 10147), though any positive rate (such as `40.0`) is
    /// accepted. When another animation runs alongside a word fade, the fade rides along at that
    /// animation's full rate; once the fade finishes, the runtime clears its wake and draws
    /// nothing at rest. Applies across macOS, Windows, Linux, and web.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, text};
    ///
    /// let spec = AppSpec::new("Chat", (720.0, 540.0), String::new, |s, _| {
    ///     text(s.as_str()).wrap().reveal(0.2)
    /// });
    /// App::new(spec).reveal_fps(30.0).run();
    /// ```
    pub fn reveal_fps(mut self, fps: f32) -> Self {
        self.runtime.reveal_fps = RuntimeConfig::<S>::normalize_reveal_fps(fps);
        self
    }

    /// How this app's windows and overlays rasterise and place glyphs (see [`crate::FontSmoothing`]):
    /// `On` by default on macOS and in a Mac's browser, `WindfoilChromium` elsewhere.
    /// `ConfiguredOverlay::font_smoothing` overrides it per overlay, and
    /// `Tx::set_font_smoothing` switches it while running.
    pub fn font_smoothing(mut self, mode: crate::FontSmoothing) -> Self {
        self.runtime.font_smoothing = mode;
        #[cfg(not(target_arch = "wasm32"))]
        for overlay in &mut self.overlays {
            overlay.inherit_font_smoothing(mode);
        }
        self
    }

    /// Opt-in preferred colour emoji font family (or comma-separated CSS family list) tried first
    /// for emoji graphemes (`Emoji_Presentation`, `U+FE0F`, ZWJ sequences, keycaps, skin tones,
    /// and regional-indicator flags) across every text slot (`Family::Sans`, `Family::Mono`,
    /// `Family::SystemMono`, `Family::Named`, and inline mono spans).
    ///
    /// Precedence for emoji graphemes is: this explicit `emoji_family` first, then any colour
    /// emoji font explicitly named in the active family stack (`AppSpec::families`,
    /// `Tx::set_font_families`, or `Family::Named`), then the lazily loaded system emoji font,
    /// then the active text stack. Plain text symbols without `U+FE0F` (such as `❤` or
    /// `☺\u{FE0E}`) keep their monochrome text faces. Uncovered emoji still fall back to the
    /// system emoji font, and the system emoji font is not loaded when every emoji in a string is
    /// already covered.
    ///
    /// Passing an empty or whitespace-only string normalizes to `None` (default system emoji
    /// fallback). Switch or clear at runtime with [`Tx::set_emoji_family`] or
    /// [`Runtime::set_emoji_family`](crate::Runtime::set_emoji_family).
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// let spec = AppSpec::new("Chat", (640.0, 480.0), || (), |_, _| el());
    /// App::new(spec).emoji_family("Twemoji").run();
    /// ```
    pub fn emoji_family(mut self, family: impl Into<String>) -> Self {
        let family = family.into();
        self.runtime.emoji_family = crate::text::normalize_emoji_family(Some(&family));
        self
    }

    /// Configure this window's frame. Overlays retain their own native behaviour.
    pub fn chrome(mut self, chrome: crate::Chrome) -> Self {
        self.chrome = chrome;
        self
    }

    /// Draw this window's titlebar in the view, `Titlebar::height` points tall, with the
    /// window's own controls over it (see [`crate::Titlebar`]): macOS keeps its traffic lights,
    /// Windows gets caption buttons Rune draws, and `Cx::chrome_insets` says what they take.
    /// Mark the band `.window_drag()`.
    pub fn titlebar(mut self, titlebar: crate::Titlebar) -> Self {
        self.titlebar = Some(titlebar);
        self
    }

    /// Native options for this app's window (size limits, resizability, ...). `WindowSpec::from_app`
    /// starts from them, and `WindowSpec::options` replaces them.
    pub fn window_options(mut self, options: crate::WindowOptions) -> Self {
        self.window_options = options;
        self
    }

    /// Size the window's height to its content, as SwiftUI's `.windowResizability(.contentSize)`
    /// does: the root lays out at its natural height for the window's width, the window opens
    /// at that height and follows it when the content changes, on the change's spring like
    /// `Tx::resize_window`. Give the root no fixed or `fill` height. Users still set the width.
    pub fn fit_content_height(mut self) -> Self {
        self.fit_content_height = true;
        self
    }

    /// Ask a tiling window manager to float the window when it opens, with no window rule.
    /// On Linux the window maps at a fixed size (its minimum equals its maximum), which
    /// sway, Hyprland, niri and i3 float; once its first frame is shown, the window's own
    /// minimum and maximum apply and the user can resize it. Ignored elsewhere.
    pub fn float_on_open(mut self) -> Self {
        self.float_on_open = true;
        self
    }

    /// The window's first open shows it without taking focus or activating the app, so the
    /// window the user works in keeps its keys and a press it holds. A later `open` or `focus`
    /// activates it. macOS and Windows; elsewhere the window manager decides.
    pub fn open_inactive(mut self) -> Self {
        self.open_inactive = true;
        self
    }

    /// Open the window maximized, as a window saved maximized reopens. The size it opens at
    /// stays what restoring it returns to.
    pub fn maximized(mut self, maximized: bool) -> Self {
        self.open_as = self.open_as.maximized(maximized);
        self
    }

    /// Open the window full screen, as a window saved full screen reopens (Electron's
    /// `setFullScreen(true)`): borderless over the display it opens on, on Windows and Linux.
    /// Leaving full screen returns to the frame it opened at, maximized if `maximized` is on too.
    pub fn full_screen(mut self, full_screen: bool) -> Self {
        self.open_as = self.open_as.full_screen(full_screen);
        self
    }

    /// Send modifier keys pressed and released on their own (`Key::Modifier`, with the side)
    /// to `on_key` and the focused input's key handler, for hold-to-talk on Fn or Right Option,
    /// or a Shift tap. Off by default, since handlers written without it may act on any key.
    pub fn modifier_keys(mut self) -> Self {
        self.runtime.modifier_keys = true;
        self
    }

    /// Opt into hardware media transport keys ([`Key::Media`](crate::Key::Media): Play/Pause,
    /// Play, Pause, Next, Previous, Stop, Fast Forward, Rewind) in `on_key`, `El::on_key`,
    /// `El::on_key_preview`, `El::on_key_up`, and `Chord`.
    ///
    /// Off by default so apps that do not control media playback receive no new keys and, on
    /// macOS, never install the local `NSEventTypeSystemDefined` monitor that claims system
    /// media keys while the window is key. Text fields ignore media keys so they still reach
    /// `on_key` while typing.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, Key, MediaKey, el};
    ///
    /// let spec = AppSpec {
    ///     on_key: Some(|playing: &mut bool, ev, _| {
    ///         if ev.key == Key::Media(MediaKey::PlayPause) {
    ///             *playing = !*playing;
    ///             return true;
    ///         }
    ///         false
    ///     }),
    ///     ..AppSpec::new("Player", (480.0, 320.0), || false, |_, _| el())
    /// };
    /// App::new(spec).media_keys().run();
    /// ```
    pub fn media_keys(mut self) -> Self {
        self.runtime.media_keys = true;
        self
    }

    /// Tab and Shift-Tab visit every focusable control, buttons included, whatever the system's
    /// keyboard navigation setting. Off (the default), Tab follows macOS: with Keyboard
    /// navigation off it visits only text fields and lists.
    pub fn tab_visits_controls(mut self, on: bool) -> Self {
        self.runtime.tab_visits_controls = on;
        self
    }

    /// Native and portable options together, including `WindowOptions::launcher_panel()`.
    pub fn configured_options(mut self, options: crate::ConfiguredWindowOptions) -> Self {
        self.window_options = options.options;
        self.macos_window = options.native;
        self.windows_window = options.windows;
        self
    }

    /// Native window policy, also retained by a single-window App.
    pub fn macos_window(mut self, options: crate::MacWindowOptions) -> Self {
        self.macos_window = options;
        self
    }

    /// Win32 native window policy, retained across recreation and ignored on other platforms.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, WindowsWindowOptions, el};
    ///
    /// App::new(AppSpec::new("Panel", (320.0, 200.0), || (), |_, _| el()))
    ///     .windows_window(WindowsWindowOptions::default().no_activate(true).tool_window(true))
    ///     .run();
    /// ```
    pub fn windows_window(mut self, options: crate::WindowsWindowOptions) -> Self {
        self.windows_window = options;
        self
    }

    /// Excludes this window from screen captures on Windows (`WDA_EXCLUDEFROMCAPTURE`) and macOS
    /// (`NSWindowSharingNone`), applied before the window first shows and reapplied on native
    /// recreation. `false` explicitly restores normal capture visibility. Ignored on Linux and web.
    ///
    /// When set, this takes precedence over `WindowsWindowOptions::exclude_from_capture` and
    /// `MacWindowOptions::exclude_from_capture`; when unset, those platform options still apply.
    /// Toggle at runtime with [`WindowHandle::set_exclude_from_capture`](crate::WindowHandle::set_exclude_from_capture).
    /// Capture exclusion is native best effort, not a security boundary.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// App::new(AppSpec::new("Private Notes", (480.0, 360.0), || (), |_, _| el()))
    ///     .exclude_from_capture(true)
    ///     .run();
    /// ```
    pub fn exclude_from_capture(mut self, exclude: bool) -> Self {
        self.exclude_from_capture = Some(exclude);
        self
    }

    /// Preferred native light or dark chrome for this window, applied on creation and retained
    /// across native recreation. Defaults to [`ColorScheme::System`](crate::ColorScheme::System).
    ///
    /// On Windows this controls the native titlebar (`DWMWA_USE_IMMERSIVE_DARK_MODE`), on macOS
    /// `NSWindow.appearance`, on Wayland client-side decorations, and on X11
    /// `_GTK_THEME_VARIANT`. Update at runtime with
    /// [`Tx::set_color_scheme`](crate::Tx::set_color_scheme) or
    /// [`WindowHandle::set_color_scheme`](crate::WindowHandle::set_color_scheme), or override
    /// the whole app with [`AppHandle::set_color_scheme`](crate::AppHandle::set_color_scheme).
    /// Ignored on web and headless.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, ColorScheme, el};
    ///
    /// App::new(AppSpec::new("Notes", (480.0, 360.0), || (), |_, _| el()))
    ///     .color_scheme(ColorScheme::Dark)
    ///     .run();
    /// ```
    pub fn color_scheme(mut self, scheme: crate::ColorScheme) -> Self {
        self.color_scheme = scheme;
        self
    }

    /// Opt-in whole-window system backdrop material ([`WindowBackdrop`](crate::WindowBackdrop)),
    /// applied on creation and retained across native window recreation. Defaults to
    /// [`WindowBackdrop::None`](crate::WindowBackdrop::None).
    ///
    /// On Windows 11 Build 22621+ transparent DirectComposition GPU windows
    /// (`WindowOptions { transparent: true, .. }`), this applies the documented
    /// `DWMWA_SYSTEMBACKDROP_TYPE` attribute. Views read [`Cx::window_backdrop`](crate::Cx::window_backdrop)
    /// to see whether the OS accepted the backdrop attribute.
    ///
    /// By Microsoft design, Windows draws Mica, Mica Alt, and Desktop Acrylic only while the window
    /// is the foreground window and shows its own solid neutral fallback when unfocused (so a
    /// non-activating window or overlay that never takes focus never shows the live material, even
    /// while `cx.window_backdrop().is_active()` is `true`). Pair `cx.window_backdrop()` with
    /// [`App::on_focus`](Self::on_focus) or [`El::on_window_focus`](crate::El::on_window_focus)
    /// when a view needs to distinguish "backdrop active and focused" from "backdrop active".
    /// Update at runtime with [`Tx::set_window_backdrop`](crate::Tx::set_window_backdrop) or
    /// [`WindowHandle::set_window_backdrop`](crate::WindowHandle::set_window_backdrop).
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, Color, WindowBackdrop, WindowOptions, el};
    ///
    /// struct State { focused: bool }
    ///
    /// let opts = WindowOptions { transparent: true, ..Default::default() };
    /// let spec = AppSpec::new("Notes", (800.0, 600.0), || State { focused: true }, |s, cx| {
    ///     // Check `s.focused` as well if you want a custom fill instead of Windows' flat
    ///     // unfocused fallback while the window is in the background.
    ///     let bg = if cx.window_backdrop().is_active() && s.focused {
    ///         Color::TRANSPARENT
    ///     } else {
    ///         rune::tok::theme(cx.system_dark()).bg
    ///     };
    ///     el().w_full().h_full().fill(bg)
    /// });
    /// App::new(spec)
    ///     .window_options(opts)
    ///     .window_backdrop(WindowBackdrop::Mica)
    ///     .on_focus(|s, focused, _| s.focused = focused)
    ///     .run();
    /// ```
    pub fn window_backdrop(mut self, backdrop: crate::WindowBackdrop) -> Self {
        self.window_backdrop = backdrop;
        self
    }

    /// Called once per native focus transition, including while hidden or occluded.
    /// Headless scripts deliver the same callback with `{"focus": false}`.
    pub fn on_focus(mut self, f: fn(&mut S, bool, &mut Tx)) -> Self {
        self.runtime.on_focus = Some(f);
        self
    }

    /// Hears the modifier keys held down (Shift, Control, Alt or Option, the logo key) each time
    /// they change, wherever focus is and whether or not anything takes the key, as a web page's
    /// window-level `keydown`/`keyup` for `Alt` hear it; and none held once the window loses
    /// focus (a `blur`). For a reveal while a modifier is held: Desktop's Home shows every
    /// session's worktree and branch while Alt is down. Return nothing: it can't take the key
    /// from anyone, and [`Tx::live_only`] says the view reads nothing that changed.
    ///
    /// ```
    /// # use rune::{AppSpec, el};
    /// # struct App { alt_held: bool }
    /// # let spec = AppSpec::new("App", (640.0, 480.0), || App { alt_held: false }, |_, _| el());
    /// # let app = rune::App::new(spec);
    /// # let _ =
    /// app.on_modifiers(|s: &mut App, mods, _| s.alt_held = mods.alt)
    /// # ;
    /// ```
    pub fn on_modifiers(mut self, f: fn(&mut S, crate::Mods, &mut Tx)) -> Self {
        self.runtime.on_mods = Some(f);
        self
    }

    /// Hears the window go off screen (minimised, hidden, or covered whole by other windows
    /// where the system says so) and come back, with whether it shows now, as a web page's
    /// `visibilitychange` hears `document.hidden` change: for work that should wait while no
    /// one can see it, such as a notification's timer. Called once per change after the first
    /// frame; a poll reads the same with [`Tx::window_visible`].
    pub fn on_visibility(mut self, f: fn(&mut S, bool, &mut Tx)) -> Self {
        self.runtime.on_visibility = Some(f);
        self
    }

    /// Runs when a window rests: it has drawn nothing for 10 s (1 s while the system says memory
    /// is low), or it was hidden, minimised or covered whole. By then its renderer and Rune's
    /// own caches have given back what they keep for frames to come (`window::rest`); this is
    /// for the app's caches of what isn't on screen. Return whether state the view reads changed
    /// (normally false: nothing on screen should).
    pub fn on_rest(mut self, f: fn(&mut S, &mut Tx) -> bool) -> Self {
        self.on_rest = Some(f);
        self
    }

    /// Hear the input method for an editor the app draws itself, whose caret is marked with
    /// [`El::ime_cursor`](crate::El::ime_cursor): `Ime::Preedit` with its cursor range (draw it
    /// at the caret, not yet in the text; empty ends it), `Ime::Commit` (insert it) and
    /// `Ime::Enabled`/`Disabled`. Return true when the editor took the event. A focused
    /// `text_input` takes them instead, but a composition this hook has open ends here even
    /// after focus moved.
    ///
    /// Typing reaches the editor once: a plain character arrives as a key, `Key::Text` in
    /// `on_key`, and `Ime::Commit` carries only what an input method composed (⌥E E's `é`, a
    /// CJK conversion, the emoji picker, dictation), as for `text_input`. While a composition
    /// is open, Return and Escape go to the input method, not `on_key`. Native windows only.
    pub fn on_ime(mut self, f: fn(&mut S, &crate::Ime, &mut Tx) -> bool) -> Self {
        self.runtime.on_ime = Some(f);
        self
    }

    /// Fallback handler for a text field's context menu (right-click, Menu key, Shift+F10) on
    /// platforms where [`crate::popup_menu_supported`] is false (such as Linux).
    ///
    /// When set and `!popup_menu_supported()`, Rune hands the ready-built
    /// [`crate::FieldMenuRequest`] to `f` instead of queuing a native pop-up menu, so the app
    /// can render it in-window (for example with `rune_kit::Menu`) and answer with
    /// [`Tx::resolve_field_menu`]. On macOS and Windows (`popup_menu_supported()` is true),
    /// native field menus continue to open as usual and `f` is not called.
    ///
    /// ```
    /// use rune::{App, AppSpec, FieldMenuRequest, Tx, el};
    ///
    /// struct S { menu: Option<FieldMenuRequest> }
    /// let _app = App::new(AppSpec::new("Notes", (480.0, 320.0), || S { menu: None }, |_, _| el()))
    ///     .on_field_menu(|s: &mut S, req: FieldMenuRequest, _tx: &mut Tx| {
    ///         s.menu = Some(req);
    ///     });
    /// ```
    pub fn on_field_menu(mut self, f: fn(&mut S, crate::FieldMenuRequest, &mut Tx)) -> Self {
        self.runtime.on_field_menu = Some(f);
        self
    }

    /// Adds a transparent overlay with its own state, hidden until `OverlayHandle::show`.
    /// Bare specs preserve primary-display edge placement and never request key focus;
    /// `OverlaySpec::options` selects displays, placement and native input policy.
    /// Uses this app's fonts and font smoothing (`ConfiguredOverlay::font_smoothing` overrides it).
    /// Headless runs don't draw overlays.
    pub fn overlay<T: 'static>(&mut self, spec: impl Into<crate::ConfiguredOverlay<T>>, state: T) -> crate::overlay::OverlayHandle<T> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut configured = spec.into();
            if configured.runtime.emoji_family.is_none() {
                configured.runtime.emoji_family = self.runtime.emoji_family.clone();
            }
            let (mut overlay, handle) = crate::overlay::OverlayHost::new(configured, state, &self.spec, self.runtime.font_smoothing);
            overlay.set_prewarm_effects(self.prewarm_effects);
            if let Some(latch) = &self.gpu_loss_latch {
                overlay.set_gpu_loss_latch(Some(Arc::clone(latch)));
            }
            self.overlays.push(Box::new(overlay));
            handle
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (spec, state);
            self.has_overlays = true;
            crate::overlay::OverlayHandle::disconnected()
        }
    }

    /// Adds an ordinary window with its own state type to this app's event loop, such as a
    /// settings window beside the main one. `spec.options` applies (`visible: false` creates it
    /// hidden; `App::windowless` on its app defers creation), and either handle opens it later.
    /// Focus, input, clocks and redraws stay with each window; quitting, the Dock icon and
    /// accessory mode stay with the process.
    ///
    /// Closing it hides it and runs its `on_close`, like `keep_running`: reopening shows the
    /// same state (Wayland destroys and recreates the window). It never keeps the app running
    /// alone: the app still ends with its own window, and `Tx::quit` from either quits both.
    /// Headless tools select it with `--window KEY`; otherwise `--scene` and `--size` are left
    /// to the primary window.
    pub fn window<T: 'static>(&mut self, spec: crate::WindowSpec<T>) -> (AppHandle<T>, crate::WindowHandle) {
        let (app, window) = (spec.app.handle(), crate::WindowHandle::new(&spec.key));
        self.windows.push(crate::AnyWindowSpec::secondary(spec, window.clone()));
        (app, window)
    }

    /// Build the first state with `f` instead of `AppSpec::init`, so it can capture what the
    /// host already made (a channel, a handle to its runtime). Scenes still apply on top.
    pub fn init(mut self, f: impl FnOnce() -> S + 'static) -> Self {
        self.init = Some(Box::new(f));
        self
    }

    /// Read Rune's flags from `argv` (without the program name) instead of the process's
    /// arguments. An app that owns its command line passes only what it means for Rune, often
    /// nothing.
    pub fn args(mut self, argv: impl IntoIterator<Item = String>) -> Self {
        self.argv = Some(argv.into_iter().collect());
        self
    }

    /// Start without a window; `AppHandle::open` or `activate` shows it. Implies `keep_running`.
    pub fn windowless(mut self) -> Self {
        self.open = false;
        self.keep_running = true;
        self
    }

    /// Closing the window, or a failed window creation, keeps the app and event loop running
    /// until `AppHandle::quit` or `Tx::quit` (for tray apps and apps that retry on `window_error`).
    pub fn keep_running(mut self) -> Self {
        self.keep_running = true;
        self
    }

    /// Closing the window also releases the GPU device and renderer (and destroys the window
    /// everywhere), so an app idling without a window holds no UI GPU memory. Reopening
    /// recreates them, which costs a device creation.
    pub fn release_gpu_when_closed(mut self) -> Self {
        self.release_gpu = true;
        self
    }

    /// Controls CPU-first startup and GPU handover for this app's window ([`crate::CpuFirst`]).
    ///
    /// Defaults to [`crate::CpuFirst::Auto`], which opens opaque Windows/Linux windows and
    /// full-window web pages on `CpuPainter` before handing over to the GPU in the background.
    /// [`crate::CpuFirst::Only`] keeps opaque windows on `CpuPainter` without starting background
    /// GPU handover on idle (on Windows and Linux, a frame that hits a GPU-only feature still
    /// triggers GPU handover so it can render; on web, stays on the 2D canvas without requesting
    /// WebGPU/WebGL2). [`crate::CpuFirst::Off`] skips CPU-first startup and opens directly on the
    /// GPU without falling back to the CPU renderer. Transparent windows always open on the GPU
    /// on Windows and whenever a GPU surface can be made on Linux; on Linux under `Auto` or `Only`
    /// (with automatic adapter selection), a visible transparent window whose pre-swapchain GPU
    /// startup fails falls back to `CpuPainter` with alpha buffers (or 24-bit opaque if X11 gives
    /// no 32-bit visual). Ignored on macOS, iOS, headless runs, and inline/document web pages.
    ///
    /// `RUNE_CPU_FIRST` (or `globalThis.RUNE_CPU_FIRST` on web) overrides this setting without a
    /// rebuild (`0`/`off`, `only`, `auto`), matching `RUNE_GPU_BACKEND`.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, CpuFirst, el};
    ///
    /// App::new(AppSpec::new("Plan", (960.0, 640.0), || (), |_, _| el()))
    ///     .cpu_first(CpuFirst::Only)
    ///     .run();
    /// ```
    pub fn cpu_first(mut self, mode: crate::CpuFirst) -> Self {
        self.cpu_first = mode;
        self
    }



    /// Names the optional effect GPU pipelines this app's windows and overlays compile in the
    /// background before their first use ([`PrewarmEffects`](crate::PrewarmEffects)): isolated
    /// layers (`Rgba16Float`), `Blend::PlusLighter`, Gaussian layer blur, and
    /// `El::backdrop_blur`. `true` names all four.
    ///
    /// Defaults to none, so an app pays no background pipeline-compile CPU, driver memory, or
    /// worker thread for effects it never draws. Name the effects the app does draw so that
    /// the first frame that shows one (a frosted button fading in mid-scroll) does not stall
    /// while the driver compiles its pipelines: they compile on a low-priority thread once
    /// the window has drawn its first frames and rests, and a frame that needs one sooner
    /// compiles it as before, once. Ignored on `wasm32`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, PrewarmEffects, el};
    ///
    /// App::new(AppSpec::new("Frosted", (640.0, 480.0), || (), |_, _| el()))
    ///     .prewarm_effects(PrewarmEffects::BACKDROP_BLUR)
    ///     .run();
    /// ```
    pub fn prewarm_effects(mut self, effects: impl Into<crate::PrewarmEffects>) -> Self {
        self.prewarm_effects = effects.into();
        #[cfg(not(target_arch = "wasm32"))]
        for overlay in &mut self.overlays {
            overlay.set_prewarm_effects(self.prewarm_effects);
        }
        self
    }

    /// Draws with `backend` on macOS. Without a call, a Mac app draws with Rune's native Metal
    /// backend (the `metal-backend` feature, on by default), so `GpuBackend::Wgpu` opts out;
    /// `GpuBackend::Metal` needs the feature. `RUNE_GPU_BACKEND=wgpu` or `=metal` overrides it
    /// without a rebuild. It applies to every GPU the process makes from here on, and a
    /// `prewarm_gpu` already running on the other backend starts over on this one; an app that
    /// opts out calls `rune::prewarm_gpu_on(GpuBackend::Wgpu)` first in `main` instead of
    /// `prewarm_gpu()` to avoid the restart. An iOS app, which embeds a `LayerHost` instead,
    /// calls `rune::prewarm_gpu_on`. Does nothing elsewhere.
    pub fn gpu_backend(self, backend: crate::GpuBackend) -> Self {
        crate::render::choose_gpu_backend(backend);
        self
    }

    /// Sets the startup GPU suballocation policy for wgpu devices ([`crate::GpuMemoryPolicy`]).
    ///
    /// Without a call, Rune uses [`crate::GpuMemoryPolicy::MemoryUsage`]. Call this (or
    /// [`crate::set_gpu_memory_policy`] before [`crate::prewarm_gpu`], [`crate::warm_gpu`], or
    /// [`crate::warm_gpu_cache`]) to pick [`crate::GpuMemoryPolicy::Performance`] or a
    /// [`crate::GpuMemoryPolicy::Manual`] block-size range before windows or overlays create
    /// their GPU devices.
    ///
    /// Applies process-wide to every wgpu device created from here on, and restarts or discards
    /// any prewarmed or parked device built under a different policy. Honored by D3D12 (Windows)
    /// and Vulkan (Linux); ignored by Rune's native Metal backend, wgpu's Metal backend, and
    /// web backends. Resources larger than a `Manual` block range still succeed via dedicated
    /// allocations.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, GpuMemoryPolicy, el};
    ///
    /// App::new(AppSpec::new("Low memory", (640.0, 480.0), || (), |_, _| el()))
    ///     .gpu_memory_policy(GpuMemoryPolicy::Manual {
    ///         block_size: (4 * 1024 * 1024)..(8 * 1024 * 1024),
    ///     })
    ///     .run();
    /// ```
    pub fn gpu_memory_policy(self, policy: crate::GpuMemoryPolicy) -> Self {
        crate::render::set_gpu_memory_policy(policy);
        self
    }

    /// macOS: run as an accessory (menu-bar) app: no Dock icon, no default menu bar, and
    /// launching doesn't activate the app or take focus. The app installs its own menu if it
    /// wants one (`standard_menus` gives it Close, Minimize and Edit), and may change the
    /// activation policy later. No effect on other platforms.
    pub fn accessory(mut self) -> Self {
        self.accessory = true;
        self
    }

    /// macOS: add the standard menu items every Mac app has to the menu bar of this process
    /// (hidden for an accessory app, whose key equivalents still work): an Edit menu (Undo ⌘Z,
    /// Redo ⇧⌘Z, Cut, Copy, Paste, Select All) and Window › Minimize ⌘M and Close ⌘W, each
    /// sent to the first responder. AppKit windows in the process (an open panel, an updater)
    /// get their editing and close keys back. In a Rune window every one of these keys still
    /// reaches text fields, `on_key` and shortcuts first, once, exactly as without the menu;
    /// a ⌘W or ⌘M nothing takes then closes or minimizes a window that has that title-bar
    /// button, as a click on it would (`on_close` and `keep_running` apply). No Quit item.
    /// Existing menus and custom `MenuItem`s are kept; items go into menus of the same title.
    /// Installed once per process when the first window of an app with it starts. A regular
    /// (non-accessory) app whose menu bar is still winit's default gets these items without
    /// asking. No effect on other platforms.
    pub fn standard_menus(mut self) -> Self {
        self.standard_menus = true;
        self
    }

    /// Intercepts a user-initiated window-close request (the native close button, Alt+F4, the
    /// window menu's Close, macOS unhandled ⌘W / Window › Close, and assistive activation of the
    /// custom close button) before the window hides, changes lifecycle or first-frame state, or
    /// runs [`App::on_close`].
    ///
    /// Runs inside a normal update transaction so the callback may update state or issue [`Tx`]
    /// requests (for example showing an unsaved-changes confirmation dialog). Returning
    /// [`CloseResponse::Defer`] keeps the window open, commits those changes, and requests a
    /// redraw; close the window later with [`Tx::close_window`], [`AppHandle::close`], or
    /// [`WindowHandle::close`](crate::WindowHandle::close). Returning [`CloseResponse::Close`]
    /// proceeds with closing as usual (running [`App::on_close`]).
    ///
    /// Programmatic close (`Tx::close_window`, `AppHandle::close`, `WindowHandle::close`),
    /// application quit/relaunch, teardown, and an already-exiting event loop bypass this hook
    /// so an approved close completes without re-asking. Apps that do not register this hook
    /// keep the existing close behaviour. Headless scripts deliver it with
    /// `{"close_request": true}`. Ignored on web.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, CloseResponse, el};
    ///
    /// struct Draft { dirty: bool, confirm_close: bool }
    ///
    /// let spec = AppSpec::new("Draft", (480.0, 320.0), || Draft { dirty: true, confirm_close: false }, |_, _| el());
    /// App::new(spec)
    ///     .on_close_request(|draft, _| {
    ///         if draft.dirty {
    ///             draft.confirm_close = true;
    ///             CloseResponse::Defer
    ///         } else {
    ///             CloseResponse::Close
    ///         }
    ///     })
    ///     .run();
    /// ```
    pub fn on_close_request(mut self, f: fn(&mut S, &mut Tx) -> CloseResponse) -> Self {
        self.on_close_request = Some(f);
        self
    }

    /// Runs whenever the window closes (close button, `Tx::close_window`, `AppHandle::close`,
    /// macOS Quit from the menu, Cmd-Q or the Dock), before it goes: flush drafts, stop previews.
    /// Its `Tx::quit` quits.
    pub fn on_close(mut self, f: fn(&mut S, &mut Tx)) -> Self {
        self.on_close = Some(f);
        self
    }

    /// Runs once in every open window when the app starts to quit, before any window closes
    /// for it, and in the last main window whose close ends the app (after its `on_close`):
    /// `Tx::quit` or `AppHandle::quit` from any window, the last main window closing, macOS
    /// Quit (⌘Q, the Dock, logout, restart and shutdown), and a Windows logoff or shutdown
    /// (`WM_QUERYENDSESSION`, or `WM_ENDSESSION` when no query came first), as Electron's
    /// `before-quit` and `session-end` do between them. Save what must survive the quit here,
    /// synchronously: a Windows session may end the process once the hooks returned. From then
    /// on [`crate::quitting`] holds, so an `on_close` that follows (macOS, the end of a Windows
    /// session, or another window closing for the quit) can tell the app ending from its window
    /// closing alone. A Windows shutdown another app refuses clears it, and a later quit runs
    /// this again. What it asks of its `Tx` beyond state is moot once the loop ends. Native
    /// windows only.
    pub fn on_quit(mut self, f: fn(&mut S, &mut Tx)) -> Self {
        self.on_quit = Some(f);
        self
    }

    /// Runs `enter` before each turn of the window's event handling that can reach its state
    /// (an event, a wake, a frame, a hook), and `leave` after it, so state the app's windows
    /// share can move into the state of the window whose turn it is and back. The windows of
    /// one event loop take turns, never two at once. Native windows only.
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    pub fn turn_scope(mut self, enter: fn(&mut S), leave: fn(&mut S)) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        { self.turn_scope = Some(TurnScope { enter, leave }); }
        #[cfg(target_arch = "wasm32")]
        let _ = (enter, leave);
        self
    }

    /// macOS: runs on the main thread when the app is reopened while it runs: its Dock icon
    /// clicked, or opened again from Finder, Spotlight or `open`, also in an accessory app.
    /// `has_visible_windows` says whether any Rune window that reopening could bring back is on
    /// screen and not minimized (launcher panels and overlays don't count). Return true when the
    /// hook handled it, which skips Rune's default; false lets the default run. When no
    /// such window shows, the default shows and activates a minimized window, else the window Rune
    /// hid last (closed with `keep_running`, or hidden), else the main window. It never brings
    /// back a launcher or other nonactivating panel, a window that closed for good, or anything
    /// in an accessory app. With several windows, every window's hook runs, in window order,
    /// and any true skips the default. Windows: runs when the app is launched again without a
    /// link while it runs, once `rune::single_instance` made it the app's one process; the
    /// default then also brings the front window forward, restored. Linux: the same, and on
    /// `org.freedesktop.Application.Activate`; the default uses the launch's activation token.
    /// Never called on other platforms. Headless scripts deliver it with `{"reopen": true}`.
    pub fn on_reopen(mut self, f: fn(&mut S, bool, &mut Tx) -> bool) -> Self {
        self.on_reopen = Some(f);
        self
    }

    /// macOS: runs on the main thread with each link the system opens with this app
    /// (`myapp://...`, from `open`, a browser or another app), including the one that launched
    /// it, which runs before the window's first frame. Each runs in arrival order as an
    /// `AppHandle::update`, so a window it opens through a handle opens in the same turn. The
    /// string is passed through as received; it comes from outside the app, so parse and
    /// validate it before acting on it. Bringing a window forward is up to the app
    /// (`AppHandle::open`). The app's bundle must declare the scheme (`CFBundleURLTypes`, see
    /// the README). With several windows whose apps have the hook, the first still running
    /// receives links.
    ///
    /// Windows: the links among the launch arguments (`scheme://…`), as Windows passes a link to
    /// the process it starts, and with `rune::single_instance` the links of every later launch,
    /// which that launch hands to this process instead of opening a window. The app registers its
    /// scheme itself (`HKEY_CURRENT_USER\Software\Classes\<scheme>`, usually at install), and
    /// names it with `rune::url_schemes` so only arguments on it are links: without that a flag's
    /// URL value (`--server http://localhost:4096`) is one too. Linux: the same (never a `file:`
    /// URI), and `org.freedesktop.Application.Open`'s links; see the README. Never called on
    /// other platforms.
    pub fn on_open_url(mut self, f: fn(&mut S, &str, &mut Tx)) -> Self {
        self.on_open_url = Some(f);
        self
    }

    /// macOS: runs on the main thread with each batch of files the system opens with this app
    /// (Finder double-click or "Open With", files dropped on the Dock icon, or `open -a MyApp
    /// file.ext`), including the `kAEOpenDocuments` (`'odoc'`) Apple Event that launched it,
    /// which runs before the window's first frame. Each batch runs in arrival order as an
    /// `AppHandle::update`, so a window it opens through a handle opens in the same turn. Never
    /// called with an empty slice (non-file URLs in the event are skipped). Paths come from
    /// outside the app, so validate them before acting. Bringing a window forward is up to the app
    /// (`AppHandle::open`). The app's bundle must declare its document types
    /// (`CFBundleDocumentTypes`, see the README). With several windows whose apps have the hook,
    /// the first still running receives files.
    ///
    /// Linux: the `file://` URIs among the launch arguments (`Exec=myapp %U`, not `%F`), with
    /// `rune::single_instance` a later launch's too, and `org.freedesktop.Application.Open`'s;
    /// see the README. Never called on Windows, which passes opened files to a new process as
    /// launch arguments. Headless scripts deliver it with `{"open_files": ["a.torrent"]}`.
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use rune::{App, AppSpec, el};
    ///
    /// let spec = AppSpec::new("Viewer", (640.0, 420.0), Vec::<PathBuf>::new, |_, _| el());
    /// let app = App::new(spec).on_open_files(|opened, paths, _| {
    ///     opened.extend(paths.iter().cloned());
    /// });
    /// let _ = app;
    /// ```
    pub fn on_open_files(mut self, f: fn(&mut S, &[std::path::PathBuf], &mut Tx)) -> Self {
        self.on_open_files = Some(f);
        self
    }

    /// On web, routes [`Tx::open_url_same_tab`] links inside a single-page app: `route` gets the
    /// absolute URL of each link on the page's own origin and returns true when it handled it
    /// (`history.pushState` and a render), or false to let the browser load it with
    /// `location.assign`. Links to other origins always load. Ignored on native.
    pub fn same_tab_router(mut self, route: impl Fn(&str) -> bool + 'static) -> Self {
        self.same_tab_router = Some(Box::new(route));
        self
    }

    /// Runs on the UI thread each time the native window is created (again after a Wayland
    /// close or a GPU release): set its icon or native styles through
    /// `WindowHandle::with_raw_handle`.
    pub fn on_window(mut self, f: impl FnMut(&crate::WindowHandle) + 'static) -> Self {
        self.on_window = Some(Box::new(f));
        self
    }

    /// First-frame readiness, once per native creation. Launcher panels call this while
    /// hidden, after GPU completion of its cached surface frame. A window that isn't on screen
    /// (covered, occluded, hidden or on another Space) doesn't wait to draw: this runs at its
    /// first frame skipped for that, or a second after it opens with no frame at all.
    /// Headless tooling skips this unless `--realtime` is passed, which runs it after
    /// the first frame. May queue a show through the supplied handle.
    pub fn on_ready(mut self, f: impl FnMut(&crate::WindowHandle) + 'static) -> Self {
        self.on_ready = Some(Box::new(f));
        self
    }

    /// Runs once on the UI thread immediately before the event loop drops windows and tears down
    /// the native display connection, after any `on_close` and `on_quit` hooks have run.
    ///
    /// Pair this with [`WindowHandle::with_raw_display_handle`](crate::WindowHandle::with_raw_display_handle)
    /// to destroy foreign protocol proxies or event queues bound on winit's display (such as
    /// Wayland objects created via `Backend::from_foreign_display`) before the display closes.
    /// Runs on normal quits, OS-initiated termination, fast exit, and display or startup error
    /// exits; closing one window while another window or `keep_running` keeps the event loop
    /// alive does not run it until the loop itself exits. Native windows only (ignored on
    /// headless and web).
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// App::new(AppSpec::new("Display", (320.0, 200.0), || (), |_, _| el()))
    ///     .on_window(|window| {
    ///         let _ = window.with_raw_display_handle(|d| format!("{:?}", d.as_raw()));
    ///     })
    ///     .on_display_close(|| {
    ///         // Destroy foreign display proxies before the event loop closes the display.
    ///     })
    ///     .run();
    /// ```
    pub fn on_display_close(mut self, f: impl FnOnce() + 'static) -> Self {
        self.on_display_close = Some(Box::new(f));
        self
    }

    /// Runs on the UI thread after each frame of the app's window, saying what it came to
    /// ([`FrameOutcome`](crate::FrameOutcome)): `Drawn` once it was encoded, submitted and
    /// presented, the first frame during creation and a hidden launcher's frames included;
    /// `Unchanged` for a frame only a timer asked for that drew what was on screen; `Skipped`
    /// for one a covered, minimised or occluded window, or a drawable that never came, kept
    /// from drawing. For logging frame timing: pair [`FrameEncoded::at`](crate::FrameEncoded::at)
    /// with `Tx::input_time` for input latency. It changes nothing about when or what the window
    /// draws; without it, each frame checks an `Option`. A single-window headless run reports
    /// every frame on its virtual clock as `Drawn`, with wall-clock instants and `refresh_hz` at
    /// its `--fps`, so an app's own meter reads its build and encode costs there too. Overlays,
    /// multi-window headless runs and the web never call it.
    pub fn on_frame_encoded(mut self, f: impl FnMut(&crate::FrameEncoded) + 'static) -> Self {
        self.on_frame_encoded = Some(Box::new(f));
        self
    }

    /// Runs on the UI thread in submission order once a `Drawn` frame actually reaches the screen
    /// (or is skipped by the system, reported as
    /// [`FramePresented::presented_at`](crate::FramePresented::presented_at) returning `None`).
    ///
    /// [`FramePresented::seq`](crate::FramePresented::seq) matches
    /// [`FrameEncoded::seq`](crate::FrameEncoded::seq) for the same `Drawn` frame, and
    /// `on_frame_encoded` always runs before `on_frame_presented` for a given sequence number.
    /// Pair them to measure real input-to-photon latency or detect missed vsyncs against
    /// [`FrameEncoded::presents_at`](crate::FrameEncoded::presents_at).
    ///
    /// Only platforms with reliable actual presentation timestamps call it (currently macOS
    /// Metal, via `MTLDrawable::addPresentedHandler`). Windows, Linux, the web, overlays, and
    /// headless runs never call it, matching
    /// [`FrameEncoded::presents_at`](crate::FrameEncoded::presents_at). Without it, no layer
    /// hook is attached and each frame pays only an `Option` check; once in-flight drawables
    /// finish presenting, the watch clears automatically so an idle window never wakes or draws.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// let spec = AppSpec::new("Demo", (320.0, 240.0), || (), |_, _| el());
    /// App::new(spec)
    ///     .on_frame_presented(|p| {
    ///         let _ = (p.seq(), p.window(), p.presented_at());
    ///     })
    ///     .run();
    /// ```
    pub fn on_frame_presented(mut self, f: impl FnMut(&crate::FramePresented) + 'static) -> Self {
        self.on_frame_presented = Some(Box::new(f));
        self
    }

    /// Runs on the UI thread after each `Drawn` frame of the app's window (desktop windows and
    /// the web runner, and never on unchanged or skipped frames), with the runtime as that frame
    /// presented it (`Runtime::elements`, `Runtime::state`), for an app that watches its own
    /// motion or waits for presented frames. Pair it with [`last_input`](crate::last_input) to
    /// tell motion the user caused from motion they didn't. Like `on_frame_encoded`, it changes
    /// nothing about when or what the window draws.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// let spec = AppSpec::new("Demo", (320.0, 240.0), || 0u32, |_, _| el());
    /// App::new(spec)
    ///     .on_frame_drawn(|rt| {
    ///         let _count = rt.elements().len();
    ///     })
    ///     .run();
    /// ```
    pub fn on_frame_drawn(mut self, f: impl FnMut(&crate::Runtime<S>) + 'static) -> Self {
        self.on_frame_drawn = Some(Box::new(f));
        self
    }

    /// Cap frame pacing (Hz) for motion no recent user input is steering (`0.0`, default =
    /// uncapped at the display's refresh rate; positive finite values clamp to at least `1.0` Hz,
    /// and zero, negative, or non-finite values clear the cap back to uncapped).
    ///
    /// **What counts as steered.** Any live key, pointer, mouse-wheel, touch, trackpad gesture,
    /// or file drag-and-drop event that reaches the window lifts the cap immediately on the very
    /// next display refresh. Motion stays steered at the display's full rate for 600 ms after the
    /// last input, and up to 2 s after that input while an unbroken run of presented frames
    /// started within 100 ms of the input and keeps presenting with gaps of at most 100 ms.
    /// Continuous pointer drags, wheel or trackpad scrolls, typing, and springs following a finger
    /// keep refreshing the input timestamp and never cap. Motion that starts after a pause (such
    /// as a streaming reply beginning after a wait) or continues past 2 s with no new input paces
    /// at `fps`, and when motion ends the link coasts for ~150 ms and stops with zero idle draws.
    ///
    /// **Interaction with other caps.** When multiple caps apply, the lower rate (longer frame
    /// period) wins: Low Power Mode (60 Hz on built-in ProMotion panels) always bounds both
    /// steered and ambient frames, and combining this with `App::reveal_fps` lets reveal-only
    /// tails wake on their timer at `reveal_fps` while any concurrent unsteered animation (such as
    /// timeline growth or scroll following a stream) paces at `ambient_fps`.
    ///
    /// Enforced on macOS `DisplayLink` windows today; Windows, Linux, web, overlays, and
    /// headless runs ignore it (matching [`App::on_frame_presented`] and
    /// [`App::exclude_from_capture`]).
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, text};
    ///
    /// let spec = AppSpec::new("Chat", (720.0, 540.0), String::new, |s, _| text(s.as_str()).wrap());
    /// App::new(spec)
    ///     .ambient_fps(60.0)
    ///     .run();
    /// ```
    pub fn ambient_fps(mut self, fps: f32) -> Self {
        self.ambient_period = ambient_period_from_fps(fps);
        self
    }

    /// Cap the swap chain's frame latency on macOS (`1` = 2 drawables, `2` = 3 drawables;
    /// clamped to `1..=2`, default `2`). Ignored on Windows, Linux, and the web.
    ///
    /// With two drawables a frame that runs late waits instead of overlapping the next; at
    /// 120 Hz that can show as a dropped frame during fast animation, whereas for a rarely
    /// animating window it is the right trade. Applied when the surface is created (and on
    /// GPU surface reattach) rather than changed live (changing `CAMetalLayer.maximumDrawableCount`
    /// while visible forces a synchronous reconfigure and can reallocate drawables or drop a frame).
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// App::new(AppSpec::new("Panel", (488.0, 873.0), || (), |_, _| el()))
    ///     .frame_latency(1)
    ///     .run();
    /// ```
    pub fn frame_latency(mut self, frames: u32) -> Self {
        self.frame_latency = Some(frames.clamp(1, 2));
        self
    }

    /// Keep the view's layout reads across launches, so the window's first frame builds the
    /// view once. A view that reads a rect ([`Cx::layout`](crate::Cx::layout), `layout_with`,
    /// `layout_size`) gets nothing in a window's first commit and runs again once layout knows
    /// it. `keep` is given what the window's first view read (its first frame's reads, as layout
    /// settled them: what the next launch's first frame reads again), once, when the window
    /// rests, closes or quits, unless they are the seed's own: save it with the window's own
    /// state, and pass it back here as `seed` at the next launch. The first view is then given
    /// those rects, and runs once when layout agrees with them. A seed for another window size
    /// or scale isn't used, and one from another state costs what no seed costs: every read is
    /// still checked against the layout. Desktop windows only.
    pub fn layout_seed(mut self, seed: Option<crate::LayoutSeed>, keep: impl FnMut(crate::LayoutSeed) + 'static) -> Self {
        self.layout_seed = seed;
        self.keep_layout = Some(Box::new(keep));
        self
    }

    /// Draw every frame of the app's window (and headless renders) through `compositor`
    /// instead of Rune's default pass (see `compose`).
    pub fn compositor(mut self, compositor: impl crate::compose::Compositor<S>) -> Self {
        self.compositor = Some(Box::new(compositor));
        self
    }

    /// A handle for other threads; requests made before `run` apply once it starts.
    pub fn handle(&self) -> AppHandle<S> {
        AppHandle {
            shared: self.shared.clone(),
        }
    }

    /// The app's own counters (memory, caches, queues), shown beside the window's frame stats
    /// in a native trace's summary. Keep it cheap. For example, with the `procwatch`
    /// crate: `.dev_stats(|| procwatch::snapshot().map_or_else(String::new, |s| format!("{} MB, {} fds", s.footprint >> 20, s.fds)))`.
    pub fn dev_stats(mut self, stats: fn() -> String) -> Self {
        self.runtime.dev_stats = Some(stats);
        self
    }

    /// A storybook in this window for auditing its scenes, stories and overlays: ⌘⇧D, a bar
    /// along the bottom, or the View menu's Playground item opens it over the app. Shown in debug
    /// builds unless `RUNE_PLAYGROUND=0`; `--playground [NAME]` opens it at launch, headless
    /// too, where it otherwise never appears, so snapshots and goldens don't change.
    pub fn playground(mut self, playground: crate::Playground<S>) -> Self {
        if playground.is_enabled() && crate::playground::offered_by_env() {
            let item = crate::MenuItem::new("Playground", |_: &mut S, tx: &mut Tx| tx.toggle_playground())
                .id("rune.playground")
                .menu("View");
            let item = match playground.shortcut_key() { Some(shortcut) => item.key_equivalent(shortcut.clone()), None => item };
            self = self.menu_item(item);
        }
        self.playground = Some(playground);
        self.attach_playground = Some(crate::playground::attach::<S>);
        self
    }

    /// Runs until the app quits. Agent-tooling arguments (`--snap`, `--tree`, ...) render
    /// headless instead: the window requests are ignored there, and updates apply each frame.
    /// Prints the error to stderr and exits 1 when the app can't start (no display, or the first
    /// window can't be created) or loses its display connection, or exits 2 when a headless
    /// tooling run (`--snap`, `--tree`, ...) fails (panics on wasm32); use [`App::try_run`] to
    /// receive the error instead of exiting. See `keep_running`.
    pub fn run(self) {
        #[cfg(target_arch = "wasm32")]
        crate::tooling::started(self.try_run());
        #[cfg(not(target_arch = "wasm32"))]
        crate::tooling::finish_start(self.try_run_start());
    }

    /// `run`, returning an error instead of exiting when the event loop can't start or fails, or
    /// the first window can't be created (unless `keep_running` or `windowless` is set).
    #[cfg(target_arch = "wasm32")]
    pub fn try_run(mut self) -> Result<(), String> {
        if let Some(argv) = self.argv.take() {
            let _ = Args::parse_from(argv);
        }
        crate::platform::web::run(self)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn try_run(self) -> Result<(), String> {
        self.try_run_start().map_err(crate::tooling::StartError::message)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn try_run_start(mut self) -> Result<(), crate::tooling::StartError> {
        // Windows' word dictionaries start loading now, on their own thread (`rune_text::segmenter`).
        #[cfg(all(target_os = "windows", not(feature = "segmenter-dictionaries")))]
        rune_text::segmenter::install();
        let args = match self.argv.take() {
            Some(argv) => Args::parse_from(argv),
            None => Args::parse(),
        };
        if args.headless() {
            #[cfg(any(feature = "tooling", debug_assertions))]
            {
                crate::render::discard_prewarm();
                crate::tooling::try_headless(self, args)
            }
            #[cfg(not(any(feature = "tooling", debug_assertions)))]
            Err(crate::tooling::StartError::Window(crate::tooling::WITHOUT_TOOLING.into()))
        } else {
            crate::window::run(self, args).map_err(crate::tooling::StartError::Window)
        }
    }
}

impl<S: 'static> App<S> {
    pub(crate) fn resolve_args(&mut self, inherited: Args) -> Args {
        self.argv.take().map_or(inherited, Args::parse_from)
    }

    /// On web: the canvas sits inline in the page (`canvas_id` without `full_viewport`, or with
    /// `fit_content_height`) rather than covering the viewport.
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    pub(crate) fn web_inline(&self) -> bool {
        self.canvas_id.is_some() && (self.fit_content_height || !self.full_viewport)
    }

    pub(crate) fn into_parts(self, scene: Option<&str>) -> (AppSpec<S>, S, Arc<Shared<S>>, RuntimeConfig<S>) {
        let state = crate::tooling::perf_warn::launch::init(|| match self.init {
            Some(init) => crate::tooling::with_scene(&self.spec, init(), scene),
            None => crate::tooling::initial_state(&self.spec, scene),
        });
        (self.spec, state, self.shared, self.runtime)
    }
}

/// Opens, focuses, closes and feeds a running `App` from any thread. Every call wakes the event
/// loop; nothing polls. Window requests collapse to the latest.
pub struct AppHandle<S: 'static> {
    pub(crate) shared: Arc<Shared<S>>,
}

impl<S> Clone for AppHandle<S> {
    fn clone(&self) -> Self {
        AppHandle {
            shared: self.shared.clone(),
        }
    }
}

impl<S: 'static> AppHandle<S> {
    /// Wake the event loop so `AppSpec::poll` runs, also while the window is hidden; for a
    /// background thread that queued work for `poll` to drain.
    #[inline]
    pub fn wake(&self) { self.shared.core.wake_app(); }

    /// Show the window (creating it if needed), restore it if minimized, and activate the app
    /// with it in front, including the first open of a window that doesn't exist yet. A
    /// nonactivating panel takes keys without activating. The window open at launch doesn't
    /// activate the app beyond what the platform does at launch.
    #[inline]
    pub fn open(&self) { self.shared.core.open(); }

    /// Show or recreate the window without requesting focus. A minimized window stays minimized;
    /// open() also restores and activates it. Window-manager policy still applies.
    #[inline]
    pub fn show(&self) { self.shared.core.show(); }

    /// Hide the window, retaining its host and state. Does not invoke the close callback.
    #[inline]
    pub fn hide(&self) { self.shared.core.hide(); }

    /// Focus/activate an already-visible window, restoring it if minimized.
    /// On Wayland this requests attention; use activate(token) for compositor-authorized focus.
    #[inline]
    pub fn focus(&self) { self.shared.core.focus(); }

    /// Set the outer position in physical desktop pixels (negative coordinates are valid).
    /// Retained across recreation. Wayland does not support application window placement.
    #[inline]
    pub fn set_position(&self, position: (i32, i32)) { self.shared.core.set_position(position); }

    /// Place in display-local points. UnderPointer samples the pointer at this call;
    /// disconnected displays fall back to primary when the UI thread applies the request.
    /// Wayland does not permit application placement.
    #[inline]
    pub fn place(&self, display: crate::DisplayTarget, placement: crate::OverlayPlacement) {
        self.shared.core.place(display, placement);
    }

    /// Show and focus the window with an activation token another process forwarded
    /// (`XDG_ACTIVATION_TOKEN` on Wayland, `DESKTOP_STARTUP_ID` on X11), so the compositor
    /// lets it take focus. Where tokens don't exist this is `open`.
    #[inline]
    pub fn activate(&self, token: impl Into<String>) { self.shared.core.activate(token.into()); }

    /// Close the window as the close button would (`App::on_close` runs; without
    /// `keep_running` the app quits).
    #[inline]
    pub fn close(&self) { self.shared.core.close(); }

    /// Minimize the window, as `Tx::minimize` does from a handler. Does nothing for a hidden
    /// window, a panel or launcher, or one that can't minimize. After a `show` it minimizes
    /// the shown window; a later `open`, `focus`, `hide` or `close` cancels it, and it cancels
    /// an earlier `focus`.
    #[inline]
    pub fn minimize(&self) { self.shared.core.minimize(); }

    /// Quit the app: every open window's `App::on_quit` runs, the window closes without
    /// `on_close`, and `run` returns.
    ///
    /// On web this unmounts the app: its loop stops, its GPU device is destroyed, its listeners
    /// come off the page, the body style and title it changed go back, and the canvas Rune made
    /// is removed, while one passed in with [`App::canvas_id`] stays, restyled as it was, for
    /// its owner. Another app may start straight away; it mounts once this one has gone. An
    /// app quit before it mounts never does.
    #[inline]
    pub fn quit(&self) { self.shared.core.quit(); }

    /// Quit the app as [`AppHandle::quit`] does, then start it again with the same arguments
    /// once this process has exited ([`Tx::relaunch`]). Web: as `quit`.
    #[inline]
    pub fn relaunch(&self) { self.shared.core.relaunch(); }

    /// macOS: show or hide the app's Dock icon (and app menu) while it runs, switching between
    /// the regular and accessory activation policies. Does nothing elsewhere.
    #[inline]
    pub fn set_dock_icon(&self, visible: bool) { self.shared.core.set_dock_icon(visible); }

    /// Override the native light or dark appearance across the whole application.
    ///
    /// On macOS this sets `NSApplication.sharedApplication.appearance` (`nil` for
    /// [`ColorScheme::System`](crate::ColorScheme::System), `NSAppearanceNameAqua` for `Light`,
    /// `NSAppearanceNameDarkAqua` for `Dark`), so native menus, open/save panels, traffic lights,
    /// system materials, and windows with no per-window override follow it. On Windows and Linux
    /// this applies to every window whose own color scheme is `ColorScheme::System`, including
    /// windows created or recreated later. Windows with an explicit per-window `Light` or `Dark`
    /// override keep their own scheme. Safe to call from any thread; no-op on web and headless.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, ColorScheme, el};
    ///
    /// let app = App::new(AppSpec::new("Notes", (480.0, 360.0), || (), |_, _| el()));
    /// let handle = app.handle();
    /// handle.set_color_scheme(ColorScheme::Dark);
    /// app.run();
    /// ```
    #[inline]
    pub fn set_color_scheme(&self, scheme: crate::ColorScheme) { self.shared.core.set_color_scheme(scheme); }

    /// Whether the window is on screen (shown, not hidden or closed). Minimized and occluded
    /// windows count as shown.
    #[inline]
    pub fn is_visible(&self) -> bool { self.shared.core.is_visible() }

    /// Why the window couldn't be created last time (no display, no GPU adapter or device),
    /// until a later creation succeeds. Needs `keep_running` or `windowless` to stay alive.
    #[inline]
    pub fn window_error(&self) -> Option<String> { self.shared.core.window_error() }

    /// Change the state from this thread: `f` runs on the UI thread, in call order, even while no
    /// window shows, and the next frame commits it with the motion its `Tx` asks for.
    pub fn update(&self, f: impl FnOnce(&mut S, &mut Tx) + Send + 'static) {
        self.update_if(move |s, tx| { f(s, tx); true });
    }

    /// `update` for a timer or feed that often finds nothing new: `f` returns whether it changed
    /// state the view reads, as `AppSpec::poll` does. On false the view doesn't re-run and the
    /// window doesn't redraw (a hidden one doesn't refresh), while what `f` asked of its `Tx`
    /// still happens: clipboard, close and quit, menus, `wake_after`, and scrolls, which take
    /// a frame. Several updates drained together commit once if any returned true.
    ///
    /// Returning false after changing state the view reads leaves the screen stale until the
    /// next change.
    pub fn update_if(&self, f: impl FnOnce(&mut S, &mut Tx) -> bool + Send + 'static) {
        self.shared.push_update(Box::new(f));
    }
}

mod config;
pub(crate) use config::RuntimeConfig;
mod requests;
pub(crate) use requests::{window_request_order, Activation, Shared, SharedCore, WindowRequest, WindowRequests};

#[cfg(test)]
mod tests;
