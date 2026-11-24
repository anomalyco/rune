//! `Runtime::presented_pose` and the moving flag cover shadow and rotation springs;
//! Picking shares press geometry, but also finds elements without handlers.
use rune::{Color, Cx, El, Env, Runtime, Shadow, Spring, col, el};
use std::f32::consts::FRAC_PI_4;

fn moving<T>(rt: &Runtime<T>, key: &str) -> bool {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().moving
}

fn settle<T>(rt: &mut Runtime<T>) {
    for _ in 0..120 {
        rt.frame(1.0 / 60.0);
    }
}

fn sh(y: f32) -> Shadow {
    Shadow { x: 0.0, y, blur: 8.0, spread: 0.0, color: Color::BLACK.alpha(0.3), inset: false }
}

fn card(on: &bool, _: &mut Cx<bool>) -> El {
    let shadow = sh(if *on { 12.0 } else { 2.0 });
    col().w_full().h_full().items_start().padding(50.0).child(
        el().key("a").size(100.0, 40.0).fill(Color::WHITE).motion(Spring::new(0.3, 0.0)).box_shadow(shadow),
    )
}

#[test]
fn a_shadow_only_step_reports_moving_until_it_lands() {
    let mut rt = Runtime::new(false, card, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!moving(&rt, "a"));
    assert_eq!(rt.presented_pose("a").unwrap().shadows()[0].y, 2.0);
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert!(moving(&rt, "a"), "the shadow's y is springing");
    let pose = rt.presented_pose("a").unwrap();
    assert!(pose.moving());
    let y = pose.shadows()[0].y;
    assert!(y > 2.0 && y < 12.0, "mid-spring y: {y}");
    settle(&mut rt);
    assert!(!moving(&rt, "a"));
    let pose = rt.presented_pose("a").unwrap();
    assert!(!pose.moving());
    assert_eq!(pose.shadows()[0].y, 12.0);
}

#[test]
fn a_rotation_only_step_reports_moving_until_it_lands() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        col().w_full().h_full().items_start().padding(50.0).child(
            el().key("a").size(100.0, 40.0).fill(Color::WHITE).motion(Spring::new(0.3, 0.0))
                .rotate(if *on { 1.0 } else { 0.0 }),
        )
    }
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!moving(&rt, "a"));
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert!(moving(&rt, "a"));
    let r = rt.presented_pose("a").unwrap().rotation();
    assert!(r > 0.0 && r < 1.0, "mid-spring rotation: {r}");
    settle(&mut rt);
    assert!(!moving(&rt, "a"));
    let pose = rt.presented_pose("a").unwrap();
    assert!((pose.rotation() - 1.0).abs() < 1e-3);
    assert!(!pose.moving());
}

