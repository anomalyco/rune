//! One event loop, independent window lifetime hosts. App/AppHandle and multi-window
//! factories share the same input, rendering, activation and close/reopen paths.

use std::sync::{Arc, Mutex, Weak, atomic::{AtomicBool, AtomicU32, Ordering}};
#[cfg(not(target_arch = "wasm32"))]
use std::{cell::RefCell, rc::Rc};
#[cfg(test)]
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use crate::time::Instant;

#[cfg(not(target_arch = "wasm32"))]
use winit::application::ApplicationHandler;
#[cfg(not(target_arch = "wasm32"))]
use winit::event::{StartCause, WindowEvent};
use winit::event::{MouseButton, TouchPhase};
#[cfg(not(target_arch = "wasm32"))]
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
#[cfg(not(target_arch = "wasm32"))]
use winit::window::WindowId;
use winit::window::Window;
use winit::raw_window_handle::HasWindowHandle;

use crate::app::WindowRequest;
#[cfg(all(test, target_os = "linux", feature = "test-gpu-noop"))]
use crate::frame::surface_stats::SurfaceMeter;
use crate::frame::surface_stats::SurfaceStats;
#[cfg(not(target_arch = "wasm32"))]
use crate::render::Gpu;
#[cfg(test)]
use crate::runtime::Runtime;
use crate::runtime::{AppDriver, RuntimeCore};
#[cfg(not(target_arch = "wasm32"))]
use crate::tooling::Args;
#[cfg(not(target_arch = "wasm32"))]
use crate::windows::WindowSpec;
use crate::windows::{AnyWindowSpec, Display};
#[cfg(test)]
use crate::el::Tx;
#[cfg(test)]
use crate::tooling::Chrome;

#[cfg(not(target_arch = "wasm32"))]
mod app_events;
mod backdrop;
#[cfg(not(target_arch = "wasm32"))]
mod chrome;
mod color_scheme;
mod cpu_first;
pub(crate) mod displays;
mod event_loop;
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) use event_loop::{ExitAction, exit_action};
#[cfg(not(target_arch = "wasm32"))]
mod file_drop;
pub(crate) mod geometry;
mod handle;
#[cfg(not(target_arch = "wasm32"))]
mod host;
mod keys;
#[cfg(not(target_arch = "wasm32"))]
mod panic_trap;
#[cfg(not(target_arch = "wasm32"))]
mod pointer_lock;
mod resize_sync;
pub mod spec;
mod state;
mod surface;
pub(crate) mod titlebar;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod warm_frame;
#[cfg(not(target_arch = "wasm32"))]
mod wayland_wake;
#[cfg(target_os = "windows")]
mod win32;
pub(crate) mod wm_state;
/// `crate::warm_gpu_cache` on Windows (`win32::handover`).
#[cfg(target_os = "windows")]
pub(crate) fn warm_gpu_cache() -> Result<(), String> { win32::handover::warm_gpu_cache() }

pub use backdrop::WindowBackdrop;
pub use color_scheme::ColorScheme;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use color_scheme::{app_color_scheme, set_app_color_scheme_state, sync_window_theme, window_theme_for_platform};
pub use cpu_first::CpuFirst;
#[cfg(target_arch = "wasm32")]
pub(crate) use cpu_first::WebStart;
pub use handle::WindowHandle;
pub use resize_sync::ResizeSync;
#[cfg(not(target_arch = "wasm32"))]
pub use event_loop::run;
#[cfg(test)]
pub(crate) use event_loop::finish_run;
pub(crate) use event_loop::run_windows;
#[cfg(any(test, target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android"))))]
pub(crate) use file_drop::FileStep;
#[cfg(any(test, target_arch = "wasm32", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
pub(crate) use keys::consumed_from;
pub(crate) use keys::{consumed_mods, key_base, map_key_event, map_mods, platform_text};
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) use keys::web_key_base_str;
pub(crate) use state::frame;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use state::rest;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use state::{Rest, RestEffect, RestEvent};
pub(crate) use state::{Life, Minimize, MinimizeEvent, Sight};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use surface::{
    Backoff, BackoffEffect, BackoffEvent, ConfigureError, HiddenPace, acquire_checked, configure,
    configure_checked, frame_latency, reblend_core, poll_timer_deadline_core, surface_config,
    timer_deadline_core, timer_wants_frame_core,
};
pub(crate) use surface::{Missed, Presented, TIMER_FRAME, rebuild_renderer_core};
#[cfg(target_arch = "wasm32")]
pub(crate) use surface::configure_then;
#[cfg(target_os = "macos")]
pub(crate) use surface::native::Prime;
#[cfg(test)]
pub(crate) use surface::{
    ConfigureErrorKind, classify_scope_error,
    HIDDEN_REFRESH, OCCLUDED_MAX, OCCLUDED_RETRY, TIMEOUT_RETRY, TIMER_FRAME_STALL, TimerFrame,
    timer_deadline, timer_wants_frame, wall_wake_deadline,
};
#[cfg(all(test, target_os = "linux", feature = "test-gpu-noop"))]
use surface::acquire;
#[cfg(target_os = "windows")]
pub(crate) use win32::poll_pace;
#[cfg(target_os = "windows")]
pub(crate) use win32::resize_sync::valid_rects as win32_valid_rects;
#[cfg(target_os = "windows")]
pub(crate) use win32::slot;

