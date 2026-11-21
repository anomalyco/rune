//! How far each presented frame moves animation time. Windows, overlays and the headless hosts
//! share this rule so they can't drift apart.
//!
//! Like Core Animation, a late frame never loses time: motion already running advances by the
//! real gap between this frame's target presentation time and the previous frame's, so after
//! a 49 ms hitch it is where an unbroken cadence would have it. Motion that starts or is
//! retargeted on the frame starts at the frame's time and shows one small step, so it never
//! jumps by the gap. Both happen in one runtime pass ([`Runtime::frame_split`]).
//!
//! A frame can be late because of what it did itself: a commit that took 50 ms presents the
//! motion it started 50 ms after its target. That motion then starts from when it showed, as
//! Core Animation starts what a transaction adds when the transaction commits: the next frame
//! moves it one frame on rather than charging it the overrun ([`FrameClock::presented`]).

use crate::machine::Effects;
use crate::time::Instant;
use rune_motion::TrackId;
#[cfg(test)]
use crate::runtime::Runtime;
use crate::runtime::{AppDriver, RuntimeCore};

/// The most a frame steps motion it starts. Its first frame shows one 120 Hz frame of movement,
/// instead of repeating its start pose or jumping ahead by a late frame's gap or the time
/// spent idle.
pub(crate) const RESUME_STEP: f64 = 1.0 / 120.0;

/// The longest gap, in seconds, charged to running motion. A hitch someone sees lasts a few
/// frames; 250 ms is 30 frames at 120 Hz, past any hitch worth keeping time through and still
/// shorter than a typical spring takes to settle. A longer gap means the host was suspended
/// (App Nap, a debugger, sleep, a stalled main thread), hidden or occluded, so running motion
/// moves at most this far rather than teleporting to its end.
pub(crate) const MAX_GAP: f64 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Step {
    /// Charged only to motion already running, before this frame's commits.
    pub catch_up: f64,
    /// This frame's step for all motion, including what its commits start.
    pub dt: f64,
}

impl Step {
    pub(crate) fn total(self) -> f64 {
        self.catch_up + self.dt
    }
}

/// The step for a frame `gap` seconds after the last. `running`: motion was still running
/// after the previous frame. Running motion gets the whole gap, at most [`MAX_GAP`]; what
/// starts on the frame gets at most [`RESUME_STEP`] of it. With nothing running the frame
/// steps [`RESUME_STEP`], so motion it starts moves one frame, however long the idle.
pub(crate) fn split(gap: f64, running: bool) -> Step {
    if !running { return Step { catch_up: 0.0, dt: RESUME_STEP }; }
    let total = gap.clamp(0.0, MAX_GAP);
    let dt = total.min(RESUME_STEP);
    Step { catch_up: total - dt, dt }
}

/// Steps `rt` by a frame `gap` seconds after the last, for hosts without presentation times
/// (the hot loop). Without a wall clock, timers and poll still see the
/// whole gap.
#[cfg(any(test, feature = "hot"))]
pub(crate) fn advance_by_core(rt: &mut RuntimeCore, app: &mut dyn AppDriver, gap: f64) -> bool {
    let step = split(gap, rt.animating);
    rt.clock += (gap - step.total()).max(0.0);
    rt.frame_split(app, step.catch_up, step.dt)
}

#[cfg(test)]
#[inline(always)]
pub(crate) fn advance_by<S: 'static>(rt: &mut Runtime<S>, gap: f64) -> bool {
    let (core, mut app) = rt.split_mut();
    advance_by_core(core, &mut app, gap)
}

#[derive(Debug)]
pub(crate) struct FrameClock {
    phase: Phase,
    /// Recycle the allocation between frames. Only Stepped owns pending starts.
    spare: Vec<TrackId>,
}

#[derive(Debug)]
enum Phase {
    Idle { last: Instant },
    Stepped { last: Instant, starts: Vec<TrackId> },
    Presented { last: Instant },
}

enum Event {
    Hold(Instant),
    Advance { at: Instant, running: bool },
    Present(Instant),
}

