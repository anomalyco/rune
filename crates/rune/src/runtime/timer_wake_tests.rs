//! A shown window's due timer while it rests: `poll` runs alone, and a frame follows only
//! when that poll changed something or anything else wants frames.
use super::*;
use crate::{Cx, El, Env};
use std::time::Duration;

/// The poll's timer, driven on a deterministic `ManualClock`.
const PERIOD: Duration = Duration::from_secs(5);

#[derive(Default)]
struct App {
    polls: u32,
    /// The poll that reports a change; none at 0.
    change_at: u32,
    /// Asks for the next frame (`wake_after(ZERO)`) instead of a quarter second.
    step: bool,
    width: f32,
}

fn view(s: &App, _: &mut Cx<App>) -> El {
    crate::col().children([crate::el().key("box").size(10.0 + s.width, 8.0)])
}

fn poll(s: &mut App, _: f64, tx: &mut Tx) -> bool {
    s.polls += 1;
    tx.wake_after(if s.step { Duration::ZERO } else { PERIOD });
    s.step || s.polls == s.change_at
}

/// Installs a deterministic manual clock while keeping `wall_start` set so `Runtime` takes the
/// windowed frame path (skipping headless `timer_step` sub-stepping). Both `now_clock()` and
/// `wall_clock()` read `ManualClock` ahead of `wall_start`, so real wall time never drifts.
fn install_clock<S>(rt: &mut Runtime<S>) {
    rt.wall_start = Some(crate::time::now());
    rt.frame.set_test_clock(crate::ManualClock::new(60.0));
}

fn runtime(app: App) -> Runtime<App> {
    let mut rt = Runtime::new(app, view, (200.0, 50.0), Env::default());
    rt.poll = Some(poll);
    install_clock(&mut rt);
    rt.frame(0.0);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(!rt.animating);
    rt
}

/// Moves the manual test clock `secs` on.
fn advance<S>(rt: &mut Runtime<S>, secs: f64) {
    let clock = rt.frame.test_clock_mut().expect("test clock installed");
    clock.seek(clock.now() + secs);
}

/// Moves the manual test clock to just past every armed timer.
fn reach_timer<S>(rt: &mut Runtime<S>) {
    let due = rt.frame.wake_deadlines().last().copied().expect("a timer is armed");
    let clock = rt.frame.test_clock_mut().expect("test clock installed");
    clock.seek(clock.now().max(due + 1e-4));
}

/// Moves the manual test clock to just past the earliest armed timer.
fn reach_next_timer<S>(rt: &mut Runtime<S>) {
    let due = rt.wake_deadline().expect("a timer is armed");
    let clock = rt.frame.test_clock_mut().expect("test clock installed");
    clock.seek(clock.now().max(due + 1e-4));
}

/// As the window host's `new_events`: a resting window polls first, `shell` runs what that
/// left (a close may hide the window), and a frame is asked for only when needed. Returns
/// whether it asked; the frame itself comes later.
fn host_wake<S: 'static>(rt: &mut Runtime<S>, shell: impl FnOnce(&mut Runtime<S>) -> bool) -> bool {
    reach_timer(rt);
    let Some(stale) = rt.resting_wake() else { return true };
    let visible = shell(rt);
    let frame = (stale || rt.stale()) && visible;
    rt.resting_wake_frame(frame);
    frame
}

/// A timer wake with nothing between it and its frame. Returns whether it drew.
fn timer_wake<S: 'static>(rt: &mut Runtime<S>) -> bool {
    if !host_wake(rt, |_| true) { return false; }
    rt.frame(crate::frame::clock::RESUME_STEP);
    true
}

#[test]
fn a_resting_window_s_timer_draws_only_for_a_poll_that_changed_something() {
    let mut rt = runtime(App::default());
    let (commits, polls) = (rt.stats.commits, rt.state.polls);
    rt.state.change_at = polls + 9;
    for _ in 0..8 { assert!(!timer_wake(&mut rt), "a poll that returns false builds and draws nothing"); }
    assert_eq!(rt.state.polls, polls + 8, "each timer still polls");
    assert_eq!(rt.stats.commits, commits, "and runs no view");
    let deadline = rt.wake_deadline().unwrap();
    assert!(deadline > rt.clock, "the poll's next timer is armed: {deadline} after {}", rt.clock);

    let frames = (0..6).filter(|_| timer_wake(&mut rt)).count();
    assert_eq!(frames, 1, "the poll that returns true gets exactly one frame");
    assert_eq!(rt.stats.commits, commits + 1);
    assert_eq!(rt.state.polls, polls + 14, "one poll a wake: the frame doesn't poll again");
}

