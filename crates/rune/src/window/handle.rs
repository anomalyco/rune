//! `WindowHandle`: a window's native reference that outlives any one host, and the requests
//! made through it before the host exists.

use super::*;

pub(super) type Request = Box<dyn FnOnce(usize) -> WakeEvent + Send>;

#[derive(Default)]
pub(super) struct NativeWindow {
    pub(super) window: Mutex<Weak<Window>>,
    pub(super) number: AtomicU32,
    pub(super) ready: AtomicBool,
    #[cfg(target_os = "windows")]
    pub(super) no_activate: AtomicBool,
    #[cfg(target_os = "windows")]
    pub(super) tool_window: AtomicBool,
    /// What the host's meter last published.
    pub(super) stats: Arc<Mutex<SurfaceStats>>,
    pub(super) route: Mutex<Route>,
}

/// Where a window's requests go.
pub(super) enum Route {
    /// No host yet, or a headless run, which takes them each turn (`take_headless`).
    Unbound { queued: Vec<Request> },
    /// The host's event loop and slot.
    Bound { proxy: EventLoopProxy<WakeEvent>, slot: usize },
}

impl Default for Route {
    fn default() -> Self { Route::Unbound { queued: Vec::new() } }
}

/// A window's native reference, refreshed across App close/reopen without owning the window.
#[derive(Clone)]
pub struct WindowHandle {
    pub(super) native: Arc<NativeWindow>,
    pub(super) key: Arc<str>,
}
impl WindowHandle {
    pub(crate) fn new(key: &str) -> Self { Self { native: Arc::default(), key: key.into() } }

    /// Routes requests to the window's host, starting with those queued before it existed.
    pub(super) fn bind(&self, proxy: EventLoopProxy<WakeEvent>, slot: usize) {
        let mut route = self.native.route.lock().unwrap();
        if let Route::Unbound { queued } = &mut *route {
            for request in std::mem::take(queued) { proxy.send_event(request(slot)).ok(); }
        }
        *route = Route::Bound { proxy, slot };
    }

    /// A headless run never binds a host; it takes the queued requests instead.
    #[cfg_attr(not(any(feature = "tooling", debug_assertions)), allow(dead_code))]
    pub(crate) fn take_headless(&self) -> Vec<WakeEvent> {
        let queued = match &mut *self.native.route.lock().unwrap() {
            Route::Unbound { queued } => std::mem::take(queued),
            Route::Bound { .. } => Vec::new(),
        };
        queued.into_iter().map(|request| request(0)).collect()
    }

    #[cfg_attr(not(any(feature = "tooling", debug_assertions)), allow(dead_code))]
    pub(crate) fn headless_pending(&self) -> bool {
        matches!(&*self.native.route.lock().unwrap(), Route::Unbound { queued } if !queued.is_empty())
    }

    pub(super) fn send(&self, request: impl FnOnce(usize) -> WakeEvent + Send + 'static) {
        match &mut *self.native.route.lock().unwrap() {
            Route::Bound { proxy, slot } => { proxy.send_event(request(*slot)).ok(); }
            Route::Unbound { queued } => queued.push(Box::new(request)),
        }
    }

