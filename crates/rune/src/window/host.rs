//! Non-generic native window host, wrapping a type-erased [`WindowApp`].

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Weak, atomic::{AtomicBool, Ordering}};

use winit::event_loop::EventLoopProxy;
use winit::keyboard::ModifiersState;

use super::file_drop::FileDrop;
use super::handle::WindowHandle;
use super::state::{Life, ReopenState, Rest};
use super::surface::Win;
use super::{WakeEvent, sync_screen_origin_core};
use crate::frame::clock::FrameClock;
use crate::frame::surface_stats::SurfaceMeter;
use crate::render::{Gpu, Renderer};
use crate::runtime::{Env, Waker};
use crate::time::Instant;
use crate::tooling::{Args, Chrome, runtime_for};
use crate::windows::{WindowOptions, WindowSpec};

pub(in crate::window) mod any_host;
pub(in crate::window) mod app;
pub(in crate::window) mod create;
pub(in crate::window) mod draw;
pub(in crate::window) mod events;
pub(in crate::window) mod lifecycle;
mod render;
pub(in crate::window) mod shell_requests;
pub(in crate::window) mod stepping;
pub(in crate::window) mod window_script;

#[cfg(test)]
#[path = "host/frame_move_tests.rs"]
mod frame_move_tests;

pub(in crate::window) use app::{CompositorFrame, WindowApp};
use app::TypedWindowApp;

/// The only parts of `AppSpec<S>` the window host reads after construction.
#[derive(Clone, Copy, Debug)]
pub(in crate::window) struct SpecGeometry {
    pub title: &'static str,
    pub size: (f32, f32),
    pub min_size: Option<(f32, f32)>,
}