#[test]
fn a_poll_stepping_every_frame_still_draws_every_frame() {
    let mut rt = runtime(App { step: true, ..App::default() });
    let polls = rt.state.polls;
    for _ in 0..10 { assert!(timer_wake(&mut rt), "`wake_after(ZERO)` steps on frames"); }
    assert_eq!(rt.state.polls - polls, 10, "the frames poll; the wakes don't poll first");
    // Steps reported as no change still take their frames, as before.
    rt.poll = Some(|s, _, tx| { s.polls += 1; tx.wake_after(Duration::ZERO); false });
    rt.frame(crate::frame::clock::RESUME_STEP);
    for _ in 0..5 { assert!(timer_wake(&mut rt)); }
    // A resting window's poll that starts stepping takes the next frame.
    let mut rt = runtime(App::default());
    assert!(!timer_wake(&mut rt));
    rt.poll = Some(|s, _, tx| { s.polls += 1; tx.wake_after(Duration::ZERO); false });
    let polls = rt.state.polls;
    assert!(timer_wake(&mut rt), "its first step draws");
    assert!(timer_wake(&mut rt), "and so does every one after");
    assert_eq!(rt.state.polls, polls + 2, "one step each");
}

#[test]
fn motion_in_flight_takes_the_timer_s_frame() {
    let mut rt = runtime(App::default());
    rt.update(|s, _| s.width = 120.0);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(rt.animating, "the box springs to its new width");
    let polls = rt.state.polls;
    assert_eq!(rt.resting_wake(), None);
    assert_eq!(rt.state.polls, polls, "the frame polls, not the wake");
}

#[test]
fn a_wake_before_the_timer_s_frame_gets_a_poll() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    // `AppHandle::wake` lands before the frame (the display link is starting).
    rt.cancel_timer_frame();
    let armed = rt.wake_deadline().expect("the timer poll's own timer counts from it");
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 2, "the frame polls for the wake");
    assert_eq!(rt.frame.wake_deadlines().first(), Some(&armed));
}

#[test]
fn a_timer_due_between_the_poll_and_its_frame_gets_the_frame_s_poll() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    let polled = rt.clock;
    rt.schedule_wakes_from(polled, [Duration::from_millis(2)]);
    advance(&mut rt, 0.005);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 2, "the later timer polls");
    assert!(rt.frame.wake_deadlines().iter().all(|t| *t > rt.clock), "nothing due is left behind: {:?}", rt.frame.wake_deadlines());
    assert!(rt.resting_wake().is_some(), "and the window rests again");
}

#[test]
fn the_timer_s_frame_keeps_the_poll_s_time() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    let polled = rt.clock;
    advance(&mut rt, 0.004);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 1);
    assert!(rt.clock > polled);
    assert_eq!(rt.polled_at(), polled, "timers pace from the poll, not the frame");
    let next = rt.wake_deadline().unwrap();
    assert!((next - (rt.clock + PERIOD.as_secs_f64())).abs() < 1e-9, "the poll's timer counts from the frame that took it");
}

#[test]
fn a_timer_poll_that_leaves_work_for_the_frame_is_its_only_poll() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    // The poll changes nothing, but a command the host runs after it does.
    assert!(host_wake(&mut rt, |rt| { rt.update(|s, _| s.width = 4.0); true }));
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 1, "the frame doesn't poll a second time");
}

#[test]
fn a_timer_poll_that_hides_the_window_asks_no_frame() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(!host_wake(&mut rt, |_| false), "a keep-running window the poll closed");
    let polled = rt.clock;
    assert!((rt.wake_deadline().unwrap() - (polled + PERIOD.as_secs_f64())).abs() < 1e-9, "its timer still runs, hidden");
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 2, "the frame that shows it polls");

    // Hidden or shown after the frame was asked for: the next frame polls, and the timer runs.
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    let polled = rt.clock;
    rt.cancel_timer_frame();
    assert!((rt.wake_deadline().unwrap() - (polled + PERIOD.as_secs_f64())).abs() < 1e-9);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 2);
}