#[cfg(not(target_arch = "wasm32"))]
use host::Host;
#[cfg(test)]
use chrome::uses_drag_regions;
#[cfg(test)]
use host::setup_waker;
#[cfg(all(test, feature = "hot"))]
pub(crate) use host::app::{TypedWindowApp, WindowApp};
#[cfg(test)]
use state::{WakeFacts, WakeWork};
#[cfg(all(test, target_os = "macos"))]
pub(crate) use surface::LaunchDrawables;
#[cfg(test)]
use surface::{SurfaceRetries, composing};

/// Records a presented frame and wakes Windows' deferred ICU word segmenter on the first one.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn frame_presented() {
    if crate::marks::presented() {
        #[cfg(all(target_os = "windows", not(feature = "segmenter-dictionaries")))]
        crate::platform::win32::segmenter::first_frame_presented();
    }
}

#[cfg(test)]
mod tests;

// Slot indices on shared headless/desktop variants are only read by the native event loop;
// wasm32's virtual_desktop matches by variant.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum WakeEvent {
    #[cfg(not(target_arch = "wasm32"))]
    DisplayTick(usize),
    #[cfg(not(target_arch = "wasm32"))]
    App(usize),
    #[cfg(not(target_arch = "wasm32"))]
    Live(usize),
    Window(usize, WindowRequest),
    Focus(usize),
    Position(usize, (i32, i32)),
    #[cfg(not(target_arch = "wasm32"))]
    Level(usize, bool),
    #[cfg(not(target_arch = "wasm32"))]
    ExcludeFromCapture(usize, bool),
    #[cfg(not(target_arch = "wasm32"))]
    ColorScheme(usize, crate::ColorScheme),
    #[cfg(not(target_arch = "wasm32"))]
    WindowBackdrop(usize, crate::WindowBackdrop),
    /// App-wide color scheme changed (`AppHandle::set_color_scheme`): every host syncs with
    /// `app_color_scheme()`.
    #[cfg(not(target_arch = "wasm32"))]
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    AppColorScheme,
    Place(usize, crate::windows::PlaceRequest),
    Frame(usize, crate::window::frame::FrameRequest),
    /// Windows: the window's or an overlay's swap chain can take the frame it waits for (`slot`).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    FrameSlot(usize),
    /// The app was reopened (macOS: its Dock icon clicked; Windows: launched again while it
    /// runs, see `single_instance`); for `Windows`, not one host.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    Reopen,
    /// The system asks the app to quit (`crate::quit`): every window's `App::on_quit` runs;
    /// with `true` the session ends now and the windows close.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Quit(bool),
    /// macOS `applicationShouldTerminate:` deferred to `App::on_quit_request` hooks.
    #[cfg(not(target_arch = "wasm32"))]
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    QuitRequest,
    /// An assistive client asked something of a surface's accessibility tree (`crate::a11y`):
    /// the host answers at the end of the turn this wakes.
    #[cfg_attr(not(feature = "accessibility"), allow(dead_code))]
    A11y(usize),
    /// A native menu's modal loop holds the event loop: run its idle turn (`Windows::turn`).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Turn,
    /// Windows: the GPU a CPU-drawn window was waiting for is made, or failed (`cpu_frame`).
    #[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
    GpuReady(usize),
    /// Windows: `WM_EXITSIZEMOVE` arrived on a window using `RUNE_RESIZE_SYNC` (`resize_sync`).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    ResizeSync(usize),
    /// Wayland: native `wl_data_device` drop events or pipe reads are ready for `Host::flush_files`.
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
    FileDrop,
    /// Wayland: a live layer surface already has events queued in libwayland from a mid-turn read;
    /// wakes the immediate next turn so `about_to_wait` drains it without entering `ControlFlow::Poll`.
    #[cfg(not(target_arch = "wasm32"))]
    LayerPending,
    /// Windows: system effects policy changed (Transparency effects, Energy/Battery Saver,
    /// `CompositionCapabilities`, High Contrast, or Remote Desktop); every host reconciles
    /// `El::system_material` native vs fallback state.
    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
    MaterialPolicyChanged,
}

