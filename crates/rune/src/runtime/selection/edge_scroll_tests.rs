use super::*;
use crate::{Cx, El, Env, GrowthSpec, Key, KeyEvent, Mods, PhysicalInput, Runtime, ScrollSpec, TextStyle, col, text};

const ROWS: usize = 40;
const ROW_H: f32 = 40.0;
const LIST: &str = "root/scroll/list";
/// The viewport's bottom edge, in window points.
const BOTTOM: f32 = 120.0;
/// What each row reads.
const LEN: usize = 6;

/// The list each test builds: virtual rows or plain ones, how many, and whether it sticks to its
/// bottom.
#[derive(Clone, Copy)]
struct List {
    rows: usize,
    virtual_rows: bool,
    stick: bool,
}

const VIRTUAL: List = List { rows: ROWS, virtual_rows: true, stick: false };
const PLAIN: List = List { rows: ROWS, virtual_rows: false, stick: false };

fn row(i: usize) -> El { text(format!("row {i:02}")).font(TextStyle::new(14.0, 400.0)).w_full().h(ROW_H).selectable(true) }

fn view(s: &List, _: &mut Cx<List>) -> El {
    let list = col().key("list").w_full().growth(GrowthSpec::default()).selection_group(true);
    let list = if s.virtual_rows {
        list.virtual_rows(s.rows, ROW_H, |_: &List, i| format!("r{i}").into(), |_: &List, i| row(i))
    } else {
        list.children((0..s.rows).map(|i| row(i).key(format!("r{i}"))))
    };
    col().key("root").w(300.0).h(BOTTOM).child(col().key("scroll").w_full().h(BOTTOM).scroll(ScrollSpec { stick_bottom: s.stick }).child(list))
}

fn window(list: List) -> Runtime<List> {
    let mut rt = Runtime::new(list, view, (300.0, BOTTOM), Env::new(true, 1.0));
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<List>) {
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) { break; }
    }
}

fn offset(rt: &Runtime<List>) -> f64 { rt.scrolls.values().next().expect("one scroll").offset }

fn max(rt: &Runtime<List>) -> f64 { rt.scrolls.values().next().expect("one scroll").max }

fn row_of(path: &str) -> usize { path.rsplit_once("/r").and_then(|(_, r)| r.parse().ok()).expect("a row path") }

/// The rows holding the selection's anchor and focus.
fn ends(rt: &Runtime<List>) -> (usize, usize) {
    let sel = rt.selectable.phase.selection().expect("a selection");
    (row_of(&sel.path), sel.far.as_ref().map_or(row_of(&sel.path), |f| row_of(&f.path)))
}

/// The selection's anchor and focus, as (row, byte).
fn range(rt: &Runtime<List>) -> ((usize, usize), (usize, usize)) {
    let sel = rt.selectable.phase.selection().expect("a selection");
    let (a, f) = ends(rt);
    ((a, sel.anchor), (f, sel.focus))
}

/// Presses the start of row 0 and drags `past` points below the list, where it holds still.
fn hold_below_in(list: List, past: f32) -> Runtime<List> {
    let mut rt = window(list);
    rt.pointer_down(2.0, 20.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(150.0, BOTTOM + past);
    rt
}

fn hold_below(past: f32) -> Runtime<List> { hold_below_in(VIRTUAL, past) }

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
    let mut rt = window(VIRTUAL);
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

/// Escape lets go of a selection drag in plain rows, which no drag handler holds, and in virtual
/// rows whose anchor has scrolled out: the list and the selection stay where they are.
#[test]
fn escape_stops_it_and_keeps_the_selection() {
    let escapes: [fn(&mut Runtime<List>); 2] = [
        |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }),
        |rt| rt.physical_input(PhysicalInput::Key { key: crate::PhysicalKeyCode::Escape, mods: Mods::default(), pressed: true, repeat: false }),
    ];
    for (list, ticks) in [(PLAIN, 3), (VIRTUAL, 3), (VIRTUAL, 30)] {
        for escape in escapes {
            let mut rt = hold_below_in(list, 100.0);
            for _ in 0..ticks { rt.frame(1.0 / 60.0); }
            if ticks == 30 { assert!(!rt.virtuals[LIST].range.contains(&0), "the anchor's row is out"); }
            assert!(rt.selectable.edge.running());
            escape(&mut rt);
            assert!(!rt.selectable.edge.running(), "Escape stops it at once");
            let (at, selected) = (offset(&rt), range(&rt));
            rt.frame(1.0 / 120.0);
            assert_eq!(rt.wake_deadline(), None, "no step asked for");
            for _ in 0..10 { rt.frame(1.0 / 120.0); }
            rt.pointer_move(150.0, BOTTOM + 200.0);
            for _ in 0..10 { rt.frame(1.0 / 120.0); }
            assert_eq!((offset(&rt), range(&rt)), (at, selected), "the list and the selection stay put");
            assert!(!rt.press.is_held() && !rt.selectable.phase.dragging());
        }
    }
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

