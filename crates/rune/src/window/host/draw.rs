//! Drawing a window's frame: the drawable, the quiet timer frame that draws nothing new, and
//! the frames a covered or occluded window steps without drawing.

use std::sync::atomic::Ordering;

use super::Host;
use super::render::{
    FrameProfile, PreparedWindowFrame, open_trace_if_needed, prepare_window_render,
    record_trace_frame, render_window_frame, submit_and_present_window,
};
use super::stepping::FrameMove;
use crate::frame::clock::FrameClock;
use crate::frame::host_step::{
    self, Acquire, FrameHost, Gate, PostStepCheck, ReacquireReason, RenderPrep, StepInfo,
};
use crate::runtime::RuntimeCore;
use crate::time::Instant;
use crate::window::chrome::sync_platform_core;
use crate::window::handle::native_window_number;
use crate::window::state::{Cover, RestEvent, rest};
use crate::window::surface::{
    Backoff, BackoffEffect, BackoffEvent, Missed, Presented, SurfaceRetries, Win, acquire,
    reblend_core,
};
#[cfg(target_os = "macos")]
use crate::window::surface::{RedrawEvent, TIMER_FRAME, configure};
#[cfg(target_os = "windows")]
use crate::window::win32::{covered_draw::CoveredDrawEvent, slot};
#[cfg(target_os = "windows")]
use crate::window::WakeEvent;

/// The first frame's marks (`crate::marks`), for the first window to draw.
static FIRST_BUILD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The first frame the swap chain called occluded (a window DXGI won't present, as on a desktop
/// no one sees): the window then waits `OCCLUDED_RETRY` before it tries again.
static FIRST_OCCLUDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Window-neutral miss state passed to [`apply_missed_core`] by [`Host::missed_frame`] and
/// unit tests.
struct MissState<'a> {
    app: &'a mut dyn super::WindowApp,
    clipboard: &'a crate::clipboard::Client,
    reports: &'a mut crate::frame::encoded::FrameReports,
    presented: &'a mut Option<Presented>,
    backoff: &'a mut Backoff,
    surface_ok: &'a mut bool,
}

/// Window-neutral miss accounting shared by [`Host::missed_frame`] and unit tests: clears the
/// cached presented frame, marks the surface unconfigured when needed, advances backoff, steps
/// `covered_wake` iff this frame has not already stepped, flushes the clipboard, and reports a
/// skipped frame.
fn apply_missed_core(
    state: MissState<'_>,
    missed: Missed,
    paced: bool,
    stepped: bool,
    now: Instant,
) -> crate::machine::Effects<BackoffEffect> {
    if missed == Missed::Unconfigured { *state.surface_ok = false; }
    *state.presented = None;
    let effects = state.backoff.step(BackoffEvent::Missed { missed, paced, now });
    // What this frame would have run still runs, without drawing (`covered_step`). Misses
    // come at the retry's pace, not the display's.
    if !stepped { state.app.covered_wake(); }
    state.clipboard.flush_core(state.app.rt_mut());
    crate::haptics::flush_live(state.app.rt_mut(), false);
    state.reports.skipped();
    effects
}

/// Outcome of settling a frame whose GPU device was lost during submission or presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LostPresent {
    Retry,
    GiveUp,
}

/// Window-neutral accounting when `gpu.is_lost()` is already true after `submit_and_present`:
/// invalidates the surface and cached presentation, marks the host idle, reports a skipped frame,
/// and either arms a bounded backoff retry or reports `GiveUp` once `SurfaceRetries` is exhausted.
#[allow(clippy::too_many_arguments)]
fn apply_lost_present_core(
    surface_retries: &SurfaceRetries,
    policy: crate::gpu::fallback::Policy,
    software_available: bool,
    surface_ok: &mut bool,
    presented: &mut Option<Presented>,
    backoff: &mut Backoff,
    reports: &mut crate::frame::encoded::FrameReports,
    idle: &mut bool,
    now: Instant,
) -> LostPresent {
    *surface_ok = false;
    *presented = None;
    *idle = true;
    reports.skipped();
    match surface_retries.loss_retry(now, policy, software_available) {
        Some(at) => {
            let _ = backoff.step(BackoffEvent::At(at));
            LostPresent::Retry
        }
        None => LostPresent::GiveUp,
    }
}

