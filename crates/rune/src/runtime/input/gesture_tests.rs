//! Wheels over elements that name the viewport they scroll (`El::wheel_scrolls`): a scroll bar
//! drawn beside the viewport it follows, which the chain up the ancestors can't find.
use super::*;

use crate::{Env, ScrollSpec, col, el};

/// The offset of the element at `path`.
fn offset(rt: &Runtime<()>, path: &str) -> f64 {
    let id = rt.tree.nodes.iter().find(|n| n.path == path).unwrap_or_else(|| panic!("{path}")).id;
    rt.scroll.scrolls[&id].offset
}

/// A bar beside its viewport, as `rune-kit`'s scroll bar is: in an `area` column with the
/// viewport, drawn after it, so a wheel over the bar hits it and the viewport is a sibling.
/// The named one hands its wheel to the viewport (`El::wheel_scrolls`); the plain one names
/// nothing, as any box taking input does.
fn barred(named: bool) -> El {
    let bar = el().key("bar").absolute(286.0, 0.0).size(14.0, 200.0).on_drag(|_: &mut (), _, _| {});
    let bar = if named { bar.wheel_scrolls("area/list") } else { bar };
    el().key("root").w_full().h_full().child(
        col().key("area").size(300.0, 200.0)
            .child(col().key("list").w_full().h_full().scroll(ScrollSpec { stick_bottom: false })
                .child(el().w_full().h(1000.0)))
            .child(bar),
    )
}

fn named_bar(_: &(), _: &mut crate::Cx<()>) -> El { barred(true) }
fn plain_bar(_: &(), _: &mut crate::Cx<()>) -> El { barred(false) }

