//! One event loop, independent window lifetime hosts. App/AppHandle and multi-window
//! factories share the same input, rendering, activation and close/reopen paths.

use std::sync::{Arc, Mutex, Weak, atomic::{AtomicBool, AtomicU32, Ordering}};
use std::{cell::RefCell, rc::Rc};
use std::time::Duration;
use crate::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
use winit::event::{ElementState, Ime as WIme, MouseButton, StartCause, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Theme, Window, WindowId, WindowLevel};
use winit::raw_window_handle::HasWindowHandle;

use crate::app::{Shared, WindowRequest};
use crate::compose::{Compositor, Frame};
use crate::frame_clock::FrameClock;
use crate::edit::{Ime, Key};
use crate::el::{Shell, Tx};
use crate::render::{Gpu, Renderer};
use crate::runtime::{Env, Runtime, TitlebarAction, Waker};
use crate::surface_stats::{Pacing, SurfaceMeter, SurfaceStats};
use crate::tooling::{AppSpec, Args, Chrome, runtime_for};
use crate::windows::{AnyWindowSpec, Display, WindowOptions, WindowSpec};

#[cfg(target_os = "macos")]
mod launch_drawables;
#[cfg(target_os = "macos")]
use launch_drawables::LaunchDrawables;
mod win;
use win::{FirstFrame, Win};
mod draw;
mod presented;
use presented::{Presented, composing};
mod platform_sync;
use platform_sync::sync_platform;
mod shell_requests;
mod keys;
pub(crate) use keys::{key_base, map_key, map_mods};
mod event_loop;
#[cfg(not(target_arch = "wasm32"))]
pub use event_loop::run;
pub(crate) use event_loop::run_windows;
#[cfg(test)]
mod tests;

pub(crate) enum WakeEvent {
    DisplayTick(usize),
    App(usize), Live(usize), Window(usize, WindowRequest), Focus(usize),
    Position(usize, (i32, i32)), Level(usize, bool),
    Place(usize, crate::windows::PlaceRequest),
    Frame(usize, crate::window_frame::FrameRequest),
    /// The app was reopened (macOS: its Dock icon clicked); for `Windows`, not one host.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Reopen,
}

/// What `Windows` needs of a host, whatever its state type: an app window or an overlay.
pub(crate) trait AnyHost {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {}
    fn wake_deadline(&self) -> Option<Instant> { None }
    fn resumed(&mut self, event_loop: &ActiveEventLoop);
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent);
    fn owns(&self, id: WindowId) -> bool;
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent);
    /// Draws what has no redraw event (a layer surface); true while it needs more frames.
    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) -> bool { false }
    fn finished(&self) -> bool { false }
    /// The OS is ending the app without Rune asking: run close hooks before the hosts drop.
    fn terminate(&mut self) {}
    /// Ends a tracking pop-up menu and answers waiting ones `None`, before the host drops or
    /// the loop exits.
    fn cancel_menus(&mut self) {}
    /// False for hosts that must not keep the loop running alone (overlays).
    fn keeps_loop(&self) -> bool { true }
    /// How reopening the app sees it; overlays never come back.
    fn reopen_candidate(&self) -> crate::reopen::Candidate { Default::default() }
    /// Runs `App::on_reopen` as an update; true when it handled the reopen.
    fn reopen_hook(&mut self, _event_loop: &ActiveEventLoop, _has_visible_windows: bool) -> bool { false }
    /// Applies what was asked of it through its handle since the last wake, now rather than
    /// when that wake arrives.
    fn apply_pending(&mut self, _event_loop: &ActiveEventLoop) {}
    /// Shows, restores and activates it as the default reopen.
    fn reopen(&mut self, _event_loop: &ActiveEventLoop) {}
}

type Request = Box<dyn FnOnce(usize) -> WakeEvent + Send>;

#[derive(Default)]
struct NativeWindow {
    window: Mutex<Weak<Window>>,
    number: AtomicU32,
    ready: AtomicBool,
    /// What the host's meter last published.
    stats: Arc<Mutex<SurfaceStats>>,
    /// The event loop and slot once the host exists, and the requests made before it did.
    route: Mutex<(Option<(EventLoopProxy<WakeEvent>, usize)>, Vec<Request>)>,
}

/// A window's native reference, refreshed across App close/reopen without owning the window.
#[derive(Clone)]
pub struct WindowHandle {
    native: Arc<NativeWindow>,
    key: Arc<str>,
}
impl WindowHandle {
    pub(crate) fn new(key: &str) -> Self { Self { native: Arc::default(), key: key.into() } }

    /// Routes requests to the window's host, starting with those queued before it existed.
    fn bind(&self, proxy: EventLoopProxy<WakeEvent>, slot: usize) {
        let mut route = self.native.route.lock().unwrap();
        for request in std::mem::take(&mut route.1) { let _ = proxy.send_event(request(slot)); }
        route.0 = Some((proxy, slot));
    }

    /// A headless run never binds a host; it takes the queued requests instead.
    pub(crate) fn take_headless(&self) -> Vec<WakeEvent> {
        let queued = std::mem::take(&mut self.native.route.lock().unwrap().1);
        queued.into_iter().map(|request| request(0)).collect()
    }

    pub(crate) fn headless_pending(&self) -> bool {
        !self.native.route.lock().unwrap().1.is_empty()
    }

    fn send(&self, request: impl FnOnce(usize) -> WakeEvent + Send + 'static) {
        let mut route = self.native.route.lock().unwrap();
        match &route.0 {
            Some((proxy, slot)) => { let _ = proxy.send_event(request(*slot)); }
            None => route.1.push(Box::new(request)),
        }
    }

    /// The first frame has completed for the current native lifetime. For a launcher
    /// this confirms GPU completion of an alpha-zero frame before reveal. False after destruction.
    pub fn is_ready(&self) -> bool { self.window().is_ok() && self.native.ready.load(Ordering::Acquire) }
    pub fn key(&self) -> &str { &self.key }
    /// The window's frame counts: redraws, presents, failed drawable acquisitions, loop turns
    /// and pacing: current totals, and per-second rates updated once a second while it draws.
    pub fn surface_stats(&self) -> SurfaceStats { crate::surface_stats::read(&self.native.stats) }
    fn window(&self) -> Result<Arc<Window>, String> {
        self.native.window.lock().unwrap().upgrade().ok_or_else(|| "window is closed".into())
    }
    /// macOS CGWindowID for ScreenCaptureKit exclusion; None before registration or after destruction.
    pub fn window_number(&self) -> Option<u32> {
        let _window = self.window().ok()?;
        let number = self.native.number.load(Ordering::Acquire);
        (number != 0).then_some(number)
    }
    pub fn set_click_through(&self, click_through: bool) -> Result<(), String> {
        self.window()?.set_cursor_hittest(!click_through).map_err(|e| e.to_string())
    }
    pub fn set_always_on_top(&self, on: bool) {
        self.send(move |slot| WakeEvent::Level(slot, on));
    }
    /// Physical desktop pixels; retained across recreation. Unsupported on Wayland.
    pub fn set_position(&self, position: (i32, i32)) {
        self.send(move |slot| WakeEvent::Position(slot, position));
    }
    pub fn place(&self, display: crate::DisplayTarget, placement: crate::OverlayPlacement) {
        let place = crate::windows::PlaceRequest::new(display, placement);
        self.send(move |slot| WakeEvent::Place(slot, place));
    }
    /// [`Tx::resize_window`] from outside a handler, on `tok::motion::DEFAULT`.
    pub fn resize(&self, size: (f32, f32)) {
        self.frame(crate::window_frame::FrameRequest { size: Some(size), animate: true, ..Default::default() });
    }
    /// [`Tx::resize_window_instant`] from outside a handler.
    pub fn resize_instant(&self, size: (f32, f32)) {
        self.frame(crate::window_frame::FrameRequest { size: Some(size), ..Default::default() });
    }
    /// [`Tx::center_window`] from outside a handler.
    pub fn center(&self) {
        self.frame(crate::window_frame::FrameRequest { center: true, ..Default::default() });
    }
    fn frame(&self, request: crate::window_frame::FrameRequest) {
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
}

#[cfg(target_os = "macos")]
fn native_window_number(window: &Window) -> Option<u32> {
    // SAFETY: called on winit's event thread.
    u32::try_from(unsafe { crate::macos::ns_window(window)?.windowNumber() }).ok().filter(|n| *n != 0)
}
#[cfg(not(target_os = "macos"))]
fn native_window_number(_: &Window) -> Option<u32> { None }

/// Builds every host, attaching its shortcut and menu guards, before any resumes: a window's
/// first updates drain on resume and may register on a later window.
fn start_all<T, H>(items: Vec<T>, mut build: impl FnMut(usize, T) -> Option<H>, resume: impl FnMut(&mut H)) -> Vec<Option<H>> {
    let mut hosts: Vec<_> = items.into_iter().enumerate().map(|(slot, item)| build(slot, item)).collect();
    hosts.iter_mut().flatten().for_each(resume);
    hosts
}

type Factory = Box<dyn FnOnce(&[Display]) -> Vec<AnyWindowSpec>>;

struct Windows {
    factory: Option<Factory>,
    args: Args,
    proxy: EventLoopProxy<WakeEvent>,
    hosts: Vec<Option<Box<dyn AnyHost>>>,
    // Weak pool preserves release_gpu_when_closed when the last GPU owner closes.
    gpu: Rc<RefCell<Weak<Gpu>>>,
}
impl Windows {
    fn cleanup(&mut self, event_loop: &ActiveEventLoop) {
        for host in &mut self.hosts {
            if let Some(h) = host.as_mut().filter(|h| h.finished()) {
                h.cancel_menus();
                *host = None;
            }
        }
        // Another window's quit: a menu still tracking would hold the loop open.
        if event_loop.exiting() {
            for host in self.hosts.iter_mut().flatten() { host.cancel_menus(); }
        }
        if !self.hosts.iter().flatten().any(|h| h.keeps_loop()) {
            // The app ends with its last main window: windows still open close as their close
            // button would. A quit already asked to exit and skips their hooks.
            if !event_loop.exiting() {
                for host in self.hosts.iter_mut().flatten() { host.terminate(); }
            }
            event_loop.exit();
        }
    }

    /// The app was reopened: every `on_reopen` hook runs, then, unless one handled it or the
    /// app is an accessory, the default brings back a window when none shows (`reopen::target`).
    fn reopen(&mut self, event_loop: &ActiveEventLoop) {
        let candidates = |hosts: &[Option<Box<dyn AnyHost>>]| -> Vec<_> {
            hosts.iter().map(|h| h.as_ref().map(|h| h.reopen_candidate()).unwrap_or_default()).collect()
        };
        let has_visible = crate::reopen::has_visible(&candidates(&self.hosts));
        let mut handled = false;
        for host in self.hosts.iter_mut().flatten() {
            handled |= host.reopen_hook(event_loop, has_visible);
        }
        if handled || event_loop.exiting() { return; }
        #[cfg(target_os = "macos")]
        if crate::reopen::accessory() { return; }
        // The default sees what the hooks did, to other windows too.
        for host in self.hosts.iter_mut().flatten() { host.apply_pending(event_loop); }
        if event_loop.exiting() { return; }
        #[cfg(target_os = "macos")]
        crate::reopen::unhide();
        if let Some(Some(host)) = crate::reopen::target(&candidates(&self.hosts)).and_then(|i| self.hosts.get_mut(i)) {
            host.reopen(event_loop);
        }
    }
}

/// What a window needs from the shell to start, whatever its state type.
pub(crate) struct Start<'a> {
    proxy: EventLoopProxy<WakeEvent>,
    slot: usize,
    gpu: Rc<RefCell<Weak<Gpu>>>,
    wayland: bool,
    /// Whether this window records `--native` traces.
    traced: bool,
    overlays: &'a mut Vec<Box<dyn crate::overlay::PendingOverlay>>,
}

/// Starts one window's host, or renders it headless when its own arguments ask for that.
pub(crate) fn start<S: 'static>(mut spec: WindowSpec<S>, handle: Option<WindowHandle>, keeps_loop: bool, mut inherited: Args, cx: Start<'_>) -> Option<Box<dyn AnyHost>> {
    if inherited.native && !cx.traced {
        inherited.trace = None;
        inherited.frames = None;
    }
    let args = spec.app.resolve_args(inherited);
    if args.headless() {
        if let Some((w, h)) = spec.options.physical_size {
            let scale = args.scale.unwrap_or(2.0);
            spec.app.spec.size = (w as f32 / scale, h as f32 / scale);
        }
        let transparent = spec.options.transparent;
        spec.app.window_options = spec.options;
        crate::tooling::headless_configured(spec.app, args, transparent);
        return None;
    }
    crate::video::enable_playback();
    cx.overlays.append(&mut spec.app.overlays);
    let handle = handle.unwrap_or_else(|| WindowHandle::new(&spec.key));
    let open_urls = crate::open_url_in::listen(&spec.app);
    let mut host = Host::new(spec, handle, args, cx.proxy, cx.slot, cx.gpu, cx.wayland);
    (host.keeps_loop, host._open_urls) = (keeps_loop, open_urls);
    Some(Box::new(host))
}

impl ApplicationHandler<WakeEvent> for Windows {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        for host in self.hosts.iter_mut().flatten() { host.new_events(event_loop, cause); }
        // Before resumed there are no hosts to clean up.
        if self.factory.is_none() { self.cleanup(event_loop); }
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(factory) = self.factory.take() else { return };
        let primary = event_loop.primary_monitor();
        let displays: Vec<_> = event_loop.available_monitors().map(|m| Display {
            id: Some(crate::DisplayId::from_monitor(m.clone(), is_wayland(event_loop))),
            name: m.name(), position: (m.position().x, m.position().y),
            size: (m.size().width, m.size().height), scale: m.scale_factor(), primary: primary.as_ref() == Some(&m),
        }).collect();
        let specs = crate::windows::flatten(factory(&displays));
        crate::windows::validate(&specs).expect("window keys");
        let trace_window = self.args.window.clone().or_else(|| specs.first().map(|s| s.key().to_owned()));
        let mut overlays = Vec::new();
        let wayland = is_wayland(event_loop);
        self.hosts = start_all(specs, |slot, spec| {
            let traced = trace_window.as_deref() == Some(spec.key());
            spec.start(self.args.clone(), Start { proxy: self.proxy.clone(), slot, gpu: self.gpu.clone(), wayland, traced, overlays: &mut overlays })
        }, |host| host.resumed(event_loop));
        for mut overlay in overlays {
            let (slot, proxy) = (self.hosts.len(), self.proxy.clone());
            let frame_proxy = proxy.clone();
            overlay.connect(Arc::new(move || { let _ = proxy.send_event(WakeEvent::App(slot)); }),
                Arc::new(move || { let _ = frame_proxy.send_event(WakeEvent::DisplayTick(slot)); }), self.gpu.clone());
            overlay.resumed(event_loop);
            self.hosts.push(Some(overlay));
        }
        #[cfg(target_os = "macos")]
        {
            let proxy = self.proxy.clone();
            crate::reopen::install(move || proxy.send_event(WakeEvent::Reopen).is_ok());
        }
        crate::open_url_in::settle();
        // A window opened at launch took it; a GPU kept for a later one would sit idle until then.
        crate::render::discard_prewarm();
        self.cleanup(event_loop);
    }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        let slot = match &event {
            WakeEvent::DisplayTick(i) | WakeEvent::App(i) | WakeEvent::Live(i) | WakeEvent::Window(i, _) | WakeEvent::Focus(i)
                | WakeEvent::Position(i, _) | WakeEvent::Level(i, _) | WakeEvent::Place(i, _) | WakeEvent::Frame(i, _) => *i,
            WakeEvent::Reopen => {
                self.reopen(event_loop);
                self.cleanup(event_loop);
                return;
            }
        };
        if let Some(Some(host)) = self.hosts.get_mut(slot) {
            host.user_event(event_loop, event);
        }
        self.cleanup(event_loop);
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(host) = self.hosts.iter_mut().flatten().find(|h| h.owns(id)) {
            let _input = crate::input_time::arrived(&event);
            host.window_event(event_loop, id, event);
        }
        self.cleanup(event_loop);
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        crate::os_cursor::make_pending(event_loop);
        let mut animating = false;
        for host in self.hosts.iter_mut().flatten() {
            animating |= host.about_to_wait(event_loop);
        }
        // Layer surfaces have no redraw events: keep turning while one animates (its vsync
        // paces the loop). Hosts whose only motion is an ambient loop sleep until its next step.
        let deadline = self.hosts.iter().flatten().filter_map(|host| host.wake_deadline()).min();
        event_loop.set_control_flow(if animating { ControlFlow::Poll }
            else { deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil) });
    }
    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        // Rune didn't ask to exit, so the OS is ending the app: macOS Quit, Cmd-Q, the Dock's
        // Quit, logout. Windows close as their close button would, before the clipboard stops.
        if !event_loop.exiting() {
            for host in self.hosts.iter_mut().flatten() { host.terminate(); }
        }
        for host in self.hosts.iter_mut().flatten() { host.cancel_menus(); }
        // Surfaces go while winit's display connection is still open (layer surfaces live on it).
        self.hosts.clear();
        crate::clipboard::shutdown();
    }
}