/// What `Windows` needs of a host, whatever its state type: an app window or an overlay.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) trait AnyHost {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {}
    fn wake_deadline(&self) -> Option<Instant> { None }
    fn resumed(&mut self, event_loop: &ActiveEventLoop);
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent);
    fn owns(&self, id: WindowId) -> bool;
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent);
    /// Raw relative mouse motion from the OS device while pointer lock may be active.
    fn pointer_motion(&mut self, _dx: f64, _dy: f64) {}
    /// Draws what has no redraw event (a layer surface); true while it needs more frames.
    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) -> bool { false }
    /// Whether this host owns a live Wayland layer-shell surface sharing winit's `wl_display`.
    fn has_live_wayland_layer(&self) -> bool { false }
    /// Whether this host has unread platform events already queued in memory (for example a
    /// Wayland layer surface whose queue filled during a mid-turn read).
    fn has_pending_platform_events(&self) -> bool { false }
    /// The turn ends: the next frame it wants is asked for, or a paint put off runs (`redraw`).
    fn turn_ends(&mut self, _event_loop: &ActiveEventLoop, _now: Instant) {}
    fn finished(&self) -> bool { false }
    /// Whether this host has ever created a native window.
    fn has_shown(&self) -> bool { false }
    /// The error that finished this host when its first window couldn't be created.
    fn take_startup_error(&mut self) -> Option<String> { None }
    /// The OS is ending the app without Rune asking: run close hooks before the hosts drop.
    fn terminate(&mut self) {}
    /// Whether this live host has `App::on_quit_request` registered (overlays return false).
    fn has_quit_request(&self) -> bool { false }
    /// Asks `App::on_quit_request` inside a normal update transaction (overlays return None).
    fn quit_request_hook(&mut self, _event_loop: &ActiveEventLoop) -> Option<crate::QuitResponse> { None }
    /// The app starts to quit: runs `App::on_quit` (including in a window whose close ended the app).
    fn quit_hook(&mut self) {}
    /// The app quits: off the screen now, before any hook runs.
    fn vanish(&mut self) {}
    /// The close about to come ends the app (`Host::close`).
    fn closing_ends_app(&mut self) {}
    /// Ends a tracking pop-up menu and answers waiting ones `None`, before the host drops or
    /// the loop exits.
    fn cancel_menus(&mut self) {}
    /// Hard-stops an active or coasting macOS display link immediately when a panic is caught so
    /// no further display ticks fire while the loop unwinds.
    #[cfg(target_os = "macos")]
    fn stop_pacing(&mut self) {}
    /// False for hosts that must not keep the loop running alone (overlays).
    fn keeps_loop(&self) -> bool { true }
    /// An app window's key; overlays have none.
    fn key(&self) -> Option<&str> { None }
    /// How reopening the app sees it; overlays never come back.
    fn reopen_candidate(&self) -> crate::platform::reopen::Candidate { Default::default() }
    /// Runs `App::on_reopen` as an update; true when it handled the reopen.
    fn reopen_hook(&mut self, _event_loop: &ActiveEventLoop, _has_visible_windows: bool) -> bool { false }
    /// Applies what was asked of it through its handle since the last wake, now rather than
    /// when that wake arrives.
    fn apply_pending(&mut self, _event_loop: &ActiveEventLoop) {}
    /// Shows, restores and activates it as the default reopen.
    fn reopen(&mut self, _event_loop: &ActiveEventLoop) {}
    /// Syncs this host's native window theme with the current process-wide `app_color_scheme()`.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn apply_app_color_scheme(&mut self) {}
    /// Windows: reconciles `El::system_material` native vs fallback state after a system effects
    /// policy notification.
    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
    fn on_material_policy_changed(&mut self) {}
}

/// Collects startup creation errors from finished hosts and returns whether any loop-keeping host
/// remains alive. When another loop-keeping host stays alive or one has already created a window,
/// a finished host's startup error is logged to stderr; otherwise the first error is stored in
/// `startup_error` so `run` / `try_run` returns it.
#[cfg(not(target_arch = "wasm32"))]
fn collect_startup_errors(
    hosts: &mut [Option<Box<dyn AnyHost>>],
    any_shown: &mut bool,
    startup_error: &mut Option<String>,
) -> bool {
    *any_shown |= hosts.iter().flatten().any(|h| h.keeps_loop() && h.has_shown());
    let keeps_running = hosts.iter().flatten().any(|h| h.keeps_loop() && !h.finished());
    for host in hosts.iter_mut().flatten().filter(|h| h.finished()) {
        if let Some(error) = host.take_startup_error() {
            if keeps_running || *any_shown {
                eprintln!("rune: {error}");
            } else if startup_error.is_none() {
                *startup_error = Some(error);
            }
        }
    }
    keeps_running
}

