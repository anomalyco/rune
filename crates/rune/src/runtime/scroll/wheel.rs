//! A notched mouse wheel's motion, as Chromium animates it on macOS, Windows, and Linux
//! (`cc/animation/scroll_offset_animation_curve.cc`, a mouse wheel's `kInverseDelta` curve):
//! each notch moves the target, and the offset eases there over a time that shrinks as the
//! distance grows (200 ms for 120 device pixels or less, 100 ms for 480 or more), on
//! `cubic-bezier(0.42, 0, 0.58, 1)`. A notch that comes while it moves sends it on from where
//! it is, keeping its speed, so stacked notches run as one motion and a notch back turns it
//! round without a jump. Sampled at whatever time each frame steps, so it runs at the
//! display's rate.
use rune_motion::CubicBezier;

/// Frames of 60 Hz a wheel's ease lasts at most and at least, and the distances in device
/// pixels where it starts and stops shortening (`kInverseDelta*`).
const LONGEST: f64 = 12.0;
const SHORTEST: f64 = 6.0;
const RAMP_START: f64 = 120.0;
const RAMP_END: f64 = 480.0;
/// Closer than this (in device pixels) is there (`kEpsilon`).
const THERE: f64 = 0.01;

/// One notched wheel's ease from `from` to `to` (offsets in points), `t` seconds into a segment
/// of `duration` seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WheelCurve {
    from: f64,
    to: f64,
    t: f64,
    duration: f64,
    ease: CubicBezier,
    /// Set since a notch began or retargeted it, until the first frame that steps it: time a
    /// frame catches up for motion that was already running is none of its.
    fresh: bool,
    /// The offset it last put the viewport at: one that differs when the ease next steps was
    /// moved under it (rows above measured or arrived, an anchor kept what's on screen).
    applied: f64,
    /// Its target was the viewport's end when a notch set it: a stick-to-bottom viewport whose end
    /// grows under it (a reply streaming) takes its target with the end, and lands pinned.
    to_end: bool,
}

impl WheelCurve {
    /// An ease from `from` to `to`, `scale` device pixels to a point.
    pub(crate) fn new(from: f64, to: f64, scale: f64) -> Self {
        WheelCurve { from, to, t: 0.0, duration: duration(to - from, scale), ease: CubicBezier::new(0.42, 0.0, 0.58, 1.0), fresh: true, applied: from, to_end: false }
    }

    /// No ease, at rest at `at`: where a notch's ease starts from when none is running. It is
    /// finished, so a notch that sends it nowhere new moves nothing and changes no mode.
    pub(crate) fn still(at: f64) -> Self {
        WheelCurve { from: at, to: at, t: 0.0, duration: 0.0, ease: CubicBezier::new(0.42, 0.0, 0.58, 1.0), fresh: false, applied: at, to_end: false }
    }

    /// The viewport's offset is `offset` before the ease steps: whatever moved it since the ease
    /// last did (rows above measured, an anchor holding what's on screen) moves the whole ease
    /// with it, so the content under the reader doesn't jump back (Chromium's `ApplyAdjustment`).
    pub(crate) fn follow(&mut self, offset: f64) {
        let moved = offset - self.applied;
        if moved != 0.0 {
            self.from += moved;
            self.to += moved;
            self.applied = offset;
        }
    }

    /// The ease put the viewport at `offset`.
    pub(crate) fn applied(&mut self, offset: f64) { self.applied = offset; }

    /// Where it heads.
    pub(crate) fn target(&self) -> f64 { self.to }

    /// Whether a notch sent it to the viewport's end (`Self::aimed_at_end`).
    pub(crate) fn to_end(&self) -> bool { self.to_end }

    /// A notch sent it to 	o: the viewport's end when `at_end`.
    pub(crate) fn aimed_at_end(&mut self, at_end: bool) { self.to_end = at_end; }

    /// The offset now.
    pub(crate) fn value(&self) -> f64 {
        if self.duration <= 0.0 || self.t >= self.duration { return self.to; }
        let (y, _) = self.ease.eval_with_slope((self.t / self.duration) as f32);
        self.from + (self.to - self.from) * f64::from(y)
    }

    /// Its speed now, in points a second.
    pub(crate) fn velocity(&self) -> f64 {
        if self.duration <= 0.0 || self.t >= self.duration { return 0.0; }
        let (_, slope) = self.ease.eval_with_slope((self.t / self.duration) as f32);
        f64::from(slope) * (self.to - self.from) / self.duration
    }

