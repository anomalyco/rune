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

#[test]
fn press_motion_drives_press_and_press_scale_tracks_with_defaults_and_reduced_motion() {
    #[derive(Clone, Copy)]
    enum Mode { Default, Instant, Timed, Inert }

    fn view(mode: &Mode, _: &mut Cx<Mode>) -> El {
        let mut btn = el().key("btn").size(100.0, 40.0)
            .press(Color::WHITE.alpha(0.2))
            .press_scale(0.9)
            .on_press(|_: &mut Mode| {});
        btn = match *mode {
            Mode::Default => btn,
            Mode::Instant => btn.press_motion(Anim::INSTANT, Anim::INSTANT),
            Mode::Timed => btn.press_motion(
                rune_motion::Curve::linear(0.2),
                rune_motion::Curve::linear(0.4),
            ),
            Mode::Inert => btn
                .press_motion(rune_motion::Curve::linear(0.2), rune_motion::Curve::linear(0.4))
                .inert(),
        };
        col().size(200.0, 100.0).child(btn)
    }

    let id = |rt: &Runtime<Mode>| rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("btn")).unwrap().id;
    let press_v = |rt: &Runtime<Mode>| rt.scene.get(id(rt), P_PRESS).unwrap_or(0.0);
    let scale_v = |rt: &Runtime<Mode>| rt.scene.get(id(rt), P_PRESS_SCALE).unwrap_or(1.0);

    // Default: P_PRESS and P_PRESS_SCALE use tok::motion::INSTANT (0.1 s spring) down, then spring up (SNAPPY / POP).
    let mut def = Runtime::new(Mode::Default, view, (200.0, 100.0), Env::new(true, 1.0));
    def.frame(0.0);
    def.pointer_move(50.0, 20.0);
    def.pointer_down(50.0, 20.0);
    def.frame(1.0 / 120.0);
    assert!(press_v(&def) > 0.0 && press_v(&def) < 1.0, "default P_PRESS down springs on tok::motion::INSTANT, got {}", press_v(&def));
    assert!(scale_v(&def) > 0.9 && scale_v(&def) < 1.0, "default P_PRESS_SCALE down springs on tok::motion::INSTANT, got {}", scale_v(&def));
    def.frame(0.15);
    assert!(press_v(&def) > 0.95, "default P_PRESS settles near 1.0");
    assert!((scale_v(&def) - 0.9).abs() < 0.01, "default P_PRESS_SCALE settles near 0.9");
    def.pointer_up(50.0, 20.0);
    def.frame(1.0 / 120.0);
    assert!(press_v(&def) > 0.1 && press_v(&def) < 1.0, "default P_PRESS release eases on SNAPPY");
    assert!(scale_v(&def) > 0.9 && scale_v(&def) < 1.0, "default P_PRESS_SCALE release springs on POP");

    // Instant: both down and release snap immediately on both tracks.
    let mut inst = Runtime::new(Mode::Instant, view, (200.0, 100.0), Env::new(true, 1.0));
    inst.frame(0.0);
    inst.pointer_move(50.0, 20.0);
    inst.pointer_down(50.0, 20.0);
    inst.frame(1.0 / 120.0);
    assert!((press_v(&inst) - 1.0).abs() < 1e-4);
    assert!((scale_v(&inst) - 0.9).abs() < 1e-4);
    inst.pointer_up(50.0, 20.0);
    inst.frame(1.0 / 120.0);
    assert!(press_v(&inst) < 1e-4, "instant press_motion snaps P_PRESS release");
    assert!((scale_v(&inst) - 1.0).abs() < 1e-4, "instant press_motion snaps P_PRESS_SCALE release");

    // Timed: down interpolates over 0.2 s linear, release interpolates over 0.4 s linear.
    let mut timed = Runtime::new(Mode::Timed, view, (200.0, 100.0), Env::new(true, 1.0));
    timed.frame(0.0);
    timed.pointer_move(50.0, 20.0);
    timed.pointer_down(50.0, 20.0);
    timed.frame(0.0);
    timed.frame(0.1);
    assert!((press_v(&timed) - 0.5).abs() < 0.08, "P_PRESS down midpoint ~0.5, got {}", press_v(&timed));
    assert!((scale_v(&timed) - 0.95).abs() < 0.01, "P_PRESS_SCALE down midpoint ~0.95, got {}", scale_v(&timed));
    timed.frame(0.15);
    assert!((press_v(&timed) - 1.0).abs() < 1e-4);
    assert!((scale_v(&timed) - 0.9).abs() < 1e-4);

    timed.pointer_up(50.0, 20.0);
    timed.frame(0.0);
    timed.frame(0.2);
    assert!((press_v(&timed) - 0.5).abs() < 0.08, "P_PRESS up midpoint ~0.5, got {}", press_v(&timed));
    assert!((scale_v(&timed) - 0.95).abs() < 0.01, "P_PRESS_SCALE up midpoint ~0.95, got {}", scale_v(&timed));
    timed.frame(0.25);
    assert!(press_v(&timed) < 1e-4);
    assert!((scale_v(&timed) - 1.0).abs() < 1e-4);

    // Reduce Motion: press_scale stays off (1.0), instant P_PRESS stays instant, long timed P_PRESS reduces to ~0.18 s fade.
    let mut rm_inst = Runtime::new(Mode::Instant, view, (200.0, 100.0), Env::new(true, 1.0));
    rm_inst.set_system_reduced_motion(true);
    rm_inst.frame(0.0);
    rm_inst.pointer_move(50.0, 20.0);
    rm_inst.pointer_down(50.0, 20.0);
    rm_inst.frame(1.0 / 120.0);
    assert!((press_v(&rm_inst) - 1.0).abs() < 1e-4);
    assert!((scale_v(&rm_inst) - 1.0).abs() < 1e-4, "press_scale is disabled under Reduce Motion");
    rm_inst.pointer_up(50.0, 20.0);
    rm_inst.frame(1.0 / 120.0);
    assert!(press_v(&rm_inst) < 1e-4, "Anim::INSTANT P_PRESS stays instant under Reduce Motion");

    let mut rm_timed = Runtime::new(Mode::Timed, view, (200.0, 100.0), Env::new(true, 1.0));
    rm_timed.set_system_reduced_motion(true);
    rm_timed.frame(0.0);
    rm_timed.pointer_move(50.0, 20.0);
    rm_timed.pointer_down(50.0, 20.0);
    rm_timed.frame(0.0);
    rm_timed.frame(0.25);
    assert!((press_v(&rm_timed) - 1.0).abs() < 1e-4);
    assert!((scale_v(&rm_timed) - 1.0).abs() < 1e-4, "press_scale stays off under Reduce Motion");
    rm_timed.pointer_up(50.0, 20.0);
    rm_timed.frame(0.0);
    rm_timed.frame(0.22);
    assert!(press_v(&rm_timed) < 1e-4, "0.4 s timed P_PRESS release reduced to 0.18 s fade settles by 0.22 s");

    // Inert element with press_motion does not animate on press.
    let mut inert = Runtime::new(Mode::Inert, view, (200.0, 100.0), Env::new(true, 1.0));
    inert.frame(0.0);
    inert.pointer_move(50.0, 20.0);
    inert.pointer_down(50.0, 20.0);
    inert.frame(0.1);
    assert_eq!(press_v(&inert), 0.0);
    assert_eq!(scale_v(&inert), 1.0);
}

