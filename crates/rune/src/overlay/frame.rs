//! An overlay frame's steps apart from its window: the animation step, the compositor or
//! default draw, whether anything asks for another frame, and the submit and present.

use super::app::OverlayApp;
use super::{Backing, Live};
use crate::frame::clock::FrameClock;
use crate::machine::Effects;
use crate::render::{Gpu, Renderer};
use crate::runtime::RuntimeCore;
use crate::time::Instant;
use crate::window::Presented;

/// Non-generic arguments passed to [`OverlayApp::render_compositor`], which constructs
/// [`crate::compose::Frame<'_, T>`] in place.
pub(super) struct OverlayRenderArgs<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub target: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub size: (u32, u32),
    pub dt: f64,
    pub update_ms: f64,
    pub ui: &'a mut Renderer,
}

/// Whether nothing asks for the frame after one just presented. `next_tick` is the running
/// display link's next tick, if one paces the overlay: a timer due by then counts as asking,
/// so a poll stepping its own animation with `Tx::wake_after(Duration::ZERO)` gets every frame
/// at the display's rate instead of the link stopping after each frame. Unpaced, the timer
/// waits a frame in `wake_deadline` and never keeps the loop turning.
pub(super) fn idle_after(rt: &RuntimeCore, animating: bool, next_tick: Option<Instant>) -> bool {
    !(animating || rt.stale() || rt.live_pending() || crate::window::timer_wants_frame_core(rt, next_tick))
}

/// `draw_frame`'s animation step, apart from the surface so tests can drive it.
pub(super) fn step(app: &mut dyn OverlayApp, clock: &mut FrameClock, frame_time: Instant, prewarm: bool) -> (bool, f64) {
    if prewarm {
        clock.hold(frame_time);
        return (app.frame(0.0), 0.0);
    }
    let animating = app.compositor_animating();
    app.clock_advance(clock, frame_time, animating)
}

/// What a shown overlay's last frame drew while it rests, and whether only a due timer asked
/// for the next. That frame builds before it takes a drawable or starts the display link, and
/// draws nothing when it would draw the same pixels: a poll that wakes on a timer and changes
/// nothing on screen presents nothing.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Quiet<P = Presented> {
    /// What it presented isn't known: nothing drawn yet, or motion, a compositor or a prewarm
    /// frame drew it, or it was covered or hidden since.
    Unknown,
    /// It rests on `presented`. `timer`: only a due timer asked for the next frame, which is
    /// that frame's alone.
    Rests { presented: P, timer: bool },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum QuietEvent<P = Presented> {
    /// A due timer asks for a frame; `resting`: nothing else does (idle, nothing dirty).
    TimerDue { resting: bool },
    /// `draw_frame` begins.
    FrameStarts,
    /// A frame presented: it rests on `Some` (recorded into what `slot` took), or its pixels
    /// are no longer known.
    Presented(Option<P>),
    /// Covered or hidden: what it last presented may not be what shows next.
    Forget,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum QuietEffect {
    /// The frame starting is a resting overlay's timer frame: it builds first, and draws
    /// nothing when it would draw `presented` again.
    TimerFrame,
}

impl<P> Default for Quiet<P> {
    fn default() -> Self { Quiet::Unknown }
}

impl<P> Quiet<P> {
    pub(super) fn step(&mut self, event: QuietEvent<P>) -> Effects<QuietEffect> {
        match (self, event) {
            (Quiet::Rests { timer, .. }, QuietEvent::TimerDue { resting: true }) => *timer = true,
            (_, QuietEvent::TimerDue { .. }) => {}
            (Quiet::Rests { timer, .. }, QuietEvent::FrameStarts) => if std::mem::take(timer) { return Effects::one(QuietEffect::TimerFrame) },
            (Quiet::Unknown, QuietEvent::FrameStarts) => {}
            (this, QuietEvent::Presented(Some(presented))) => *this = Quiet::Rests { presented, timer: false },
            (this, QuietEvent::Presented(None) | QuietEvent::Forget) => *this = Quiet::Unknown,
        }
        Effects::none()
    }

    /// The frame it rests on.
    pub(super) fn presented(&self) -> Option<&P> {
        match self {
            Quiet::Rests { presented, .. } => Some(presented),
            Quiet::Unknown => None,
        }
    }

    /// After a frame presents: it rests on that frame (`record` writes it over the last one's
    /// buffers), or its pixels are no longer known (a prewarm frame, motion, a compositor).
    pub(super) fn presented_frame(&mut self, rests: bool, record: impl FnOnce(&mut Option<P>)) {
        let presented = rests.then(|| {
            let mut slot = match std::mem::take(self) {
                Quiet::Rests { presented, .. } => Some(presented),
                Quiet::Unknown => None,
            };
            record(&mut slot);
            slot
        }).flatten();
        self.step(QuietEvent::Presented(presented));
    }
}

/// What a turn's frame does once the surface can draw, before it takes a drawable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Turn {
    /// Shown but none of it on screen (macOS): it draws nothing. `recheck`: its retry is due,
    /// or it has none, so a miss runs what the frame would have and waits again.
    Occluded { recheck: bool },
    /// Nothing changed and nothing moves.
    Rest,
    /// Waiting out a missed frame's retry, or the timer pacing unpaced motion.
    BackedOff,
    /// Waiting for a Wayland layer surface's `wl_surface.frame` callback from the last present
    /// before acquiring or presenting again.
    AwaitingFrame,
    /// A frame is due. A timer frame may still find nothing new to present (`Quiet`).
    Draw,
}

