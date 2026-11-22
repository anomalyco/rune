//! `--window-script FILE`: a real window replays a script's input on the wall clock (tooling).
//!
//! The same parsed events a headless `--script` run feeds, landed by the same `script_point` and
//! `script_input`, but timed from the window's first drawn frame and delivered between real
//! frames: an idle gap in the script is a window that really sleeps. While a script replays, the
//! system's pointer events are dropped, so a desktop the system cursor never reaches (a harness's
//! hidden desktop answers every move with a leave) still drives hover and the wheel. What it
//! cannot show is the system's message path: it starts where those messages would arrive.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use winit::event::WindowEvent;

use crate::time::Instant;
use crate::tooling::{Ev, Kind};

static CLAIMED: AtomicBool = AtomicBool::new(false);

/// Without `--window KEY`, the first window the app opens replays the script: true once per process.
pub(in crate::window) fn claim_first() -> bool { !CLAIMED.swap(true, Ordering::SeqCst) }

/// What one replay turn did: the lines it printed, and whether any input ran (a frame is owed).
#[derive(Debug, Default, PartialEq)]
pub(in crate::window) struct Turn {
    pub lines: Vec<String>,
    pub ran: bool,
}

/// How long a window may go without drawing its first frame before the script says it never started.
pub(in crate::window) const NO_FRAME: Duration = Duration::from_secs(10);

pub(in crate::window) struct WindowScript {
    events: Vec<Ev>,
    next: usize,
    /// When the window's host was made: the no-frame wait counts from here.
    created: Instant,
    /// The window's first drawn frame: event times count from here.
    start: Option<Instant>,
    last_down: (f32, f32),
    /// An event could not run, or no frame came (`rune-script-error`): nothing more replays.
    stopped: bool,
    done_told: bool,
}

impl WindowScript {
    pub(in crate::window) fn new(events: Vec<Ev>, created: Instant) -> Self {
        Self { events, next: 0, created, start: None, last_down: (0.0, 0.0), stopped: false, done_told: false }
    }

    /// The window drew its first frame at `at`; later calls change nothing.
    pub(in crate::window) fn start(&mut self, at: Instant) { self.start.get_or_insert(at); }

    /// When the loop should next turn for the script: the next event's time, or, before the first
    /// frame, the moment the no-frame wait runs out. None once it ended.
    pub(in crate::window) fn next_at(&self) -> Option<Instant> {
        if self.stopped { return None; }
        match self.start {
            None => Some(self.created + NO_FRAME),
            Some(_) => self.next_event_at(),
        }
    }

    fn next_event_at(&self) -> Option<Instant> {
        let start = self.start.filter(|_| !self.stopped)?;
        self.events.get(self.next).map(|e| start + Duration::from_secs_f64(e.t.max(0.0)))
    }

    /// The events due at `now`, in script order; each is handed out once. None before the first frame.
    pub(in crate::window) fn take_due(&mut self, now: Instant) -> Vec<Ev> {
        let mut due = Vec::new();
        while let Some(at) = self.next_event_at() && at <= now {
            due.push(self.events[self.next].clone());
            self.next += 1;
        }
        due
    }

    /// Seconds measured since the first drawn frame (before it, since the host was made): what
    /// each printed line carries beside the script's own time.
    pub(in crate::window) fn elapsed(&self, now: Instant) -> f64 {
        now.saturating_duration_since(self.start.unwrap_or(self.created)).as_secs_f64()
    }

    /// Runs the events due at `now`. `step` lands and feeds one event (`script_point`, then
    /// `script_input`): its point, and false for a kind a window does not replay, or why it could
    /// not land. Each printed line carries the script's time, then the measured one. A mark prints
    /// once the events before it ran, before the frame that shows them.
    pub(in crate::window) fn replay(
        &mut self,
        now: Instant,
        mut step: impl FnMut(&Ev, (f32, f32)) -> Result<((f32, f32), bool), String>,
    ) -> Turn {
        let elapsed = self.elapsed(now);
        let mut turn = Turn::default();
        if self.start.is_none() {
            if !self.stopped && now >= self.created + NO_FRAME {
                turn.lines.push(format!("rune-script-error 0 {elapsed:.3} no frame was drawn within {} s: the script never started", NO_FRAME.as_secs()));
                self.stop();
            }
            return turn;
        }
        for e in self.take_due(now) {
            if let Kind::Mark(name) = &e.kind {
                turn.lines.push(format!("rune-mark {name} {} {elapsed:.3}", e.t));
                continue;
            }
            match step(&e, self.last_down) {
                Ok((at, true)) => {
                    if e.kind.is_down() { self.last_down = at; }
                    turn.ran = true;
                }
                Ok((_, false)) => {
                    turn.lines.push(format!("rune-script-error {} {elapsed:.3} a window does not replay {:?}", e.t, e.kind));
                    self.stop();
                    break;
                }
                Err(why) => {
                    turn.lines.push(format!("rune-script-error {} {elapsed:.3} {why}", e.t));
                    self.stop();
                    break;
                }
            }
        }
        if let Some(t) = self.take_done() { turn.lines.push(format!("rune-script-done {t} {elapsed:.3}")); }
        turn
    }

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
    /// Runs the script's events that are due through the same `script_point` and `script_input` a
    /// headless run uses, prints what the turn says, and asks for a frame if any input ran.
    pub(in crate::window) fn replay_window_script(&mut self, now: Instant) {
        use std::io::Write as _;
        let Some(script) = self.window_script.as_mut() else { return };
        let app = &mut self.app;
        let turn = script.replay(now, |e, last_down| {
            let at = crate::tooling::script_point(app.rt(), e, last_down)?;
            let mut handled = false;
            app.with_driver(&mut |rt, driver| handled = crate::tooling::script_input(rt, driver, &e.kind, at));
            Ok((at, handled))
        });
        if !turn.lines.is_empty() {
            let mut out = std::io::stdout().lock();
            for line in &turn.lines { writeln!(out, "{line}").ok(); }
            out.flush().ok();
        }
        if turn.ran && let Some(w) = &mut self.win { w.request_redraw(); }
    }
}