/// A file drag from another app over the window. winit reports one event per file; the host
/// gathers them and hands the runtime one drag and one drop (`Host::flush_files`).
#[derive(Default)]
struct FileDrop {
    /// What winit reported since the last pass, in order.
    steps: Vec<FileStep>,
    /// Samples the pointer while the drag is over the window, and where it last was.
    pointer: Option<crate::drop_pointer::DropPointer>,
    at: Option<(f32, f32)>,
}

enum FileStep {
    Enter(Vec<std::path::PathBuf>),
    Drop(Vec<std::path::PathBuf>),
    Leave,
}

impl FileDrop {
    /// One file of a drag that entered (`drop` false) or dropped: the run of files it belongs
    /// to grows, or a new run starts.
    fn file(&mut self, path: std::path::PathBuf, drop: bool) {
        match self.steps.last_mut() {
            Some(FileStep::Enter(files)) if !drop => files.push(path),
            Some(FileStep::Drop(files)) if drop => files.push(path),
            _ => self.steps.push(if drop { FileStep::Drop(vec![path]) } else { FileStep::Enter(vec![path]) }),
        }
    }
}

/// How often a file drag's position is sampled while it's over the window.
const DROP_SAMPLE: Duration = Duration::from_millis(16);

struct Host<S: 'static> {
    chrome: Chrome,
    options: WindowOptions,
    placement: Option<crate::windows::PlaceRequest>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    macos_window: crate::MacWindowOptions,
    native_handle: WindowHandle,
    on_created: Option<Box<dyn FnOnce(WindowHandle)>>,
    on_window: Option<crate::app::OnWindow>,
    on_ready: Option<crate::app::OnWindow>,
    frame_reports: crate::frame_encoded::FrameReports<S>,
    finished: bool,
    /// False for windows `App::window` added: they live as long as the loop, never keep it.
    keeps_loop: bool,
    spec: AppSpec<S>,
    args: Args,
    keep_running: bool,
    on_close: Option<fn(&mut S, &mut Tx)>,
    on_reopen: Option<fn(&mut S, bool, &mut Tx) -> bool>,
    /// Its place among the windows receiving links (`App::on_open_url`).
    _open_urls: Option<crate::open_url_in::Listening>,
    /// When it last hid, in `reopen::stamp` order.
    hidden_at: Option<u64>,
    release_gpu: bool,
    open_at_start: bool,
    /// `App::float_on_open`: each window maps at a fixed size, then takes its own limits.
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
    float_on_open: bool,
    shared: Arc<Shared<S>>,
    shortcuts: crate::shortcuts::Guard,
    menus: crate::native_menu::Guard<S>,
    popup: crate::popup_menu::Presenter,
    file_dialog: crate::file_dialog::Presenter,
    rt: Runtime<S>,
    /// Before `win` and `gpu`: fields drop in order, and the trace waits for the window's last
    /// present times.
    trace: Option<crate::tooling::Trace>,
    trace_start: Option<Instant>,
    trace_frames: usize,
    meter: SurfaceMeter,
    win: Option<Win>,
    gpu: Option<(Arc<Gpu>, Renderer, wgpu::TextureFormat)>,
    gpu_pool: Rc<RefCell<Weak<Gpu>>>,
    /// A window has shown before: the next one opens settled.
    shown: bool,
    started: bool,
    clock: FrameClock,
    idle: bool,
    mods: ModifiersState,
    control_click: bool,
    files: FileDrop,
    clipboard: crate::clipboard::Client,
    secure_input: crate::secure_input::SecureInput,
    ime_allowed: bool,
    comp: Option<Box<dyn Compositor<S>>>,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    wayland: bool,
    /// The window's frame moving to where a handler asked (`Tx::resize_window`), top-down
    /// screen points on the host's frame clock. macOS only; elsewhere frames change at once.
    frame_anim: Option<crate::window_frame::FrameAnim>,
    /// The window origin `frame_anim` last set, to notice a drag mid-flight.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    frame_origin: Option<[f32; 2]>,
    /// The content size a resize request jumps to, so its Resized event lays out like a step
    /// of a resize animation rather than a user's resize.
    frame_jump: Option<(f32, f32)>,
    /// Present times of every window this host opens while tracing.
    #[cfg(target_os = "macos")]
    present_log: Option<Arc<crate::present_time::metal::Log>>,
    #[cfg(target_os = "macos")]
    frame_wake: Waker,
}

impl<S: 'static> Host<S> {
    fn new(window_spec: WindowSpec<S>, native_handle: WindowHandle, args: Args, proxy: EventLoopProxy<WakeEvent>, slot: usize, gpu_pool: Rc<RefCell<Weak<Gpu>>>, wayland: bool) -> Self {
        let WindowSpec { mut app, options, key: _, on_created } = window_spec;
        let mut args = app.resolve_args(args);
        let on_window = app.on_window.take();
        let on_ready = app.on_ready.take();
        let frame_reports = crate::frame_encoded::FrameReports::new(app.on_frame_encoded.take(), Some(native_handle.key())).inspecting(app.on_frame_drawn.take());
        let macos_window = std::mem::take(&mut app.macos_window);
        // Before the menu guard, so custom items in Edit or Window join these menus.
        #[cfg(target_os = "macos")]
        crate::native_menu::install_standard_menus(app.standard_menus);
        native_handle.bind(proxy.clone(), slot);
        let live_proxy = proxy.clone();
        #[cfg(target_os = "macos")]
        let frame_proxy = proxy.clone();
        let waker: Waker = Arc::new(move || {
            let _ = proxy.send_event(WakeEvent::App(slot));
        });
        let (open, keep_running, on_close, release_gpu) = (app.open, app.keep_running, app.on_close, app.release_gpu);
        let on_reopen = app.on_reopen;
        #[cfg(target_os = "macos")]
        let present_log = args.trace.as_ref().map(|_| Arc::default());
        let mut app = app;
        let comp = app.compositor.take();
        let (on_focus, on_ime) = (app.on_focus, app.on_ime);
        let chrome = app.chrome;
        let srgb_blend = app.srgb_blend;
        let modifier_keys = app.modifier_keys;
        let tab_visits_controls = app.tab_visits_controls;
        let fit_height = app.fit_content_height;
        let float_on_open = app.float_on_open;
        let app_stats = app.dev_stats;
        let playground = app.playground.take();
        let (mut spec, state, shared) = app.into_parts(args.scene.as_deref());
        let mut size = args.size.unwrap_or(spec.size);
        // The window reports the system appearance and scale when it opens.
        let env = Env::new(args.theme.as_deref() != Some("light"), 1.0);
        let mut rt = runtime_for(&spec, state, size, env, setup_waker(&shared));
        crate::playground::attach(&mut rt, playground, &spec, args.playground.clone(), args.variant.clone(), true);
        if fit_height {
            spec.min_size = options.fit_min_size(spec.min_size);
            // Opens at the content's height; the first frame corrects any difference its
            // real scale makes.
            rt.fit_height = true;
            let (min, max) = options.size_limits(spec.min_size);
            size = crate::window_frame::clamp_size((size.0, rt.content_height()), min, max);
            rt.set_viewport(size, env.scale);
            spec.size = size;
            args.size = None;
        }
        rt.on_focus = on_focus;
        rt.app_ime.hook = on_ime;
        rt.set_modifier_keys(modifier_keys);
        rt.set_srgb_blend(srgb_blend);
        rt.set_system_reduced_motion(crate::preferences::reduced_motion());
        rt.set_full_keyboard_access(crate::preferences::full_keyboard_access());
        rt.set_tab_visits_controls(tab_visits_controls);
        crate::accent::watch();
        rt.set_system_accent(crate::system_accent());
        rt.trace_channels = args.trace.as_ref().map(|_| Default::default());
        rt.set_live_waker(Arc::new(move || { let _ = live_proxy.send_event(WakeEvent::Live(slot)); }));
        rt.set_live_visible(false);
        rt.set_os_drag(crate::drag_out::SUPPORTED);
        rt.transparent = options.transparent;
        rt.track_marks = comp.is_some();
        rt.wall_start = Some(Instant::now());
        rt.open_urls_natively = args.script.is_none() && args.trace.is_none() && args.frames.is_none();
        rt.record_events = false;
        let meter = SurfaceMeter::new(native_handle.key(), native_handle.native.stats.clone());
        rt.surface_stats = Some(meter.shared());
        rt.app_stats = app_stats;
        let mut clipboard = crate::clipboard::Client::default();
        clipboard.connect(waker.clone());
        shared.connect(waker.clone());
        let popup = crate::popup_menu::Presenter::new(waker.clone());
        let file_dialog = crate::file_dialog::Presenter::new(waker.clone());
        let shortcuts = crate::shortcuts::Guard::attach(&shared, waker, false, wayland);
        let menus = crate::native_menu::Guard::attach(&shared, false);
        Host {
            chrome,
            options,
            placement: None,
            macos_window,
            native_handle,
            on_created,
            on_window,
            on_ready,
            frame_reports,
            finished: false,
            keeps_loop: true,
            spec,
            args,
            keep_running,
            on_close,
            on_reopen,
            _open_urls: None,
            hidden_at: None,
            release_gpu,
            open_at_start: open,
            float_on_open,
            shared,
            shortcuts,
            menus,
            popup,
            file_dialog,
            rt,
            win: None,
            gpu: None,
            gpu_pool,
            shown: false,
            started: false,
            clock: FrameClock::new(Instant::now()),
            idle: true,
            mods: ModifiersState::empty(),
            control_click: false,
            files: FileDrop::default(),
            clipboard,
            secure_input: Default::default(),
            ime_allowed: false,
            comp,
            wayland,
            frame_anim: None,
            frame_origin: None,
            frame_jump: None,
            trace: None,
            trace_start: None,
            trace_frames: 0,
            meter,
            #[cfg(target_os = "macos")]
            present_log,
            #[cfg(target_os = "macos")]
            frame_wake: Arc::new(move || { let _ = frame_proxy.send_event(WakeEvent::DisplayTick(slot)); }),
        }
    }
}

/// The runtime's next timer, on the wall clock that started with the host.
pub(crate) fn wall_wake_deadline<S>(rt: &Runtime<S>) -> Option<Instant> {
    rt.wall_start?.checked_add(Duration::try_from_secs_f64(rt.wake_deadline()?).ok()?)
}

/// The runtime's next timer as a host with no display link to take it waits for it: hidden,
/// covered, faded out or unpaced. A timer due within a frame interval of the last poll
/// (`Tx::wake_after(Duration::ZERO)` asks for the next frame) waits for that interval, so a
/// poll that keeps asking steps at most once per `TIMER_FRAME` and never spins the loop.
pub(crate) fn timer_deadline<S>(rt: &Runtime<S>) -> Option<Instant> {
    let due = wall_wake_deadline(rt)?;
    let polled = rt.wall_start.and_then(|t0| t0.checked_add(Duration::try_from_secs_f64(rt.polled_at()).ok()?));
    Some(polled.map_or(due, |polled| due.max(polled + TIMER_FRAME)))
}

/// Paces the frame a hidden host keeps ready to show (a launcher's cached pixels, a retained
/// overlay's commit). A change after a quiet spell renders at once; later ones wait until
/// `HIDDEN_REFRESH` has passed since the last render, so state that changes many times a
/// second costs a few builds a second while nobody sees them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HiddenPace { last: Option<Instant>, waiting: bool }

pub(crate) const HIDDEN_REFRESH: Duration = Duration::from_millis(250);

impl HiddenPace {
    /// Whether a change at `now` renders now. Otherwise it waits for `deadline`.
    pub(crate) fn changed(&mut self, now: Instant) -> bool {
        self.waiting = !self.ready(now);
        !self.waiting
    }
    /// Whether a change at `now` would render at once.
    pub(crate) fn ready(&self, now: Instant) -> bool {
        self.last.is_none_or(|last| now >= last + HIDDEN_REFRESH)
    }
    /// When the change that waits renders.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.last.filter(|_| self.waiting).map(|last| last + HIDDEN_REFRESH)
    }
    /// Whether the change that waits is due at `now`; it stops waiting either way it renders.
    pub(crate) fn take_due(&mut self, now: Instant) -> bool {
        let due = self.deadline().is_some_and(|at| at <= now);
        if due { self.waiting = false; }
        due
    }
    /// A hidden frame rendered what had changed.
    pub(crate) fn rendered(&mut self, now: Instant) {
        *self = HiddenPace { last: Some(now), waiting: false };
    }
    /// Shown, the host draws its own frames; hidden again, the first change renders at once.
    pub(crate) fn reset(&mut self) { *self = HiddenPace::default(); }
}

#[test]
fn hidden_pace_renders_a_change_at_once_then_at_most_every_refresh() {
    let t0 = Instant::now();
    let mut pace = HiddenPace::default();
    assert!(pace.changed(t0), "the first change renders at once");
    pace.rendered(t0);
    assert_eq!(pace.deadline(), None);
    assert!(!pace.changed(t0 + Duration::from_millis(100)));
    assert!(!pace.changed(t0 + Duration::from_millis(200)));
    assert_eq!(pace.deadline(), Some(t0 + HIDDEN_REFRESH), "later ones wait for one render");
    assert!(!pace.take_due(t0 + Duration::from_millis(249)));
    assert!(pace.take_due(t0 + HIDDEN_REFRESH));
    assert_eq!(pace.deadline(), None, "a due render asked for doesn't wake the loop again");
    assert!(pace.changed(t0 + HIDDEN_REFRESH));
    pace.rendered(t0 + HIDDEN_REFRESH);
    assert_eq!(pace.deadline(), None);
    assert!(pace.changed(t0 + Duration::from_secs(5)), "after a quiet spell, at once again");
    pace.reset();
    assert!(pace.changed(t0 + Duration::from_secs(5)));
}

/// Whether a timer asks for the frame after one just presented. Only a surface whose display
/// link paces it (`next_frame` is its next tick) takes a timer due by then as a frame request;
/// anywhere else the timer waits in `timer_deadline`, so it never turns the loop by itself.
pub(crate) fn timer_wants_frame<S>(rt: &Runtime<S>, next_frame: Option<Instant>) -> bool {
    next_frame.is_some_and(|next| wall_wake_deadline(rt).is_some_and(|due| due <= next))
}

/// Why a surface gave no drawable this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Missed {
    /// Nothing of the window is on screen (covered, or its displays asleep).
    Occluded,
    /// No drawable was free, or acquisition failed some other passing way.
    Timeout,
    /// Outdated or lost, and reconfiguring didn't bring it back.
    Outdated,
}

