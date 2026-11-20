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

#[derive(Clone, Copy, Debug)]
pub struct LoopTrack {
    spec: LoopSpec,
    phase: Phase,
    phase0: f64,
    amplitude: Animated<f32>,
}

/// Where a track is in its play. A periodic loop only ever plays (its cooling lives on
/// `amplitude`); a finite sequence can also be done.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Playing, with phase 0 landing on `since`. A delayed start is a `since` still ahead.
    Playing { since: f64 },
    /// A finite sequence that played out: it holds its last pose, and enabling it again leaves
    /// it there. It does not replay.
    Done,
}

impl LoopTrack {
    pub fn new(spec: LoopSpec, now: f64) -> Self {
        assert!(spec.period.is_finite() && spec.period > 0.0);
        assert!(spec.delay.is_finite() && spec.delay >= 0.0);
        Self { spec, phase: Phase::Playing { since: now + spec.delay }, phase0: 0.0,
            amplitude: Animated::new(1.0).with_class(PropClass::Opacity) }
    }
    /// Whether this track has an end: a finite sequence plays once and then rests.
    fn finite(&self) -> bool {
        matches!(self.spec.kind, LoopKind::Sequence(_))
    }
    pub fn phase(&self, now: f64) -> f64 {
        let since = match self.phase {
            Phase::Playing { since } => since,
            // A played-out sequence rests at its last pose.
            Phase::Done => return 1.0,
        };
        if self.finite() {
            return ((now - since) / self.spec.period).clamp(0.0, 1.0);
        }
        (self.phase0 + (now - since).max(0.0) / self.spec.period).rem_euclid(1.0)
    }
    /// Preserve the presented phase when changing speed. An unchanged spec is a no-op.
    pub fn retime(&mut self, spec: LoopSpec, now: f64) {
        assert!(spec.period.is_finite() && spec.period > 0.0);
        if self.spec == spec { return; }
        if self.finite() || matches!(spec.kind, LoopKind::Sequence(_)) {
            *self = Self::new(spec, now);
            return;
        }
        self.phase0 = self.phase(now);
        match &mut self.phase {
            Phase::Playing { since } => *since = now.max(*since),
            // Only a finite sequence is `Done`, and those restarted above.
            Phase::Done => {}
        }
        self.spec = spec;
    }
    /// Cool to rest without stopping or resetting phase. Reversal inherits envelope velocity.
    /// A finite sequence that cooled mid-play carries on from where its phase had reached; one
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
    /// loop still waiting starts `dt` sooner. Finite sequences keep their place.
    pub fn advance(&mut self, dt: f64) {
        if !self.finite() && let Phase::Playing { since } = &mut self.phase { *since -= dt; }
    }
    /// Explicit settle/reduced motion lands finite sequences; periodic loops keep their phase.
    pub fn finish(&mut self) {
        if self.finite() { self.phase = Phase::Done; }
    }
    pub fn active(&self, now: f64) -> bool {
        match self.phase {
            Phase::Done => false,
            // A cooled sequence keeps its phase clock but draws nothing, so it asks for no
            // frames.
            Phase::Playing { since } if self.finite() => {
                self.amplitude.target() > 0.0 && now >= since && now < since + self.spec.period
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
        let t = if reduced { 0.5 } else { easing.eval(self.phase(now) as f32) };
        let a = self.amplitude.sample(now);
        Some([0, 1].map(|i| (from[i] + (to[i] - from[i]) * t) * a))
    }
    pub fn pose(&self, now: f64, reduced: bool) -> Option<Beat> {
        let LoopKind::Sequence(frames) = self.spec.kind else { return None };
        let at = if reduced || self.phase == Phase::Done { frames.duration() } else { self.phase(now) * frames.duration() };
        Some(frames.sample(at))
    }
}

#[cfg(test)]
#[path = "looping_tests.rs"]
mod tests;
