use super::*;
use crate::{Cx, El, Env, GrowthSpec, Key, KeyEvent, Mods, PhysicalInput, Runtime, ScrollSpec, TextStyle, col, text};

const ROWS: usize = 40;
const ROW_H: f32 = 40.0;
const LIST: &str = "root/scroll/list";
/// The viewport's bottom edge, in window points.
const BOTTOM: f32 = 120.0;
/// What each row reads.
const LEN: usize = 6;

/// The list each test builds: virtual rows or plain ones, how many, whether it sticks to its
/// bottom, and whether row 10 is half height.
#[derive(Clone, Copy)]
struct List {
    rows: usize,
    virtual_rows: bool,
    stick: bool,
    short: bool,
}

const VIRTUAL: List = List { rows: ROWS, virtual_rows: true, stick: false, short: false };
const PLAIN: List = List { rows: ROWS, virtual_rows: false, stick: false, short: false };

fn row(s: &List, i: usize) -> El {
    let h = if s.short && i == 10 { ROW_H / 2.0 } else { ROW_H };
    text(format!("row {i:02}")).font(TextStyle::new(14.0, 400.0)).w_full().h(h).selectable(true)
}

fn view(s: &List, _: &mut Cx<List>) -> El {
    let list = col().key("list").w_full().growth(GrowthSpec::default()).selection_group(true);
    let list = if s.virtual_rows {
        list.virtual_rows(s.rows, ROW_H, |_: &List, i| format!("r{i}").into(), |s: &List, i| row(s, i))
    } else {
        list.children((0..s.rows).map(|i| row(s, i).key(format!("r{i}"))))
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

/// The frame whose step brings row 39 in selects it: blurring right after that frame, with no
/// step left to take, leaves the selection at what shows.
#[test]
fn losing_focus_right_after_the_last_row_scrolls_in_keeps_it_selected() {
    for (list, past) in [(VIRTUAL, 371.0), (PLAIN, 371.0), (VIRTUAL, 100.0)] {
        let mut rt = hold_below_in(list, past);
        let mut ticks = 0;
        while offset(&rt) < max(&rt) {
            rt.frame(1.0 / 60.0);
            ticks += 1;
            assert!(ticks < 2000, "reached the end");
        }
        rt.window_focus(true);
        rt.window_focus(false);
        assert!(!rt.selectable.edge.running() && !rt.selectable.phase.dragging());
        assert_eq!(range(&rt), ((0, 0), (ROWS - 1, LEN)), "virtual {}, {past} past", list.virtual_rows);
    }
    // And midway: each frame's selection reaches the last row that frame shows.
    let mut rt = hold_below(200.0);
    for tick in 0..20 {
        rt.frame(1.0 / 60.0);
        let shown = ((offset(&rt) + f64::from(BOTTOM)) / f64::from(ROW_H)).ceil() as usize - 1;
        assert_eq!(range(&rt).1, (shown, LEN), "tick {tick} at {}", offset(&rt));
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
        let mut rt = window(List { rows: 10, virtual_rows: true, stick: true, short: false });
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

fn edge(speed: f32, room: f64, offset: f64) -> Option<Edge> { Some(Edge { scroll: NodeId(7), speed, room, offset, max: 1000.0 }) }

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
    assert_eq!(effects, Effects::two(Effect::Scroll { scroll: NodeId(7), by: 30.0 }, Effect::WakeAt(1.25)), "600 points a second for 50 ms");
    assert_eq!(step(&mut seq, &mut state, Event::Due { attempt, now: 1.25, edge: e }), Effects::none(), "a step is taken once");
    assert_eq!(step(&mut seq, &mut state, Event::Laid), Effects::one(Effect::Extend), "laid out, the selection follows");
    assert_eq!(step(&mut seq, &mut state, Event::Laid), Effects::none());
}

#[test]
fn a_step_stops_at_the_room_left_and_a_late_frame_doesnt_jump() {
    let (mut seq, mut state) = (Seq::default(), State::Idle);
    let up = |room, offset| edge(-600.0, room, offset);
    let _ = step(&mut seq, &mut state, Event::Moved { now: 0.0, edge: up(100.0, 100.0) });
    let State::Running { attempt, .. } = state else { panic!() };
    let effects = step(&mut seq, &mut state, Event::Due { attempt, now: 1.0, edge: up(100.0, 100.0) });
    assert_eq!(effects, Effects::two(Effect::Scroll { scroll: NodeId(7), by: -30.0 }, Effect::WakeAt(1.0)), "a second late steps 50 ms");
    let _ = step(&mut seq, &mut state, Event::Laid);
    let State::Running { attempt, .. } = state else { panic!() };
    let effects = step(&mut seq, &mut state, Event::Due { attempt, now: 1.05, edge: up(10.0, 10.0) });
    assert_eq!(effects, Effects::one(Effect::Scroll { scroll: NodeId(7), by: -10.0 }), "the last step asks for no other");
    assert_eq!(step(&mut seq, &mut state, Event::Laid), Effects::one(Effect::Extend), "the selection follows it in the same frame");
    assert_eq!(state, State::AtEnd { scroll: NodeId(7), max: 1000.0 }, "at the end, waiting on the extent");
    let _ = step(&mut seq, &mut state, Event::Moved { now: 2.0, edge: up(0.0, 0.0) });
    assert_eq!(state, State::AtEnd { scroll: NodeId(7), max: 1000.0 }, "nothing left to scroll");
    assert!(step(&mut seq, &mut state, Event::Laid).is_empty());
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
    let _ = step(&mut seq, &mut state, Event::Laid);
    let State::Running { attempt, .. } = state else { panic!() };
    let _ = step(&mut seq, &mut state, Event::Due { attempt, now: 0.01, edge: up(500.0) });
    let _ = step(&mut seq, &mut state, Event::Laid);
    let State::Running { attempt, .. } = state else { panic!() };
    assert_eq!(step(&mut seq, &mut state, Event::Due { attempt, now: 0.02, edge: up(500.0) }), Effects::none(), "pulled back");
    assert_eq!(state, State::AtEnd { scroll: NodeId(7), max: 1000.0 }, "until the content changes");
}

/// Frames until the edge scroll stops stepping.
fn run_out(rt: &mut Runtime<List>) {
    let mut ticks = 0;
    while rt.selectable.edge.running() {
        rt.frame(1.0 / 60.0);
        ticks += 1;
        assert!(ticks < 2000, "reached the end");
    }
}

/// Holds below the list until it has scrolled to its end and rests there with no wake.
fn rest_at_end(list: List) -> Runtime<List> {
    let mut rt = hold_below_in(list, 400.0);
    run_out(&mut rt);
    settle(&mut rt);
    assert_eq!((offset(&rt), ends(&rt).1), (max(&rt), ROWS - 1), "at the end, the last row selected");
    assert_eq!(rt.wake_deadline(), None, "no wake while it rests");
    assert!(matches!(rt.selectable.edge.state, State::AtEnd { .. }), "{:?}", rt.selectable.edge.state);
    rt
}

/// Held still past the end of a list it scrolled to the end of, rows arriving at that end set it
/// going again: it scrolls to the new end, selecting them, and rests there with no wake again.
#[test]
fn rows_arriving_at_the_end_a_held_drag_rests_at_set_it_going_again() {
    for list in [VIRTUAL, PLAIN] {
        let mut rt = rest_at_end(list);
        let (before, anchor) = (max(&rt), range(&rt).0);
        rt.update(|s, _| s.rows += 20);
        rt.frame(1.0 / 60.0);
        assert!(max(&rt) > before, "virtual {}: the extent grew", list.virtual_rows);
        let at = offset(&rt);
        for _ in 0..3 { rt.frame(1.0 / 60.0); }
        assert!(offset(&rt) > at, "virtual {}: it goes on under a still pointer", list.virtual_rows);
        run_out(&mut rt);
        settle(&mut rt);
        assert_eq!(offset(&rt), max(&rt), "to the new end");
        assert_eq!(range(&rt), (anchor, (ROWS + 19, LEN)), "virtual {}: the new rows selected", list.virtual_rows);
        assert_eq!(rt.wake_deadline(), None, "and rests again");
        assert!(!rt.frame(1.0 / 60.0), "an idle frame");
        assert!(rt.press.is_held() && rt.selectable.phase.dragging(), "still held");
    }
}

/// Rows that arrive after the drag let go of the edge (released, Escape, back inside, focus
/// lost) move neither the list nor the selection.
#[test]
fn rows_arriving_after_it_let_go_of_the_end_change_nothing() {
    let stops: [(&str, fn(&mut Runtime<List>)); 4] = [
        ("release", |rt| rt.pointer_up(150.0, BOTTOM + 400.0)),
        ("escape", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() })),
        ("inside", |rt| rt.pointer_move(150.0, 60.0)),
        ("blur", |rt| { rt.window_focus(true); rt.window_focus(false); }),
    ];
    for list in [VIRTUAL, PLAIN] {
        for (name, stop) in stops {
            let mut rt = rest_at_end(list);
            stop(&mut rt);
            assert_eq!(rt.selectable.edge.state, State::Idle, "{name}");
            settle(&mut rt);
            let (at, selected) = (offset(&rt), range(&rt));
            rt.update(|s, _| s.rows += 20);
            settle(&mut rt);
            assert_eq!((offset(&rt), range(&rt)), (at, selected), "virtual {} {name}: the list and the selection stay", list.virtual_rows);
            assert_eq!(rt.wake_deadline(), None, "{name}");
        }
    }
}

/// A list stuck to its bottom follows rows that arrive there under a still pointer held below
/// it: the selection follows the pointer over them, to the new last row, and it rests there
/// with no wake.
#[test]
fn a_list_stuck_to_its_bottom_brings_new_rows_into_a_held_selection() {
    let mut rt = rest_at_end(List { stick: true, ..VIRTUAL });
    assert!(rt.scrolls.values().next().unwrap().pinned(), "stuck to its bottom");
    let anchor = range(&rt).0;
    rt.update(|s, _| s.rows += 20);
    let mut frames = 0;
    while offset(&rt) < max(&rt) || frames == 0 {
        rt.frame(1.0 / 60.0);
        frames += 1;
        assert!(frames < 600, "the list followed its bottom");
    }
    settle(&mut rt);
    assert_eq!(offset(&rt), max(&rt), "at the new end");
    assert_eq!(range(&rt), (anchor, (ROWS + 19, LEN)), "the selection reaches the new last row");
    assert_eq!(rt.wake_deadline(), None, "and rests");
    assert!(!rt.frame(1.0 / 60.0), "an idle frame");
    assert!(rt.press.is_held() && rt.selectable.phase.dragging(), "still held");
}

/// A list that shrinks while a held drag waits at its end ends the wait: rows appended after it
/// move neither the list nor the selection, whose ends both stay.
#[test]
fn a_list_that_shrinks_under_a_resting_drag_ends_the_wait() {
    let mut rt = rest_at_end(PLAIN);
    let (before, selected) = (max(&rt), range(&rt));
    rt.update(|s, _| s.short = true);
    settle(&mut rt);
    assert!(max(&rt) < before, "the extent shrank");
    assert_eq!((offset(&rt), range(&rt)), (max(&rt), selected), "at the new end, the selection as it was");
    let at = offset(&rt);
    rt.update(|s, _| s.rows += 20);
    settle(&mut rt);
    assert_eq!((offset(&rt), range(&rt)), (at, selected), "the rows that arrive after it move nothing");
    assert_eq!(rt.wake_deadline(), None);
    assert!(rt.press.is_held() && rt.selectable.phase.dragging(), "still held");
}

/// The edge scroll's letters, as the runtime feeds them: a pointer move, the frame for the step
/// asked for (or an old one's), a stepped layout, a layout that changed the extent, a stop.
#[derive(Clone, Copy, Debug)]
enum Letter { Moved(At), Due(At), Stale, Laid, Extent(At), Stop }

/// Where the pointer is: inside, at the bottom of viewport 7 with room, with less room than a
/// step, at its end, at its end after more rows arrived or after some left, at the bottom of
/// another viewport.
#[derive(Clone, Copy, Debug)]
enum At { Inside, Room, Near, End, Grown, Shrunk, Other }

fn at(at: At) -> Option<Edge> {
    let e = |scroll, room, offset, max| Some(Edge { scroll: NodeId(scroll), speed: 600.0, room, offset, max });
    match at {
        At::Inside => None,
        At::Room => e(7, 100.0, 0.0, 100.0),
        At::Near => e(7, 3.0, 97.0, 100.0),
        At::End => e(7, 0.0, 100.0, 100.0),
        At::Grown => e(7, 50.0, 100.0, 150.0),
        At::Shrunk => e(7, 0.0, 80.0, 80.0),
        At::Other => e(8, 100.0, 0.0, 100.0),
    }
}

/// origin/dev's `edge_scroll::step`, transcribed: its three states, and where it went instead of
/// resting at an end.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Dev {
    Idle,
    Running { attempt: u32, scroll: NodeId, last: f64, from: Option<f64>, due: f64 },
    Laying { next: Option<u32>, scroll: NodeId, last: f64, from: Option<f64>,
        /// differs from dev: the extent the step saw.
        max: f64 },
    /// differs from dev: dev went idle wherever this rests.
    AtEnd { scroll: NodeId, max: f64 },
}

