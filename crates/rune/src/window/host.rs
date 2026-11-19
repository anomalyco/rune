//! Non-generic native window host, wrapping a type-erased [`WindowApp`].

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Weak, atomic::{AtomicBool, Ordering}};

use winit::event_loop::EventLoopProxy;
use winit::keyboard::ModifiersState;

use super::file_drop::FileDrop;
use super::handle::WindowHandle;
use super::state::{Life, Rest};
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
pub(in crate::window) mod shell_requests;
pub(in crate::window) mod stepping;

#[cfg(test)]
#[path = "host/frame_move_tests.rs"]
mod frame_move_tests;

pub(in crate::window) use app::{CompositorFrame, WindowApp};
use app::TypedWindowApp;

/// The only parts of `AppSpec<S>` the window host reads after construction.
#[derive(Clone, Copy, Debug)]
pub(in crate::window) struct WindowGeometry {
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
    pub(in crate::window) native_handle: WindowHandle,
    pub(in crate::window) on_created: Option<Box<dyn FnOnce(WindowHandle)>>,
    pub(in crate::window) on_window: Option<crate::app::OnWindow>,
    pub(in crate::window) on_ready: Option<crate::app::OnWindow>,
    pub(in crate::window) frame_reports: crate::frame::encoded::FrameReports,
    /// Shown, hidden or closed for good; `win` holds the window while it has one.
    pub(in crate::window) life: Life,
    /// False for windows `App::window` added: they live as long as the loop, never keep it.
    pub(in crate::window) keeps_loop: bool,
    pub(in crate::window) geometry: WindowGeometry,
    pub(in crate::window) args: Args,
    pub(in crate::window) keep_running: bool,
    /// What the window still owes after its first frame (`post_frame`).
    #[cfg(target_os = "windows")]
    pub(in crate::window) chores: super::win32::post_frame::Chores,
    /// Its close ends the app (`Windows::window_event`): the last window of a fast-exiting app.
    pub(in crate::window) ends_app: bool,
    /// Whether a turn is inside `App::turn_scope` now.
    pub(in crate::window) turn: any_host::Turn,
    /// Its place among the windows receiving links (`App::on_open_url`).
    pub(in crate::window) _open_urls: Option<crate::open_url::incoming::Listening>,
    pub(in crate::window) release_gpu: bool,
    pub(in crate::window) open_at_start: bool,
    /// `App::float_on_open`: each window maps at a fixed size, then takes its own limits.
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
    pub(in crate::window) float_on_open: bool,
    /// `App::open_inactive`: the first open leaves focus where it is.
    pub(in crate::window) open_inactive: bool,
    /// `App::maximized`, `App::full_screen`.
    pub(in crate::window) open_as: crate::windows::OpenAs,
    pub(in crate::window) shortcuts: crate::shortcuts::Guard,
    pub(in crate::window) popup: crate::popup_menu::Presenter,
    pub(in crate::window) file_dialog: crate::file_dialog::Presenter,
    pub(in crate::window) app: Box<dyn WindowApp>,
    /// Before `win` and `gpu`: fields drop in order, and the trace waits for the window's last
    /// present times.
    pub(in crate::window) trace: Option<crate::tooling::Trace>,
    pub(in crate::window) trace_start: Option<Instant>,
    pub(in crate::window) trace_frames: usize,
    pub(in crate::window) meter: SurfaceMeter,
    pub(in crate::window) win: Option<Win>,
    pub(in crate::window) gpu: Option<(Arc<Gpu>, Renderer)>,
    pub(in crate::window) gpu_pool: Rc<RefCell<Weak<Gpu>>>,
    /// A window has shown before: the next one opens settled.
    pub(in crate::window) shown: bool,
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
    pub(in crate::window) ime_allowed: bool,
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

struct HostInit {
    app_woken: Arc<AtomicBool>,
    chrome: Chrome,
    titlebar: Option<crate::Titlebar>,
    options: WindowOptions,
    macos_window: crate::MacWindowOptions,
    native_handle: WindowHandle,
    on_created: Option<Box<dyn FnOnce(WindowHandle)>>,
    on_window: Option<crate::app::OnWindow>,
    on_ready: Option<crate::app::OnWindow>,
    frame_reports: crate::frame::encoded::FrameReports,
    geometry: WindowGeometry,
    args: Args,
    keep_running: bool,
    release_gpu: bool,
    open_at_start: bool,
    float_on_open: bool,
    open_inactive: bool,
    open_as: crate::windows::OpenAs,
    shortcuts: crate::shortcuts::Guard,
    waker: Waker,
    live_proxy: EventLoopProxy<WakeEvent>,
    #[cfg(target_os = "macos")]
    frame_proxy: EventLoopProxy<WakeEvent>,
    slot: usize,
    gpu_pool: Rc<RefCell<Weak<Gpu>>>,
    wayland: bool,
}

impl Host {
    pub(in crate::window) fn new<S: 'static>(
        window_spec: WindowSpec<S>,
        native_handle: WindowHandle,
        args: Args,
        proxy: EventLoopProxy<WakeEvent>,
        slot: usize,
        gpu_pool: Rc<RefCell<Weak<Gpu>>>,
        wayland: bool,
    ) -> Self {
        let WindowSpec { mut app, options, key: _, on_created } = window_spec;
        let mut args = app.resolve_args(args);
        let on_window = app.on_window.take();
        let on_ready = app.on_ready.take();
        let frame_reports = crate::frame::encoded::FrameReports::new(app.on_frame_encoded.take(), Some(native_handle.key()));
        let on_frame_drawn = app.on_frame_drawn.take();
        let macos_window = std::mem::take(&mut app.macos_window);
        // Before the menu guard, so custom items in Edit or Window join these menus.
        #[cfg(target_os = "macos")]
        crate::native_menu::install_standard_menus(app.standard_menus);
        native_handle.bind(proxy.clone(), slot);
        let live_proxy = proxy.clone();
        #[cfg(target_os = "macos")]
        let frame_proxy = proxy.clone();
        let app_woken = Arc::new(AtomicBool::new(false));
        let woken = app_woken.clone();
        let waker: Waker = Arc::new(move || {
            woken.store(true, Ordering::Release);
            let _ = proxy.send_event(WakeEvent::App(slot));
        });
        let (open, keep_running, on_close, release_gpu) = (app.open, app.keep_running, app.on_close, app.release_gpu);
        let on_rest = app.on_rest;
        let (on_reopen, on_quit, turn_scope) = (app.on_reopen, app.on_quit, app.turn_scope);
        let comp = app.compositor.take();
        let titlebar = app.titlebar.filter(|_| !options.borderless);
        let chrome = app.chrome;
        let fit_height = app.fit_content_height;
        let float_on_open = app.float_on_open;
        // `RUNE_OPEN_INACTIVE`: a tool's launch (an accessibility probe, a capture) leaves the user's
        // focus and keyboard where they are, as `App::open_inactive` does.
        let (open_inactive, open_as) = (app.open_inactive || std::env::var_os("RUNE_OPEN_INACTIVE").is_some(), app.open_as);
        let (playground, attach_playground) = (app.playground.take(), app.attach_playground);
        let (mut spec, state, shared, runtime) = app.into_parts(args.scene.as_deref());
        let mut size = args.size.unwrap_or(spec.size);
        // The window reports the system appearance and scale when it opens.
        let env = Env::new(args.theme.as_deref() != Some("light"), 1.0);
        let mut rt = runtime_for(&spec, state, size, env, setup_waker(&shared));
        if let Some(attach) = attach_playground { attach(&mut rt, playground, &spec, args.playground.clone(), args.variant.clone(), true); }
        rt.configure(runtime);
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
        shared.connect(waker.clone());
        let shortcuts = crate::shortcuts::Guard::attach(&shared, waker.clone(), false, wayland);
        let menus = crate::native_menu::Guard::attach(&shared, false);
        let geometry = WindowGeometry { title: spec.title, size: spec.size, min_size: spec.min_size };
        let typed = Box::new(TypedWindowApp {
            rt,
            shared,
            menus,
            comp,
            on_frame_drawn,
            on_close,
            on_rest,
            on_quit,
            on_reopen,
            turn_scope,
        });
        Self::from_app(typed, HostInit {
            app_woken,
            chrome,
            titlebar,
            options,
            macos_window,
            native_handle,
            on_created,
            on_window,
            on_ready,
            frame_reports,
            geometry,
            args,
            keep_running,
            release_gpu,
            open_at_start: open,
            float_on_open,
            open_inactive,
            open_as,
            shortcuts,
            waker,
            live_proxy,
            #[cfg(target_os = "macos")]
            frame_proxy,
            slot,
            gpu_pool,
            wayland,
        })
    }