enum Effect<'a> {
    Frame(Step, &'a mut Vec<TrackId>),
    Delay(f64, &'a mut [TrackId]),
}

impl FrameClock {
    pub(crate) fn new(now: Instant) -> Self {
        Self { phase: Phase::Idle { last: now }, spare: Vec::new() }
    }

    /// The target time of the last frame the clock stepped to.
    pub(crate) fn last(&self) -> Instant {
        match self.phase {
            Phase::Idle { last } | Phase::Stepped { last, .. } | Phase::Presented { last } => last,
        }
    }

    #[inline]
    fn step(&mut self, event: Event) -> Effects<Effect<'_>> {
        let last = self.last();
        match event {
            Event::Hold(at) => {
                if let Phase::Stepped { starts, .. } = &mut self.phase {
                    self.spare = std::mem::take(starts);
                }
                self.phase = Phase::Idle { last: last.max(at) };
                Effects::none()
            }
            Event::Advance { at, running } => {
                let gap = at.saturating_duration_since(last).as_secs_f64();
                let last = last.max(at);
                if let Phase::Stepped { last: target, .. } = &mut self.phase {
                    *target = last;
                } else {
                    self.phase = Phase::Stepped { last, starts: std::mem::take(&mut self.spare) };
                }
                let Phase::Stepped { starts, .. } = &mut self.phase else { unreachable!() };
                starts.clear();
                Effects::one(Effect::Frame(split(gap, running), starts))
            }
            Event::Present(done) => {
                let Phase::Stepped { starts, .. } = &mut self.phase else { return Effects::none() };
                self.spare = std::mem::take(starts);
                self.phase = Phase::Presented { last };
                if self.spare.is_empty() { return Effects::none(); }
                let late = done.saturating_duration_since(last).as_secs_f64().min(MAX_GAP);
                if late > 0.0 { Effects::one(Effect::Delay(late, &mut self.spare)) } else { Effects::none() }
            }
        }
    }

    /// A frame at `at` that steps no time (prewarm, hidden), or one that caught motion up to
    /// `at`. Display-link targets run ahead of `Instant::now()`, so the clock never goes back.
    pub(crate) fn hold(&mut self, at: Instant) {
        self.step(Event::Hold(at));
    }

    /// The step for a frame presented at `at`. See [`split`].
    fn begin(&mut self, at: Instant, running: bool) -> (Step, &mut Vec<TrackId>) {
        let Effects::One(Effect::Frame(step, starts)) = self.step(Event::Advance { at, running }) else { unreachable!() };
        (step, starts)
    }

    pub(crate) fn frame_step(&mut self, at: Instant, running: bool) -> Step {
        self.begin(at, running).0
    }

    /// Steps `rt` to the frame presented at `at`, in one runtime pass. `compositing`: a
    /// compositor's own animation was still running after the previous frame. Returns whether
    /// anything still animates and the seconds stepped.
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn advance<S: 'static>(&mut self, rt: &mut Runtime<S>, at: Instant, compositing: bool) -> (bool, f64) {
        let (core, mut app) = rt.split_mut();
        self.advance_core(core, &mut app, at, compositing)
    }

    pub(crate) fn advance_core(&mut self, rt: &mut RuntimeCore, app: &mut dyn AppDriver, at: Instant, compositing: bool) -> (bool, f64) {
        let (step, starts) = self.begin(at, rt.animating || compositing);
        rt.frame.set_frame_target(at);
        let animating = rt.frame_split(app, step.catch_up, step.dt);
        starts.extend_from_slice(rt.scene.store().started());
        (animating, step.total())
    }

    /// The frame last stepped to finished at `done`. When that's past its target, the motion it
    /// started holds its first pose for the overrun, so the next frame moves it about a frame
    /// rather than by how late this one was. Motion already running keeps real time.
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn presented<S: 'static>(&mut self, rt: &mut Runtime<S>, done: Instant) {
        self.presented_core(rt.core_mut(), done);
    }

    pub(crate) fn presented_core(&mut self, rt: &mut RuntimeCore, done: Instant) {
        if let Effects::One(Effect::Delay(late, starts)) = self.step(Event::Present(done)) {
            rt.scene.store_mut().delay(starts, late * f64::from(rt.time_scale));
        }
    }
}

#[cfg(test)]
#[path = "clock_walk_tests.rs"]
mod walk_tests;

#[cfg(test)]
#[path = "clock_tests.rs"]
mod tests;
