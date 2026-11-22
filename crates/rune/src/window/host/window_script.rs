//! `--window-script FILE`: a real window replays a script's input on the wall clock (tooling).
//!
//! The same parsed events a headless `--script` run feeds, landed by the same `script_point` and
//! `script_input`, but timed from the window's first drawn frame and delivered between real
//! frames: an idle gap in the script is a window that really sleeps. While a script replays, the
//! system's pointer events are dropped, so a desktop the system cursor never reaches (a harness's
//! hidden desktop answers every move with a leave) still drives hover and the wheel. What it
//! cannot show is the system's message path: it starts where those messages would arrive.

use std::time::Duration;

use winit::event::WindowEvent;

use crate::time::Instant;
use crate::tooling::Ev;

pub(in crate::window) struct WindowScript {
    events: Vec<Ev>,
    next: usize,
    /// The window's first drawn frame: event times count from here.
    start: Option<Instant>,
    last_down: (f32, f32),
    /// An event could not run (`rune-script-error`): nothing more replays.
    stopped: bool,
    done_told: bool,
}

impl WindowScript {
    pub(in crate::window) fn new(events: Vec<Ev>) -> Self {
        Self { events, next: 0, start: None, last_down: (0.0, 0.0), stopped: false, done_told: false }
    }

    /// The window drew its first frame at `at`; later calls change nothing.
    pub(in crate::window) fn start(&mut self, at: Instant) { self.start.get_or_insert(at); }

    /// When the next event is due, while the script replays: the loop sleeps until then.
    pub(in crate::window) fn next_at(&self) -> Option<Instant> {
        let start = self.start.filter(|_| !self.stopped)?;
        self.events.get(self.next).map(|e| start + Duration::from_secs_f64(e.t.max(0.0)))
    }

    /// The events due at `now`, in script order; each is handed out once.
    pub(in crate::window) fn take_due(&mut self, now: Instant) -> Vec<Ev> {
        let mut due = Vec::new();
        while let Some(at) = self.next_at() && at <= now {
            due.push(self.events[self.next].clone());
            self.next += 1;
        }
        due
    }

    pub(in crate::window) fn last_down(&self) -> (f32, f32) { self.last_down }
    pub(in crate::window) fn pressed_at(&mut self, at: (f32, f32)) { self.last_down = at; }

    /// An event could not run: the replay ends here (the system's pointer stays dropped).
    pub(in crate::window) fn stop(&mut self) { self.stopped = true; }

    /// The script's last event time, once, after every event has run.
    pub(in crate::window) fn take_done(&mut self) -> Option<f64> {
        if self.stopped || self.done_told || self.start.is_none() || self.next < self.events.len() { return None; }
        self.done_told = true;
        Some(self.events.last().map_or(0.0, |e| e.t))
    }
}

impl super::Host {
    /// Runs the script's events that are due, then asks for a frame if any ran. A mark prints
    /// `rune-mark NAME T`; an event that cannot run prints `rune-script-error T WHY` and ends the
    /// replay; after the last event, `rune-script-done T`.
    pub(in crate::window) fn replay_window_script(&mut self, now: Instant) {
        use std::io::Write as _;
        use crate::tooling::Kind;
        let Some(script) = self.window_script.as_mut() else { return };
        let due = script.take_due(now);
        let mut ran = false;
        for e in &due {
            if let Kind::Mark(name) = &e.kind {
                println!("rune-mark {name} {}", e.t);
                continue;
            }
            let at = match crate::tooling::script_point(self.app.rt(), e, script.last_down()) {
                Ok(at) => at,
                Err(why) => {
                    println!("rune-script-error {} {why}", e.t);
                    script.stop();
                    break;
                }
            };
            if e.kind.is_down() { script.pressed_at(at); }
            let mut handled = false;
            self.app.with_driver(&mut |rt, app| handled = crate::tooling::script_input(rt, app, &e.kind, at));
            if !handled {
                println!("rune-script-error {} a window does not replay {:?}", e.t, e.kind);
                script.stop();
                break;
            }
            ran = true;
        }
        if let Some(t) = script.take_done() { println!("rune-script-done {t}"); }
        std::io::stdout().flush().ok();
        if ran && let Some(w) = &mut self.win { w.request_redraw(); }
    }
}

