//! A held selection drag past the left or right edge of a viewport that scrolls sideways (an
//! unwrapped code block's `scroll_x`) scrolls it and extends the selection.
use crate::{Direction, El, Env, Runtime, ScrollSpec, TextStyle, col, el, text};

const W: f32 = 200.0;
const H: f32 = 40.0;
const OUTER_H: f32 = 120.0;
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

fn scroll_of<S: 'static>(rt: &Runtime<S>, key: &str) -> ((f64, f64), (f64, f64)) {
    let i = rt.tree.nodes.iter().position(|n| n.named(key)).unwrap_or_else(|| panic!("missing {key}"));
    let st = rt.scroll.scrolls.get(&rt.tree.nodes[i].id).unwrap_or_else(|| panic!("no scroll state for {key}"));
    ((st.x.offset, st.x.max), (st.offset, st.max))
}

fn settle_any<S: 'static>(rt: &mut Runtime<S>) {
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) { break; }
    }
}

fn selectable_line() -> El {
    text(LINE).font(TextStyle::new(14.0, 400.0)).h(H).selectable(true).key("line")
}

#[derive(Clone, Copy)]
struct Nested {
    above: usize,
    below: usize,
}

fn nested_window(cfg: Nested) -> Runtime<Nested> {
    let mut rt = Runtime::new(
        cfg,
        |s: &Nested, _: &mut crate::Cx<Nested>| {
            let mut inner = col().key("content").w_full();
            for i in 0..s.above {
                inner = inner.child(el().key(format!("above-{i}")).w_full().h(H));
            }
            inner = inner.child(col().key("inner").w_full().h(H).scroll_x().child(selectable_line()));
            for i in 0..s.below {
                inner = inner.child(el().key(format!("below-{i}")).w_full().h(H));
            }
            col().key("root").w(W).h(OUTER_H).child(
                col().key("outer").w_full().h(OUTER_H).scroll(ScrollSpec::default()).child(inner),
            )
        },
        (W, OUTER_H),
        Env::new(true, 1.0),
    );
    settle_any(&mut rt);
    rt
}

#[test]
fn scroll_x_inside_vertical_scroll_with_no_vertical_overflow_scrolls_horizontally() {
    for drag_y in [-40.0, OUTER_H + 40.0] {
        let mut rt = nested_window(Nested { above: 0, below: 0 });
        let ((start_x, max_x), (start_y, max_y)) = (scroll_of(&rt, "inner").0, scroll_of(&rt, "outer").1);
        assert!(start_x == 0.0 && max_x > 100.0, "inner overflows horizontally: {start_x} {max_x}");
        assert_eq!((start_y, max_y), (0.0, 0.0), "outer has no vertical overflow");
        rt.pointer_down(0.5, 20.0);
        rt.frame(1.0 / 60.0);
        rt.pointer_move(W + 100.0, drag_y);
        rt.frame(1.0 / 60.0);
        let mut last_x = scroll_of(&rt, "inner").0.0;
        for tick in 0..10 {
            rt.frame(1.0 / 60.0);
            let now_x = scroll_of(&rt, "inner").0.0;
            assert!(now_x > last_x, "drag_y={drag_y} tick {tick}: horizontal scroll advances ({last_x} -> {now_x})");
            last_x = now_x;
        }
        for _ in 0..240 { rt.frame(1.0 / 60.0); }
        assert_eq!(scroll_of(&rt, "inner").0.0, max_x, "drag_y={drag_y}: reaches horizontal end");
        assert_eq!(scroll_of(&rt, "outer").1.0, 0.0, "drag_y={drag_y}: vertical stays at 0");
        assert_eq!(rt.selected_text().as_deref(), Some(LINE), "drag_y={drag_y}: full line selected");
    }
}

