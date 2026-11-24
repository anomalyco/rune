use super::*;
use crate::{col, el};

struct State { custom: bool, growth: bool, rows: usize }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let rows = (0..s.rows).map(|i| {
        let row = el().key(format!("row-{i}")).w_full().h(30.0)
            .hover(Color::WHITE).on_press(|_: &mut State| {});
        let group = col().key(format!("group-{i}")).w_full().hover_group()
            .child(row)
            .child(el().key("action").size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0));
        if s.custom {
            // Explicit animations take the original, unconditional update path.
            group.hover_motion_within(Anim::INSTANT, rune_motion::Curve::tween(0.1, rune_motion::Easing::EASE_OUT))
        } else { group }
    });
    let stack = col().key("rows").w_full().children(rows);
    let stack = if s.growth { stack.growth(GrowthSpec::default()) } else { stack };
    col().w_full().h_full().child(
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false })
            .child(stack),
    )
}

#[test]
fn default_hover_matches_explicit_trajectories_through_scroll_and_reversal() {
    for (reduced, growth) in [(false, false), (true, false), (false, true), (true, true)] {
        let make = |custom| Runtime::new(State { custom, growth, rows: 30 }, view, (240.0, 150.0), Env::new(true, 2.0));
        let mut fast = make(false);
        let mut reference = make(true);
        for rt in [&mut fast, &mut reference] {
            rt.set_system_reduced_motion(reduced);
            rt.frame(0.0);
        }
        for frame in 0..120 {
            for rt in [&mut fast, &mut reference] {
                if growth && frame % 12 == 0 { rt.update(|s, _| s.rows += 1); }
                if frame == 0 { rt.pointer_move(60.0, 65.0); }
                if frame < 60 { rt.wheel(60.0, 65.0, if frame < 30 { -7.0 } else { 7.0 }); }
                if frame == 15 { rt.pointer_down(60.0, 65.0); }
                if frame == 18 { rt.pointer_up(60.0, 65.0); }
                if frame == 65 { rt.pointer_leave(); }
                if frame == 67 { rt.pointer_move(60.0, 65.0); }
                if frame == 90 { rt.pointer_leave(); }
                rt.frame(1.0 / 120.0);
            }
            assert_eq!(bytemuck::cast_slice::<_, u8>(&fast.instances), bytemuck::cast_slice::<_, u8>(&reference.instances),
                "same painted hover at frame {frame}, reduced={reduced}, growth={growth}");
        }
    }
}

#[test]
fn unchanged_default_hover_does_not_dirty_scene_poses() {
    for custom in [false, true] {
        let mut rt = Runtime::new(State { custom, growth: false, rows: 30 }, view, (240.0, 150.0), Env::new(true, 1.0));
        rt.frame(0.0);
        // Drain any requests made by the initial paint's interaction update.
        rt.scene.frame();
        rt.update_interaction(Some(false));
        assert_eq!(!rt.scene.frame().changed.is_empty(), custom,
            "only explicit animations may dirty poses for unchanged targets");
    }
}