/// Pointer input from the system, which a window replaying a script drops.
pub(in crate::window) fn is_system_pointer(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::CursorMoved { .. }
            | WindowEvent::CursorEntered { .. }
            | WindowEvent::CursorLeft { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::Touch(_)
            | WindowEvent::PinchGesture { .. }
            | WindowEvent::PanGesture { .. }
            | WindowEvent::RotationGesture { .. }
            | WindowEvent::DoubleTapGesture { .. }
            | WindowEvent::TouchpadPressure { .. }
    )
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalSize;
    use winit::event::{DeviceId, MouseScrollDelta, TouchPhase};

    use super::*;
    use crate::tooling::{Kind, Pos, load_script};

    fn script(events: serde_json::Value) -> WindowScript { WindowScript::new(load_script(&events)) }
    fn secs(s: f64) -> Duration { Duration::from_secs_f64(s) }
    fn times(due: &[Ev]) -> Vec<f64> { due.iter().map(|e| e.t).collect() }

    #[test]
    fn events_fire_at_their_wall_clock_time_from_the_first_frame() {
        let mut s = script(serde_json::json!([
            {"t": 0.0, "type": "move", "x": 10, "y": 20},
            {"t": 0.5, "type": "move", "x": 30, "y": 40},
            {"t": 1.25, "type": "mark", "name": "end"},
        ]));
        let t0 = crate::time::now();
        assert_eq!(s.next_at(), None, "nothing is due before the window's first frame");
        assert!(s.take_due(t0 + secs(5.0)).is_empty(), "nothing runs before the first frame");

        s.start(t0);
        s.start(t0 + secs(3.0));
        assert_eq!(times(&s.take_due(t0)), vec![0.0]);
        assert_eq!(s.next_at(), Some(t0 + secs(0.5)));
        assert!(s.take_due(t0 + secs(0.49)).is_empty(), "no event runs early");
        assert_eq!(times(&s.take_due(t0 + secs(0.5))), vec![0.5]);
        assert_eq!(s.take_done(), None, "the script is not done while an event waits");
        assert_eq!(times(&s.take_due(t0 + secs(2.0))), vec![1.25]);
        assert_eq!(s.next_at(), None);
        assert_eq!(s.take_done(), Some(1.25));
        assert_eq!(s.take_done(), None, "done is told once");
    }

    #[test]
    fn an_idle_gap_asks_for_nothing_until_the_next_event() {
        // The window sleeps across the gap: no event is due, and the loop's only wake from the
        // replay is the next event's time.
        let mut s = script(serde_json::json!([
            {"t": 0.0, "type": "move", "x": 1, "y": 1},
            {"t": 6.2, "type": "move", "x": 2, "y": 2},
        ]));
        let t0 = crate::time::now();
        s.start(t0);
        assert_eq!(s.take_due(t0).len(), 1);
        for step in 1..62 {
            let now = t0 + secs(f64::from(step) * 0.1 - 0.05);
            assert!(s.take_due(now).is_empty(), "nothing is due at {now:?}");
            assert_eq!(s.next_at(), Some(t0 + secs(6.2)));
        }
        assert_eq!(s.take_due(t0 + secs(6.2)).len(), 1);
    }

    #[test]
    fn a_stopped_replay_runs_nothing_more_and_is_never_done() {
        let mut s = script(serde_json::json!([
            {"t": 0.0, "type": "move", "key": "missing"},
            {"t": 0.1, "type": "move", "x": 1, "y": 1},
        ]));
        let t0 = crate::time::now();
        s.start(t0);
        let due = s.take_due(t0);
        assert!(matches!(due[0].pos, Pos::Key(ref k) if k == "missing"));
        s.stop();
        assert_eq!(s.next_at(), None);
        assert!(s.take_due(t0 + secs(1.0)).is_empty());
        assert_eq!(s.take_done(), None, "an error is not a done");
    }

    #[test]
    fn a_tap_replays_as_its_move_down_and_up() {
        let mut s = script(serde_json::json!([{"t": 0.0, "type": "tap", "x": 5, "y": 6}]));
        let t0 = crate::time::now();
        s.start(t0);
        let kinds: Vec<_> = s.take_due(t0 + secs(1.0)).into_iter().map(|e| e.kind).collect();
        assert!(matches!(kinds.as_slice(), [Kind::Move, Kind::Down, Kind::Up]), "{kinds:?}");
    }

    #[test]
    fn only_the_flag_turns_it_on_and_the_run_keeps_its_window() {
        let off = crate::tooling::Args::parse_from(vec!["--size".into(), "1280x800".into(), "--theme".into(), "light".into()]);
        assert!(off.window_script.is_none());
        assert!(crate::tooling::read_window_script(&off).is_none(), "no flag, no script");
        let on = crate::tooling::Args::parse_from(vec!["--window-script".into(), "s.json".into()]);
        assert_eq!(on.window_script.as_deref(), Some(std::path::Path::new("s.json")));
        assert!(!on.headless(), "a real window replays it: it is not a headless run");
    }

    #[test]
    fn events_land_where_a_headless_run_lands_them() {
        // Both hosts resolve through `script_point`: an element's centre, a point over the zoom,
        // an offset from the last press, and the pointer for a wheel without a point.
        fn view(_: &u32, _: &mut crate::Cx<u32>) -> crate::El {
            crate::el().key("box").size(20.0, 20.0).fill(crate::Color::WHITE)
        }
        let mut rt = crate::runtime::Runtime::new(0u32, view, (100.0, 80.0), crate::runtime::Env::new(true, 1.0));
        rt.frame(0.0);
        let at = |pos: Pos, kind: Kind| crate::tooling::script_point(rt.core(), &Ev { t: 0.0, kind, pos }, (30.0, 40.0));
        assert_eq!(at(Pos::Key("box".into()), Kind::Move), Ok(rt.find("box").expect("the box is laid out")));
        assert_eq!(at(Pos::Key("nothing".into()), Kind::Move), Err(String::from("no element with key \"nothing\"")));
        assert_eq!(at(Pos::Xy(12.0, 7.0), Kind::Move), Ok((12.0, 7.0)), "zoom 1: the window's points");
        assert_eq!(at(Pos::Rel(5.0, -5.0), Kind::Up), Ok((35.0, 35.0)), "a release lands from the last press");
        assert_eq!(at(Pos::Rel(0.0, 0.0), Kind::Wheel(0.0, 10.0)), Ok((0.0, 0.0)), "a wheel without a point lands at the pointer");
    }

    #[test]
    fn the_system_pointer_is_dropped_and_everything_else_passes() {
        let device = DeviceId::dummy();
        for pointer in [
            WindowEvent::CursorLeft { device_id: device },
            WindowEvent::CursorEntered { device_id: device },
            WindowEvent::CursorMoved { device_id: device, position: (1.0, 2.0).into() },
            WindowEvent::MouseWheel { device_id: device, delta: MouseScrollDelta::LineDelta(0.0, 1.0), phase: TouchPhase::Moved },
            WindowEvent::MouseInput { device_id: device, state: winit::event::ElementState::Pressed, button: winit::event::MouseButton::Left },
        ] {
            assert!(is_system_pointer(&pointer), "{pointer:?}");
        }
        for other in [WindowEvent::Focused(true), WindowEvent::Resized(PhysicalSize::new(10, 10)), WindowEvent::RedrawRequested, WindowEvent::CloseRequested] {
            assert!(!is_system_pointer(&other), "{other:?}");
        }
    }
}
