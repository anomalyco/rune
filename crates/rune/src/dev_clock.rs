//! Host/logical clock bridge. Stored runtime deadlines always stay in logical seconds.

#[derive(Debug)]
pub struct DevClock {
    pub scale: f64,
    pub paused: bool,
    host: f64,
    logical: f64,
    steps: u32,
}

impl DevClock {
    pub fn new(host: f64, logical: f64) -> Self {
        Self { scale: 1.0, paused: false, host, logical, steps: 0 }
    }

    pub fn step(&mut self) {
        self.paused = true;
        self.steps = self.steps.saturating_add(1);
    }

    /// Returns `(logical, animation)` seconds: the logical clock follows host time, while
    /// animations step by the frame's scaled `dt`. Manual steps count toward both.
    pub fn advance(&mut self, host: Option<f64>, dt: f64) -> (f64, f64) {
        let next = host.unwrap_or(self.host + dt.max(0.0)).max(self.host);
        let steps = std::mem::take(&mut self.steps) as f64 / 60.0;
        let scaled = |seconds: f64| if self.paused { 0.0 } else { seconds * self.scale };
        let (logical, animation) = (scaled(next - self.host) + steps, scaled(dt.max(0.0)) + steps);
        self.host = next;
        self.logical += logical;
        (logical, animation)
    }

    pub fn deadline(&self, logical: f64) -> Option<f64> {
        (!self.paused).then(|| self.host + (logical - self.logical).max(0.0) / self.scale)
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_pause_step_and_resume_keep_deadlines_in_logical_time() {
        let mut c = DevClock::new(10.0, 2.0);
        c.scale = 0.1;
        let (logical, animation) = c.advance(Some(11.0), 0.01);
        assert!((logical - 0.1).abs() < 1e-12);
        assert!((animation - 0.001).abs() < 1e-12, "animations step by the frame's dt, not idle host time");
        assert!((c.deadline(2.5).unwrap() - 15.0).abs() < 1e-12);
        c.paused = true;
        assert_eq!(c.advance(Some(21.0), 0.01), (0.0, 0.0));
        assert_eq!(c.deadline(2.5), None);
        c.step();
        assert_eq!(c.advance(Some(22.0), 0.01), (1.0 / 60.0, 1.0 / 60.0));
        c.paused = false;
        c.scale = 0.5;
        assert_eq!(c.advance(Some(23.0), 0.01), (0.5, 0.005));
        let (logical, animation) = c.advance(None, 0.2);
        assert!((logical - 0.1).abs() < 1e-12 && (animation - 0.1).abs() < 1e-12, "headless: dt drives both");
        assert!((c.logical - (2.7 + 1.0 / 60.0)).abs() < 1e-12);
    }

    #[test]
    fn a_spring_committed_after_idle_starts_from_rest_on_its_first_frame() {
        use crate::{Cx, El, Env, Runtime, Spring, el};
        fn view(n: &f32, _: &mut Cx<f32>) -> El {
            crate::col().child(el().key("box").absolute(*n, 0.0).size(10.0, 10.0))
        }
        let mut rt = Runtime::new(0.0, view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
        rt.wall_start = Some(std::time::Instant::now());
        rt.dev_clock = Some(DevClock::new(0.0, 0.0));
        rt.frame(1.0 / 120.0);
        // Two idle seconds on the host clock, then a commit and the shell's capped first dt.
        rt.wall_start = Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
        rt.update(|n, tx| {
            *n = 80.0;
            tx.motion(Spring::new(0.4, 0.0));
        });
        rt.frame(1.0 / 120.0);
        assert!(rt.clock >= 2.0, "timers and poll still follow host time");
        let x = rt.presented("box").unwrap().0[0];
        assert!(x < 8.0, "first frame after idle jumped to {x}");
        rt.frame(0.2);
        assert!(rt.presented("box").unwrap().0[0] > x);
    }
}
