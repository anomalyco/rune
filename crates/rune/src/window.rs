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
use crate::frame::clock::FrameClock;
use crate::edit::{Ime, Key};
use crate::el::{Shell, Tx};
use crate::render::{Gpu, Renderer};
use crate::runtime::{Env, Runtime, TitlebarAction, Waker};
use crate::frame::surface_stats::{Pacing, SurfaceMeter, SurfaceStats};
use crate::tooling::{AppSpec, Args, Chrome, runtime_for};
use crate::windows::{AnyWindowSpec, Display, WindowOptions, WindowSpec};

#[cfg(target_os = "macos")]
mod launch_drawables;
#[cfg(target_os = "macos")]
use launch_drawables::LaunchDrawables;
mod native;
use native::Native;
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
mod any_host;
mod event_loop;
mod events;
mod file_drop;
pub(crate) mod frame;
mod handle;
pub use handle::WindowHandle;
use handle::native_window_number;
mod lifecycle;
#[cfg(target_os = "macos")]
mod mac;
mod pacing;
pub(crate) use pacing::{
    HiddenPace, Missed, OCCLUDED_MAX, OCCLUDED_RETRY, TIMEOUT_RETRY, TIMER_FRAME, acquire, acquire_checked,
    timer_deadline, timer_wants_frame,
};
#[cfg(test)]
pub(crate) use pacing::HIDDEN_REFRESH;
#[cfg(test)]
use pacing::wall_wake_deadline;
mod stepping;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod float_on_open;
#[cfg(not(target_arch = "wasm32"))]
pub use event_loop::run;
pub(crate) use event_loop::run_windows;
#[cfg(test)]
#[path = "window/covered_tests.rs"]
mod covered;
#[cfg(all(test, target_os = "macos"))]
#[path = "window/occluded_surface_tests.rs"]
mod occluded_surface;
#[cfg(test)]
mod tests;

pub(crate) enum WakeEvent {
    DisplayTick(usize),
    App(usize), Live(usize), Window(usize, WindowRequest), Focus(usize),
    Position(usize, (i32, i32)), Level(usize, bool),
    Place(usize, crate::windows::PlaceRequest),
    Frame(usize, crate::window::frame::FrameRequest),
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
    let open_urls = crate::open_url::incoming::listen(&spec.app);
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
        crate::open_url::incoming::settle();
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
            let _input = crate::input::time::arrived(&event);
            host.window_event(event_loop, id, event);
        }
        self.cleanup(event_loop);
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        crate::cursor::os::make_pending(event_loop);
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

use file_drop::{DROP_SAMPLE, FileDrop, FileStep};

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
    frame_reports: crate::frame::encoded::FrameReports<S>,
    finished: bool,
    /// False for windows `App::window` added: they live as long as the loop, never keep it.
    keeps_loop: bool,
    spec: AppSpec<S>,
    args: Args,
    keep_running: bool,
    on_close: Option<fn(&mut S, &mut Tx)>,
    on_reopen: Option<fn(&mut S, bool, &mut Tx) -> bool>,
    /// Its place among the windows receiving links (`App::on_open_url`).
    _open_urls: Option<crate::open_url::incoming::Listening>,
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
    gpu: Option<(Arc<Gpu>, Renderer)>,
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
    secure_input: crate::input::secure_input::SecureInput,
    ime_allowed: bool,
    comp: Option<Box<dyn Compositor<S>>>,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    wayland: bool,
    /// The window's frame moving to where a handler asked (`Tx::resize_window`), top-down
    /// screen points on the host's frame clock. macOS only; elsewhere frames change at once.
    frame_anim: Option<crate::window::frame::FrameAnim>,
    /// The window origin `frame_anim` last set, to notice a drag mid-flight.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    frame_origin: Option<[f32; 2]>,
    /// The content size a resize request jumps to, so its Resized event lays out like a step
    /// of a resize animation rather than a user's resize.
    frame_jump: Option<(f32, f32)>,
    /// Present times of every window this host opens while tracing.
    #[cfg(target_os = "macos")]
    present_log: Option<Arc<crate::frame::present_time::metal::Log>>,
    #[cfg(target_os = "macos")]
    frame_wake: Waker,
}