#[test]
fn inherited_hover_curves_leave_descendants_visuals_shared() {
    let mut rt = Runtime::new(State { custom: true, growth: false, rows: 3 }, view, (240.0, 150.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.rows += 1);
    rt.frame(1.0 / 120.0);
    let rows: Vec<_> = rt.tree.nodes.iter().filter(|n| n.path.rsplit('/').next().is_some_and(|k| k.starts_with("row-"))).collect();
    assert_eq!(rows.len(), 4);
    for n in rows {
        assert!(n.hover_motion().is_some(), "{} goes by its group's curves", n.path);
        assert!(n.vis.cold.is_empty(), "{} keeps no cold visuals of its own", n.path);
    }
}

/// A face in a hoverable row: on the face only the face shows
/// hover, and the row's wash and its group styles rest; back on the row they light again.
/// The pointer moving between them only paints.
#[test]
fn a_hover_alone_child_keeps_the_row_around_it_unlit() {
    struct S;
    fn view(_: &S, _: &mut Cx<S>) -> El {
        let face = el().key("face").size(20.0, 20.0).hover(Color::WHITE).hover_alone();
        let row = crate::row().key("row").size(200.0, 30.0).hover(Color::WHITE)
            .child(el().key("label").size(100.0, 30.0))
            .child(el().key("dot").size(4.0, 4.0).fill(Color::WHITE).group_opacity(0.0, 1.0))
            .child(face);
        col().w_full().h_full().child(row)
    }
    let mut rt = Runtime::new(S, view, (240.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let id = |rt: &Runtime<S>, key: &str| rt.tree.nodes.iter().find(|n| n.path.rsplit('/').next() == Some(key)).map(|n| n.id).unwrap();
    let lit = |rt: &Runtime<S>, key: &str| rt.scene.store().target(id(rt, key), P_HOVER).unwrap_or(0.0);
    let commits = rt.stats.commits;

    rt.pointer_move(50.0, 15.0);
    rt.frame(1.0 / 120.0);
    assert_eq!((lit(&rt, "row"), lit(&rt, "face")), (1.0, 0.0), "on the label the row lights");

    rt.pointer_move(114.0, 10.0);
    rt.frame(1.0 / 120.0);
    assert_eq!((lit(&rt, "row"), lit(&rt, "face")), (0.0, 1.0), "on the face only the face lights");

    rt.pointer_move(50.0, 15.0);
    rt.frame(1.0 / 120.0);
    assert_eq!((lit(&rt, "row"), lit(&rt, "face")), (1.0, 0.0), "back on the label the row lights again");
    assert_eq!(rt.stats.commits, commits, "hover between them never commits");
}

#[test]
fn a_released_hold_holds_nothing_until_layout_settles() {
    let mut h = HoverHold::default();
    h.set_held(true);
    assert!(h.held());
    h.release(true);
    assert!(h.released() && !h.held(), "the hand let go of the hold while layout moves");
    h.set_held(true);
    assert!(h.released() && !h.held(), "released, a pass can't hold hover");
    h.release(true);
    assert!(h.released(), "a second move keeps it released");
    h.settled();
    h.set_held(true);
    assert!(h.held() && !h.released());
    h.release(false);
    assert!(!h.held() && !h.released(), "with layout still, a move just ends the hold");
}

#[test]
fn children_of_an_exiting_parent_stop_hovering_and_pressing() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        let mut root = col().key("root").size(200.0, 100.0);
        if *show {
            root = root.child(
                col().key("panel").size(120.0, 60.0).transition(crate::Transition::opacity())
                    .child(el().key("btn").size(80.0, 40.0).hover(Color::WHITE).hover_alone()
                        .hover_group_shared("shared-exit").on_press(|_: &mut bool| {})),
            );
        }
        root
    }
    let mut rt = Runtime::new(true, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|show, _| *show = false);
    rt.frame(1.0 / 60.0);

    // Move pointer onto the child of the exiting panel while the panel is mid-exit.
    let btn_idx = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("btn")).expect("child still in exiting subtree");
    assert!(rt.core().subtree_exiting(btn_idx) && !rt.core().is_exiting(rt.tree.nodes[btn_idx].id), "only parent holds Exiting phase");
    rt.pointer_move(20.0, 20.0);
    rt.frame(1.0 / 60.0);

    let btn_id = rt.tree.nodes[btn_idx].id;
    let target = rt.scene.store().target(btn_id, P_HOVER).unwrap_or(0.0);
    assert_eq!(target, 0.0, "child of exiting parent must not light hover");
    assert!(rt.core().hovered_alone().is_none(), "child of exiting parent must not claim hover_alone");
    assert!(rt.core().shared_hovered().is_empty(), "child of exiting parent must not light hover_group_shared");
}