#[test]
fn scroll_x_inside_vertical_scroll_at_top_or_bottom_scrolls_horizontally_and_prefers_vertical_when_movable() {
    // At top (y == 0, y.max > 0): dragging past top + right scrolls horizontally because vertical cannot move up.
    let mut rt = nested_window(Nested { above: 0, below: 6 });
    let ((_, max_x), (start_y, max_y)) = (scroll_of(&rt, "inner").0, scroll_of(&rt, "outer").1);
    assert!(max_x > 100.0 && max_y > 100.0 && start_y == 0.0);
    rt.pointer_down(0.5, 20.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(W + 100.0, -40.0);
    for _ in 0..240 { rt.frame(1.0 / 60.0); }
    assert_eq!(scroll_of(&rt, "inner").0.0, max_x, "at vertical top, past top+right scrolls horizontally");
    assert_eq!(scroll_of(&rt, "outer").1.0, 0.0, "vertical stays at top");
    assert_eq!(rt.selected_text().as_deref(), Some(LINE));

    // At top (y == 0, y.max > 0): dragging past bottom + right still prefers vertical while vertical has room.
    let mut rt = nested_window(Nested { above: 0, below: 6 });
    rt.pointer_down(0.5, 20.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(W + 100.0, OUTER_H + 40.0);
    for _ in 0..10 { rt.frame(1.0 / 60.0); }
    assert!(scroll_of(&rt, "outer").1.0 > 0.0, "movable vertical edge still wins over horizontal");
    assert_eq!(scroll_of(&rt, "inner").0.0, 0.0, "horizontal waits while vertical moves");

    // At bottom (y == y.max, y.max > 0): dragging past bottom + right scrolls horizontally because vertical cannot move down.
    let mut rt = nested_window(Nested { above: 6, below: 0 });
    rt.wheel(100.0, 60.0, -10_000.0);
    settle_any(&mut rt);
    let ((_, max_x), (at_y, max_y)) = (scroll_of(&rt, "inner").0, scroll_of(&rt, "outer").1);
    assert!(max_x > 100.0 && max_y > 100.0 && at_y == max_y, "at vertical bottom: {at_y}/{max_y}");
    rt.pointer_down(0.5, OUTER_H - H / 2.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(W + 100.0, OUTER_H + 40.0);
    for _ in 0..240 { rt.frame(1.0 / 60.0); }
    assert_eq!(scroll_of(&rt, "inner").0.0, max_x, "at vertical bottom, past bottom+right scrolls horizontally");
    assert_eq!(scroll_of(&rt, "outer").1.0, max_y, "vertical stays at bottom");
    assert_eq!(rt.selected_text().as_deref(), Some(LINE));
}

fn xy_window(cfg: Nested) -> Runtime<Nested> {
    let mut rt = Runtime::new(
        cfg,
        |s: &Nested, _: &mut crate::Cx<Nested>| {
            let mut content = col().key("content");
            for i in 0..s.above {
                content = content.child(el().key(format!("above-{i}")).w(W).h(H));
            }
            content = content.child(selectable_line());
            for i in 0..s.below {
                content = content.child(el().key(format!("below-{i}")).w(W).h(H));
            }
            col().key("root").w(W).h(OUTER_H).child(
                col().key("xy").w_full().h(OUTER_H).scroll_xy().child(content),
            )
        },
        (W, OUTER_H),
        Env::new(true, 1.0),
    );
    settle_any(&mut rt);
    rt
}

#[test]
fn scroll_xy_scrolls_horizontally_when_vertical_has_no_room() {
    // y.max == 0: dragging past bottom + right scrolls horizontally.
    let mut rt = xy_window(Nested { above: 0, below: 0 });
    let ((start_x, max_x), (start_y, max_y)) = scroll_of(&rt, "xy");
    assert!(start_x == 0.0 && max_x > 100.0 && start_y == 0.0 && max_y == 0.0);
    rt.pointer_down(0.5, 20.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(W + 100.0, OUTER_H + 40.0);
    for _ in 0..240 { rt.frame(1.0 / 60.0); }
    assert_eq!(scroll_of(&rt, "xy").0.0, max_x, "scroll_xy with y.max == 0 reaches x.max");
    assert_eq!(rt.selected_text().as_deref(), Some(LINE));

    // y.max > 0 and already at top: dragging past top + right scrolls horizontally.
    let mut rt = xy_window(Nested { above: 0, below: 6 });
    let ((_, max_x), (start_y, max_y)) = scroll_of(&rt, "xy");
    assert!(max_x > 100.0 && max_y > 100.0 && start_y == 0.0);
    rt.pointer_down(0.5, 20.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(W + 100.0, -40.0);
    for _ in 0..240 { rt.frame(1.0 / 60.0); }
    assert_eq!(scroll_of(&rt, "xy").0.0, max_x, "scroll_xy at top reaches x.max when past top+right");
    assert_eq!(scroll_of(&rt, "xy").1.0, 0.0);
    assert_eq!(rt.selected_text().as_deref(), Some(LINE));

    // y.max > 0 and already at bottom: dragging past bottom + right scrolls horizontally.
    let mut rt = xy_window(Nested { above: 6, below: 0 });
    rt.wheel(100.0, 60.0, -10_000.0);
    settle_any(&mut rt);
    let ((_, max_x), (at_y, max_y)) = scroll_of(&rt, "xy");
    assert!(max_x > 100.0 && max_y > 100.0 && at_y == max_y);
    rt.pointer_down(0.5, OUTER_H - H / 2.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(W + 100.0, OUTER_H + 40.0);
    for _ in 0..240 { rt.frame(1.0 / 60.0); }
    assert_eq!(scroll_of(&rt, "xy").0.0, max_x, "scroll_xy at bottom reaches x.max when past bottom+right");
    assert_eq!(scroll_of(&rt, "xy").1.0, max_y);
    assert_eq!(rt.selected_text().as_deref(), Some(LINE));
}

#[test]
fn rtl_scroll_x_edge_scrolls_left_to_max_and_right_back_to_start() {
    // Nested inside a vertical scroll with y.max == 0 so both RTL flow and vertical fallback are exercised.
    let mut rt = Runtime::new(
        (),
        |_: &(), _: &mut crate::Cx<()>| {
            let line = text(LINE).font(TextStyle::new(14.0, 400.0)).h(H).selectable(true).key("line");
            let inner = col().key("inner").dir(Direction::Rtl).w_full().h(H).scroll_x().child(line);
            col().key("root").w(W).h(OUTER_H).child(
                col().key("outer").w_full().h(OUTER_H).scroll(ScrollSpec::default()).child(inner),
            )
        },
        (W, OUTER_H),
        Env::new(true, 1.0),
    );
    settle_any(&mut rt);
    let ((start_x, max_x), (_, max_y)) = (scroll_of(&rt, "inner").0, scroll_of(&rt, "outer").1);
    assert!(start_x == 0.0 && max_x > 100.0 && max_y == 0.0, "RTL overflows left: {start_x} {max_x} {max_y}");

    // Dragging past the left edge (and past the unmovable top vertical edge) grows offset from 0 toward max.
    rt.pointer_down(W - 0.5, 20.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(-100.0, -40.0);
    rt.frame(1.0 / 60.0);
    let mut last_x = scroll_of(&rt, "inner").0.0;
    for tick in 0..10 {
        rt.frame(1.0 / 60.0);
        let now_x = scroll_of(&rt, "inner").0.0;
        assert!(now_x > last_x, "RTL tick {tick}: offset grows past left edge ({last_x} -> {now_x})");
        last_x = now_x;
    }
    for _ in 0..240 { rt.frame(1.0 / 60.0); }
    assert_eq!(scroll_of(&rt, "inner").0.0, max_x, "RTL reached far left end");
    assert!(!rt.edit.selectable.edge.running());
    rt.pointer_up(-100.0, -40.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some(LINE), "RTL drag selected full line");

    // Dragging past the right edge (and past the unmovable bottom vertical edge) returns offset toward 0.
    rt.pointer_down(4.0, 20.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(W + 100.0, OUTER_H + 40.0);
    rt.frame(1.0 / 60.0);
    let mut last_x = scroll_of(&rt, "inner").0.0;
    for tick in 0..10 {
        rt.frame(1.0 / 60.0);
        let now_x = scroll_of(&rt, "inner").0.0;
        assert!(now_x < last_x, "RTL back tick {tick}: offset shrinks past right edge ({last_x} -> {now_x})");
        last_x = now_x;
    }
    for _ in 0..240 { rt.frame(1.0 / 60.0); }
    assert_eq!(scroll_of(&rt, "inner").0.0, 0.0, "RTL returned to start");
    rt.pointer_up(W + 100.0, OUTER_H + 40.0);
    assert!(!rt.edit.selectable.edge.running());
}