/// Builds every host, attaching its shortcut and menu guards, before any resumes: a window's
/// first updates drain on resume and may register on a later window.
#[cfg(not(target_arch = "wasm32"))]
fn start_all<T, H>(items: Vec<T>, mut build: impl FnMut(usize, T) -> Option<H>, resume: impl FnMut(&mut H)) -> Vec<Option<H>> {
    let mut hosts: Vec<_> = items.into_iter().enumerate().map(|(slot, item)| build(slot, item)).collect();
    hosts.iter_mut().flatten().for_each(resume);
    hosts
}

type Factory = Box<dyn FnOnce(&[Display]) -> Vec<AnyWindowSpec>>;

#[cfg(not(target_arch = "wasm32"))]
struct Windows {
    factory: Option<Factory>,
    args: Args,
    proxy: EventLoopProxy<WakeEvent>,
    hosts: Vec<Option<Box<dyn AnyHost>>>,
    // Weak pool preserves release_gpu_when_closed when the last GPU owner closes.
    gpu: Rc<RefCell<Weak<Gpu>>>,
    /// True once any loop-keeping host has created a native window.
    any_shown: bool,
    /// First startup window creation error when no loop-keeping host stays alive or ever showed.
    startup_error: Option<String>,
    /// First panic caught at the macOS `ApplicationHandler` boundary, re-raised after `run_app` returns.
    #[cfg(target_os = "macos")]
    panic: panic_trap::PanicTrap,
    /// Fatal Wayland display error captured while `ActiveEventLoop` is still alive.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    display_error: Option<String>,
    /// Scheduling state for waking winit on Wayland display reads and draining mid-turn layer events.
    wayland_wake: wayland_wake::State,
    /// `App::on_display_close` hooks collected from started windows, run once on the UI thread
    /// before the event loop drops windows and tears down the native display.
    on_display_close: Vec<crate::app::OnDisplayClose>,
}
#[cfg(not(target_arch = "wasm32"))]
impl Windows {
    /// On macOS, catches panics before they cross winit's `extern "C"` AppKit delegate or view
    /// boundary, skips subsequent callbacks, and requests loop exit so `run` re-raises the
    /// original panic once `run_app` returns. Inside a nested tracking or modal run-loop mode
    /// where `event_loop.exit()` cannot return from `[NSApp run]`, logs the mode and re-raises
    /// immediately rather than hanging.
    #[cfg(target_os = "macos")]
    fn guard(&mut self, event_loop: &ActiveEventLoop, f: impl FnOnce(&mut Self)) {
        if self.panic.is_panicking() {
            return;
        }
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            for host in self.hosts.iter_mut().flatten() { host.stop_pacing(); }
            if let Some(mode) = panic_trap::nested_run_loop_mode() {
                eprintln!("rune: panic inside macOS nested run-loop mode {mode} cannot unwind out of run()");
                std::panic::resume_unwind(payload);
            }
            if self.panic.record(payload) {
                event_loop.exit();
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[inline(always)]
    fn guard(&mut self, _event_loop: &ActiveEventLoop, f: impl FnOnce(&mut Self)) {
        f(self);
    }
    /// Starts the windows `crate::open_window` asked for this turn, in fresh slots, each like
    /// a window the factory returned but without `--scene` or `--size`, which describe the
    /// launch's windows.
    fn open_requested(&mut self, event_loop: &ActiveEventLoop) {
        if self.factory.is_some() { return; }
        loop {
            let specs = crate::windows::flatten(crate::windows::take_opened());
            if specs.is_empty() { return; }
            let wayland = is_wayland(event_loop);
            for spec in specs {
                if self.hosts.iter().flatten().any(|h| h.key() == Some(spec.key())) {
                    eprintln!("rune: a window keyed {:?} is already open", spec.key());
                    continue;
                }
                let mut args = self.args.clone();
                (args.scene, args.size) = (None, None);
                let mut overlays = Vec::new();
                let slot = self.hosts.len();
                let host = spec.start(args, Start { proxy: self.proxy.clone(), slot, gpu: self.gpu.clone(), wayland, traced: false, overlays: &mut overlays, on_display_close: &mut self.on_display_close });
                self.hosts.push(host);
                if let Some(Some(host)) = self.hosts.get_mut(slot) { host.resumed(event_loop); }
                self.connect_overlays(overlays, event_loop);
            }
            app_events::sync_quit_request_hooks(&self.hosts);
        }
    }

    fn connect_overlays(&mut self, overlays: Vec<Box<dyn crate::overlay::PendingOverlay>>, event_loop: &ActiveEventLoop) {
        for mut overlay in overlays {
            let (slot, proxy) = (self.hosts.len(), self.proxy.clone());
            let frame_proxy = proxy.clone();
            let slot_proxy = proxy.clone();
            overlay.connect(Arc::new(move || { proxy.send_event(WakeEvent::App(slot)).ok(); }),
                Arc::new(move || { frame_proxy.send_event(WakeEvent::DisplayTick(slot)).ok(); }),
                Arc::new(move || { slot_proxy.send_event(WakeEvent::FrameSlot(slot)).ok(); }), self.gpu.clone());
            overlay.resumed(event_loop);
            self.hosts.push(Some(overlay));
        }
    }

    fn cleanup(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(target_os = "macos")]
        if self.panic.is_panicking() {
            return;
        }
        if !event_loop.exiting() { self.open_requested(event_loop); }
        let _ = collect_startup_errors(&mut self.hosts, &mut self.any_shown, &mut self.startup_error);
        let ok = self.startup_error.is_none();
        #[cfg(not(target_arch = "wasm32"))]
        let fast_exit = ok && crate::quit::fast_exit_hook().is_some();
        #[cfg(target_arch = "wasm32")]
        let fast_exit = false;
        let exiting = event_loop.exiting();
        let should_exit = app_events::cleanup_hosts(&mut self.hosts, &mut self.on_display_close, !exiting && ok, exiting, fast_exit, crate::quit::send);
        // Another window's quit: a menu still tracking would hold the loop open.
        if exiting || should_exit {
            for host in self.hosts.iter_mut().flatten() { host.cancel_menus(); }
        }
        if should_exit && !(ok && crate::quit::programmatic_quit()) {
            event_loop.exit();
        }
    }

    fn apply_wayland_wake(
        &self,
        event_loop: &ActiveEventLoop,
        effects: crate::machine::Effects<wayland_wake::Effect>,
    ) {
        for effect in effects {
            match effect {
                wayland_wake::Effect::SetWakeOnRead(on) => {
                    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                    winit::platform::wayland::ActiveEventLoopExtWayland::set_wake_on_wayland_read(event_loop, on);
                    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
                    let _ = (event_loop, on);
                }
                wayland_wake::Effect::SendWake => {
                    self.proxy.send_event(WakeEvent::LayerPending).ok();
                }
            }
        }
    }
}

/// What a window needs from the shell to start, whatever its state type.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Start<'a> {
    proxy: EventLoopProxy<WakeEvent>,
    slot: usize,
    gpu: Rc<RefCell<Weak<Gpu>>>,
    wayland: bool,
    /// Whether this window records `--native` traces.
    traced: bool,
    overlays: &'a mut Vec<Box<dyn crate::overlay::PendingOverlay>>,
    on_display_close: &'a mut Vec<crate::app::OnDisplayClose>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Start<'_> {
    fn inherit_args(&self, mut inherited: Args) -> Args {
        if inherited.native && !self.traced {
            inherited.trace = None;
            inherited.frames = None;
        }
        inherited
    }
}

/// Starts one window's host, or renders it headless when its own arguments ask for that.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn start<S: 'static>(mut spec: WindowSpec<S>, handle: Option<WindowHandle>, keeps_loop: bool, inherited: Args, cx: Start<'_>) -> Option<Box<dyn AnyHost>> {
    let args = spec.app.resolve_args(cx.inherit_args(inherited));
    #[cfg(not(any(feature = "tooling", debug_assertions)))]
    if args.headless() { crate::tooling::die(crate::tooling::WITHOUT_TOOLING); }
    #[cfg(any(feature = "tooling", debug_assertions))]
    if args.headless() {
        let scale = args.scale.unwrap_or(2.0);
        crate::tooling::finish_start(crate::tooling::headless_window_spec(spec.app, spec.options, args, scale));
        return None;
    }
    let incoming = crate::open_url::incoming::listen(&spec.app);
    Some(Box::new(Host::new(spec, handle, args, cx, keeps_loop, incoming)))
}

