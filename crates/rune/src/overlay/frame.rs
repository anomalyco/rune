//! An overlay frame's steps apart from its window: the animation step, the compositor or
//! default draw, whether anything asks for another frame, and the submit and present.

use super::{Backing, Live};
use crate::compose::{Compositor, Frame};
use crate::frame::clock::FrameClock;
use crate::machine::Effects;
use crate::render::{Gpu, Renderer};
use crate::runtime::Runtime;
use crate::time::Instant;
use crate::window::Presented;

/// Whether nothing asks for the frame after one just presented. `next_tick` is the running
/// display link's next tick, if one paces the overlay: a timer due by then counts as asking,
/// so a poll stepping its own animation with `Tx::wake_after(Duration::ZERO)` gets every frame
/// at the display's rate instead of the link stopping after each frame. Unpaced, the timer
/// waits a frame in `wake_deadline` and never keeps the loop turning.
pub(super) fn idle_after<T: 'static>(rt: &Runtime<T>, animating: bool, next_tick: Option<Instant>) -> bool {
    !(animating || rt.has_pending_input() || crate::window::timer_wants_frame(rt, next_tick))
}

/// `draw_frame`'s animation step, apart from the surface so tests can drive it.
pub(super) fn step<T: 'static>(rt: &mut Runtime<T>, clock: &mut FrameClock, compositor: &Option<Box<dyn Compositor<T>>>, frame_time: Instant, prewarm: bool) -> (bool, f64) {
    if prewarm {
        clock.hold(frame_time);
        return (rt.frame(0.0), 0.0);
    }
    clock.advance(rt, frame_time, compositor.as_ref().is_some_and(|c| c.animating()))
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
    /// A frame is due. A timer frame may still find nothing new to present (`Quiet`).
    Draw,
}

/// What `turn` decides from. `occluded` is the panel's reading this turn (macOS).
#[derive(Clone, Copy, Debug)]
pub(super) struct Gate {
    pub(super) prewarm: bool,
    pub(super) occluded: bool,
    pub(super) dirty: bool,
    pub(super) idle: bool,
    pub(super) retry: Option<Instant>,
}

/// Until it's uncovered, only the retry (or a due timer, in `wake_due`) steps an occluded
/// overlay, however often other windows turn the loop; input waits for the retry too. A
/// prewarm frame draws regardless of both. `now` is read only where a retry is compared.
pub(super) fn turn(gate: Gate, now: impl Fn() -> Instant) -> Turn {
    if !gate.prewarm && gate.occluded {
        return Turn::Occluded { recheck: gate.retry.is_none_or(|retry| retry <= now()) };
    }
    if !gate.dirty && gate.idle { return Turn::Rest; }
    if !gate.prewarm && gate.retry.is_some_and(|retry| retry > now()) { return Turn::BackedOff; }
    Turn::Draw
}

/// A resting overlay's timer frame, stepped before any drawable: `None` when it draws what
/// `presented` shows and nothing asks for the next (`next_tick`: the display link's next tick,
/// had it run), else the step's `(animating, dt)` to draw with.
pub(super) fn quiet_step<T: 'static>(rt: &mut Runtime<T>, clock: &mut FrameClock, presented: &Presented,
    config: &wgpu::SurfaceConfiguration, now: Instant, next_tick: Option<Instant>) -> Option<(bool, f64)> {
    let (animating, dt) = step(rt, clock, &None, now, false);
    (!presented.unchanged(rt, config, animating, next_tick)).then_some((animating, dt))
}

/// Whether a frame can leave the runtime as it is, drawing its last build again: after the
/// last frame it asked for no other (`runtime_idle`: only the compositor animates), nothing
/// marked the overlay dirty since, and no input, live value, timer, playground or poll (which
/// runs on every frame drawn) waits. An overlay without a compositor draws nothing then.
pub(super) fn rests<T: 'static>(rt: &Runtime<T>, runtime_idle: bool, dirty: bool) -> bool {
    runtime_idle && !dirty && rt.poll.is_none() && rt.playground.is_none() && rt.wake_deadline().is_none() && rt.rests()
}

/// `step` for a runtime that `rests`: its clocks (`clock`, `t`) move to the frame as a step
/// would move them, so a compositor reading them animates the same, and nothing runs or builds.
pub(super) fn rest<T: 'static>(rt: &mut Runtime<T>, clock: &mut FrameClock, compositor: &Option<Box<dyn Compositor<T>>>, frame_time: Instant) -> (bool, f64) {
    let elapsed = clock.frame_step(frame_time, compositor.as_ref().is_some_and(|c| c.animating())).total();
    rt.frame_target = Some(frame_time);
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
pub(super) fn render_frame<T: 'static>(f: &mut Frame<'_, T>, compositor: &mut Option<Box<dyn Compositor<T>>>) -> bool {
    f.ui.upload_images(f.device, f.queue, &mut f.rt.images);
    f.ui.build_surfaces(f.device, &f.rt.surfaces, f.rt.live_notifier());
    f.ui.prepare_surfaces(f.device, f.queue, &f.rt.surfaces, f.size);
    if let Some(c) = compositor {
        c.render(f);
        c.animating()
    } else {
        f.ui.prepare(f.device, f.queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
        f.ui.draw(f.encoder, f.target, [0.0; 4]);
        false
    }
}

/// Submits the frame's commands and presents its drawable. On macOS a plain frame (no material
/// views to move with the drawable, no transaction, no native video surface waiting on the
/// submission) is finished, submitted and presented on the overlay's present thread, so the
/// event loop's thread spends none of wgpu's encode, submit and present on it. Prewarm frames
/// present here, before the window is revealed.
pub(super) fn submit_and_present<T: 'static>(live: &mut Live, renderer: &Renderer, gpu: &Gpu, rt: &Runtime<T>,
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
        live.materials.present(&rt.materials, rt.env.scale, || gpu.queue.present(frame));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (rt, prewarm);
        renderer.submit(&gpu.queue, encoder);
        gpu.queue.present(frame);
    }
}

impl Live {
    /// Waits for the frame on the present thread, before the surface is touched again.
    pub(super) fn settle(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(worker) = &mut self.present { worker.wait(); }
    }
}
