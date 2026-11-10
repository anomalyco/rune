//! One animation clock for everything: tracks, presence, choreography and traces read it, so slow
//! motion is truthful.

/// Animation time `τ = base + (host − anchor) · scale`, re-anchored whenever the scale changes so
/// running animations continue from where they are.
///
/// - Display mode: call [`Clock::frame`] with the display link's **target presentation timestamp**,
///   so a late frame still lands on the right pose.
/// - Manual mode: [`Clock::step`] advances exactly one frame at `hz`; [`Clock::seek`] jumps. Tests
///   and frame traces are bit-identical run to run.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    base: f64,
    source: Source,
    scale: f64,
    paused: bool,
    frame: u64,
    now: f64,
}

/// What moves the clock.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    /// Host timestamps: τ runs from `base` at host time `anchor`. No anchor until the first
    /// frame after creation or a seek, so that frame shows `base` exactly.
    Host { anchor: Option<f64> },
    /// [`Clock::step`], `1/hz` seconds a frame.
    Manual { hz: f64 },
}

impl Clock {
    /// Driven by host timestamps (seconds, any epoch).
    pub fn display() -> Clock {
        Clock { base: 0.0, source: Source::Host { anchor: None }, scale: 1.0, paused: false, frame: 0, now: 0.0 }
    }

    /// Driven by `step()` at a fixed rate, starting at τ = 0.
    pub fn manual(hz: f64) -> Clock {
        assert!(hz > 0.0);
        Clock { source: Source::Manual { hz }, ..Clock::display() }
    }

    /// Animation time for a host timestamp, without advancing.
    pub fn time_at(&self, host: f64) -> f64 {
        match self.source {
            Source::Host { anchor: Some(anchor) } if !self.paused => self.base + (host - anchor) * self.scale,
            _ => self.base,
        }
    }

    /// Advances to the frame presented at host time `host` and returns τ.
    pub fn frame(&mut self, host: f64) -> f64 {
        if let Source::Host { anchor: anchor @ None } = &mut self.source {
            *anchor = Some(host);
        }
        self.now = self.time_at(host);
        self.frame += 1;
        self.now
    }

    /// Manual mode: one frame (`1/hz` seconds × scale, or nothing while paused). Returns τ.
    pub fn step(&mut self) -> f64 {
        let Source::Manual { hz } = self.source else { panic!("Clock::step needs Clock::manual") };
        self.frame += 1;
        if !self.paused {
            self.base += self.scale / hz;
        }
        self.now = self.base;
        self.now
    }

    /// Jumps to τ = `t` (scrubbing, deterministic tests).
    pub fn seek(&mut self, t: f64) {
        self.base = t;
        self.now = t;
        if let Source::Host { anchor } = &mut self.source {
            *anchor = None;
        }
    }

    /// Slow motion (< 1) or fast-forward (> 1), clamped to [0.02, 4]. Re-anchors at `host`.
    pub fn set_scale(&mut self, scale: f64, host: f64) {
        let scale = scale.clamp(0.02, 4.0);
        if let Source::Host { anchor: Some(_) } = self.source {
            self.base = self.time_at(host);
            self.source = Source::Host { anchor: Some(host) };
        }
        self.scale = scale;
    }

    pub fn pause(&mut self, host: f64) {
        if !self.paused {
            self.base = self.time_at(host);
            self.paused = true;
        }
    }

    pub fn resume(&mut self, host: f64) {
        if self.paused {
            self.paused = false;
            if let Source::Host { anchor } = &mut self.source {
                *anchor = Some(host);
            }
        }
    }

    pub fn now(&self) -> f64 {
        self.now
    }
    pub fn scale(&self) -> f64 {
        self.scale
    }
    pub fn is_paused(&self) -> bool {
        self.paused
    }
    pub fn frame_count(&self) -> u64 {
        self.frame
    }
    /// The manual frame interval, if any.
    pub fn frame_interval(&self) -> Option<f64> {
        match self.source {
            Source::Manual { hz } => Some(1.0 / hz),
            Source::Host { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_reanchors_without_a_jump() {
        let mut c = Clock::display();
        assert_eq!(c.frame(100.0), 0.0);
        assert!((c.frame(101.0) - 1.0).abs() < 1e-12);
        c.set_scale(0.5, 101.0);
        assert!((c.frame(101.0) - 1.0).abs() < 1e-12);
        assert!((c.frame(103.0) - 2.0).abs() < 1e-12);
        c.pause(103.0);
        assert!((c.frame(110.0) - 2.0).abs() < 1e-12);
        c.resume(110.0);
        assert!((c.frame(112.0) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn seeks_and_pauses_hold_until_a_frame_anchors_them() {
        let mut c = Clock::display();
        // Paused before any frame: the first frame anchors, and time holds.
        c.pause(5.0);
        assert_eq!(c.frame(7.0), 0.0);
        c.resume(9.0);
        assert!((c.frame(10.0) - 1.0).abs() < 1e-12);
        c.seek(4.0);
        assert_eq!(c.time_at(50.0), 4.0, "a seek shows its time until a frame anchors it");
        assert_eq!(c.frame(50.0), 4.0);
        assert!((c.frame(50.5) - 4.5).abs() < 1e-12);
        // A seek while paused shows at once and runs from there on resume.
        c.pause(50.5);
        c.seek(2.0);
        assert_eq!(c.frame(60.0), 2.0);
        c.resume(60.0);
        assert!((c.frame(61.0) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn a_manual_clock_never_reads_host_time() {
        let mut c = Clock::manual(60.0);
        assert_eq!(c.frame(100.0), 0.0);
        assert_eq!(c.frame(101.0), 0.0, "only step moves a manual clock");
        c.set_scale(0.5, 102.0);
        c.pause(103.0);
        c.resume(200.0);
        assert!((c.step() - 0.5 / 60.0).abs() < 1e-12);
        assert_eq!(c.time_at(300.0), c.now());
    }

    #[test]
    fn manual_steps_are_exact() {
        let mut c = Clock::manual(120.0);
        for _ in 0..120 {
            c.step();
        }
        assert!((c.now() - 1.0).abs() < 1e-9);
    }
}