#[cfg(not(target_arch = "wasm32"))]
impl ApplicationHandler<WakeEvent> for Windows {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        self.guard(event_loop, |this| {
            let _handling = crate::quit::handling();
            for host in this.hosts.iter_mut().flatten() { host.new_events(event_loop, cause); }
            // Before resumed there are no hosts to clean up.
            if this.factory.is_none() { this.cleanup(event_loop); }
        });
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.guard(event_loop, |this| {
            let _handling = crate::quit::handling();
            let Some(factory) = this.factory.take() else { return };
            crate::marks::mark("loop.resumed");
            let wayland = is_wayland(event_loop);
            let displays = displays::collect(event_loop);
            let specs = crate::windows::flatten(factory(&displays));
            crate::windows::validate(&specs).expect("window keys");
            let trace_window = this.args.window.clone().or_else(|| specs.first().map(|s| s.key().to_owned()));
            let mut overlays = Vec::new();
            let mut on_display_close = Vec::new();
            this.hosts = start_all(specs, |slot, spec| {
                let traced = trace_window.as_deref() == Some(spec.key());
                spec.start(this.args.clone(), Start { proxy: this.proxy.clone(), slot, gpu: this.gpu.clone(), wayland, traced, overlays: &mut overlays, on_display_close: &mut on_display_close })
            }, |host| host.resumed(event_loop));
            this.on_display_close.extend(on_display_close);
            this.connect_overlays(overlays, event_loop);
            // A launch with no window makes its menu bar now; a window's first frame made it.
            #[cfg(target_os = "macos")]
            crate::native_menu::bar_ready();
            #[cfg(target_os = "macos")]
            {
                let proxy = this.proxy.clone();
                crate::platform::reopen::install(move || proxy.send_event(WakeEvent::Reopen).is_ok());
            }
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            {
                let proxy = this.proxy.clone();
                crate::open_url::instance::on_relaunch(move || proxy.send_event(WakeEvent::Reopen).is_ok());
            }
            #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
            {
                let proxy = this.proxy.clone();
                crate::system_material::windows::set_policy_waker(std::sync::Arc::new(move || {
                    proxy.send_event(WakeEvent::MaterialPolicyChanged).ok();
                }));
            }
            app_events::install_quit(&this.hosts, &this.proxy);
            crate::open_url::incoming::settle();
            // A window opened at launch took it; a GPU kept for a later one would sit idle until then.
            crate::render::discard_prewarm();
            this.cleanup(event_loop);
        });
    }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        self.guard(event_loop, |this| {
            let _handling = crate::quit::handling();
            let slot = match &event {
                WakeEvent::DisplayTick(i) | WakeEvent::App(i) | WakeEvent::Live(i) | WakeEvent::Window(i, _) | WakeEvent::Focus(i)
                    | WakeEvent::Position(i, _) | WakeEvent::Level(i, _) | WakeEvent::ExcludeFromCapture(i, _) | WakeEvent::ColorScheme(i, _) | WakeEvent::WindowBackdrop(i, _) | WakeEvent::Place(i, _) | WakeEvent::Frame(i, _)
                    | WakeEvent::A11y(i) | WakeEvent::FrameSlot(i) | WakeEvent::GpuReady(i) | WakeEvent::ResizeSync(i) => *i,
                WakeEvent::Reopen => {
                    this.reopen(event_loop);
                    this.cleanup(event_loop);
                    return;
                }
                // A session query (`ending: false`) can still be cancelled: the windows stay up.
                WakeEvent::Quit(ending) => {
                    this.quit(event_loop, *ending, *ending);
                    this.cleanup(event_loop);
                    return;
                }
                WakeEvent::QuitRequest => { this.quit_request(event_loop); this.cleanup(event_loop); return; }
                WakeEvent::Turn => { #[cfg(not(target_arch = "wasm32"))] this.turn(event_loop); return; }
                WakeEvent::FileDrop => return,
                WakeEvent::AppColorScheme => {
                    for host in this.hosts.iter_mut().flatten() {
                        host.apply_app_color_scheme();
                    }
                    return;
                }
                WakeEvent::LayerPending => {
                    let _ = wayland_wake::step(&mut this.wayland_wake, wayland_wake::Event::WakeReceived);
                    return;
                }
                #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
                WakeEvent::MaterialPolicyChanged => {
                    for host in this.hosts.iter_mut().flatten() {
                        host.on_material_policy_changed();
                    }
                    return;
                }
            };
            if let Some(Some(host)) = this.hosts.get_mut(slot) {
                host.user_event(event_loop, event);
            }
            this.cleanup(event_loop);
        });
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.guard(event_loop, |this| {
            let _handling = crate::quit::handling();
            // The last window's close ends a fast-exiting app: the window can go at once.
            #[cfg(not(target_arch = "wasm32"))]
            if matches!(event, WindowEvent::CloseRequested) && crate::quit::fast_exit_hook().is_some()
                && !this.hosts.iter().flatten().any(|h| !h.owns(id) && !h.finished() && h.keeps_loop())
                && let Some(host) = this.hosts.iter_mut().flatten().find(|h| h.owns(id) && h.keeps_loop())
            {
                host.closing_ends_app();
            }
            if let Some(host) = this.hosts.iter_mut().flatten().find(|h| h.owns(id)) {
                let _input = crate::input::time::arrived(&event);
                host.window_event(event_loop, id, event);
            }
            this.cleanup(event_loop);
        });
    }
    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        let winit::event::DeviceEvent::MouseMotion { delta: (dx, dy) } = event else { return };
        self.guard(event_loop, |this| {
            let _handling = crate::quit::handling();
            for host in this.hosts.iter_mut().flatten() {
                host.pointer_motion(dx, dy);
            }
        });
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.guard(event_loop, |this| {
            let _handling = crate::quit::handling();
            // Windows, Linux: a later launch handed to this loop (`single_instance`) is served once
            // the app goes on with a window shown. One that came as the app ended (its last window
            // already closing, the quit not started) isn't: the process starts the app again for it.
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            if crate::open_url::instance::launches_waiting() && !event_loop.exiting() {
                let windows: Vec<_> = this.hosts.iter().map(|h| h.as_ref().map(|h| h.reopen_candidate()).unwrap_or_default()).collect();
                if crate::platform::reopen::has_visible(&windows) {
                    crate::open_url::instance::served();
                }
            }
            crate::marks::loop_waits();
            #[cfg(target_os = "macos")]
            crate::native_menu::settle_standard_menus();
            #[cfg(target_os = "macos")]
            crate::platform::macos::launch::settle();
            crate::cursor::os::make_pending(event_loop);
            let mut animating = false;
            for host in this.hosts.iter_mut().flatten() {
                animating |= host.about_to_wait(event_loop);
            }
            // A window an idle hook closed (`App::on_window_geometry`) goes before the loop sleeps.
            if this.hosts.iter().flatten().any(|host| host.finished()) { this.cleanup(event_loop); }
            // The frames windows want next (`redraw`), and on Windows the paints the system keeps
            // putting off behind another window's.
            let now = crate::time::now();
            for host in this.hosts.iter_mut().flatten() { host.turn_ends(event_loop, now); }
            let effects = wayland_wake::about_to_wait(&mut this.wayland_wake, &this.hosts);
            this.apply_wayland_wake(event_loop, effects);
            // Layer surfaces have no redraw events: keep turning while one animates (its vsync
            // paces the loop). Hosts whose only motion is an ambient loop sleep until its next step.
            let deadline = this.hosts.iter().flatten().filter_map(|host| host.wake_deadline()).min();
            event_loop.set_control_flow(if animating { ControlFlow::Poll }
                else { deadline.map_or(ControlFlow::Wait, |at| ControlFlow::WaitUntil(crate::time::for_winit(at))) });
        });
    }
    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        // A panic caught at the macOS callback boundary skips quit hooks, close hooks and fast exit
        // (matching Windows and Linux where `exiting` does not run on panic), then unwinds from
        // `run_configured`. `exiting` itself stays unguarded because `applicationWillTerminate:`
        // never returns to `run_app`.
        #[cfg(target_os = "macos")]
        if self.panic.is_panicking() {
            return;
        }
        let _handling = crate::quit::handling();
        crate::marks::mark("loop.exiting");
        #[cfg(not(target_arch = "wasm32"))]
        crate::render::cancel_prewarm();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.display_error.is_none()
            && let Ok(display) = winit::raw_window_handle::HasDisplayHandle::display_handle(event_loop)
        {
            self.display_error = crate::platform::layer_shell::display_error(display.as_raw());
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let has_display_error = self.display_error.is_some();
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        let has_display_error = false;
        #[cfg(not(target_arch = "wasm32"))]
        let fast_hook = crate::quit::fast_exit_hook();
        #[cfg(target_arch = "wasm32")]
        let fast_hook: Option<fn()> = None;
        let step = app_events::exiting_hosts(
            &mut self.hosts,
            &mut self.on_display_close,
            app_events::ExitFlags {
                has_startup_error: self.startup_error.is_some(),
                has_display_error,
                loop_exiting: event_loop.exiting(),
                quitting: crate::quit::quitting(),
                fast_exit: fast_hook.is_some(),
            },
            crate::quit::send,
        );
        #[cfg(not(target_arch = "wasm32"))]
        if step == app_events::ExitStep::FastExit && let Some(last) = fast_hook {
            crate::quit::end_fast(last);
        }
        let effects = wayland_wake::step(&mut self.wayland_wake, wayland_wake::Event::Exiting);
        self.apply_wayland_wake(event_loop, effects);
        // Surfaces and tray icons go while winit's display connection is still open (layer
        // surfaces live on it) and before `run_app` returns to `shutdown_prewarm()`, so nothing
        // stays visible on screen if a background GPU warm-up thread is slow or hung.
        app_events::finish_orderly_exiting(
            &mut self.hosts,
            matches!(step, app_events::ExitStep::Orderly { relaunch: true }),
            || {
                #[cfg(not(target_arch = "wasm32"))]
                crate::tray::remove_all();
            },
            || {
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                {
                    crate::platform::wayland::drop::shutdown();
                    crate::platform::wayland::backdrop::shutdown();
                }
                crate::marks::mark("windows.dropped");
                crate::clipboard::shutdown();
            },
            || crate::relaunch::spawn(),
            || {
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                crate::open_url::instance::exited();
            },
        );
    }
}

