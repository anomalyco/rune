use super::*;
use crate::{Align, Live, Side, col, el, row};

#[derive(Default)]
struct FollowState {
    height: f32,
    removed: bool,
    hidden: bool,
    streaming: bool,
}

fn follow_view(s: &FollowState, _: &mut Cx<FollowState>) -> El {
    if s.hidden { return el(); }
    let mut stack = col().key("stack").w_full().growth(GrowthSpec::default())
        .child(el().key("lead").w_full().h(400.0))
        .child(el().key("grow").w_full().h(s.height).streaming(s.streaming));
    if !s.removed { stack = stack.child(el().key("tail").w_full().h(35.0)); }
    col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: true }).child(stack)
}

fn following(keep: bool) -> Runtime<FollowState> {
    let mut rt = Runtime::new(FollowState { height: 37.0, ..Default::default() }, follow_view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(move |s, tx| {
        s.height = 130.0;
        if keep { tx.keep_on_screen("tail"); } else { tx.scroll_into_view("tail"); }
    });
    rt.frame(1.0 / 120.0);
    rt
}

fn scroll(rt: &Runtime<FollowState>) -> &ScrollState {
    &rt.scroll.scrolls[&rt.node("scroll").unwrap().id]
}

#[test]
fn completed_reveal_and_keep_repin_the_end_and_go_idle() {
    for keep in [false, true] {
        let mut rt = following(keep);
        if !keep { assert!(!scroll(&rt).pinned()); }
        for _ in 0..240 { rt.frame(1.0 / 120.0); }
        assert!(rt.scroll.requests.is_empty());
        assert!(!rt.animating);
        assert!(scroll(&rt).pinned(), "the completed hold returns the end to stick_bottom");
        rt.update(|s, _| { s.height = 230.0; s.streaming = true; });
        for _ in 0..60 {
            rt.frame(1.0 / 120.0);
            assert!(scroll(&rt).pinned());
            assert!((scroll(&rt).offset - scroll(&rt).max).abs() < 0.05);
        }
    }
}

#[test]
fn all_scroll_input_and_later_destinations_cancel_a_reveal() {
    for input in 0..5 {
        let mut rt = following(false);
        assert!(rt.scroll.requests.reveals().next().is_some());
        match input {
            0 => rt.wheel_xy(100.0, 100.0, 0.0, 50.0),
            1 => { let i = rt.tree.node_index[&rt.node("scroll").unwrap().id]; rt.page_to(i, |_, _, _| 0.0); }
            2 => rt.wheel(100.0, 100.0, 50.0),
            3 => rt.fling(100.0, 100.0, 100.0),
            _ => rt.update(|_, tx| { tx.scroll_into_view("tail"); tx.scroll_to_y("scroll", 0.0); }),
        }
        if input != 4 { assert!(rt.scroll.requests.reveals().next().is_none(), "input {input} cancels at arrival, even before wheel dispatch"); }
        rt.frame(1.0 / 120.0);
        assert!(rt.scroll.requests.reveals().next().is_none(), "input {input} cancels the reveal");
        let offset = scroll(&rt).offset;
        if input != 3 {
            for _ in 0..60 { rt.frame(1.0 / 120.0); }
            assert!((scroll(&rt).offset - offset).abs() < 0.05, "no reveal fights input {input}");
        }
    }
}

#[test]
fn a_missing_reveal_target_or_viewport_is_pruned_during_other_growth() {
    for hidden in [false, true] {
        let mut rt = following(false);
        rt.update(move |s, _| { s.removed = true; s.hidden = hidden; s.height = 300.0; });
        rt.frame(1.0 / 120.0);
        assert!(rt.scroll.requests.reveals().next().is_none(), "missing targets must not wait for unrelated growth");
        for _ in 0..240 { rt.frame(1.0 / 120.0); }
        assert!(!rt.animating);
    }
}

#[test]
fn instant_only_overrides_scroll_requests_in_that_commit() {
    let mut rt = Runtime::new(FollowState { height: 37.0, ..Default::default() }, follow_view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 0.0));
    rt.frame(1.0 / 120.0);
    assert!(scroll(&rt).glide_to().is_some());
    rt.update(|_, tx| tx.instant());
    rt.frame(1.0 / 120.0);
    assert!(scroll(&rt).glide_to().is_some(), "an unrelated instant commit leaves an existing glide alone");
    rt.update(|_, tx| { tx.instant(); tx.scroll_to("scroll", 0.0, 100.0); });
    rt.frame(1.0 / 120.0);
    assert!(scroll(&rt).glide_to().is_none());
    assert!((scroll(&rt).offset - 100.0).abs() < 0.05);
    rt.update(|_, tx| { tx.instant(); tx.scroll_to_bottom("scroll"); });
    rt.frame(1.0 / 120.0);
    assert!(scroll(&rt).glide().is_none());
    assert!((scroll(&rt).offset - scroll(&rt).max).abs() < 0.05);
}

#[test]
fn revealing_an_already_visible_row_stops_a_running_glide() {
    let mut rt = Runtime::new(FollowState { height: 37.0, ..Default::default() }, follow_view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 0.0));
    rt.frame(0.0);
    assert!(scroll(&rt).glide_to().is_some());
    rt.update(|_, tx| tx.scroll_into_view("tail"));
    rt.frame(1.0 / 120.0);
    assert!(scroll(&rt).glide().is_none() && scroll(&rt).glide_to().is_none());
    assert!((scroll(&rt).offset - scroll(&rt).max).abs() < 0.05);
    assert!(!rt.animating);
}