#[derive(Default)]
struct Hits(Vec<&'static str>);

fn bar(_: &Hits, _: &mut Cx<Hits>) -> El {
    col().w_full().h_full().items_start().padding(50.0)
        .on_press(|s: &mut Hits| s.0.push("bg"))
        .child(
            // 100x20 about (100, 60), turned 45°.
            el().key("bar").size(100.0, 20.0).fill(Color::WHITE).rotate(FRAC_PI_4)
                .on_press(|s: &mut Hits| s.0.push("bar"))
                .child(el().key("inner").size(10.0, 10.0)),
        )
}

fn key_at(rt: &Runtime<Hits>, x: f32, y: f32) -> Option<String> {
    rt.element_at(x, y).and_then(|p| p.key().map(str::to_owned))
}

#[test]
fn element_at_follows_the_turned_box_and_agrees_with_presses() {
    let mut rt = Runtime::new(Hits::default(), bar, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // In the upright layout box [50..150, 50..70]; off the 45° diagonal.
    assert_ne!(key_at(&rt, 60.0, 52.0).as_deref(), Some("bar"));
    assert!(!rt.contains("bar", 60.0, 52.0));
    // On the diagonal, outside the upright box.
    assert_eq!(key_at(&rt, 120.0, 80.0).as_deref(), Some("bar"));
    assert!(rt.contains("bar", 120.0, 80.0));
    // In the upright bounding box, outside the turned rect.
    assert!(!rt.contains("bar", 130.0, 30.0));
    assert!(rt.element_at(130.0, 30.0).unwrap().key().is_none(), "the background column");
    // A child with no handlers is found.
    assert_eq!(key_at(&rt, 71.7, 24.6).as_deref(), Some("inner"));
    assert_eq!(rt.element_at(500.0, 500.0), None);

    for (x, y) in [(60.0, 52.0), (120.0, 80.0), (130.0, 30.0), (80.0, 40.0), (140.0, 90.0), (71.7, 24.6)] {
        let picked_bar = matches!(key_at(&rt, x, y).as_deref(), Some("bar" | "inner"));
        rt.state.0.clear();
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.0 == ["bar"], picked_bar, "({x}, {y}): press {:?}", rt.state.0);
    }
}

#[test]
fn a_canvas_handler_picks_its_last_presented_children_before_rebuilding() {
    #[derive(Default)]
    struct Canvas { point: (f32, f32), picked: Option<String>, hide: bool }
    fn view(s: &Canvas, _: &mut Cx<Canvas>) -> El {
        el().w_full().h_full()
            .child(col().key("canvas").w_full().h_full().items_start().padding(50.0)
                .on_pointer_move_local(|s: &mut Canvas, p, _| s.point = (p.window_x, p.window_y))
                .on_press_tx(|s: &mut Canvas, tx| {
                    s.hide = true;
                    tx.element_at_in("canvas", s.point.0, s.point.1, |s: &mut Canvas, p, tx| {
                        s.picked = p.and_then(|p| p.key().map(str::to_owned));
                        tx.instant();
                    });
                })
                .children((!s.hide).then(|| el().key("bar").size(100.0, 20.0).rotate(FRAC_PI_4))))
            // This paints last, but has no input handlers. Scoped picking ignores it.
            .child(el().key("overlay").w_full().h_full().absolute(0.0, 0.0))
    }
    let mut rt = Runtime::new(Canvas::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.element_at(120.0, 80.0).unwrap().key(), Some("overlay"));
    assert!(rt.element_at_in("absent", 120.0, 80.0).is_none());
    rt.pointer_move(120.0, 80.0);
    rt.frame(0.0);
    rt.pointer_down(120.0, 80.0);
    rt.pointer_up(120.0, 80.0);
    rt.frame(0.0);
    assert_eq!(rt.state.picked.as_deref(), Some("bar"));
    assert!(!rt.contains("canvas/bar", 120.0, 80.0));
}

#[test]
fn picking_uses_raise_cache_scale_origin_offset_and_ancestor_clip() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().w_full().h_full()
            .child(col().key("clip").size(100.0, 100.0).items_start().clip_rounded(0.0)
                .child(el().key("raised").size(80.0, 40.0).scale(2.0).origin(0.0, 0.0)
                    .offset(20.0, 20.0).raise().cache()))
            .child(el().key("later").size(100.0, 100.0).absolute(0.0, 0.0))
            .child(el().key("hidden").w_full().h_full().opacity(0.0).absolute(0.0, 0.0))
    }
    let mut rt = Runtime::new((), view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(rt.element_at(90.0, 50.0).unwrap().key(), Some("raised"));
    assert!(!rt.contains("raised", 110.0, 50.0), "ancestor clip cuts the scaled child");
    assert!(!rt.contains("hidden", 50.0, 50.0));
    assert!(rt.contains("raised", 30.0, 30.0), "no fill is still pickable geometry");
}

#[test]
fn a_shadow_colour_only_step_settles() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        el().key("a").size(40.0, 40.0).motion(Spring::new(0.3, 0.0))
            .box_shadow(Shadow { color: if *on { Color::WHITE } else { Color::BLACK }, ..sh(2.0) })
    }
    let mut rt = Runtime::new(false, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert!(moving(&rt, "a"));
    assert_ne!(rt.presented_pose("a").unwrap().shadows()[0].color, Color::WHITE);
    settle(&mut rt);
    assert!(!moving(&rt, "a"));
    assert!(!rt.presented_pose("a").unwrap().moving());
}