impl<S: 'static> Host<S> {
    fn new(window_spec: WindowSpec<S>, native_handle: WindowHandle, args: Args, proxy: EventLoopProxy<WakeEvent>, slot: usize, gpu_pool: Rc<RefCell<Weak<Gpu>>>, wayland: bool) -> Self {
        let WindowSpec { mut app, options, key: _, on_created } = window_spec;
        let mut args = app.resolve_args(args);
        let on_window = app.on_window.take();
        let on_ready = app.on_ready.take();
        let frame_reports = crate::frame::encoded::FrameReports::new(app.on_frame_encoded.take(), Some(native_handle.key())).inspecting(app.on_frame_drawn.take());
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
        let (srgb_blend, font_smoothing) = (app.srgb_blend, app.font_smoothing);
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
            size = crate::window::frame::clamp_size((size.0, rt.content_height()), min, max);
            rt.set_viewport(size, env.scale);
            spec.size = size;
            args.size = None;
        }
        rt.on_focus = on_focus;
        rt.app_ime.hook = on_ime;
        rt.set_modifier_keys(modifier_keys);
        rt.set_srgb_blend(srgb_blend); rt.set_font_smoothing(font_smoothing);
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
    // Backdrop copies read the drawable mid-frame (`ActivePass::split`: backdrop blur, effect
    // backdrops, rune-fx blend modes). On Metal it costs `framebufferOnly`, but without it they
    // quietly fall back.
    config.usage |= caps.usages & wgpu::TextureUsages::COPY_SRC;
    config.present_mode = wgpu::PresentMode::AutoVsync;
    config.desired_maximum_frame_latency = 2;
    Some((config, crate::render::translucency::mode(surface, adapter)))
}

/// Follows a blend switch (`Tx::set_srgb_blend`) on a configured surface: reconfigures it for
/// `rt`'s mode at its size and alpha mode, rebuilds `renderer` when its target format changes,
/// and rebuilds the `alpha` pass (when there is one) for the new format. Returns false when the
/// surface can't be configured.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn reblend<S: 'static>(surface: &wgpu::Surface, gpu: &Gpu, config: &mut wgpu::SurfaceConfiguration,
    renderer: &mut Renderer, alpha: &mut Option<crate::render::surface_alpha::AlphaSurface>, rt: &mut Runtime<S>) -> bool {
    let srgb = rt.srgb_blend();
    let Some((next, _)) = surface_config(surface, &gpu.adapter, (config.width, config.height), srgb) else { return false };
    if (config.format, &config.view_formats) != (next.format, &next.view_formats) {
        (config.format, config.view_formats) = (next.format, next.view_formats);
        surface.configure(&gpu.device, config);
    }
    let target = if srgb { config.format.remove_srgb_suffix() } else { config.format };
    rebuild_renderer(&gpu.device, renderer, target, rt);
    if alpha.is_some() {
        *alpha = Some(crate::render::surface_alpha::AlphaSurface::new(&gpu.device, target, (config.width, config.height)));
    }
    true
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
    return crate::platform::macos::content_origin(window);
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
    if let Some(scale) = crate::platform::macos::backing_scale(window) { return scale; }
    window.scale_factor() as f32
}

/// Native titlebar behaviour the framework owns: traffic-light placement, drag regions and the
/// double-click action. Elsewhere only the portable parts apply.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn uses_drag_regions(chrome: Chrome, borderless: bool, panel: bool) -> bool {
    chrome.drag_regions && (borderless || chrome.full_size_content) && !panel
}