#[test]
fn revealing_another_viewport_does_not_move_a_virtual_reading_anchor() {
    fn view(_: &[f32; 2], _: &mut Cx<[f32; 2]>) -> El {
        row().size(600.0, 200.0).children((0..2).map(|list| {
            col().key(format!("scroll{list}")).size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
                .child(col().key(format!("stack{list}")).w_full().growth(GrowthSpec::default())
                    .virtual_rows(12, 50.0,
                        move |_: &[f32; 2], i| format!("r{list}-{i}").into(),
                        move |s: &[f32; 2], i| el().w_full().h(if i == 1 { s[list] } else { 50.0 })))
        }))
    }
    let mut rt = Runtime::new([50.0; 2], view, (600.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_y("scroll1", 150.0));
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    let y = |rt: &Runtime<[f32; 2]>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r1-3")).unwrap().rect[1];
    let was = y(&rt);
    for height in [100.0, 30.0] {
        rt.update(move |s, tx| { s[1] = height; tx.scroll_into_view("r0-4"); });
        for _ in 0..60 {
            rt.frame(1.0 / 120.0);
            assert!((y(&rt) - was).abs() < 0.05, "the reader's row moved after height {height}: {} vs {was}", y(&rt));
        }
    }
}

#[test]
fn sticky_left_holds_a_header_to_the_viewport_while_content_scrolls_sideways() {
    struct S {
        pressed: usize,
        offset: Live<[f32; 2]>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().size(300.0, 200.0)
            .child(col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).scroll_xy()
                .child(col().key("content").min_w(1000.0)
                    .child(row().key("header").w(300.0).h(30.0).justify_end().items_center().fill(Color::WHITE).sticky_left(0.0)
                        .live_offset(&s.offset)
                        .child(el().key("action").size(40.0, 20.0).focusable(true).help("Run action").on_press(|s: &mut S| s.pressed += 1)))
                    .child(el().w_full().h(600.0))))
            .child(el().key("pop").size(50.0, 20.0).anchored("action", Side::Bottom, Align::End, 0.0))
    }
    let mut rt = Runtime::new(S { pressed: 0, offset: Live::new([0.0, 0.0]) }, view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.wheel_xy(100.0, 100.0, -250.0, 0.0);
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    let els = rt.elements();
    let h = els.iter().find(|e| e.key.as_deref() == Some("header")).unwrap();
    assert_eq!(h.rect[0], 0.0);
    assert_eq!(h.stuck, 250.0);

    // A control at the trailing end of the shifted header is clickable, focusable and anchors
    // overlays and tooltips at its shifted viewport coordinates.
    let action = els.iter().find(|e| e.key.as_deref() == Some("action")).unwrap();
    assert_eq!(action.rect, [260.0, 5.0, 40.0, 20.0]);
    let pop = els.iter().find(|e| e.key.as_deref() == Some("pop")).unwrap();
    assert_eq!(pop.rect[0], 242.0, "anchored overlay follows sticky_left shift (kept 8pt inside viewport)");

    rt.pointer_move(280.0, 15.0);
    rt.frame(0.0);
    rt.frame(rt.help_delay);
    assert!(rt.help_tooltip().is_some(), "tooltip shows on shifted sticky_left child");
    rt.pointer_down(280.0, 15.0);
    rt.pointer_up(280.0, 15.0);
    rt.frame(0.0);
    assert_eq!(rt.state.pressed, 1, "shifted header action takes press after sideways scroll");

    // `live_offset` on a sticky_left element translates from the stuck position rather than
    // being clamped away by the sticky edge.
    rt.state.offset.set([-12.0, 4.0]);
    rt.frame(0.0);
    let h = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("header")).unwrap();
    assert_eq!((h.rect[0], h.rect[1], h.stuck), (-12.0, 4.0, 250.0));
}

#[test]
fn sticky_left_inside_nested_scrollers_moves_vertically_and_stops_at_parent_right() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().key("vscroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
            .child(col().w_full()
                .child(el().w_full().h(50.0))
                .child(col().key("hscroll").w(300.0).h(100.0).scroll(ScrollSpec { stick_bottom: false }).scroll_x()
                    .child(row().key("band").w(500.0).h(40.0).pr(20.0)
                        .child(el().key("pin").w(100.0).h(40.0).sticky_left(10.0))))
                .child(el().w_full().h(400.0)))
    }
    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    // Scroll horizontally inside `hscroll` and vertically in `vscroll`.
    rt.wheel_xy(150.0, 80.0, -120.0, 0.0);
    rt.wheel(150.0, 80.0, -30.0);
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    let pin = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("pin")).unwrap();
    assert_eq!(pin.rect[0], 10.0, "held at inset from horizontal viewport's left");
    assert_eq!(pin.rect[1], 20.0, "scrolled vertically with its outer row");
    assert_eq!(pin.stuck, 130.0);
}

#[test]
fn cached_section_with_sticky_descendant_stays_live_on_scroll() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).scroll_xy()
            .child(col().key("cached").w_full().cache()
                .child(el().key("header").w_full().h(30.0).fill(Color::WHITE).sticky_top(0.0))
                .child(el().key("left").w(300.0).h(30.0).fill(Color::WHITE).sticky_left(0.0))
                .child(el().w_full().h(600.0)))
    }
    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.wheel_xy(100.0, 100.0, -80.0, -100.0);
    rt.frame(0.0);
    let cached = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("cached")).unwrap();
    assert!(rt.paint.cache_live_scratch[cached], "sticky contents must not reuse a frozen raster");
    assert!(!rt.paint.node_caches.contains_key(&rt.tree.nodes[cached].id));
    let h = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("header")).unwrap();
    assert_eq!(h.rect[1], 0.0);
    assert_eq!(h.stuck, 100.0);
}