/// The step that reaches the end brings the last row in; the one after it, over that layout,
/// selects it, before any release. 371 points past at 60 Hz, the last step goes from 1426 to
/// 1480, which shows row 39 only after it.
#[test]
fn the_last_row_a_scroll_brings_in_is_selected() {
    for list in [VIRTUAL, PLAIN] {
        for hz in [40.0, 60.0, 120.0] {
            for past in [371.0, 100.0, 3000.0] {
                let mut rt = hold_below_in(list, past);
                let mut ticks = 0;
                while rt.selectable.edge.running() {
                    rt.frame(1.0 / hz);
                    ticks += 1;
                    assert!(ticks < 2000, "reached the end");
                }
                assert_eq!(offset(&rt), max(&rt));
                assert_eq!(range(&rt), ((0, 0), (ROWS - 1, LEN)), "virtual {} at {hz} Hz, {past} past", list.virtual_rows);
            }
        }
    }
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

/// A list stuck to its bottom lets go of it while the edge scroll runs, however small each step
/// is: at 240 Hz a step at the edge is a third of a point, inside the half point that re-pins it.
/// It travels to the top, then stops waking; no step at 120 Hz is longer than its speed asks.
#[test]
fn a_list_stuck_to_its_bottom_scrolls_up_and_then_rests() {
    for hz in [240.0, 120.0] {
        let mut rt = window(List { rows: 10, virtual_rows: true, stick: true });
        rt.wheel(150.0, 60.0, -10_000.0);
        settle(&mut rt);
        let end = max(&rt);
        assert!(end > 0.0 && offset(&rt) == end && rt.scrolls.values().next().unwrap().pinned(), "pinned at its end");
        rt.pointer_down(296.0, 100.0);
        rt.frame(1.0 / hz);
        // Inside the band at the top edge: 80 points a second.
        rt.pointer_move(150.0, 0.5);
        let mut last = offset(&rt);
        for tick in 0..120 {
            rt.frame(1.0 / hz);
            let moved = last - offset(&rt);
            assert!(moved > 0.0 && moved <= 80.0 / hz + 1e-6, "{hz} Hz tick {tick}: moved {moved}");
            last = offset(&rt);
        }
        let mut ticks = 0;
        while rt.selectable.edge.running() {
            rt.frame(1.0 / hz);
            ticks += 1;
            assert!(ticks < 2000, "{hz} Hz: reached the top");
        }
        assert_eq!(offset(&rt), 0.0, "{hz} Hz");
        rt.frame(1.0 / hz);
        assert_eq!(rt.wake_deadline(), None, "{hz} Hz: no wake once at the top");
    }
}

#[test]
fn an_idle_window_with_no_drag_draws_nothing() {
    let mut rt = window(VIRTUAL);
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

fn edge(speed: f32, room: f64, offset: f64) -> Option<Edge> { Some(Edge { scroll: NodeId(7), speed, room, offset }) }

#[test]
fn a_stale_step_does_nothing() {
    let (mut seq, mut state) = (Seq::default(), State::Idle);
    let e = edge(600.0, 1000.0, 0.0);
    assert_eq!(step(&mut seq, &mut state, Event::Moved { now: 1.0, edge: e }), Effects::one(Effect::WakeAt(1.0)));
    let State::Running { attempt: first, .. } = state else { panic!("{state:?}") };
    let _ = step(&mut seq, &mut state, Event::Stop);
    assert_eq!(step(&mut seq, &mut state, Event::Due { attempt: first, now: 1.1, edge: e }), Effects::none(), "stopped");
    assert_eq!(state, State::Idle);
    let _ = step(&mut seq, &mut state, Event::Moved { now: 1.2, edge: e });
    let held = state;
    assert_eq!(step(&mut seq, &mut state, Event::Due { attempt: first, now: 1.3, edge: e }), Effects::none(), "an earlier drag's step");
    assert_eq!(state, held);
    let State::Running { attempt, .. } = state else { panic!() };
    assert_ne!(attempt, first);
    assert_eq!(step(&mut seq, &mut state, Event::Due { attempt, now: 1.15, edge: e }), Effects::none(), "a frame before it's due");
    assert_eq!(state, held);
    let effects = step(&mut seq, &mut state, Event::Due { attempt, now: 1.25, edge: e });
    assert_eq!(effects, Effects::two(Effect::Step { scroll: NodeId(7), by: 30.0 }, Effect::WakeAt(1.25)), "600 points a second for 50 ms");
}

#[test]
fn a_step_stops_at_the_room_left_and_a_late_frame_doesnt_jump() {
    let (mut seq, mut state) = (Seq::default(), State::Idle);
    let up = |room, offset| edge(-600.0, room, offset);
    let _ = step(&mut seq, &mut state, Event::Moved { now: 0.0, edge: up(100.0, 100.0) });
    let State::Running { attempt, .. } = state else { panic!() };
    let effects = step(&mut seq, &mut state, Event::Due { attempt, now: 1.0, edge: up(100.0, 100.0) });
    assert_eq!(effects, Effects::two(Effect::Step { scroll: NodeId(7), by: -30.0 }, Effect::WakeAt(1.0)), "a second late steps 50 ms");
    let State::Running { attempt, .. } = state else { panic!() };
    let effects = step(&mut seq, &mut state, Event::Due { attempt, now: 1.05, edge: up(10.0, 10.0) });
    assert_eq!(effects, Effects::two(Effect::Step { scroll: NodeId(7), by: -10.0 }, Effect::WakeAt(1.05)), "the last step");
    let State::Landing { attempt, .. } = state else { panic!("{state:?}") };
    assert_eq!(step(&mut seq, &mut state, Event::Due { attempt, now: 1.06, edge: up(0.0, 0.0) }), Effects::one(Effect::Extend), "the selection follows");
    assert_eq!(state, State::Idle);
    let _ = step(&mut seq, &mut state, Event::Moved { now: 2.0, edge: up(0.0, 0.0) });
    assert_eq!(state, State::Idle, "nothing left to scroll");
}

/// A step that something undid (here a list pulling itself back to its bottom) ends it, with no
/// wake asked for; a step that moved nothing because no time passed doesn't.
#[test]
fn a_step_that_moved_nothing_ends_it() {
    let (mut seq, mut state) = (Seq::default(), State::Idle);
    let up = |offset| edge(-60.0, offset, offset);
    let _ = step(&mut seq, &mut state, Event::Moved { now: 0.0, edge: up(500.0) });
    let State::Running { attempt, .. } = state else { panic!() };
    assert_eq!(step(&mut seq, &mut state, Event::Due { attempt, now: 0.0, edge: up(500.0) }).into_iter().count(), 2, "no time: no move, and on");
    let State::Running { attempt, .. } = state else { panic!() };
    let _ = step(&mut seq, &mut state, Event::Due { attempt, now: 0.01, edge: up(500.0) });
    let State::Running { attempt, .. } = state else { panic!() };
    assert_eq!(step(&mut seq, &mut state, Event::Due { attempt, now: 0.02, edge: up(500.0) }), Effects::none(), "pulled back");
    assert_eq!(state, State::Idle);
}
