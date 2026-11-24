use crate::*;
use std::rc::{Rc, Weak};
use super::{step, Effect, Event, Watch};

thread_local! { static TOLD: std::cell::Cell<ScrollPos> = std::cell::Cell::new(ScrollPos::default()); }

struct S {
    follow: Live<[f32; 2]>,
    told: std::rc::Rc<std::cell::Cell<usize>>,
}

/// A marker outside the viewport that moves up as far as the content scrolls.
fn view(s: &S, _: &mut Cx<S>) -> El {
    let (follow, told) = (s.follow.clone(), s.told.clone());
    col().size(300.0, 300.0)
        .child(col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
            .on_scroll_offset(move |at: ScrollPos| { TOLD.with(|t| t.set(at)); told.set(told.get() + 1); follow.set([0.0, -at.offset[1]]); })
            .child(el().key("content").w_full().h(1000.0)))
        .child(el().key("marker").size(10.0, 10.0).live_offset(&s.follow))
}

fn y(rt: &Runtime<S>, key: &str) -> f32 {
    rt.elements().iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.rect[1]).unwrap()
}

fn rig() -> (Runtime<S>, std::rc::Rc<std::cell::Cell<usize>>) {
    let told = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut rt = Runtime::new(S { follow: Live::new([0.0, 0.0]), told: told.clone() }, view, (300.0, 300.0), Env::default());
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    (rt, told)
}

#[test]
fn a_live_value_set_from_the_offset_shows_in_the_frame_that_scrolled() {
    let (mut rt, _) = rig();
    let (content0, marker0) = (y(&rt, "content"), y(&rt, "marker"));
    for dy in [-7.0, -13.5, -40.0, 22.0, -3.0] {
        rt.wheel(100.0, 100.0, dy);
        rt.frame(1.0 / 60.0);
        assert_eq!(y(&rt, "marker") - marker0, y(&rt, "content") - content0, "after a wheel of {dy}");
    }
}

#[test]
fn it_follows_every_frame_of_a_glide() {
    let (mut rt, _) = rig();
    let (content0, marker0) = (y(&rt, "content"), y(&rt, "marker"));
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 500.0));
    let mut moved = 0;
    for _ in 0..60 {
        let before = y(&rt, "content");
        rt.frame(1.0 / 60.0);
        moved += usize::from(y(&rt, "content") != before);
        assert_eq!(y(&rt, "marker") - marker0, y(&rt, "content") - content0);
    }
    assert!(moved > 5, "the glide ran over several frames ({moved})");
    assert_eq!(y(&rt, "content") - content0, -500.0);
}

#[test]
fn it_is_told_at_first_then_only_when_the_offset_moves() {
    let (mut rt, told) = rig();
    let first = told.get();
    assert!(first >= 1, "told where it starts");
    for _ in 0..10 { rt.frame(1.0 / 60.0); }
    assert_eq!(told.get(), first, "not again while nothing scrolls or rebuilds");
    rt.wheel(100.0, 100.0, -10.0);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(told.get(), first + 1);
    assert_eq!(TOLD.with(|t| t.get()), ScrollPos { offset: [0.0, 10.0], max: [0.0, 800.0], viewport: [300.0, 200.0], drawn: [300.0, 200.0], direction: Direction::Ltr, source: ScrollSource::Direct });
}

#[test]
fn a_rebuilt_handler_hears_where_the_viewport_stands() {
    let (mut rt, told) = rig();
    rt.wheel(100.0, 100.0, -30.0);
    rt.frame(1.0 / 60.0);
    let before = told.get();
    rt.update(|_, _| {});
    rt.frame(1.0 / 60.0);
    assert_eq!(told.get(), before + 1, "the commit told the new handler, though nothing moved");
    assert_eq!(TOLD.with(|t| t.get()).offset, [0.0, 30.0]);
    rt.frame(1.0 / 60.0);
    assert_eq!(told.get(), before + 1, "and the frame after didn't again");
}