/// How a host waits after a frame it couldn't draw. There is no `ControlFlow::Poll` here:
/// retrying at once is what spins a core while a window is occluded or its displays sleep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NextFrame {
    WaitUntil(Instant),
    /// Sleep until an event: a display tick, a notification, input or a timer.
    Idle,
}

/// The first re-check of an occluded surface. Its occlusion notification usually redraws it
/// first; this catches one that never arrives. Consecutive misses back off to `OCCLUDED_MAX`.
pub(crate) const OCCLUDED_RETRY: Duration = Duration::from_millis(250);
pub(crate) const OCCLUDED_MAX: Duration = Duration::from_secs(2);
/// An unpaced surface's retry after a drawable timeout: about one frame.
pub(crate) const TIMEOUT_RETRY: Duration = Duration::from_millis(16);
/// Motion's frame interval when no display link paces a surface.
pub(crate) const TIMER_FRAME: Duration = Duration::from_micros(16_667);

impl Missed {
    fn of(status: wgpu::CurrentSurfaceTexture) -> Result<wgpu::SurfaceTexture, Missed> {
        use wgpu::CurrentSurfaceTexture as S;
        match status {
            S::Success(f) | S::Suboptimal(f) => Ok(f),
            S::Occluded => Err(Missed::Occluded),
            S::Outdated | S::Lost => Err(Missed::Outdated),
            S::Timeout | S::Validation => Err(Missed::Timeout),
        }
    }

    /// What follows a frame this surface couldn't draw. A display link that is still running
    /// retries a timeout on its next tick. `occluded_wait` is the caller's backoff, held to
    /// `OCCLUDED_RETRY..=OCCLUDED_MAX`.
    pub(crate) fn next(self, paced: bool, now: Instant, occluded_wait: Duration) -> NextFrame {
        match self {
            Missed::Occluded => NextFrame::WaitUntil(now + occluded_wait.clamp(OCCLUDED_RETRY, OCCLUDED_MAX)),
            Missed::Timeout if paced => NextFrame::Idle,
            Missed::Timeout => NextFrame::WaitUntil(now + TIMEOUT_RETRY),
            Missed::Outdated => NextFrame::WaitUntil(now + OCCLUDED_RETRY),
        }
    }
}

/// The next drawable. An outdated or lost surface is reconfigured and tried again, up to
/// `reconfigures` times, synchronously; every other miss returns at once. On macOS a surface
/// wgpu would call occluded is never asked: each failed `get_current_texture` leaks a texture
/// id in wgpu-core 30.0.1.
pub(crate) fn acquire(surface: &wgpu::Surface, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration, mut reconfigures: u32,
    meter: &mut SurfaceMeter) -> Result<wgpu::SurfaceTexture, Missed> {
    #[cfg(target_os = "macos")]
    if crate::macos::surface_occluded(surface) {
        meter.skipped();
        return Err(Missed::Occluded);
    }
    loop {
        let status = Missed::of(surface.get_current_texture());
        if let Err(missed) = status { meter.failed(missed); }
        match status {
            Err(Missed::Outdated) if reconfigures > 0 => {
                surface.configure(device, config);
                reconfigures -= 1;
            }
            Err(Missed::Outdated) => {
                surface.configure(device, config);
                return Err(Missed::Outdated);
            }
            other => return other,
        }
    }
}

#[test]
fn missed_frames_never_poll() {
    use wgpu::CurrentSurfaceTexture as S;
    let now = Instant::now();
    for status in [S::Occluded, S::Timeout, S::Outdated, S::Lost, S::Validation] {
        let missed = Missed::of(status).err().expect("a miss");
        for paced in [false, true] {
            for wait in [Duration::ZERO, OCCLUDED_RETRY, Duration::from_secs(60)] {
                // `NextFrame` has no Poll; a deadline at or before now would spin the same way.
                let flow = match missed.next(paced, now, wait) {
                    NextFrame::WaitUntil(at) => { assert!(at > now, "{missed:?} retries later, not now"); ControlFlow::WaitUntil(at) }
                    NextFrame::Idle => ControlFlow::Wait,
                };
                assert_ne!(flow, ControlFlow::Poll, "{missed:?} (paced: {paced}) must not poll");
            }
        }
    }
    let occluded = |wait| Missed::Occluded.next(true, now, wait);
    assert_eq!(occluded(Duration::ZERO), NextFrame::WaitUntil(now + OCCLUDED_RETRY), "the safety net is slow");
    assert_eq!(occluded(Duration::from_secs(60)), NextFrame::WaitUntil(now + OCCLUDED_MAX));
    assert_eq!(Missed::Timeout.next(true, now, Duration::ZERO), NextFrame::Idle, "a running display link retries on its tick");
    assert_eq!(Missed::Timeout.next(false, now, Duration::ZERO), NextFrame::WaitUntil(now + TIMEOUT_RETRY));
}

/// A Metal surface whose window reports itself covered, with no window server needed: the layer's
/// delegate stands in for the backing view, and is its own window.
#[cfg(all(test, target_os = "macos"))]
mod occluded_surface {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use objc2_video::{ClassType, define_class, msg_send, rc::Retained, runtime::{AnyClass, AnyObject, NSObject}};

    /// The fake window's `occlusionState`: 0 covered, 2 (`NSWindowOcclusionStateVisible`) shown.
    static OCCLUSION: AtomicUsize = AtomicUsize::new(0);

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "RuneFakeOccludedView"]
        struct FakeView;

        impl FakeView {
            #[unsafe(method(window))]
            fn window(&self) -> *const Self { self }

            #[unsafe(method(occlusionState))]
            fn occlusion_state(&self) -> usize { OCCLUSION.load(Ordering::Relaxed) }
        }
    );

    /// An hour covered costs no failed `get_current_texture` (each leaks a texture id in
    /// wgpu-core 30.0.1) and a re-check at most every two seconds; uncovered, it draws.
    #[test]
    fn an_occluded_surface_never_asks_wgpu_and_draws_once_visible() {
        let view: Retained<FakeView> = unsafe { msg_send![FakeView::class(), new] };
        let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
        let _: () = unsafe { msg_send![&*layer, setDelegate: &*view] };
        let instance = Gpu::instance();
        let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
        let surface = unsafe { instance.create_surface_unsafe(target) }.expect("a Metal layer surface");
        let gpu = match Gpu::try_new(instance, Some(&surface)) {
            Ok(gpu) => gpu,
            Err(error) => return eprintln!("skipped: {error}"),
        };
        let Some((config, _)) = surface_config(&surface, &gpu.adapter, (64, 64), false) else {
            return eprintln!("skipped: the adapter can't present to a Metal layer");
        };
        surface.configure(&gpu.device, &config);

        OCCLUSION.store(0, Ordering::Relaxed);
        assert!(unsafe { crate::macos::layer_occluded(&layer) });
        let mut meter = SurfaceMeter::new("occluded", Arc::default());
        let start = Instant::now();
        let (mut at, mut wait, mut checks) = (start, OCCLUDED_RETRY, 0u64);
        while at < start + Duration::from_secs(3600) {
            if let Ok(_frame) = acquire(&surface, &gpu.device, &config, 1, &mut meter) { panic!("an occluded surface got a drawable"); }
            checks += 1;
            let NextFrame::WaitUntil(next) = Missed::Occluded.next(false, at, wait) else { panic!("occlusion re-checks on a deadline") };
            wait = (wait * 2).min(OCCLUDED_MAX);
            at = next;
        }
        let totals = meter.totals();
        assert_eq!(totals.failed(), 0, "wgpu was asked for drawables it refuses as occluded");
        assert_eq!(totals.skipped, checks);
        assert!(checks <= 3600 / 2 + 3, "{checks} re-checks in an hour occluded");

        OCCLUSION.store(2, Ordering::Relaxed);
        let frame = acquire(&surface, &gpu.device, &config, 1, &mut meter).expect("a visible surface gets a drawable");
        let target = frame.texture.create_view(&Default::default());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        gpu.queue.submit([encoder.finish()]);
        gpu.queue.present(frame);
        meter.presented();
        assert_eq!((meter.totals().presents, meter.totals().failed()), (1, 0));
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "RuneFakeCoveredView"]
        struct CoveredView;

        impl CoveredView {
            #[unsafe(method(window))]
            fn window(&self) -> *const Self { self }

            #[unsafe(method(occlusionState))]
            fn occlusion_state(&self) -> usize { 0 }
        }
    );

    /// A window's first frame draws before the window server reports the window on screen:
    /// `PresumeShown` gets it a drawable from wgpu, and puts the layer's own delegate back.
    #[test]
    fn a_presumed_shown_surface_gets_its_first_drawable_while_reported_covered() {
        let view: Retained<CoveredView> = unsafe { msg_send![CoveredView::class(), new] };
        let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
        let _: () = unsafe { msg_send![&*layer, setDelegate: &*view] };
        let instance = Gpu::instance();
        let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
        let surface = unsafe { instance.create_surface_unsafe(target) }.expect("a Metal layer surface");
        let gpu = match Gpu::try_new(instance, Some(&surface)) {
            Ok(gpu) => gpu,
            Err(error) => return eprintln!("skipped: {error}"),
        };
        let Some((config, _)) = surface_config(&surface, &gpu.adapter, (64, 64), false) else {
            return eprintln!("skipped: the adapter can't present to a Metal layer");
        };
        surface.configure(&gpu.device, &config);
        let mut meter = SurfaceMeter::new("first", Arc::default());
        assert!(acquire(&surface, &gpu.device, &config, 1, &mut meter).is_err(), "a covered window gets no drawable");

        let shown = crate::macos::PresumeShown::new(&surface).expect("a Metal surface");
        assert!(!crate::macos::surface_occluded(&surface));
        let stand_in: *const AnyObject = unsafe { msg_send![&*layer, delegate] };
        let null: *const AnyObject = unsafe { msg_send![AnyClass::get(c"NSNull").expect("Foundation"), null] };
        let action: *const AnyObject = unsafe { msg_send![stand_in, actionForLayer: &*layer, forKey: std::ptr::null::<AnyObject>()] };
        assert_eq!(action, null, "PresumeShown suppresses implicit Core Animation actions");
        let frame = acquire(&surface, &gpu.device, &config, 1, &mut meter).expect("the first frame's drawable");
        drop(frame);
        drop(shown);

        let delegate: *const AnyObject = unsafe { msg_send![&*layer, delegate] };
        assert!(std::ptr::eq(delegate, Retained::as_ptr(&view).cast()), "the layer's delegate is back");
        assert!(crate::macos::surface_occluded(&surface));
        assert_eq!(meter.totals().failed(), 0, "wgpu was never asked while covered");
    }
}

/// `setup`'s waker: `AppHandle::wake`, so a drain that also holds updates that changed nothing
/// still polls and redraws for it.
fn setup_waker<S: 'static>(shared: &Arc<crate::app::Shared<S>>) -> Waker {
    let handle = crate::AppHandle { shared: shared.clone() };
    Arc::new(move || handle.wake())
}

/// What a shown window that can't draw runs for an update, a wake, a due timer or a redraw:
/// covered (an occlusion event, or a drawable refused as occluded), minimised, or waiting out
/// a missed frame. `poll`, due timers and queued input run now, as for a hidden window
/// (`Runtime::hidden_wake`); the view and layout wait, dirty, for the frame that uncovers it.
/// Uncovering settles first, so that frame builds once and paints motion where it would have
/// ended, as Core Animation's time-based motion has by the time a covered window shows again.
pub(crate) fn covered_step<S: 'static>(rt: &mut Runtime<S>) { rt.hidden_wake(); }

#[cfg(test)]
mod covered {
    use super::*;
    use crate::{Cx, El, Env, Spring, el, text};
    use std::cell::Cell;

    thread_local! { static BUILDS: Cell<u32> = const { Cell::new(0) }; }

    #[derive(Default)]
    struct Chat { messages: u32, read: u32, polls: u32 }

    fn view(chat: &Chat, _: &mut Cx<Chat>) -> El {
        BUILDS.set(BUILDS.get() + 1);
        el().child(text(format!("{} {}", chat.messages, chat.read)).key("n"))
    }

    fn shown_text(rt: &Runtime<Chat>) -> Option<String> {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("n")).and_then(|e| e.text)
    }

    /// A covered window whose state changes 100 times, from updates, their timers and a poll
    /// that reads an inbox, builds no view and lays nothing out, while `poll` and the timers
    /// still run. Uncovering builds once, with the latest state.
    #[test]
    fn a_covered_window_builds_nothing_until_it_uncovers() {
        let mut rt = Runtime::new(Chat::default(), view, (200.0, 50.0), Env { system_dark: true, scale: 1.0 });
        rt.poll = Some(|chat, _, _| { chat.polls += 1; let unread = chat.read < chat.messages; chat.read = chat.messages; unread });
        let start = Instant::now();
        rt.wall_start = Some(start);
        rt.frame(0.0);
        assert_eq!(shown_text(&rt).as_deref(), Some("0 0"));
        // As the host does for `WindowEvent::Occluded(true)`.
        rt.set_live_visible(false);
        let (builds, commits, polls) = (BUILDS.get(), rt.stats.commits, rt.state.polls);
        for k in 1..=100 {
            rt.update(|chat, tx| { chat.messages += 1; tx.wake_after(Duration::from_millis(10)); });
            covered_step(&mut rt);
            // Its timer comes due before the next message.
            rt.wall_start = Some(start - k * Duration::from_millis(20));
            covered_step(&mut rt);
        }
        assert_eq!((rt.state.messages, rt.state.read), (100, 100), "updates and poll ran");
        assert_eq!(rt.state.polls - polls, 200, "poll runs for every update and timer");
        assert_eq!(rt.wake_deadline(), None, "due timers are spent on time");
        assert_eq!(BUILDS.get() - builds, 0, "no view builds while covered");
        assert_eq!(rt.stats.commits, commits, "no layout while covered");
        assert!(rt.stale(), "the window is marked out of date");

        // As the host does for `WindowEvent::Occluded(false)`, then its first frame.
        rt.set_live_visible(true);
        rt.settle_all();
        rt.frame(1.0 / 120.0);
        assert_eq!(BUILDS.get() - builds, 1, "uncovering builds once");
        assert_eq!(shown_text(&rt).as_deref(), Some("100 100"), "with the latest state");
    }

    /// Queued input still runs its handlers while covered (a key into a covered window that's
    /// still key), which takes a frame without drawing.
    #[test]
    fn a_covered_window_still_hears_queued_input() {
        let mut rt = Runtime::new(Chat::default(), view, (200.0, 50.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.set_live_visible(false);
        rt.window_focus(false);
        assert!(rt.keys_queued());
        covered_step(&mut rt);
        assert!(!rt.has_pending_input(), "the focus change was delivered");
    }

    /// Motion running when the window is covered, and motion asked for while it's covered,
    /// neither advances nor builds while covered; uncovering lands both in one frame, so
    /// nothing plays out after.
    #[test]
    fn a_covered_window_lands_its_motion_when_it_uncovers() {
        fn view(x: &f32, _: &mut Cx<f32>) -> El {
            crate::col().child(el().key("box").absolute(*x, 0.0).size(10.0, 10.0))
        }
        let x = |rt: &Runtime<f32>| rt.presented("box").unwrap().0[0];
        let mut rt = Runtime::new(0.0_f32, view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.update(|x, tx| { *x = 80.0; tx.motion(Spring::new(0.3, 0.0)); });
        rt.frame(1.0 / 120.0);
        rt.frame(0.05);
        let mid = x(&rt);
        assert!(mid > 0.0 && mid < 80.0 && rt.animating, "in flight: {mid}");
        rt.set_live_visible(false);
        let commits = rt.stats.commits;
        rt.update(|x, tx| { *x = 40.0; tx.motion(Spring::new(0.3, 0.0)); });
        covered_step(&mut rt);
        assert_eq!(x(&rt), mid, "nothing moves while covered");
        assert_eq!(rt.stats.commits, commits, "or builds");
        rt.set_live_visible(true);
        rt.settle_all();
        rt.frame(1.0 / 120.0);
        assert_eq!(x(&rt), 40.0, "uncovering paints where the motion ends");
        assert!(!rt.animating, "with nothing left to play");
    }
}

/// Whether a drain of other threads' requests asks the window for a frame. A hidden launcher
/// renders a change into its cached frame (paced); a shown launcher redraws for a change or
/// as it shows; any other shown window redraws, unless only updates that changed nothing
/// (`quiet`) woke it.
fn drain_frame(launcher: bool, visible: bool, was_shown: bool, changed: bool, quiet: bool) -> bool {
    if launcher && !visible { return changed; }
    !quiet && if launcher { changed || was_shown && visible } else { visible }
}

/// A vsynced sRGB (or non-sRGB when `srgb_blend` is set) configuration for the surface, and the
/// first alpha mode it supports that composites translucent pixels.
pub(crate) fn surface_config(surface: &wgpu::Surface, adapter: &wgpu::Adapter, (width, height): (u32, u32), srgb_blend: bool)
    -> Option<(wgpu::SurfaceConfiguration, Option<wgpu::CompositeAlphaMode>)> {
    let mut config = surface.get_default_config(adapter, width.max(1), height.max(1))?;
    let caps = surface.get_capabilities(adapter);
    let srgb = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(config.format);
    config.format = if srgb_blend {
        let non_srgb = srgb.remove_srgb_suffix();
        if caps.formats.contains(&non_srgb) {
            non_srgb
        } else {
            config.view_formats = vec![non_srgb];
            srgb
        }
    } else {
        srgb
    };
    config.usage |= caps.usages & wgpu::TextureUsages::COPY_SRC;
    config.present_mode = wgpu::PresentMode::AutoVsync;
    config.desired_maximum_frame_latency = 2;
    Some((config, crate::translucency::mode(surface, adapter)))
}

/// Follows a blend switch (`Tx::set_srgb_blend`) on a configured surface: reconfigures it for
/// `rt`'s mode at its size and alpha mode, and rebuilds `renderer` when its target format
/// changes. Returns the new target format, or `None` when the surface can't be configured.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn reblend<S: 'static>(surface: &wgpu::Surface, adapter: &wgpu::Adapter, device: &wgpu::Device,
    config: &mut wgpu::SurfaceConfiguration, renderer: &mut Renderer, rt: &mut Runtime<S>) -> Option<wgpu::TextureFormat> {
    let srgb = rt.srgb_blend();
    let (next, _) = surface_config(surface, adapter, (config.width, config.height), srgb)?;
    if (config.format, &config.view_formats) != (next.format, &next.view_formats) {
        (config.format, config.view_formats) = (next.format, next.view_formats);
        surface.configure(device, config);
    }
    let target = if srgb { config.format.remove_srgb_suffix() } else { config.format };
    rebuild_renderer(device, renderer, target, rt);
    Some(target)
}

/// Switches `renderer` to `target` when its format changed, keeping its atlases, layer targets
/// and extra pipelines, and sets its blend mode to `rt`'s.
pub(crate) fn rebuild_renderer<S: 'static>(device: &wgpu::Device, renderer: &mut Renderer, target: wgpu::TextureFormat, rt: &mut Runtime<S>) {
    renderer.set_format(device, target);
    renderer.set_srgb_blend(rt.srgb_blend());
}