/// Whether a window drops `event`: while it replays a script, the system's pointer.
pub(in crate::window) fn drops(replaying: bool, event: &WindowEvent) -> bool { replaying && is_system_pointer(event) }

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

    fn script(events: serde_json::Value) -> WindowScript { WindowScript::new(load_script(&events), crate::time::now()) }
    fn secs(s: f64) -> Duration { Duration::from_secs_f64(s) }
    fn times(due: &[Ev]) -> Vec<f64> { due.iter().map(|e| e.t).collect() }

    #[test]
    fn events_fire_at_their_wall_clock_time_from_the_first_frame() {
        let t0 = crate::time::now();
        let mut s = WindowScript::new(load_script(&serde_json::json!([
            {"t": 0.0, "type": "move", "x": 10, "y": 20},
            {"t": 0.5, "type": "move", "x": 30, "y": 40},
            {"t": 1.25, "type": "mark", "name": "end"},
        ])), t0);
        assert_eq!(s.next_at(), Some(t0 + NO_FRAME), "before the first frame the loop wakes only for the no-frame check");
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
        // The file is read and parsed with the arguments, before any window opens.
        let file = std::env::temp_dir().join(format!("rune-window-script-{}.json", std::process::id()));
        std::fs::write(&file, r#"[{"t": 0.5, "type": "move", "x": 1, "y": 2}]"#).unwrap();
        let on = crate::tooling::Args::parse_from(vec!["--window-script".into(), file.to_string_lossy().into_owned()]);
        assert_eq!(on.window_script.as_deref(), Some(file.as_path()));
        assert!(!on.headless(), "a real window replays it: it is not a headless run");
        assert_eq!(crate::tooling::read_window_script(&on).map(|events| times(&events)), Some(vec![0.5]));
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn a_relaunch_takes_the_real_pointer_again() {
        let os = |a: &[&str]| a.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let strip = |a: &[&str]| crate::tooling::without_relaunch_drops(os(a));
        let kept = os(&["--server", "http://127.0.0.1:1", "--size", "1280x800"]);
        assert_eq!(strip(&["--server", "http://127.0.0.1:1", "--window-script", "s.json", "--size", "1280x800"]), kept);
        assert_eq!(strip(&["--window-script=s.json", "--server", "http://127.0.0.1:1", "--size", "1280x800"]), kept);
        assert_eq!(strip(&["--server", "http://127.0.0.1:1", "--size", "1280x800", "--window-script"]), kept, "a flag with no value");
        assert_eq!(strip(&["--window", "main", "--window-scripts", "x"]), os(&["--window", "main", "--window-scripts", "x"]), "other flags stay");
        assert_eq!(strip(&["--harness-writes", "--dirs-root", "r", "--trace", "t.json"]), os(&["--harness-writes", "--dirs-root", "r", "--trace", "t.json"]), "isolating and trace flags stay");
        assert!(crate::tooling::RELAUNCH_DROPS.contains(&"--window-script"));
        for flag in crate::tooling::RELAUNCH_DROPS {
            assert_eq!(strip(&["--size", "1280x800", *flag, "v"]), os(&["--size", "1280x800"]), "{flag} VALUE");
            assert_eq!(strip(&[format!("{flag}=v").as_str(), "--size", "1280x800"]), os(&["--size", "1280x800"]), "{flag}=VALUE");
            assert_eq!(strip(&["--", *flag, "v"]), os(&["--"]), "{flag} after a bare --");
        }
    }

    #[test]
    fn the_system_pointer_passes_without_a_script_and_is_dropped_with_one() {
        let left = WindowEvent::CursorLeft { device_id: DeviceId::dummy() };
        assert!(!drops(false, &left), "no script: the system's pointer reaches the app as today");
        assert!(drops(true, &left), "a script replays: the hidden desktop's instant leave cannot clear its hover");
        assert!(!drops(true, &WindowEvent::Focused(true)), "focus, keys and the rest still come from the system");
    }

    #[test]
    fn a_window_that_never_draws_says_so_instead_of_staying_silent() {
        let t0 = crate::time::now();
        let mut s = WindowScript::new(load_script(&serde_json::json!([{"t": 0.0, "type": "move", "x": 1, "y": 1}])), t0);
        let mut fed = 0;
        assert_eq!(s.replay(t0 + secs(9.9), |_, _| { fed += 1; Ok(((0.0, 0.0), true)) }), Turn::default(), "still waiting for a frame");
        let turn = s.replay(t0 + secs(10.0), |_, _| { fed += 1; Ok(((0.0, 0.0), true)) });
        assert_eq!(turn.lines, vec![String::from("rune-script-error 0 10.000 no frame was drawn within 10 s: the script never started")]);
        assert_eq!(fed, 0, "nothing was fed");
        assert_eq!(s.next_at(), None, "the replay has ended");
        s.start(t0 + secs(11.0));
        assert!(s.replay(t0 + secs(12.0), |_, _| Ok(((0.0, 0.0), true))).lines.is_empty(), "a late frame does not restart it");
    }

    #[test]
    fn a_hover_the_script_set_survives_the_system_s_leave() {
        // The window host feeds a system event to the runtime only if `drops` lets it through: on
        // the hidden desktop a `CursorLeft` comes right after every move.
        fn view(_: &u32, _: &mut crate::Cx<u32>) -> crate::El {
            crate::el().key("box").size(40.0, 40.0).fill(crate::Color::WHITE)
        }
        let left = WindowEvent::CursorLeft { device_id: DeviceId::dummy() };
        for replaying in [true, false] {
            let mut rt = crate::runtime::Runtime::new(0u32, view, (100.0, 80.0), crate::runtime::Env::new(true, 1.0));
            rt.frame(0.0);
            let (x, y) = rt.find("box").unwrap();
            rt.pointer_move(x, y);
            rt.frame(1.0 / 60.0);
            assert_eq!(rt.pointer(), Some((x, y)));
            if !drops(replaying, &left) { rt.pointer_leave(); }
            rt.frame(1.0 / 60.0);
            if replaying {
                assert_eq!(rt.pointer(), Some((x, y)), "replaying: the script's pointer stays over the box");
            } else {
                assert_eq!(rt.pointer(), None, "no script: the system's leave clears it, as today");
            }
        }
    }

    #[test]
    fn a_turn_prints_marks_done_and_errors_with_both_times() {
        let mut s = script(serde_json::json!([
            {"t": 0.0, "type": "move", "x": 4, "y": 4},
            {"t": 0.0, "type": "mark", "name": "moved"},
            {"t": 0.2, "type": "move", "x": 6, "y": 6},
        ]));
        let t0 = crate::time::now();
        s.start(t0);
        let mut fed = Vec::new();
        let mut step = |e: &Ev, _: (f32, f32)| -> Result<((f32, f32), bool), String> { fed.push(e.t); Ok(((1.0, 1.0), true)) };
        assert_eq!(s.replay(t0, &mut step), Turn { lines: vec!["rune-mark moved 0 0.000".into()], ran: true });
        assert_eq!(s.replay(t0 + secs(0.25), &mut step), Turn { lines: vec!["rune-script-done 0.2 0.250".into()], ran: true });
        assert_eq!(fed, vec![0.0, 0.2], "the mark feeds nothing");

        let mut s = script(serde_json::json!([{"t": 0.0, "type": "move", "key": "gone"}, {"t": 0.1, "type": "move", "x": 1, "y": 1}]));
        s.start(t0);
        let turn = s.replay(t0 + secs(1.0), |_, _| Err("no element with key \"gone\"".into()));
        assert_eq!(turn, Turn { lines: vec!["rune-script-error 0 1.000 no element with key \"gone\"".into()], ran: false });
        assert_eq!(s.next_at(), None, "an error ends the replay");

        let mut s = script(serde_json::json!([{"t": 0.0, "menu": "Edit/Copy"}]));
        s.start(t0);
        let turn = s.replay(t0, |_, _| Ok(((0.0, 0.0), false)));
        assert!(turn.lines[0].starts_with("rune-script-error 0 0.000 a window does not replay Menu("), "{:?}", turn.lines);
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

        // At zoom 2 a script's window points are half as many view points.
        fn zoomed(_: &u32, cx: &mut crate::Cx<u32>) -> crate::El {
            cx.zoom(2.0);
            crate::el().key("box").size(20.0, 20.0).fill(crate::Color::WHITE)
        }
        let mut rt = crate::runtime::Runtime::new(0u32, zoomed, (100.0, 80.0), crate::runtime::Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.core().zoom(), 2.0);
        let at = crate::tooling::script_point(rt.core(), &Ev { t: 0.0, kind: Kind::Move, pos: Pos::Xy(12.0, 8.0) }, (0.0, 0.0));
        assert_eq!(at, Ok((6.0, 4.0)));
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
