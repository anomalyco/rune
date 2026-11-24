//! A sidebar list's drag to reorder, through the runtime: drops, and a release the row never
//! hears.

use rune::{Cx, El, Env, LiveRegion, Runtime, Tx, col};
use rune_kit::Kit;
use rune_kit::shell::{Reorder, ReorderLanding, SidebarRow};

#[derive(Default)]
struct S {
    reorder: Reorder,
    /// The rows shown, by id.
    ids: Vec<usize>,
    dropped: Vec<(usize, usize)>,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    let count = s.ids.len();
    col().size(240.0, 400.0).children(s.ids.iter().enumerate().map(|(i, id)| {
        SidebarRow::new(format!("Row {id}"))
            .reorder(&s.reorder, i, count, |s: &mut S| &mut s.reorder, |s: &mut S, from, to| s.dropped.push((from, to)))
            .build(&k)
            .key(format!("r{id}"))
    }))
}

fn runtime() -> Runtime<S> {
    let mut rt = Runtime::new(S { ids: (0..5).collect(), ..S::default() }, view, (240.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt
}

/// The middle of row `id`.
fn at(rt: &Runtime<S>, id: usize) -> (f32, f32) {
    let key = format!("r{id}");
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str())).expect("the row").rect;
    (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

#[test]
fn a_drop_moves_the_row_and_a_cancel_doesnt() {
    let mut rt = runtime();
    let (x, y) = at(&rt, 1);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 60.0);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.reorder.dragging(), rt.state.reorder.target()), (Some(1), Some(3)));
    rt.pointer_up(x, y + 60.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.dropped, [(1, 3)]);
    assert_eq!(rt.state.reorder.dragging(), None);

    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 60.0);
    rt.frame(1.0 / 60.0);
    rt.window_focus(false);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.dropped.len(), rt.state.reorder.dragging()), (1, None), "a cancel moves nothing");
}

#[test]
fn a_row_whose_release_is_lost_lets_go_and_the_next_press_drags_its_own_row() {
    let mut rt = runtime();
    let (x, y) = at(&rt, 1);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 60.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.reorder.dragging(), Some(1));
    // The dragged row goes mid-drag, so its release never reaches it.
    rt.update(|s, _| s.ids.retain(|&id| id != 1));
    rt.frame(1.0 / 60.0);
    rt.pointer_up(x, y + 60.0);
    rt.frame(1.0 / 60.0);
    // Over the first row, which the drag never moved.
    rt.pointer_move(x, 10.0);
    rt.pointer_move(x, 20.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.reorder.dragging(), None, "the list let go of the row once the pointer moved on");

    // Row 3, now the list's third, drags down a place.
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = at(&rt, 3);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 30.0);
    rt.pointer_up(x, y + 30.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.dropped, [(2, 3)]);
}

struct TxState {
    reorder: Reorder,
    ids: Vec<usize>,
    dropped: Vec<(usize, usize)>,
    tx_drops: usize,
    inert: bool,
    reduced_motion: bool,
}

fn tx_view(s: &TxState, cx: &mut Cx<TxState>) -> El {
    if s.reduced_motion {
        cx.reduced_motion(true);
    }
    let k = Kit::neutral(true);
    let count = s.ids.len();
    let list = col().size(240.0, 200.0).gap(2.0).children(s.ids.iter().enumerate().map(|(i, id)| {
        SidebarRow::new(format!("Row {id}"))
            .height(24.0)
            .reorder_tx(
                &s.reorder,
                i,
                count,
                |s: &mut TxState| &mut s.reorder,
                |s: &mut TxState, from, to, tx: &mut Tx| {
                    let moved = s.ids.remove(from);
                    s.ids.insert(to, moved);
                    s.dropped.push((from, to));
                    s.tx_drops += 1;
                    tx.instant();
                    tx.announce(format!("Moved to position {}", to + 1), LiveRegion::Polite);
                },
            )
            .build(&k)
            .key(format!("r{id}"))
    }));
    if s.inert { list.inert() } else { list }
}