/// What a shown window that can't draw runs for an update, a wake, a due timer or a redraw:
/// covered (an occlusion event, or a drawable refused as occluded), minimised, or waiting out
/// a missed frame. `poll`, due timers and queued input run now, as for a hidden window
/// (`Runtime::hidden_wake`); the view and layout wait, dirty, for the frame that uncovers it,
/// even with an assistive client attached, except on an assistive action, stale first activation,
/// or a programmatic focus request (`Runtime::covered_wake`).
/// Uncovering settles first, so that frame builds once and paints motion where it would have
/// ended, as Core Animation's time-based motion has by the time a covered window shows again.
#[cfg(test)]
#[inline(always)]
pub(crate) fn covered_step<S: 'static>(rt: &mut Runtime<S>) { rt.covered_wake(); }

#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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

/// Tells `rt` where `window`'s content is on screen: in points for `El::on_screen_layout`
/// ([`screen_origin`]), and in desktop pixels for `Drag::screen_point`. True when that moved
/// an `El::on_screen_layout` element.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn sync_screen_origin_core(rt: &mut RuntimeCore, window: &Window) -> bool {
    rt.set_window_pixels(window.inner_position().ok().map(|p| (p.x, p.y)));
    screen_origin(window).is_some_and(|o| rt.set_window_origin(o))
}