/// Whether this event-loop turn has already stepped or missed a frame for this overlay, so
/// `wake_due` + occluded recheck or a paced `DisplayTick` miss + `about_to_wait` never advance
/// the runtime twice in one turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum TurnFrame {
    #[default]
    Idle,
    /// `wake_due` already stepped the runtime for a due timer this turn.
    WakeStepped,
    /// A frame missed in this turn: wait for the next turn or tick instead of retrying in
    /// `about_to_wait`.
    Missed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TurnEvent {
    NewEvents,
    WakeStepped,
    FrameMissed,
}

impl TurnFrame {
    pub(super) fn step(&mut self, event: TurnEvent) {
        *self = match (*self, event) {
            (_, TurnEvent::NewEvents) => TurnFrame::Idle,
            (TurnFrame::Idle, TurnEvent::WakeStepped) => TurnFrame::WakeStepped,
            (state, TurnEvent::WakeStepped) => state,
            (_, TurnEvent::FrameMissed) => TurnFrame::Missed,
        };
    }

    pub(super) fn stepped(self) -> bool {
        !matches!(self, TurnFrame::Idle)
    }

    pub(super) fn missed(self) -> bool {
        matches!(self, TurnFrame::Missed)
    }
}

/// What `turn` decides from. `occluded` is the panel's reading this turn (macOS);
/// `awaiting_frame` is an outstanding Wayland layer-shell `wl_surface.frame` callback.
#[derive(Clone, Copy, Debug)]
pub(super) struct Gate {
    pub(super) prewarm: bool,
    pub(super) occluded: bool,
    pub(super) awaiting_frame: bool,
    pub(super) dirty: bool,
    pub(super) idle: bool,
    pub(super) retry: Option<Instant>,
    pub(super) missed_this_turn: bool,
}

/// Until it's uncovered, only the retry (or a due timer, in `wake_due`) steps an occluded
/// overlay, however often other windows turn the loop; input waits for the retry too. While a
/// Wayland layer surface awaits its `wl_surface.frame` callback, draws are skipped until the
/// compositor answers. A prewarm frame draws regardless. `now` is read only where a retry is
/// compared.
pub(super) fn turn(gate: Gate, now: impl Fn() -> Instant) -> Turn {
    if !gate.prewarm && gate.missed_this_turn {
        return Turn::BackedOff;
    }
    if !gate.prewarm && gate.occluded {
        return Turn::Occluded { recheck: gate.retry.is_none_or(|retry| retry <= now()) };
    }
    if !gate.dirty && gate.idle { return Turn::Rest; }
    if !gate.prewarm && gate.awaiting_frame { return Turn::AwaitingFrame; }
    if !gate.prewarm && gate.retry.is_some_and(|retry| retry > now()) { return Turn::BackedOff; }
    Turn::Draw
}