    /// The first frame has completed for the current native lifetime. For a launcher
    /// this confirms GPU completion of an alpha-zero frame before reveal. False after destruction.
    pub fn is_ready(&self) -> bool { self.window().is_ok() && self.native.ready.load(Ordering::Acquire) }
    pub fn key(&self) -> &str { &self.key }
    /// The window's frame counts: redraws, presents, failed drawable acquisitions, loop turns
    /// and pacing: current totals, and per-second rates updated once a second while it draws.
    pub fn surface_stats(&self) -> SurfaceStats { crate::frame::surface_stats::read(&self.native.stats) }
    pub(super) fn window(&self) -> Result<Arc<Window>, String> {
        self.native.window.lock().unwrap().upgrade().ok_or_else(|| "window is closed".into())
    }
    /// macOS CGWindowID for ScreenCaptureKit exclusion; None before registration or after destruction.
    pub fn window_number(&self) -> Option<u32> {
        let _window = self.window().ok()?;
        let number = self.native.number.load(Ordering::Acquire);
        (number != 0).then_some(number)
    }
    /// The window's scale factor, as `placement` reports it, without reading where the window
    /// is and how it is shown (on macOS the position is asked of the window server: about
    /// 1,700 µs in a first frame). None while it's closed. Call it on the UI thread.
    pub fn scale(&self) -> Option<f64> {
        Some(self.window().ok()?.scale_factor())
    }
    /// Where the window is on the desktop and whether it's maximized, minimized or full screen,
    /// to save and reopen it there. None while it's closed, and on Wayland, which hides window
    /// positions. Call it on the UI thread.
    pub fn placement(&self) -> Option<crate::windows::WindowPlacement> {
        let window = self.window().ok()?;
        let (outer, inner) = (window.outer_position().ok()?, window.inner_position().ok()?);
        let size = window.inner_size();
        #[cfg(target_os = "macos")]
        let panel = crate::platform::macos::is_panel(&window);
        #[cfg(not(target_os = "macos"))]
        let panel = false;
        let (maximized, minimized, full_screen) = if panel {
            (false, false, false)
        } else {
            (window.is_maximized(), window.is_minimized().unwrap_or(false), window.fullscreen().is_some())
        };
        #[cfg(target_os = "windows")]
        let restore = ((maximized || minimized) && !full_screen).then(|| crate::platform::win32::restore_frame(&window)).flatten();
        #[cfg(not(target_os = "windows"))]
        let restore = None;
        Some(crate::windows::WindowPlacement {
            position: (outer.x, outer.y),
            content_position: (inner.x, inner.y),
            content_size: (size.width, size.height),
            scale: window.scale_factor(),
            maximized,
            minimized,
            full_screen,
            restore,
            display: window.current_monitor().map(|m| ((m.position().x, m.position().y), (m.size().width, m.size().height))),
        })
    }
    /// Shows `state` on the window's taskbar button: a progress bar and a count badge (see
    /// [`crate::Taskbar`]). macOS shows the count on the Dock icon, summed over the app's windows
    /// (the Dock has no progress bar of its own). Call it on the UI thread.
    pub fn set_taskbar(&self, state: crate::Taskbar) {
        #[cfg(target_os = "windows")]
        if let Ok(window) = self.window() { crate::platform::win32::taskbar::set(&window, state); }
        #[cfg(target_os = "macos")]
        if let Ok(window) = self.window() { crate::platform::macos::dock_badge(&window, state.count()); }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let _ = state;
    }
    /// Flashes the window's taskbar button twice and leaves it lit until the window comes to the
    /// front, for something that needs the person; `false` stops it once that's handled. A
    /// window in front, or Focus Assist holding notifications back, flashes nothing. Windows
    /// only for now. Call it on the UI thread.
    pub fn flash(&self, on: bool) {
        #[cfg(target_os = "windows")]
        if let Ok(window) = self.window() { crate::platform::win32::taskbar::flash(&window, on); }
        #[cfg(not(target_os = "windows"))]
        let _ = on;
    }
    pub fn set_click_through(&self, click_through: bool) -> Result<(), String> {
        let window = self.window()?;
        window.set_cursor_hittest(!click_through).map_err(|e| e.to_string())?;
        #[cfg(target_os = "windows")]
        if let Some(hwnd) = crate::platform::win32::hwnd(&window) {
            crate::platform::win32::apply_hwnd_styles(
                hwnd,
                self.native.no_activate.load(Ordering::Acquire),
                self.native.tool_window.load(Ordering::Acquire),
            );
        }
        Ok(())
    }
    pub fn set_always_on_top(&self, on: bool) {
        self.send(move |slot| WakeEvent::Level(slot, on));
    }
    /// Sets whether this window is excluded from screen captures, retained across native
    /// recreation and overriding any creation-time `exclude_from_capture` option.
    ///
    /// On Windows, calls `SetWindowDisplayAffinity` with `WDA_EXCLUDEFROMCAPTURE` (`true`) or
    /// `WDA_NONE` (`false`). On macOS, sets `NSWindow.sharingType` to `NSWindowSharingNone`
    /// (`true`) or `NSWindowSharingReadOnly` (`false`), or updates a nonactivating panel's
    /// presence state (applied while shown). Ignored on Linux and web.
    ///
    /// Safe to call from any thread; requests made before the window starts apply once it does.
    /// Capture exclusion is native best effort, not a security boundary; on macOS, capture
    /// clients should also filter [`Self::window_number`].
    ///
    /// ```no_run
    /// use rune::{AppSpec, WindowSpec, el};
    ///
    /// let spec = WindowSpec::new("notes", AppSpec::new("Notes", (480.0, 360.0), || (), |_, _| el()))
    ///     .on_created(|handle| handle.set_exclude_from_capture(true));
    /// let _ = spec;
    /// ```
    pub fn set_exclude_from_capture(&self, exclude: bool) {
        self.send(move |slot| WakeEvent::ExcludeFromCapture(slot, exclude));
    }
    /// Overrides this window's native light or dark chrome at runtime, retained across native
    /// recreation.
    ///
    /// On Windows this updates the native titlebar (`DWMWA_USE_IMMERSIVE_DARK_MODE`), on macOS
    /// `NSWindow.appearance` (and `NSPanel.appearance` for non-activating panels), on Wayland
    /// client-side decorations, and on X11 `_GTK_THEME_VARIANT`. Passing
    /// [`ColorScheme::System`](crate::ColorScheme::System) clears the per-window override so the
    /// window inherits any app-wide override from
    /// [`AppHandle::set_color_scheme`](crate::AppHandle::set_color_scheme) or the operating
    /// system's appearance.
    ///
    /// Safe to call from any thread; requests made before the window starts apply once it does.
    /// Ignored on web and headless.
    ///
    /// ```no_run
    /// use rune::{AppSpec, ColorScheme, WindowSpec, el};
    ///
    /// let spec = WindowSpec::new("notes", AppSpec::new("Notes", (480.0, 360.0), || (), |_, _| el()))
    ///     .on_created(|handle| handle.set_color_scheme(ColorScheme::Dark));
    /// let _ = spec;
    /// ```
    pub fn set_color_scheme(&self, scheme: crate::ColorScheme) {
        self.send(move |slot| WakeEvent::ColorScheme(slot, scheme));
    }
    /// Physical desktop pixels; retained across recreation. Unsupported on Wayland.
    pub fn set_position(&self, position: (i32, i32)) {
        self.send(move |slot| WakeEvent::Position(slot, position));
    }
    pub fn place(&self, display: crate::DisplayTarget, placement: crate::OverlayPlacement) {
        let place = crate::windows::PlaceRequest::new(display, placement);
        self.send(move |slot| WakeEvent::Place(slot, place));
    }
    /// [`crate::Tx::resize_window`] from outside a handler, on `tok::motion::DEFAULT`.
    pub fn resize(&self, size: (f32, f32)) {
        self.frame(crate::window::frame::FrameRequest { size: Some(size), animate: true, ..Default::default() });
    }
    /// [`crate::Tx::resize_window_instant`] from outside a handler.
    pub fn resize_instant(&self, size: (f32, f32)) {
        self.frame(crate::window::frame::FrameRequest { size: Some(size), ..Default::default() });
    }
    /// [`crate::Tx::center_window`] from outside a handler.
    pub fn center(&self) {
        self.frame(crate::window::frame::FrameRequest { center: true, ..Default::default() });
    }
    pub(super) fn frame(&self, request: crate::window::frame::FrameRequest) {
        self.send(move |slot| WakeEvent::Frame(slot, request));
    }
    /// Show/recreate, or hide while keeping state. A launcher takes keys and reveals cached
    /// pixels; an ordinary window shows without requesting focus.
    pub fn set_visible(&self, visible: bool) {
        let request = if visible { WindowRequest::Show } else { WindowRequest::Hide };
        self.send(move |slot| WakeEvent::Window(slot, request));
    }
    pub fn show(&self) { self.set_visible(true); }
    pub fn hide(&self) { self.set_visible(false); }
    /// Focus/activate an already-visible window, restoring it if minimized. Wayland requests attention.
    pub fn focus(&self) { self.send(WakeEvent::Focus); }
    /// Show, restore and activate, creating the window if needed (see `AppHandle::open`).
    /// A default close destroys its host; use App::keep_running to reopen.
    /// Windows added with `App::window` always reopen. Requests made before the loop runs
    /// apply once the window's host starts.
    pub fn open(&self) { self.send(|slot| WakeEvent::Window(slot, WindowRequest::Open(None))); }
    pub fn close(&self) { self.send(|slot| WakeEvent::Window(slot, WindowRequest::Close)); }
    /// Borrow a raw native handle while alive. Platform thread restrictions still apply;
    /// do not retain an extracted pointer beyond the callback.
    pub fn with_raw_handle<R>(&self, f: impl for<'a> FnOnce(winit::raw_window_handle::WindowHandle<'a>) -> R) -> Result<R, String> {
        let window = self.window()?;
        let handle = window.window_handle().map_err(|e| e.to_string())?;
        Ok(f(handle))
    }
    /// Borrow the display's raw native handle while the window is alive.
    ///
    /// Platform thread restrictions still apply (call on the UI thread). The borrowed
    /// [`DisplayHandle`](crate::raw_window_handle::DisplayHandle) names winit's native display
    /// connection (`wl_display` on Wayland, the Xlib display or XCB connection on X11, etc.), so
    /// protocol objects or event queues attached to the window's surface belong on this display.
    ///
    /// The display connection stays alive only while the event loop runs, not after
    /// [`App::run`](crate::App::run) or [`App::try_run`](crate::App::try_run) returns; closing an
    /// individual window does not close the shared display while other windows or `keep_running`
    /// keep the loop alive. Register [`App::on_display_close`](crate::App::on_display_close) to
    /// destroy foreign proxies or queues bound on this display before the event loop drops windows
    /// and closes the native display.
    ///
    /// Returns `Err("window is closed")` before the native window exists or after it closes, and
    /// on headless or web runs.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// App::new(AppSpec::new("Display", (320.0, 200.0), || (), |_, _| el()))
    ///     .on_window(|window| {
    ///         let _ = window.with_raw_display_handle(|display| format!("{:?}", display.as_raw()));
    ///     })
    ///     .run();
    /// ```
    pub fn with_raw_display_handle<R>(
        &self,
        f: impl for<'a> FnOnce(winit::raw_window_handle::DisplayHandle<'a>) -> R,
    ) -> Result<R, String> {
        use winit::raw_window_handle::HasDisplayHandle;
        let window = self.window()?;
        let handle = window.display_handle().map_err(|e| e.to_string())?;
        Ok(f(handle))
    }
}

#[cfg(target_os = "macos")]
pub(super) fn native_window_number(window: &Window) -> Option<u32> {
    u32::try_from(crate::platform::macos::ns_window(window)?.windowNumber()).ok().filter(|n| *n != 0)
}
#[cfg(not(target_os = "macos"))]
pub(super) fn native_window_number(_: &Window) -> Option<u32> { None }

#[cfg(test)]
#[path = "handle_tests.rs"]
mod tests;