#[test]
fn press_motion_change_on_memo_root_is_not_reused_as_stale() {
    fn view(instant: &bool, _: &mut Cx<bool>) -> El {
        let base = crate::memo("btn", 0u32, || {
            el().size(100.0, 40.0).press(Color::WHITE.alpha(0.2)).on_press(|_: &mut bool| {})
        });
        let btn = if *instant {
            base.press_motion(Anim::INSTANT, Anim::INSTANT)
        } else {
            base.press_motion(Anim::INSTANT, rune_motion::Curve::linear(0.4))
        };
        col().size(200.0, 100.0).child(btn)
    }

    let mut rt = Runtime::new(false, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|inst, _| *inst = true);
    rt.frame(0.0);
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("btn")).unwrap().id;
    rt.pointer_move(50.0, 20.0);
    rt.pointer_down(50.0, 20.0);
    rt.frame(1.0 / 120.0);
    rt.pointer_up(50.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.scene.get(id, P_PRESS).unwrap_or(0.0) < 1e-4, "updated press_motion took effect on memo root");
}

#[test]
fn hit_only_cover_blocks_on_hover_and_focus_highlight_until_removed_or_pass_through() {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Cover { Off, HitOnly, PassThrough, DismissWithMenu, PanelWithChild }
    struct S {
        cover: Cover,
        hovers: Vec<&'static str>,
        dismiss_presses: u32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let row0 = crate::row().key("row-0").size(200.0, 40.0)
            .hover_fill(Color::WHITE.alpha(0.2))
            .hover_motion(Anim::INSTANT, Anim::INSTANT)
            .focusable(true).label("row-0")
            .focus_highlight(Color::WHITE.alpha(0.3))
            .on_hover(|s: &mut S| s.hovers.push("row-0"))
            .on_press(|_: &mut S| {})
            .child(el().key("badge").absolute(140.0, 5.0).size(30.0, 30.0).hit());
        let row1 = crate::row().key("row-1").size(200.0, 40.0)
            .hover_fill(Color::WHITE.alpha(0.2))
            .hover_motion(Anim::INSTANT, Anim::INSTANT)
            .focusable(true).label("row-1")
            .focus_highlight(Color::WHITE.alpha(0.3))
            .on_hover(|s: &mut S| s.hovers.push("row-1"))
            .on_press(|_: &mut S| {});
        let mut root = col().key("root").size(240.0, 120.0)
            .child(row0)
            .child(row1);
        match s.cover {
            Cover::Off => {}
            Cover::HitOnly => {
                root = root.child(el().key("cover").absolute(0.0, 0.0).size(120.0, 40.0).hit());
            }
            Cover::PassThrough => {
                root = root.child(el().key("cover").absolute(0.0, 0.0).size(120.0, 40.0).hit().hover_passes_below());
            }
            Cover::DismissWithMenu => {
                root = root
                    .child(el().key("scrim").absolute(0.0, 0.0).size(240.0, 120.0)
                        .hover_passes_below()
                        .on_press(|s: &mut S| s.dismiss_presses += 1))
                    .child(el().key("menu").absolute(60.0, 0.0).size(60.0, 40.0).hit());
            }
            Cover::PanelWithChild => {
                root = root.child(
                    col().key("panel").absolute(0.0, 0.0).size(120.0, 40.0).hit()
                        .child(el().key("panel-item").absolute(60.0, 0.0).size(60.0, 40.0)
                            .focusable(true).label("panel-item").focus_highlight(Color::WHITE.alpha(0.3))
                            .on_hover(|s: &mut S| s.hovers.push("panel-item"))
                            .on_press(|_: &mut S| {})),
                );
            }
        }
        root
    }
    let lit = |rt: &Runtime<S>, key: &str| {
        let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().id;
        rt.scene.store().target(id, P_HOVER).unwrap_or(0.0)
    };
    let focus_key = |rt: &Runtime<S>| {
        rt.kbd.focus.path().and_then(|p| p.rsplit('/').next()).map(str::to_owned)
    };

    let mut rt = Runtime::new(S { cover: Cover::Off, hovers: Vec::new(), dismiss_presses: 0 }, view, (240.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Over row-0 with no cover: on_hover, focus_highlight, and hover_fill all activate.
    rt.pointer_move(30.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.hovers, vec!["row-0"]);
    assert_eq!(focus_key(&rt).as_deref(), Some("row-0"));
    assert!(rt.kbd.focus.hovered());
    assert_eq!(lit(&rt, "row-0"), 1.0);

    // A .hit()-only child inside row-0 keeps row-0 hovered.
    rt.pointer_move(155.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(focus_key(&rt).as_deref(), Some("row-0"));
    assert_eq!(lit(&rt, "row-0"), 1.0);

    // Open a .hit()-only overlying sibling covering (0..120, 0..40): row-0's hover & focus_highlight clear.
    rt.pointer_move(30.0, 20.0);
    rt.frame(1.0 / 120.0);
    rt.state.hovers.clear();
    rt.update(|s, _| s.cover = Cover::HitOnly);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.state.hovers.is_empty(), "blocked row must not re-fire on_hover: {:?}", rt.state.hovers);
    assert!(!rt.kbd.focus.hovered(), "hit-only cover clears focus_highlight on row underneath");
    assert_eq!(lit(&rt, "row-0"), 0.0, "hit-only cover blocks hover_fill underneath");

    // Moving inside the cover never leaks on_hover or focus_highlight to row-0.
    rt.pointer_move(80.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.state.hovers.is_empty());
    assert!(!rt.kbd.focus.hovered());
    assert_eq!(lit(&rt, "row-0"), 0.0);

    // Uncovered row-1 still hovers normally.
    rt.pointer_move(30.0, 60.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.hovers, vec!["row-1"]);
    assert_eq!(focus_key(&rt).as_deref(), Some("row-1"));
    assert!(rt.kbd.focus.hovered());

    // Moving back onto the cover over row-0 drops row-1 and does not hover row-0.
    rt.state.hovers.clear();
    rt.pointer_move(30.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.state.hovers.is_empty());
    assert!(!rt.kbd.focus.hovered());

    // Removing the cover under a still pointer restores row-0's on_hover, focus_highlight, and hover_fill.
    rt.update(|s, _| s.cover = Cover::Off);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.hovers, vec!["row-0"]);
    assert_eq!(focus_key(&rt).as_deref(), Some("row-0"));
    assert!(rt.kbd.focus.hovered());
    assert_eq!(lit(&rt, "row-0"), 1.0);

    // Marking the .hit() cover with hover_passes_below restores hover through it.
    rt.update(|s, _| s.cover = Cover::HitOnly);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert!(!rt.kbd.focus.hovered());
    rt.state.hovers.clear();
    rt.update(|s, _| s.cover = Cover::PassThrough);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.hovers, vec!["row-0"]);
    assert_eq!(focus_key(&rt).as_deref(), Some("row-0"));
    assert!(rt.kbd.focus.hovered());
    assert_eq!(lit(&rt, "row-0"), 1.0);

    // A pressable dismiss scrim with hover_passes_below passes on_hover & focus_highlight through,
    // while still taking its press and letting a .hit() menu above it block row-0.
    rt.update(|s, _| s.cover = Cover::DismissWithMenu);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(focus_key(&rt).as_deref(), Some("row-0"));
    assert!(rt.kbd.focus.hovered());
    assert_eq!(lit(&rt, "row-0"), 1.0);
    rt.pointer_down(30.0, 20.0);
    rt.pointer_up(30.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.dismiss_presses, 1, "dismiss scrim still takes presses");
    rt.state.hovers.clear();
    rt.pointer_move(80.0, 20.0); // over the .hit() menu above the scrim
    rt.frame(1.0 / 120.0);
    assert!(rt.state.hovers.is_empty());
    assert!(!rt.kbd.focus.hovered(), "menu above scrim blocks focus_highlight underneath");
    assert_eq!(lit(&rt, "row-0"), 0.0);

    // A .hit() panel blocks row-0 in its padding, while its own item still gets on_hover and focus_highlight.
    rt.pointer_move(20.0, 20.0); // panel padding over row-0
    rt.update(|s, _| s.cover = Cover::PanelWithChild);
    rt.frame(1.0 / 120.0);
    rt.state.hovers.clear();
    rt.pointer_move(25.0, 20.0); // still in panel padding over row-0
    rt.frame(1.0 / 120.0);
    assert!(rt.state.hovers.is_empty());
    assert!(!rt.kbd.focus.hovered());
    rt.pointer_move(80.0, 20.0); // panel-item inside panel
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.hovers, vec!["panel-item"]);
    assert_eq!(focus_key(&rt).as_deref(), Some("panel-item"));
    assert!(rt.kbd.focus.hovered());
}

