//! Bounded same-frame follow-up `poll` after non-`live_only` handlers (#2391).

use std::time::Duration;

use super::*;
use crate::window::covered_step;
use crate::{Cx, El, Env, Key, KeyEvent, Latest, Mods, Spring, col, el, text_input};

/// Reporter's case (#2391): `.on_press(|s| s.queued.push(1))` (no `&mut Tx`, no `request_poll`)
/// is drained by `poll` in that same frame before commit, in 1 commit, keeping `CommitWhy::INPUT`
/// so transitions animate under `motion_needs_cause`, and leaving `wake_deadline()` at `None`.
#[test]
fn press_handler_queued_action_runs_in_same_frame_with_motion_cause_and_no_wake() {
    #[derive(Default)]
    struct App {
        queued: Vec<u32>,
        done: Vec<u32>,
        box_x: f32,
        polls: Vec<(f64, bool)>,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        col().size(200.0, 100.0)
            .child(el().key("go").size(60.0, 30.0).on_press(|s: &mut App| s.queued.push(1)))
            .child(el().key("box").absolute(s.box_x, 40.0).size(10.0, 10.0))
    }
    fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
        s.polls.push((now, tx.input_waiting()));
        if s.queued.is_empty() { return false; }
        s.done.append(&mut s.queued);
        s.box_x = 80.0;
        tx.motion(Spring::new(0.3, 0.0));
        true
    }

    let mut rt = Runtime::new(App::default(), view, (200.0, 100.0), Env::default());
    rt.motion.enable_motion_needs_cause();
    rt.poll = Some(poll);
    rt.frame(0.0);
    let (polls0, commits0) = (rt.state.polls.len(), rt.stats.commits);

    rt.pointer_down(20.0, 15.0);
    rt.pointer_up(20.0, 15.0);
    let animating = rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.done, [1], "poll drains handler-queued action in the same frame");
    assert!(rt.state.queued.is_empty());
    assert_eq!(rt.stats.commits, commits0 + 1, "single commit for press + follow-up poll");
    assert_eq!(rt.wake_deadline(), None, "no lingering wake needed");
    assert!(animating, "follow-up poll keeps CommitWhy::INPUT so motion animates");
    let x = rt.presented("box").expect("box presented").0[0];
    assert!(x > 0.0 && x < 80.0, "box animates from 0 toward 80 instead of snapping (got {x})");
    // Both initial poll and follow-up poll saw the same frame clock and input_waiting == true.
    let frame_polls = &rt.state.polls[polls0..];
    assert_eq!(frame_polls.len(), 2, "initial poll + one post-handler follow-up poll");
    assert_eq!(frame_polls[0].0, frame_polls[1].0, "follow-up poll uses the same frame clock");
    assert!(frame_polls[0].1 && frame_polls[1].1, "both polls on an input frame see input_waiting");
}