impl Host {
    /// A shown frame with no drawable. Asking for a redraw here would turn the loop at once, for
    /// as long as the window is occluded or its displays sleep, so it waits out a backoff
    /// (`Backoff`), with its display link stopped so no tick redraws sooner. `stepped` when this
    /// frame already ran the runtime.
    pub(in crate::window) fn missed_frame(&mut self, missed: Missed, stepped: bool) {
        if self.poll_device_for_terminal_loss() { return; }
        #[cfg(feature = "frame-profile")]
        crate::frame::live_log::missed(match missed { Missed::Occluded => 0, Missed::Timeout => 1, Missed::Outdated => 2, Missed::Unconfigured => 3 });
        let Some(l) = self.win.as_mut() else { return };
        l.timer_frame.drawn();
        let now = crate::time::now();
        let occluded = missed == Missed::Occluded;
        if occluded {
            crate::marks::mark_once(&FIRST_OCCLUDED, "frame.occluded");
            // Covered or asleep with no notification yet: rest as if one came, and look
            // again when the backoff is up.
            self.life.cover(Cover::Surface, true);
            l.first_frame.skipped(now);
            #[cfg(target_os = "macos")]
            l.set_live_resize(false);
            self.secure_input.set(false);
            self.idle = true;
        }
        let paced = l.paced();
        let effects = apply_missed_core(
            MissState {
                app: &mut *self.app,
                clipboard: &self.clipboard,
                reports: &mut self.frame_reports,
                presented: &mut l.presented,
                backoff: &mut l.backoff,
                surface_ok: &mut l.surface_ok,
            },
            missed,
            paced,
            stepped,
            now,
        );
        for effect in effects {
            match effect {
                BackoffEffect::StopPacing => l.stop_pacing(),
            }
        }
        self.meter.set_pacing(l.pacing_state(!occluded));
        if occluded { self.app.set_live_visible(false); }
    }

    /// A frame for a window that's hidden, covered, asleep or minimised: while shown, what it
    /// would have run runs without drawing (`covered_step`). It isn't live: a minimize X11
    /// reports with no event lands here first, and the frame that next draws settles.
    pub(in crate::window) fn covered_frame(&mut self) {
        let Some(l) = self.win.as_mut() else { return };
        l.timer_frame.drawn();
        l.presented = None;
        l.stop_pacing();
        let shown = self.life.shown();
        l.first_frame.skipped(crate::time::now());
        // Covered by other windows, it still draws what changed, later (`CoveredDraw`).
        #[cfg(target_os = "windows")]
        if shown && self.life.covered() && !l.minimized() { l.covered_draw = l.covered_draw.step(CoveredDrawEvent::Asked(crate::time::now())); }
        self.land_frame();
        if shown {
            self.app.set_live_visible(false);
            self.app.covered_wake();
            self.clipboard.flush_core(self.app.rt_mut());
            self.frame_reports.skipped();
        }
        crate::haptics::flush_live(self.app.rt_mut(), false);
    }

    pub(in crate::window) fn redraw(&mut self) { self.draw(false) }

    /// Draws a frame. `resizing` draws one for a size the user just dragged the window to, at
    /// once: AppKit shows the new size when the event returns, with or without it.
    pub(in crate::window) fn draw(&mut self, resizing: bool) {
        if self.poll_gpu_loss() { return; }
        // Windows: a window still on the CPU renderer draws there (`cpu_frame`).
        #[cfg(target_os = "windows")]
        {
            self.look();
            self.hand_over_if_done();
            if self.win.as_ref().is_some_and(|w| w.native.mode.cpu_draws()) { return self.draw_cpu(resizing); }
            if self.try_draw_drag_resize() { return; }
        }
        // Linux: so does one there (`linux::cpu_frame`).
        #[cfg(target_os = "linux")]
        if self.win.as_ref().is_some_and(|w| w.native.mode.cpu_draws()) { return self.draw_cpu(resizing); }
        let mut frame = WindowFrame { host: self, resizing, state: FrameState::default() };
        host_step::run(&mut frame);
    }
}

#[derive(Default)]
struct FrameState {
    hidden: bool,
    live_resize: bool,
    corners: f32,
    target: Option<Instant>,
    #[cfg(target_os = "macos")]
    prewarm: Option<crate::platform::macos::Prewarm>,
    #[cfg(target_os = "macos")]
    deferred: bool,
    #[cfg(target_os = "macos")]
    shown: Option<crate::platform::macos::PresumeShown>,
    #[cfg(target_os = "macos")]
    drawable: Option<u64>,
    #[cfg(any(target_os = "macos", all(target_os = "windows", feature = "windows-system-material")))]
    materials_checked: bool,
    #[cfg(target_os = "macos")]
    frame_time: Option<Instant>,
    only_loops: bool,
    looped: Option<Vec<u32>>,
    profile: FrameProfile,
    post_acquire_done: bool,
    now: Option<Instant>,
    encoded: Option<Instant>,
}

struct WindowFrame<'a> {
    host: &'a mut Host,
    resizing: bool,
    state: FrameState,
}

impl WindowFrame<'_> {
    fn reblend_surface(&mut self) -> bool {
        let (Some(l), Some((gpu, renderer))) = (self.host.win.as_mut(), self.host.gpu.as_mut()) else { return false };
        let Some(surface) = l.surface.as_ref() else { return false };
        let ok = reblend_core(surface, gpu, &mut l.config, renderer, &mut l.alpha, self.host.app.rt_mut());
        #[cfg(target_os = "windows")]
        l.native.slot.reconfigured(ok);
        ok
    }
}