    /// It has reached its target.
    pub(crate) fn done(&self) -> bool { self.t >= self.duration }

    /// Steps it `dt` seconds on: a frame's own step, or (`catch_up`) the time a frame charges to
    /// motion already running, which a notch that came since then doesn't take.
    pub(crate) fn advance(&mut self, dt: f64, catch_up: bool) {
        // Landing at once (reduced motion, or an app that turned the glide off) lands whatever.
        if dt.is_infinite() {
            self.fresh = false;
            self.t = self.duration.max(0.0);
            return;
        }
        // A step of nothing (a commit's `step_scroll(0.0)`) isn't the first frame.
        if dt == 0.0 { return; }
        // An ease from rest starts at its first frame, as Chromium's does (the animation's start
        // time is that frame's, and official's first frame after a notch draws where it was): that
        // frame's catch-up and step both pass, and the next frame moves.
        if self.fresh {
            if !catch_up { self.fresh = false; }
            return;
        }
        self.t = (self.t + dt).min(self.duration.max(0.0));
    }

    /// Sends it on to `to` from where it is now, keeping its speed (`UpdateTarget`). Returns
    /// false when it's already there and nothing is left to move.
    pub(crate) fn retarget(&mut self, to: f64, scale: f64) -> bool {
        // What it last put the viewport at stays: a new segment starts from there.
        let applied = self.applied;
        let moving = self.send_to(to, scale);
        self.applied = applied;
        moving
    }

    fn send_to(&mut self, to: f64, scale: f64) -> bool {
        if ((self.to - to) * scale).abs() < THERE {
            self.to = to;
            return !self.done();
        }
        let at = self.value();
        let delta = to - at;
        if (delta * scale).abs() < THERE {
            *self = WheelCurve { from: to, to, t: 0.0, duration: 0.0, ..*self };
            return false;
        }
        if self.done() || self.duration <= 0.0 {
            *self = WheelCurve::new(at, to, scale);
            return true;
        }
        let v = self.velocity();
        // At this speed it would get there in this long, with room to ease out: a long segment
        // after a fast one would make it rubber-band (`VelocityBasedDurationBound`).
        // Heading the other way, there's no bound.
        let bound = delta / v * 2.5;
        let bound = if (v * scale).abs() < THERE || bound < 0.0 { f64::INFINITY } else { bound };
        let duration = duration(delta, scale).min(bound);
        if duration < THERE {
            *self = WheelCurve { from: to, to, t: 0.0, duration: 0.0, ..*self };
            return false;
        }
        // The new segment starts at the old one's speed (`EaseInOutWithInitialSlope`). It is the
        // same motion going on, so it keeps whether its first frame has come: an ease from rest
        // that a second notch retargets before that frame still starts on it, and one already
        // moving keeps the frame's catch-up (marking it fresh would make the frame after each
        // stacked notch a half step at 60 Hz).
        let slope = (v * duration / delta).clamp(-1000.0, 1000.0) as f32;
        *self = WheelCurve { from: at, to, t: 0.0, duration, ease: CubicBezier::new(0.42, 0.42 * slope, 0.58, 1.0), ..*self };
        true
    }

}

/// `offset` on the device grid (`device_pixel`), except within a device pixel of the end `max`:
/// there the end wins, so a viewport the ease takes to its end reads as at its end (the room
/// test hands the next run on, and a stick-to-bottom viewport pins) rather than a fraction short.
pub(crate) fn on_grid_or_end(offset: f64, max: f64, scale: f64) -> f64 {
    let v = device_pixel(offset, scale);
    if (max - v) * scale < 1.0 { max } else { v.clamp(0.0, max) }
}

/// `offset` points on the nearest whole device pixel, `scale` of them to a point.
pub(crate) fn device_pixel(offset: f64, scale: f64) -> f64 {
    if scale > 0.0 { (offset * scale).round() / scale } else { offset }
}

/// How long a wheel's ease over `delta` points lasts, in seconds.
fn duration(delta: f64, scale: f64) -> f64 {
    let slope = (SHORTEST - LONGEST) / (RAMP_END - RAMP_START);
    let frames = (LONGEST - RAMP_START * slope + (delta * scale).abs() * slope).clamp(SHORTEST, LONGEST);
    frames / 60.0
}
