use super::*;
use crate::{Direction, Env, GrowthSpec, PointerButton, ScrollSpec, col, el, row};

#[test]
fn state_machine_room_rearms_only_when_stuck_with_velocity() {
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let views = (None, Some(NodeId(1)));

    // Room while idle is a no-op.
    let (took, effects) = step(&mut seq, &mut state, Event::Room { now: 1.0 });
    assert!(!took);
    assert!(effects.is_empty());
    assert_eq!(state, State::Idle);

    // Start active inside deadzone: Room does nothing because velocity is zero.
    let (took, _) = step(&mut seq, &mut state, Event::Down { views, origin: (100.0, 100.0), now: 1.0 });
    assert!(took);
    let (_, effects) = step(&mut seq, &mut state, Event::Room { now: 2.0 });
    assert!(effects.is_empty());
    assert!(matches!(state, State::Active { attempt: None, .. }));

    // Move pointer above origin: arms an attempt.
    let (_, effects) = step(&mut seq, &mut state, Event::Moved { at: (100.0, 20.0), now: 2.0 });
    assert_eq!(effects.into_iter().collect::<Vec<_>>(), vec![Effect::WakeAt(2.0)]);
    let State::Active { attempt: Some(first), .. } = state else { panic!("expected armed") };

    // Room while already armed does not disturb the pending attempt.
    let (_, effects) = step(&mut seq, &mut state, Event::Room { now: 2.5 });
    assert!(effects.is_empty());
    assert!(matches!(state, State::Active { attempt: Some(a), .. } if a == first));

    // Hitting the end marks it stuck (attempt: None).
    let (_, effects) = step(&mut seq, &mut state, Event::Stuck);
    assert!(effects.is_empty());
    assert!(matches!(state, State::Active { attempt: None, last, .. } if last == 2.0));

    // Room later at t = 7.0 re-arms with last = 7.0 so the first resumed frame has no dt jump.
    let (_, effects) = step(&mut seq, &mut state, Event::Room { now: 7.0 });
    assert_eq!(effects.into_iter().collect::<Vec<_>>(), vec![Effect::WakeAt(7.0)]);
    let State::Active { attempt: Some(resumed), last, .. } = state else { panic!("expected resumed") };
    assert_ne!(resumed, first);
    assert_eq!(last, 7.0);

    // Moving back inside deadzone clears attempt; Room does not re-arm inside deadzone.
    let (_, effects) = step(&mut seq, &mut state, Event::Moved { at: (100.0, 105.0), now: 7.5 });
    assert!(effects.is_empty());
    let (_, effects) = step(&mut seq, &mut state, Event::Room { now: 8.0 });
    assert!(effects.is_empty());
    assert!(matches!(state, State::Active { attempt: None, .. }));
}

fn scroll_y<S>(rt: &Runtime<S>, key: &str) -> (f64, f64) {
    let i = rt.tree.nodes.iter().position(|n| n.named(key)).expect("scroll viewport");
    let st = rt.scroll.scrolls.get(&rt.tree.nodes[i].id).expect("scroll state");
    (st.offset, st.max)
}

fn scroll_x<S>(rt: &Runtime<S>, key: &str) -> (f64, f64) {
    let i = rt.tree.nodes.iter().position(|n| n.named(key)).expect("scroll viewport");
    let st = rt.scroll.scrolls.get(&rt.tree.nodes[i].id).expect("scroll state");
    (st.x.offset, st.x.max)
}