struct Reference { seq: u32, state: Dev }

impl Reference {
    fn next(&mut self) -> u32 { self.seq += 1; self.seq }

    fn moves(e: &Edge) -> bool { e.speed != 0.0 && e.room > 0.0 }

    /// differs from dev: dev went idle; at an edge of `scroll` (any, when `None`) it rests.
    fn rest(edge: Option<Edge>, scroll: Option<NodeId>) -> Dev {
        match edge {
            Some(e) if e.speed != 0.0 && (scroll.is_none() || scroll == Some(e.scroll)) => Dev::AtEnd { scroll: e.scroll, max: e.max },
            _ => Dev::Idle,
        }
    }

    fn moved(&mut self, now: f64, edge: Option<Edge>) -> Vec<Effect> {
        match (edge.filter(Self::moves), self.state) {
            (None, _) => {
                self.state = Self::rest(edge, None);
                vec![]
            }
            (Some(e), Dev::Running { scroll, .. }) if scroll == e.scroll => vec![],
            (Some(e), _) => {
                let attempt = self.next();
                self.state = Dev::Running { attempt, scroll: e.scroll, last: now, from: None, due: now };
                vec![Effect::WakeAt(now)]
            }
        }
    }

    fn due(&mut self, attempt: u32, now: f64, edge: Option<Edge>) -> Vec<Effect> {
        match self.state {
            Dev::Running { attempt: armed, scroll, last, from, due } if armed == attempt && now >= due => {
                let Some(e) = edge.filter(|e| Self::moves(e) && e.scroll == scroll) else {
                    self.state = Self::rest(edge, Some(scroll));
                    return vec![];
                };
                if from.is_some_and(|f| (e.offset - f).abs() <= 1e-9) {
                    // differs from dev: dev went idle.
                    self.state = Dev::AtEnd { scroll, max: e.max };
                    return vec![];
                }
                let by = f64::from(e.speed) * (now - last).clamp(0.0, 0.05);
                let (by, end) = if e.room - by.abs() <= 1e-3 { (e.room.copysign(f64::from(e.speed)), true) } else { (by, false) };
                let next = if end { None } else { Some(self.next()) };
                self.state = Dev::Laying { next, scroll, last: now, from: if by != 0.0 { Some(e.offset) } else { None }, max: e.max };
                let mut effects = vec![Effect::Scroll { scroll, by }];
                if !end { effects.push(Effect::WakeAt(now)); }
                effects
            }
            _ => vec![],
        }
    }