#[test]
fn overhanging_child_keeps_its_hover_group_lit_and_blocks_row_underneath() {
    #[derive(Default)]
    struct S {
        hit_only: bool,
        changes: Vec<(usize, bool)>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let row = |i: usize| {
            let mut bar = el().key(format!("bar-{i}")).size(40.0, 20.0)
                .absolute(150.0, -10.0) // overhangs the row above by 10 pt
                .fill(Color::WHITE)
                .hover(Color::WHITE.alpha(0.1))
                .group_opacity(0.0, 1.0)
                .child(el().key(format!("btn-{i}")).size(40.0, 20.0).on_press(|_: &mut S| {}));
            bar = if s.hit_only { bar.hit() } else { bar.on_press(|_: &mut S| {}) };
            let detached = el().key(format!("detached-{i}")).size(40.0, 14.0)
                .absolute(50.0, -24.0) // detached 10 pt above the row
                .fill(Color::WHITE).hit()
                .group_opacity(0.0, 1.0);
            col().key(format!("row-{i}")).w_full().h(40.0).hover_group()
                .hover_motion(Anim::INSTANT, Anim::INSTANT)
                .on_hover_change(move |s: &mut S, inside, _| s.changes.push((i, inside)))
                .child(bar)
                .child(detached)
        };
        col().w_full().h_full().child(row(0)).child(row(1))
    }
    let lit = |rt: &Runtime<S>, key: &str| {
        let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().id;
        rt.scene.store().target(id, P_HOVER).unwrap_or(0.0)
    };
    let alpha = |rt: &Runtime<S>, key: &str| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().opacity
    };

    for hit_only in [false, true] {
        let mut rt = Runtime::new(S { hit_only, changes: Vec::new() }, view, (300.0, 120.0), Env::new(true, 1.0));
        rt.frame(0.0);

        // Before row-1 is hovered, its invisible bar must not steal hover from row-0.
        rt.pointer_move(170.0, 35.0);
        rt.frame(1.0 / 120.0);
        assert_eq!((lit(&rt, "row-0"), lit(&rt, "row-1")), (1.0, 0.0), "hit_only={hit_only}: unseen overhang leaves row-0 lit");
        assert_eq!(alpha(&rt, "bar-1"), 0.0);

        // Move onto row-1: row-1 lights and bar-1 becomes visible.
        rt.pointer_move(170.0, 60.0);
        rt.frame(1.0 / 120.0);
        rt.frame(1.0 / 120.0);
        assert_eq!((lit(&rt, "row-0"), lit(&rt, "row-1")), (0.0, 1.0), "hit_only={hit_only}: row-1 lights");
        assert_eq!(alpha(&rt, "bar-1"), 1.0, "hit_only={hit_only}: bar-1 shows");
        rt.state.changes.clear();

        // Move onto bar-1's upper half overhanging row-0: row-1 stays lit and row-0 stays dark.
        rt.pointer_move(170.0, 35.0);
        rt.frame(1.0 / 120.0);
        assert_eq!((lit(&rt, "row-0"), lit(&rt, "row-1")), (0.0, 1.0), "hit_only={hit_only}: overhanging child keeps row-1 lit and blocks row-0");
        assert_eq!(alpha(&rt, "bar-1"), 1.0, "hit_only={hit_only}: bar-1 stays visible");
        assert!(rt.state.changes.is_empty(), "hit_only={hit_only}: on_hover_change stays inside row-1: {:?}", rt.state.changes);

        // Move onto the detached child above row-1: row-1 stays lit and row-0 stays dark.
        rt.pointer_move(70.0, 22.0);
        rt.frame(1.0 / 120.0);
        assert_eq!((lit(&rt, "row-0"), lit(&rt, "row-1")), (0.0, 1.0), "hit_only={hit_only}: detached overhanging child keeps row-1 lit");

        // Move into the gap between row-1 and its detached child (over row-0): row-0 lights and row-1 goes dark.
        rt.pointer_move(70.0, 35.0);
        rt.frame(1.0 / 120.0);
        assert_eq!((lit(&rt, "row-0"), lit(&rt, "row-1")), (1.0, 0.0), "hit_only={hit_only}: gap hands hover to row-0");
    }
}