#[test]
fn autoscroll_resumes_after_virtual_rows_prepend_at_top() {
    if !ENABLED { return; }
    struct Timeline {
        start: usize,
        end: usize,
    }
    fn view(s: &Timeline, _: &mut Cx<Timeline>) -> El {
        let start = s.start;
        let count = s.end - s.start;
        col()
            .key("scroll")
            .size(200.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: true })
            .child(
                col()
                    .key("rows")
                    .w_full()
                    .growth(GrowthSpec::default())
                    .virtual_rows(
                        count,
                        40.0,
                        move |_: &Timeline, i| format!("row-{}", start + i).into(),
                        move |_: &Timeline, i| el().key(format!("row-{}", start + i)).w_full().h(40.0),
                    ),
            )
    }

    let mut rt = Runtime::new(Timeline { start: 40, end: 80 }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (off0, max0) = scroll_y(&rt, "scroll");
    assert!(max0 > 1000.0 && (off0 - max0).abs() < 1.0, "starts pinned at bottom: off={off0} max={max0}");

    // Middle-click and release inside deadzone to latch autoscroll, then move pointer up.
    rt.button_down(100.0, 150.0, PointerButton::Middle);
    rt.button_up(100.0, 150.0, PointerButton::Middle);
    rt.pointer_move(100.0, 30.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.core().autoscroll_cursor().is_some(), "autoscroll latched");

    // Step until autoscroll hits the top (offset == 0) and stops waking.
    for _ in 0..300 {
        rt.frame(1.0 / 60.0);
        if scroll_y(&rt, "scroll").0 == 0.0 && rt.wake_deadline().is_none() {
            break;
        }
    }
    assert_eq!(scroll_y(&rt, "scroll").0, 0.0, "reached top of loaded history");
    assert_eq!(rt.wake_deadline(), None, "stuck at top arms no wake while waiting");
    assert!(rt.core().autoscroll_cursor().is_some(), "autoscroll stays active while waiting");

    // Staying at the top with no new content stays idle.
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.wake_deadline(), None, "remains idle when nothing arrives");

    // Older messages arrive and prepend WITHOUT any pointer movement.
    rt.update(|s, _| s.start = 20);
    rt.frame(1.0 / 60.0);
    let (after_prepend, _) = scroll_y(&rt, "scroll");
    assert!(after_prepend > 400.0, "virtual list anchored offset down when older rows prepended: {after_prepend}");
    assert!(rt.wake_deadline().is_some(), "autoscroll must re-arm once room appears above");

    // Next frames resume scrolling up automatically without moving the pointer.
    let mut moved_up = false;
    for _ in 0..300 {
        rt.frame(1.0 / 60.0);
        let cur = scroll_y(&rt, "scroll").0;
        if cur < after_prepend - 1.0 { moved_up = true; }
        if cur == 0.0 && rt.wake_deadline().is_none() {
            break;
        }
    }
    assert!(moved_up, "autoscroll resumed scrolling upward without pointer movement");
    assert_eq!(scroll_y(&rt, "scroll").0, 0.0, "scrolled cleanly to the new top");
    assert_eq!(rt.wake_deadline(), None, "stops waking again at the new top");
}