#[test]
fn an_icon_turn_only_step_settles() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        static DOT: rune::Icon = rune::Icon { name: "dot", view: 24.0, body: "<circle cx=\"12\" cy=\"12\" r=\"8\"/>" };
        rune::icon(&DOT, 40.0).key("a").motion(Spring::new(0.3, 0.0))
            .icon_rotation(if *on { 1.0 } else { 0.0 })
    }
    let mut rt = Runtime::new(false, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert!(moving(&rt, "a"));
    assert!(rt.presented_pose("a").unwrap().moving());
    settle(&mut rt);
    assert!(!moving(&rt, "a"));
    assert!(!rt.presented_pose("a").unwrap().moving());
}

#[test]
fn pose_and_element_moving_agree_for_loops_and_an_anchors_shadow() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        col().w_full().h_full()
            .child(el().key("anchor").size(40.0, 40.0).motion(Spring::new(0.3, 0.0))
                .box_shadow(sh(if *on { 12.0 } else { 2.0 })))
            .child(el().key("popup").size(20.0, 20.0).above("anchor", 0.0))
            .child(el().key("loop").size(20.0, 20.0).looping(rune::LoopSpec::rotation(1.0)))
    }
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert!(moving(&rt, "popup"));
    assert!(rt.presented_pose("popup").unwrap().moving());
    settle(&mut rt);
    assert!(!moving(&rt, "popup"));
    assert!(!rt.presented_pose("popup").unwrap().moving());
    assert!(moving(&rt, "loop"));
    assert!(rt.presented_pose("loop").unwrap().moving());
}

#[test]
fn a_turned_ancestor_clips_its_child_in_layer_coordinates() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full().padding(50.0).items_start().child(
            el().key("clip").size(100.0, 100.0).rotate(FRAC_PI_4).clip_rounded(0.0)
                .child(el().key("child").size(200.0, 100.0).absolute(0.0, 0.0)),
        )
    }
    let mut rt = Runtime::new((), view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(rt.contains("child", 100.0, 100.0));
    assert!(!rt.contains("child", 149.5, 149.5), "inside child but outside the turned parent clip");
    assert_eq!(rt.element_at(100.0, 100.0).unwrap().key(), Some("child"));
    assert_ne!(rt.element_at(149.5, 149.5).unwrap().key(), Some("child"));
}

#[test]
fn sticky_and_anchored_elements_pick_in_paint_order_after_scrolling() {
    fn view(_: &Hits, _: &mut Cx<Hits>) -> El {
        col().w_full().h_full().child(
            col().key("scroll").size(100.0, 100.0).scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().w_full().h(300.0)
                    .child(el().key("header").w_full().h(30.0).sticky_top(0.0)
                        .on_press(|s: &mut Hits| s.0.push("header")))
                    .child(el().key("row").w_full().h(270.0)
                        .on_press(|s: &mut Hits| s.0.push("row")))
                    .child(el().key("popup").size(60.0, 20.0)
                        .anchored("header", rune::Side::Bottom, rune::Align::Start, 0.0)
                        .on_press(|s: &mut Hits| s.0.push("popup")))),
        )
    }
    let mut rt = Runtime::new(Hits::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_y("scroll", 20.0));
    rt.frame(0.0);
    for (y, expected) in [(15.0, "header"), (35.0, "popup"), (65.0, "row")] {
        assert_eq!(rt.element_at(20.0, y).unwrap().key(), Some(expected));
        rt.state.0.clear();
        rt.pointer_down(20.0, y);
        rt.pointer_up(20.0, y);
        rt.frame(0.0);
        assert_eq!(rt.state.0, [expected]);
    }
}