/// The frame a resting wake asked for runs covered or misses its drawable (`covered_step`):
/// the wake's poll stands for it, and its timers count from that poll.
#[test]
fn a_timer_frame_that_runs_covered_does_not_poll_again() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    let polled = rt.clock;
    assert!(rt.hidden_wake(), "the poll's change is still to draw");
    assert_eq!(rt.state.polls, polls + 1, "one poll for the wake");
    assert!((rt.wake_deadline().unwrap() - (polled + PERIOD.as_secs_f64())).abs() < 1e-9, "its timer counts from the poll");
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 2, "the frame that draws next polls");

    // Another timer due by then still polls.
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    let polled = rt.clock;
    rt.schedule_wakes_from(polled, [Duration::from_millis(2)]);
    advance(&mut rt, 0.005);
    rt.hidden_wake();
    assert_eq!(rt.state.polls, polls + 2);
}

#[derive(Default)]
struct StoryClock {
    polls: u32,
    change_at: u32,
    steps: u32,
    keys: Vec<crate::Key>,
}

fn story_clock_view(s: &StoryClock, _: &mut Cx<StoryClock>) -> El {
    crate::col().children([
        crate::text(format!("polls {} steps {}", s.polls, s.steps)),
    ])
}

fn story_clock_poll(s: &mut StoryClock, _: f64, tx: &mut Tx) -> bool {
    s.polls += 1;
    tx.wake_after(PERIOD);
    s.polls == s.change_at
}

#[test]
fn a_resting_window_polls_an_open_playground_story_and_draws_only_on_change() {
    let story = crate::Story::new("Repro/Clock", || StoryClock { change_at: 4, ..StoryClock::default() }, story_clock_view)
        .poll(story_clock_poll);
    let spec = crate::AppSpec::new("story_timer", (800.0, 400.0), App::default, view);
    let mut rt = crate::App::new(spec)
        .playground(crate::Playground::new().enabled(true).story(story))
        .headless_runtime((800.0, 400.0), Env::default());
    install_clock(&mut rt);
    rt.frame(0.0);
    rt.open_playground("Repro/Clock", None);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(!rt.animating && rt.rests(), "window rests with story open");

    let commits = rt.stats.commits;
    let polls = rt.playground_state::<StoryClock>().unwrap().polls;
    assert_eq!(polls, 2, "open_playground and the settle frame each polled once");

    // Next wake (poll 3): returns false, draws nothing, and re-arms the story's timer.
    assert!(!timer_wake(&mut rt), "an unchanged story poll draws no frame");
    assert_eq!(rt.playground_state::<StoryClock>().unwrap().polls, polls + 1, "the story polled");
    assert_eq!(rt.stats.commits, commits, "no view commit ran");
    assert!(rt.wake_deadline().is_some_and(|d| d > rt.clock), "the story's next timer is armed");

    // Next wake (poll 4 == change_at): returns true, draws one frame, and does not double-poll.
    assert!(timer_wake(&mut rt), "a changing story poll draws one frame");
    assert_eq!(rt.playground_state::<StoryClock>().unwrap().polls, polls + 2, "one poll for the wake, not a second on the frame");
    assert_eq!(rt.stats.commits, commits + 1, "one view commit ran");
    assert!(rt.wake_deadline().is_some_and(|d| d > rt.clock), "the story's next timer is armed");

    // Next wake (poll 5): returns false again, draws nothing.
    assert!(!timer_wake(&mut rt), "back to resting without drawing");
    assert_eq!(rt.playground_state::<StoryClock>().unwrap().polls, polls + 3);
    assert_eq!(rt.stats.commits, commits + 1);
}