#[test]
fn covering_interactive_sibling_blocks_hover_while_interactive_ancestor_allows_hover_child() {
    struct S;
    fn view(_: &S, _: &mut Cx<S>) -> El {
        let root = col().key("root").size(300.0, 120.0).on_press(|_: &mut S| {})
            .child(
                col().key("row-0").size(300.0, 40.0).hover(Color::WHITE.alpha(0.1)).hover_group_shared("g0")
                    .hover_motion(Anim::INSTANT, Anim::INSTANT)
                    .child(el().key("inner-hover").size(60.0, 40.0).hover(Color::WHITE.alpha(0.2))
                        .hover_motion(Anim::INSTANT, Anim::INSTANT)),
            )
            .child(
                col().key("row-1").size(300.0, 40.0).hover_group_shared("g1")
                    .hover_motion(Anim::INSTANT, Anim::INSTANT)
                    .child(el().key("overhang").size(40.0, 20.0).absolute(150.0, -10.0).hit()),
            )
            .child(el().key("peer-0").size(100.0, 20.0).hover_group_shared("g0").hover_motion(Anim::INSTANT, Anim::INSTANT))
            .child(el().key("peer-1").size(100.0, 20.0).hover_group_shared("g1").hover_motion(Anim::INSTANT, Anim::INSTANT))
            .child(el().key("cover").absolute(80.0, 0.0).size(40.0, 40.0).hit());
        root
    }
    let mut rt = Runtime::new(S, view, (300.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let lit = |rt: &Runtime<S>, key: &str| {
        let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().id;
        rt.scene.store().target(id, P_HOVER).unwrap_or(0.0)
    };

    // Over inner-hover inside clickable root: top_hit is root (an ancestor), so inner-hover and row-0 both light.
    rt.pointer_move(20.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert_eq!((lit(&rt, "row-0"), lit(&rt, "inner-hover"), lit(&rt, "peer-0")), (1.0, 1.0, 1.0));

    // Over pinned interactive cover on top of row-0: row-0 and its shared peer go dark.
    rt.pointer_move(100.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert_eq!((lit(&rt, "row-0"), lit(&rt, "peer-0")), (0.0, 0.0), "interactive cover blocks row-0 and its shared peer");

    // Over row-1's overhanging child (over row-0): row-1 and peer-1 light; row-0 and peer-0 stay dark.
    rt.pointer_move(170.0, 35.0);
    rt.frame(1.0 / 120.0);
    assert_eq!((lit(&rt, "row-0"), lit(&rt, "peer-0"), lit(&rt, "row-1"), lit(&rt, "peer-1")), (0.0, 0.0, 1.0, 1.0));
}

#[test]
fn hover_passes_below_a_layer_that_asks_for_it_while_the_layer_keeps_its_press() {
    #[derive(Default)]
    struct S {
        passes: bool,
        pressed: u32,
        changes: Vec<bool>,
    }
    // A tab that shows its close button on hover, a window-sized dismiss layer over it, and a
    // menu over part of the tab: a context menu open on the tab.
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let tab = col().key("tab").size(100.0, 30.0).hover_group().hover(Color::WHITE.alpha(0.1))
            .hover_motion(Anim::INSTANT, Anim::INSTANT)
            .on_hover_change(|s: &mut S, inside, _| s.changes.push(inside))
            .on_press(|_: &mut S| {})
            .child(el().key("close").size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0));
        let layer = el().key("layer").absolute(0.0, 0.0).w_full().h_full().on_press(|s: &mut S| s.pressed += 1);
        let menu = el().key("menu").absolute(60.0, 10.0).size(60.0, 60.0).fill(Color::WHITE).on_press(|_: &mut S| {});
        col().w_full().h_full()
            .child(tab)
            .child(if s.passes { layer.hover_passes_below() } else { layer })
            .child(menu)
    }
    let lit = |rt: &Runtime<S>, key: &str| {
        let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().id;
        rt.scene.store().target(id, P_HOVER).unwrap_or(0.0)
    };
    let alpha = |rt: &Runtime<S>, key: &str| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().opacity
    };

    for passes in [false, true] {
        let mut rt = Runtime::new(S { passes, ..S::default() }, view, (300.0, 120.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let want = if passes { 1.0 } else { 0.0 };

        // Over the tab, under the layer alone: the tab hovers only when the layer lets hover through.
        rt.pointer_move(20.0, 15.0);
        rt.frame(1.0 / 120.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(lit(&rt, "tab"), want, "passes={passes}: the tab under the layer");
        assert_eq!(alpha(&rt, "close"), want, "passes={passes}: the tab's hover-only close button");
        assert_eq!(rt.state.changes, if passes { vec![true] } else { vec![] }, "passes={passes}: on_hover_change");

        // Over the menu, which lies over the tab: the menu is the topmost hit either way.
        rt.pointer_move(80.0, 20.0);
        rt.frame(1.0 / 120.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(lit(&rt, "tab"), 0.0, "passes={passes}: the menu over the tab blocks its hover");

        // The layer still takes the press that dismisses the menu.
        rt.pointer_move(20.0, 15.0);
        rt.frame(1.0 / 120.0);
        rt.pointer_down(20.0, 15.0);
        rt.pointer_up(20.0, 15.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.pressed, 1, "passes={passes}: the layer's press");
    }
}