/// Whether a Wayland layer surface must wait for its outstanding `wl_surface.frame` callback.
/// Once the GPU device is lost, nothing was committed so the callback will never arrive and the
/// surface must not wait on it.
pub(super) fn is_awaiting_frame(frame_ready: bool, device_lost: bool) -> bool {
    !frame_ready && !device_lost
}

/// Whether a non-macOS overlay asks the event loop to keep polling after presenting a frame.
/// A layer surface awaiting its `wl_surface.frame` callback wakes through its watch thread rather
/// than `ControlFlow::Poll`.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(super) fn wants_more(idle: bool, awaiting_frame: bool) -> bool {
    !idle && !awaiting_frame
}

/// Whether no frame will come on its own to run due timers, updates, or drag ends.
pub(super) fn is_frameless(drawing: bool, occluded: bool, awaiting_frame: bool) -> bool {
    !drawing || occluded || awaiting_frame
}

/// Whether advancing a drawing overlay without pending input handlers only polls instead of
/// rebuilding the view and laying out.
pub(super) fn polls_only_when_drawing(shown: bool, occluded: bool, awaiting_frame: bool) -> bool {
    shown && (occluded || awaiting_frame)
}

/// The runtime timer deadline for an overlay: while awaiting a Wayland layer frame callback,
/// ambient visual loops pause until the compositor paints again while app timers still fire.
pub(super) fn timer_deadline(rt: &RuntimeCore, awaiting_frame: bool) -> Option<Instant> {
    if awaiting_frame {
        crate::window::poll_timer_deadline_core(rt)
    } else {
        crate::window::timer_deadline_core(rt)
    }
}

/// A resting overlay's timer frame, stepped before any drawable: `None` when it draws what
/// `presented` shows and nothing asks for the next (`next_tick`: the display link's next tick,
/// had it run), else the step's `(animating, dt)` to draw with.
#[cfg(test)]
pub(super) fn quiet_step(app: &mut dyn OverlayApp, clock: &mut FrameClock, presented: &Presented,
    config: &wgpu::SurfaceConfiguration, now: Instant, next_tick: Option<Instant>) -> Option<(bool, f64)> {
    let (animating, dt) = app.clock_advance(clock, now, false);
    (!presented.unchanged_core(app.rt(), config, animating, next_tick)).then_some((animating, dt))
}

/// Whether a frame can leave the runtime as it is, drawing its last build again: after the
/// last frame it asked for no other (`runtime_idle`: only the compositor animates), nothing
/// marked the overlay dirty since, and no input, live value, timer, playground or poll (which
/// runs on every frame drawn) waits. An overlay without a compositor draws nothing then.
pub(super) fn rests(app: &dyn OverlayApp, runtime_idle: bool, dirty: bool) -> bool {
    let rt = app.rt();
    runtime_idle && !dirty && !app.has_poll() && !rt.tree.has_playground && rt.wake_deadline().is_none() && rt.rests()
}

/// `step` for a runtime that `rests`: its clocks (`clock`, `t`) move to the frame as a step
/// would move them, so a compositor reading them animates the same, and nothing runs or builds.
pub(super) fn rest(app: &mut dyn OverlayApp, clock: &mut FrameClock, frame_time: Instant) -> (bool, f64) {
    let elapsed = clock.frame_step(frame_time, app.compositor_animating()).total();
    let rt = app.rt_mut();
    rt.frame.set_frame_target(frame_time);
    rt.clock = rt.wall_start.map_or(rt.clock + elapsed, |t0| t0.elapsed().as_secs_f64());
    rt.t += elapsed * rt.time_scale as f64;
    rt.scene.set_time(rt.t);
    (false, elapsed)
}