/// The scale an overlay draws at. On macOS, read from the panel AppKit shows, and before it's
/// ordered in from the display holding most of its frame, where it will appear.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn backing_scale(window: &Window) -> f32 {
    #[cfg(target_os = "macos")]
    if let Some(scale) = crate::platform::macos::backing_scale(window) { return scale; }
    window.scale_factor() as f32
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

#[cfg(not(target_arch = "wasm32"))]
fn gesture_phase(phase: TouchPhase) -> crate::GesturePhase {
    match phase {
        TouchPhase::Started => crate::GesturePhase::Began,
        TouchPhase::Moved => crate::GesturePhase::Moved,
        TouchPhase::Ended | TouchPhase::Cancelled => crate::GesturePhase::Ended,
    }
}

/// Hands a winit touch (a finger, or a pen where the system reports it as one) to the runtime
/// in the view's points at `time` (seconds on a steady clock; on Web, mapped onto the runtime
/// clock from `PointerEvent.timeStamp`).
pub(crate) fn touch_core_at(rt: &mut RuntimeCore, app: &mut dyn AppDriver, touch: &winit::event::Touch, time: f64) {
    let scale = rt.input_scale();
    let phase = match touch.phase {
        TouchPhase::Started => crate::TouchPhase::Started,
        TouchPhase::Moved => crate::TouchPhase::Moved,
        TouchPhase::Ended => crate::TouchPhase::Ended,
        TouchPhase::Cancelled => crate::TouchPhase::Cancelled,
    };
    rt.touch(app, touch.id, phase, touch.location.x as f32 / scale, touch.location.y as f32 / scale, time);
}

/// Hands a winit touch to the runtime, timed on a clock that starts with the first touch.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn touch_core(rt: &mut RuntimeCore, app: &mut dyn AppDriver, touch: &winit::event::Touch) {
    static START: std::sync::OnceLock<crate::time::Instant> = std::sync::OnceLock::new();
    let time = START.get_or_init(crate::time::now).elapsed().as_secs_f64();
    touch_core_at(rt, app, touch, time);
}
