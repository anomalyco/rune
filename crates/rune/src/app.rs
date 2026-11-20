//! The app's lifetime beside its window. `App` runs an `AppSpec` with or without a window and
//! decides what closing it means; `AppHandle` opens, focuses, closes and feeds the running app
//! from any thread.
//!
//! The state, scene and text live with the app, not the window: closing the window keeps them,
//! and reopening shows the same UI, settled. Closing hides the window where the platform can
//! (macOS, Windows, X11) and destroys it on Wayland, where a hidden toplevel doesn't exist;
//! the GPU device and renderer stay either way.
//!
//! ```ignore
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

#[path = "app/web.rs"]
mod web;
#[cfg(target_arch = "wasm32")]
pub(crate) use web::with_font;

pub(crate) type Update<S> = Box<dyn FnOnce(&mut S, &mut Tx) -> bool + Send>;
/// `App::on_window`'s hook, run after each native window creation.
pub(crate) type OnWindow = Box<dyn FnMut(&crate::WindowHandle)>;

/// `App::turn_scope`'s hooks.
pub(crate) struct TurnScope<S> {
    pub(crate) enter: fn(&mut S),
    pub(crate) leave: fn(&mut S),
}

impl<S> Clone for TurnScope<S> {
    fn clone(&self) -> Self { *self }
}