    fn laid(&mut self) -> Vec<Effect> {
        let Dev::Laying { next, scroll, last, from, max } = self.state else { return vec![] };
        self.state = match next {
            Some(attempt) => Dev::Running { attempt, scroll, last, from, due: last },
            // differs from dev: dev went idle.
            None => Dev::AtEnd { scroll, max },
        };
        vec![Effect::Extend]
    }

    /// differs from dev: dev had no such event; resting at the end, room the layout made goes on.
    fn extent(&mut self, now: f64, edge: Option<Edge>) -> Vec<Effect> {
        let Dev::AtEnd { scroll, max } = self.state else { return vec![] };
        match edge {
            Some(e) if e.scroll == scroll && e.max < max => {
                self.state = Dev::Idle;
                vec![]
            }
            Some(e) if Self::moves(&e) && e.scroll == scroll => {
                let attempt = self.next();
                self.state = Dev::Running { attempt, scroll, last: now, from: None, due: now };
                vec![Effect::WakeAt(now)]
            }
            // differs from dev: dev rested at the new extent without extending the selection.
            Some(e) if e.scroll == scroll && e.speed != 0.0 && e.max > max => {
                self.state = Dev::AtEnd { scroll, max: e.max };
                vec![Effect::Extend]
            }
            _ => {
                self.state = Self::rest(edge, Some(scroll));
                vec![]
            }
        }
    }
}