/// README's documented `Latest::request_if_changed` pattern inside `poll`: typing into a text
/// field (`.on_edit`, which takes no `&mut Tx`) starts the `Latest` request in the same frame
/// without waiting for a mouse move or scheduling a wake.
#[test]
fn latest_request_if_changed_triggered_by_on_edit_runs_in_same_frame_without_wake() {
    struct SearchApp {
        query: crate::TextEdit,
        results: Latest<String>,
        hits: String,
    }
    impl Default for SearchApp {
        fn default() -> Self {
            Self {
                query: crate::TextEdit::new(""),
                results: Latest::default().debounce(Duration::from_secs(60)),
                hits: String::new(),
            }
        }
    }
    fn view(s: &SearchApp, _: &mut Cx<SearchApp>) -> El {
        col().size(240.0, 80.0).child(
            text_input(&s.query, crate::TextStyle::new(14.0, 400.0))
                .key("search")
                .size(200.0, 32.0)
                .on_edit(|s: &mut SearchApp| &mut s.query),
        )
    }
    fn poll(s: &mut SearchApp, _: f64, _: &mut Tx) -> bool {
        let q = s.query.text.clone();
        if !q.is_empty() {
            s.results.request_if_changed(&q, |q| format!("hits:{q}"));
        }
        if let Some(hits) = s.results.take() {
            s.hits = hits;
            return true;
        }
        false
    }

    let mut rt = Runtime::new(SearchApp::default(), view, (240.0, 80.0), Env::default());
    rt.poll = Some(poll);
    rt.frame(0.0);
    // Focus the search field and type "a".
    rt.pointer_down(20.0, 16.0);
    rt.pointer_up(20.0, 16.0);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.results.is_pending());

    rt.key(KeyEvent { key: Key::Text("a".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.query.text, "a");
    assert!(
        rt.state.results.is_pending(),
        "Latest::request_if_changed in poll must start in the same frame as on_edit",
    );
    assert_eq!(rt.wake_deadline(), None, "no manual wake scheduled");
}

/// Chained handler -> poll (`tx.send_key`) -> handler -> poll drains in one frame at one clock.
#[test]
fn chained_handler_poll_send_key_handler_poll_completes_in_one_frame() {
    #[derive(Default)]
    struct Chain {
        stage: u32,
        trace: Vec<(&'static str, f64)>,
    }
    fn view(_: &Chain, _: &mut Cx<Chain>) -> El {
        col().size(200.0, 100.0)
            .child(
                el().key("go")
                    .size(60.0, 24.0)
                    .on_press_tx(|s: &mut Chain, tx: &mut Tx| {
                        s.stage = 1;
                        s.trace.push(("press", tx.clock()));
                    }),
            )
            .child(
                el().key("target")
                    .role("button")
                    .focusable(true)
                    .size(60.0, 24.0)
                    .on_focus_change(|s: &mut Chain, focused, tx: &mut Tx| {
                        if focused {
                            s.stage = 3;
                            s.trace.push(("focus", tx.clock()));
                        }
                    }),
            )
    }
    fn poll(s: &mut Chain, now: f64, tx: &mut Tx) -> bool {
        match s.stage {
            1 => {
                s.stage = 2;
                s.trace.push(("poll1", now));
                tx.send_key(KeyEvent { key: Key::Tab, mods: Mods::default() });
                true
            }
            3 => {
                s.stage = 4;
                s.trace.push(("poll2", now));
                true
            }
            _ => false,
        }
    }

    let mut rt = Runtime::new(Chain::default(), view, (200.0, 100.0), Env::default());
    rt.set_tab_visits_controls(true);
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt.pointer_down(20.0, 12.0);
    rt.pointer_up(20.0, 12.0);
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.stage, 4, "entire handler -> poll -> key -> focus -> poll chain ran in one frame");
    let clock = rt.state.trace[0].1;
    assert_eq!(
        rt.state.trace,
        [("press", clock), ("poll1", clock), ("focus", clock), ("poll2", clock)],
        "every step in the chain ran in order at the same frame clock",
    );
    assert!(!rt.has_input_waiting());
    assert_eq!(rt.wake_deadline(), None);
}

/// Infinite handler <-> poll feedback loop hits `MAX_POLL_ROUNDS` (4), commits cleanly without
/// spinning, schedules a next-frame continuation, and logs the debug warning once.
#[test]
fn infinite_handler_poll_feedback_loop_caps_at_max_poll_rounds_and_warns_once() {
    #[derive(Default)]
    struct LoopApp {
        pending: bool,
        polls: u32,
        focuses: u32,
    }
    fn view(_: &LoopApp, _: &mut Cx<LoopApp>) -> El {
        let btn = |key: &'static str| {
            el().key(key)
                .role("button")
                .focusable(true)
                .size(40.0, 20.0)
                .on_focus_change(|s: &mut LoopApp, focused, _| {
                    if focused {
                        s.focuses += 1;
                        s.pending = true;
                    }
                })
        };
        col().size(200.0, 160.0).children([btn("a"), btn("b"), btn("c"), btn("d"), btn("e"), btn("f")])
    }
    fn poll(s: &mut LoopApp, _: f64, tx: &mut Tx) -> bool {
        s.polls += 1;
        if std::mem::take(&mut s.pending) {
            tx.send_key(KeyEvent { key: Key::Tab, mods: Mods::default() });
            return true;
        }
        false
    }

    let mut rt = Runtime::new(LoopApp::default(), view, (200.0, 160.0), Env::default());
    rt.set_tab_visits_controls(true);
    rt.poll = Some(poll);
    rt.frame(0.0);
    #[cfg(debug_assertions)]
    let _ = crate::tooling::perf_warn::take_log();

    let (polls0, commits0) = (rt.state.polls, rt.stats.commits);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);

    // 1 initial poll + MAX_POLL_ROUNDS follow-up polls; 1 initial focus + MAX_POLL_ROUNDS chained focuses.
    assert_eq!(rt.state.polls - polls0, 1 + frame::drain::MAX_POLL_ROUNDS as u32);
    assert_eq!(rt.state.focuses, 1 + frame::drain::MAX_POLL_ROUNDS as u32);
    assert!(rt.stats.commits > commits0, "frame still committed after hitting cap");
    assert!(rt.wake_deadline().is_some(), "capped frame schedules next-frame continuation");
    #[cfg(debug_assertions)]
    {
        let warns = crate::tooling::perf_warn::take_log();
        assert!(warns.iter().any(|w| w.contains("MAX_POLL_ROUNDS")), "cap warning emitted: {warns:?}");
        rt.frame(1.0 / 60.0);
        let repeat = crate::tooling::perf_warn::take_log();
        assert!(!repeat.iter().any(|w| w.contains("MAX_POLL_ROUNDS")), "cap warning emitted once");
    }
}

