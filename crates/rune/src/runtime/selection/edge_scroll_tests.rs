use super::*;
use crate::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, TextStyle, col, text};

const ROWS: usize = 40;
const ROW_H: f32 = 40.0;
const LIST: &str = "root/scroll/list";
/// The viewport's bottom edge, in window points.
const BOTTOM: f32 = 120.0;

fn view(_: &(), _: &mut Cx<()>) -> El {
    col().key("root").w(300.0).h(BOTTOM).child(
        col().key("scroll").w_full().h(BOTTOM).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("list").w_full().growth(GrowthSpec::default()).selection_group(true).virtual_rows(
                ROWS,
                ROW_H,
                |_: &(), i| format!("r{i}").into(),
                |_: &(), i| text(format!("row {i:02}")).font(TextStyle::new(14.0, 400.0)).w_full().h(ROW_H).selectable(true),
            ),
        ),
    )
}

fn window() -> Runtime<()> {
    let mut rt = Runtime::new((), view, (300.0, BOTTOM), Env::new(true, 1.0));
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<()>) {
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) { break; }
    }
}

fn offset(rt: &Runtime<()>) -> f64 { rt.scrolls.values().next().expect("one scroll").offset }

fn max(rt: &Runtime<()>) -> f64 { rt.scrolls.values().next().expect("one scroll").max }

fn row_of(path: &str) -> usize { path.rsplit_once("/r").and_then(|(_, r)| r.parse().ok()).expect("a row path") }

/// The rows holding the selection's anchor and focus.
fn ends(rt: &Runtime<()>) -> (usize, usize) {
    let sel = rt.selectable.phase.selection().expect("a selection");
    (row_of(&sel.path), sel.far.as_ref().map_or(row_of(&sel.path), |f| row_of(&f.path)))
}