impl<S> Copy for TurnScope<S> {}

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
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) accessory: bool,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) standard_menus: bool,
    pub(crate) keep_running: bool,
    pub(crate) on_close: Option<fn(&mut S, &mut Tx)>,
    pub(crate) on_quit: Option<fn(&mut S, &mut Tx)>,
    pub(crate) turn_scope: Option<TurnScope<S>>,
    pub(crate) on_rest: Option<fn(&mut S, &mut Tx) -> bool>,
    pub(crate) on_reopen: Option<fn(&mut S, bool, &mut Tx) -> bool>,
    pub(crate) on_open_url: Option<crate::open_url::incoming::OnOpenUrl<S>>,
    pub(crate) shared: Arc<Shared<S>>,
    pub(crate) overlays: Vec<Box<dyn crate::overlay::PendingOverlay>>,
    pub(crate) windows: Vec<crate::AnyWindowSpec>,
    pub(crate) on_window: Option<OnWindow>,
    pub(crate) on_ready: Option<OnWindow>,
    pub(crate) on_frame_encoded: Option<crate::frame::encoded::OnFrameEncoded>,
    pub(crate) on_frame_drawn: Option<crate::frame::encoded::OnFrameDrawn<S>>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) same_tab_router: Option<Box<dyn Fn(&str) -> bool>>,
    pub(crate) macos_window: crate::MacWindowOptions,
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
            accessory: false,
            standard_menus: false,
            keep_running: false,
            on_close: None,
            on_quit: None,
            turn_scope: None,
            on_rest: None,
            on_reopen: None,
            on_open_url: None,
            shared,
            overlays: Vec::new(),
            windows: Vec::new(),
            on_window: None,
            on_ready: None,
            on_frame_encoded: None,
            on_frame_drawn: None,
            same_tab_router: None,
            macos_window: crate::MacWindowOptions::default(),
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

    /// How this app's windows rasterise and place glyphs (see [`crate::FontSmoothing`]):
    /// `On` by default on macOS and in a Mac's browser, `Off` elsewhere (CoreGraphics' pipeline
    /// without font smoothing's dilation, as `-webkit-font-smoothing: antialiased` draws).
    /// `Tx::set_font_smoothing` switches it while running.
    pub fn font_smoothing(mut self, mode: crate::FontSmoothing) -> Self {
        self.runtime.font_smoothing = mode;
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
        self
    }

    /// Native window policy, also retained by a single-window App.
    pub fn macos_window(mut self, options: crate::MacWindowOptions) -> Self {
        self.macos_window = options;
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
    /// ```ignore
    /// app.on_modifiers(|s: &mut App, mods, _| s.alt_held = mods.alt)
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

    /// Adds a transparent overlay with its own state, hidden until `OverlayHandle::show`.
    /// Bare specs preserve primary-display edge placement and never request key focus;
    /// `OverlaySpec::options` selects displays, placement and native input policy.
    /// Uses this app's fonts. Headless runs don't draw overlays.
    pub fn overlay<T: 'static>(&mut self, spec: impl Into<crate::ConfiguredOverlay<T>>, state: T) -> crate::overlay::OverlayHandle<T> {
        let (overlay, handle) = crate::overlay::OverlayHost::new(spec.into(), state, &self.spec);
        self.overlays.push(Box::new(overlay));
        handle
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

    /// Closing the window keeps the app, its state and its event loop running until
    /// `AppHandle::quit` or `Tx::quit`.
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

    /// Runs whenever the window closes (close button, `Tx::close_window`, `AppHandle::close`,
    /// macOS Quit from the menu, Cmd-Q or the Dock), before it goes: flush drafts, stop previews.
    /// Its `Tx::quit` quits.
    pub fn on_close(mut self, f: fn(&mut S, &mut Tx)) -> Self {
        self.on_close = Some(f);
        self
    }

    /// Runs once in every open window when the app starts to quit, before any window closes
    /// for it: `Tx::quit` or `AppHandle::quit` from any window, the last window closing (in the
    /// windows still open), macOS Quit (⌘Q, the Dock, logout, restart and shutdown), and a
    /// Windows logoff or shutdown (`WM_QUERYENDSESSION`, or `WM_ENDSESSION` when no query came
    /// first), as Electron's `before-quit` and `session-end` do between them. Save what must
    /// survive the quit here, synchronously: a Windows session may end the process once the
    /// hooks returned. From then on [`crate::quitting`] holds, so an `on_close` that follows
    /// (macOS, the end of a Windows session) can tell the app ending from its window closing
    /// alone. A Windows shutdown another app refuses clears it, and a later quit runs this again.
    /// What it asks of its `Tx` beyond state is moot once the loop ends. Native windows only.
    pub fn on_quit(mut self, f: fn(&mut S, &mut Tx)) -> Self {
        self.on_quit = Some(f);
        self
    }

    /// Runs `enter` before each turn of the window's event handling that can reach its state
    /// (an event, a wake, a frame, a hook), and `leave` after it, so state the app's windows
    /// share can move into the state of the window whose turn it is and back. The windows of
    /// one event loop take turns, never two at once. Native windows only.
    pub fn turn_scope(mut self, enter: fn(&mut S), leave: fn(&mut S)) -> Self {
        self.turn_scope = Some(TurnScope { enter, leave });
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
    /// default then also brings the front window forward, restored. Never called on other
    /// platforms. Headless scripts deliver it with `{"reopen": true}`.
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
    /// scheme itself (`HKEY_CURRENT_USER\Software\Classes\<scheme>`, usually at install). Never
    /// called on other platforms (Linux passes links to a new process as arguments).
    pub fn on_open_url(mut self, f: fn(&mut S, &str, &mut Tx)) -> Self {
        self.on_open_url = Some(f);
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
    /// Not called by headless tooling. May queue a show through the supplied handle.
    pub fn on_ready(mut self, f: impl FnMut(&crate::WindowHandle) + 'static) -> Self {
        self.on_ready = Some(Box::new(f));
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

    /// Runs on the UI thread after each `Drawn` frame of the app's window, with the runtime as
    /// that frame presented it (`Runtime::elements`, `Runtime::state`), for an app that watches
    /// its own motion. Pair it with [`last_input`](crate::last_input) to tell motion the user
    /// caused from motion they didn't. Like `on_frame_encoded`, it changes nothing about when
    /// or what the window draws, and only desktop windows call it.
    pub fn on_frame_drawn(mut self, f: impl FnMut(&crate::Runtime<S>) + 'static) -> Self {
        self.on_frame_drawn = Some(Box::new(f));
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
    /// Prints the error to stderr and exits 1 when the app can't start, such as without a
    /// display (wasm32 has no process exit, so there it panics); `try_run` returns that error.
    pub fn run(self) {
        crate::tooling::started(self.try_run());
    }

    /// `run`, returning an error instead of exiting when the event loop can't start (no
    /// display over ssh or in CI, no Wayland or X11 server) or fails.
    #[cfg(target_arch = "wasm32")]
    pub fn try_run(mut self) -> Result<(), String> {
        if let Some(argv) = self.argv.take() {
            let _ = Args::parse_from(argv);
        }
        crate::platform::web::run(self)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn try_run(mut self) -> Result<(), String> {
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
            Err(crate::tooling::WITHOUT_TOOLING.into())
        } else {
            crate::window::run(self, args)
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

    /// Whether the window is on screen (shown, not hidden or closed). Minimized and occluded
    /// windows count as shown.
    #[inline]
    pub fn is_visible(&self) -> bool { self.shared.core.is_visible() }

    /// Why the window couldn't be created the last time it was asked for (no display, no GPU
    /// adapter or device), until a later creation succeeds. The app keeps running windowless.
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