pub(crate) fn is_wayland(event_loop: &ActiveEventLoop) -> bool {
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    return winit::platform::wayland::ActiveEventLoopExtWayland::is_wayland(event_loop);
    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    {
        let _ = event_loop;
        false
    }
}

/// A window's content top-left in points from the primary display's top-left (see
/// `El::on_screen_layout`), the space an overlay's `OverlayPlacement::Rect` on
/// `DisplayTarget::Primary` is in. Elsewhere than macOS that's desktop pixels from the
/// primary's origin over the window's scale, as `overlay::position` converts the other way.
/// Wayland hides it.
pub(crate) fn screen_origin(window: &Window) -> Option<(f32, f32)> {
    #[cfg(target_os = "macos")]
    return crate::macos::content_origin(window);
    #[cfg(not(target_os = "macos"))]
    {
        let p = window.inner_position().ok()?;
        let o = window.primary_monitor().map_or(winit::dpi::PhysicalPosition::new(0, 0), |m| m.position());
        let scale = window.scale_factor();
        Some(((f64::from(p.x - o.x) / scale) as f32, (f64::from(p.y - o.y) / scale) as f32))
    }
}

/// The scale an overlay draws at. On macOS, read from the panel AppKit shows, and before it's
/// ordered in from the display holding most of its frame, where it will appear.
pub(crate) fn backing_scale(window: &Window) -> f32 {
    #[cfg(target_os = "macos")]
    if let Some(scale) = crate::macos::backing_scale(window) { return scale; }
    window.scale_factor() as f32
}

/// Native titlebar behaviour the framework owns: traffic-light placement, drag regions and the
/// double-click action. Elsewhere only the portable parts apply.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn uses_drag_regions(chrome: Chrome, borderless: bool, panel: bool) -> bool {
    chrome.drag_regions && (borderless || chrome.full_size_content) && !panel
}

#[test]
fn a_window_s_first_updates_can_register_on_a_later_window() {
    use crate::{App, AppSpec, MenuItem, el};
    type S = Vec<String>;
    let apps: Vec<App<S>> = ["first", "second"].map(|title| App::new(AppSpec::new(title, (10.0, 10.0), Vec::new, |_, _| el()))).into();
    let second = apps[1].handle();
    apps[0].handle().update(move |s: &mut S, _| {
        s.push(format!("{:?}", second.register_shortcut("ctrl+alt+shift+f18".parse().unwrap(), |_, _| {}).map(|_| ())));
        s.push(format!("{:?}", second.set_menu([MenuItem::new("Item", |_: &mut S, _| {})])));
    });
    // As Host::new and Host::resumed: attach the guards, then drain the first updates.
    let hosts = start_all(apps.into(), |_, app| {
        let shortcuts = crate::shortcuts::Guard::attach(&app.shared, Arc::new(|| {}), true, false);
        let menus = crate::native_menu::Guard::attach(&app.shared, true);
        Some((app, shortcuts, menus, S::new()))
    }, |(app, _, _, state)| for f in app.shared.take().updates { f(state, &mut Tx::default()); });
    assert_eq!(hosts[0].as_ref().unwrap().3, ["Ok(())", "Ok(())"]);
}

