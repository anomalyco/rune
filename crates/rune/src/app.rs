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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::el::Tx;
use crate::tooling::{AppSpec, Args};

type Update<S> = Box<dyn FnOnce(&mut S, &mut Tx) + Send>;
/// `App::on_window`'s hook, run after each native window creation.
pub(crate) type OnWindow = Box<dyn FnMut(&crate::WindowHandle)>;

/// An `AppSpec` with its lifetime: by default it opens one window and quits when it closes,
/// like `rune::run`.
pub struct App<S: 'static> {
    pub(crate) spec: AppSpec<S>,
    pub(crate) chrome: crate::Chrome,
    init: Option<Box<dyn FnOnce() -> S>>,
    argv: Option<Vec<String>>,
    pub(crate) open: bool,
    pub(crate) release_gpu: bool,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) accessory: bool,
    pub(crate) keep_running: bool,
    pub(crate) on_close: Option<fn(&mut S, &mut Tx)>,
    pub(crate) shared: Arc<Shared<S>>,
    pub(crate) overlays: Vec<Box<dyn crate::overlay::PendingOverlay>>,
    pub(crate) on_window: Option<OnWindow>,
    pub(crate) macos_window: crate::MacWindowOptions,
    pub(crate) compositor: Option<Box<dyn crate::compose::Compositor<S>>>,
    pub(crate) window_options: crate::WindowOptions,
    #[cfg(all(feature = "dev", debug_assertions))]
    pub(crate) dev: Option<crate::dev::Config<S>>,
}

impl<S: 'static> App<S> {
    pub fn new(spec: AppSpec<S>) -> Self {
        App {
            #[cfg(all(feature = "dev", debug_assertions))]
            dev: None,
            spec,
            chrome: crate::Chrome::DEFAULT,
            init: None,
            argv: None,
            open: true,
            release_gpu: false,
            accessory: false,
            keep_running: false,
            on_close: None,
            shared: Arc::default(),
            overlays: Vec::new(),
            on_window: None,
            macos_window: crate::MacWindowOptions::default(),
            compositor: None,
            window_options: crate::WindowOptions::default(),
        }
    }

    /// Configure this window's frame. Overlays retain their own native behaviour.
    pub fn chrome(mut self, chrome: crate::Chrome) -> Self {
        self.chrome = chrome;
        self
    }

    /// Native options for this app's window under `run` (size limits, resizability, ...);
    /// `run_windows` takes them per `WindowSpec` instead.
    pub fn window_options(mut self, options: crate::WindowOptions) -> Self {
        self.window_options = options;
        self
    }

    /// Adds a transparent overlay with its own state, hidden until `OverlayHandle::show`.
    /// Bare specs preserve primary-display edge placement and never request key focus;
    /// `OverlaySpec::options` selects displays, placement and native input policy.
    /// Uses this app's fonts. Headless runs don't draw overlays.
    pub fn overlay<T: 'static>(&mut self, spec: impl Into<crate::ConfiguredOverlay<T>>, state: T) -> crate::overlay::OverlayHandle<T> {
        let (overlay, handle) = crate::overlay::Overlay::new(spec.into(), state, &self.spec);
        self.overlays.push(Box::new(overlay));
        handle
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

    /// macOS: run as an accessory (menu-bar) app: no Dock icon, no default menu bar, and
    /// launching doesn't activate the app or take focus. The app installs its own menu if it
    /// wants one, and may change the activation policy later. No effect on other platforms.
    pub fn accessory(mut self) -> Self {
        self.accessory = true;
        self
    }

    /// Runs whenever the window closes (close button, `Tx::close_window`, `AppHandle::close`),
    /// before it goes: flush drafts, stop previews. Its `Tx::quit` quits.
    pub fn on_close(mut self, f: fn(&mut S, &mut Tx)) -> Self {
        self.on_close = Some(f);
        self
    }

    /// Runs on the UI thread each time the native window is created (again after a Wayland
    /// close or a GPU release): set its icon or native styles through
    /// `WindowHandle::with_raw_handle`.
    pub fn on_window(mut self, f: impl FnMut(&crate::WindowHandle) + 'static) -> Self {
        self.on_window = Some(Box::new(f));
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

    /// Opt into the knobs remote panel for this debug runtime.
    #[cfg(all(feature = "dev", debug_assertions))]
    pub fn dev_panel(mut self, config: crate::dev::Config<S>) -> Self {
        self.dev = Some(config);
        self
    }

    /// Runs until the app quits. Agent-tooling arguments (`--snap`, `--tree`, ...) render
    /// headless instead: the window requests are ignored there, and updates apply each frame.
    pub fn run(mut self) {
        let args = match self.argv.take() {
            Some(argv) => Args::parse_from(argv),
            None => Args::parse(),
        };
        if args.headless() {
            crate::tooling::headless(self, args);
        } else {
            crate::window::run(self, args);
        }
    }
}

