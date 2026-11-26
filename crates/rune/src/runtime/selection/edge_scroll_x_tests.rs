//! A held selection drag past the left or right edge of a viewport that scrolls sideways (an
//! unwrapped code block's `scroll_x`) scrolls it and extends the selection.
use crate::{El, Env, Runtime, TextStyle, col, text};

const W: f32 = 200.0;
const H: f32 = 40.0;
const LINE: &str = "let value = some_function(first_argument, second_argument, third_argument);";

fn view(_: &(), _: &mut crate::Cx<()>) -> El {
    let line = text(LINE).font(TextStyle::new(14.0, 400.0)).h(H).selectable(true).key("line");
    col().key("root").w(W).h(H).child(col().key("scroll").w_full().h(H).scroll_x().child(line))
}

fn window() -> Runtime<()> {
    let mut rt = Runtime::new((), view, (W, H), Env::new(true, 1.0));
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<()>) {
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) { break; }
    }
}

fn x(rt: &Runtime<()>) -> (f64, f64) {
    let st = rt.scroll.scrolls.values().next().expect("one scroll");
    (st.x.offset, st.x.max)
}

fn focus(rt: &Runtime<()>) -> usize { rt.edit.selectable.phase.selection().expect("a selection").focus }

#[test]
fn held_past_the_right_edge_it_scrolls_across_and_selects_to_the_end() {
    let mut rt = window();
    let (start, max) = x(&rt);
    assert!(start == 0.0 && max > 100.0, "the line overflows: {start} {max}");
    rt.pointer_down(0.5, 20.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(W + 100.0, 20.0);
    rt.frame(1.0 / 60.0);
    let (mut last_offset, mut last_focus) = (x(&rt).0, focus(&rt));
    for tick in 0..10 {
        rt.frame(1.0 / 60.0);
        assert!(x(&rt).0 > last_offset, "tick {tick}: the line scrolls under a still pointer");
        assert!(focus(&rt) >= last_focus, "tick {tick}: the focus only moves right");
        (last_offset, last_focus) = (x(&rt).0, focus(&rt));
    }
    for _ in 0..240 { rt.frame(1.0 / 60.0); }
    assert_eq!(x(&rt).0, max, "it runs to the end of the line");
    assert!(!rt.edit.selectable.edge.running(), "and stops there");
    assert_eq!(focus(&rt), LINE.len(), "the selection reached the end");
    rt.pointer_up(W + 100.0, 20.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some(LINE));
}

#[test]
fn held_past_the_left_edge_it_scrolls_back() {
    let mut rt = window();
    rt.wheel_xy(100.0, 20.0, -10_000.0, 0.0);
    settle(&mut rt);
    let (end, max) = x(&rt);
    assert_eq!(end, max, "scrolled to the end of the line");
    rt.pointer_down(W - 4.0, 20.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(-100.0, 20.0);
    let mut last = end;
    for tick in 0..10 {
        rt.frame(1.0 / 60.0);
        assert!(x(&rt).0 < last, "tick {tick}");
        last = x(&rt).0;
    }
    rt.pointer_up(-100.0, 20.0);
    assert!(!rt.edit.selectable.edge.running(), "the release stops it");
}
