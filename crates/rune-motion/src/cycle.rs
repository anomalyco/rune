//! Wall-clock opacity curve used by `LoopSpec::cycle` and Rune's `.cycle()` sugar.

/// A looping opacity curve sampled from the wall clock: `stops` are (loop fraction 0–1, opacity)
/// pairs interpolated linearly; this element is at `frac(t / period - phase)`. Every cycle with
/// the same period stays in phase. Reduced motion holds `still`. Use [`Cycle::DISPLAY_RATE`]
/// for a continuous sweep, or a positive `fps` for an intentionally stepped indicator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cycle {
    pub period: f32,
    pub phase: f32,
    pub stops: &'static [(f32, f32)],
    pub still: f32,
    pub fps: f32,
}

impl Cycle {
    /// Sample on the host's display frames, without arming an independent timer.
    pub const DISPLAY_RATE: f32 = 0.0;

    pub(crate) fn frame_interval(self) -> Option<f64> {
        (self.fps != Self::DISPLAY_RATE).then(|| f64::from(1.0 / self.fps.max(1.0)))
    }

    pub fn sample(&self, t: f64) -> f32 {
        let x = (t / f64::from(self.period) - f64::from(self.phase)).rem_euclid(1.0) as f32;
        let s = self.stops;
        let Some(k) = s.iter().position(|&(at, _)| at >= x) else { return s.last().map_or(1.0, |p| p.1) };
        if k == 0 {
            return s[0].1;
        }
        let ((x0, v0), (x1, v1)) = (s[k - 1], s[k]);
        v0 + (v1 - v0) * (x - x0) / (x1 - x0).max(1e-6)
    }
}