fn tx_runtime(reorder: Reorder) -> Runtime<TxState> {
    let state = TxState { reorder, ids: vec![0, 1, 2], dropped: Vec::new(), tx_drops: 0, inert: false, reduced_motion: false };
    let mut rt = Runtime::new(state, tx_view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt
}

fn tx_at(rt: &Runtime<TxState>, id: usize) -> (f32, f32) {
    let key = format!("r{id}");
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str())).expect("the row").rect;
    (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

#[test]
fn half_row_landing_and_cancel_outside_x_through_runtime() {
    // Default Nearest: 3 rows, 24 pt tall + 2 pt gap, dragging row 0 down 39 pt drops at (0, 2).
    let mut rt_nearest = tx_runtime(Reorder::new().gap(2.0));
    let (x, y) = tx_at(&rt_nearest, 0);
    rt_nearest.pointer_move(x, y);
    rt_nearest.pointer_down(x, y);
    rt_nearest.pointer_move(x, y + 39.0);
    rt_nearest.frame(1.0 / 60.0);
    assert_eq!(rt_nearest.state.reorder.target(), Some(2));
    assert_eq!(rt_nearest.state.reorder.offset(), Some(39.0));
    assert_eq!(rt_nearest.state.reorder.pitch(), Some(26.0));
    rt_nearest.pointer_up(x, y + 39.0);
    rt_nearest.frame(1.0 / 60.0);
    assert_eq!(rt_nearest.state.dropped, [(0, 2)]);
    assert_eq!(rt_nearest.state.tx_drops, 1);
    assert_eq!(rt_nearest.state.ids, [1, 2, 0]);

    // HalfRow + cancel_outside_x(80.0): dragging 39 pt down targets (0, 1); moving 100 pt sideways
    // clears target and neighbour shifts, and releasing cancels.
    let mut rt = tx_runtime(Reorder::new().gap(2.0).landing(ReorderLanding::HalfRow).cancel_outside_x(80.0));
    let (x, y) = tx_at(&rt, 0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 39.0);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.reorder.dragging(), rt.state.reorder.target()), (Some(0), Some(1)));
    assert_eq!((rt.state.reorder.shift(0), rt.state.reorder.shift(1), rt.state.reorder.shift(2)), (39.0, -26.0, 0.0));

    // Move sideways past 80 pt: target clears and neighbours spring back to 0.
    rt.pointer_move(x + 100.0, y + 39.0);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.reorder.dragging(), rt.state.reorder.target()), (Some(0), None));
    assert_eq!((rt.state.reorder.offset(), rt.state.reorder.pitch()), (Some(39.0), Some(26.0)));
    assert_eq!((rt.state.reorder.shift(0), rt.state.reorder.shift(1), rt.state.reorder.shift(2)), (39.0, 0.0, 0.0));

    // Releasing outside cancels the drop.
    rt.pointer_up(x + 100.0, y + 39.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.dropped.is_empty());
    assert_eq!(rt.state.ids, [0, 1, 2]);
    assert_eq!(rt.state.reorder.dragging(), None);

    // Dragging outside and returning inside before release commits (0, 1) via reorder_tx.
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = tx_at(&rt, 0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x + 100.0, y + 39.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.reorder.target(), None);
    rt.pointer_move(x + 20.0, y + 39.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.reorder.target(), Some(1));
    rt.pointer_up(x + 20.0, y + 39.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.dropped, [(0, 1)]);
    assert_eq!(rt.state.tx_drops, 1);
    assert_eq!(rt.state.ids, [1, 0, 2]);
}

#[test]
fn reorder_tx_respects_inert_and_reduced_motion() {
    // Inert list ignores drags.
    let mut rt = tx_runtime(Reorder::new().gap(2.0).landing(ReorderLanding::HalfRow).cancel_outside_x(80.0));
    rt.update(|s, _| s.inert = true);
    rt.frame(1.0 / 60.0);
    let (x, y) = tx_at(&rt, 0);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 39.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.reorder.dragging(), None);
    rt.pointer_up(x, y + 39.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.dropped.is_empty());

    // Turning inert mid-drag cancels without calling drop.
    rt.update(|s, _| s.inert = false);
    rt.frame(1.0 / 60.0);
    let (x, y) = tx_at(&rt, 0);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 39.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.reorder.dragging(), Some(0));
    rt.update(|s, _| s.inert = true);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.reorder.dragging(), None);
    rt.pointer_up(x, y + 39.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.dropped.is_empty());

    // Reduced motion still commits cleanly via reorder_tx.
    let mut rm = tx_runtime(Reorder::new().gap(2.0).landing(ReorderLanding::HalfRow).cancel_outside_x(80.0));
    rm.update(|s, _| s.reduced_motion = true);
    rm.frame(1.0 / 60.0);
    let (x, y) = tx_at(&rm, 0);
    rm.pointer_down(x, y);
    rm.pointer_move(x, y + 39.0);
    rm.frame(1.0 / 60.0);
    rm.pointer_up(x, y + 39.0);
    rm.frame(1.0 / 60.0);
    assert_eq!(rm.state.dropped, [(0, 1)]);
    assert_eq!(rm.state.ids, [1, 0, 2]);
}