#[test]
fn drag_region_policy_preserves_titled_windows_and_panels() {
    for enabled in [false, true] {
        for full_size in [false, true] {
            let chrome = Chrome { drag_regions: enabled, full_size_content: full_size, ..Chrome::DEFAULT };
            assert_eq!(uses_drag_regions(chrome, false, false), enabled && full_size);
            assert_eq!(uses_drag_regions(chrome, true, false), enabled);
            assert!(!uses_drag_regions(chrome, false, true));
            assert!(!uses_drag_regions(chrome, true, true));
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
    use objc2::{msg_send, sel, ClassType};
    use objc2_app_kit::{NSEvent, NSPanel, NSWindowButton, NSWindowStyleMask};
    use objc2_foundation::{NSString, NSUserDefaults};
    use winit::window::Window;
    use crate::macos::{content_view, ns_window};

    unsafe extern "C" fn cannot_move_window(_: &AnyObject, _: Sel) -> Bool {
        Bool::NO
    }

    /// The content view stops AppKit from moving the window when the titlebar band is pressed;
    /// only `.window_drag()` regions move it. Install an equal-sized subclass on this instance,
    /// never a method on winit's shared class. Other windows keep their original class.
    pub fn own_window_dragging(window: &Window) {
        let Some(view) = content_view(window) else { return };
        let Some(win) = view.window() else { return };
        let panel: Bool = unsafe { msg_send![&*win, isKindOfClass: NSPanel::class()] };
        let full_size = win.styleMask().contains(NSWindowStyleMask::FullSizeContentView);
        let borderless = !win.styleMask().contains(NSWindowStyleMask::Titled);
        let chrome = super::Chrome { full_size_content: full_size, drag_regions: true, ..super::Chrome::DEFAULT };
        if !super::uses_drag_regions(chrome, borderless, panel.as_bool()) { return; }
        unsafe { scope_drag_regions(&view) };
    }

    /// Main-thread native views only. The subclass adds no ivars, preserving winit's layout,
    /// methods and deallocation. Registered classes live for the process lifetime.
    unsafe fn scope_drag_regions(view: &AnyObject) {
        let base = view.class();
        if base.name().starts_with("RuneDragRegions_") { return; }
        let name = format!("RuneDragRegions_{}", base.name());
        let class = match ClassBuilder::new(&name, base) {
            Some(mut builder) => {
                // SAFETY: the signature matches -[NSView mouseDownCanMoveWindow].
                unsafe { builder.add_method(sel!(mouseDownCanMoveWindow), cannot_move_window as unsafe extern "C" fn(_, _) -> _) };
                builder.register()
            }
            None => AnyClass::get(&name).expect("register Rune drag-region view"),
        };
        // SAFETY: called on a live view on the main thread. Only its isa changes, to a subclass
        // without ivars; the original class is never modified.
        unsafe { AnyObject::set_class(view, class) };
    }

    #[test]
    fn drag_subclass_changes_only_the_selected_instance() {
        // NSObject exercises the Objective-C class operation without constructing a window
        // or requiring an AppKit event loop on the test thread.
        use objc2::{ffi, rc::Retained};
        let selected = objc2_foundation::NSObject::new();
        let sibling = objc2_foundation::NSObject::new();
        unsafe {
            let selected_ptr = Retained::as_ptr(&selected).cast_mut().cast();
            let sibling_ptr = Retained::as_ptr(&sibling).cast_mut().cast();
            let base = ffi::object_getClass(selected_ptr);
            let selector = sel!(mouseDownCanMoveWindow).as_ptr();
            let original = ffi::class_getInstanceMethod(base, selector);
            scope_drag_regions(&selected);
            let scoped = ffi::object_getClass(selected_ptr);
            assert_ne!(scoped, base);
            assert_eq!(ffi::class_getSuperclass(scoped), base);
            assert_eq!(ffi::class_getInstanceSize(scoped), ffi::class_getInstanceSize(base));
            assert_eq!(ffi::object_getClass(sibling_ptr), base);
            assert_eq!(ffi::class_getInstanceMethod(base, selector), original);
            let moves: Bool = msg_send![&*selected, mouseDownCanMoveWindow];
            assert!(!moves.as_bool());
            scope_drag_regions(&selected);
            assert_eq!(ffi::object_getClass(selected_ptr), scoped);
        }
    }

    /// Redraws the window once it has entered or left full screen, so the redraw can place the
    /// traffic lights and read the new `titlebar_inset`: winit sends no event for either, and
    /// resizes during the transition arrive while the style mask still says full screen.
    pub struct FullScreenWatch {
        observers: Vec<objc2::rc::Retained<objc2_foundation::NSObject>>,
        changed: std::rc::Rc<std::cell::Cell<bool>>,
    }

    impl FullScreenWatch {
        pub fn new(window: &Window) -> Self {
            use objc2::rc::{Retained, Weak};
            use objc2_foundation::NSObject;
            let changed = std::rc::Rc::new(std::cell::Cell::new(false));
            let none = || Self { observers: Vec::new(), changed: changed.clone() };
            let (Some(win), Some(view)) = (ns_window(window), content_view(window)) else { return none() };
            let Some(center) = AnyClass::get("NSNotificationCenter") else { return none() };
            let weak = Weak::from_retained(&view);
            let flag = changed.clone();
            let block = block2::RcBlock::new(move |_: *mut AnyObject| {
                flag.set(true);
                if let Some(view) = weak.load() { unsafe { view.setNeedsDisplay(true) }; }
            });
            let names = ["NSWindowDidEnterFullScreenNotification", "NSWindowDidExitFullScreenNotification"];
            // SAFETY: main thread; the block outlives the observers, which `Drop` removes.
            let observers = names.into_iter().filter_map(|name| unsafe {
                let center: Retained<AnyObject> = objc2::msg_send_id![center, defaultCenter];
                let name = NSString::from_str(name);
                let token: Option<Retained<NSObject>> = objc2::msg_send_id![&*center, addObserverForName: &*name, object: &*win,
                    queue: std::ptr::null::<AnyObject>(), usingBlock: &*block];
                token
            }).collect();
            Self { observers, changed }
        }

        /// Whether full screen started or ended since the last call.
        pub fn take(&self) -> bool {
            self.changed.replace(false)
        }
    }

    impl Drop for FullScreenWatch {
        fn drop(&mut self) {
            let Some(center) = AnyClass::get("NSNotificationCenter") else { return };
            for token in &self.observers {
                // SAFETY: removes an observer this watch added.
                unsafe {
                    let center: objc2::rc::Retained<AnyObject> = objc2::msg_send_id![center, defaultCenter];
                    let _: () = msg_send![&*center, removeObserver: &**token];
                }
            }
        }
    }

    /// The zoom button's trailing edge in window points, or 0 when the buttons don't sit over
    /// the content: full screen, a standard titlebar, or hidden buttons.
    pub fn titlebar_inset(window: &Window) -> f32 {
        let Some(win) = ns_window(window) else { return 0.0 };
        let mask = win.styleMask();
        if mask.contains(NSWindowStyleMask::FullScreen) || !mask.contains(NSWindowStyleMask::FullSizeContentView) {
            return 0.0;
        }
        let Some(zoom) = win.standardWindowButton(NSWindowButton::NSWindowZoomButton) else { return 0.0 };
        // SAFETY: AppKit's own button, on the main thread.
        let r = unsafe {
            if zoom.isHiddenOrHasHiddenAncestor() { return 0.0; }
            zoom.convertRect_toView(zoom.bounds(), None)
        };
        (r.origin.x + r.size.width) as f32
    }

    pub fn double_click_interval() -> f64 {
        unsafe { NSEvent::doubleClickInterval() }
    }

    /// System Settings › Desktop & Dock › "Double-click a window's title bar to".
    pub fn titlebar_double_click(window: &Window) {
        // SAFETY: reading a user default on the main thread.
        let action = unsafe { NSUserDefaults::standardUserDefaults().stringForKey(&NSString::from_str("AppleActionOnDoubleClick")) };
        match action.map(|s| s.to_string()).as_deref() {
            Some("Minimize") => window.set_minimized(true),
            Some("None") => {}
            _ => window.set_maximized(!window.is_maximized()),
        }
    }
}

/// Places the traffic lights and returns `Cx::titlebar_inset`.
fn place_chrome(window: &Window, chrome: Chrome) -> f32 {
    #[cfg(target_os = "macos")]
    {
        if let Some(p) = chrome.traffic_lights {
            crate::macos_traffic_lights::place(window, p);
        }
        mac::titlebar_inset(window)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, chrome);
        0.0
    }
}

fn titlebar_double_click(window: &Window) {
    #[cfg(target_os = "macos")]
    mac::titlebar_double_click(window);
    #[cfg(not(target_os = "macos"))]
    window.set_maximized(!window.is_maximized());
}

pub(crate) fn pointer_button(button: MouseButton) -> crate::PointerButton {
    match button {
        MouseButton::Left => crate::PointerButton::Primary,
        MouseButton::Right => crate::PointerButton::Secondary,
        MouseButton::Middle => crate::PointerButton::Middle,
        MouseButton::Back => crate::PointerButton::Back,
        MouseButton::Forward => crate::PointerButton::Forward,
        MouseButton::Other(n) => crate::PointerButton::Other(n),
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn gesture_phase(phase: TouchPhase) -> crate::GesturePhase {
    match phase {
        TouchPhase::Started => crate::GesturePhase::Began,
        TouchPhase::Moved => crate::GesturePhase::Moved,
        TouchPhase::Ended | TouchPhase::Cancelled => crate::GesturePhase::Ended,
    }
}

impl<S: 'static> Host<S> {
    fn launcher(&self) -> bool { cfg!(target_os = "macos") && self.macos_window.launcher }

    fn reveal_launcher(&mut self, event_loop: &ActiveEventLoop) {
        self.apply_placement(event_loop);
        if self.win.is_none() { self.create(event_loop, None, false); }
        let stale = self.rt.needs_commit();
        let Some(w) = &mut self.win else { return };
        w.hidden_pace.reset();
        let visible = w.visible;
        if !w.first_frame.request_reveal(stale, visible) {
            w.window.request_redraw();
            return;
        }
        // Already showing: its next frame draws the new state, or a covered one's retry does.
        if visible && stale { w.window.request_redraw(); }
        self.show_cached();
    }

    fn show_cached(&mut self) {
        let Some(w) = &mut self.win else { return };
        w.show(true);
        w.visible = true;
        w.occluded = false;
        self.rt.set_live_visible(true);
        // No settle, frame, surface configure or acquisition on a cached reveal, unless
        // the refreshed frame left motion running.
        if !self.idle { w.window.request_redraw(); }
    }

    fn first_frame_ready(&mut self, event_loop: &ActiveEventLoop) {
        let Some(w) = &mut self.win else { return };
        if !w.first_frame.pending() { return; }
        let reveal = w.first_frame.complete();
        // Mapped at a fixed size, so a tiling window manager floated it: now the user can resize it.
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.float_on_open && !w.first_frame.notified {
            use winit::dpi::LogicalSize;
            let (min, max) = self.options.size_limits(self.spec.min_size);
            w.window.set_min_inner_size(Some(LogicalSize::new(min.0, min.1)));
            w.window.set_max_inner_size(max.map(|(w, h)| LogicalSize::new(w, h)));
        }
        self.native_handle.native.ready.store(true, Ordering::Release);
        *self.shared.window_error.lock().unwrap() = None;
        if !std::mem::replace(&mut w.first_frame.notified, true)
            && let Some(ready) = self.on_ready.as_mut() { ready(&self.native_handle); }
        if reveal {
            self.reveal_launcher(event_loop);
            self.shell(event_loop);
        }
        self.publish();
    }

    fn place(&mut self, event_loop: &ActiveEventLoop, request: crate::windows::PlaceRequest) {
        self.options.position = None;
        self.placement = Some(request);
        self.apply_placement(event_loop);
    }

    fn apply_placement(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(request), Some(w)) = (&self.placement, &self.win) else { return };
        if is_wayland(event_loop) { return; }
        #[cfg(target_os = "macos")]
        {
            let _ = request.pointer;
            crate::macos::place_window(&w.window, self.rt.viewport, request);
        }
        #[cfg(not(target_os = "macos"))]
        if let Some(monitor) = crate::overlay::monitor_at(event_loop, &request.display, request.pointer) {
            let scale = monitor.scale_factor();
            let area = monitor.size();
            let rect = crate::overlay_options::local_rect(crate::windows::PlaceRequest::at(self.rt.viewport), Some(request.placement),
                (area.width as f32 / scale as f32, area.height as f32 / scale as f32));
            let origin = monitor.position();
            let _ = w.window.request_inner_size(LogicalSize::new(rect[2], rect[3]));
            w.window.set_outer_position(PhysicalPosition::new(origin.x + (f64::from(rect[0]) * scale).round() as i32,
                origin.y + (f64::from(rect[1]) * scale).round() as i32));
        }
        self.sync_origin();
    }

    /// Seconds on the host's frame clock, which frame animations sample.
    fn frame_secs(&self, at: Instant) -> f64 {
        self.rt.wall_start.map_or(0.0, |start| at.saturating_duration_since(start).as_secs_f64())
    }

    /// Takes the frame change handlers asked for. A painting macOS window moves over its next
    /// frames on the request's spring, from `start`; anywhere else the frame jumps. Sizes stay
    /// inside the window's minimum and maximum. A size outlives the window: one created later
    /// opens at it.
    fn take_frame_request(&mut self, start: f64, painting: bool) {
        self.rt.fit_held = false;
        let Some(mut request) = self.rt.window_frame.take() else { return };
        let (min, max) = self.options.size_limits(self.spec.min_size);
        request.size = request.size.map(|size| crate::window_frame::clamp_size(size, min, max));
        if let Some(size) = request.size {
            self.spec.size = size;
            self.args.size = None;
        }
        let Some(w) = &self.win else { return };
        #[cfg(target_os = "macos")]
        {
            use crate::window_frame::{FrameAnim, snap, target};
            let Some((frame, visible, scale)) = crate::macos::window_geometry(&w.window) else { return };
            // Resizing again mid-flight keeps the edge the first resize keeps.
            let base = self.frame_anim.map_or(frame, |a| a.to);
            let outer = request.size.and_then(|size| crate::macos::outer_size(&w.window, size));
            let to = snap(target(base, outer, request.center, visible), scale);
            // Centring alone carries on with a resize already moving.
            let spring = request.spring.or(self.frame_anim.and_then(|a| a.spring).filter(|_| request.size.is_none()));
            if painting {
                self.frame_anim = Some(FrameAnim::retarget(self.frame_anim.as_ref(), spring, start, frame, to));
                self.frame_origin = Some([frame[0], frame[1]]);
            } else {
                self.frame_anim = None;
                crate::macos::set_window_frame(&w.window, to);
                self.sync_origin();
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (start, painting);
            let window = &w.window;
            // Read before the resize request, which may apply at once.
            let geometry = (window.current_monitor(), window.outer_position(), window.outer_size(), window.inner_size());
            if let Some((width, height)) = request.size {
                self.frame_jump = Some((width, height));
                let _ = window.request_inner_size(LogicalSize::new(width, height));
            }
            // Wayland windows can't see or set their position.
            if self.wayland { return; }
            let (Some(monitor), Ok(position), outer, inner) = geometry else { return };
            let scale = window.scale_factor() as f32;
            let size = request.size.map(|(width, height)| (
                width * scale + outer.width.saturating_sub(inner.width) as f32,
                height * scale + outer.height.saturating_sub(inner.height) as f32,
            ));
            let current = [position.x as f32, position.y as f32, outer.width as f32, outer.height as f32];
            let (origin, area) = (monitor.position(), monitor.size());
            let visible = [origin.x as f32, origin.y as f32, area.width as f32, area.height as f32];
            let [x, y, ..] = crate::window_frame::target(current, size, request.center, visible);
            if [x, y] != [current[0], current[1]] {
                window.set_outer_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
                self.sync_origin();
            }
        }
    }

    /// Moves the frame one display frame along, before the frame's drawable is acquired, and
    /// lays the content out at the new size: content follows the edge in the same frame, with
    /// no stretched or blank strip.
    fn step_frame(&mut self, at: Instant) {
        let now = self.frame_secs(at);
        self.take_frame_request(now, true);
        #[cfg(target_os = "macos")]
        {
            use crate::window_frame::snap;
            // The window server shows a move on the first frame after an idle spell before the
            // frame drawn for it, leaving a stale strip. A request taken then starts here and
            // moves from the next frame.
            let resting = self.idle;
            let moving = self.frame_anim.is_some();
            let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) else { return };
            w.present_with_transaction(moving);
            if resting { return; }
            let Some(anim) = self.frame_anim.as_mut() else { return };
            let Some((frame, _, scale)) = crate::macos::window_geometry(&w.window) else { return };
            // Someone dragged the window mid-flight: the drag wins. The window stays where they
            // put it and the size carries on from there.
            if let Some([x, y]) = self.frame_origin && ((frame[0] - x).abs() > 0.5 || (frame[1] - y).abs() > 0.5) {
                anim.shift(frame[0] - x, frame[1] - y);
            }
            let rect = snap(anim.sample(now).0, scale);
            crate::macos::set_window_frame(&w.window, rect);
            self.frame_origin = Some([rect[0], rect[1]]);
            if anim.done(now) { self.frame_anim = None; }
            self.rt.set_titlebar_inset(place_chrome(&w.window, self.chrome));
            let size = w.window.inner_size();
            w.resize_surface(&gpu.device, size);
            let scale = w.window.scale_factor() as f32;
            self.rt.step_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
            // In this frame's layout, not the next one when `Moved` arrives.
            if let Some(origin) = screen_origin(&w.window) {
                self.rt.set_window_origin(origin);
            }
        }
    }

    /// A window that stops painting mid-move lands where it was going.
    fn land_frame(&mut self) {
        let anim = self.frame_anim.take();
        #[cfg(target_os = "macos")]
        if let Some(w) = &mut self.win {
            w.present_with_transaction(false);
            w.set_live_resize(false);
            if let Some(anim) = anim { crate::macos::set_window_frame(&w.window, anim.to); }
        }
        #[cfg(target_os = "macos")]
        self.sync_origin();
        #[cfg(not(target_os = "macos"))]
        let _ = anim;
    }

    fn window_request(&mut self, event_loop: &ActiveEventLoop, request: WindowRequest) {
        match request {
            WindowRequest::Open(token) => self.open(event_loop, token),
            WindowRequest::Show => self.show(event_loop),
            WindowRequest::Hide => self.hide(event_loop),
            WindowRequest::Close => self.close(event_loop),
        }
    }

    fn set_position(&mut self, event_loop: &ActiveEventLoop, position: (i32, i32)) {
        self.placement = None;
        self.options.position = Some(position);
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_outer_position(PhysicalPosition::new(position.0, position.1));
        }
        self.sync_origin();
    }

    fn set_always_on_top(&mut self, event_loop: &ActiveEventLoop, on: bool) {
        self.options.always_on_top = on;
        self.macos_window.level = None;
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_window_level(if on { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
        }
    }

    fn show(&mut self, event_loop: &ActiveEventLoop) {
        if self.launcher() { self.reveal_launcher(event_loop); return; }
        // Hidden, it may have missed a new accent's redraw: build with the current one.
        self.rt.set_system_accent(crate::system_accent());
        let built = self.win.as_ref().is_some_and(|w| !w.visible) && self.build_for_show();
        self.apply_placement(event_loop);
        let Some(w) = self.win.as_mut() else { self.create(event_loop, None, false); return };
        if !w.visible {
            w.show(false);
        }
        w.visible = true;
        w.occluded = false;
        self.rt.set_live_visible(!w.minimized());
        self.rt.cancel_timer_frame();
        self.idle = true;
        self.native_handle.native.number.store(native_window_number(&w.window).unwrap_or(0), Ordering::Release);
        if !built { self.rt.settle_all(); }
        w.window.request_redraw();
    }

    /// A hidden or released window that fits its content builds what changed while it was
    /// away and takes its new height before it is ordered in (`Runtime::build_for_show`).
    /// Returns whether it built, settled.
    fn build_for_show(&mut self) -> bool {
        if !self.rt.build_for_show() { return false; }
        self.clipboard.flush(&mut self.rt);
        self.take_frame_request(0.0, false);
        // macOS sets the frame at once: the first drawable and layout take the new size.
        // Elsewhere the window's `Resized` brings it.
        #[cfg(target_os = "macos")]
        if let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) {
            let size = w.window.inner_size();
            w.resize_surface(&gpu.device, size);
            let scale = w.window.scale_factor() as f32;
            self.rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
        }
        true
    }

    fn focus(&self, event_loop: &ActiveEventLoop) {
        let Some(w) = self.win.as_ref().filter(|w| w.visible) else { return };
        if w.window.is_minimized() == Some(true) { w.window.set_minimized(false); }
        if is_wayland(event_loop) {
            w.window.request_user_attention(Some(winit::window::UserAttentionType::Informational));
            return;
        }
        w.focus();
    }

    /// Applies everything other threads asked for since the last wake-up. A `wake` polls a
    /// hidden window first, so a reveal in the same turn sees what `poll` changed.
    fn drain(&mut self, event_loop: &ActiveEventLoop, wake: bool) {
        let shown = self.win.as_ref().is_some_and(|w| w.visible);
        let stale_before = self.rt.needs_commit();
        let clipboard = self.clipboard.receive(&mut self.rt);
        let answered = self.popup.answer(&mut self.rt) | self.file_dialog.answer(&mut self.rt);
        self.shortcuts.drain();
        self.menus.drain();
        let (mut had_updates, mut updated) = (false, false);
        let requests = self.shared.drain(|f| {
            had_updates = true;
            updated |= self.rt.update_if(f);
            self.menus.drain();
        });
        let changed = clipboard || answered || updated || self.rt.drag_ends_pending();
        // Only updates that changed nothing (`AppHandle::update_if`), or an empty startup drain
        // after `try_create` already drew the first frame, ran: no poll, no frame.
        let quiet = (had_updates || (!wake && !stale_before)) && !changed && requests.only_updates()
            && !self.rt.patch_pending() && (stale_before || !self.rt.needs_commit()) && !composing(self.comp.as_deref());
        self.clipboard.flush(&mut self.rt);
        // The next frame polls for the wake.
        if wake && !quiet { self.rt.cancel_timer_frame(); }
        if requests.quit {
            // A tracking menu holds the event loop until it ends.
            self.popup.cancel(&mut self.rt);
            self.file_dialog.cancel(&mut self.rt);
            event_loop.exit();
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(visible) = requests.dock {
            crate::macos::set_dock_icon(visible);
        }
        let shows = matches!(requests.window, Some(WindowRequest::Show | WindowRequest::Open(_)));
        let polled = wake && !quiet && self.poll_hidden(event_loop, shows);
        if let Some(position) = requests.position { self.set_position(event_loop, position); }
        if let Some(place) = requests.place { self.place(event_loop, place); }
        if let Some(request) = requests.window { self.window_request(event_loop, request); }
        if requests.focus { self.focus(event_loop); }
        // After the show it may follow; `focus` and `minimize` are last-wins (`AppHandle`).
        self.rt.minimize_request |= requests.minimize;
        self.shell(event_loop);
        // A pop-up or file dialog a quiet update asked for may have been answered at once.
        let answered = quiet && ((!stale_before && self.rt.needs_commit()) || self.rt.has_pending_input());
        if quiet { self.clipboard.flush(&mut self.rt); }
        let (changed, quiet) = (changed || answered, quiet && !answered);
        // A launcher's cached reveal draws nothing; a hidden one renders its changes hidden,
        // paced (`refresh_hidden`). A covered or minimised one runs them without asking for a
        // frame it wouldn't draw.
        let launcher = self.launcher();
        let framed = match self.win.as_ref().filter(|w| drain_frame(launcher, w.visible, shown, changed, quiet)) {
            None => false,
            Some(w) if launcher && !w.visible => self.refresh_hidden(),
            Some(w) if (!launcher && w.occluded) || w.minimized() => {
                self.covered_frame();
                self.shell(event_loop);
                true
            }
            Some(w) => { w.window.request_redraw(); true }
        };
        // Neither a poll nor a frame comes to schedule the updates' `wake_after`s.
        if !polled && !framed { self.rt.schedule_pending_wakes(); }
    }

    /// A hidden, keep-running or windowless window still reads its inbox when woken: `poll` runs
    /// with no drawable, present or animation, and its changes render when the window is next
    /// shown. A hidden launcher renders poll's changes hidden, paced, keeping its cached frame
    /// fresh. A minimize waits for a show in the same turn (`shows`). Returns whether it polled.
    fn poll_hidden(&mut self, event_loop: &ActiveEventLoop, shows: bool) -> bool {
        if event_loop.exiting() || self.win.as_ref().is_some_and(|w| w.visible) { return false; }
        let changed = self.rt.poll_hidden();
        self.clipboard.flush(&mut self.rt);
        let minimize = shows && std::mem::take(&mut self.rt.minimize_request);
        self.shell(event_loop);
        self.rt.minimize_request |= minimize;
        if changed && self.launcher() { self.refresh_hidden(); }
        true
    }

    /// A hidden launcher renders what changed into its cached frame: at once after a quiet
    /// spell, then at most every `HIDDEN_REFRESH` (`HiddenPace`). A show of state newer than
    /// that frame renders it hidden first (`reveal_launcher`), as for an update in the same
    /// turn. A frame being prepared, or one a reveal waits on, renders at once. Returns whether
    /// it asked for a frame.
    fn refresh_hidden(&mut self) -> bool {
        let now_due = self.hidden_launcher_renders();
        let Some(w) = &mut self.win else { return false };
        let render = now_due || w.first_frame.deadline.is_some() || w.hidden_pace.changed(Instant::now());
        if render { w.window.request_redraw(); }
        render
    }

    /// What a hidden launcher's frame must render as it commits: queued input and drag ends.
    /// Committed without a render, they'd leave a cached frame that looks fresh.
    fn hidden_launcher_renders(&self) -> bool {
        self.rt.has_pending_input() || self.rt.drag_ends_pending()
    }

    /// Carries out what a handler or update asked with `Tx::close_window` / `Tx::quit`, then
    /// opens the pop-up menu it asked for, if the window is still there to open it over.
    fn shell(&mut self, event_loop: &ActiveEventLoop) {
        self.show_title();
        for _ in 0..8 {
            let minimize = std::mem::take(&mut self.rt.minimize_request);
            match self.rt.shell_request.take() {
                Some(Shell::Quit) => {
                    self.popup.cancel(&mut self.rt);
                    self.file_dialog.cancel(&mut self.rt);
                    event_loop.exit();
                }
                Some(Shell::Close) => self.close(event_loop),
                None if minimize => self.minimize(),
                None => {}
            }
            if self.finished || event_loop.exiting() {
                self.popup.cancel(&mut self.rt);
                self.file_dialog.cancel(&mut self.rt);
                return;
            }
            let shown = self.win.as_ref().filter(|w| w.visible);
            let paints = shown.is_some_and(|w| !w.occluded && !w.minimized());
            // A painting macOS window moves its frame over its next frames (`redraw`).
            if self.rt.window_frame.is_some() && !(paints && cfg!(target_os = "macos")) {
                self.take_frame_request(self.frame_secs(Instant::now()), false);
            }
            let shown = self.win.as_ref().filter(|w| w.visible);
            let window = shown.map(|w| &w.window);
            if self.popup.open(&mut self.rt, window, paints) && let Some(w) = window { w.request_redraw(); }
            if self.file_dialog.open(&mut self.rt, window.map(|w| &**w)) && let Some(w) = window { w.request_redraw(); }
            // A callback that ran (a menu that couldn't open) may have asked to close, quit or
            // minimize.
            if self.rt.shell_request.is_none() && !self.rt.minimize_request { return; }
        }
    }

    fn close(&mut self, event_loop: &ActiveEventLoop) {
        self.rt.minimize_request = false;
        if let Some(w) = &mut self.win {
            w.first_frame.hide();
            #[cfg(target_os = "macos")]
            { w.prime = 0; }
        }
        self.secure_input.set(false);
        if !self.closes() { return; }
        self.close_hook();
        self.rt.set_live_visible(false);
        // While the window still shows its last frames.
        if let Some(trace) = &mut self.trace { trace.settle_presents(crate::present_time::SETTLE); }
        if self.rt.shell_request.take() == Some(Shell::Quit) {
            self.popup.cancel(&mut self.rt);
            self.file_dialog.cancel(&mut self.rt);
            event_loop.exit();
            return;
        }
        if !self.keep_running {
            self.popup.cancel(&mut self.rt);
            self.file_dialog.cancel(&mut self.rt);
            self.rt.cancel_pointer();
            self.files = FileDrop::default();
            if let Some(w) = &mut self.win { w.apply_cursor(&self.rt, false); w.hide(); }
            self.finished = true;
            return;
        }
        self.hide(event_loop);
    }

    /// Whether a close runs `on_close`: a window is shown, or hidden without `keep_running`.
    fn closes(&self) -> bool { self.win.as_ref().is_some_and(|w| w.visible || !self.keep_running) }

    fn close_hook(&mut self) {
        if let Some(f) = self.on_close {
            self.rt.update(f);
        }
        // Before the window hides or the app exits: a lock or clear must reach the worker.
        self.clipboard.flush(&mut self.rt);
    }

    fn hide(&mut self, event_loop: &ActiveEventLoop) {
        if self.win.as_ref().is_some_and(|w| w.visible) { self.hidden_at = Some(crate::reopen::stamp()); }
        self.rt.minimize_request = false;
        self.land_frame();
        // Before the window can go: a hide need not report a blur. `about_to_wait` delivers it,
        // after any Fn change the monitor saw first.
        #[cfg(target_os = "macos")]
        self.fn_key();
        self.rt.keys_lost();
        if let Some(w) = &mut self.win {
            w.first_frame.hide();
            #[cfg(target_os = "macos")]
            { w.prime = 0; }
        }
        self.secure_input.set(false);
        self.ime_allowed = false;
        if let Some(w) = &self.win { w.window.set_ime_allowed(false); }
        self.rt.set_live_visible(false);
        self.rt.cancel_timer_frame();
        self.popup.cancel(&mut self.rt);
        self.file_dialog.cancel(&mut self.rt);
        self.rt.cancel_pointer();
        self.files = FileDrop::default();
        if let Some(w) = &mut self.win { w.apply_cursor(&self.rt, false); }
        self.mods = ModifiersState::empty();
        if let Some(w) = &self.win { w.hide(); }
        let launcher = self.launcher();
        if self.release_gpu && !launcher {
            self.win = None;
            self.native_handle.native.ready.store(false, Ordering::Release);
            self.native_handle.native.number.store(0, Ordering::Release);
            self.gpu = None;
            self.rt.text.atlas.invalidate();
            self.rt.images.invalidate();
        } else if is_wayland(event_loop) {
            // Wayland has no hidden toplevel: unmap by destroying, recreate on open.
            self.win = None;
            self.native_handle.native.ready.store(false, Ordering::Release);
            self.native_handle.native.number.store(0, Ordering::Release);
        } else if let Some(w) = self.win.as_mut() {
            w.stop_pacing();
            w.visible = false;
            w.retry = None;
            w.presented = None;
            // Its hidden frame delivers the focus loss.
            if launcher { w.window.request_redraw(); }
        }
    }

    fn open(&mut self, event_loop: &ActiveEventLoop, token: Option<String>) {
        if self.launcher() { self.reveal_launcher(event_loop); return; }
        // A minimized window drew nothing either, so it comes back as a hidden one does.
        let restoring = self.win.as_ref().is_some_and(|w| w.window.is_minimized() == Some(true));
        let built = self.win.as_ref().is_some_and(|w| !w.visible || restoring) && self.build_for_show();
        self.apply_placement(event_loop);
        // The launcher's startup token belongs to the first window only.
        #[allow(unused_mut)]
        let mut from_env = None;
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if !self.shown {
            use winit::platform::startup_notify::{EventLoopExtStartupNotify, reset_activation_token_env};
            from_env = event_loop.read_token_from_env().map(|t| t.into_raw());
            reset_activation_token_env();
        }
        let token = token.or(from_env);
        let Some(w) = self.win.as_mut() else {
            self.create(event_loop, token, true);
            // On macOS a new window becomes key without activating the app, so activate it
            // like a reopen does. The launch open leaves activation to the platform.
            #[cfg(target_os = "macos")]
            if self.started && let Some(w) = self.win.as_ref().filter(|w| w.visible) { w.focus(); }
            return;
        };
        if !w.visible {
            w.show(false);
            w.visible = true;
            if !built { self.rt.settle_all(); }
            self.idle = true;
        }
        if restoring {
            w.window.set_minimized(false);
            if !built { self.rt.settle_all(); }
        }
        w.occluded = false;
        self.rt.set_live_visible(true);
        self.rt.cancel_timer_frame();
        self.native_handle.native.number.store(native_window_number(&w.window).unwrap_or(0), Ordering::Release);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if is_wayland(event_loop) {
            match token {
                Some(token) => {
                    if w.activator.is_none() {
                        w.activator = crate::activation::Activator::new(&w.window);
                    }
                    if let Some(a) = w.activator.as_mut() {
                        a.activate(&token);
                    }
                }
                // Without a token the compositor may only mark the window urgent.
                None => w.window.request_user_attention(Some(winit::window::UserAttentionType::Informational)),
            }
            w.window.request_redraw();
            return;
        }
        let _ = token;
        w.focus();
        w.window.request_redraw();
    }

    /// Creates the window, or records why it can't be and leaves the app windowless; a later
    /// open tries again.
    fn create(&mut self, event_loop: &ActiveEventLoop, token: Option<String>, focus: bool) {
        let result = self.try_create(event_loop, token, focus);
        if let Err(error) = &result {
            eprintln!("rune: could not create the window {}: {error}", self.native_handle.key());
        }
        *self.shared.window_error.lock().unwrap() = result.err();
    }

    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(unused_variables))]
    fn try_create(&mut self, event_loop: &ActiveEventLoop, token: Option<String>, focus: bool) -> Result<(), String> {
        // A released window reopens at the height its content reached while it was gone.
        let built = self.shown && self.build_for_show();
        let size = self.args.size.unwrap_or(self.spec.size);
        let launcher = self.launcher();
        let visible = !launcher && (self.started || self.options.visible);
        #[allow(unused_mut)]
        let mut attrs = self.options.attributes(self.rt.window_title.current().unwrap_or(self.spec.title), size, self.spec.min_size, visible).with_active(focus);
        #[cfg(target_os = "macos")]
        if self.macos_window.nonactivating_panel { attrs = attrs.with_visible(false).with_active(false); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            if let Some(id) = crate::tooling::app_id() {
                attrs = winit::platform::wayland::WindowAttributesExtWayland::with_name(attrs, id, id);
                attrs = winit::platform::x11::WindowAttributesExtX11::with_name(attrs, id, id);
            }
            // Tiling window managers float a toplevel that maps at a fixed size;
            // `first_frame_ready` restores the window's own limits.
            if self.float_on_open {
                let fixed: winit::dpi::Size = match self.options.physical_size {
                    Some((w, h)) => winit::dpi::PhysicalSize::new(w, h).into(),
                    None => winit::dpi::LogicalSize::new(size.0, size.1).into(),
                };
                attrs = attrs.with_min_inner_size(fixed).with_max_inner_size(fixed);
            }
            if let Some(token) = token {
                let token = winit::window::ActivationToken::from_raw(token);
                attrs = winit::platform::startup_notify::WindowAttributesExtStartupNotify::with_activation_token(attrs, token);
            }
        }
        let chrome = self.chrome;
        #[cfg(target_os = "macos")]
        if !self.options.borderless && chrome.full_size_content {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs.with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true);
        }
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        #[cfg(target_os = "macos")]
        crate::native_menu::adopt_standard_view();
        #[cfg(target_os = "macos")]
        let panel = if self.macos_window.nonactivating_panel {
            Some(crate::macos::Panel::adopt_window(&window, size, &self.macos_window).ok_or("could not create nonactivating panel")?)
        } else { None };
        #[cfg(target_os = "macos")]
        crate::macos::configure_window(&window, &self.macos_window);
        #[cfg(target_os = "macos")]
        crate::macos_drop::install(&window);
        if self.options.click_through && let Err(error) = window.set_cursor_hittest(false) {
            eprintln!("click-through unavailable for {}: {error}", self.native_handle.key());
        }
        window.set_ime_allowed(false);
        self.rt.ime_reset = true;
        #[cfg(target_os = "macos")]
        if uses_drag_regions(chrome, self.options.borderless, false) {
            mac::own_window_dragging(&window);
        }
        self.rt.set_titlebar_inset(place_chrome(&window, chrome));
        #[cfg(target_os = "macos")]
        {
            self.rt.double_click_interval = mac::double_click_interval();
            self.rt.set_native_help(true);
        }
        #[cfg(windows)]
        {
            let ms = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
            self.rt.double_click_interval = f64::from(ms) / 1000.0;
            // A tooltip control's initial delay is the double-click time.
            self.rt.help_delay = self.rt.double_click_interval;
        }
        let mut prewarmed = None;
        let (gpu, surface) = match self.gpu.take() {
            Some((gpu, renderer, format)) => {
                let surface = gpu.instance.create_surface(window.clone());
                self.gpu = Some((gpu, renderer, format));
                (None, surface.map_err(|e| e.to_string())?)
            }
            None => {
                let features = self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
                let transparent = self.options.transparent;
                // A shared device that can't composite this window's translucent pixels gets its own.
                let cached = self.gpu_pool.borrow().upgrade().and_then(|gpu| {
                    let surface = gpu.instance.create_surface(window.clone()).ok()?;
                    crate::translucency::serves(&gpu, &surface, features, transparent).then_some((gpu, surface))
                });
                let (gpu, surface) = if let Some(cached) = cached {
                    cached
                } else {
                    let (gpu, surface, renderer) = Gpu::for_surface(features, transparent, |instance| instance.create_surface(window.clone()).map_err(|e| e.to_string()))?;
                    prewarmed = renderer;
                    let gpu = Arc::new(gpu);
                    *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                    (gpu, surface)
                };
                (Some(gpu), surface)
            }
        };
        let adapter = &gpu.as_ref().or(self.gpu.as_ref().map(|g| &g.0)).unwrap().adapter;
        let phys = window.inner_size();
        let (mut config, translucent) = surface_config(&surface, adapter, (phys.width, phys.height), self.rt.srgb_blend())
            .ok_or("the GPU adapter can't present to this window")?;
        #[cfg(target_os = "macos")]
        let launch_drawables = LaunchDrawables::cap(&mut config);
        if self.options.transparent {
            config.alpha_mode = translucent.unwrap_or_else(|| {
                eprintln!("transparency unavailable for {}; using an opaque window", self.native_handle.key());
                wgpu::CompositeAlphaMode::Opaque
            });
            self.rt.transparent = config.alpha_mode != wgpu::CompositeAlphaMode::Opaque;
        }
        let target_format = if self.rt.srgb_blend() { config.format.remove_srgb_suffix() } else { config.format };
        if let Some(gpu) = gpu {
            let mut renderer = prewarmed.unwrap_or_else(|| Renderer::new(&gpu.device, target_format));
            renderer.set_srgb_blend(self.rt.srgb_blend());
            self.gpu = Some((gpu, renderer, target_format));
        }
        let (gpu, renderer, format) = self.gpu.as_mut().unwrap();
        rebuild_renderer(&gpu.device, renderer, target_format, &mut self.rt);
        *format = target_format;
        surface.configure(&gpu.device, &config);
        #[cfg(target_os = "macos")]
        crate::macos::pin_contents_top_left(&surface);
        #[cfg(target_os = "macos")]
        crate::macos_window_shape::apply_corners(&window, &self.macos_window, self.options.transparent, self.options.borderless);
        // Metal's PostMultiplied mode only makes CAMetalLayer non-opaque; Core Animation
        // still composites premultiplied pixels. Other backends follow wgpu's straight-alpha contract.
        let alpha = (self.options.transparent && config.alpha_mode == wgpu::CompositeAlphaMode::PostMultiplied
            && gpu.adapter.get_info().backend != wgpu::Backend::Metal)
            .then(|| crate::surface_alpha::AlphaSurface::new(&gpu.device, target_format, (config.width, config.height)));
        let scale = window.scale_factor() as f32;
        self.rt.set_system_dark(crate::preferences::window_dark(&window, self.args.theme.as_deref()));
        self.rt.set_viewport((phys.width as f32 / scale, phys.height as f32 / scale), scale);
        if (self.shown || launcher) && !built {
            // Reopening paints the settled state in the first frame (guarantee 8).
            self.rt.settle_all();
        }
        self.shown = true;
        self.idle = true;
        self.rt.set_live_visible(visible);
        *self.native_handle.native.window.lock().unwrap() = Arc::downgrade(&window);
        self.native_handle.native.number.store(native_window_number(&window).unwrap_or(0), Ordering::Release);
        #[cfg(target_os = "macos")]
        let resamples = crate::macos_window_shape::resamples(&window, self.options.transparent);
        #[cfg(target_os = "macos")]
        let hook_log = self.present_log.clone()
            .or_else(|| resamples.then(|| Arc::new(crate::present_time::metal::Log::untraced())));
        #[cfg(target_os = "macos")]
        let present = hook_log.clone().and_then(|log| crate::present_time::metal::LayerHook::attach(&surface, log));
        #[cfg(target_os = "macos")]
        let shadow = hook_log.filter(|_| resamples && present.is_some())
            .and_then(|log| crate::macos_window_shape::Shadow::new(&window, log, visible || launcher));
        #[cfg(target_os = "macos")]
        if self.present_log.is_some() && present.is_none() { eprintln!("rune: trace: this surface reports no present times"); }
        #[cfg(target_os = "macos")]
        let fn_key = if self.rt.modifier_keys() { crate::fn_key::FnKey::new(&window) } else { None };
        #[cfg(target_os = "macos")]
        let full_screen = mac::FullScreenWatch::new(&window);
        #[cfg(target_os = "macos")]
        let prime = if visible && panel.is_none() { 2 } else { 0 };
        self.win = Some(Win {
            first_frame: FirstFrame::default(),
            #[cfg(target_os = "macos")]
            pacing: crate::display_link::DisplayLink::new(&window, self.frame_wake.clone()),
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            activator: None,
            #[cfg(target_os = "macos")]
            present,
            #[cfg(target_os = "macos")]
            shadow,
            surface,
            config,
            #[cfg(target_os = "macos")]
            panel,
            #[cfg(target_os = "macos")]
            materials: crate::system_material::macos::Materials::new(),
            window,
            visible,
            occluded: false,
            retry: None,
            occluded_wait: OCCLUDED_RETRY,
            hidden_wait: TIMEOUT_RETRY,
            hidden_pace: HiddenPace::default(),
            cursor: crate::os_cursor::OsCursor::new(event_loop),
            alpha,
            #[cfg(target_os = "macos")]
            help: Default::default(),
            #[cfg(target_os = "macos")]
            full_screen,
            #[cfg(target_os = "macos")]
            fn_key,
            #[cfg(target_os = "macos")]
            prime,
            #[cfg(target_os = "macos")]
            launch_drawables,
            quiet: false,
            presented: None,
        });
        if launcher { self.win.as_mut().unwrap().first_frame.prepare(); }
        self.apply_placement(event_loop);
        #[cfg(target_os = "macos")]
        if visible && let Some(w) = &self.win && w.panel.is_some() { w.show(focus); }
        if let Some(created) = self.on_created.take() { created(self.native_handle.clone()); }
        if let Some(on_window) = self.on_window.as_mut() { on_window(&self.native_handle); }
        if let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) {
            #[cfg(target_os = "macos")]
            if let Some(pacing) = &mut w.pacing { pacing.update_display(&w.window); }
            let size = w.window.inner_size();
            let scale = w.window.scale_factor() as f32;
            if size.width > 0 && size.height > 0 {
                w.resize_surface(&gpu.device, size);
                self.rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
            }
            self.rt.set_titlebar_inset(place_chrome(&w.window, self.chrome));
            if let Some(origin) = screen_origin(&w.window) {
                self.rt.set_window_origin(origin);
            }
            if w.window.has_focus() {
                self.rt.window_focus(true);
            }
        }
        // The first frame, now, so the window comes on screen with it instead of empty for a
        // frame or two. AppKit has ordered the window in, but it reaches the screen with this
        // turn's Core Animation commit, so nothing reports it visible yet.
        let mut drew = false;
        #[cfg(target_os = "macos")]
        if visible && !self.rt.minimize_request && !self.finished && !event_loop.exiting()
            && self.win.as_ref().is_some_and(|w| w.panel.is_none() && w.window.is_minimized() != Some(true))
        {
            let presents = self.meter.totals().presents;
            self.redraw();
            drew = self.meter.totals().presents > presents;
            self.shell(event_loop);
        }
        if !drew && (visible || launcher) && let Some(w) = &self.win {
            w.window.request_redraw();
        }
        Ok(())
    }

    /// Tells the runtime where the window's content is on screen, drawing a frame when that
    /// moves an `El::on_screen_layout` element.
    fn sync_origin(&mut self) {
        let Some(w) = &self.win else { return };
        if let Some(origin) = screen_origin(&w.window) && self.rt.set_window_origin(origin) {
            w.window.request_redraw();
        }
    }

    /// Publishes what `AppHandle::is_visible` reports.
    fn publish(&self) {
        let visible = !self.finished && self.win.as_ref().is_some_and(|w| w.visible);
        self.shared.visible.store(visible, Ordering::Release);
    }
}

