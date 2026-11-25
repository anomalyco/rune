use super::*;


#[test]
fn window_frame_requests_take_the_change_spring() {
    let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| el(), (100.0, 100.0), Env::new(true, 1.0));
    let spring = |rt: &mut Runtime<()>, f: fn(&mut Tx)| {
        rt.update(move |_, tx| f(tx));
        rt.host.window_frame.take().unwrap().spring
    };
    assert_eq!(spring(&mut rt, |tx| tx.resize_window((1.0, 2.0))), Some(crate::tok::motion::DEFAULT));
    assert_eq!(spring(&mut rt, |tx| { tx.motion(crate::tok::motion::SNAP); tx.resize_window((1.0, 2.0)) }), Some(crate::tok::motion::SNAP));
    assert_eq!(spring(&mut rt, |tx| { tx.instant(); tx.resize_window((1.0, 2.0)) }), None);
    assert_eq!(spring(&mut rt, |tx| tx.resize_window_instant((1.0, 2.0))), None);
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    assert_eq!(spring(&mut rt, |tx| tx.resize_window((1.0, 2.0))), None);
    rt.update(|_, tx| { tx.resize_window((1.0, 2.0)); tx.center_window(); });
    let request = rt.host.window_frame.take().unwrap();
    assert_eq!((request.size, request.center), (Some((1.0, 2.0)), true));
}

#[test]
fn a_window_resize_step_keeps_springs_and_flights_going() {
    fn view(grown: &bool, _: &mut Cx<bool>) -> El {
        let tile = || el().key("tile").matched("tile").size(40.0, 40.0).fill(Color::WHITE);
        let (mut left, mut right) = (el().key("left").size(100.0, 60.0), el().key("right").size(100.0, 60.0).ml(150.0));
        if *grown { right = right.child(tile()) } else { left = left.child(tile()) }
        col().key("root").w_full().h_full()
            .child(el().key("box").size(50.0, if *grown { 150.0 } else { 50.0 }).fill(Color::WHITE))
            .child(left).child(right)
            .child(crate::el::spacer())
            .child(el().key("bar").w_full().h(20.0).fill(Color::WHITE))
    }
    let resize = |user: bool| {
        let mut rt = Runtime::new(false, view, (300.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|grown, tx| { *grown = true; tx.resize_window((300.0, 600.0)) });
        rt.frame(1.0 / 60.0);
        rt.host.window_frame.take();
        if user { rt.set_viewport((300.0, 450.0), 1.0) } else { rt.step_viewport((300.0, 450.0), 1.0) }
        rt.frame(1.0 / 60.0);
        rt
    };
    let rt = resize(false);
    let rect = |rt: &Runtime<bool>, key: &str| rt.presented(key).unwrap().0;
    let h = rect(&rt, "box")[3];
    assert!(h > 50.0 && h < 150.0, "the box's spring keeps going: {h}");
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("tile") && e.moving), "the tile is still in flight");
    let x = rect(&rt, "tile")[0];
    assert!(x > 0.0 && x < 150.0, "mid-flight: {x}");
    assert_eq!(rect(&rt, "bar")[1], 430.0, "content at rest follows the window");
    assert_eq!(rect(&resize(true), "box")[3], 150.0, "a user's resize lands running motion");
}

#[test]
fn a_window_that_fits_its_content_refits_after_a_user_resize() {
    fn view(_: &(), _: &mut Cx<()>) -> El { col().key("root").w_full().child(el().size(100.0, 90.0)) }
    let mut rt = Runtime::new((), view, (300.0, 400.0), Env::new(true, 1.0));
    rt.host.set_fit_height(true);
    let height = rt.content_height();
    rt.set_viewport((300.0, height), 1.0);
    rt.frame(0.0);
    rt.host.window_frame.take();
    // Mid-move, the window's own frame changes aren't a user's resize.
    rt.set_viewport((300.0, height + 40.0), 1.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.host.window_frame.take().is_none());
    rt.host.window_resting = true;
    rt.set_viewport((300.0, height + 100.0), 1.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.host.window_frame.take().and_then(|r| r.size), Some((300.0, height)));
}

