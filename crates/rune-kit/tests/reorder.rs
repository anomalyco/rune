//! A sidebar list's drag to reorder, through the runtime: drops, and a release the row never
//! hears.

use rune::{Cx, El, Env, Runtime, col};
use rune_kit::Kit;
use rune_kit::shell::{Reorder, SidebarRow};

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