#[test]
fn autoscroll_resumes_when_content_appends_at_bottom_and_stays_idle_in_deadzone() {
    if !ENABLED { return; }
    struct Feed {
        height: f32,
        mounted: bool,
    }
    fn view(s: &Feed, _: &mut Cx<Feed>) -> El {
        if !s.mounted { return el().key("empty").size(200.0, 200.0); }
        col()
            .key("feed")
            .size(200.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(el().key("body").w_full().h(s.height))
    }

    let mut rt = Runtime::new(Feed { height: 600.0, mounted: true }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Latch autoscroll downward.
    rt.button_down(100.0, 50.0, PointerButton::Middle);
    rt.button_up(100.0, 50.0, PointerButton::Middle);
    rt.pointer_move(100.0, 170.0);
    rt.frame(1.0 / 60.0);

    for _ in 0..200 {
        rt.frame(1.0 / 60.0);
        let (off, max) = scroll_y(&rt, "feed");
        if (off - max).abs() < 1e-6 && rt.wake_deadline().is_none() {
            break;
        }
    }
    let (off, max) = scroll_y(&rt, "feed");
    assert!((off - max).abs() < 1e-6 && max == 400.0, "reached initial bottom: off={off} max={max}");
    assert_eq!(rt.wake_deadline(), None, "idle at bottom");

    // Append content without moving pointer -> resumes downward.
    rt.update(|s, _| s.height = 1200.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.wake_deadline().is_some(), "re-arms when bottom extends");
    for _ in 0..200 {
        rt.frame(1.0 / 60.0);
        let (off, max) = scroll_y(&rt, "feed");
        if (off - max).abs() < 1e-6 && rt.wake_deadline().is_none() {
            break;
        }
    }
    assert_eq!(scroll_y(&rt, "feed"), (1000.0, 1000.0), "scrolled to new bottom");
    assert_eq!(rt.wake_deadline(), None);

    // Move pointer back inside deadzone, then extend content: must NOT wake.
    rt.pointer_move(100.0, 55.0);
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.height = 1600.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.wake_deadline(), None, "pointer in deadzone must not wake on content growth");
    assert_eq!(scroll_y(&rt, "feed").0, 1000.0);

    // Move pointer back out, reach end, then unmount viewport: autoscroll cancels cleanly.
    rt.pointer_move(100.0, 170.0);
    rt.frame(1.0 / 60.0);
    for _ in 0..200 {
        rt.frame(1.0 / 60.0);
        let (off, max) = scroll_y(&rt, "feed");
        if (off - max).abs() < 1e-6 && rt.wake_deadline().is_none() { break; }
    }
    rt.update(|s, _| s.mounted = false);
    rt.frame(1.0 / 60.0);
    assert!(rt.core().autoscroll_cursor().is_none(), "unmounting viewport cancels stuck autoscroll");
    assert_eq!(rt.wake_deadline(), None);
}

#[test]
fn autoscroll_resumes_horizontal_ltr_and_rtl() {
    if !ENABLED { return; }
    struct Strip {
        width: f32,
        dir: Direction,
    }
    fn view(s: &Strip, _: &mut Cx<Strip>) -> El {
        row()
            .key("strip")
            .dir(s.dir)
            .size(200.0, 100.0)
            .scroll_x()
            .child(el().key("wide").w(s.width).h_full())
    }

    for (dir, pointer_x) in [(Direction::Ltr, 20.0_f32), (Direction::Rtl, 180.0_f32)] {
        let mut rt = Runtime::new(Strip { width: 500.0, dir }, view, (200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);

        // Start autoscroll toward increasing x.offset (in LTR toward(vx) > 0 when vx < 0; in RTL when vx > 0).
        rt.button_down(100.0, 50.0, PointerButton::Middle);
        rt.button_up(100.0, 50.0, PointerButton::Middle);
        rt.pointer_move(pointer_x, 50.0);
        rt.frame(1.0 / 60.0);

        for _ in 0..200 {
            rt.frame(1.0 / 60.0);
            let (off, max) = scroll_x(&rt, "strip");
            if (off - max).abs() < 1e-6 && rt.wake_deadline().is_none() { break; }
        }
        assert_eq!(scroll_x(&rt, "strip"), (300.0, 300.0), "dir={dir:?}: reached initial x end");
        assert_eq!(rt.wake_deadline(), None, "dir={dir:?}: idle at x end");

        // Grow horizontal content without moving pointer -> resumes.
        rt.update(|s, _| s.width = 900.0);
        rt.frame(1.0 / 60.0);
        assert!(rt.wake_deadline().is_some(), "dir={dir:?}: re-arms when x extent grows");
        for _ in 0..200 {
            rt.frame(1.0 / 60.0);
            let (off, max) = scroll_x(&rt, "strip");
            if (off - max).abs() < 1e-6 && rt.wake_deadline().is_none() { break; }
        }
        assert_eq!(scroll_x(&rt, "strip"), (700.0, 700.0), "dir={dir:?}: reached new x end");
        assert_eq!(rt.wake_deadline(), None, "dir={dir:?}: idle again at new x end");
    }
}