#[test]
fn hover_and_press_replacements_drive_tracks_groups_motion_and_memo_without_commits() {
    struct S { variant: u8, inert: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let c_h = if s.variant == 0 { Color::WHITE } else { Color::BLACK };
        let base = crate::memo("btn", 0u32, || {
            el().size(100.0, 40.0)
                .fill(Color::BLACK)
                .border(1.0, Color::BLACK)
                .press_motion(rune_motion::Curve::linear(0.2), rune_motion::Curve::linear(0.4))
                .on_press_tx(|_: &mut S, tx| tx.live_only())
                .child(el().key("dot").size(8.0, 8.0).fill(Color::WHITE).group_opacity(0.0, 1.0))
        })
        .hover_fill(c_h)
        .press_fill(c_h.alpha(0.5))
        .hover_border(c_h)
        .press_border(c_h.alpha(0.5));
        let btn = if s.inert { base.inert() } else { base };
        col().size(200.0, 100.0).child(btn)
    }
    let mut rt = Runtime::new(S { variant: 0, inert: false }, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.check_live_only(false);
    rt.frame(0.0);
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("btn")).unwrap().id;
    let commits = rt.stats.commits;

    // Hover activates P_HOVER and descendant group_opacity with zero commits.
    rt.pointer_move(50.0, 20.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.scene.get(id, P_HOVER), Some(1.0));
    let dot_op = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("dot")).unwrap().opacity;
    assert_eq!(dot_op, 1.0, "hover_fill/hover_border element acts as hover group owner");

    // Press uses press_motion (0.2s down, 0.4s up) with zero commits.
    rt.pointer_down(50.0, 20.0);
    rt.frame(0.0);
    rt.frame(0.1);
    let mid = rt.scene.get(id, P_PRESS).unwrap_or(0.0);
    assert!((mid - 0.5).abs() < 0.08, "press_fill/press_border follow press_motion down midpoint, got {mid}");
    rt.frame(0.15);
    assert!((rt.scene.get(id, P_PRESS).unwrap_or(0.0) - 1.0).abs() < 1e-4);
    rt.pointer_up(50.0, 20.0);
    rt.frame(0.0);
    rt.frame(0.2);
    let up_mid = rt.scene.get(id, P_PRESS).unwrap_or(0.0);
    assert!((up_mid - 0.5).abs() < 0.08, "press_fill/press_border follow press_motion up midpoint, got {up_mid}");
    rt.frame(0.25);
    assert!(rt.scene.get(id, P_PRESS).unwrap_or(0.0) < 1e-4);
    assert_eq!(rt.stats.commits, commits, "hover/press replacement colours never commit");

    // Memo equality: changing replacement colours on a memo root updates the node's visual fields.
    rt.update(|s, _| s.variant = 1);
    rt.frame(0.0);
    let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("btn")).unwrap();
    assert_eq!(n.vis.hover_fill, Some(Color::BLACK));
    assert_eq!(n.vis.press_fill, Some(Color::BLACK.alpha(0.5)));
    assert_eq!(n.vis.hover_border, Some(Color::BLACK));
    assert_eq!(n.vis.press_border, Some(Color::BLACK.alpha(0.5)));

    // Inert element does not track hover or press replacements.
    rt.update(|s, _| s.inert = true);
    rt.frame(1.0 / 120.0);
    rt.pointer_down(50.0, 20.0);
    rt.frame(0.2);
    assert_eq!(rt.scene.store().target(id, P_HOVER).unwrap_or(0.0), 0.0);
    assert_eq!(rt.scene.store().target(id, P_PRESS).unwrap_or(0.0), 0.0);
}