/// A window that fits its content, and grew while hidden, learns its new height before it
/// shows: its first frame isn't clipped, then resized in view.
#[test]
fn a_hidden_window_that_fits_its_content_learns_its_height_before_it_shows() {
    fn view(s: &(usize, bool), _: &mut Cx<(usize, bool)>) -> El {
        col().key("root").w_full().children((0..s.0).map(|_| el().size(100.0, 40.0)))
    }
    let mut rt = Runtime::new((1, false), view, (300.0, 400.0), Env::new(true, 1.0));
    rt.host.set_fit_height(true);
    let height = rt.content_height();
    rt.set_viewport((300.0, height), 1.0);
    rt.frame(0.0);
    rt.host.window_frame.take();
    assert!(!rt.build_for_show(), "unchanged, the last frame still shows it");
    rt.poll = Some(|s: &mut (usize, bool), _, _: &mut Tx| {
        if !std::mem::take(&mut s.1) { return false; }
        s.0 += 1;
        true
    });
    rt.state.1 = true;
    let commits = rt.stats.commits;
    assert!(rt.poll_hidden());
    assert!(rt.host.window_frame.is_none() && rt.stats.commits == commits, "hidden, nothing builds yet");
    assert!(rt.build_for_show());
    let request = rt.host.window_frame.take().expect("the new height, before the window shows");
    assert_eq!(request.size, Some((300.0, 80.0)));
    assert!(!request.animate, "it opens at that height");
    rt.set_viewport((300.0, 80.0), 1.0);
    rt.frame(0.0);
    assert_eq!(rt.stats.commits, commits + 1, "the first frame shown has nothing left to build");
    assert!(rt.host.window_frame.is_none());
}