#[test]
fn a_resting_window_runs_playground_play_after_steps_and_queues_pressed_keys() {
    static SCENES: &[(&str, fn(&mut StoryClock))] = &[("Repro/Steps", |_| {})];
    let mut spec = crate::AppSpec::new("story_steps", (800.0, 400.0), StoryClock::default, story_clock_view);
    spec.scenes = SCENES;
    spec.on_key = Some(|s, ev, _| {
        s.keys.push(ev.key.clone());
        true
    });
    let playground = crate::Playground::new()
        .enabled(true)
        .trigger("Repro/Steps", 'r', "Three steps", |_, play| {
            play.after(5.0, |s: &mut StoryClock, _| s.steps += 1);
            play.after(10.0, |s: &mut StoryClock, play| {
                s.steps += 1;
                play.press(crate::Key::Down);
            });
            play.after(15.0, |s: &mut StoryClock, _| s.steps += 1);
        });
    let mut rt = crate::App::new(spec)
        .playground(playground)
        .headless_runtime((800.0, 400.0), Env::default());
    install_clock(&mut rt);
    rt.frame(0.0);
    rt.open_playground("Repro/Steps", None);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(!rt.animating && rt.rests(), "window rests waiting for Play::after");
    assert_eq!(rt.playground_state::<StoryClock>().unwrap().steps, 0);

    // Step 1 at +5 s.
    assert!(timer_wake(&mut rt), "step 1 draws a frame");
    assert_eq!(rt.playground_state::<StoryClock>().unwrap().steps, 1);
    assert!(rt.playground_state::<StoryClock>().unwrap().keys.is_empty());

    // Step 2 at +10 s presses Key::Down, which routes to the stage's on_key on the frame it asked for.
    assert!(timer_wake(&mut rt), "step 2 draws a frame");
    assert_eq!(rt.playground_state::<StoryClock>().unwrap().steps, 2);
    assert_eq!(rt.playground_state::<StoryClock>().unwrap().keys, [crate::Key::Down]);

    // Step 3 at +15 s.
    assert!(timer_wake(&mut rt), "step 3 draws a frame");
    assert_eq!(rt.playground_state::<StoryClock>().unwrap().steps, 3);
    assert_eq!(rt.wake_deadline(), None, "no timer remains after the last step");
    assert!(rt.rests(), "the window rests again");
}

#[test]
fn an_earlier_app_timer_does_not_draw_before_a_play_after_step_is_due() {
    let story = crate::Story::new("Repro/Delayed", StoryClock::default, story_clock_view)
        .trigger('r', "Delayed", |_, play| {
            play.after(12.0, |s: &mut StoryClock, _| s.steps += 1);
        });
    let mut spec = crate::AppSpec::new("story_mixed", (800.0, 400.0), App::default, view);
    spec.poll = Some(poll);
    let mut rt = crate::App::new(spec)
        .playground(crate::Playground::new().enabled(true).story(story))
        .headless_runtime((800.0, 400.0), Env::default());
    install_clock(&mut rt);
    rt.frame(0.0);
    rt.open_playground("Repro/Delayed", None);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(!rt.animating && rt.rests());

    // Clear every setup frame's +5 s app timer at once; Play::after is not due until +12 s.
    let commits = rt.stats.commits;
    advance(&mut rt, 5.5);
    assert_eq!(rt.resting_wake(), Some(false), "neither app poll nor pending step changed anything");
    rt.resting_wake_frame(false);
    assert_eq!(rt.playground_state::<StoryClock>().unwrap().steps, 0);

    // Next app poll at +10.5 s still draws nothing.
    reach_next_timer(&mut rt);
    assert_eq!(rt.resting_wake(), Some(false), "second app poll before the step still draws nothing");
    rt.resting_wake_frame(false);
    assert_eq!(rt.playground_state::<StoryClock>().unwrap().steps, 0);
    assert_eq!(rt.stats.commits, commits, "no view commit ran before the step was due");

    // At +12 s the Play::after step is due and asks for a frame.
    reach_next_timer(&mut rt);
    assert_eq!(rt.resting_wake(), Some(true), "the due Play::after step asks for a frame");
    rt.resting_wake_frame(true);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.playground_state::<StoryClock>().unwrap().steps, 1);
    assert_eq!(rt.stats.commits, commits + 1);
}

#[derive(Default)]
struct HandlerClockApp {
    polls: Vec<f64>,
    focuses: Vec<(&'static str, f64)>,
    update_clock: Option<f64>,
    send_tab: bool,
    handler_wake_at: Option<f64>,
}

fn handler_clock_view(_: &HandlerClockApp, _: &mut Cx<HandlerClockApp>) -> El {
    let button = |key: &'static str| {
        crate::el()
            .key(key)
            .role("button")
            .label(key)
            .focusable(true)
            .size(60.0, 24.0)
            .on_focus_change(move |s: &mut HandlerClockApp, focused, tx: &mut Tx| {
                if focused {
                    s.focuses.push((key, tx.clock()));
                    if let Some(delay) = s.handler_wake_at.take() {
                        tx.wake_at(tx.clock() + delay);
                    }
                }
            })
    };
    crate::col().children([button("one"), button("two"), button("three")])
}