#[test]
fn a_live_rect_set_from_the_offset_lands_in_the_frame_that_mounts_it() {
    struct On { on: bool, bar: Live<[f32; 4]> }
    fn view(s: &On, _: &mut Cx<On>) -> El {
        let mut root = col().size(200.0, 100.0);
        if s.on {
            let bar = s.bar.clone();
            root = root
                .child(col().size(200.0, 100.0).scroll(ScrollSpec { stick_bottom: false })
                    .on_scroll_offset(move |at: ScrollPos| { bar.set([10.0, 5.0, 6.0, at.max[1] / 10.0]); })
                    .child(el().w_full().h(400.0)))
                .child(el().key("bar").absolute(0.0, 0.0).live_rect(&s.bar).fill(Color::WHITE));
        }
        root
    }
    let mut rt = Runtime::new(On { on: false, bar: Live::new([0.0; 4]) }, view, (200.0, 100.0), Env::default());
    rt.set_live_waker(std::sync::Arc::new(|| {}));
    rt.frame(0.0);
    rt.update(|s, tx| { s.on = true; tx.instant(); });
    rt.frame(1.0 / 120.0);
    let bar = info(&rt, "bar");
    assert_eq!(bar.rect, [10.0, 5.0, 6.0, 30.0]);
    assert!(!bar.moving);
}

fn info<T>(rt: &Runtime<T>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
}

#[test]
fn what_a_glide_carries_knows_how_far() {
    let (mut rt, _) = rig();
    let content0 = y(&rt, "content");
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 500.0));
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
        let c = info(&rt, "content");
        assert_eq!(c.rect[1] + c.scrolled[1], content0);
        assert_eq!(info(&rt, "marker").scrolled, [0.0; 2], "outside the viewport");
    }
    assert_eq!(info(&rt, "content").scrolled, [0.0, 500.0]);
}

#[test]
fn what_a_pinned_end_carries_knows_how_far() {
    // Content growing under a stuck bottom carries the rows up without a glide.
    fn view(h: &f32, _: &mut Cx<f32>) -> El {
        col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: true })
            .child(col().key("stack").w_full().growth(GrowthSpec::default())
                .child(el().key("lead").w_full().h(400.0))
                .child(el().key("grow").w_full().h(*h)))
    }
    let mut rt = Runtime::new(37.0, view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(|h, _| *h = 230.0);
    let mut seen = Vec::new();
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
        let lead = info(&rt, "lead");
        assert!((lead.rect[1] + lead.scrolled[1]).abs() < 0.01, "{lead:?}");
        assert!(!lead.scrolling);
        if seen.last() != Some(&lead.scrolled[1]) { seen.push(lead.scrolled[1]); }
    }
    assert!(seen.len() > 5, "the end followed over several frames: {seen:?}");
}

/// origin/dev's watcher, transcribed from `runtime/scroll/watch.rs`: it kept where it last
/// told (`told`) and a weak reference to the handler that heard it (`told_by`), and called
/// when where the viewport stood changed, or the handler was new.
#[derive(Default)]
struct Dev {
    told: Option<ScrollPos>,
    told_by: Option<Weak<dyn Fn(ScrollPos)>>,
}

impl Dev {
    fn seen(&mut self, y: f32, handler: Option<&Rc<dyn Fn(ScrollPos)>>, by_hand: bool) -> Option<(ScrollPos, bool)> {
        let f = handler?;
        let at = pos(y);
        let prev = self.told.replace(at);
        let changed = prev != Some(at);
        let new_handler = !self.told_by.as_ref().is_some_and(|by| std::ptr::addr_eq(by.as_ptr(), Rc::as_ptr(f)));
        if new_handler {
            self.told_by = Some(Rc::downgrade(f));
        }
        if !changed && !new_handler {
            return None;
        }
        let scrolled = by_hand && prev.is_some_and(|old| {
            (old.offset[0] - at.offset[0]).abs() > 0.01 || (old.offset[1] - at.offset[1]).abs() > 0.01
        });
        Some((at, scrolled))
    }
}