/// A wheel over an element naming the viewport it scrolls (`El::wheel_scrolls`) is that
/// viewport's, though the viewport is its sibling drawn under it: an overlay scroll bar's
/// track beside the viewport it follows.
#[test]
fn a_wheel_over_an_element_that_names_its_viewport_scrolls_that_viewport() {
    let mut rt = Runtime::new((), named_bar, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.wheel(293.0, 100.0, -30.0);
    rt.frame(0.0);
    assert_eq!(offset(&rt, "root/area/list"), 30.0, "the wheel over the bar scrolled the list it follows");

    // Unnamed, the same bar leaves the wheel with what lies above it, as any box taking input
    // does: the list is a sibling, not an ancestor, so it never sees the wheel.
    let mut plain = Runtime::new((), plain_bar, (300.0, 200.0), Env::default());
    plain.frame(0.0);
    plain.wheel(293.0, 100.0, -30.0);
    plain.frame(0.0);
    assert_eq!(offset(&plain, "root/area/list"), 0.0, "a bar that names nothing scrolls nothing");
}

/// A wheel over a bar takes the named viewport before the page around it, and chains on up from
/// that viewport when it has no room, as one over the viewport itself does.
#[test]
fn a_wheel_a_named_viewport_cannot_take_chains_up_from_it() {
    fn paged(content: f32) -> El {
        el().key("root").w_full().h_full().child(
            col().key("page").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
                .child(el().key("body").w_full().h(1000.0).child(
                    col().key("area").size(300.0, 200.0)
                        .child(col().key("list").w_full().h_full().scroll(ScrollSpec { stick_bottom: false })
                            .child(el().w_full().h(content)))
                        .child(el().key("bar").absolute(286.0, 0.0).size(14.0, 200.0)
                            .on_drag(|_: &mut (), _, _| {}).wheel_scrolls("area/list")),
                )),
        )
    }
    fn page_tall(_:&(), _: &mut crate::Cx<()>) -> El { paged(1000.0) }
    fn page_short(_:&(), _: &mut crate::Cx<()>) -> El { paged(100.0) }

    let mut tall = Runtime::new((), page_tall, (300.0, 200.0), Env::default());
    tall.frame(0.0);
    tall.wheel(293.0, 100.0, -30.0);
    tall.frame(0.0);
    assert_eq!(offset(&tall, "root/page/body/area/list"), 30.0, "the named viewport takes the wheel before the page around it");
    assert_eq!(offset(&tall, "root/page"), 0.0, "the page around it stays where it is");

    let mut short = Runtime::new((), page_short, (300.0, 200.0), Env::default());
    short.frame(0.0);
    short.wheel(293.0, 100.0, -30.0);
    short.frame(0.0);
    assert_eq!(offset(&short, "root/page"), 30.0, "a short list hands the wheel on to the page around it");
    assert_eq!(offset(&short, "root/page/body/area/list"), 0.0, "the list itself never moved");
}

/// A raised panel over a bar keeps the wheel from the viewport under it, as #1137 chains a
/// wheel from what takes input or paints above: naming a viewport is how a bar belongs to
/// one, not a way through a panel over it.
#[test]
fn a_raised_panel_over_a_bar_keeps_the_wheel_from_the_viewport_under_it() {
    fn panel(_:&(), _: &mut crate::Cx<()>) -> El {
        el().key("root").w_full().h_full().child(
            col().key("area").size(300.0, 200.0)
                .child(col().key("list").w_full().h_full().scroll(ScrollSpec { stick_bottom: false })
                    .child(el().w_full().h(1000.0)))
                .child(el().key("bar").absolute(286.0, 0.0).size(14.0, 200.0)
                    .on_drag(|_: &mut (), _, _| {}).wheel_scrolls("area/list"))
                .child(el().key("panel").absolute(280.0, 0.0).size(20.0, 100.0).raise()),
        )
    }
    let mut rt = Runtime::new((), panel, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.wheel(293.0, 60.0, -30.0);
    rt.frame(0.0);
    assert_eq!(offset(&rt, "root/area/list"), 0.0, "the panel over the bar takes the wheel, as it did before");
}

/// Two panes keying their list and bar alike, as a chat and a sidebar do: the wheel over the
/// second pane's bar is its own, not the first element keyed `area/list` in the window.
#[test]
fn a_wheel_over_one_pane_s_bar_scrolls_that_pane_when_the_panes_key_alike() {
    fn panes(_: &(), _: &mut crate::Cx<()>) -> El {
        let pane = |key: &str| col().key(key.to_owned()).w(150.0).h(200.0).child(
            col().key("area").w_full().h_full()
                .child(col().key("list").w_full().h_full().scroll(ScrollSpec { stick_bottom: false })
                    .child(el().w_full().h(1000.0)))
                .child(el().key("bar").absolute(136.0, 0.0).size(14.0, 200.0)
                    .on_drag(|_: &mut (), _, _| {}).wheel_scrolls("area/list")),
        );
        crate::row().key("root").w_full().h_full().child(pane("left")).child(pane("right"))
    }
    let mut rt = Runtime::new((), panes, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    assert_eq!((offset(&rt, "root/left/area/list"), offset(&rt, "root/right/area/list")), (0.0, 0.0));
    // The second pane's bar: its strip over the pane's right edge, at x 286..300.
    rt.wheel(293.0, 100.0, -30.0);
    rt.frame(0.0);
    assert_eq!(offset(&rt, "root/right/area/list"), 30.0, "the wheel over the right pane's bar scrolled its own list");
    assert_eq!(offset(&rt, "root/left/area/list"), 0.0, "the left pane, keyed alike, stayed where it was");
}

/// A sideways wheel over a bar is the viewport the bar follows too: a two-way pane's vertical
/// track scrolls it across, and a table's horizontal track scrolls the table itself, where a
/// chain up from either track finds no horizontal scroller.
#[test]
fn a_sideways_wheel_over_a_bar_scrolls_the_viewport_it_follows() {
    fn both_ways(_: &(), _: &mut crate::Cx<()>) -> El {
        el().key("root").w_full().h_full().child(
            col().key("area").size(300.0, 200.0)
                .child(col().key("list").w_full().h_full().scroll_xy().child(el().size(900.0, 1000.0)))
                .child(el().key("bar").absolute(286.0, 0.0).size(14.0, 200.0)
                    .on_drag(|_: &mut (), _, _| {}).wheel_scrolls("area/list")),
        )
    }
    fn sideways(_: &(), _: &mut crate::Cx<()>) -> El {
        el().key("root").w_full().h_full().child(
            col().key("area").size(300.0, 200.0)
                .child(col().key("table").w_full().h(100.0).scroll_x().child(el().size(900.0, 100.0)))
                .child(el().key("bar").absolute(0.0, 102.0).size(300.0, 8.0)
                    .on_drag(|_: &mut (), _, _| {}).wheel_scrolls("area/table")),
        )
    }

    // A vertical bar over a viewport that scrolls both ways.
    let mut across = Runtime::new((), both_ways, (300.0, 200.0), Env::default());
    across.frame(0.0);
    across.wheel_xy(293.0, 100.0, -40.0, 0.0);
    across.frame(0.0);
    {
        let id = across.tree.nodes.iter().find(|n| n.path == "root/area/list").unwrap().id;
        assert_eq!(across.scroll.scrolls[&id].x.offset, 40.0, "the sideways wheel over the pane's bar scrolled it across");
        assert_eq!(across.scroll.scrolls[&id].offset, 0.0, "and left it where it was down");
    }

    // A horizontal bar under the table it belongs to.
    let mut along = Runtime::new((), sideways, (300.0, 200.0), Env::default());
    along.frame(0.0);
    along.wheel_xy(150.0, 106.0, -40.0, 0.0);
    along.frame(0.0);
    {
        let id = along.tree.nodes.iter().find(|n| n.path == "root/area/table").unwrap().id;
        assert_eq!(along.scroll.scrolls[&id].x.offset, 40.0, "the sideways wheel over the table's own bar scrolled the table");
    }
}

#[test]
fn drag_direction_follows_inherited_layout_direction() {
    fn view(_: &Vec<crate::Direction>, _: &mut crate::Cx<Vec<crate::Direction>>) -> El {
        col().size(200.0, 100.0)
            .child(el().key("ltr").size(200.0, 40.0).on_drag(|s: &mut Vec<crate::Direction>, d, _| s.push(d.direction())))
            .child(col().dir(crate::Direction::Rtl).size(200.0, 40.0)
                .child(el().key("rtl").size(200.0, 40.0).on_drag(|s: &mut Vec<crate::Direction>, d, _| s.push(d.direction()))))
    }
    let mut rt = Runtime::new(Vec::new(), view, (200.0, 100.0), Env::default());
    rt.frame(0.0);
    rt.pointer_down(100.0, 20.0);
    rt.pointer_move(120.0, 20.0);
    rt.pointer_up(120.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state, vec![crate::Direction::Ltr; 3]);
    rt.state.clear();
    rt.pointer_down(100.0, 60.0);
    rt.pointer_move(80.0, 60.0);
    rt.pointer_up(80.0, 60.0);
    rt.frame(0.0);
    assert_eq!(rt.state, vec![crate::Direction::Rtl; 3]);
}
