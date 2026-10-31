use super::*;
use crate::{Cx, El, Env, Key, KeyEvent, Mods, Tx};

const PERIOD: Duration = Duration::from_secs(5);

#[derive(Default)]
struct App { polls: u32, value: u32, keys: u32, step: bool }

fn view(s: &App, _: &mut Cx<App>) -> El {
    crate::col().size(200.0, 50.0).focusable(true)
        .on_key(|s: &mut App, ev, tx| {
            if ev.key != Key::Down { return false; }
            s.keys += 1;
            tx.instant();
            true
        })
        .child(crate::text(s.value.to_string()).key("value"))
}

fn poll(s: &mut App, _: f64, tx: &mut Tx) -> bool {
    s.polls += 1;
    s.value += 1;
    tx.instant();
    tx.wake_after(if s.step { Duration::ZERO } else { PERIOD });
    true
}

fn runtime() -> Runtime<App> {
    let mut rt = Runtime::new(App::default(), view, (200.0, 50.0), Env::default());
    rt.wall_start = Some(Instant::now());
    rt.frame(0.0);
    rt.poll = Some(poll);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(rt.rests());
    rt
}

fn shown(rt: &Runtime<App>) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("value")).unwrap().text.unwrap()
}

fn reach_timer<S>(rt: &mut Runtime<S>) {
    let at = timer_deadline(rt).expect("a timer is armed");
    let jump = at.saturating_duration_since(Instant::now()) + Duration::from_millis(1);
    rt.wall_start = rt.wall_start.map(|start| start - jump);
    assert!(timer_deadline(rt).unwrap() <= Instant::now());
}

fn expire(frame: &mut TimerFrame) {
    let TimerFrame::Waiting { fallback } = frame else { panic!("no redraw waits") };
    *fallback = Some(Instant::now() - TIMER_FRAME);
}

fn frame_floor<S>(rt: &Runtime<S>) -> Instant {
    rt.wall_start.unwrap() + Duration::from_secs_f64(rt.polled_at()) + TIMER_FRAME
}

fn poll_due<S: 'static>(frame: &mut TimerFrame, rt: &mut Runtime<S>) -> bool {
    if !frame.deadline(timer_deadline(rt)).is_some_and(|at| at <= Instant::now()) { return false; }
    frame.poll_runtime(rt)
}

#[test]
fn withheld_stale_redraw_keeps_polling_without_building_or_repeated_requests() {
    let mut rt = runtime();
    rt.update(|s, _| s.value = 20);
    reach_timer(&mut rt);
    assert_eq!(rt.resting_wake(), None, "stale state asks for a redraw without polling first");
    let (polls, commits, old_text) = (rt.state.polls, rt.stats.commits, shown(&rt));
    let mut frame = TimerFrame::default();
    frame.requested(Instant::now() + Duration::from_secs(3600));
    let grace = frame.deadline(timer_deadline(&rt));
    for _ in 0..20 {
        rt.update_if(|_, _| false);
        rt.schedule_pending_wakes();
        frame.requested(Instant::now());
        assert_eq!(frame.deadline(timer_deadline(&rt)), grace, "updates never extend the grace");
    }
    assert!(!poll_due(&mut frame, &mut rt), "another host's wake cannot bypass the grace");
    expire(&mut frame);
    assert!(poll_due(&mut frame, &mut rt));
    for _ in 0..4 {
        assert!(frame.deadline(timer_deadline(&rt)).unwrap() >= frame_floor(&rt));
        reach_timer(&mut rt);
        assert!(poll_due(&mut frame, &mut rt), "later timers poll instead of asking for another redraw");
    }
    assert_eq!(rt.state.polls, polls + 5);
    assert_eq!(rt.stats.commits, commits);
    assert_eq!(shown(&rt), old_text);
    assert!(rt.stale());
    frame = TimerFrame::default();
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(shown(&rt), rt.state.value.to_string(), "the eventual redraw draws the latest state");
    assert!(!frame.poll_runtime(&mut rt), "a delivered redraw restores ordinary timer handling");
}