pub(in crate::window) struct Host {
    /// The app's waker ran and its event isn't handled yet (`WakeEvent::App`): work it brought (a
    /// streamed token, a tool's result) may already be readable, so no frame is loop-only until
    /// the event's `drain` has polled it.
    pub(in crate::window) app_woken: Arc<AtomicBool>,
    /// `App::chrome`, as the app set it (`chrome::frame_chrome` lays a titlebar over it).
    pub(in crate::window) chrome: Chrome,
    /// `App::titlebar`, unless the window is borderless.
    pub(in crate::window) titlebar: Option<crate::Titlebar>,
    pub(in crate::window) options: WindowOptions,
    pub(in crate::window) placement: Option<crate::windows::PlaceRequest>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(in crate::window) macos_window: crate::MacWindowOptions,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(in crate::window) windows_window: crate::WindowsWindowOptions,
    #[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
    pub(in crate::window) exclude_from_capture: Option<bool>,
    /// Per-window native color scheme (`App::color_scheme`, `Tx::set_color_scheme`,
    /// `WindowHandle::set_color_scheme`), retained across native window recreation.
    pub(in crate::window) color_scheme: crate::ColorScheme,
    /// Requested whole-window system backdrop (`App::window_backdrop`, `Tx::set_window_backdrop`,
    /// `WindowHandle::set_window_backdrop`), retained across native window recreation.
    pub(in crate::window) window_backdrop: crate::WindowBackdrop,
    /// Optional macOS swap-chain frame latency (`App::frame_latency`).
    pub(in crate::window) frame_latency: Option<u32>,
    /// Last winit theme applied to the live `self.win`, avoiding redundant `set_theme` calls.
    pub(in crate::window) applied_theme: Option<winit::window::Theme>,
    pub(in crate::window) native_handle: WindowHandle,
    pub(in crate::window) on_created: Option<Box<dyn FnOnce(WindowHandle)>>,
    pub(in crate::window) on_window: Option<crate::app::OnWindow>,
    pub(in crate::window) on_ready: Option<crate::app::OnWindow>,
    pub(in crate::window) frame_reports: crate::frame::encoded::FrameReports,
    #[cfg(target_os = "macos")]
    pub(in crate::window) frame_presented: crate::frame::presented::FramePresentReports,
    #[cfg(target_os = "macos")]
    pub(in crate::window) ambient_period: Option<std::time::Duration>,
    /// Shown, hidden or closed for good; `win` holds the window while it has one.
    pub(in crate::window) life: Life,
    /// False for windows `App::window` added: they live as long as the loop, never keep it.
    pub(in crate::window) keeps_loop: bool,
    pub(in crate::window) geometry: SpecGeometry,
    /// The last sample `App::on_window_geometry` heard.
    pub(in crate::window) geometry_watch: super::geometry::GeometryWatch,
    pub(in crate::window) args: Args,
    pub(in crate::window) keep_running: bool,
    /// What the window still owes after its first frame (`post_frame`).
    #[cfg(target_os = "windows")]
    pub(in crate::window) chores: super::win32::post_frame::Chores,
    /// Its close ends the app (`Windows::window_event`): the last window of a fast-exiting app.
    pub(in crate::window) ends_app: bool,
    /// Whether a turn is inside `App::turn_scope` now.
    pub(in crate::window) turn: any_host::Turn,
    /// Its place among the windows receiving links (`App::on_open_url`) and files (`App::on_open_files`).
    pub(in crate::window) _incoming: crate::open_url::incoming::Listeners,
    pub(in crate::window) release_gpu: bool,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(in crate::window) cpu_first: crate::CpuFirst,
    pub(in crate::window) prewarm_effects: crate::PrewarmEffects,
    pub(in crate::window) open_at_start: bool,
    /// `App::float_on_open`: each window maps at a fixed size, then takes its own limits.
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
    pub(in crate::window) float_on_open: bool,
    /// `App::open_inactive`: the first open leaves focus where it is.
    pub(in crate::window) open_inactive: bool,
    /// Opening mode (`App::maximized`, `App::full_screen`) and restored frame across recreation.
    pub(in crate::window) reopen: ReopenState,
    pub(in crate::window) shared: Arc<crate::app::SharedCore>,
    pub(in crate::window) shortcuts: crate::shortcuts::Guard,
    pub(in crate::window) menus: crate::native_menu::MenuGuard,
    pub(in crate::window) popup: crate::popup_menu::Presenter,
    pub(in crate::window) file_dialog: crate::file_dialog::Presenter,
    pub(in crate::window) app: Box<dyn WindowApp>,
    /// Before `win` and `gpu`: fields drop in order, and the trace waits for the window's last
    /// present times.
    pub(in crate::window) trace: Option<crate::tooling::Trace>,
    pub(in crate::window) trace_start: Option<Instant>,
    pub(in crate::window) trace_frames: usize,
    /// `--window-script`: the script this window replays (`window_script`).
    pub(in crate::window) window_script: Option<window_script::WindowScript>,
    pub(in crate::window) meter: SurfaceMeter,
    pub(in crate::window) win: Option<Win>,
    pub(in crate::window) gpu: Option<(Arc<Gpu>, Renderer)>,
    pub(in crate::window) gpu_pool: Rc<RefCell<Weak<Gpu>>>,
    /// A window has shown before: the next one opens settled.
    pub(in crate::window) shown: bool,
    /// Why the first window couldn't be created when this host won't stay alive without one.
    pub(in crate::window) startup_error: Option<String>,
    pub(in crate::window) started: bool,
    pub(in crate::window) clock: FrameClock,
    pub(in crate::window) idle: bool,
    /// Whether the renderer holds what it gives back at rest (`rest`).
    pub(in crate::window) rest: Rest,
    pub(in crate::window) mods: ModifiersState,
    pub(in crate::window) control_click: bool,
    pub(in crate::window) files: FileDrop,
    pub(in crate::window) clipboard: crate::clipboard::Client,
    pub(in crate::window) secure_input: crate::input::secure_input::SecureInput,
    pub(in crate::window) ime: crate::input::host_events::ImeSync,
    /// While a new window's first frame is drawn: its text input starts once that frame is with
    /// the window server, not inside it. On macOS, allowing the IME for a window of the app in
    /// front (one started from the Dock) loads AppKit's text input services: about 15,000 µs.
    pub(in crate::window) ime_waits: bool,
    /// macOS, while a new window's first frame is drawn: its display link is made once that
    /// frame is with the window server. The frame is drawn at once, unpaced, and making the
    /// link asks the window server three times (about 2,000 µs).
    #[cfg(target_os = "macos")]
    pub(in crate::window) pacing_waits: bool,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(in crate::window) wayland: bool,
    /// The frame moving or jumping to where a handler asked (`stepping::FrameMove`).
    pub(in crate::window) frame_move: Option<stepping::FrameMove>,
    /// Present times of every window this host opens while tracing.
    #[cfg(target_os = "macos")]
    pub(in crate::window) present_log: Option<Arc<crate::frame::present_time::metal::Log>>,
    #[cfg(target_os = "macos")]
    pub(in crate::window) frame_wake: Waker,
}

