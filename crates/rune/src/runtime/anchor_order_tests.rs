//! Anchored overlays mounted before their anchor in tree order (such as a select portal built
//! before its enclosing dialog): they resolve their later anchor, fit against it, inherit its
//! zoom, lift to paint and hit-test above it, and guard against self or cyclic anchors.
use super::*;
use crate::el::{Align, ScrollSpec, Side};
use crate::{Color, Cx, El, Env, col, el};

#[test]
fn overlay_mounted_before_anchor_resolves_paints_and_hits_above_later_siblings() {
    #[derive(Default)]
    struct S {
        panel_hits: usize,
        option_hits: usize,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        let pop = col().key("pop").w(120.0).h(60.0).fill(Color::WHITE)
            .anchored("trigger", Side::Bottom, Align::Start, 4.0)
            .child(el().key("opt").size(120.0, 60.0).fill(Color::WHITE).on_press(|s: &mut S| s.option_hits += 1));
        let panel = col().key("panel").absolute(80.0, 80.0).size(300.0, 220.0).fill(Color::BLACK)
            .on_press(|s: &mut S| s.panel_hits += 1)
            .child(el().key("trigger").absolute(20.0, 20.0).size(100.0, 30.0));
        // Mount the anchored overlay BEFORE the panel containing its anchor.
        col().size(600.0, 400.0).child(pop).child(panel)
    }

    let mut rt = Runtime::new(S::default(), view, (600.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Trigger sits at (100, 100, 100, 30); pop sits 4pt below at (100, 134, 120, 60), overlapping panel.
    assert_eq!(rt.presented("trigger").unwrap().0, [100.0, 100.0, 100.0, 30.0]);
    assert_eq!(rt.presented("pop").unwrap().0, [100.0, 134.0, 120.0, 60.0]);
    assert_eq!(rt.presented("opt").unwrap().0, [100.0, 134.0, 120.0, 60.0]);
    // Pop paints above the later panel and wins hit testing on the overlapping region.
    assert_eq!(rt.element_at(160.0, 160.0).and_then(|p| p.key().map(str::to_owned)), Some("opt".into()));
    rt.pointer_down(160.0, 160.0);
    rt.pointer_up(160.0, 160.0);
    rt.frame(0.0);
    assert_eq!(rt.state.option_hits, 1);
    assert_eq!(rt.state.panel_hits, 0);
    // motion_elements steps up from the forward-mounted overlay to its later anchor without panicking.
    let mut motion = Vec::new();
    rt.motion_elements(&mut motion);
    assert!(motion.iter().any(|m| m.rect == [100.0, 134.0, 120.0, 60.0]));
}

#[test]
fn overlay_mounted_before_anchor_fits_available_room() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let rows = col().key("rows").children((0..40).map(|i| el().key(format!("row{i}")).w(100.0).h(20.0)));
        let list = col().key("scroll").shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(rows);
        let menu = col().key("menu").w(100.0).child(list)
            .anchored_fit("target", Side::Bottom, Align::Start, 4.0);
        // Mount menu before its anchor near the bottom: above has 200 - 4 - 8 = 188pt, below 58pt.
        col().size(400.0, 300.0)
            .child(menu)
            .child(el().key("target").absolute(20.0, 200.0).size(100.0, 30.0))
    }

    let mut rt = Runtime::new((), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let r = rt.presented("menu").unwrap().0;
    assert_eq!(r[3], 187.0, "bounded to the larger room above: {r:?}");
    assert!(r[1] >= 8.0 && r[1] + r[3] <= 196.0, "flipped above the anchor: {r:?}");
}

#[test]
fn overlay_mounted_before_zoomed_anchor_inherits_zoom() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let pop = col().key("pop").size(100.0, 40.0)
            .anchored("target", Side::Bottom, Align::Start, 5.0);
        let page = col().key("page").size(800.0, 600.0).scale(1.5).origin(0.0, 0.0)
            .child(el().key("target").absolute(100.0, 100.0).size(80.0, 20.0));
        col().size(800.0, 600.0).child(pop).child(page)
    }

    let mut rt = Runtime::new((), view, (800.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.presented("target").unwrap().0, [150.0, 150.0, 120.0, 30.0]);
    assert_eq!(rt.presented("pop").unwrap().0, [150.0, 187.5, 150.0, 60.0]);
}

#[test]
fn chained_overlays_mounted_before_their_anchors_resolve_and_paint_in_order() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let outer = col().key("outer").size(80.0, 30.0).fill(Color::WHITE)
            .anchored("inner-anchor", Side::Right, Align::Start, 4.0);
        let inner = col().key("inner").size(100.0, 40.0).fill(Color::BLACK)
            .anchored("root-trigger", Side::Bottom, Align::Start, 4.0)
            .child(el().key("inner-anchor").size(100.0, 40.0));
        let trigger = el().key("root-trigger").absolute(50.0, 50.0).size(100.0, 20.0).fill(Color::WHITE);
        col().size(600.0, 400.0).child(outer).child(inner).child(trigger)
    }

    let mut rt = Runtime::new((), view, (600.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // root-trigger at (50, 50, 100, 20) -> inner at (50, 74, 100, 40) -> outer at (154, 74, 80, 30).
    assert_eq!(rt.presented("inner").unwrap().0, [50.0, 74.0, 100.0, 40.0]);
    assert_eq!(rt.presented("outer").unwrap().0, [154.0, 74.0, 80.0, 30.0]);
    let order = rt.paint.layer_paint_order();
    let idx = |k: &str| rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some(k)).unwrap();
    let pos = |k: &str| order.iter().position(|&n| n == idx(k)).unwrap();
    assert!(pos("root-trigger") < pos("inner"), "inner must paint after root-trigger");
    assert!(pos("inner") < pos("outer"), "outer must paint after inner");
}

#[test]
fn missing_self_and_cyclic_anchors_do_not_loop_and_missing_stays_suppressed() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let missing = col().key("missing").size(80.0, 30.0).fill(Color::WHITE)
            .anchored("does-not-exist", Side::Bottom, Align::Start, 4.0);
        let self_child = col().key("self-wrap").size(80.0, 30.0).fill(Color::WHITE)
            .anchored("inside-self", Side::Bottom, Align::Start, 4.0)
            .child(el().key("inside-self").size(20.0, 20.0));
        let cyc_a = col().key("cyc-a").size(40.0, 20.0)
            .anchored("cyc-b", Side::Bottom, Align::Start, 4.0);
        let cyc_b = col().key("cyc-b").size(40.0, 20.0)
            .anchored("cyc-a", Side::Bottom, Align::Start, 4.0);
        col().size(400.0, 300.0).child(missing).child(self_child).child(cyc_a).child(cyc_b)
    }

    let mut rt = Runtime::new((), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Unresolved and self-anchored overlays remain suppressed (never drawn at origin).
    assert_eq!(rt.presented("missing").unwrap().0, [0.0; 4]);
    assert_eq!(rt.presented("self-wrap").unwrap().0, [0.0; 4]);
    // Queries walking step_up terminate cleanly on cyclic anchors.
    let _ = rt.elements();
    let mut motion = Vec::new();
    rt.motion_elements(&mut motion);
    let _ = rt.presented_pose("cyc-a");
    let _ = rt.presented_pose("cyc-b");
}