/// Covered/hidden window performs handler-queued work in the same wake with zero GPU draws.
#[test]
fn covered_window_performs_handler_queued_work_in_same_wake_with_zero_gpu_draws() {
    #[cfg(feature = "test-gpu-counters")]
    let gpu = crate::render::Gpu::try_new(crate::render::Gpu::wgpu_instance(), None).ok();
    #[cfg(feature = "test-gpu-counters")]
    let snap = |g: &crate::render::Gpu| {
        let c = g.device.get_internal_counters().hal;
        (c.command_encoders.read(), c.textures.read(), c.buffers.read(), c.bind_groups.read())
    };
    #[cfg(feature = "test-gpu-counters")]
    let before = gpu.as_ref().map(&snap);

    #[derive(Default)]
    struct App { queued: Vec<u32>, done: Vec<u32> }
    fn view(_: &App, _: &mut Cx<App>) -> El {
        col().size(120.0, 60.0).child(
            el().key("btn")
                .role("button")
                .focusable(true)
                .size(60.0, 24.0)
                .on_focus_change(|s: &mut App, focused, _| {
                    if focused { s.queued.push(42); }
                }),
        )
    }
    let mut rt = Runtime::new(App::default(), view, (120.0, 60.0), Env::default());
    rt.set_tab_visits_controls(true);
    rt.poll = Some(|s, _, _| {
        if s.queued.is_empty() { return false; }
        s.done.append(&mut s.queued);
        true
    });
    rt.frame(0.0);
    rt.set_live_visible(false);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    covered_step(&mut rt);

    assert_eq!(rt.state.done, [42], "handler-queued work drained in the same covered wake");
    assert!(rt.state.queued.is_empty());
    assert!(!rt.has_input_waiting());
    #[cfg(feature = "test-gpu-counters")]
    if let (Some(g), Some(b)) = (&gpu, before) {
        assert_eq!(snap(g), b, "covered wake performs zero GPU draws or submissions");
    }
}