#[test]
fn timed_wakes_are_exact_coalesced_and_do_not_commit_or_animate() {
    use std::time::Duration;
    fn view(_: &Vec<f64>, cx: &mut Cx<Vec<f64>>) -> El {
        cx.time_scale(0.1);
        el()
    }
    fn poll(seen: &mut Vec<f64>, clock: f64, tx: &mut Tx) -> bool {
        seen.push(clock);
        if clock == 0.0 {
            tx.wake_after(Duration::from_millis(250));
            tx.wake_after(Duration::from_millis(250));
            tx.wake_after(Duration::from_millis(375));
        }
        false
    }
    let mut rt = Runtime::new(Vec::new(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.poll = Some(poll);
    assert!(!rt.frame(0.0));
    assert_eq!(rt.frame.wake_deadlines(), [0.25, 0.375]);
    assert!(!rt.frame(0.5));
    assert_eq!(rt.state, [0.0, 0.25, 0.375, 0.5]);
    assert_eq!(rt.commit.commits, 1);
    assert_eq!(rt.wake_deadline(), None);
    assert!((rt.t - 0.05).abs() < 1e-8);
}

#[test]
fn wakes_a_poll_keeps_asking_for_collapse_to_one_poll_a_frame() {
    use std::time::Duration;
    // (polls this frame, delay each poll asks for)
    fn poll(s: &mut (usize, f64), _: f64, tx: &mut Tx) -> bool {
        s.0 += 1;
        tx.wake_after(Duration::from_secs_f64(s.1));
        false
    }
    // A wake every frame, a wake that lands between frames, a save timer that rearms on
    // every poll, and one that lands just past a frame boundary.
    for delay in [0.0, 1.0 / 60.0, 1.0 / 120.0, 0.505, 0.5] {
        fn view(_: &(usize, f64), _: &mut Cx<(usize, f64)>) -> El { el() }
        let mut rt = Runtime::new((0, delay), view, (100.0, 100.0), Env::new(true, 1.0));
        rt.poll = Some(poll);
        for frame in 0..1000 {
            rt.state.0 = 0;
            rt.frame(1.0 / 60.0);
            assert_eq!(rt.state.0, 1, "delay {delay}: frame {frame} polled {} times", rt.state.0);
        }
        assert!(rt.frame.wake_deadlines().len() <= 32, "delay {delay}: {} deadlines pending", rt.frame.wake_deadlines().len());
    }
}

#[test]
fn a_zero_wake_does_not_hide_a_later_wake_in_a_coarse_frame() {
    use std::time::Duration;
    fn poll(s: &mut Vec<f64>, now: f64, tx: &mut Tx) -> bool {
        s.push((now * 1000.0).round() / 1000.0);
        if s.len() == 1 {
            tx.wake_after(Duration::ZERO);
            tx.wake_after(Duration::from_millis(50));
        }
        false
    }
    fn view(_: &Vec<f64>, _: &mut Cx<Vec<f64>>) -> El { el() }
    let mut rt = Runtime::new(Vec::new(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt.frame(0.1);
    assert_eq!(rt.state, [0.0, 0.017, 0.05, 0.1]);
}

#[test]
fn timed_wakes_from_handlers_survive_transaction_merging() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().key("button").size(100.0, 100.0).on_press_tx(|_: &mut (), tx| {
            tx.wake_after(std::time::Duration::from_millis(250));
        })
    }
    let mut rt = Runtime::new((), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(0.1);
    assert_eq!(rt.wake_deadline(), Some(0.35));
    rt.frame(0.25);
    assert_eq!(rt.wake_deadline(), None);
}

#[test]
fn a_titlebar_inset_change_rebuilds_the_view() {
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        crate::row().size(300.0, 40.0).child(el().key("leading").w(cx.titlebar_inset())).child(el().key("title").size(50.0, 20.0))
    }
    let x = |rt: &Runtime<()>| rt.presented("title").unwrap().0[0];
    let mut rt = Runtime::new((), view, (300.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(x(&rt), 0.0, "no traffic lights by default");
    rt.set_titlebar_inset(crate::Chrome::DEFAULT.titlebar_inset());
    rt.frame(0.1);
    rt.frame(5.0);
    assert_eq!(x(&rt), 67.0, "AppKit's default lights end at 7 + 2 × 23 + 14");
    rt.set_titlebar_inset(0.0);
    rt.frame(5.1);
    rt.frame(10.0);
    assert_eq!(x(&rt), 0.0, "full screen hides them");
    let titled = crate::Chrome { full_size_content: false, ..crate::Chrome::DEFAULT };
    assert_eq!(titled.titlebar_inset(), 0.0);
}

#[test]
fn a_new_system_accent_rebuilds_the_view_and_its_memos() {
    // Widths stand in for colours: the view draws what it read.
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        let (accent, focus) = (cx.accent(), cx.focus_color());
        crate::row().size(300.0, 40.0)
            .child(el().key("accent").size(accent.g * 100.0, 10.0))
            .child(crate::memo("ring", 0, || el().key("ring").size(focus.a * 100.0, 10.0)))
    }
    let mut rt = Runtime::new((), view, (300.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let w = |rt: &Runtime<()>, key: &str| (rt.presented(key).unwrap().0[2] * 10.0).round() / 10.0;
    assert_eq!((w(&rt, "accent"), w(&rt, "ring")), (23.1, 50.0), "macOS's dark blue, rings at 50%");
    let pink = crate::Color { r: 1.0, g: 0.2, b: 0.6, a: 1.0 };
    rt.set_system_accent(crate::SystemAccent::new(pink, pink).with_focus(pink, pink.alpha(0.7)));
    rt.frame(0.1);
    rt.frame(5.0);
    assert_eq!((w(&rt, "accent"), w(&rt, "ring")), (20.0, 70.0));
}

#[test]
fn a_new_system_scroll_bar_setting_rebuilds_the_view_and_its_memos() {
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        let overlay = cx.overlay_scroll_bars();
        crate::row().size(300.0, 40.0)
            .child(el().key("cx").size(if overlay { 20.0 } else { 80.0 }, 10.0))
            .child(crate::memo("memo", 0, || {
                let scoped = crate::system_scroll_bars().is_overlay(crate::system_scroller_overlay());
                el().size(if scoped { 30.0 } else { 90.0 }, 10.0)
            }))
    }
    let mut rt = Runtime::new((), view, (300.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let w = |rt: &Runtime<()>, key: &str| rt.presented(key).unwrap().0[2];
    assert_eq!((w(&rt, "cx"), w(&rt, "memo")), (20.0, 30.0), "headless defaults to WhenScrolling");
    rt.set_system_scroll_bars(crate::ScrollBars::Always);
    rt.frame(5.0);
    assert_eq!((w(&rt, "cx"), w(&rt, "memo")), (80.0, 90.0));
    rt.set_system_scroll_bars(crate::ScrollBars::Automatic);
    rt.set_system_scroller_overlay(true);
    rt.frame(5.0);
    assert_eq!((w(&rt, "cx"), w(&rt, "memo")), (20.0, 30.0));
    rt.set_system_scroller_overlay(false);
    rt.frame(5.0);
    assert_eq!((w(&rt, "cx"), w(&rt, "memo")), (80.0, 90.0));
}

#[test]
fn drag_resize_window_populates_frame_request_and_merges_with_resize_window() {
    use crate::{Phase, ResizeEdge};
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().key("grip").size(40.0, 40.0).on_drag(|_: &mut (), d, tx| {
            if d.phase == Phase::Down {
                tx.drag_resize_window(ResizeEdge::SouthEast);
            }
        })
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.host.window_frame.is_none());

    // Triggered from on_drag Phase::Down on a custom grip.
    rt.pointer_move(20.0, 20.0);
    rt.pointer_down(20.0, 20.0);
    rt.frame(1.0 / 60.0);
    let req = rt.host.window_frame.take().expect("drag_resize_window request");
    assert_eq!(req.resize_drag, Some(ResizeEdge::SouthEast));
    assert_eq!(req.size, None);
    assert!(!req.center);
    rt.pointer_up(20.0, 20.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.host.window_frame.is_none(), "Phase::Up does not re-emit resize_drag");

    // Merges cleanly with resize_window and center_window; later drag_resize_window wins.
    rt.update(|_, tx| {
        tx.resize_window((640.0, 480.0));
        tx.drag_resize_window(ResizeEdge::East);
        tx.center_window();
        tx.drag_resize_window(ResizeEdge::NorthWest);
    });
    let merged = rt.host.window_frame.take().expect("merged frame request");
    assert_eq!(merged.size, Some((640.0, 480.0)));
    assert!(merged.center);
    assert_eq!(merged.resize_drag, Some(ResizeEdge::NorthWest));
}

#[test]
fn maximized_and_full_screen_state_rebuilds_view_and_memos_only_on_confirmed_change() {
    use std::cell::Cell;
    use std::rc::Rc;

    struct Counters {
        view_builds: Rc<Cell<usize>>,
        memo_builds: Rc<Cell<usize>>,
        cleared_maximized: Rc<Cell<bool>>,
        cleared_full_screen: Rc<Cell<bool>>,
    }

    fn view(c: &Counters, cx: &mut Cx<Counters>) -> El {
        c.view_builds.set(c.view_builds.get() + 1);
        let max = cx.is_maximized();
        let full = cx.is_full_screen();
        let mut sub = cx.retype::<()>();
        sub.clear_chrome();
        c.cleared_maximized.set(sub.is_maximized());
        c.cleared_full_screen.set(sub.is_full_screen());
        let memo_builds = c.memo_builds.clone();
        crate::row().size(300.0, 40.0)
            .child(el().key("max").size(if max { 80.0 } else { 20.0 }, 10.0))
            .child(el().key("full").size(if full { 90.0 } else { 30.0 }, 10.0))
            .child(crate::memo("memo-state", 0, move || {
                memo_builds.set(memo_builds.get() + 1);
                el().size(if max { 60.0 } else { 10.0 } + if full { 5.0 } else { 0.0 }, 10.0)
            }))
    }

    let view_builds = Rc::new(Cell::new(0));
    let memo_builds = Rc::new(Cell::new(0));
    let cleared_maximized = Rc::new(Cell::new(true));
    let cleared_full_screen = Rc::new(Cell::new(true));
    let mut rt = Runtime::new(
        Counters {
            view_builds: view_builds.clone(),
            memo_builds: memo_builds.clone(),
            cleared_maximized: cleared_maximized.clone(),
            cleared_full_screen: cleared_full_screen.clone(),
        },
        view,
        (300.0, 40.0),
        Env::new(true, 1.0),
    );
    let w = |rt: &Runtime<Counters>, key: &str| rt.presented(key).unwrap().0[2];
    rt.frame(0.0);
    assert_eq!((w(&rt, "max"), w(&rt, "full"), w(&rt, "memo-state")), (20.0, 30.0, 10.0));
    assert_eq!((view_builds.get(), memo_builds.get()), (1, 1));

    // Fire-and-forget requests never flip Cx::is_maximized or Cx::is_full_screen optimistically.
    rt.update(|_, tx| {
        tx.toggle_maximized();
        tx.set_full_screen(true);
    });
    rt.frame(1.0);
    assert_eq!((w(&rt, "max"), w(&rt, "full"), w(&rt, "memo-state")), (20.0, 30.0, 10.0));

    // Setting the already-current state does not mark dirty or rebuild the view or memo.
    let builds_before = (view_builds.get(), memo_builds.get());
    rt.set_maximized(false);
    rt.set_full_screen(false);
    assert!(!rt.core().stale());
    rt.frame(2.0);
    assert_eq!((view_builds.get(), memo_builds.get()), builds_before);

    // Confirmed OS maximize rebuilds the view and invalidates the memo.
    rt.set_maximized(true);
    assert!(rt.core().stale());
    rt.frame(3.0);
    rt.frame(8.0);
    assert_eq!((w(&rt, "max"), w(&rt, "full"), w(&rt, "memo-state")), (80.0, 30.0, 60.0));
    assert_eq!((view_builds.get(), memo_builds.get()), (builds_before.0 + 1, builds_before.1 + 1));
    assert!(!cleared_maximized.get(), "clear_chrome resets is_maximized for staged subviews");

    // Repeating true is a no-op.
    let builds_max = (view_builds.get(), memo_builds.get());
    rt.set_maximized(true);
    assert!(!rt.core().stale());
    rt.frame(9.0);
    assert_eq!((view_builds.get(), memo_builds.get()), builds_max);

    // Confirmed OS full screen rebuilds the view and invalidates the memo.
    rt.set_full_screen(true);
    assert!(rt.core().stale());
    rt.frame(10.0);
    rt.frame(15.0);
    assert_eq!((w(&rt, "max"), w(&rt, "full"), w(&rt, "memo-state")), (80.0, 90.0, 65.0));
    assert_eq!((view_builds.get(), memo_builds.get()), (builds_max.0 + 1, builds_max.1 + 1));
    assert!(!cleared_full_screen.get(), "clear_chrome resets is_full_screen for staged subviews");

    // Exiting both restores both view and memo back to false.
    rt.set_maximized(false);
    rt.set_full_screen(false);
    rt.frame(16.0);
    rt.frame(21.0);
    assert_eq!((w(&rt, "max"), w(&rt, "full"), w(&rt, "memo-state")), (20.0, 30.0, 10.0));
    assert_eq!((view_builds.get(), memo_builds.get()), (builds_max.0 + 2, builds_max.1 + 2));
}

#[test]
fn full_screen_transition_and_failed_revert_keep_inset_and_memo_in_sync() {
    use std::cell::Cell;
    use std::rc::Rc;

    struct State {
        builds: Rc<Cell<usize>>,
        memo_builds: Rc<Cell<usize>>,
    }

    fn view(s: &State, cx: &mut Cx<State>) -> El {
        s.builds.set(s.builds.get() + 1);
        let full = cx.is_full_screen();
        let inset = cx.titlebar_inset();
        let memo_builds = s.memo_builds.clone();
        crate::row().size(300.0, 40.0)
            .child(el().key("inset").size(inset.max(1.0), 10.0))
            .child(crate::memo("titlebar", 0, move || {
                memo_builds.set(memo_builds.get() + 1);
                el().size(if full { 120.0 } else { 40.0 } + inset, 10.0)
            }))
    }

    let builds = Rc::new(Cell::new(0));
    let memo_builds = Rc::new(Cell::new(0));
    let mut rt = Runtime::new(
        State { builds: builds.clone(), memo_builds: memo_builds.clone() },
        view,
        (300.0, 40.0),
        Env::new(true, 1.0),
    );
    let inset = crate::Chrome::DEFAULT.titlebar_inset();
    rt.set_titlebar_inset(inset);
    rt.frame(0.0);
    assert_eq!(rt.presented("inset").unwrap().0[2], inset);
    assert_eq!(rt.presented("titlebar").unwrap().0[2], 40.0 + inset);

    // Transition start (WillEnter): full_screen flips true and titlebar_inset drops to 0.
    rt.set_full_screen(true);
    rt.set_titlebar_inset(0.0);
    assert!(rt.core().stale());
    rt.frame(1.0);
    rt.frame(6.0);
    assert_eq!(rt.presented("inset").unwrap().0[2], 1.0);
    assert_eq!(rt.presented("titlebar").unwrap().0[2], 120.0);
    assert_eq!((builds.get(), memo_builds.get()), (2, 2));

    // Failed enter revert (DidExit / Sync): both flip back and re-run view and memo.
    rt.set_full_screen(false);
    rt.set_titlebar_inset(inset);
    assert!(rt.core().stale());
    rt.frame(7.0);
    rt.frame(12.0);
    assert_eq!(rt.presented("inset").unwrap().0[2], inset);
    assert_eq!(rt.presented("titlebar").unwrap().0[2], 40.0 + inset);
    assert_eq!((builds.get(), memo_builds.get()), (3, 3));
}

#[test]
fn a_handler_that_queues_work_or_requests_a_poll_has_its_work_polled_in_the_same_frame() {
    #[derive(Default)]
    struct App { queued: Vec<u32>, done: Vec<u32>, request: bool }
    fn view(_: &App, _: &mut Cx<App>) -> El {
        el().key("go").size(40.0, 40.0).on_press_tx(|s: &mut App, tx: &mut Tx| {
            s.queued.push(1);
            if s.request { tx.request_poll(); }
        })
    }
    fn poll(s: &mut App, _: f64, _: &mut Tx) -> bool {
        let any = !s.queued.is_empty();
        s.done.append(&mut s.queued);
        any
    }
    for request in [false, true] {
        let mut rt = Runtime::new(App { request, ..App::default() }, view, (100.0, 100.0), Env::new(true, 1.0));
        rt.poll = Some(poll);
        rt.frame(0.0);
        rt.pointer_down(20.0, 20.0);
        rt.pointer_up(20.0, 20.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.done, [1], "request={request}: work queued by a mutating press is polled in the same frame");
        assert!(rt.state.queued.is_empty());
        assert_eq!(rt.wake_deadline(), None, "request={request}: no extra wake is left armed");
    }
}
