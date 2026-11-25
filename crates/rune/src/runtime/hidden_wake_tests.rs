//! Covered, hidden and non-drawable wakes whose `poll` (or Playground step) queues input:
//! handlers hear the input in that same wake with the poll's clock, bounded against loops,
//! without drawing or submitting to the GPU.

use std::time::Duration;

use super::*;
use crate::window::covered_step;
use crate::{Cx, El, Env, Key, KeyEvent, Mods, Spring, col, el, text};

fn install_clock<S>(rt: &mut Runtime<S>) {
    rt.wall_start = Some(crate::time::now());
    rt.frame.set_test_clock(crate::ManualClock::new(60.0));
}

fn reach_next_timer<S>(rt: &mut Runtime<S>) {
    let due = rt.wake_deadline().expect("a timer is armed");
    let clock = rt.frame.test_clock_mut().expect("test clock installed");
    clock.seek(clock.now().max(due + 1e-4));
}

#[derive(Default)]
struct CoveredTabApp {
    next: f64,
    sent: u32,
    polls: Vec<f64>,
    focuses: Vec<(&'static str, f64)>,
}

fn covered_tab_view(_: &CoveredTabApp, _: &mut Cx<CoveredTabApp>) -> El {
    let btn = |key: &'static str| {
        el().key(key)
            .role("button")
            .label(key)
            .focusable(true)
            .size(60.0, 20.0)
            .child(text(key))
            .on_focus_change(move |s: &mut CoveredTabApp, focused, tx: &mut Tx| {
                if focused {
                    s.focuses.push((key, tx.clock()));
                }
            })
    };
    col().children([btn("one"), btn("two"), btn("three"), btn("four"), btn("five"), btn("six")])
}

fn covered_tab_poll(s: &mut CoveredTabApp, now: f64, tx: &mut Tx) -> bool {
    s.polls.push(now);
    if s.next == 0.0 {
        s.next = now + 2.5;
    }
    let due = now >= s.next && s.sent < 5;
    if due {
        s.sent += 1;
        s.next = now + 0.5;
        tx.send_key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    }
    if s.sent < 5 {
        tx.wake_after(Duration::from_secs_f64((s.next - now).max(0.0)));
    }
    due
}

/// Issue #2042 (reopened): a covered window whose timer poll sends Tab five times over six
/// buttons delivers each Tab's focus handler in the SAME wake as its poll, with the poll's clock,
/// without waiting for the next timer or uncover, and with zero GPU draws or submissions.
/// Uncovering finds no stale queued input.
#[test]
fn covered_window_poll_send_key_delivers_each_key_in_same_wake_with_poll_clock() {
    #[cfg(feature = "test-gpu-counters")]
    let gpu = crate::render::Gpu::try_new(crate::render::Gpu::wgpu_instance(), None).ok();
    #[cfg(feature = "test-gpu-counters")]
    let gpu_snapshot = |g: &crate::render::Gpu| {
        let c = g.device.get_internal_counters().hal;
        (
            c.command_encoders.read(),
            c.textures.read(),
            c.buffers.read(),
            c.bind_groups.read(),
        )
    };
    #[cfg(feature = "test-gpu-counters")]
    let gpu_before = gpu.as_ref().map(&gpu_snapshot);

    let mut rt = Runtime::new(CoveredTabApp::default(), covered_tab_view, (300.0, 200.0), Env::default());
    rt.set_tab_visits_controls(true);
    rt.poll = Some(covered_tab_poll);
    install_clock(&mut rt);
    rt.frame(0.0);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(!rt.animating && rt.rests());

    // Cover the window (as the host does on WindowEvent::Occluded(true)) and drain the
    // immediate visibility-change wake armed by set_live_visible(false).
    rt.set_live_visible(false);
    let commits_at_cover = rt.stats.commits;
    reach_next_timer(&mut rt);
    covered_step(&mut rt);
    assert_eq!(rt.stats.commits, commits_at_cover, "visibility-only covered wake builds nothing");
    assert!(rt.state.focuses.is_empty());

    let expected = ["one", "two", "three", "four", "five"];
    for (i, &button) in expected.iter().enumerate() {
        let polls_before = rt.state.polls.len();
        reach_next_timer(&mut rt);
        covered_step(&mut rt);

        let poll_clock = rt.state.polls[polls_before];
        assert_eq!(
            rt.state.polls[polls_before..],
            [poll_clock, poll_clock],
            "Tab {}: covered wake runs initial poll + one post-handler follow-up poll at the same poll clock",
            i + 1
        );
        assert_eq!(
            rt.state.focuses.len(),
            i + 1,
            "Tab {}: focus handler must run in the same covered wake as its poll",
            i + 1
        );
        assert_eq!(
            rt.state.focuses[i],
            (button, poll_clock),
            "Tab {}: focus handler must see the poll's clock",
            i + 1
        );
        assert!(!rt.has_input_waiting(), "Tab {}: queued input drained in the same wake", i + 1);
    }

    // No further timers are armed after the fifth Tab.
    assert_eq!(rt.wake_deadline(), None);

    #[cfg(feature = "test-gpu-counters")]
    if let (Some(g), Some(before)) = (&gpu, gpu_before) {
        assert_eq!(gpu_snapshot(g), before, "covered wakes must perform zero GPU draws or submissions");
    }

    // Uncover: no stale input remains and nothing doubles.
    rt.set_live_visible(true);
    rt.settle_all();
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.focuses.len(), 5, "uncovering must not replay or double any focus event");
    assert!(!rt.has_input_waiting());
}