/// `Tx::request_poll` inside a `live_only` handler triggers a same-frame follow-up poll;
/// without it, `live_only` handlers, layout observers, idle frames, animation frames, and
/// `poll: None` apps run zero extra polls. Outside a frame (`update`), `request_poll` wakes
/// the next turn.
#[test]
fn request_poll_live_only_and_zero_overhead_for_idle_anim_layout_and_no_poll() {
    #[derive(Default)]
    struct App {
        polls: u32,
        queued: Vec<u32>,
        done: Vec<u32>,
        ask_poll: bool,
        layouts: u32,
        box_x: f32,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        col().size(200.0, 100.0)
            .on_layout_tx(|s: &mut App, _, _| s.layouts += 1)
            .child(
                el().key("live")
                    .size(60.0, 30.0)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| {
                        tx.live_only();
                        if s.ask_poll {
                            s.queued.push(7);
                            tx.request_poll();
                        }
                    }),
            )
            .child(el().key("mover").absolute(s.box_x, 40.0).size(10.0, 10.0))
    }
    fn poll(s: &mut App, _: f64, _: &mut Tx) -> bool {
        s.polls += 1;
        if s.queued.is_empty() { return false; }
        s.done.append(&mut s.queued);
        true
    }

    // Check both check_live_only(false) and check_live_only(true) so debug live_only_check commits
    // never trigger a follow-up poll.
    for check_live in [false, true] {
        let mut rt = Runtime::new(App::default(), view, (200.0, 100.0), Env::default());
        rt.check_live_only(check_live);
        rt.poll = Some(poll);
        rt.frame(0.0);
        // Drain initial layout observer frame: layout observer must not trigger a follow-up poll.
        let p_layout = rt.state.polls;
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.polls, p_layout + 1, "check_live={check_live}: layout observer triggers 0 extra polls");

        // Idle frame: 1 poll only.
        let p_idle = rt.state.polls;
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.polls, p_idle + 1, "check_live={check_live}: idle frame triggers 0 extra polls");

        // Animation-only frame: 1 poll only.
        rt.update(|s, tx| { s.box_x = 80.0; tx.motion(Spring::new(0.3, 0.0)); });
        rt.frame(1.0 / 60.0);
        assert!(rt.animating);
        let p_anim = rt.state.polls;
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.polls, p_anim + 1, "check_live={check_live}: animation-only frame triggers 0 extra polls");

        // Wait past LIVE_ONLY_CHECK so debug check fires when check_live == true.
        rt.frame(2.0);
        let p_live = rt.state.polls;
        rt.pointer_down(20.0, 15.0);
        rt.pointer_up(20.0, 15.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.polls,
            p_live + 1,
            "check_live={check_live}: live_only handler without request_poll triggers 0 extra polls",
        );

        // live_only handler WITH tx.request_poll(): runs follow-up poll in the SAME frame!
        rt.state.ask_poll = true;
        let p_req = rt.state.polls;
        rt.pointer_down(20.0, 15.0);
        rt.pointer_up(20.0, 15.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.polls, p_req + 2, "check_live={check_live}: live_only + request_poll runs follow-up poll in same frame");
        assert_eq!(rt.state.done, [7]);
        assert_eq!(rt.wake_deadline(), None);
    }

    // Outside a frame (update): tx.request_poll() schedules a next-turn wake.
    let mut rt = Runtime::new(App::default(), view, (200.0, 100.0), Env::default());
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.wake_deadline(), None);
    rt.update(|s, tx| {
        s.queued.push(99);
        tx.request_poll();
    });
    rt.schedule_pending_wakes();
    assert!(rt.wake_deadline().is_some(), "outside-frame request_poll schedules next turn");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.done, [99]);

    // App with poll: None: 1 frame, 1 commit on press, 0 polls.
    let mut no_poll = Runtime::new(0u32, |_: &u32, _: &mut Cx<u32>| {
        el().key("b").size(60.0, 30.0).on_press(|n: &mut u32| *n += 1)
    }, (100.0, 60.0), Env::default());
    no_poll.frame(0.0);
    let c0 = no_poll.stats.commits;
    no_poll.pointer_down(10.0, 10.0);
    no_poll.pointer_up(10.0, 10.0);
    no_poll.frame(1.0 / 60.0);
    assert_eq!((no_poll.state, no_poll.stats.commits - c0), (1, 1));
    assert_eq!(no_poll.wake_deadline(), None);
}