struct HostWaker {
    app_woken: Arc<AtomicBool>,
    waker: Waker,
    live_proxy: EventLoopProxy<WakeEvent>,
    #[cfg(target_os = "macos")]
    frame_proxy: EventLoopProxy<WakeEvent>,
}

impl HostWaker {
    fn prepare(
        key: &str,
        handle: Option<WindowHandle>,
        overlays: &mut Vec<Box<dyn crate::overlay::PendingOverlay>>,
        cx: super::Start<'_>,
        standard_menus: bool,
    ) -> (Self, WindowHandle, usize, Rc<RefCell<Weak<Gpu>>>, bool) {
        crate::video::enable_playback();
        cx.overlays.append(overlays);
        let native_handle = handle.unwrap_or_else(|| WindowHandle::new(key));
        let super::Start { proxy, slot, gpu: gpu_pool, wayland, .. } = cx;
        // Before the menu guard, so custom items in Edit or Window join these menus.
        #[cfg(target_os = "macos")]
        crate::native_menu::install_standard_menus(standard_menus);
        #[cfg(not(target_os = "macos"))]
        let _ = standard_menus;
        native_handle.bind(proxy.clone(), slot);
        let live_proxy = proxy.clone();
        #[cfg(target_os = "macos")]
        let frame_proxy = proxy.clone();
        let app_woken = Arc::new(AtomicBool::new(false));
        let woken = app_woken.clone();
        let waker: Waker = Arc::new(move || {
            woken.store(true, Ordering::Release);
            proxy.send_event(WakeEvent::App(slot)).ok();
        });
        let prepared = Self {
            app_woken,
            waker,
            live_proxy,
            #[cfg(target_os = "macos")]
            frame_proxy,
        };
        (prepared, native_handle, slot, gpu_pool, wayland)
    }
}

struct HostInit {
    prepared: HostWaker,
    shared: Arc<crate::app::SharedCore>,
    chrome: Chrome,
    titlebar: Option<crate::Titlebar>,
    options: WindowOptions,
    macos_window: crate::MacWindowOptions,
    windows_window: crate::WindowsWindowOptions,
    exclude_from_capture: Option<bool>,
    color_scheme: crate::ColorScheme,
    window_backdrop: crate::WindowBackdrop,
    frame_latency: Option<u32>,
    native_handle: WindowHandle,
    on_created: Option<Box<dyn FnOnce(WindowHandle)>>,
    on_window: Option<crate::app::OnWindow>,
    on_ready: Option<crate::app::OnWindow>,
    on_frame_encoded: Option<crate::frame::encoded::OnFrameEncoded>,
    #[cfg(target_os = "macos")]
    on_frame_presented: Option<crate::frame::presented::OnFramePresented>,
    #[cfg(target_os = "macos")]
    ambient_period: Option<std::time::Duration>,
    geometry: SpecGeometry,
    args: Args,
    keep_running: bool,
    release_gpu: bool,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    cpu_first: crate::CpuFirst,
    prewarm_effects: crate::PrewarmEffects,
    open_at_start: bool,
    fit_height: bool,
    float_on_open: bool,
    open_inactive: bool,
    open_as: crate::windows::OpenAs,
    shortcuts: crate::shortcuts::Guard,
    menus: crate::native_menu::MenuGuard,
    slot: usize,
    gpu_pool: Rc<RefCell<Weak<Gpu>>>,
    wayland: bool,
    keeps_loop: bool,
    incoming: crate::open_url::incoming::Listeners,
}