/// Places the traffic lights and returns `Cx::titlebar_inset`.
fn place_chrome(window: &Window, chrome: Chrome) -> f32 {
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
            if self.float_on_open { attrs = float_on_open::fixed(attrs, &self.options, size); }
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
            Some(crate::platform::macos::Panel::adopt_window(&window, size, &self.macos_window).ok_or("could not create nonactivating panel")?)
        } else { None };
        #[cfg(target_os = "macos")]
        crate::platform::macos::configure_window(&window, &self.macos_window);
        #[cfg(target_os = "macos")]
        crate::platform::macos::drop::install(&window);
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
            Some((gpu, renderer)) => {
                let surface = gpu.instance.create_surface(window.clone());
                self.gpu = Some((gpu, renderer));
                (None, surface.map_err(|e| e.to_string())?)
            }
            None => {
                let features = self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
                let transparent = self.options.transparent;
                // A shared device that can't composite this window's translucent pixels gets its own.
                let cached = self.gpu_pool.borrow().upgrade().and_then(|gpu| {
                    let surface = gpu.instance.create_surface(window.clone()).ok()?;
                    crate::render::translucency::serves(&gpu, &surface, features, transparent).then_some((gpu, surface))
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
            self.gpu = Some((gpu, renderer));
        }
        let (gpu, renderer) = self.gpu.as_mut().unwrap();
        rebuild_renderer(&gpu.device, renderer, target_format, &mut self.rt);
        surface.configure(&gpu.device, &config);
        #[cfg(target_os = "macos")]
        crate::platform::macos::pin_contents_top_left(&surface);
        #[cfg(target_os = "macos")]
        crate::platform::macos::window_shape::apply_corners(&window, &self.macos_window, self.options.transparent, self.options.borderless);
        // Metal's PostMultiplied mode only makes CAMetalLayer non-opaque; Core Animation
        // still composites premultiplied pixels. Other backends follow wgpu's straight-alpha contract.
        let alpha = (self.options.transparent && config.alpha_mode == wgpu::CompositeAlphaMode::PostMultiplied
            && gpu.adapter.get_info().backend != wgpu::Backend::Metal)
            .then(|| crate::render::surface_alpha::AlphaSurface::new(&gpu.device, target_format, (config.width, config.height)));
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
        let resamples = crate::platform::macos::window_shape::resamples(&window, self.options.transparent);
        #[cfg(target_os = "macos")]
        let hook_log = self.present_log.clone()
            .or_else(|| resamples.then(|| Arc::new(crate::frame::present_time::metal::Log::untraced())));
        #[cfg(target_os = "macos")]
        let present = hook_log.clone().and_then(|log| crate::frame::present_time::metal::LayerHook::attach(&surface, log));
        #[cfg(target_os = "macos")]
        let shadow = hook_log.filter(|_| resamples && present.is_some())
            .and_then(|log| crate::platform::macos::window_shape::Shadow::new(&window, log, visible || launcher));
        #[cfg(target_os = "macos")]
        if self.present_log.is_some() && present.is_none() { eprintln!("rune: trace: this surface reports no present times"); }
        #[cfg(target_os = "macos")]
        let fn_key = if self.rt.modifier_keys() { crate::input::fn_key::FnKey::new(&window) } else { None };
        #[cfg(target_os = "macos")]
        let full_screen = mac::FullScreenWatch::new(&window);
        #[cfg(target_os = "macos")]
        let prime = if visible && panel.is_none() { 2 } else { 0 };
        #[cfg(target_os = "macos")]
        let native = Native {
            pacing: crate::platform::display_link::DisplayLink::new(&window, self.frame_wake.clone()),
            present,
            shadow,
            panel,
            materials: crate::system_material::macos::Materials::new(),
            help: Default::default(),
            full_screen,
            fn_key,
            prime,
            launch_drawables,
            _spot: crate::platform::macos::traffic_lights::Spot::new(&window),
        };
        #[cfg(not(target_os = "macos"))]
        let native = Native::default();
        self.win = Some(Win {
            native,
            first_frame: FirstFrame::opened(Instant::now()),
            surface,
            config,
            window,
            visible,
            occluded: false,
            retry: None,
            occluded_wait: OCCLUDED_RETRY,
            hidden_wait: TIMEOUT_RETRY,
            hidden_pace: HiddenPace::default(),
            cursor: crate::cursor::os::OsCursor::new(event_loop),
            alpha,
            quiet: false,
            presented: None,
        });
        if launcher { self.win.as_mut().unwrap().first_frame.prepare(); }
        self.apply_placement(event_loop);
        #[cfg(target_os = "macos")]
        if visible && let Some(w) = &self.win && w.native.panel.is_some() { w.show(focus); }
        if let Some(created) = self.on_created.take() { created(self.native_handle.clone()); }
        if let Some(on_window) = self.on_window.as_mut() { on_window(&self.native_handle); }
        if let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) {
            #[cfg(target_os = "macos")]
            if let Some(pacing) = &mut w.native.pacing { pacing.update_display(&w.window); }
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
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut drew = false;
        #[cfg(target_os = "macos")]
        if visible && !self.rt.minimize_request && !self.finished && !event_loop.exiting()
            && self.win.as_ref().is_some_and(|w| w.native.panel.is_none() && w.window.is_minimized() != Some(true))
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