fn handler_clock_poll(s: &mut HandlerClockApp, now: f64, tx: &mut Tx) -> bool {
    s.polls.push(now);
    tx.wake_after(PERIOD);
    if std::mem::take(&mut s.send_tab) {
        tx.send_key(crate::KeyEvent { key: crate::Key::Tab, mods: crate::Mods::default() });
        true
    } else {
        false
    }
}

fn handler_clock_runtime() -> Runtime<HandlerClockApp> {
    let mut rt = Runtime::new(HandlerClockApp::default(), handler_clock_view, (240.0, 120.0), Env::default());
    rt.set_tab_visits_controls(true);
    rt.poll = Some(handler_clock_poll);
    install_clock(&mut rt);
    rt.frame(0.0);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(!rt.animating && rt.rests());
    rt
}

#[test]
fn handlers_in_a_resting_timer_s_frame_see_the_poll_s_clock() {
    let mut rt = handler_clock_runtime();
    let polls_before = rt.state.polls.len();
    rt.state.send_tab = true;
    rt.state.handler_wake_at = Some(0.5);
    assert!(host_wake(&mut rt, |_| true), "resting poll that sends Tab asks for a frame");
    let polled = *rt.state.polls.last().unwrap();
    assert_eq!(rt.state.polls.len(), polls_before + 1);

    // The frame arrives a fraction of a millisecond (4 ms here) after the resting poll.
    advance(&mut rt, 0.004);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls.len(), polls_before + 1, "the resting timer's frame does not poll again");
    assert!(rt.clock > polled, "the runtime clock advanced to the frame's wall time");
    assert_eq!(
        rt.state.focuses,
        [("one", polled)],
        "handlers in the frame standing for a resting poll must see that poll's clock",
    );
    // A handler calling `tx.wake_at(tx.clock() + 0.5)` still schedules a deadline >= due.
    let expected_due = polled + 0.5;
    let next_due = rt.wake_deadline().expect("handler wake_at is armed");
    assert!(next_due >= expected_due, "scheduled deadline {next_due} must reach requested {expected_due}");

    // On an ordinary frame, poll runs and handlers see that frame's poll clock.
    rt.key(crate::KeyEvent { key: crate::Key::Tab, mods: crate::Mods::default() });
    advance(&mut rt, 0.01);
    rt.frame(crate::frame::clock::RESUME_STEP);
    let ordinary_poll = *rt.state.polls.last().unwrap();
    assert_eq!(rt.state.polls.len(), polls_before + 2);
    assert_eq!(
        rt.state.focuses.last().copied(),
        Some(("two", ordinary_poll)),
        "ordinary frame handlers see that frame's poll clock",
    );
}

#[test]
fn an_update_after_resting_wake_cancels_the_timer_frame_so_clock_never_goes_backwards() {
    let mut rt = handler_clock_runtime();
    let polls_before = rt.state.polls.len();
    rt.state.send_tab = true;
    assert!(host_wake(&mut rt, |_| true));
    let polled = *rt.state.polls.last().unwrap();

    // An update runs between resting_wake_frame(true) and its frame at a later wall clock.
    advance(&mut rt, 0.004);
    rt.update(|s, tx| {
        s.update_clock = Some(tx.clock());
    });
    let update_clock = rt.state.update_clock.unwrap();
    assert!(update_clock > polled);

    // The frame must run a fresh poll instead of running handlers at the older `polled` clock.
    advance(&mut rt, 0.002);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls.len(), polls_before + 2, "an intervening update requires a fresh poll");
    let frame_poll = *rt.state.polls.last().unwrap();
    assert!(frame_poll >= update_clock, "frame poll clock {frame_poll} must not go backwards past {update_clock}");
    assert_eq!(rt.state.focuses, [("one", frame_poll)]);
    // The resting poll's own timer was scheduled from `polled` when the timer frame was cancelled.
    assert!(rt.wake_deadline().is_some_and(|d| d >= polled + PERIOD.as_secs_f64() - 1e-6));
}