    fn from_app(mut app: Box<dyn WindowApp>, init: HostInit) -> Self {
        let HostInit {
            app_woken,
            chrome,
            titlebar,
            options,
            macos_window,
            native_handle,
            on_created,
            on_window,
            on_ready,
            frame_reports,
            geometry,
            args,
            keep_running,
            release_gpu,
            open_at_start,
            float_on_open,
            open_inactive,
            open_as,
            shortcuts,
            waker,
            live_proxy,
            #[cfg(target_os = "macos")]
            frame_proxy,
            slot,
            gpu_pool,
            wayland,
        } = init;
        #[cfg(target_os = "macos")]
        let present_log = args.trace.as_ref().map(|_| Arc::default());
        let has_comp = app.has_compositor();
        let meter = SurfaceMeter::new(native_handle.key(), native_handle.native.stats.clone());
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
            rt.trace_channels = args.trace.as_ref().map(|_| Default::default());
            rt.set_live_waker(Arc::new(move || { let _ = live_proxy.send_event(WakeEvent::Live(slot)); }));
            rt.set_os_drag(crate::drag_out::SUPPORTED);
            rt.transparent = options.transparent;
            rt.track_marks = has_comp;
            rt.wall_start = Some(Instant::now());
            rt.open_urls_natively = args.script.is_none() && args.trace.is_none() && args.frames.is_none();
            rt.record_events = false;
            rt.surface_stats = Some(meter.shared());
        }
        app.set_live_visible(false);
        let mut clipboard = crate::clipboard::Client::default();
        clipboard.connect(waker.clone());
        let popup = crate::popup_menu::Presenter::new(waker.clone());
        let file_dialog = crate::file_dialog::Presenter::new(waker);
        Host {
            app_woken,
            chrome,
            titlebar,
            options,
            placement: None,
            macos_window,
            native_handle,
            on_created,
            on_window,
            on_ready,
            frame_reports,
            life: Life::Unopened,
            keeps_loop: true,
            geometry,
            args,
            keep_running,
            #[cfg(target_os = "windows")]
            chores: Default::default(),
            ends_app: false,
            turn: any_host::Turn::Between,
            _open_urls: None,
            release_gpu,
            open_at_start,
            float_on_open,
            open_inactive,
            open_as,
            shortcuts,
            popup,
            file_dialog,
            app,
            win: None,
            gpu: None,
            gpu_pool,
            shown: false,
            started: false,
            clock: FrameClock::new(Instant::now()),
            idle: true,
            rest: Rest::default(),
            mods: ModifiersState::empty(),
            control_click: false,
            files: FileDrop::default(),
            clipboard,
            secure_input: Default::default(),
            ime_allowed: false,
            wayland,
            frame_move: None,
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

    /// Tells the runtime where the window's content is on screen, drawing a frame when that
    /// moves an `El::on_screen_layout` element.
    pub(in crate::window) fn sync_origin(&mut self) {
        let Some(w) = &self.win else { return };
        if sync_screen_origin_core(self.app.rt_mut(), &w.window) {
            w.request_redraw();
        }
    }

    /// Publishes what `AppHandle::is_visible` reports.
    pub(in crate::window) fn publish(&self) {
        let visible = self.life.shown() && self.win.is_some();
        self.app.set_visible(visible);
    }
}

/// `setup`'s waker: `AppHandle::wake`, so a drain that also holds updates that changed nothing
/// still polls and redraws for it.
pub(in crate::window) fn setup_waker<S: 'static>(shared: &Arc<crate::app::Shared<S>>) -> Waker {
    let handle = crate::AppHandle { shared: shared.clone() };
    Arc::new(move || handle.wake())
}