/// Presses the start of row 0 and drags `past` points below the list, where it holds still.
fn hold_below(past: f32) -> Runtime<()> {
    let mut rt = window();
    rt.pointer_down(2.0, 20.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(150.0, BOTTOM + past);
    rt
}

#[test]
fn held_still_past_the_bottom_the_list_and_the_selection_keep_going() {
    let mut rt = hold_below(200.0);
    let (mut last_offset, mut last_focus) = (offset(&rt), 0);
    let mut advanced = 0;
    for tick in 0..30 {
        rt.frame(1.0 / 60.0);
        let (anchor, focus) = ends(&rt);
        assert_eq!(anchor, 0, "tick {tick}: the anchor stays");
        assert!(offset(&rt) > last_offset, "tick {tick}: the list scrolls under a still pointer");
        assert!(focus >= last_focus, "tick {tick}: the focus only moves down");
        advanced += usize::from(focus > last_focus);
        (last_offset, last_focus) = (offset(&rt), focus);
    }
    assert!(advanced >= 10 && last_focus >= 15, "the selection followed the list: focus row {last_focus}, moved {advanced} times");
    assert!(!rt.virtuals[LIST].range.contains(&0), "the anchor's row scrolled out: {:?}", rt.virtuals[LIST].range);
    rt.pointer_up(150.0, BOTTOM + 200.0);
    rt.frame(1.0 / 60.0);
    let (_, focus) = ends(&rt);
    let copied = rt.selected_text().expect("a selection");
    assert!(focus >= last_focus && copied.starts_with("row 00\nrow 01\n") && copied.ends_with(&format!("row {focus:02}")), "{copied:?}");
}

#[test]
fn the_step_grows_with_the_distance_past_the_edge() {
    let step = |past: f32| {
        let mut rt = hold_below(past);
        rt.frame(1.0 / 60.0);
        let before = offset(&rt);
        rt.frame(1.0 / 60.0);
        offset(&rt) - before
    };
    let (at, near, far) = (step(0.0), step(20.0), step(80.0));
    assert!(at > 0.0, "at the edge it still scrolls");
    assert!(near > at && far > near, "{at} {near} {far}");
    assert!((far - 810.0 / 60.0).abs() < 0.5, "81 points past the band at the edge: 810 points a second, {far} a frame");
}

#[test]
fn held_still_above_the_top_it_scrolls_back_up() {
    let mut rt = window();
    rt.wheel(150.0, 60.0, -10_000.0);
    settle(&mut rt);
    let end = offset(&rt);
    assert!(end > 0.0);
    rt.pointer_down(296.0, 100.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(150.0, -200.0);
    let mut last = end;
    for tick in 0..10 {
        rt.frame(1.0 / 60.0);
        assert!(offset(&rt) < last, "tick {tick}");
        last = offset(&rt);
    }
    let (anchor, focus) = ends(&rt);
    assert!(anchor == ROWS - 1 && focus < ROWS - 5, "the focus followed the list up: {anchor} {focus}");
}

#[test]
fn release_stops_it_and_leaves_no_wake() {
    let mut rt = hold_below(100.0);
    for _ in 0..3 { rt.frame(1.0 / 60.0); }
    assert!(rt.selectable.edge.running());
    rt.pointer_up(150.0, BOTTOM + 100.0);
    assert!(!rt.selectable.edge.running(), "the release stops it at once");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.wake_deadline(), None, "nothing left to wake for");
    let at = offset(&rt);
    settle(&mut rt);
    assert_eq!(offset(&rt), at, "the list stays where the release left it");
    assert!(!rt.frame(1.0 / 60.0));
}

#[test]
fn moving_back_inside_or_losing_focus_stops_it() {
    let mut rt = hold_below(100.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(150.0, 60.0);
    assert!(!rt.selectable.edge.running(), "back inside");
    let at = offset(&rt);
    settle(&mut rt);
    assert_eq!(offset(&rt), at);
    assert!(rt.selectable.phase.dragging(), "still selecting, only not scrolling");

    let mut rt = hold_below(100.0);
    rt.frame(1.0 / 60.0);
    rt.window_focus(true);
    rt.window_focus(false);
    assert!(!rt.selectable.edge.running(), "focus lost");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.wake_deadline(), None);
}

#[test]
fn at_the_end_of_the_list_it_stops_asking_for_frames() {
    let mut rt = hold_below(400.0);
    let mut ticks = 0;
    while rt.selectable.edge.running() {
        rt.frame(1.0 / 60.0);
        ticks += 1;
        assert!(ticks < 200, "reached the end");
    }
    assert_eq!(offset(&rt), max(&rt), "scrolled to the end");
    assert_eq!(ends(&rt), (0, ROWS - 1), "the selection reaches the last row");
    settle(&mut rt);
    assert_eq!(rt.wake_deadline(), None, "no wake while the pointer stays held at the end");
    assert!(!rt.frame(1.0 / 60.0), "an idle frame");
    assert!(rt.press.is_held() && rt.selectable.phase.dragging(), "still held");
}

#[test]
fn an_idle_window_with_no_drag_draws_nothing() {
    let mut rt = window();
    // A placed selection, and a pointer past the edge with no press.
    rt.pointer_down(2.0, 20.0);
    rt.pointer_move(150.0, 60.0);
    rt.pointer_up(150.0, 60.0);
    settle(&mut rt);
    rt.pointer_move(150.0, BOTTOM + 100.0);
    settle(&mut rt);
    let commits = rt.stats.commits;
    for _ in 0..10 { assert!(!rt.frame(1.0 / 60.0)); }
    assert_eq!(rt.wake_deadline(), None);
    assert_eq!(rt.stats.commits, commits, "nothing rebuilt");
    assert!(!rt.selectable.edge.running());
    assert_eq!(offset(&rt), 0.0);
}

#[test]
fn a_stale_step_does_nothing() {
    let (mut seq, mut state) = (Seq::default(), State::Idle);
    let edge = Some(Edge { scroll: NodeId(7), speed: 600.0, room: 1000.0 });
    assert_eq!(step(&mut seq, &mut state, Event::Moved { now: 1.0, edge }), Effects::one(Effect::WakeAt(1.0)));
    let State::Running { attempt: first, .. } = state else { panic!("{state:?}") };
    let _ = step(&mut seq, &mut state, Event::Stop);
    assert_eq!(step(&mut seq, &mut state, Event::Due { attempt: first, now: 1.1, edge }), Effects::none(), "stopped");
    assert_eq!(state, State::Idle);
    let _ = step(&mut seq, &mut state, Event::Moved { now: 1.2, edge });
    let held = state;
    assert_eq!(step(&mut seq, &mut state, Event::Due { attempt: first, now: 1.3, edge }), Effects::none(), "an earlier drag's step");
    assert_eq!(state, held);
    let State::Running { attempt, .. } = state else { panic!() };
    assert_ne!(attempt, first);
    assert_eq!(step(&mut seq, &mut state, Event::Due { attempt, now: 1.15, edge }), Effects::none(), "a frame before it's due");
    assert_eq!(state, held);
    let effects = step(&mut seq, &mut state, Event::Due { attempt, now: 1.25, edge });
    assert_eq!(effects, Effects::two(Effect::Scroll { scroll: NodeId(7), by: 30.0 }, Effect::WakeAt(1.25)), "600 points a second for 50 ms");
}

#[test]
fn a_step_stops_at_the_room_left_and_a_late_frame_doesnt_jump() {
    let (mut seq, mut state) = (Seq::default(), State::Idle);
    let edge = |room| Some(Edge { scroll: NodeId(7), speed: -600.0, room });
    let _ = step(&mut seq, &mut state, Event::Moved { now: 0.0, edge: edge(100.0) });
    let State::Running { attempt, .. } = state else { panic!() };
    let effects = step(&mut seq, &mut state, Event::Due { attempt, now: 1.0, edge: edge(100.0) });
    assert_eq!(effects, Effects::two(Effect::Scroll { scroll: NodeId(7), by: -30.0 }, Effect::WakeAt(1.0)), "a second late steps 50 ms");
    let State::Running { attempt, .. } = state else { panic!() };
    let effects = step(&mut seq, &mut state, Event::Due { attempt, now: 1.05, edge: edge(10.0) });
    assert_eq!(effects, Effects::one(Effect::Scroll { scroll: NodeId(7), by: -10.0 }), "the last step");
    assert_eq!(state, State::Idle);
    let _ = step(&mut seq, &mut state, Event::Moved { now: 2.0, edge: edge(0.0) });
    assert_eq!(state, State::Idle, "nothing left to scroll");
}