#[test]
fn playground_play_press_in_a_resting_timer_s_frame_sees_the_poll_s_clock() {
    type Scene = (&'static str, fn(&mut HandlerClockApp));
    static SCENES: &[Scene] = &[("Repro/Clock", |_| {})];
    let mut spec = crate::AppSpec::new("play_clock", (800.0, 400.0), HandlerClockApp::default, handler_clock_view);
    spec.scenes = SCENES;
    let playground = crate::Playground::new()
        .enabled(true)
        .trigger("Repro/Clock", 'r', "Press Tab", |_, play| {
            play.focus("one", true);
            play.after(5.0, |s: &mut HandlerClockApp, play| {
                s.polls.push(play.clock());
                play.press(crate::Key::Tab);
            });
        });
    let mut rt = crate::App::new(spec)
        .tab_visits_controls(true)
        .playground(playground)
        .headless_runtime((800.0, 400.0), Env::default());
    install_clock(&mut rt);
    rt.frame(0.0);
    rt.open_playground("Repro/Clock", None);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(!rt.animating && rt.rests());

    assert!(host_wake(&mut rt, |_| true), "due Play::after step asks for a frame");
    let polled = *rt.playground_state::<HandlerClockApp>().unwrap().polls.last().unwrap();
    advance(&mut rt, 0.004);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(rt.clock > polled);
    assert_eq!(
        rt.playground_state::<HandlerClockApp>().unwrap().focuses.last().copied(),
        Some(("two", polled)),
        "handlers triggered by Play::press on a resting wake must see the resting poll's clock",
    );
}

#[test]
fn resting_wake_with_queued_input_refuses_to_rest_until_delivered() {
    // Even when poll returns false, a key sent with Tx::send_key marks input waiting
    // and keeps the runtime out of rest until a frame delivers it.
    let mut rt = handler_clock_runtime();
    rt.poll = Some(|s, now, tx| {
        s.polls.push(now);
        tx.wake_after(PERIOD);
        if std::mem::take(&mut s.send_tab) {
            tx.send_key(crate::KeyEvent { key: crate::Key::Tab, mods: crate::Mods::default() });
        }
        false
    });
    rt.state.send_tab = true;
    reach_timer(&mut rt);
    assert_eq!(rt.resting_wake(), Some(true), "a poll returning false that sends a key still asks for a frame");
    rt.resting_wake_frame(true);
    assert!(rt.has_input_waiting(), "queued Tab must report has_input_waiting");
    assert!(rt.has_pending_input(), "queued Tab must report has_pending_input");
    assert!(rt.stale(), "queued Tab must keep runtime stale");
    assert!(!rt.rests(), "runtime must not rest while user input is queued");
    assert_eq!(rt.resting_wake(), None, "a second wake while input is queued must draw, not poll alone");

    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.focuses.len(), 1, "frame delivered the queued Tab");
    assert!(!rt.has_input_waiting(), "input queue drains once delivered");
    assert!(rt.rests(), "runtime rests again after input is delivered");

    // Same invariant for Playground Play::press on a resting wake.
    type Scene = (&'static str, fn(&mut HandlerClockApp));
    static SCENES: &[Scene] = &[("Repro/Guard", |_| {})];
    let mut spec = crate::AppSpec::new("play_guard", (800.0, 400.0), HandlerClockApp::default, handler_clock_view);
    spec.scenes = SCENES;
    let playground = crate::Playground::new()
        .enabled(true)
        .trigger("Repro/Guard", 'r', "Press Tab", |_, play| {
            play.focus("one", true);
            play.after(5.0, |_: &mut HandlerClockApp, play| {
                play.press(crate::Key::Tab);
            });
        });
    let mut pg = crate::App::new(spec)
        .tab_visits_controls(true)
        .playground(playground)
        .headless_runtime((800.0, 400.0), Env::default());
    install_clock(&mut pg);
    pg.frame(0.0);
    pg.open_playground("Repro/Guard", None);
    pg.frame(crate::frame::clock::RESUME_STEP);
    assert!(pg.rests());

    reach_timer(&mut pg);
    assert_eq!(pg.resting_wake(), Some(true), "Play::press on a resting wake asks for a frame");
    pg.resting_wake_frame(true);
    assert!(pg.has_input_waiting(), "Play::press must report has_input_waiting");
    assert!(!pg.rests(), "runtime must not rest while Play::press input is queued");
    pg.frame(crate::frame::clock::RESUME_STEP);
    assert!(!pg.has_input_waiting());
    assert_eq!(pg.playground_state::<HandlerClockApp>().unwrap().focuses.last().map(|(k, _)| *k), Some("two"));
}