fn pos(y: f32) -> ScrollPos {
    ScrollPos { offset: [0.0, y], max: [0.0, 800.0], viewport: [300.0, 200.0], drawn: [300.0, 200.0], direction: Direction::Ltr, source: ScrollSource::None }
}

/// Two handlers built separately, whose allocations stay alive for the walk.
fn handlers() -> Vec<Rc<dyn Fn(ScrollPos)>> {
    (0..2).map(|_| Rc::new(|_: ScrollPos| {}) as Rc<dyn Fn(ScrollPos)>).collect()
}

fn same_handler(a: &Option<Weak<dyn Fn(ScrollPos)>>, b: &Option<Weak<dyn Fn(ScrollPos)>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Weak::ptr_eq(a, b),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug)]
struct Act {
    y: f32,
    /// 0: the viewport has no handler; otherwise one of the walk's handlers.
    handler: usize,
    by_hand: bool,
}

/// Drives every sequence through the watcher and through dev's, one event at a time, and
/// compares what each does after every event.
fn walk(acts: &[Act]) {
    let (mut watch, mut dev) = (Watch::default(), Dev::default());
    let handlers = handlers();
    for (i, act) in acts.iter().enumerate() {
        let handler = (act.handler != 0).then(|| &handlers[act.handler - 1]);
        let effects: Vec<Effect> = step(&mut watch, Event::Seen { at: pos(act.y), handler, by_hand: act.by_hand }).into_iter().collect();
        let expected = dev.seen(act.y, handler, act.by_hand);
        match expected {
            Some((at, scrolled)) => assert_eq!(effects, [Effect::Tell { at, scrolled }], "{acts:?}, event {i}"),
            None => assert!(effects.is_empty(), "{acts:?}, event {i}: told though nothing changed"),
        }
        assert_eq!(watch.told, dev.told, "{acts:?}, event {i}");
        assert!(same_handler(&watch.handler, &dev.told_by), "{acts:?}, event {i}: the handler told");
    }
}

/// Two handlers built separately are two handlers, even at the same offset: the weak
/// reference tells them apart, where an attempt that wrapped would not.
#[test]
fn two_handlers_at_the_same_offset_each_hear_the_first_time() {
    let handlers = handlers();
    let (first, second) = (&handlers[0], &handlers[1]);
    let mut watch = Watch::default();
    let at = pos(0.0);
    let told = |watch: &mut Watch, handler: &Rc<dyn Fn(ScrollPos)>| {
        step(watch, Event::Seen { at, handler: Some(handler), by_hand: false }).into_iter().collect::<Vec<_>>()
    };
    assert_eq!(told(&mut watch, first), [Effect::Tell { at, scrolled: false }]);
    assert_eq!(told(&mut watch, first), [], "the same handler hears it once");
    assert_eq!(told(&mut watch, second), [Effect::Tell { at, scrolled: false }], "a second handler is new, even with the same value");
}

#[test]
fn the_watcher_matches_devs_told_offset_and_handler_over_every_order() {
    let acts: Vec<Act> = [0.0, 12.0].into_iter().flat_map(|y| [0, 1, 2].into_iter().flat_map(move |handler| {
        [false, true].into_iter().map(move |by_hand| Act { y, handler, by_hand })
    })).collect();
    for len in 0..=5 {
        for_each_walk(&acts, len, &mut Vec::new());
    }
}

fn for_each_walk(acts: &[Act], len: usize, prefix: &mut Vec<Act>) {
    if prefix.len() == len {
        walk(prefix);
        return;
    }
    for &act in acts {
        prefix.push(act);
        for_each_walk(acts, len, prefix);
        prefix.pop();
    }
}