impl<S: 'static> App<S> {
    pub(crate) fn into_parts(self, scene: Option<&str>) -> (AppSpec<S>, S, Arc<Shared<S>>) {
        let state = match self.init {
            Some(init) => crate::tooling::with_scene(&self.spec, init(), scene),
            None => crate::tooling::initial_state(&self.spec, scene),
        };
        (self.spec, state, self.shared)
    }
}

/// Opens, focuses, closes and feeds a running `App` from any thread. Every call wakes the event
/// loop; nothing polls. Window requests collapse to the latest.
pub struct AppHandle<S: 'static> {
    shared: Arc<Shared<S>>,
}

impl<S> Clone for AppHandle<S> {
    fn clone(&self) -> Self {
        AppHandle {
            shared: self.shared.clone(),
        }
    }
}

impl<S: 'static> AppHandle<S> {
    /// Show the window (creating it if needed) and bring it forward.
    pub fn open(&self) {
        self.request(|q| q.window = Some(WindowRequest::Open(None)));
    }

    /// Show or recreate the window without requesting focus. A minimized window stays minimized;
    /// open() also restores and activates it. Window-manager policy still applies.
    pub fn show(&self) {
        self.request(|q| q.window = Some(WindowRequest::Show));
    }

    /// Hide the window, retaining its host and state. Does not invoke the close callback.
    pub fn hide(&self) {
        self.request(|q| q.window = Some(WindowRequest::Hide));
    }

    /// Focus/activate an already-visible window, restoring it if minimized.
    /// On Wayland this requests attention; use activate(token) for compositor-authorized focus.
    pub fn focus(&self) {
        self.request(|q| q.focus = true);
    }

    /// Set the outer position in physical desktop pixels (negative coordinates are valid).
    /// Retained across recreation. Wayland does not support application window placement.
    pub fn set_position(&self, position: (i32, i32)) {
        self.request(|q| q.position = Some(position));
    }

    /// Show and focus the window with an activation token another process forwarded
    /// (`XDG_ACTIVATION_TOKEN` on Wayland, `DESKTOP_STARTUP_ID` on X11), so the compositor
    /// lets it take focus. Where tokens don't exist this is `open`.
    pub fn activate(&self, token: impl Into<String>) {
        let token = token.into();
        self.request(|q| q.window = Some(WindowRequest::Open(Some(token).filter(|t| !t.is_empty()))));
    }

    /// Close the window as the close button would (`App::on_close` runs; without
    /// `keep_running` the app quits).
    pub fn close(&self) {
        self.request(|q| q.window = Some(WindowRequest::Close));
    }

    /// Quit the app: the window closes without `on_close`, and `run` returns.
    pub fn quit(&self) {
        self.request(|q| q.quit = true);
    }

    /// macOS: show or hide the app's Dock icon (and app menu) while it runs, switching between
    /// the regular and accessory activation policies. Does nothing elsewhere.
    pub fn set_dock_icon(&self, visible: bool) {
        self.request(|q| q.dock = Some(visible));
    }

    /// Whether the window is on screen (shown, not hidden or closed). Minimized and occluded
    /// windows count as shown.
    pub fn is_visible(&self) -> bool {
        self.shared.visible.load(Ordering::Acquire)
    }

    /// Why the window couldn't be created the last time it was asked for (no display, no GPU
    /// adapter or device), until a later creation succeeds. The app keeps running windowless.
    pub fn window_error(&self) -> Option<String> {
        self.shared.window_error.lock().unwrap().clone()
    }

    /// Change the state from this thread: `f` runs on the UI thread, in call order, even while no
    /// window shows, and the next frame commits it with the motion its `Tx` asks for.
    pub fn update(&self, f: impl FnOnce(&mut S, &mut Tx) + Send + 'static) {
        self.request(|q| q.updates.push(Box::new(f)));
    }

    fn request(&self, f: impl FnOnce(&mut Requests<S>)) {
        f(&mut self.shared.requests.lock().unwrap());
        self.shared.wake();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowRequest {
    /// Show and focus, with an activation token when one was forwarded.
    Open(Option<String>),
    Show,
    Hide,
    Close,
}

pub(crate) struct Requests<S> {
    pub window: Option<WindowRequest>,
    pub dock: Option<bool>,
    pub focus: bool,
    pub position: Option<(i32, i32)>,
    pub quit: bool,
    pub updates: Vec<Update<S>>,
}

impl<S> Default for Requests<S> {
    fn default() -> Self {
        Requests {
            window: None,
            dock: None,
            focus: false,
            position: None,
            quit: false,
            updates: Vec::new(),
        }
    }
}

pub(crate) struct Shared<S> {
    requests: Mutex<Requests<S>>,
    wake: Mutex<Option<crate::runtime::Waker>>,
    /// Published by the window host for `AppHandle::is_visible` / `window_error`.
    pub(crate) visible: AtomicBool,
    pub(crate) window_error: Mutex<Option<String>>,
}

impl<S> Default for Shared<S> {
    fn default() -> Self {
        Shared {
            requests: Mutex::default(),
            wake: Mutex::new(None),
            visible: AtomicBool::new(false),
            window_error: Mutex::new(None),
        }
    }
}

impl<S> Shared<S> {
    /// Installs the event loop's wake-up and wakes it once for requests made before it existed.
    pub fn connect(&self, wake: crate::runtime::Waker) {
        *self.wake.lock().unwrap() = Some(wake);
        self.wake();
    }

    pub fn wake(&self) {
        let wake = self.wake.lock().unwrap().clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    pub fn take(&self) -> Requests<S> {
        std::mem::take(&mut self.requests.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::el::{El, Shell, el};

    use crate::runtime::{Cx, Env, Runtime};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn spec() -> AppSpec<u32> {
        AppSpec::new("test", (100.0, 100.0), || 0, |_: &u32, _: &mut Cx<u32>| el())
    }

    #[test]
    fn handles_report_the_window_the_host_publishes() {
        let app = App::new(spec());
        let handle = app.handle();
        assert!(!handle.is_visible() && handle.window_error().is_none());
        app.shared.visible.store(true, Ordering::Release);
        *app.shared.window_error.lock().unwrap() = Some("no GPU adapter".into());
        assert!(handle.is_visible());
        assert_eq!(handle.window_error().as_deref(), Some("no GPU adapter"));
    }

    #[test]
    fn handles_cross_threads_whatever_the_state() {
        fn send_sync<T: Send + Sync + Clone>() {}
        send_sync::<AppHandle<std::rc::Rc<()>>>();
    }

    #[test]
    fn init_closures_replace_the_spec_init_and_scenes_apply_on_top() {
        let mut spec = spec();
        spec.scenes = &[("big", |n| *n += 100)];
        let captured = 7;
        let (_, state, _) = App::new(spec).init(move || captured).into_parts(Some("big"));
        assert_eq!(state, 107);
        let (_, state, _) = App::new(self::spec()).into_parts(None);
        assert_eq!(state, 0);
    }

    #[test]
    fn handle_requests_queue_before_the_loop_and_wake_it_after() {
        let app = App::new(spec()).windowless();
        assert!(!app.open && app.keep_running);
        let handle = app.handle();
        let other = handle.clone();
        std::thread::spawn(move || {
            other.update(|n, _| *n += 1);
            other.activate("token");
        })
        .join()
        .unwrap();
        handle.update(|n, _| *n *= 10);
        handle.close();
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        app.shared.connect(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            1,
            "connecting wakes once for queued requests"
        );
        let requests = app.shared.take();
        assert_eq!(
            requests.window,
            Some(WindowRequest::Close),
            "window requests collapse to the latest"
        );
        assert!(!requests.quit);
        let mut n = 0;
        for f in requests.updates {
            f(&mut n, &mut Tx::default());
        }
        assert_eq!(n, 10, "updates apply in call order");
        handle.activate("");
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
        assert_eq!(
            app.shared.take().window,
            Some(WindowRequest::Open(None)),
            "an empty token is no token"
        );
    }

    #[test]
    fn tray_requests_keep_show_focus_and_geometry_together() {
        let app = App::new(spec()).windowless();
        let handle = app.handle();
        std::thread::spawn(move || {
            handle.set_position((10, 20));
            handle.hide();
            handle.set_position((-300, 40));
            handle.show();
            handle.focus();
        }).join().unwrap();
        let requests = app.shared.take();
        assert_eq!(requests.window, Some(WindowRequest::Show), "focus must not overwrite show");
        assert!(requests.focus);
        assert_eq!(requests.position, Some((-300, 40)));
        let drained = app.shared.take();
        assert!(!drained.focus && drained.window.is_none() && drained.position.is_none());
    }

    #[test]
    fn updates_commit_on_the_next_frame_and_carry_shell_requests() {
        fn view(n: &u32, _: &mut Cx<u32>) -> El {
            crate::el::row().child(el().key("box").size(10.0 + *n as f32, 10.0))
        }
        let mut rt = Runtime::new(
            0u32,
            view,
            (200.0, 200.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.frame(0.0);
        rt.update(|n, tx| {
            *n = 30;
            tx.instant();
        });
        assert!(rt.shell_request.is_none());
        rt.frame(1.0 / 120.0);
        let b = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("box"))
            .unwrap();
        assert_eq!(b.rect[2], 40.0, "the update's instant commit lands at once");
        rt.update(|_, tx| tx.close_window());
        rt.update(|_, tx| tx.quit());
        rt.update(|_, tx| tx.close_window());
        assert_eq!(rt.shell_request.take(), Some(Shell::Quit), "quit outranks close");
    }
}