impl<S: 'static> AnyHost for Host<S> {
    fn owns(&self, id: WindowId) -> bool {
        self.win.as_ref().is_some_and(|w| w.window.id() == id)
    }
    fn finished(&self) -> bool { self.finished }
    fn keeps_loop(&self) -> bool { self.keeps_loop }
    fn terminate(&mut self) {
        // Once: a window that already closed ran its hook then.
        if self.finished || !self.closes() { return; }
        self.finished = true;
        self.close_hook();
        self.popup.cancel(&mut self.rt);
        self.file_dialog.cancel(&mut self.rt);
    }

    fn cancel_menus(&mut self) {
        self.popup.cancel(&mut self.rt);
        self.file_dialog.cancel(&mut self.rt);
    }

    fn reopen_candidate(&self) -> crate::reopen::Candidate {
        let shown = self.win.as_ref().filter(|w| w.visible);
        let minimized = shown.is_some_and(|w| w.window.is_minimized() == Some(true));
        crate::reopen::Candidate {
            reopenable: !self.finished && !self.macos_window.nonactivating_panel,
            visible: shown.is_some() && !minimized,
            minimized,
            hidden_at: self.hidden_at,
            main: self.keeps_loop,
        }
    }

    fn reopen_hook(&mut self, event_loop: &ActiveEventLoop, has_visible_windows: bool) -> bool {
        let Some(f) = self.on_reopen.filter(|_| self.started && !self.finished) else { return false };
        // As an update, so its frame, wakes, window requests and quit go as an update's do.
        let handled = Arc::new(AtomicBool::new(false));
        let answer = handled.clone();
        self.shared.queue_update(Box::new(move |s, tx| { answer.store(f(s, has_visible_windows, tx), Ordering::Relaxed); true }));
        self.drain(event_loop, false);
        self.publish();
        handled.load(Ordering::Relaxed)
    }

    fn apply_pending(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started || self.finished || !self.shared.window_pending() { return; }
        self.drain(event_loop, false);
        self.publish();
    }

    fn reopen(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started || self.finished { return; }
        self.open(event_loop, None);
        self.shell(event_loop);
        // winit's focus does nothing while the window is still deminiaturizing.
        #[cfg(target_os = "macos")]
        if let Some(w) = self.win.as_ref().filter(|w| w.visible) { crate::reopen::front(&w.window); }
        self.publish();
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        #[cfg(target_os = "macos")]
        if let Some(shadow) = self.win.as_ref().and_then(|w| w.shadow.as_ref()) { shadow.turn(); }
        self.meter.turn(matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll), crate::surface_stats::slept(cause));
        if matches!(cause, StartCause::ResumeTimeReached { .. }) && let Some(w) = &self.win { w.cursor.reassert(); }
        if let Some(w) = &mut self.win && let Some((retry, until)) = w.first_frame.deadline && retry <= Instant::now() {
            if until <= Instant::now() {
                let notified = w.first_frame.notified;
                if w.first_frame.expire() {
                    self.show_cached();
                    self.shell(event_loop);
                    if let Some(w) = &self.win { w.window.request_redraw(); }
                    self.publish();
                } else if !notified {
                    *self.shared.window_error.lock().unwrap() = Some("launcher first frame was not ready within two seconds".into());
                }
            } else {
                // Bounded retries while the frame prepares, whatever stops each one (occlusion,
                // no drawable). A retry left due would turn the loop at once until it gives up.
                w.first_frame.deadline = Some((Instant::now() + Duration::from_millis(16), until));
                w.window.request_redraw();
            }
        }
        let launcher = self.launcher();
        if let Some(w) = &mut self.win && w.retry.is_some_and(|retry| retry <= Instant::now()) {
            w.retry = None;
            // A missed frame marked it occluded; look at the surface again.
            w.occluded = false;
            if launcher || w.visible { w.window.request_redraw(); }
        }
        if let Some(w) = &mut self.win && !w.visible && w.hidden_pace.take_due(Instant::now()) {
            w.window.request_redraw();
        }
        if matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll)
            && timer_deadline(&self.rt).is_some_and(|deadline| deadline <= Instant::now())
        {
            let hidden = self.win.as_ref().is_none_or(|w| !w.visible);
            if hidden && !(launcher && self.hidden_launcher_renders()) {
                // Hidden or windowless: poll and due timers run, and the view waits for the
                // frame that shows the window. A launcher's cached frame follows, paced.
                let changed = self.rt.hidden_wake();
                self.clipboard.flush(&mut self.rt);
                self.shell(event_loop);
                if changed && launcher { self.refresh_hidden(); }
            // A redraw while the window backs off would draw nothing and leave the timer due.
            } else if self.win.as_ref().is_some_and(|w| (launcher || (w.visible && !w.occluded)) && !w.minimized()
                && !w.backing_off(Instant::now())) {
                // A resting window polls first and draws only for what that changed.
                let rests = self.idle && !composing(self.comp.as_deref()) && self.win.as_ref().is_some_and(Win::rests);
                let frame = match rests.then(|| self.rt.resting_wake()).flatten() {
                    Some(stale) => {
                        self.clipboard.flush(&mut self.rt);
                        self.shell(event_loop);
                        // A close the poll asked for may have hidden the window.
                        let frame = (stale || self.rt.stale()) && self.win.as_ref().is_some_and(|w| w.visible);
                        self.rt.resting_wake_frame(frame);
                        frame
                    }
                    None => true,
                };
                if frame && let Some(w) = &mut self.win {
                    w.quiet = true;
                    w.window.request_redraw();
                }
            } else {
                covered_step(&mut self.rt);
                self.clipboard.flush(&mut self.rt);
                self.shell(event_loop);
            }
        }
    }

    fn wake_deadline(&self) -> Option<Instant> {
        // The next display tick consumes due timers too. Leaving an overdue timer
        // armed while its redraw waits for that tick would spin the event loop. Without a
        // running link, a timer due sooner than a frame waits a frame (`timer_deadline`).
        let runtime = timer_deadline(&self.rt).filter(|_| !self.win.as_ref().is_some_and(Win::paced));
        let win = self.win.as_ref();
        runtime.into_iter()
            .chain(win.and_then(|w| w.first_frame.deadline.map(|(retry, _)| retry)))
            .chain(win.and_then(|w| w.retry))
            .chain(win.filter(|w| !w.visible).and_then(|w| w.hidden_pace.deadline()))
            .chain(self.files.pointer.as_ref().filter(|_| win.is_some_and(|w| w.visible)).map(|_| Instant::now() + DROP_SAMPLE))
            .chain(win.filter(|w| w.visible).and_then(|w| w.cursor.reassert_at(Instant::now())))
            .min()
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) -> bool {
        #[cfg(target_os = "macos")]
        self.fn_key();
        self.flush_files(event_loop);
        // Process queued hide/close requests before completing a pending first reveal.
        self.first_frame_ready(event_loop);
        if !self.win.as_ref().is_some_and(|w| w.visible || w.first_frame.reveal) {
            self.rt.refuse_file_dialog("cannot open a file dialog while the window is hidden");
        }
        self.hidden_input(event_loop);
        false
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.resume(event_loop);
        self.publish();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        self.meter.waited();
        self.handle_user_event(event_loop, event);
        self.publish();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if !matches!(event, WindowEvent::RedrawRequested) { self.meter.waited(); }
        self.handle_window_event(event_loop, id, event);
        self.publish();
    }
}

