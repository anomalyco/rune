//! `El::anchored_fit`: an overlay that fits on neither side of its anchor goes to the side
//! with more room and is at most that room long, so the list in it scrolls.
use super::*;
use crate::el::{Align, ScrollSpec, Side};
use crate::{Cx, El, Env, col, el};

#[derive(Clone, Copy)]
struct S {
    /// The anchor's top edge.
    top: f32,
    rows: usize,
    fit: bool,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let rows = col().key("rows").children((0..s.rows).map(|i| el().key(format!("row{i}")).w(100.0).h(20.0)));
    let list = col().key("scroll").shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(rows);
    let menu = col().key("menu").w(100.0).child(list);
    let menu = if s.fit {
        menu.anchored_fit("target", Side::Bottom, Align::Start, 4.0)
    } else {
        menu.anchored("target", Side::Bottom, Align::Start, 4.0)
    };
    col().size(400.0, 300.0).child(el().key("target").absolute(20.0, s.top).size(100.0, 30.0)).child(menu)
}

fn menu(s: S) -> [f32; 4] {
    let mut rt = Runtime::new(s, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    rt.presented("menu").unwrap().0
}

#[test]
fn a_list_that_fits_below_is_left_alone() {
    let r = menu(S { top: 20.0, rows: 5, fit: true });
    assert_eq!((r[1], r[3]), (54.0, 100.0));
}

#[test]
fn a_list_that_fits_above_flips_whole() {
    let r = menu(S { top: 200.0, rows: 8, fit: true });
    assert_eq!((r[1], r[3]), (36.0, 160.0));
}

#[test]
fn a_long_list_takes_the_larger_room_and_scrolls() {
    // Below has 300 - 8 - 234 = 58 points, above 200 - 4 - 8 = 188.
    let r = menu(S { top: 200.0, rows: 40, fit: true });
    assert_eq!(r[3], 187.0, "bounded to the room above");
    assert!(r[1] >= 8.0 && r[1] + r[3] <= 196.0, "above the anchor, inside the window: {r:?}");
    // Without fit it stays below and runs off the window.
    let r = menu(S { top: 200.0, rows: 40, fit: false });
    assert_eq!((r[1], r[3]), (234.0, 800.0));
}

#[test]
fn a_long_list_with_more_room_below_stays_below() {
    // Below has 300 - 8 - 84 = 208 points, above 50 - 4 - 8 = 38.
    let r = menu(S { top: 50.0, rows: 40, fit: true });
    assert_eq!((r[1], r[3]), (84.0, 207.0));
}

#[test]
fn side_anchored_fit_clamps_cross_axis_height_and_main_axis_min_width() {
    struct SideState { rows: usize, anchor_x: f32 }
    fn side_view(s: &SideState, _: &mut Cx<SideState>) -> El {
        let rows = col().key("rows").children((0..s.rows).map(|i| el().key(format!("row{i}")).w(100.0).h(28.0)));
        let list = col().key("scroll").shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(rows);
        let sub = col().key("sub").min_w(160.0).child(list)
            .anchored_fit("target", Side::Right, Align::Start, 0.0);
        col().size(420.0, 400.0)
            .child(el().key("target").absolute(s.anchor_x, 90.0).size(156.0, 28.0))
            .child(sub)
    }

    // 8 rows (224 pt tall) fits vertically, but neither side has 160 pt horizontally (124 pt each).
    let mut short = Runtime::new(SideState { rows: 8, anchor_x: 132.0 }, side_view, (420.0, 400.0), Env::new(true, 1.0));
    short.frame(0.0);
    let r = short.presented("sub").unwrap().0;
    assert_eq!(r[2], 123.0, "min_w(160) shrinks to horizontal room 123: {r:?}");
    assert_eq!(r[3], 224.0, "height that fits is untouched: {r:?}");
    assert!(r[0] >= 8.0 && r[0] + r[2] <= 412.0 && r[1] >= 8.0 && r[1] + r[3] <= 392.0, "inside 8pt margin: {r:?}");

    // 20 rows (560 pt tall) clamps both width (123 pt) and cross-axis height (400 - 16 = 384 pt).
    let mut tall = Runtime::new(SideState { rows: 20, anchor_x: 132.0 }, side_view, (420.0, 400.0), Env::new(true, 1.0));
    tall.frame(0.0);
    let r = tall.presented("sub").unwrap().0;
    assert_eq!(r[2], 123.0, "horizontal main axis clamped: {r:?}");
    assert_eq!(r[3], 384.0, "vertical cross axis clamped to viewport.h - 2*MARGIN: {r:?}");
    assert_eq!(r[1], 8.0, "clamped to top margin: {r:?}");
    assert!(r[0] >= 8.0 && r[0] + r[2] <= 412.0 && r[1] + r[3] <= 392.0, "inside 8pt margin: {r:?}");

    // When there is room on the right (anchor at x=20..176 -> 236 pt right), width stays 160 pt.
    let mut roomy = Runtime::new(SideState { rows: 20, anchor_x: 20.0 }, side_view, (420.0, 400.0), Env::new(true, 1.0));
    roomy.frame(0.0);
    let r = roomy.presented("sub").unwrap().0;
    assert_eq!(r[2], 160.0, "width stays 160 when side fits: {r:?}");
    assert_eq!(r[3], 384.0, "cross-axis height still clamps to 384: {r:?}");
}

#[test]
fn top_bottom_anchored_fit_clamps_overflowing_cross_axis_width_and_respects_boundary_and_zoom() {
    fn wide_view(_: &(), _: &mut Cx<()>) -> El {
        let wide = col().key("wide").min_w(500.0).h(40.0)
            .anchored_fit("target", Side::Bottom, Align::Start, 4.0);
        let narrow = col().key("narrow").w(120.0).h(40.0)
            .anchored_fit("target", Side::Bottom, Align::Start, 4.0);
        col().size(300.0, 240.0)
            .child(el().key("target").absolute(20.0, 20.0).size(80.0, 24.0))
            .child(wide)
            .child(narrow)
    }
    let mut rt = Runtime::new((), wide_view, (300.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let wr = rt.presented("wide").unwrap().0;
    assert_eq!(wr[2], 284.0, "cross-axis width clamped to 300 - 16: {wr:?}");
    assert_eq!(wr[0], 8.0, "shifted inside left margin: {wr:?}");
    let nr = rt.presented("narrow").unwrap().0;
    assert_eq!(nr, [20.0, 48.0, 120.0, 40.0], "non-overflowing overlay untouched");

    // Inside an El::anchored_in boundary [40, 30, 340, 230] -> cross height room = 200 - 16 = 184.
    fn bounded_view(_: &(), _: &mut Cx<()>) -> El {
        let rows = col().key("rows").children((0..20).map(|i| el().key(format!("r{i}")).w(80.0).h(20.0)));
        let list = col().key("scroll").shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(rows);
        let pane = col().key("pane").absolute(40.0, 30.0).size(300.0, 200.0)
            .child(el().key("t").absolute(10.0, 40.0).size(60.0, 24.0));
        let sub = col().key("sub").w(100.0).child(list)
            .anchored_fit("t", Side::Right, Align::Start, 4.0)
            .anchored_in("pane");
        col().size(600.0, 500.0).child(pane).child(sub)
    }
    let mut bounded = Runtime::new((), bounded_view, (600.0, 500.0), Env::new(true, 1.0));
    bounded.frame(0.0);
    let br = bounded.presented("sub").unwrap().0;
    assert_eq!(br[3], 184.0, "cross-axis height clamped to boundary height - 16: {br:?}");
    assert!(br[1] >= 38.0 && br[1] + br[3] <= 222.0, "inside boundary vertical margins: {br:?}");

    // Degenerate tiny viewport (10x10) never panics and stays finite.
    let mut tiny = Runtime::new((), wide_view, (10.0, 10.0), Env::new(true, 1.0));
    tiny.frame(0.0);
    let tr = tiny.presented("wide").unwrap().0;
    assert!(tr.iter().all(|v| v.is_finite()), "tiny viewport stays finite: {tr:?}");
}