/// Issue #2042 (reopened): Playground `Play::press` scheduled via `Play::after` in a covered
/// window delivers each pressed key in the same wake as its step, with the step's poll clock.
#[test]
fn covered_window_playground_play_press_delivers_in_same_wake_with_poll_clock() {
    type Scene = (&'static str, fn(&mut CoveredTabApp));
    static SCENES: &[Scene] = &[("Repro/CoveredPlay", |_| {})];
    let mut spec = crate::AppSpec::new(
        "covered_play",
        (800.0, 400.0),
        CoveredTabApp::default,
        covered_tab_view,
    );
    spec.scenes = SCENES;
    let playground = crate::Playground::new()
        .enabled(true)
        .trigger("Repro/CoveredPlay", 'r', "Three Tabs", |_, play| {
            play.focus("one", true);
            play.after(0.5, |s: &mut CoveredTabApp, play| {
                s.polls.push(play.clock());
                play.press(Key::Tab);
            });
            play.after(1.0, |s: &mut CoveredTabApp, play| {
                s.polls.push(play.clock());
                play.press(Key::Tab);
            });
            play.after(1.5, |s: &mut CoveredTabApp, play| {
                s.polls.push(play.clock());
                play.press(Key::Tab);
            });
        });
    let mut rt = crate::App::new(spec)
        .tab_visits_controls(true)
        .playground(playground)
        .headless_runtime((800.0, 400.0), Env::default());
    install_clock(&mut rt);
    rt.frame(0.0);
    rt.open_playground("Repro/CoveredPlay", None);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(
        rt.playground_state::<CoveredTabApp>().unwrap().focuses.last().map(|(k, _)| *k),
        Some("one"),
    );

    // Cover the window while the three Play::after steps run.
    rt.set_live_visible(false);

    for (step_idx, expected) in ["two", "three", "four"].into_iter().enumerate() {
        reach_next_timer(&mut rt);
        covered_step(&mut rt);
        let st = rt.playground_state::<CoveredTabApp>().unwrap();
        let step_clock = *st.polls.last().expect("playground step ran");
        assert_eq!(
            st.focuses.last().copied(),
            Some((expected, step_clock)),
            "Play::press step {}: focus must move in the same covered wake with the step's clock",
            step_idx + 1,
        );
        assert!(!rt.has_input_waiting());
    }

    // Uncover: nothing stale or doubled.
    rt.set_live_visible(true);
    rt.settle_all();
    rt.frame(crate::frame::clock::RESUME_STEP);
    let st = rt.playground_state::<CoveredTabApp>().unwrap();
    assert_eq!(st.focuses.len(), 4, "initial focus + 3 Play::press steps, nothing doubled on uncover");
}

/// Chained keys queued by a handler drain in the same wake with the poll's clock and a single
/// poll; a handler that re-sends a key on every focus change hits `MAX_HIDDEN_INPUT_FRAMES`
/// instead of spinning; a timer requested by a handler stays armed for a later wake; and an idle
/// covered window does zero commits.
#[test]
fn hidden_wake_drains_chained_keys_with_bound_and_honors_handler_timers() {
    #[derive(Default)]
    struct ChainApp {
        polls: u32,
        send_from_poll: bool,
        chain_once: bool,
        loop_forever: bool,
        arm_handler_timer: bool,
        arm_zero_timer: bool,
        focuses: Vec<(&'static str, f64)>,
    }

    fn chain_view(_: &ChainApp, _: &mut Cx<ChainApp>) -> El {
        let btn = |key: &'static str| {
            el().key(key)
                .role("button")
                .label(key)
                .focusable(true)
                .size(60.0, 20.0)
                .on_focus_change(move |s: &mut ChainApp, focused, tx: &mut Tx| {
                    if !focused { return; }
                    s.focuses.push((key, tx.clock()));
                    if s.loop_forever || std::mem::take(&mut s.chain_once) {
                        tx.send_key(KeyEvent { key: Key::Tab, mods: Mods::default() });
                    }
                    if std::mem::take(&mut s.arm_handler_timer) {
                        tx.wake_after(Duration::from_millis(500));
                    }
                    if std::mem::take(&mut s.arm_zero_timer) {
                        tx.wake_after(Duration::ZERO);
                    }
                })
        };
        col().children([btn("a"), btn("b"), btn("c"), btn("d"), btn("e"), btn("f"), btn("g")])
    }

    let mut rt = Runtime::new(ChainApp::default(), chain_view, (240.0, 200.0), Env::default());
    rt.set_tab_visits_controls(true);
    rt.poll = Some(|s, _, tx| {
        s.polls += 1;
        if std::mem::take(&mut s.send_from_poll) {
            tx.send_key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        }
        false
    });
    install_clock(&mut rt);
    rt.frame(0.0);
    rt.frame(crate::frame::clock::RESUME_STEP);
    rt.set_live_visible(false);
    rt.hidden_wake(); // drain visibility wake

    // 1. Idle covered wake: zero commits, zero extra wakes.
    let commits_idle = rt.stats.commits;
    let polls_idle = rt.state.polls;
    assert!(!rt.hidden_wake(), "idle covered poll reports no change");
    assert_eq!(rt.state.polls, polls_idle + 1);
    assert_eq!(rt.stats.commits, commits_idle, "idle covered window builds nothing");
    assert_eq!(rt.wake_deadline(), None, "idle covered window arms no extra wake");

    // 2. Chained key (poll sends Tab -> "a" sends Tab -> "b") plus handler timer + ZERO timer:
    // both handlers run in the SAME wake at the poll's clock, poll runs ONCE, and timers stay armed.
    rt.state.send_from_poll = true;
    rt.state.chain_once = true;
    rt.state.arm_handler_timer = true;
    rt.state.arm_zero_timer = true;
    let polls_before = rt.state.polls;
    let wake_clock = rt.core().now_clock();
    assert!(rt.hidden_wake());
    // 1 initial poll + 1 post-"a" follow-up poll + 1 post-"b" follow-up poll (all inside one frame).
    assert_eq!(rt.state.polls, polls_before + 3, "initial poll + follow-up poll after each chained handler");
    assert_eq!(
        rt.state.focuses,
        [("a", wake_clock), ("b", wake_clock)],
        "chained key drains in the same wake with the poll's clock",
    );
    assert!(!rt.has_input_waiting());
    assert!(
        rt.wake_deadline().is_some_and(|d| d <= wake_clock + 1e-6),
        "wake_after(ZERO) stays armed for the next host wake instead of being consumed inside follow-up",
    );
    assert!(
        rt.frame.wake_deadlines().iter().any(|&d| (d - (wake_clock + 0.5)).abs() < 1e-6),
        "handler's 500 ms timer stays armed for a later wake",
    );

    // 3. Spin bound: a handler that re-sends Tab on every focus change stops at
    // 1 + MAX_POLL_ROUNDS frames, calls poll only ONCE, arms a continuation wake, and logs
    // the debug-only cap warning once.
    rt.frame.clear_wake_deadlines();
    rt.state.focuses.clear();
    rt.state.send_from_poll = true;
    rt.state.loop_forever = true;
    #[cfg(debug_assertions)]
    let _ = crate::tooling::perf_warn::take_log();
    let polls_before = rt.state.polls;
    let commits_before = rt.stats.commits;
    rt.hidden_wake();
    // 1 initial poll + MAX_POLL_ROUNDS follow-up polls inside the single bounded frame.
    assert_eq!(rt.state.polls, polls_before + 1 + frame::drain::MAX_POLL_ROUNDS as u32);
    assert_eq!(
        rt.state.focuses.len(),
        1 + frame::drain::MAX_POLL_ROUNDS,
        "re-sending handler must stop at 1 + MAX_POLL_ROUNDS rounds",
    );
    assert_eq!(
        rt.stats.commits - commits_before,
        (1 + frame::drain::MAX_POLL_ROUNDS) as u64,
        "routed Key::Tab commits once per bounded round inside the single frame",
    );
    assert!(rt.has_input_waiting(), "excess re-sent key remains queued rather than spinning");
    assert!(rt.wake_deadline().is_some(), "hitting the cap schedules a continuation wake");
    #[cfg(debug_assertions)]
    {
        let first_warnings = crate::tooling::perf_warn::take_log();
        assert!(
            first_warnings.iter().any(|w| w.contains("MAX_POLL_ROUNDS")),
            "hitting the cap logs a debug warning naming MAX_POLL_ROUNDS: {first_warnings:?}",
        );
        // A second capped wake on the same thread does not spam the warning again.
        rt.hidden_wake();
        let repeat_warnings = crate::tooling::perf_warn::take_log();
        assert!(
            !repeat_warnings.iter().any(|w| w.contains("MAX_POLL_ROUNDS")),
            "cap warning must be logged once: {repeat_warnings:?}",
        );
    }
}

/// One rule covers other input kinds too (clipboard paste from poll, file drops) and any motion
/// triggered by a covered handler lands settled when the window uncovers rather than animating
/// from a stale start.
#[test]
fn hidden_wake_covers_paste_and_motion_settles_on_uncover() {
    #[derive(Default)]
    struct PasteMotionApp {
        trigger_paste: bool,
        trigger_move_key: bool,
        pasted: Vec<(String, f64)>,
        box_x: f32,
    }

    fn view(s: &PasteMotionApp, _: &mut Cx<PasteMotionApp>) -> El {
        col().size(200.0, 100.0)
            .on_paste(|s: &mut PasteMotionApp, text: &str, tx: &mut Tx| {
                s.pasted.push((text.to_owned(), tx.clock()));
            })
            .child(
                el().key("mover")
                    .role("button")
                    .focusable(true)
                    .size(40.0, 20.0)
                    .on_focus_change(|s: &mut PasteMotionApp, focused, tx: &mut Tx| {
                        if focused {
                            s.box_x = 80.0;
                            tx.motion(Spring::new(0.3, 0.0));
                        }
                    }),
            )
            .child(el().key("box").absolute(s.box_x, 40.0).size(10.0, 10.0))
    }

    let clipboard = crate::clipboard::memory();
    clipboard.set_text("from-hidden-poll");
    let mut rt = Runtime::new(PasteMotionApp::default(), view, (200.0, 100.0), Env::default());
    rt.use_clipboard(&clipboard);
    rt.set_tab_visits_controls(true);
    rt.poll = Some(|s, _, tx| {
        if std::mem::take(&mut s.trigger_paste) {
            tx.paste = true;
        }
        if std::mem::take(&mut s.trigger_move_key) {
            tx.send_key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        }
        false
    });
    install_clock(&mut rt);
    rt.frame(0.0);
    rt.frame(crate::frame::clock::RESUME_STEP);
    rt.set_live_visible(false);
    covered_step(&mut rt); // visibility wake

    // Paste queued from poll runs in the same covered wake with the poll's clock.
    rt.state.trigger_paste = true;
    let paste_clock = rt.core().now_clock();
    covered_step(&mut rt);
    assert_eq!(rt.state.pasted, [("from-hidden-poll".to_owned(), paste_clock)]);
    assert!(!rt.has_input_waiting());

    // Key from poll triggers a spring animation while covered: box stays settled at target when
    // uncovered, never animating from the stale pre-cover position.
    rt.state.trigger_move_key = true;
    covered_step(&mut rt);
    assert_eq!(rt.state.box_x, 80.0);
    assert!(!rt.has_input_waiting());

    rt.set_live_visible(true);
    rt.settle_all();
    rt.frame(crate::frame::clock::RESUME_STEP);
    let x = rt.presented("box").expect("box presented").0[0];
    assert_eq!(x, 80.0, "uncovering lands covered handler motion at its final pose");
    assert!(!rt.animating, "no animation left playing from a stale start after uncover");
}

/// Direct state-machine coverage for `frame::drain` (`step_drain` and `drain_turn`) and the web
/// hidden-tab wake policy.
#[test]
fn pure_hidden_wake_state_machine_and_web_hidden_tab_policy() {
    use crate::platform::web_math::{web_hidden_timer_target, web_wake_covered};
    use frame::drain::{
        DrainEffect, DrainEvent, DrainOutcome, DrainState, MAX_POLL_ROUNDS, drain_turn, step_drain,
    };

    // No work waiting: transitions directly to Done { capped: false } with zero follow-ups.
    let mut s = DrainState::Active;
    assert_eq!(
        step_drain(&mut s, DrainEvent::StepDone { more: false }).into_iter().collect::<Vec<_>>(),
        [],
    );
    assert_eq!(s, DrainState::Done { capped: false });

    // Repeated work: emits 1..=MAX_POLL_ROUNDS follow-ups, then Capped -> Done { capped: true }.
    let mut s = DrainState::Active;
    for round in 1..=MAX_POLL_ROUNDS {
        assert_eq!(
            step_drain(&mut s, DrainEvent::StepDone { more: true }).into_iter().collect::<Vec<_>>(),
            [DrainEffect::FollowUp { round }],
            "follow-up round {round}",
        );
    }
    assert_eq!(
        step_drain(&mut s, DrainEvent::StepDone { more: true }).into_iter().collect::<Vec<_>>(),
        [DrainEffect::Capped],
        "bound stops further follow-up rounds and signals Capped",
    );
    assert_eq!(s, DrainState::Done { capped: true });

    // `drain_turn` drains finite work cleanly and caps unbounded work at MAX_POLL_ROUNDS.
    let mut remaining = 2usize;
    let out = drain_turn(&mut remaining, |r| *r > 0, |r, _| *r -= 1);
    assert_eq!((out, remaining), (DrainOutcome { rounds: 2, capped: false }, 0));

    let mut infinite = 100usize;
    let out = drain_turn(&mut infinite, |r| *r > 0, |r, _| *r -= 1);
    assert_eq!(
        (out, infinite),
        (DrainOutcome { rounds: MAX_POLL_ROUNDS, capped: true }, 100 - MAX_POLL_ROUNDS),
    );

    // Web hidden tab and suspended canvas policy.
    assert!(!web_wake_covered(false, false));
    assert!(web_wake_covered(true, false));
    assert!(web_wake_covered(false, true));
    assert!(web_wake_covered(true, true));
    let frame_floor = crate::window::TIMER_FRAME.as_secs_f64();
    assert!((web_hidden_timer_target(10.0, 10.0) - (10.0 + frame_floor)).abs() < 1e-9);
    assert_eq!(web_hidden_timer_target(10.5, 10.0), 10.5);
}
