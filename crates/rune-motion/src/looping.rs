//! Clock-driven periodic and finite motion. Visibility is a renderer decision, not a second clock.
use crate::{Animated, Beat, Cycle, Easing, Sequence, PropClass, Spring};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoopKind {
    Opacity { low: f32, high: f32 },
    /// Piecewise-linear, wall-clock opacity sampled at display rate or a fixed cadence.
    OpacityCycle(Cycle),
    /// Finite opacity/scale choreography; holds its last pose at rest.
    Sequence(Sequence),
    Rotation,
    /// An eased one-way sweep that wraps, in fractions of the node's own size (an
    /// indeterminate progress segment). Descendants and hit tests follow it.
    Slide { from: [f32; 2], to: [f32; 2] },
    /// A `Slide` swept along `easing` instead of ease-in-out ([`LoopSpec::easing`]).
    SlideEased { from: [f32; 2], to: [f32; 2], easing: Easing },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopSpec {
    pub period: f64,
    pub delay: f64,
    pub kind: LoopKind,
}

impl LoopSpec {
    pub fn sequence(frames: Sequence) -> Self {
        Self { period: frames.duration(), delay: frames.delay, kind: LoopKind::Sequence(frames) }
    }
    pub fn cycle(cycle: Cycle) -> Self {
        Self { period: f64::from(cycle.period), delay: 0.0, kind: LoopKind::OpacityCycle(cycle) }
    }
    pub const fn opacity(period: f64, delay: f64, low: f32, high: f32) -> Self {
        Self { period, delay, kind: LoopKind::Opacity { low, high } }
    }
    pub const fn rotation(period: f64) -> Self {
        Self { period, delay: 0.0, kind: LoopKind::Rotation }
    }
    /// Reduced motion holds the sweep's midpoint; removal cools it back to no offset.
    pub const fn slide(period: f64, from: [f32; 2], to: [f32; 2]) -> Self {
        Self { period, delay: 0.0, kind: LoopKind::Slide { from, to } }
    }
    /// The curve a slide sweeps along each period, ease-in-out by default; `Easing::LINEAR`
    /// sweeps at a constant speed (a shimmer). Only slides take a curve; other kinds keep theirs.
    pub const fn easing(mut self, easing: Easing) -> Self {
        if let LoopKind::Slide { from, to } | LoopKind::SlideEased { from, to, .. } = self.kind {
            self.kind = LoopKind::SlideEased { from, to, easing };
        }
        self
    }
}

const fn is_slide(kind: LoopKind) -> bool {
    matches!(kind, LoopKind::Slide { .. } | LoopKind::SlideEased { .. })
}

#[derive(Clone, Copy, Debug)]
pub struct LoopTrack {
    spec: LoopSpec,
    /// Finite repetition count for slide loops (`Slide` / `SlideEased`); `None` repeats forever.
    times: Option<u32>,
    phase: Phase,
    phase0: f64,
    amplitude: Animated<f32>,
}

/// Where a track is in its play. A periodic loop only ever plays (its cooling lives on
/// `amplitude`); a finite sequence or finite slide can also be done.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Playing, with phase 0 landing on `since`. A delayed start is a `since` still ahead.
    Playing { since: f64 },
    /// A finite track that played out: it holds its end pose, and enabling it again leaves
    /// it there. It does not replay.
    Done,
}