#[test]
fn scroll_watch_state_uses_runtime_clock_after_idle_gap() {
    fn view(seen: &Rc<std::cell::Cell<f64>>, _: &mut Cx<Rc<std::cell::Cell<f64>>>) -> El {
        let seen = seen.clone();
        col()
            .key("scroll")
            .size(300.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .on_scroll_offset(move |_: ScrollPos| {
                seen.set(scroll_watch_state().2);
            })
            .child(el().w_full().h(1000.0))
    }
    let seen = Rc::new(std::cell::Cell::new(0.0f64));
    let mut rt = Runtime::new(seen.clone(), view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    // Simulate a 5 s idle wake: `clock` advances by 5 s while animation time `t` caps at RESUME_STEP.
    crate::frame::clock::advance_by(&mut rt, 5.0);
    rt.wheel(100.0, 100.0, -40.0);
    crate::frame::clock::advance_by(&mut rt, 1.0 / 60.0);
    assert!(seen.get() >= 5.0, "scroll_watch_state clock {} must track unscaled runtime clock after 5 s idle", seen.get());
}

#[test]
fn watch_step_ignores_source_changes_when_geometry_is_unchanged() {
    let handlers = handlers();
    let h = &handlers[0];
    let mut watch = Watch::default();
    let direct = ScrollPos { source: ScrollSource::Direct, ..pos(20.0) };
    let settled = ScrollPos { source: ScrollSource::None, ..pos(20.0) };
    assert_eq!(
        step(&mut watch, Event::Seen { at: direct, handler: Some(h), by_hand: true }).into_iter().collect::<Vec<_>>(),
        [Effect::Tell { at: direct, scrolled: false }],
    );
    assert_eq!(
        step(&mut watch, Event::Seen { at: settled, handler: Some(h), by_hand: false }).into_iter().collect::<Vec<_>>(),
        [],
        "settling source to None without a geometry change must not trigger an extra callback",
    );
    assert_eq!(watch.told, Some(direct), "watch.told stays at the last told observation");
}

#[test]
fn touch_overscroll_release_and_mid_rebound_reversal_distinguish_direct_from_edge_return() {
    use std::cell::RefCell;

    let log: Rc<RefCell<Vec<(f32, ScrollSource, bool)>>> = Rc::new(RefCell::new(Vec::new()));
    let view = |log: &Rc<RefCell<Vec<(f32, ScrollSource, bool)>>>, _: &mut Cx<Rc<RefCell<Vec<(f32, ScrollSource, bool)>>>>| {
        let log = log.clone();
        col()
            .key("list")
            .w_full()
            .h_full()
            .scroll(ScrollSpec { stick_bottom: false })
            .on_scroll_offset(move |pos: ScrollPos| {
                let (by_hand, _, _) = scroll_watch_state();
                log.borrow_mut().push((pos.offset[1], pos.source, by_hand));
            })
            .child(el().w_full().h(1600.0))
    };

    let mut rt = Runtime::new(log.clone(), view, (300.0, 400.0), Env::new(false, 1.0));
    rt.frame(0.0);
    assert_eq!(log.borrow().as_slice(), &[(0.0, ScrollSource::None, false)], "initial layout reports None");
    log.borrow_mut().clear();

    // Drag downward past the top edge (negative overscroll): every frame must be Direct.
    rt.touch(1, TouchPhase::Started, 120.0, 100.0, 0.0);
    for i in 1..=5 {
        rt.touch(1, TouchPhase::Moved, 120.0, 100.0 + 30.0 * i as f32, f64::from(i) * 0.016);
        rt.frame(0.016);
    }
    let drag_events = log.borrow().clone();
    assert!(!drag_events.is_empty(), "drag must report scroll observations");
    assert!(drag_events.iter().all(|&(y, src, _)| y < 0.0 && src == ScrollSource::Direct), "drag frames must be Direct: {drag_events:?}");
    let peak_overscroll = drag_events.last().unwrap().0;
    log.borrow_mut().clear();

    // Release finger: every elastic rebound frame until rest at 0.0 must be EdgeReturn.
    rt.touch(1, TouchPhase::Ended, 120.0, 250.0, 0.081);
    for _ in 0..100 {
        rt.frame(0.016);
    }
    let rebound_events = log.borrow().clone();
    assert!(rebound_events.len() > 2, "rebound should span multiple frames: {rebound_events:?}");
    assert!(
        rebound_events.iter().all(|&(_, src, _)| src == ScrollSource::EdgeReturn),
        "every rebound frame must be EdgeReturn: {rebound_events:?}"
    );
    assert!(rebound_events.first().unwrap().0 > peak_overscroll || rebound_events.last().unwrap().0 == 0.0);
    assert_eq!(rebound_events.last().unwrap().0, 0.0, "rebound lands at 0.0");

    // Idle frames after rebound settles must emit zero callbacks and request no redraw.
    log.borrow_mut().clear();
    for _ in 0..10 {
        assert!(!rt.frame(0.016), "idle window must not stay animating after edge return");
    }
    assert!(log.borrow().is_empty(), "idle frames must not fire on_scroll_offset: {:?}", log.borrow());

    // Pull past top again, stop finger (0 release velocity), release, then catch mid-rebound
    // and drag inward while still at negative offset -> must report Direct, not EdgeReturn.
    rt.touch(2, TouchPhase::Started, 120.0, 100.0, 2.0);
    for i in 1..=4 {
        rt.touch(2, TouchPhase::Moved, 120.0, 100.0 + 35.0 * i as f32, 2.0 + f64::from(i) * 0.016);
        rt.frame(0.016);
    }
    // Hold still for > FLING_HOLD (0.1 s) so release velocity is 0.0.
    rt.touch(2, TouchPhase::Ended, 120.0, 240.0, 2.3);
    log.borrow_mut().clear();
    rt.frame(0.016);
    assert_eq!(
        log.borrow().last().map(|&(_, src, _)| src),
        Some(ScrollSource::EdgeReturn),
        "first rebound frame with zero release velocity must still report EdgeReturn: {:?}",
        log.borrow()
    );
    let mid_y = log.borrow().last().unwrap().0;
    assert!(mid_y < -10.0, "still in overscroll mid-rebound: {mid_y}");

    // New touch catches mid-rebound and drags inward (upward) while offset is still negative.
    log.borrow_mut().clear();
    rt.touch(3, TouchPhase::Started, 120.0, 200.0, 2.32);
    rt.touch(3, TouchPhase::Moved, 120.0, 180.0, 2.336);
    rt.frame(0.016);
    let caught = log.borrow().clone();
    assert!(!caught.is_empty(), "reversal drag must report offset");
    assert!(
        caught.iter().all(|&(y, src, _)| y < 0.0 && y > mid_y && src == ScrollSource::Direct),
        "deliberate inward drag while still negative must be Direct: {caught:?}"
    );
}

#[test]
fn touch_fling_wheel_keys_and_programmatic_scrolls_report_expected_sources() {
    use std::cell::RefCell;

    let log: Rc<RefCell<Vec<(f32, ScrollSource)>>> = Rc::new(RefCell::new(Vec::new()));
    let view = |log: &Rc<RefCell<Vec<(f32, ScrollSource)>>>, _: &mut Cx<Rc<RefCell<Vec<(f32, ScrollSource)>>>>| {
        let log = log.clone();
        col()
            .key("list")
            .w_full()
            .h_full()
            .scroll(ScrollSpec { stick_bottom: false })
            .on_scroll_offset(move |pos: ScrollPos| {
                log.borrow_mut().push((pos.offset[1], pos.source));
            })
            .child(el().w_full().h(2000.0))
    };

    let mut rt = Runtime::new(log.clone(), view, (300.0, 400.0), Env::new(false, 1.0));
    rt.frame(0.0);
    log.borrow_mut().clear();

    // In-bounds touch drag followed by fling -> Direct then Momentum.
    rt.touch(1, TouchPhase::Started, 120.0, 300.0, 0.0);
    for i in 1..=4 {
        rt.touch(1, TouchPhase::Moved, 120.0, 300.0 - 30.0 * i as f32, f64::from(i) * 0.016);
        rt.frame(0.016);
    }
    assert!(log.borrow().iter().all(|&(_, s)| s == ScrollSource::Direct), "in-bounds drag is Direct: {:?}", log.borrow());
    log.borrow_mut().clear();
    rt.touch(1, TouchPhase::Ended, 120.0, 180.0, 0.080);
    for _ in 0..10 {
        rt.frame(0.016);
    }
    assert!(
        !log.borrow().is_empty() && log.borrow().iter().all(|&(_, s)| s == ScrollSource::Momentum),
        "in-bounds touch fling must report Momentum: {:?}",
        log.borrow()
    );

    // Runtime::fling -> Momentum.
    log.borrow_mut().clear();
    rt.fling(120.0, 200.0, -600.0);
    for _ in 0..5 {
        rt.frame(0.016);
    }
    assert!(
        !log.borrow().is_empty() && log.borrow().iter().all(|&(_, s)| s == ScrollSource::Momentum),
        "Runtime::fling must report Momentum: {:?}",
        log.borrow()
    );

    // Direct wheel -> Direct.
    log.borrow_mut().clear();
    rt.wheel(120.0, 200.0, -25.0);
    rt.frame(0.016);
    assert_eq!(log.borrow().last().map(|&(_, s)| s), Some(ScrollSource::Direct), "wheel must report Direct");

    // Phased trackpad momentum wheel -> Momentum.
    log.borrow_mut().clear();
    rt.wheel_event(WheelEvent::new(0.0, -20.0).at(120.0, 200.0).phase(GesturePhase::Began));
    rt.frame(0.016);
    assert_eq!(log.borrow().last().map(|&(_, s)| s), Some(ScrollSource::Direct), "phased touch wheel is Direct");
    rt.wheel_event(WheelEvent::new(0.0, 0.0).at(120.0, 200.0).phase(GesturePhase::Ended));
    log.borrow_mut().clear();
    rt.wheel_event(WheelEvent::new(0.0, -15.0).at(120.0, 200.0).phase(GesturePhase::Began));
    rt.frame(0.016);
    assert_eq!(log.borrow().last().map(|&(_, s)| s), Some(ScrollSource::Momentum), "phased trackpad momentum is Momentum");
    rt.wheel_event(WheelEvent::new(0.0, 0.0).at(120.0, 200.0).phase(GesturePhase::Ended));

    // Keyboard PageDown -> Direct.
    log.borrow_mut().clear();
    rt.key(KeyEvent { key: Key::PageDown, mods: Mods::default() });
    rt.frame(0.016);
    assert_eq!(log.borrow().last().map(|&(_, s)| s), Some(ScrollSource::Direct), "PageDown key must report Direct");

    // Tx::scroll_to_bottom (glide) -> Programmatic across every glide frame.
    log.borrow_mut().clear();
    rt.update(|_, tx| tx.scroll_to_bottom("list"));
    for _ in 0..80 {
        rt.frame(0.016);
    }
    assert!(
        !log.borrow().is_empty() && log.borrow().iter().all(|&(_, s)| s == ScrollSource::Programmatic),
        "Tx::scroll_to_bottom glide must report Programmatic: {:?}",
        log.borrow()
    );

    // Tx::scroll_to_instant -> Programmatic.
    log.borrow_mut().clear();
    rt.update(|_, tx| tx.scroll_to_instant("list", 0.0, 100.0));
    rt.frame(0.016);
    assert_eq!(log.borrow().as_slice(), &[(100.0, ScrollSource::Programmatic)]);

    // Tx::scroll_page -> Programmatic.
    log.borrow_mut().clear();
    rt.update(|_, tx| tx.scroll_page("list", 1));
    rt.frame(0.016);
    assert_eq!(log.borrow().last().map(|&(_, s)| s), Some(ScrollSource::Programmatic));

    // Rebuilt view while idle -> new handler receives ScrollSource::None.
    log.borrow_mut().clear();
    rt.update(|_, _| {});
    rt.frame(0.016);
    assert_eq!(log.borrow().last().map(|&(_, s)| s), Some(ScrollSource::None));
}

#[test]
fn horizontal_touch_rebound_on_scroll_xy_reports_edge_return_and_stick_bottom_reports_programmatic() {
    use std::cell::RefCell;

    let log: Rc<RefCell<Vec<([f32; 2], ScrollSource)>>> = Rc::new(RefCell::new(Vec::new()));
    let view = |log: &Rc<RefCell<Vec<([f32; 2], ScrollSource)>>>, _: &mut Cx<Rc<RefCell<Vec<([f32; 2], ScrollSource)>>>>| {
        let log = log.clone();
        col()
            .key("xy")
            .w_full()
            .h_full()
            .scroll_xy()
            .on_scroll_offset(move |pos: ScrollPos| {
                log.borrow_mut().push((pos.offset, pos.source));
            })
            .child(el().w(1200.0).h(1200.0))
    };

    let mut rt = Runtime::new(log.clone(), view, (300.0, 300.0), Env::new(false, 1.0));
    rt.frame(0.0);
    log.borrow_mut().clear();

    // Horizontal drag past left edge on a two-axis viewport.
    rt.touch(1, TouchPhase::Started, 100.0, 150.0, 0.0);
    for i in 1..=4 {
        rt.touch(1, TouchPhase::Moved, 100.0 + 30.0 * i as f32, 150.0, f64::from(i) * 0.016);
        rt.frame(0.016);
    }
    assert!(log.borrow().iter().all(|&(o, s)| o[0] < 0.0 && s == ScrollSource::Direct), "horizontal overscroll drag is Direct: {:?}", log.borrow());
    log.borrow_mut().clear();

    // Release: horizontal rebound must stay EdgeReturn (never clobbered by vertical ModeEvent::Step).
    rt.touch(1, TouchPhase::Ended, 220.0, 150.0, 0.065);
    for _ in 0..80 {
        rt.frame(0.016);
    }
    assert!(
        !log.borrow().is_empty() && log.borrow().iter().all(|&(_, s)| s == ScrollSource::EdgeReturn),
        "horizontal edge return on scroll_xy must report EdgeReturn: {:?}",
        log.borrow()
    );

    // stick_bottom following growing content -> Programmatic.
    struct StickyState {
        h: f32,
        log: Rc<RefCell<Vec<(f32, ScrollSource)>>>,
    }
    fn sticky_view(s: &StickyState, _: &mut Cx<StickyState>) -> El {
        let log = s.log.clone();
        col()
            .key("chat")
            .w_full()
            .h_full()
            .scroll(ScrollSpec::stick_bottom())
            .on_scroll_offset(move |pos: ScrollPos| log.borrow_mut().push((pos.offset[1], pos.source)))
            .child(el().w_full().h(s.h))
    }
    let slog = Rc::new(RefCell::new(Vec::new()));
    let mut srt = Runtime::new(StickyState { h: 800.0, log: slog.clone() }, sticky_view, (300.0, 300.0), Env::new(false, 1.0));
    srt.frame(0.0);
    slog.borrow_mut().clear();
    srt.update(|s, _| s.h = 1000.0);
    srt.frame(0.016);
    assert_eq!(slog.borrow().last(), Some(&(700.0, ScrollSource::Programmatic)), "stick_bottom growth follow must report Programmatic");
}