impl<S: 'static> Host<S> {
    fn resume(&mut self, event_loop: &ActiveEventLoop) {
        if self.started {
            return;
        }
        // Updates queued before launch (the link that launched the app) show in the first frame.
        for f in self.shared.take_updates() { self.rt.update_if(f); self.menus.drain(); }
        if self.launcher() {
            self.create(event_loop, None, false);
            if self.open_at_start && self.options.visible { self.reveal_launcher(event_loop); }
        } else if self.open_at_start {
            self.open(event_loop, None);
        }
        self.started = true;
        self.drain(event_loop, false);
    }

    /// A hidden window delivers queued input without drawing, such as the releases of the
    /// modifier keys held when it hid, rather than when it next shows. A shown window's
    /// redraw delivers it, as does a hidden launcher's.
    fn hidden_input(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started || self.finished || self.win.as_ref().is_some_and(|w| w.visible || self.launcher()) { return; }
        if !self.rt.keys_queued() { return; }
        self.rt.hidden_timer_frame();
        self.clipboard.flush(&mut self.rt);
        self.shell(event_loop);
    }

    #[cfg(target_os = "macos")]
    fn fn_key(&mut self) {
        let Some(l) = &self.win else { return };
        let Some(changes) = l.fn_key.as_ref().map(|f| f.take()) else { return };
        let mut redraw = false;
        for pressed in changes {
            redraw |= self.rt.physical_with_fallback(crate::PhysicalInput::Key {
                key: winit::keyboard::KeyCode::Fn, mods: map_mods(self.mods), pressed, repeat: false,
            }, None);
        }
        if redraw { l.window.request_redraw(); }
    }

    /// Hands the runtime the file drag winit reported since the last pass, in order: the files
    /// that entered, as one drag, and those dropped, as one drop, each at a fresh pointer
    /// sample; and, while the drag is over the window, a new sample, which redraws only when it
    /// changes the drop target. Runs even while the window is covered, as queued input does.
    fn flush_files(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(target_os = "macos")]
        let appkit = self.win.as_ref().map(|l| crate::macos_drop::take(&l.window)).unwrap_or_default();
        let Some(l) = self.win.as_ref().filter(|w| w.visible) else {
            self.files = FileDrop::default();
            return;
        };
        let f = &mut self.files;
        let mut queued = false;
        // AppKit's drags say where they are at every step (`macos_drop`), so nothing samples.
        #[cfg(target_os = "macos")]
        for step in appkit {
            use crate::macos_drop::Step;
            queued |= match step {
                Step::Enter(paths, at) => { self.rt.drag_files(paths, Some(at)); true }
                Step::Move(at) => self.rt.drag_files_to(Some(at)),
                Step::Drop(paths, at) => { self.rt.drop_files(paths, Some(at)); true }
                Step::Leave => { self.rt.drag_files_leave(); true }
            };
        }
        for step in std::mem::take(&mut f.steps) {
            queued = true;
            match step {
                FileStep::Enter(paths) => {
                    let pointer = crate::drop_pointer::DropPointer::new();
                    f.at = pointer.sample(&l.window).or_else(|| self.rt.pointer());
                    f.pointer = Some(pointer);
                    self.rt.drag_files(paths, f.at);
                }
                FileStep::Drop(paths) => {
                    // A drop with no drag before it (Windows' WM_DROPFILES) samples too.
                    let pointer = f.pointer.take().unwrap_or_else(crate::drop_pointer::DropPointer::new);
                    let at = pointer.sample(&l.window).or(f.at.take()).or_else(|| self.rt.pointer());
                    self.rt.drop_files(paths, at);
                }
                FileStep::Leave => {
                    f.pointer = None;
                    f.at = None;
                    self.rt.drag_files_leave();
                }
            }
        }
        // Sampled even when still, which also notices a target that unmounted under the drag.
        if !queued && let Some(at) = f.pointer.as_ref().and_then(|p| p.sample(&l.window)) {
            f.at = Some(at);
            queued = self.rt.drag_files_to(Some(at));
        }
        if !queued { return; }
        if !l.occluded && !l.minimized() { l.window.request_redraw(); } else {
            self.covered_frame();
            self.shell(event_loop);
        }
    }

    fn handle_user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        if !self.started { return; }
        // The display tick a timer's frame waits for draws that frame.
        if !matches!(event, WakeEvent::DisplayTick(_)) && let Some(w) = &mut self.win { w.quiet = false; }
        match event {
            WakeEvent::App(_) => {
                self.drain(event_loop, true);
            }
            WakeEvent::DisplayTick(_) => {
                if self.win.as_ref().is_some_and(Win::paced) {
                    self.redraw();
                    self.shell(event_loop);
                }
            }
            WakeEvent::Live(_) => {
                if self.rt.live_pending() && let Some(w) = self.win.as_ref().filter(|w| w.visible && !w.occluded) {
                    w.window.request_redraw();
                }
            }
            WakeEvent::Window(_, request) => {
                self.window_request(event_loop, request);
                self.shell(event_loop);
            }
            WakeEvent::Focus(_) => self.focus(event_loop),
            WakeEvent::Position(_, position) => self.set_position(event_loop, position),
            WakeEvent::Place(_, place) => self.place(event_loop, place),
            WakeEvent::Level(_, on) => self.set_always_on_top(event_loop, on),
            WakeEvent::Frame(_, mut request) => {
                request.spring = (request.animate && !self.rt.reduced_motion).then_some(crate::tok::motion::DEFAULT);
                self.rt.window_frame = Some(self.rt.window_frame.unwrap_or_default().merge(request));
                if let Some(w) = self.win.as_ref().filter(|w| w.visible) { w.window.request_redraw(); }
                self.shell(event_loop);
            }
            WakeEvent::Reopen => {}
        }
    }

    fn handle_window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.win.as_ref().is_none_or(|w| w.window.id() != id) {
            return;
        }
        if !matches!(event, WindowEvent::RedrawRequested) && let Some(w) = &mut self.win { w.quiet = false; }
        #[cfg(target_os = "macos")]
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Occluded(_))
            && let Some(w) = &mut self.win && let Some(pacing) = &mut w.pacing {
            pacing.update_display(&w.window);
        }
        if matches!(&event, WindowEvent::ScaleFactorChanged { .. }) { self.apply_placement(event_loop); }
        match event {
            WindowEvent::Focused(focused) => {
                if self.launcher() && self.win.as_ref().is_some_and(|w| !w.first_frame.notified) { return; }
                #[cfg(target_os = "macos")]
                if self.win.as_ref().and_then(|w| w.panel.as_ref()).is_some_and(|p| p.is_key() != focused) {
                    // winit queues an initial false from its hidden source window before
                    // adoption. It must not dismiss an already-key panel on first open.
                    return;
                }
                #[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
                if let Some(w) = &self.win { crate::file_dialog::window_focus(&w.window, focused && w.visible); }
                // Deliver independently of drawable availability, including hide/occlusion.
                self.rt.window_focus(focused);
                self.shell(event_loop);
                if self.finished || self.win.is_none() { return; }
            }
            WindowEvent::CloseRequested => {
                self.close(event_loop);
                return;
            }
            WindowEvent::HoveredFile(path) => { self.files.file(path, false); return; }
            WindowEvent::DroppedFile(path) => { self.files.file(path, true); return; }
            WindowEvent::HoveredFileCancelled => { self.files.steps.push(FileStep::Leave); return; }
            WindowEvent::RedrawRequested => {
                // A new accent or light/dark preference asks every window to redraw (`accent::watch`, `window_dark`).
                self.rt.set_system_accent(crate::system_accent());
                if self.args.theme.is_none() && let Some(dark) = self.win.as_ref().and_then(|l| crate::preferences::desktop_dark(&l.window)) {
                    self.rt.set_system_dark(dark);
                }
                #[cfg(target_os = "macos")]
                if let Some(l) = self.win.as_ref().filter(|l| l.full_screen.take()) {
                    self.rt.set_titlebar_inset(place_chrome(&l.window, self.chrome));
                }
                self.redraw();
                self.shell(event_loop);
                return;
            }
            _ => {}
        }
        // Occluded or minimised mid-move: land, so the window comes back at its new size.
        let stops = match &event {
            WindowEvent::Occluded(occluded) => *occluded && !(self.macos_window.launcher && cfg!(target_os = "macos")),
            WindowEvent::Resized(size) => size.width == 0 || size.height == 0,
            _ => false,
        };
        if stops { self.land_frame(); }
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. }) { self.sync_origin(); }
        let (Some(l), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) else { return };
        let rt = &mut self.rt;
        let scale = l.window.scale_factor() as f32;
        let chrome = self.chrome;
        match event {
            WindowEvent::Moved(_) => {
                // winit queues `ScaleFactorChanged` in `kCFRunLoopDefaultMode`, which waits
                // until a window drag ends; resize the surface now when a drag crosses displays
                // so top-left gravity never shows the old scale's drawable at half or double size.
                if (scale - rt.env.scale).abs() > f32::EPSILON {
                    let size = l.window.inner_size();
                    if size.width > 0 && size.height > 0 {
                        l.resize_surface(&gpu.device, size);
                        rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
                        rt.set_titlebar_inset(place_chrome(&l.window, chrome));
                        l.window.request_redraw();
                    }
                }
            }
            WindowEvent::Resized(size) => {
                rt.set_titlebar_inset(place_chrome(&l.window, chrome));
                let was_live = rt.live_visible();
                rt.set_live_visible(l.visible && !l.occluded && size.width > 0 && size.height > 0 && !l.minimized());
                // Restored where no occlusion event says so (Windows): as uncovering does, the
                // first frame paints the settled state of what changed meanwhile.
                let restored = !was_live && rt.live_visible();
                if restored { rt.settle_all(); }
                if size.width == 0 || size.height == 0 {
                    self.secure_input.set(false);
                    l.stop_pacing();
                    l.presented = None;
                    rt.cancel_timer_frame();
                    self.idle = true; return;
                }
                let origin_moved = screen_origin(&l.window).is_some_and(|o| rt.set_window_origin(o));
                // A frame Rune moved itself has its drawable and viewport already.
                let resized = l.resize_surface(&gpu.device, size);
                let viewport = (size.width as f32 / scale, size.height as f32 / scale);
                if !resized && viewport == rt.viewport && scale == rt.env.scale {
                    if restored || origin_moved { l.window.request_redraw(); }
                    return;
                }
                let asked = self.frame_jump.take().is_some_and(|(w, h)| (w - viewport.0).abs() < 1.0 && (h - viewport.1).abs() < 1.0);
                if asked { rt.step_viewport(viewport, scale) } else { rt.set_viewport(viewport, scale) }
                // The user is dragging the edge. AppKit commits the new size when this returns,
                // and a frame drawn later would leave the old one stretched to it until then.
                #[cfg(target_os = "macos")]
                if crate::macos::in_live_resize(&l.window) {
                    if self.frame_anim.take().is_some() { l.present_with_transaction(false); }
                    self.draw(true);
                    self.shell(event_loop);
                    return;
                }
                l.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                #[cfg(target_os = "macos")]
                if let Some(shadow) = &l.shadow { shadow.redraw(); }
                let scale = scale_factor as f32;
                let size = l.window.inner_size();
                if size.width > 0 && size.height > 0 {
                    let resized = l.resize_surface(&gpu.device, size);
                    let viewport = (size.width as f32 / scale, size.height as f32 / scale);
                    if resized || viewport != rt.viewport || scale != rt.env.scale {
                        rt.set_viewport(viewport, scale);
                        rt.set_titlebar_inset(place_chrome(&l.window, chrome));
                        l.window.request_redraw();
                    }
                }
            }
            WindowEvent::Occluded(occluded) => {
                // Alpha-zero launcher panels remain renderable on explicit invalidation.
                // Their native occlusion transitions must not settle or redraw a reveal.
                if self.macos_window.launcher && cfg!(target_os = "macos") { return; }
                #[cfg(target_os = "macos")]
                { l.prime = 0; }
                let was_occluded = std::mem::replace(&mut l.occluded, occluded);
                l.retry = None;
                l.presented = None;
                l.occluded_wait = OCCLUDED_RETRY;
                rt.set_live_visible(l.visible && !occluded && !l.minimized());
                if occluded {
                    self.secure_input.set(false);
                    l.stop_pacing();
                    rt.cancel_timer_frame();
                    self.idle = true; return;
                }
                rt.set_titlebar_inset(place_chrome(&l.window, chrome));
                // A window opened while the displays slept has no display link yet.
                #[cfg(target_os = "macos")]
                if l.pacing.is_none() { l.pacing = crate::display_link::DisplayLink::new(&l.window, self.frame_wake.clone()); }
                if was_occluded {
                    // Coming back paints the settled state in the first frame (guarantee 8).
                    rt.settle_all();
                    l.window.request_redraw();
                } else if rt.stale() {
                    l.window.request_redraw();
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                if self.args.theme.is_none() {
                    rt.set_system_dark(theme == Theme::Dark);
                    l.window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                if rt.physical_observation(crate::PhysicalInput::Modifiers(map_mods(self.mods))) {
                    l.window.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                rt.set_system_reduced_motion(crate::preferences::reduced_motion());
                rt.set_full_keyboard_access(crate::preferences::full_keyboard_access());
                self.secure_input.set(focused && l.visible && !l.occluded && rt.has_secure_focus());
                rt.set_titlebar_inset(place_chrome(&l.window, chrome));
                if !focused {
                    rt.cancel_composition();
                    l.window.set_ime_allowed(false);
                    self.ime_allowed = false;
                    // A minimized or occluded window skips redraw.
                    l.apply_cursor(rt, false);
                } else if rt.stale() {
                    rt.ime_reset = true;
                } else {
                    let ime_allowed = rt.ime_area.is_some() && !rt.has_secure_focus();
                    if std::mem::take(&mut rt.ime_reset) || ime_allowed != self.ime_allowed {
                        l.window.set_ime_allowed(false);
                        l.window.set_ime_allowed(ime_allowed);
                        self.ime_allowed = ime_allowed;
                    }
                    if let Some(a) = rt.ime_area {
                        l.window.set_ime_cursor_area(
                            LogicalPosition::new(a[0] as f64, a[1] as f64),
                            LogicalSize::new(a[2] as f64, a[3] as f64),
                        );
                    }
                    l.apply_cursor(rt, true);
                }
                if rt.stale() { l.window.request_redraw(); }
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                let pressed = event.state == ElementState::Pressed;
                // A release's logical key only matters to `on_key_up` handlers.
                let logical = (pressed || rt.wants_key_up()).then(|| map_key(&event.logical_key, self.mods)).flatten();
                let mut redraw = false;
                // A standard ⌘W or ⌘M menu item handing Rune its key: close or minimize after,
                // if no handler takes it.
                #[cfg(target_os = "macos")]
                let unhandled = logical.as_ref().filter(|_| pressed).and_then(crate::native_menu::take_unhandled);
                #[cfg(not(target_os = "macos"))]
                let unhandled = None;
                if let winit::keyboard::PhysicalKey::Code(key) = event.physical_key {
                    redraw = rt.physical_then(crate::PhysicalInput::Key {
                        key, mods: map_mods(self.mods), pressed, repeat: event.repeat,
                    }, logical, key_base(&event), unhandled);
                } else if let Some(ev) = logical {
                    if !pressed { redraw = rt.key_up(ev); }
                    else if ev.key == Key::Command('v') && rt.accepts_paste() { self.clipboard.read(rt); }
                    else if event.repeat { rt.repeat_key(ev); }
                    else { rt.key(ev); }
                    redraw |= pressed;
                }
                if redraw { l.window.request_redraw(); }
            }
            WindowEvent::Ime(ime) => {
                match ime {
                    WIme::Preedit(s, cur) => rt.ime(Ime::Preedit(s, cur)),
                    WIme::Commit(s) => rt.ime(Ime::Commit(s)),
                    WIme::Enabled => rt.ime(Ime::Enabled),
                    WIme::Disabled => rt.ime(Ime::Disabled),
                }
                l.window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let shift = crate::runtime::SHIFT_TURNS_WHEEL && self.mods.shift_key();
                let (dx, dy) = crate::runtime::wheel_delta(delta, scale, shift);
                let precise = matches!(delta, winit::event::MouseScrollDelta::PixelDelta(_));
                let phase = gesture_phase(phase);
                match rt.pointer() {
                    // Off macOS a trackpad pinch is a Ctrl + wheel.
                    Some((x, y)) if !cfg!(target_os = "macos") && self.mods.control_key() && rt.pinch_wheel(x, y, dy) => {}
                    Some((x, y)) => rt.wheel_event(crate::WheelEvent::new(dx, dy).at(x, y).mods(map_mods(self.mods))
                        .phase(phase).precise(precise)),
                    None if phase == crate::GesturePhase::Began => rt.wheel_began(),
                    None if phase == crate::GesturePhase::Ended => rt.wheel_ended(),
                    None => {}
                }
                l.window.request_redraw();
            }
            WindowEvent::PinchGesture { delta, phase, .. } => {
                if let Some((x, y)) = rt.pointer() {
                    rt.magnify_event(crate::MagnifyEvent::new(delta as f32).at(x, y).phase(gesture_phase(phase)));
                    l.window.request_redraw();
                }
            }
            WindowEvent::RotationGesture { delta, phase, .. } => {
                if let Some((x, y)) = rt.pointer() {
                    rt.rotate_event(crate::RotateEvent::new(delta).at(x, y).phase(gesture_phase(phase)));
                    l.window.request_redraw();
                }
            }
            WindowEvent::DoubleTapGesture { .. } => {
                if let Some((x, y)) = rt.pointer() {
                    rt.smart_magnify(x, y);
                    l.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                rt.pointer_move(position.x as f32 / scale, position.y as f32 / scale);
                // AppKit needs the drag session started inside this mouse-dragged event.
                if let Some(start) = rt.take_drag_out() {
                    let image = self.gpu.as_ref().filter(|_| start.item.drag_image().is_none()).and_then(|(gpu, renderer, _)|
                        crate::drag_out::snapshot(gpu, renderer, (l.config.width, l.config.height), scale, rt.clear_color(), start.rect));
                    let (sink, path) = (rt.drag_end_sink(), start.path.clone());
                    let handle = self.native_handle.clone();
                    crate::drag_out::begin(&l.window, start, image, Box::new(move |outcome| {
                        sink.borrow_mut().push((path, outcome));
                        handle.send(WakeEvent::App);
                    }));
                    l.apply_cursor(rt, false);
                }
                l.window.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                rt.pointer_leave();
                l.window.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if state == ElementState::Pressed {
                    self.control_click = cfg!(target_os = "macos") && self.mods.control_key();
                }
                if let Some((x, y)) = rt.pointer() {
                    match (state, self.control_click) {
                        (ElementState::Pressed, true) => rt.secondary_down(x, y),
                        (ElementState::Released, true) => rt.secondary_up(x, y),
                        (ElementState::Pressed, false) => {
                            rt.pointer_down(x, y);
                            rt.physical_input(crate::PhysicalInput::Modifiers(map_mods(self.mods)));
                            // AppKit needs the drag started inside this mouse-down.
                            match rt.take_titlebar_action() {
                                Some(TitlebarAction::Drag) => {
                                    let _ = l.window.drag_window();
                                }
                                Some(TitlebarAction::DoubleClick) => titlebar_double_click(&l.window),
                                None => {}
                            }
                        }
                        (ElementState::Released, false) => rt.pointer_up(x, y),
                    }
                }
                if state == ElementState::Released { self.control_click = false; }
                l.window.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Right, .. } => {
                if let Some((x, y)) = rt.pointer() {
                    match state {
                        ElementState::Pressed => rt.secondary_down(x, y),
                        ElementState::Released => rt.secondary_up(x, y),
                    }
                }
                l.window.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some((x, y)) = rt.pointer() {
                    match state {
                        ElementState::Pressed => rt.button_down(x, y, pointer_button(button)),
                        ElementState::Released => rt.button_up(x, y, pointer_button(button)),
                    }
                }
                l.window.request_redraw();
            }
            _ => {}
        }
    }
}