#[test]
fn pick_transparent_finds_opacity_zero_elements_but_not_exiting_ones() {
    #[derive(Default)]
    struct S { plain: Option<String>, with: Option<String>, gone: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        el().w_full().h_full().child(
            col().key("canvas").w_full().h_full().items_start()
                .child(el().key("clear").size(50.0, 50.0).opacity(0.0))
                .child(el().key("parent").size(50.0, 50.0).opacity(0.0)
                    .child(el().key("kid").size(20.0, 20.0)))
                .children((!s.gone).then(|| el().key("leaver").size(50.0, 50.0).fill(Color::WHITE)
                    .transition(rune::Transition::opacity()))),
        )
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let on = rune::Pick::new().transparent(true);
    let key = |p: Option<rune::Picked>| p.and_then(|p| p.key().map(str::to_owned));
    // `clear` is at y 0..50, `parent` (and `kid`) at 50..100.
    assert_ne!(key(rt.element_at(10.0, 10.0)).as_deref(), Some("clear"));
    assert_eq!(key(rt.element_at_with(10.0, 10.0, rune::Pick::default())).as_deref(), key(rt.element_at(10.0, 10.0)).as_deref());
    assert_eq!(key(rt.element_at_with(10.0, 10.0, on)).as_deref(), Some("clear"));
    assert_eq!(key(rt.element_at_in_with("canvas", 10.0, 10.0, on)).as_deref(), Some("clear"));
    assert_ne!(key(rt.element_at_in("canvas", 10.0, 10.0)).as_deref(), Some("clear"));
    assert!(!rt.contains("clear", 10.0, 10.0));
    assert!(rt.contains_with("clear", 10.0, 10.0, on));
    // A child of an opacity-0 parent inherits 0.
    assert_ne!(key(rt.element_at(10.0, 60.0)).as_deref(), Some("kid"));
    assert_eq!(key(rt.element_at_with(10.0, 60.0, on)).as_deref(), Some("kid"));
    assert!(rt.contains_with("kid", 10.0, 60.0, on) && !rt.contains("kid", 10.0, 60.0));

    // The Tx path.
    fn read(s: &mut S, with: bool, tx: &mut rune::Tx) {
        let on = rune::Pick::new().transparent(with);
        let f = move |s: &mut S, p: Option<rune::Picked>, _: &mut rune::Tx| {
            let k = p.and_then(|p| p.key().map(str::to_owned));
            if with { s.with = k } else { s.plain = k }
        };
        let _ = s;
        tx.element_at_in_with("canvas", 10.0, 10.0, on, f);
    }
    rt.update(|s, tx| { read(s, false, tx); read(s, true, tx); });
    rt.frame(0.0);
    assert_ne!(rt.state.plain.as_deref(), Some("clear"));
    assert_eq!(rt.state.with.as_deref(), Some("clear"));

    // An exiting element is skipped even with the option.
    let (x, y) = (10.0, 110.0);
    assert_eq!(key(rt.element_at_with(x, y, on)).as_deref(), Some("leaver"));
    rt.update(|s, _| s.gone = true);
    rt.frame(1.0 / 60.0);
    assert_ne!(key(rt.element_at_with(x, y, on)).as_deref(), Some("leaver"));
    assert!(!rt.contains_with("leaver", x, y, on));
}
#[test]
fn pick_interactive_skips_passive_layers_and_matches_presses() {
    #[derive(Default)]
    struct S { pressed: bool }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().size(300.0, 200.0)
            .child(col().key("rows")
                .child(el().key("row-1").size(300.0, 30.0)
                    .on_press(|s: &mut S| s.pressed = true)
                    .child(el().key("row-label").size(120.0, 20.0))))
            .child(col().key("layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .child(el().key("bar").w(6.0).h_full()))
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Default pick still stops on the topmost passive overlay layer.
    let plain = rt.element_at(150.0, 15.0).unwrap();
    assert_eq!(plain.key(), Some("layer"));
    assert_eq!(rt.element_at_with(150.0, 15.0, rune::Pick::default()), Some(plain));

    // Pick::interactive skips both the passive overlay layer and passive child label inside row-1.
    let inter = rune::Pick::new().interactive(true);
    let hit = rt.element_at_with(150.0, 15.0, inter).unwrap();
    assert_eq!(hit.key(), Some("row-1"));
    assert_eq!(hit.rect(), [0.0, 0.0, 300.0, 30.0]);
    assert_eq!(hit.local(), [150.0, 15.0]);

    let over_child = rt.element_at_with(20.0, 10.0, inter).unwrap();
    assert_eq!(over_child.key(), Some("row-1"));
    assert_eq!(over_child.local(), [20.0, 10.0]);
    // PartialEq / Eq compare element identity (path and key), not the queried local point.
    assert_eq!(hit, over_child);

    // Empty area below row-1 has only passive boxes.
    assert_eq!(rt.element_at_with(150.0, 100.0, inter), None);

    // contains_with respects Pick::interactive.
    assert!(rt.contains("layer", 150.0, 15.0));
    assert!(!rt.contains_with("layer", 150.0, 15.0, inter));
    assert!(rt.contains_with("row-1", 150.0, 15.0, inter));

    // A real press at the same point reaches row-1.
    rt.pointer_move(150.0, 15.0);
    rt.pointer_down(150.0, 15.0);
    rt.pointer_up(150.0, 15.0);
    rt.frame(0.016);
    assert!(rt.state.pressed);
}

#[test]
fn pick_drop_targets_and_combined_paint_order() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(300.0, 200.0)
            // Left half: drop zone painted below an interactive button.
            .child(el().key("zone-under").absolute(10.0, 10.0).size(120.0, 80.0)
                .on_drop_files(|_: &mut (), _, _| {})
                .child(el().key("inner-drop").absolute(10.0, 10.0).size(40.0, 30.0)
                    .on_drop_files(|_: &mut (), _, _| {})))
            .child(el().key("btn-over").absolute(60.0, 20.0).size(60.0, 40.0)
                .on_press(|_: &mut ()| {}))
            // Right half: interactive button painted below a drop zone.
            .child(el().key("btn-under").absolute(160.0, 10.0).size(120.0, 80.0)
                .on_press(|_: &mut ()| {}))
            .child(el().key("zone-over").absolute(180.0, 20.0).size(80.0, 40.0)
                .on_drop_files(|_: &mut (), _, _| {}))
            // Passive full-size overlay over everything.
            .child(el().key("passive-cover").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
    }
    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let drops = rune::Pick::new().drop_targets(true);
    let inter = rune::Pick::new().interactive(true);
    let both = rune::Pick::new().interactive(true).drop_targets(true);

    // Passive cover wins for default pick everywhere.
    assert_eq!(rt.element_at(30.0, 30.0).unwrap().key(), Some("passive-cover"));

    // Nested drop zones: innermost wins at overlap, outer wins outside inner.
    let inner = rt.element_at_with(30.0, 30.0, drops).unwrap();
    assert_eq!(inner.key(), Some("inner-drop"));
    assert_eq!(inner.rect(), [20.0, 20.0, 40.0, 30.0]);
    assert_eq!(inner.local(), [10.0, 10.0]);
    assert_eq!(rt.element_at_with(30.0, 30.0, inter), None);

    // Where btn-over overlaps zone-under (80, 35):
    assert_eq!(rt.element_at_with(80.0, 35.0, drops).unwrap().key(), Some("zone-under"));
    assert_eq!(rt.element_at_with(80.0, 35.0, inter).unwrap().key(), Some("btn-over"));
    assert_eq!(rt.element_at_with(80.0, 35.0, both).unwrap().key(), Some("btn-over"));

    // Where zone-over overlaps btn-under (200, 35):
    assert_eq!(rt.element_at_with(200.0, 35.0, drops).unwrap().key(), Some("zone-over"));
    assert_eq!(rt.element_at_with(200.0, 35.0, inter).unwrap().key(), Some("btn-under"));
    assert_eq!(rt.element_at_with(200.0, 35.0, both).unwrap().key(), Some("zone-over"));

    // contains_with checks the target directly regardless of paint order.
    assert!(rt.contains_with("zone-under", 80.0, 35.0, drops));
    assert!(!rt.contains_with("zone-under", 80.0, 35.0, inter));
    assert!(rt.contains_with("btn-under", 200.0, 35.0, inter));
    assert!(!rt.contains_with("btn-under", 200.0, 35.0, drops));
    assert!(rt.contains_with("zone-over", 200.0, 35.0, both));
    assert!(!rt.contains_with("passive-cover", 200.0, 35.0, both));
}

#[test]
fn pick_filters_respect_inert_modal_exiting_opacity_and_edges() {
    #[derive(Default)]
    struct S { modal_open: bool, gone: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        el().size(300.0, 200.0)
            .child(col().key("inert-group").absolute(0.0, 0.0).size(100.0, 60.0).inert()
                .child(el().key("inert-btn").size(50.0, 30.0).on_press(|_: &mut S| {}))
                .child(el().key("inert-drop").size(50.0, 30.0).on_drop_files(|_: &mut S, _, _| {})))
            .child(el().key("live-btn").absolute(110.0, 10.0).size(60.0, 40.0).on_press(|_: &mut S| {}))
            .child(el().key("live-drop").absolute(180.0, 10.0).size(60.0, 40.0).on_drop_files(|_: &mut S, _, _| {}))
            .child(el().key("clear-btn").absolute(10.0, 80.0).size(60.0, 40.0).opacity(0.0).on_press(|_: &mut S| {}))
            .child(el().key("zero-btn").absolute(100.0, 80.0).size(0.0, 0.0).on_press(|_: &mut S| {}))
            .children((!s.gone).then(|| el().key("leaving-btn").absolute(120.0, 80.0).size(60.0, 40.0)
                .fill(Color::WHITE).transition(rune::Transition::opacity()).on_press(|_: &mut S| {})))
            .children(s.modal_open.then(|| {
                el().key("dialog").absolute(80.0, 130.0).size(140.0, 50.0).modal_trap()
                    .child(el().key("modal-btn").size(60.0, 40.0).on_press(|_: &mut S| {}))
            }))
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let inter = rune::Pick::new().interactive(true);
    let drops = rune::Pick::new().drop_targets(true);

    // Inert elements are found by default pick, skipped by interactive and drop_targets.
    assert_eq!(rt.element_at(15.0, 15.0).unwrap().key(), Some("inert-btn"));
    assert_eq!(rt.element_at_with(15.0, 15.0, inter), None);
    assert_eq!(rt.element_at_with(15.0, 45.0, drops), None);
    assert!(!rt.contains_with("inert-btn", 15.0, 15.0, inter));
    assert!(!rt.contains_with("inert-drop", 15.0, 45.0, drops));

    // Opacity-0 interactive element takes no input, so interactive skips it.
    assert_eq!(rt.element_at_with(30.0, 100.0, inter), None);
    assert_eq!(rt.element_at_with(30.0, 100.0, rune::Pick::new().transparent(true)).unwrap().key(), Some("clear-btn"));

    // Zero-size and non-finite coordinates never match.
    assert_eq!(rt.element_at_with(100.0, 80.0, inter), None);
    assert_eq!(rt.element_at_with(f32::NAN, 15.0, inter), None);
    assert_eq!(rt.element_at_with(f32::INFINITY, 15.0, drops), None);
    assert_eq!(rt.element_at(f32::NAN, f32::NAN), None);

    // Exiting element is skipped.
    assert_eq!(rt.element_at_with(140.0, 95.0, inter).unwrap().key(), Some("leaving-btn"));
    rt.update(|s, _| s.gone = true);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.element_at_with(140.0, 95.0, inter), None);

    // Opening a modal blocks background interactive and drop targets, while modal controls match.
    assert_eq!(rt.element_at_with(130.0, 25.0, inter).unwrap().key(), Some("live-btn"));
    assert_eq!(rt.element_at_with(200.0, 25.0, drops).unwrap().key(), Some("live-drop"));
    rt.update(|s, tx| { s.modal_open = true; tx.instant(); });
    rt.frame(0.0);
    assert_eq!(rt.element_at(130.0, 25.0).unwrap().key(), Some("live-btn"));
    assert_eq!(rt.element_at_with(130.0, 25.0, inter), None);
    assert_eq!(rt.element_at_with(200.0, 25.0, drops), None);
    assert_eq!(rt.element_at_with(100.0, 145.0, inter).unwrap().key(), Some("modal-btn"));
}

#[test]
fn picked_rect_and_local_match_pointer_events_under_scroll_scale_and_rotate() {
    #[derive(Default)]
    struct S {
        last_local: Option<[f32; 2]>,
        tx_picked: Option<rune::Picked>,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().size(400.0, 300.0)
            // Scrolled list at (10, 10)
            .child(col().key("scroller").absolute(10.0, 10.0).size(140.0, 80.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().w_full().h(240.0)
                    .child(el().key("pad-top").w_full().h(60.0))
                    .child(el().key("scroll-row").w_full().h(40.0)
                        .on_pointer_move_local(|s: &mut S, p, _| s.last_local = Some([p.x, p.y]))
                        .on_press_tx(|_: &mut S, tx| {
                            tx.element_at_in_with(
                                "scroller",
                                45.0,
                                55.0,
                                rune::Pick::new().interactive(true),
                                |s: &mut S, p, _| s.tx_picked = p,
                            );
                        }))))
            // Scaled box at (180, 20), layout 80x40 scaled 2x about origin (0, 0) -> drawn [180, 20, 160, 80]
            .child(el().key("scaled").absolute(180.0, 20.0).size(80.0, 40.0)
                .scale(2.0).origin(0.0, 0.0)
                .on_pointer_move_local(|s: &mut S, p, _| s.last_local = Some([p.x, p.y])))
            // Rotated box at (100, 160), layout 100x20 rotated 90 deg CW about center (150, 170)
            .child(el().key("turned").absolute(100.0, 160.0).size(100.0, 20.0)
                .rotate(std::f32::consts::FRAC_PI_2)
                .on_pointer_move_local(|s: &mut S, p, _| s.last_local = Some([p.x, p.y])))
            // Passive cover over everything
            .child(el().key("cover").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
    }

    fn assert_eq_trait<T: Eq>() {}
    assert_eq_trait::<rune::Picked>();

    for scale in [1.0, 2.0] {
        let mut rt = Runtime::new(S::default(), view, (400.0, 300.0), Env::new(true, scale));
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_to_y("scroller", 30.0));
        rt.frame(0.0);

        let inter = rune::Pick::new().interactive(true);
        let elem_rect = |rt: &Runtime<S>, k: &str| {
            rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).unwrap().rect
        };

        // 1. Scrolled row: scroller at y=10, pad-top=60, scrolled by 30 => row drawn at y=40..80.
        let p = rt.element_at_with(45.0, 55.0, inter).unwrap();
        assert_eq!(p.key(), Some("scroll-row"));
        assert_eq!(p.rect(), elem_rect(&rt, "scroll-row"));
        assert!((p.rect()[0] - 10.0).abs() < 1e-3 && (p.rect()[1] - 40.0).abs() < 1e-3);
        assert!((p.local()[0] - 35.0).abs() < 1e-3 && (p.local()[1] - 15.0).abs() < 1e-3);
        rt.pointer_move(45.0, 55.0);
        rt.frame(0.0);
        let ev = rt.state.last_local.unwrap();
        assert!((ev[0] - p.local()[0]).abs() < 1e-3 && (ev[1] - p.local()[1]).abs() < 1e-3);

        // Tx::element_at_in_with returns identical rect and local from a handler.
        rt.pointer_down(45.0, 55.0);
        rt.pointer_up(45.0, 55.0);
        rt.frame(0.0);
        let txp = rt.state.tx_picked.clone().unwrap();
        assert_eq!(txp, p);
        assert_eq!(txp.rect(), p.rect());
        assert_eq!(txp.local(), p.local());

        // 2. Scaled element: drawn [180, 20, 160, 80]; window point (260, 60) -> local (40, 20).
        let ps = rt.element_at_with(260.0, 60.0, inter).unwrap();
        assert_eq!(ps.key(), Some("scaled"));
        assert_eq!(ps.rect(), elem_rect(&rt, "scaled"));
        assert!((ps.rect()[2] - 160.0).abs() < 1e-3 && (ps.rect()[3] - 80.0).abs() < 1e-3);
        assert!((ps.local()[0] - 40.0).abs() < 1e-3 && (ps.local()[1] - 20.0).abs() < 1e-3);
        rt.state.last_local = None;
        rt.pointer_move(260.0, 60.0);
        rt.frame(0.0);
        let ev = rt.state.last_local.unwrap();
        assert!((ev[0] - ps.local()[0]).abs() < 1e-3 && (ev[1] - ps.local()[1]).abs() < 1e-3);

        // 3. Rotated element: center (150, 170); window point (150, 200) is 30pt down from center
        // which unturns 90 deg CCW to +30pt right of center -> local (80, 10).
        let pt = rt.element_at_with(150.0, 200.0, inter).unwrap();
        assert_eq!(pt.key(), Some("turned"));
        assert_eq!(pt.rect(), elem_rect(&rt, "turned"));
        assert!((pt.local()[0] - 80.0).abs() < 1e-2 && (pt.local()[1] - 10.0).abs() < 1e-2,
            "scale={scale}: rotated local={:?}", pt.local());
        rt.state.last_local = None;
        rt.pointer_move(150.0, 200.0);
        rt.frame(0.0);
        let ev = rt.state.last_local.unwrap();
        assert!((ev[0] - pt.local()[0]).abs() < 1e-3 && (ev[1] - pt.local()[1]).abs() < 1e-3);
    }
}