#[test]
fn withheld_resting_redraw_releases_parked_wakes_including_zero() {
    for step in [false, true] {
        let mut rt = runtime();
        reach_timer(&mut rt);
        rt.state.step = step;
        assert_eq!(rt.resting_wake(), Some(true));
        rt.resting_wake_frame(true);
        assert_eq!(rt.wake_deadline(), None, "the poll's next wake waits for its requested frame");
        let (polls, commits, clock) = (rt.state.polls, rt.stats.commits, rt.polled_at());
        let mut frame = TimerFrame::default();
        frame.requested(Instant::now());
        assert!(frame.deadline(timer_deadline(&rt)).is_some());
        expire(&mut frame);
        assert!(poll_due(&mut frame, &mut rt));
        assert_eq!(rt.state.polls, polls, "the fallback retains the resting poll");
        let delay = if step { Duration::ZERO } else { PERIOD };
        assert_eq!(rt.wake_deadline(), Some(clock + delay.as_secs_f64()), "the original cadence survives");
        for count in 1..=5 {
            reach_timer(&mut rt);
            assert!(poll_due(&mut frame, &mut rt));
            assert_eq!(rt.state.polls, polls + count);
            assert_eq!(rt.wake_deadline(), Some(rt.polled_at() + delay.as_secs_f64()), "only one timer chain remains");
            assert!(frame.deadline(timer_deadline(&rt)).unwrap() >= frame_floor(&rt));
        }
        assert_eq!(rt.state.polls, polls + 5, "background polling continues while redraw is withheld");
        assert_eq!(rt.stats.commits, commits, "polling leaves visual work for the redraw");
    }
}

#[test]
fn prompt_redraw_preserves_the_resting_polls_frame() {
    let mut rt = runtime();
    reach_timer(&mut rt);
    assert_eq!(rt.resting_wake(), Some(true));
    rt.resting_wake_frame(true);
    let polls = rt.state.polls;
    let mut frame = TimerFrame::default();
    frame.requested(Instant::now() + Duration::from_secs(3600));
    assert!(!poll_due(&mut frame, &mut rt));
    frame = TimerFrame::default();
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls, "the redraw uses the resting poll once");
    assert_eq!(shown(&rt), rt.state.value.to_string());
    assert_eq!(frame.deadline(timer_deadline(&rt)), timer_deadline(&rt));
}

#[test]
fn withheld_redraw_delivers_pending_input_once() {
    let mut rt = runtime();
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    reach_timer(&mut rt);
    assert_eq!(rt.resting_wake(), Some(true));
    rt.resting_wake_frame(true);
    let mut frame = TimerFrame::default();
    frame.requested(Instant::now());
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    assert!(rt.has_pending_input());
    expire(&mut frame);
    assert!(poll_due(&mut frame, &mut rt));
    assert_eq!(rt.state.keys, 1);
    assert!(!rt.has_pending_input());
    reach_timer(&mut rt);
    assert!(poll_due(&mut frame, &mut rt));
    frame = TimerFrame::default();
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.keys, 1, "later polls and the resumed redraw never replay input");
    assert_eq!(shown(&rt), rt.state.value.to_string());
    assert!(!frame.poll_runtime(&mut rt));
}

#[test]
fn withheld_loop_redraw_paces_the_still_due_animation_deadline() {
    fn looping(_: &u32, _: &mut Cx<u32>) -> El {
        crate::el().key("loop").size(20.0, 20.0).fill(crate::Color::WHITE)
            .looping(crate::LoopSpec::opacity(1.0, 5.0, 0.5, 1.0))
    }
    let mut rt = Runtime::new(0u32, looping, (50.0, 50.0), Env::default());
    rt.wall_start = Some(Instant::now());
    rt.frame(0.0);
    rt.poll = Some(|polls, _, _| { *polls += 1; false });
    let loop_deadline = rt.wake_deadline().expect("the delayed loop has a wake");
    reach_timer(&mut rt);
    assert_eq!(rt.resting_wake(), None);
    let mut frame = TimerFrame::default();
    frame.requested(Instant::now() - TIMER_FRAME);
    for _ in 0..5 {
        assert!(poll_due(&mut frame, &mut rt));
        assert_eq!(rt.wake_deadline(), Some(loop_deadline), "polling preserves the loop's next frame");
        assert_eq!(frame.deadline(timer_deadline(&rt)), Some(frame_floor(&rt)), "the overdue loop stays paced");
        reach_timer(&mut rt);
    }
    assert_eq!(rt.state, 5);
    frame = TimerFrame::default();
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(rt.animating, "the delivered frame resumes the loop");
    assert!(!frame.poll_runtime(&mut rt));
}