impl Host {
    pub(in crate::window) fn new<S: 'static>(
        window_spec: WindowSpec<S>,
        handle: Option<WindowHandle>,
        args: Args,
        cx: super::Start<'_>,
        keeps_loop: bool,
        incoming: crate::open_url::incoming::Listeners,
    ) -> Self {
        let WindowSpec { mut app, options, key, on_created } = window_spec;
        let args = app.resolve_args(args);
        if let Some(hook) = app.on_display_close.take() {
            cx.on_display_close.push(hook);
        }
        let (prepared, native_handle, slot, gpu_pool, wayland) =
            HostWaker::prepare(&key, handle, &mut app.overlays, cx, app.standard_menus);
        let (on_window, on_ready, on_frame_encoded, on_frame_drawn) = (
            app.on_window.take(),
            app.on_ready.take(),
            app.on_frame_encoded.take(),
            app.on_frame_drawn.take(),
        );
        #[cfg(target_os = "macos")]
        let on_frame_presented = app.on_frame_presented.take();
        #[cfg(target_os = "macos")]
        let ambient_period = app.ambient_period;
        let macos_window = std::mem::take(&mut app.macos_window);
        let windows_window = std::mem::take(&mut app.windows_window);
        let exclude_from_capture = app.exclude_from_capture;
        let color_scheme = app.color_scheme;
        let window_backdrop = app.window_backdrop;
        let frame_latency = app.frame_latency;
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let cpu_first = app.cpu_first;
        let (open, keep_running, release_gpu, prewarm_effects, fit_height, float_on_open) = (
            app.open, app.keep_running, app.release_gpu, app.prewarm_effects, app.fit_content_height, app.float_on_open,
        );
        let (open_inactive, open_as, chrome, titlebar) = (app.open_inactive, app.open_as, app.chrome, app.titlebar);
        let (on_close_request, on_quit_request, on_close, on_rest, on_reopen, on_quit, turn_scope) = (
            app.on_close_request, app.on_quit_request, app.on_close, app.on_rest, app.on_reopen, app.on_quit, app.turn_scope,
        );
        let on_geometry = app.on_window_geometry;
        let comp = app.compositor.take();
        let (layout_seed, keep_layout) = (app.layout_seed.take(), app.keep_layout.take());
        let (playground, attach_playground) = (app.playground.take(), app.attach_playground);
        let (spec, state, shared, runtime) = app.into_parts(args.scene.as_deref());
        let size = args.size.unwrap_or(spec.size);
        // The window reports the system appearance and scale when it opens.
        let env = Env::new(args.theme.as_deref() != Some("light"), 1.0);
        let mut rt = runtime_for(&spec, state, size, env, setup_waker(&shared.core));
        if let Some(attach) = attach_playground { attach(&mut rt, playground, &spec, args.playground.clone(), args.variant.clone(), true); }
        rt.configure(runtime);
        if let Some(seed) = layout_seed.clone() { rt.core_mut().offer_layout_seed(seed); }
        shared.core.connect(prepared.waker.clone());
        let shortcuts = crate::shortcuts::Guard::attach(&shared, prepared.waker.clone(), false, wayland);
        let menus = crate::native_menu::MenuGuard::attach(&shared, false);
        let shared_core = shared.core.clone();
        let geometry = SpecGeometry { title: spec.title, size: spec.size, min_size: spec.min_size };
        let typed = Box::new(TypedWindowApp {
            rt, shared, comp, on_frame_drawn, on_close_request, on_quit_request, on_close, on_rest, on_quit, on_reopen, on_geometry, turn_scope,
            keep_layout, kept_layout: layout_seed,
        });
        Self::from_app(typed, HostInit {
            prepared,
            shared: shared_core,
            chrome,
            titlebar,
            options,
            macos_window,
            windows_window,
            exclude_from_capture,
            color_scheme,
            window_backdrop,
            frame_latency,
            native_handle,
            on_created,
            on_window,
            on_ready,
            on_frame_encoded,
            #[cfg(target_os = "macos")]
            on_frame_presented,
            #[cfg(target_os = "macos")]
            ambient_period,
            geometry,
            args,
            keep_running,
            release_gpu,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            cpu_first,
            prewarm_effects,
            open_at_start: open,
            fit_height,
            float_on_open,
            open_inactive,
            open_as,
            shortcuts,
            menus,
            slot,
            gpu_pool,
            wayland,
            keeps_loop,
            incoming,
        })
    }

    fn from_app(mut app: Box<dyn WindowApp>, init: HostInit) -> Self {
        let HostInit {
            prepared: HostWaker {
                app_woken,
                waker,
                live_proxy,
                #[cfg(target_os = "macos")]
                frame_proxy,
            },
            shared,
            chrome,
            titlebar,
            options,
            macos_window,
            windows_window,
            exclude_from_capture,
            color_scheme,
            window_backdrop,
            frame_latency,
            native_handle,
            on_created,
            on_window,
            on_ready,
            on_frame_encoded,
            #[cfg(target_os = "macos")]
            on_frame_presented,
            #[cfg(target_os = "macos")]
            ambient_period,
            mut geometry,
            mut args,
            keep_running,
            release_gpu,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            cpu_first,
            prewarm_effects,
            open_at_start,
            fit_height,
            float_on_open,
            open_inactive,
            open_as,
            shortcuts,
            menus,
            slot,
            gpu_pool,
            wayland,
            keeps_loop,
            incoming,
        } = init;
        let titlebar = titlebar.filter(|_| !options.borderless);
        // `RUNE_OPEN_INACTIVE`: a tool's launch (an accessibility probe, a capture) leaves the user's
        // focus and keyboard where they are, as `App::open_inactive` does.
        let open_inactive = open_inactive || std::env::var_os("RUNE_OPEN_INACTIVE").is_some();
        #[cfg(target_os = "macos")]
        let mut frame_presented = crate::frame::presented::FramePresentReports::new(on_frame_presented, Some(native_handle.key()));
        #[cfg(target_os = "macos")]
        let track_presented = frame_presented.wanted();
        #[cfg(not(target_os = "macos"))]
        let track_presented = false;
        let frame_reports = crate::frame::encoded::FrameReports::with_presented_seq(
            on_frame_encoded,
            track_presented,
            Some(native_handle.key()),
        );
        if fit_height {
            let size = args.size.unwrap_or(geometry.size);
            geometry.min_size = options.fit_min_size(geometry.min_size);
            // Opens at the content's height; the first frame corrects any difference its
            // real scale makes.
            app.rt_mut().host.fit_height = true;
            let (min, max) = options.size_limits(geometry.min_size);
            let fitted = crate::window::frame::clamp_size((size.0, app.content_height()), min, max);
            app.rt_mut().set_viewport(fitted, 1.0);
            geometry.size = fitted;
            args.size = None;
        }
        #[cfg(target_os = "macos")]
        let present_log = (args.trace.is_some() || frame_presented.wanted())
            .then(|| Arc::new(crate::frame::present_time::metal::Log::new(args.trace.is_some(), frame_presented.wanted())));
        #[cfg(target_os = "macos")]
        if let Some(log) = &present_log && frame_presented.wanted() {
            frame_presented.set_source(log.clone());
        }
        let has_comp = app.has_compositor();
        let meter = SurfaceMeter::new(native_handle.key(), native_handle.native.stats.clone());
        // One window replays: the one `--window` names, else the first window the app opens.
        let replays = args.window_script.is_some() && match args.window.as_deref() {
            Some(key) => native_handle.key() == key,
            None => window_script::claim_first(),
        };
        let window_script = replays.then(|| crate::tooling::read_window_script(&args).map(|events| window_script::WindowScript::new(events, crate::time::now()))).flatten();
        if replays { println!("rune-script-window {}", native_handle.key()); }
        {
            let rt = app.rt_mut();
            rt.set_system_reduced_motion(crate::platform::preferences::reduced_motion());
            rt.set_system_contrast(crate::platform::preferences::contrast());
            rt.set_system_increased_contrast(crate::platform::preferences::increased_contrast());
            rt.set_full_keyboard_access(crate::platform::preferences::full_keyboard_access());
            crate::platform::accent::watch();
            rt.set_system_accent(crate::system_accent());
            crate::platform::scroll_bars::watch();
            rt.set_system_scroll_bars_state(crate::platform::scroll_bars::current().with_override(args.scroll_bars));
            rt.frame.enable_trace_channels(args.trace.is_some());
            rt.set_live_waker(Arc::new(move || { live_proxy.send_event(WakeEvent::Live(slot)).ok(); }));
            rt.set_os_drag(crate::drag_out::SUPPORTED);
            rt.transparent = options.transparent;
            rt.track_marks = has_comp;
            rt.wall_start = Some(crate::time::now());
            rt.host.open_urls_natively = args.script.is_none() && args.window_script.is_none() && args.trace.is_none() && args.frames.is_none();
            rt.frame.set_record_events(false);
            rt.host.surface_stats = Some(meter.shared());
            rt.host.represented_file.seed(macos_window.represented_file.clone());
            rt.host.document_edited.seed(macos_window.document_edited);
        }
        app.set_live_visible(false);
        let mut clipboard = crate::clipboard::Client::default();
        clipboard.connect(waker.clone());
        let popup = crate::popup_menu::Presenter::new(waker.clone());
        let file_dialog = crate::file_dialog::Presenter::new(waker);
        #[cfg(target_os = "windows")]
        {
            native_handle.native.no_activate.store(windows_window.no_activate, Ordering::Release);
            native_handle.native.tool_window.store(windows_window.tool_window, Ordering::Release);
        }
        Host {
            app_woken,
            chrome,
            titlebar,
            options,
            placement: None,
            macos_window,
            windows_window,
            exclude_from_capture,
            color_scheme,
            window_backdrop,
            frame_latency,
            applied_theme: None,
            native_handle,
            on_created,
            on_window,
            on_ready,
            frame_reports,
            #[cfg(target_os = "macos")]
            frame_presented,
            #[cfg(target_os = "macos")]
            ambient_period,
            life: Life::Unopened,
            keeps_loop,
            geometry,
            geometry_watch: Default::default(),
            args,
            keep_running,
            #[cfg(target_os = "windows")]
            chores: Default::default(),
            ends_app: false,
            turn: any_host::Turn::Between,
            _incoming: incoming,
            release_gpu,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            cpu_first,
            prewarm_effects,
            open_at_start,
            float_on_open,
            open_inactive,
            reopen: ReopenState::new(open_as),
            shared,
            shortcuts,
            menus,
            popup,
            file_dialog,
            app,
            win: None,
            gpu: None,
            gpu_pool,
            shown: false,
            startup_error: None,
            started: false,
            clock: FrameClock::new(crate::time::now()),
            idle: true,
            rest: Rest::default(),
            mods: ModifiersState::empty(),
            control_click: false,
            files: FileDrop::default(),
            clipboard,
            secure_input: Default::default(),
            ime_waits: false,
            #[cfg(target_os = "macos")]
            pacing_waits: false,
            ime: Default::default(),
            wayland,
            frame_move: None,
            trace: None,
            trace_start: None,
            trace_frames: 0,
            window_script,
            meter,
            #[cfg(target_os = "macos")]
            present_log,
            #[cfg(target_os = "macos")]
            frame_wake: Arc::new(move || { frame_proxy.send_event(WakeEvent::DisplayTick(slot)).ok(); }),
        }
    }

    /// Notes live user input reaching this window so an opt-in `App::ambient_fps` cap lifts
    /// immediately on the next display refresh.
    pub(in crate::window) fn note_input(&mut self, at: Instant) {
        #[cfg(target_os = "macos")]
        if let Some(w) = &mut self.win
            && let Some(pacing) = &mut w.native.pacing
        {
            pacing.note_input(at);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = at;
    }

    /// Tells the runtime where the window's content is on screen, drawing a frame when that
    /// moves an `El::on_screen_layout` element.
    pub(in crate::window) fn sync_origin(&mut self) {
        let Some(w) = &self.win else { return };
        if sync_screen_origin_core(self.app.rt_mut(), &w.window) {
            w.request_redraw();
        }
    }

    /// Captures the live native window's state and restored bounds into `self.reopen` so a
    /// window recreated after hide/release or Wayland unmapping opens the same way. Skips
    /// hidden/unopened windows and unconfigured Wayland windows before their first frame.
    pub(in crate::window) fn record_live_window_state(&mut self) {
        let Some(w) = self.shown_win() else { return };
        if self.wayland && !w.first_frame.notified() { return; }
        let allow_position = self.placement.is_none() && !self.wayland;
        let track_size = !self.app.rt().host.fit_height;
        self.reopen.record_live(
            w.live_state(self.wayland),
            super::state::reopen::use_physical_size(self.wayland),
            allow_position,
            track_size,
        );
    }

    /// Effective CPU-first policy for this window, with `RUNE_CPU_FIRST` taking precedence over
    /// `App::cpu_first` / `WindowSpec::cpu_first`.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(in crate::window) fn effective_cpu_first(&self) -> crate::CpuFirst {
        self.cpu_first.effective()
    }

    /// Publishes what `AppHandle::is_visible` reports.
    pub(in crate::window) fn publish(&self) {
        let visible = self.life.shown() && self.win.is_some();
        self.shared.set_visible(visible);
    }
}

/// `setup`'s waker: `AppHandle::wake`, so a drain that also holds updates that changed nothing
/// still polls and redraws for it.
pub(in crate::window) fn setup_waker(shared: &Arc<crate::app::SharedCore>) -> Waker {
    let shared = shared.clone();
    Arc::new(move || shared.wake_app())
}