impl FrameHost for WindowFrame<'_> {
    type Drawable = wgpu::SurfaceTexture;
    type Prepared = PreparedWindowFrame;

    fn gate(&mut self) -> Gate {
        if self.host.poll_gpu_loss() { return Gate::Skip; }
        let launcher = self.host.launcher();
        let Some(l) = self.host.win.as_ref() else { return Gate::Skip };
        let hidden = launcher && !self.host.life.shown();
        let covered = (!launcher && !self.host.life.uncovered()) || l.minimized();
        #[cfg(target_os = "windows")]
        let owed = covered && !l.minimized() && l.covered_draw.drawing();
        #[cfg(not(target_os = "windows"))]
        let owed = false;
        if covered && !owed {
            // A covered window's clips stop presenting on their layers, as they stop being pulled.
            #[cfg(target_os = "windows")]
            if let Some(w) = self.host.win.as_mut() {
                let native = &mut w.native;
                native.video_layers.release(native.mode.handover());
            }
            self.host.covered_frame();
            return Gate::Skip;
        }
        // A covered window's owed frame shows the settled state, as its first one uncovered does.
        if owed { self.host.app.rt_mut().settle_all(); }
        // A missed frame's retry, or the timer pacing unpaced motion: input waits for it, and
        // `new_events` runs due timers meanwhile.
        if !self.resizing && l.backing_off(crate::time::now()) { return Gate::Skip; }
        // A device that died (a driver reset, a GPU removed, a resume from Modern Standby) and a
        // surface that refused its configuration can draw nothing: the window makes both again
        // before this frame, or waits out a retry of its own (`SurfaceRetries`) and then gives
        // them up. Neither can give a drawable, and wgpu panics asking one for it.
        if !self.host.replace_lost_surface() { return Gate::Skip; }
        // The radius a client-side frame rounds this window's bottom corners to, in device
        // pixels: zero unless it draws one, now and with a surface that composites alpha
        // (`corners`). Asked every frame, so a maximize or a tile that arrives with a configure
        // squares the next one.
        self.state.corners = self.host.win.as_ref().map_or(0., |l| self.host.corner_radius(l));
        // A clip's layer began or stopped showing: the UI's swap chain composites premultiplied
        // only while one does (`video_layer`), made again before this frame draws.
        #[cfg(target_os = "windows")]
        if let Some(premultiplied) = self.host.win.as_mut().and_then(|w| w.native.video_layers.take_alpha())
            && let Err(error) = self.host.rebind_alpha(premultiplied)
        {
            eprintln!("rune: the window's swap chain couldn't change its alpha for a clip's layer: {error}");
        }
        // The swap chain still holds a frame the compositor hasn't shown: this one draws when it
        // has room, built from the input that came by then (`slot`).
        #[cfg(target_os = "windows")]
        if !hidden && let Some(l) = self.host.win.as_mut() && let Some(surface) = l.surface.as_ref() {
            let handle = self.host.native_handle.clone();
            if l.native.slot.take_surface(surface, move || handle.send(WakeEvent::FrameSlot)) == slot::Take::Later { return Gate::Skip; }
        }
        let Some(l) = self.host.win.as_mut() else { return Gate::Skip };
        #[cfg(target_os = "windows")]
        if owed { l.covered_draw = l.covered_draw.step(CoveredDrawEvent::Drawn); }
        // The frame a due timer asked for is here, drawing or running covered below.
        l.timer_frame.drawn();
        // Starting the display link for a window that isn't on screen would wait for a tick that
        // sleeping displays never send, and a running link holds back its timers (`wake_deadline`).
        #[cfg(target_os = "macos")]
        if !hidden {
            if !l.natively_occluded() {
                l.native.prime.stop();
            } else if !l.native.prime.presumes() {
                self.host.meter.skipped();
                self.host.missed_frame(Missed::Occluded, false);
                return Gate::Skip;
            }
        }
        #[cfg(target_os = "macos")]
        if hidden { self.state.prewarm = l.native.panel.as_ref().map(|p| p.prewarm()); }
        // Only a timer woke the resting window, so its frame may draw what's on screen. It
        // decides that before starting the display link, which would draw it at once anyway.
        #[cfg(target_os = "macos")]
        let deferred = l.timer_frame.quiet() && !hidden && !self.resizing && !l.native.prime.presumes()
            && l.native.pacing.as_ref().is_some_and(|p| p.defers_start(crate::time::now()));
        #[cfg(not(target_os = "macos"))]
        let deferred = false;
        #[cfg(target_os = "macos")]
        let target = if hidden || self.resizing || deferred { None } else if let Some(pacing) = &mut l.native.pacing {
            if l.native.prime.presumes() && !pacing.paced() {
                None
            } else {
                let Some(target) = pacing.next_frame().target() else { return Gate::Skip };
                target
            }
        } else { None };
        #[cfg(target_os = "macos")]
        let shown = if !hidden && l.native.prime.presumes() {
            // A frame drawn while AppKit still launches the app isn't counted: AppKit says
            // nothing of the window until its own launch work is done, and the frames right
            // after it presume as those of a window made after the launch do.
            if !crate::platform::macos::launching() { l.native.prime.spend(); }
            if let Some(surface) = l.surface.as_ref()
                && let Some(shown) = crate::platform::macos::PresumeShown::new(surface) {
                Some(shown)
            } else {
                self.host.meter.skipped();
                self.host.missed_frame(Missed::Occluded, false);
                return Gate::Skip;
            }
        } else {
            None
        };
        if self.host.native_handle.native.number.load(Ordering::Relaxed) == 0 {
            self.host.native_handle.native.number.store(native_window_number(&l.window).unwrap_or(0), Ordering::Release);
        }
        #[cfg(not(target_os = "macos"))]
        let target: Option<Instant> = None;
        if !hidden { self.host.meter.redraw(l.paced_for_stats() || deferred); }
        #[cfg(target_os = "macos")]
        let live_resize = !hidden && (self.resizing || crate::platform::macos::in_live_resize(&l.window));
        #[cfg(not(target_os = "macos"))]
        let live_resize = self.resizing;
        self.state.hidden = hidden;
        self.state.live_resize = live_resize;
        self.state.target = target;
        #[cfg(target_os = "macos")]
        {
            self.state.deferred = deferred;
            self.state.shown = shown;
        }
        if self.host.gpu.is_none() { return Gate::Skip; }
        self.state.profile.mark_entry();
        let (Some(l), Some((_, renderer))) = (self.host.win.as_mut(), self.host.gpu.as_ref()) else { return Gate::Skip };
        // Only the window's loops asked for this frame (`LoopFrame`): a step of them may stand
        // for the whole of it (`Runtime::loop_step`), drawn only where they draw (`arm_loop`).
        let only_loops = l.loop_frame.only_loops();
        if !only_loops { self.state.profile.set_loop_why("something reached the window"); }
        // The app's waker ran and its event is still on its way: what it brought may be readable now.
        let only_loops = only_loops && !self.host.app_woken.load(Ordering::Acquire);
        if !only_loops { self.state.profile.set_loop_why_if_none("an app wake on its way"); }
        #[cfg(target_os = "windows")]
        let only_loops = only_loops && !hidden && !live_resize && self.host.trace.is_none() && self.host.args.trace.is_none()
            && self.host.app.rt().host.window_frame.is_none() && !FrameMove::moving(&self.host.frame_move) && !self.host.app.compositor_animating();
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        let only_loops = only_loops && !l.native.materials.native();
        #[cfg(not(target_os = "windows"))]
        let only_loops = only_loops && false;
        self.state.only_loops = only_loops;
        // Only a timer woke the resting window: build the frame first, and take no drawable
        // when it draws what's on screen.
        let quiet = l.timer_frame.take_quiet() && !hidden && !live_resize && l.presented.is_some()
            && !self.host.app.compositor_draws_own()
            && self.host.trace.is_none() && self.host.args.trace.is_none()
            && self.host.app.rt().live_visible()
            && self.host.app.rt().host.window_frame.is_none() && !FrameMove::moving(&self.host.frame_move)
            && renderer.srgb_blend() == self.host.app.rt().srgb_blend();
        #[cfg(target_os = "macos")]
        let quiet = quiet && self.state.shown.is_none() && !l.native.materials.native();
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        let quiet = quiet && !l.native.materials.native();
        if quiet {
            #[cfg(target_os = "macos")]
            l.set_live_resize(false);
            l.backoff_step(BackoffEvent::Lapsed);
            #[cfg(target_os = "macos")]
            { self.host.app.rt_mut().paint.set_native_materials(l.native.materials.native()); }
            #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
            { self.host.app.rt_mut().paint.set_windows_acrylic_materials(l.native.materials.native()); }
            if self.host.frame_reports.wanted() { self.host.frame_reports.set_refresh(display_hz(l)); }
            Gate::QuietTimer
        } else {
            #[cfg(target_os = "macos")]
            if self.state.deferred && let Some(pacing) = &mut l.native.pacing { let _ = pacing.next_frame(); }
            Gate::Draw
        }
    }

    fn before_acquire(&mut self) {
        // The frame moves before the drawable is acquired, so it has the new size; never while
        // the user holds the window's edge in a live resize.
        if !self.state.hidden && !self.state.live_resize {
            self.host.step_frame(self.state.target.unwrap_or_else(crate::time::now));
        }
        let Some(l) = self.host.win.as_mut() else { return };
        #[cfg(target_os = "macos")]
        l.set_live_resize(self.resizing && self.state.live_resize);
        // Acquire before latching live values so a drawable wait cannot age the input.
        l.backoff_step(BackoffEvent::Lapsed);
        #[cfg(target_os = "macos")]
        { self.host.app.rt_mut().paint.set_native_materials(l.native.materials.native()); }
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        { self.host.app.rt_mut().paint.set_windows_acrylic_materials(l.native.materials.native()); }
        if self.host.frame_reports.wanted() { self.host.frame_reports.set_refresh(display_hz(l)); }
    }

    fn needs_reblend(&self) -> bool {
        self.host.gpu.as_ref().is_some_and(|(_, renderer)| renderer.srgb_blend() != self.host.app.rt().srgb_blend())
    }

    fn reblend(&mut self) -> bool { self.reblend_surface() }

    fn step_quiet(&mut self) -> StepInfo {
        let now = crate::time::now();
        self.state.now = Some(now);
        self.host.app.rt_mut().host.window_resting = true;
        let at = self.state.target.unwrap_or(now);
        #[cfg(target_os = "macos")]
        { self.state.frame_time = Some(at); }
        let step = if self.state.only_loops { loop_step(&mut self.host.clock, self.host.app.rt_mut(), at) } else { Err("") };
        let (animating, dt) = match step {
            Ok((patched, dt)) => {
                if !patched.is_empty() {
                    self.state.looped = Some(patched);
                }
                (self.host.app.rt().animating, dt)
            }
            Err(why) => {
                if !why.is_empty() { self.state.profile.set_loop_why(why); }
                self.host.app.clock_advance(&mut self.host.clock, at, false)
            }
        };
        self.state.profile.take_loop_check(self.host.app.rt_mut());
        let _ = self.host.poll_gpu_loss();
        StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.0, presents_at: self.state.target }
    }

    fn is_unchanged(&mut self, info: &StepInfo) -> bool {
        let (Some(l), Some((_, renderer))) = (self.host.win.as_ref(), self.host.gpu.as_ref()) else { return false };
        #[cfg(target_os = "windows")]
        if l.native.video_layers.needs_frame() { return false; }
        let now = self.state.now.unwrap_or_else(crate::time::now);
        #[cfg(target_os = "macos")]
        let next_tick = if self.state.deferred { l.native.pacing.as_ref().map(|p| p.tick_after(now)) } else { l.next_tick(now) };
        #[cfg(not(target_os = "macos"))]
        let next_tick = l.next_tick(now);
        self.state.looped.is_none()
            && renderer.srgb_blend() == self.host.app.rt().srgb_blend()
            && l.presented.as_ref().is_some_and(|p| p.unchanged_core(self.host.app.rt(), &l.config, info.animating, next_tick))
    }

    fn on_unchanged(&mut self, _info: &StepInfo) {
        let Some(l) = self.host.win.as_mut() else { return };
        sync_platform_core(l, self.host.app.rt_mut(), &mut self.host.secure_input, &mut self.host.ime, self.host.ime_waits, &self.host.clipboard);
        crate::haptics::flush_live(self.host.app.rt_mut(), !self.state.hidden);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        self.host.sync_wayland_drop();
        let Some(l) = self.host.win.as_mut() else { return };
        #[cfg(target_os = "macos")]
        l.redraw_step(RedrawEvent::DrewOutsidePaint);
        // When `deferred` is true, the link was already `Stopped` or `Coasting`: leave a coasting
        // countdown running without aborting or extending it.
        #[cfg(target_os = "macos")]
        if !self.state.deferred { l.stop_pacing(); }
        #[cfg(not(target_os = "macos"))]
        l.stop_pacing();
        self.host.meter.set_pacing(l.pacing_state(false));
        self.host.frame_reports.unchanged(self.state.target);
    }

    fn on_quiet_will_draw(&mut self) {
        // It draws: the link starts, and still says draw at once.
        #[cfg(target_os = "macos")]
        if self.state.deferred && let Some(l) = self.host.win.as_mut() && let Some(pacing) = &mut l.native.pacing {
            let _ = pacing.next_frame();
        }
        crate::marks::mark_once(&FIRST_BUILD, "frame.first.build");
    }

    fn acquire(&mut self) -> Acquire<wgpu::SurfaceTexture> {
        if self.host.poll_gpu_loss() { return Acquire::Missed(Missed::Unconfigured); }
        self.state.profile.mark_start();
        let (Some(l), Some((gpu, _))) = (self.host.win.as_mut(), self.host.gpu.as_ref()) else {
            return Acquire::Missed(Missed::Unconfigured);
        };
        let Some(surface) = l.surface.as_ref() else { return Acquire::Missed(Missed::Unconfigured) };
        let frame = match acquire(surface, gpu, &l.config, l.surface_ok, 1, &mut self.host.meter) {
            Ok(frame) => {
                self.host.terminal_loss.note_frame_texture_acquired_direct();
                frame
            }
            Err(missed) => return Acquire::Missed(missed),
        };
        self.state.profile.mark_acquired();
        if !self.state.post_acquire_done {
            self.state.post_acquire_done = true;
            // A covered window's owed frame (`CoveredDraw`) draws settled and stays covered: live, its
            // loops would ask for frames no one sees.
            if !self.state.hidden && !self.host.life.covered() && !self.host.app.rt().live_visible()
                && { let size = l.window.inner_size(); size.width > 0 && size.height > 0 }
            {
                // Uncovered after a missed frame said it was occluded.
                self.host.app.set_live_visible(true);
                self.host.app.rt_mut().settle_all();
                #[cfg(target_os = "macos")]
                if l.native.pacing.is_none() && !self.host.pacing_waits {
                    l.native.pacing = crate::platform::display_link::DisplayLink::new(&l.window, self.host.frame_wake.clone())
                        .map(|p| p.with_ambient_period(self.host.ambient_period));
                }
            }
            let now = *self.state.now.get_or_insert_with(crate::time::now);
            open_trace_if_needed(self.host, now);
            // `--window-script` times its events from this, the window's first drawn frame.
            if let Some(script) = &mut self.host.window_script { script.start(now); }
        }
        #[cfg(target_os = "macos")]
        if let Some(l) = self.host.win.as_ref() {
            self.state.drawable = l.native.present.as_ref().and(self.host.present_log.as_ref()).and_then(|log| log.acquired());
        }
        Acquire::Ready(frame)
    }

    fn step_runtime(&mut self) -> StepInfo {
        let now = *self.state.now.get_or_insert_with(crate::time::now);
        let frame_time = self.state.target.unwrap_or(now);
        #[cfg(target_os = "macos")]
        { self.state.frame_time = Some(frame_time); }
        self.host.app.rt_mut().host.window_resting = !FrameMove::moving(&self.host.frame_move) && self.host.app.rt().host.window_frame.is_none();
        crate::marks::mark_once(&FIRST_BUILD, "frame.first.build");
        if self.state.hidden && self.host.win.as_ref().is_some_and(|l| l.first_frame.settles()) {
            self.host.app.rt_mut().settle_all();
        }
        let stepped = if self.state.only_loops { loop_step(&mut self.host.clock, self.host.app.rt_mut(), frame_time) } else { Err("") };
        if let Err(why) = stepped && !why.is_empty() { self.state.profile.set_loop_why(why); }
        let (animating, dt) = if self.state.hidden {
            self.host.clock.hold(frame_time);
            (self.host.app.frame(0.0), 0.0)
        } else if let Ok((patched, dt)) = stepped {
            if !patched.is_empty() {
                self.state.looped = Some(patched);
            }
            (self.host.app.rt().animating, dt)
        } else {
            let compositing = self.host.app.compositor_animating();
            self.host.app.clock_advance(&mut self.host.clock, frame_time, compositing)
        };
        // `RUNE_LOOP_CHECK=1`: how the loop-only step this frame stood in for compares with it.
        self.state.profile.take_loop_check(self.host.app.rt_mut());
        let _ = self.host.poll_gpu_loss();
        StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.0, presents_at: self.state.target }
    }

    fn post_step_check(&mut self, info: &mut StepInfo) -> PostStepCheck {
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        let _ = info;
        // frame-profile, `RUNE_MISS_LOOP_FRAMES=<n>`: every nth loop-only frame ends here as if its
        // drawable never came, after the step: what the next frame redraws (with
        // `RUNE_LOOP_CHECK=2`, whether its picture is right).
        #[cfg(feature = "frame-profile")]
        if !self.state.profile.forced_miss_checked {
            self.state.profile.forced_miss_checked = true;
            if self.state.looped.is_some() && crate::window::surface::loop_frame::forced_miss() {
                return PostStepCheck::Missed(Missed::Timeout);
            }
        }
        #[cfg(target_os = "macos")]
        if !self.state.materials_checked {
            self.state.materials_checked = true;
            if let Some(l) = self.host.win.as_mut()
                && l.surface.as_ref().is_some_and(|surface| l.native.materials.update(self.host.app.rt().transparent, !self.host.app.rt().paint.materials().is_empty(), &l.window, surface))
            {
                return PostStepCheck::Reacquire(ReacquireReason::NativeMaterials);
            }
        }
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        if !self.state.materials_checked {
            self.state.materials_checked = true;
            let device_type = self.host.gpu.as_ref().map(|(gpu, _)| gpu.adapter.get_info().device_type);
            let transparent = self.host.options.transparent;
            let window_backdrop = self.host.app.rt().window_backdrop();
            let found = !self.host.app.rt().paint.windows_material_draws().is_empty();
            let high_contrast = self.host.app.rt().system_contrast().is_some();
            if let Some(l) = self.host.win.as_mut()
                && (found || l.native.materials.native())
            {
                let mut effects = crate::system_material::windows::current_policy();
                effects.high_contrast |= high_contrast;
                let hwnd = crate::platform::win32::hwnd(&l.window).map(|h| h as isize);
                let inputs = crate::system_material::windows::MaterialInputs {
                    transparent,
                    build: crate::platform::win32::dark_mode::windows_build(),
                    swapchain: crate::render::Gpu::dx12_swapchain_kind(),
                    alpha_mode: l.surface.is_some().then_some(l.config.alpha_mode),
                    cpu_painter: l.native.mode.cpu_draws(),
                    renderer_kind: self.host.app.rt().renderer_kind(),
                    device_type,
                    window_backdrop,
                    mode: l.native.materials.mode(),
                    has_handover: l.native.mode.handover().is_some(),
                    effects,
                };
                let native = &mut l.native;
                if native.materials.update(hwnd, inputs, found, native.mode.handover_mut()) {
                    return PostStepCheck::Reacquire(ReacquireReason::NativeMaterials);
                }
            }
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.host.prepare_wayland_material_blur() {
            let more = self.host.app.frame(0.0);
            info.runtime_animating |= more;
            info.animating |= more;
        }
        if self.needs_reblend() {
            return PostStepCheck::Reacquire(ReacquireReason::Reblend);
        }
        PostStepCheck::Keep
    }

    fn prepare_reacquire(&mut self, reason: ReacquireReason, info: &mut StepInfo) -> bool {
        match reason {
            #[cfg(any(test, target_os = "macos", all(target_os = "windows", feature = "windows-system-material")))]
            ReacquireReason::NativeMaterials => {
                // Rebuild with the updated native-materials state before acquiring the replacement drawable.
                #[cfg(target_os = "macos")]
                {
                    let native = self.host.win.as_ref().is_some_and(|l| l.native.materials.native());
                    self.host.app.rt_mut().paint.set_native_materials(native);
                }
                #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
                {
                    let native = self.host.win.as_ref().is_some_and(|l| l.native.materials.native());
                    self.host.app.rt_mut().paint.set_windows_acrylic_materials(native);
                }
                #[cfg(not(any(target_os = "macos", all(target_os = "windows", feature = "windows-system-material"))))]
                self.host.app.rt_mut().paint.set_native_materials(true);
                let more = self.host.app.frame(0.0);
                info.runtime_animating |= more;
                info.animating |= more;
                true
            }
            ReacquireReason::Reblend => {
                let _ = info;
                self.reblend_surface()
            }
            #[cfg(not(target_arch = "wasm32"))]
            ReacquireReason::ContentResize => true,
        }
    }

    fn on_drawable_dropped(&mut self) {
        self.host.note_frame_texture_released();
    }

    fn on_missed(&mut self, missed: Missed, stepped: bool) {
        if missed == Missed::Unconfigured {
            self.host.missed_frame(Missed::Unconfigured, stepped);
            return;
        }
        if self.state.hidden {
            if let Some(l) = self.host.win.as_mut() {
                l.first_frame.missed();
                l.backoff_step(BackoffEvent::HiddenMissed {
                    preparing: l.first_frame.deadline().is_some(),
                    now: crate::time::now(),
                });
            }
            crate::haptics::flush_live(self.host.app.rt_mut(), false);
            self.host.frame_reports.skipped();
            return;
        }
        self.host.missed_frame(missed, stepped);
    }

    fn sync_after_step(&mut self, info: &mut StepInfo) {
        #[cfg(target_os = "macos")]
        self.state.shown.take();
        let now = self.state.now.unwrap_or_else(crate::time::now);
        let Some(l) = self.host.win.as_mut() else { return };
        l.backoff_step(BackoffEvent::Presented);
        if self.state.hidden { l.hidden_pace.rendered(now); }
        info.update_ms = now.elapsed().as_secs_f64() * 1000.0;
        self.state.profile.mark_built();
        sync_platform_core(l, self.host.app.rt_mut(), &mut self.host.secure_input, &mut self.host.ime, self.host.ime_waits, &self.host.clipboard);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        self.host.sync_wayland_drop();
    }

    fn prepare_render(&mut self, drawable: wgpu::SurfaceTexture, info: &mut StepInfo) -> RenderPrep<PreparedWindowFrame> {
        prepare_window_render(self.host, drawable, self.state.corners, self.state.looped.as_ref(), &mut self.state.profile, info)
    }

    fn render(&mut self, prepared: &mut PreparedWindowFrame, info: &StepInfo) -> bool {
        render_window_frame(self.host, prepared, self.state.corners, &mut self.state.profile, info)
    }

    fn submit_and_present(&mut self, prepared: PreparedWindowFrame, info: &StepInfo) -> Instant {
        submit_and_present_window(self.host, prepared, self.state.looped.as_ref(), &mut self.state.profile, &mut self.state.encoded, info)
    }

    fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore) {
        (Some(&mut self.host.clock), self.host.app.rt_mut())
    }

    fn settle(&mut self, submitted: Instant, info: &StepInfo) {
        if self.host.poll_gpu_loss() { return; }
        let now = self.state.now.unwrap_or(submitted);
        if self.host.gpu.as_ref().is_none_or(|(gpu, _)| gpu.is_lost()) {
            #[cfg(target_os = "windows")]
            self.host.on_resize_sync_event(crate::window::win32::resize_sync::Event::DeviceLost);
            let outcome = if let Some(l) = self.host.win.as_mut() {
                #[cfg(target_os = "windows")]
                l.native.slot.reset();
                l.stop_pacing();
                self.host.meter.set_pacing(l.pacing_state(false));
                apply_lost_present_core(
                    &l.surface_retries,
                    crate::render::Gpu::env_fallback_policy(),
                    crate::gpu::fallback::software_fallback_available(),
                    &mut l.surface_ok,
                    &mut l.presented,
                    &mut l.backoff,
                    &mut self.host.frame_reports,
                    &mut self.host.idle,
                    now,
                )
            } else {
                self.host.idle = true;
                self.host.frame_reports.skipped();
                LostPresent::Retry
            };
            crate::haptics::flush_live(self.host.app.rt_mut(), false);
            if outcome == LostPresent::GiveUp {
                let _ = self.host.replace_lost_surface();
            }
            return;
        }
        crate::haptics::flush_live(self.host.app.rt_mut(), !self.state.hidden);
        self.host.meter.presented();
        #[cfg(target_os = "macos")]
        if let Some(l) = self.host.win.as_mut() {
            l.redraw_step(RedrawEvent::DrewOutsidePaint);
            if let Some(pacing) = &mut l.native.pacing { pacing.presented(submitted); }
            // A display link that couldn't be made while the displays slept: they're back.
            if !self.state.hidden && l.native.pacing.is_none() && !self.host.pacing_waits {
                l.native.pacing = crate::platform::display_link::DisplayLink::retry(&l.window, self.host.frame_wake.clone())
                    .map(|p| p.with_ambient_period(self.host.ambient_period));
            }
        }
        let frame_seq = self.host.frame_reports.drawn(self.state.encoded, info.presents_at, now, info.update_ms);
        self.host.app.inspect_frame();
        #[cfg(target_os = "macos")]
        let drawable = self.state.drawable;
        #[cfg(not(target_os = "macos"))]
        let drawable = None;
        #[cfg(target_os = "macos")]
        if let (Some(frame_seq), Some(drawable_seq)) = (frame_seq, drawable) {
            self.host.frame_presented.push_and_drain(drawable_seq, frame_seq);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = frame_seq;
        if record_trace_frame(self.host, now, submitted, self.state.target, drawable) {
            return;
        }
        let Some(l) = self.host.win.as_mut() else { return };
        // A poll asking for the next frame (`wake_after(Duration::ZERO)`) keeps a running display
        // link going; without one its timer waits a frame in `wake_deadline`.
        let next_tick = l.next_tick(now).filter(|_| !self.state.hidden);
        let surfaces_pending = self.host.gpu.as_ref().is_some_and(|(_, r)| r.surfaces_pending());
        let rests = !self.state.hidden && !surfaces_pending && !(self.host.trace.is_some() && self.host.args.frames.is_some());
        self.host.idle = Presented::settle_core(
            &mut l.presented,
            self.host.app.rt(),
            &l.config,
            info.animating || FrameMove::moving(&self.host.frame_move),
            next_tick,
            rests,
            self.host.app.compositor_draws_own(),
        );
        // A drawn frame asks for nothing: it only puts the rest off.
        if !self.state.hidden { let _ = rest::step(&mut self.host.rest, RestEvent::Drew { now, low: rest::low_memory() }); }
        #[cfg(target_os = "macos")]
        if !self.state.hidden && let Some(latency) = l.native.launch_drawables.settle(self.host.idle, now) {
            l.config.desired_maximum_frame_latency = latency;
            if let (Some(surface), Some((gpu, _))) = (l.surface.as_ref(), self.host.gpu.as_ref())
                && let Err(error) = configure(surface, gpu, &l.config)
            {
                eprintln!("rune: a window's surface refused its frame latency: {error}");
                l.surface_ok = false;
            }
        }
        if !self.state.hidden && (!self.host.idle || (self.host.trace.is_some() && self.host.args.frames.is_some())) {
            // Without a running display link, macOS hands out drawables faster than the
            // display shows them, so a timer paces the next frame instead of the loop.
            #[cfg(target_os = "macos")]
            if !l.native.pacing.as_mut().is_some_and(|p| p.start()) { l.backoff_step(BackoffEvent::At(now + TIMER_FRAME)); }
            #[cfg(not(target_os = "macos"))]
            l.request_redraw();
            self.host.meter.set_pacing(l.pacing_state(true));
        } else if self.state.hidden {
            l.stop_pacing();
            self.host.meter.set_pacing(l.pacing_state(false));
        } else {
            l.coast_pacing();
            self.host.meter.set_pacing(l.pacing_state(false));
        }
        self.state.profile.flush(self.host, info.animating, next_tick);
        // A frame change this frame's commit asked for starts with the layout springs it
        // started, which have shown a step unless they held for the window; the next frame
        // moves it. While the user drags the edge, it waits until they let go.
        #[cfg(target_os = "macos")]
        if !self.state.hidden && !self.state.live_resize && self.host.app.rt().host.window_frame.is_some() {
            let frame_time = self.state.frame_time.unwrap_or(now);
            let shown = if self.host.app.rt_mut().host.fit.take_held() { 0.0 } else { crate::frame::clock::RESUME_STEP };
            let start = self.host.frame_secs(frame_time) - shown;
            self.host.take_frame_request(start, true);
        }
    }
}

/// A loop-only step for the frame presented at `at` (`Runtime::loop_step`): the instances it
/// wrote and the seconds it stepped, or why it takes an ordinary frame. The frame clock takes
/// the step only when the runtime does.
fn loop_step(clock: &mut FrameClock, rt: &mut RuntimeCore, at: Instant) -> Result<(Vec<u32>, f64), &'static str> {
    let dt = at.saturating_duration_since(clock.last()).as_secs_f64();
    let patched = rt.loop_step(dt)?;
    clock.frame_step(at, true);
    rt.frame.set_frame_target(at);
    Ok((patched, dt))
}

/// The window's display refresh: its display link's on macOS, else the monitor's.
pub(in crate::window) fn display_hz(l: &Win) -> Option<f64> {
    #[cfg(target_os = "macos")]
    if let Some(pacing) = &l.native.pacing { return Some(pacing.refresh_hz()); }
    l.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).map(|hz| hz as f64 / 1000.0)
}

#[cfg(test)]
#[path = "draw_tests.rs"]
mod tests;