fn same(state: &State, dev: &Dev) -> bool {
    match (*state, *dev) {
        (State::Idle, Dev::Idle) => true,
        (State::Running { attempt, scroll, last, from, due }, Dev::Running { attempt: a, scroll: s, last: l, from: f, due: d }) =>
            (attempt.0, scroll, last, from, due) == (a, s, l, f, d),
        (State::Laying { next, scroll, last, from, max }, Dev::Laying { next: n, scroll: s, last: l, from: f, max: m }) =>
            (next.map(|a| a.0), scroll, last, from, max) == (n, s, l, f, m),
        (State::AtEnd { scroll, max }, Dev::AtEnd { scroll: s, max: m }) => (scroll, max) == (s, m),
        _ => false,
    }
}

/// Drives `letters` through `step` and the reference, a letter a frame's tenth apart, comparing
/// the effects and the state after every one.
fn walk(letters: &[Letter]) {
    let (mut seq, mut state) = (Seq::default(), State::Idle);
    let mut dev = Reference { seq: 0, state: Dev::Idle };
    for (i, &letter) in letters.iter().enumerate() {
        let now = 1.0 + i as f64 * 0.01;
        // The runtime asks for a step with the attempt its state holds; an old one is attempt 0.
        let armed = match state { State::Running { attempt, .. } => attempt, State::Laying { next: Some(a), .. } => a, _ => Attempt(0) };
        let (event, theirs) = match letter {
            Letter::Moved(a) => (Event::Moved { now, edge: at(a) }, dev.moved(now, at(a))),
            Letter::Due(a) => (Event::Due { attempt: armed, now, edge: at(a) }, dev.due(armed.0, now, at(a))),
            Letter::Stale => (Event::Due { attempt: Attempt(0), now, edge: at(At::Room) }, dev.due(0, now, at(At::Room))),
            Letter::Laid => (Event::Laid, dev.laid()),
            Letter::Extent(a) => (Event::Extent { now, edge: at(a) }, dev.extent(now, at(a))),
            Letter::Stop => (Event::Stop, { dev.state = Dev::Idle; vec![] }),
        };
        let ours: Vec<Effect> = step(&mut seq, &mut state, event).into_iter().collect();
        assert_eq!(ours, theirs, "{letters:?}, letter {i}");
        assert!(same(&state, &dev.state), "{letters:?}, letter {i}: {state:?}, reference {:?}", dev.state);
    }
}

fn walks(alphabet: &[Letter], len: usize, prefix: &mut Vec<Letter>, ran: &mut usize) {
    walk(prefix);
    *ran += 1;
    if prefix.len() == len { return; }
    for &letter in alphabet {
        prefix.push(letter);
        walks(alphabet, len, prefix, ran);
        prefix.pop();
    }
}

#[test]
fn every_short_sequence_steps_as_the_reference_does() {
    let ats = [At::Inside, At::Room, At::Near, At::End, At::Grown, At::Shrunk, At::Other];
    let alphabet: Vec<Letter> = ats.iter().map(|&a| Letter::Moved(a))
        .chain(ats.iter().map(|&a| Letter::Due(a)))
        .chain(ats.iter().map(|&a| Letter::Extent(a)))
        .chain([Letter::Stale, Letter::Laid, Letter::Stop])
        .collect();
    let mut ran = 0;
    walks(&alphabet, 5, &mut Vec::new(), &mut ran);
    assert_eq!(ran, (0..=5).map(|n| alphabet.len().pow(n)).sum::<usize>());
}