/// A due frame's target presentation time, or `None` when the display link says wait. The
/// opportunity is claimed before `settle` waits for the frame still on the present thread, so
/// a turn that draws nothing (the loop's second look after a tick drew) never blocks on it.
#[cfg(target_os = "macos")]
pub(super) fn claim(opportunity: Option<crate::platform::display_link::Opportunity>, settle: impl FnOnce()) -> Option<Option<Instant>> {
    let target = match opportunity { Some(opportunity) => opportunity.target()?, None => None };
    settle();
    Some(target)
}

/// Both native surface frames and offscreen prewarming use the same compositor path.
pub(super) fn render_frame(app: &mut dyn OverlayApp, f: OverlayRenderArgs<'_>) -> bool {
    {
        let rt = app.rt_mut();
        f.ui.upload_images(f.device, f.queue, &mut rt.images);
        f.ui.build_surfaces(f.device, &rt.surfaces, rt.live_notifier());
        f.ui.prepare_surfaces(f.device, f.queue, &rt.surfaces, f.size);
        f.ui.set_frame_wake(rt.live_wake());
    }
    if app.has_compositor() {
        app.render_compositor(f)
    } else {
        let rt = app.rt_mut();
        f.ui.set_fade_stops(rt.fade_stops());
        f.ui.prepare_ui(f.device, f.queue, &mut rt.text.atlas, &rt.instances, f.size);
        f.ui.draw(f.encoder, f.target, [0.0; 4]);
        false
    }
}

/// Submits the frame's commands and presents its drawable. On macOS a plain frame (no material
/// views to move with the drawable, no transaction, no native video surface waiting on the
/// submission) is finished, submitted and presented on the overlay's present thread, so the
/// event loop's thread spends none of wgpu's encode, submit and present on it. Prewarm frames
/// present here, before the window is revealed.
pub(super) fn submit_and_present(live: &mut Live, renderer: &Renderer, gpu: &Gpu, rt: &RuntimeCore,
    encoder: wgpu::CommandEncoder, frame: wgpu::SurfaceTexture, prewarm: bool) {
    #[allow(irrefutable_let_patterns)]
    if let Backing::Window(w) = &live.backing {
        w.pre_present_notify();
    }
    #[cfg(target_os = "macos")]
    {
        if !prewarm && live.materials.presents_plainly() && renderer.submits_plainly() && !crate::gpu::native(&gpu.queue) {
            if live.present.is_none() { live.present = crate::frame::present_worker::PresentWorker::spawn(); }
            if let Some(worker) = &mut live.present {
                let mut encoder = encoder;
                renderer.before_submit(&mut encoder);
                let queue = gpu.queue.clone();
                return worker.run(move || {
                    queue.submit([encoder.finish()]);
                    queue.present(frame);
                });
            }
        }
        renderer.submit(&gpu.queue, encoder);
        let (draws, scale) = crate::system_material::present_draws(rt);
        live.materials.present(draws, scale, || gpu.queue.present(frame));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (rt, prewarm);
        renderer.submit(&gpu.queue, encoder);
        #[cfg(all(unix, not(target_os = "ios"), not(target_os = "android")))]
        if !gpu.is_lost() && let Backing::Layer(layer) = &mut live.backing {
            layer.request_frame();
        }
        gpu.queue.present(frame);
    }
}

/// Records a presented overlay frame for launch marks once it is actually shown. Prewarm frames
/// render while the panel is hidden, so they do not record `frame.first.presented`.
pub(super) fn record_presented(prewarm: bool) {
    if !prewarm {
        crate::window::frame_presented();
    }
}

/// Whether revealing a retained macOS overlay exposes an already-drawn prewarm frame without
/// scheduling another draw (so the reveal itself is the first visible present).
pub(super) fn retained_reveal_presents(retained: bool, dirty: bool, live: bool) -> bool {
    retained && !dirty && live
}

impl Live {
    /// Waits for the frame on the present thread, before the surface is touched again.
    pub(super) fn settle(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(worker) = &mut self.present { worker.wait(); }
    }
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;