impl LoopTrack {
    pub fn new(spec: LoopSpec, now: f64) -> Self {
        assert!(spec.period.is_finite() && spec.period > 0.0);
        assert!(spec.delay.is_finite() && spec.delay >= 0.0);
        Self { spec, times: None, phase: Phase::Playing { since: now + spec.delay }, phase0: 0.0,
            amplitude: Animated::new(1.0).with_class(PropClass::Opacity) }
    }
    /// Starts a track whose slide loop (`LoopKind::Slide` or `LoopKind::SlideEased`) plays
    /// `times` periods and then rests at its end pose (`to`), matching CSS
    /// `animation-iteration-count` with `animation-fill-mode: forwards`.
    ///
    /// `times == 0` rests immediately at `to` with no active frames. Under Reduce Motion a finite
    /// slide lands at `to` instead of holding its midpoint. Non-slide loop kinds ignore `times`.
    ///
    /// ```
    /// use rune_motion::{Easing, LoopSpec, LoopTrack};
    ///
    /// let spec = LoopSpec::slide(0.5, [0.0, 0.0], [2.0, 0.0]).easing(Easing::LINEAR);
    /// let track = LoopTrack::with_times(spec, 2, 0.0);
    /// assert!(track.active(0.75));
    /// assert!(!track.active(1.0));
    /// assert_eq!(track.offset(1.0, false), Some([2.0, 0.0]));
    /// ```
    pub fn with_times(spec: LoopSpec, times: u32, now: f64) -> Self {
        assert!(spec.period.is_finite() && spec.period > 0.0);
        assert!(spec.delay.is_finite() && spec.delay >= 0.0);
        let times = is_slide(spec.kind).then_some(times);
        let phase = if times == Some(0) { Phase::Done } else { Phase::Playing { since: now + spec.delay } };
        Self { spec, times, phase, phase0: 0.0,
            amplitude: Animated::new(1.0).with_class(PropClass::Opacity) }
    }
    /// The finite repetition count for a slide track, or `None` when the loop repeats forever.
    ///
    /// ```
    /// use rune_motion::{LoopSpec, LoopTrack};
    /// let track = LoopTrack::with_times(LoopSpec::slide(0.5, [0.0, 0.0], [1.0, 0.0]), 2, 0.0);
    /// assert_eq!(track.times(), Some(2));
    /// ```
    pub const fn times(&self) -> Option<u32> {
        self.times
    }
    /// Whether this track has an end: a one-shot `Sequence` or a slide with a finite `times`
    /// count plays out and then rests.
    ///
    /// ```
    /// use rune_motion::{LoopSpec, LoopTrack};
    /// let finite = LoopTrack::with_times(LoopSpec::slide(0.5, [0.0, 0.0], [1.0, 0.0]), 1, 0.0);
    /// assert!(finite.is_finite());
    /// assert!(!LoopTrack::new(LoopSpec::rotation(1.0), 0.0).is_finite());
    /// ```
    pub fn is_finite(&self) -> bool {
        matches!(self.spec.kind, LoopKind::Sequence(frames) if !frames.repeating)
            || (is_slide(self.spec.kind) && self.times.is_some())
    }
    fn total_duration(&self) -> f64 {
        match self.times {
            Some(n) if is_slide(self.spec.kind) => self.spec.period * f64::from(n),
            _ => self.spec.period,
        }
    }
    pub fn phase(&self, now: f64) -> f64 {
        let since = match self.phase {
            Phase::Playing { since } => since,
            // A played-out finite track rests at its end pose.
            Phase::Done => return 1.0,
        };
        if self.is_finite() {
            if is_slide(self.spec.kind) {
                let n = self.times.unwrap_or(0);
                if n == 0 { return 1.0; }
                let elapsed = now - since;
                if elapsed <= 0.0 { return 0.0; }
                let total = self.spec.period * f64::from(n);
                return if elapsed >= total { 1.0 } else { (elapsed / self.spec.period).rem_euclid(1.0) };
            }
            return ((now - since) / self.spec.period).clamp(0.0, 1.0);
        }
        (self.phase0 + (now - since).max(0.0) / self.spec.period).rem_euclid(1.0)
    }
    /// Preserve the presented phase when changing speed. An unchanged spec is a no-op.
    pub fn retime(&mut self, spec: LoopSpec, now: f64) {
        self.retime_with_times(spec, self.times, now);
    }
    /// [`Self::retime`], also updating a slide's finite repetition count (`None` for infinite).
    /// An unchanged `(spec, times)` is a no-op; switching to, from, or between finite specs
    /// restarts the track from `now`.
    ///
    /// ```
    /// use rune_motion::{LoopSpec, LoopTrack};
    /// let spec = LoopSpec::slide(0.5, [0.0, 0.0], [1.0, 0.0]);
    /// let mut track = LoopTrack::with_times(spec, 1, 0.0);
    /// track.retime_with_times(spec, Some(2), 1.0);
    /// assert!(track.active(1.5));
    /// ```
    pub fn retime_with_times(&mut self, spec: LoopSpec, times: Option<u32>, now: f64) {
        assert!(spec.period.is_finite() && spec.period > 0.0);
        let times = times.filter(|_| is_slide(spec.kind));
        if self.spec == spec && self.times == times { return; }
        let new_finite = matches!(spec.kind, LoopKind::Sequence(frames) if !frames.repeating) || times.is_some();
        if self.is_finite() || new_finite {
            *self = match times {
                Some(n) => Self::with_times(spec, n, now),
                None => Self::new(spec, now),
            };
            return;
        }
        self.phase0 = self.phase(now);
        match &mut self.phase {
            Phase::Playing { since } => *since = now.max(*since),
            // Only finite tracks reach `Done`, and those restarted above.
            Phase::Done => {}
        }
        self.spec = spec;
        self.times = None;
    }
    /// Cool to rest without stopping or resetting phase. Reversal inherits envelope velocity.
    /// A finite track that cooled mid-play carries on from where its phase had reached; one
    /// that played out stays `Done` and does not replay.
    pub fn enabled(&mut self, enabled: bool, now: f64) {
        self.amplitude.animate_to(if enabled { 1.0 } else { 0.0 }, Spring::new(0.18, 0.0), now);
    }
    /// Moves the track onto a clock reading `delta` more, at the same place.
    pub fn shift(&mut self, delta: f64) {
        if let Phase::Playing { since } = &mut self.phase { *since += delta; }
        self.amplitude.shift(delta);
    }
    /// Moves a periodic loop `dt` further along its rhythm, for time its clock didn't see; a
    /// loop still waiting starts `dt` sooner. Finite tracks keep their place.
    pub fn advance(&mut self, dt: f64) {
        if !self.is_finite() && let Phase::Playing { since } = &mut self.phase { *since -= dt; }
    }
    /// Explicit settle/reduced motion lands finite tracks; periodic loops keep their phase.
    pub fn finish(&mut self) {
        if self.is_finite() { self.phase = Phase::Done; }
    }
    pub fn active(&self, now: f64) -> bool {
        match self.phase {
            Phase::Done => false,
            // A cooled finite track keeps its phase clock but draws nothing, so it asks for no
            // frames.
            Phase::Playing { since } if self.is_finite() => {
                self.amplitude.target() > 0.0 && now >= since && now < since + self.total_duration()
            }
            Phase::Playing { since } => {
                (self.amplitude.target() > 0.0 && now >= since) || !self.amplitude.is_settled(now)
            }
        }
    }
    /// The animation-clock instant to wake at; no frames are needed before it.
    pub fn pending_start(&self, now: f64) -> Option<f64> {
        match self.phase {
            Phase::Playing { since } if self.amplitude.target() > 0.0 && now < since => Some(since),
            _ => None,
        }
    }
    /// A stepped cycle's timer interval. Continuous tracks use the host's frame request.
    pub fn frame_interval(&self) -> Option<f64> {
        match self.spec.kind {
            LoopKind::OpacityCycle(c) => c.frame_interval(),
            _ => None,
        }
    }
    /// Opacity multiplier or clockwise radians. Reduced motion uses the calm first pose.
    pub fn sample(&self, now: f64, reduced: bool) -> f32 {
        self.sample_at(now, now, reduced)
    }
    pub fn sample_at(&self, now: f64, wall: f64, reduced: bool) -> f32 {
        let phase = if reduced { 0.0 } else { self.phase(now) as f32 };
        match self.spec.kind {
            LoopKind::Sequence(_) => self.pose(now, reduced).unwrap().opacity,
            LoopKind::OpacityCycle(c) => if reduced { c.still } else { c.sample(wall) },
            LoopKind::Rotation => phase * core::f32::consts::TAU,
            LoopKind::Slide { .. } | LoopKind::SlideEased { .. } => 1.0,
            LoopKind::Opacity { low, high } => {
                let triangle = if phase < 0.5 { phase * 2.0 } else { (1.0 - phase) * 2.0 };
                low + (high - low) * Easing::EASE_IN_OUT.eval(triangle) * self.amplitude.sample(now)
            }
        }
    }
    /// A `Slide`'s offset in fractions of the node's own size.
    pub fn offset(&self, now: f64, reduced: bool) -> Option<[f32; 2]> {
        let (from, to, easing) = match self.spec.kind {
            LoopKind::Slide { from, to } => (from, to, Easing::EASE_IN_OUT),
            LoopKind::SlideEased { from, to, easing } => (from, to, easing),
            _ => return None,
        };
        let t = if reduced {
            if self.times.is_some() { easing.eval(1.0) } else { 0.5 }
        } else {
            easing.eval(self.phase(now) as f32)
        };
        let a = self.amplitude.sample(now);
        Some([0, 1].map(|i| (from[i] + (to[i] - from[i]) * t) * a))
    }
    pub fn pose(&self, now: f64, reduced: bool) -> Option<Beat> {
        let LoopKind::Sequence(frames) = self.spec.kind else { return None };
        if frames.repeating {
            let at = if reduced { 0.0 } else { self.phase(now) * frames.duration() };
            let mut p = frames.sample(at);
            let a = self.amplitude.sample(now);
            if a < 1.0 {
                p.opacity = 1.0 + (p.opacity - 1.0) * a;
                p.scale = (1.0 + (p.scale - 1.0) * a).max(0.0);
            }
            return Some(p);
        }
        let at = if reduced || self.phase == Phase::Done { frames.duration() } else { self.phase(now) * frames.duration() };
        Some(frames.sample(at))
    }
}

#[cfg(test)]
#[path = "looping_tests.rs"]
mod tests;
