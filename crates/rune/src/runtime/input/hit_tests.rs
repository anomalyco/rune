use crate::{Cx, El, Env, Pick, Runtime, ScrollSpec, TouchPhase, col, el};

#[test]
fn click_after_mount_lands_on_frame_zero_while_paint_opacity_starts_at_zero() {
    #[derive(Default)]
    struct S {
        shown: bool,
        direct_presses: u32,
        nested_presses: u32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().w_full().h_full();
        if s.shown {
            root = root
                .child(
                    el().key("direct")
                        .size(100.0, 40.0)
                        .role("button")
                        .on_press(|s: &mut S| s.direct_presses += 1),
                )
                .child(
                    col().key("panel").size(100.0, 40.0).child(
                        el().key("nested")
                            .size(100.0, 40.0)
                            .role("button")
                            .on_press(|s: &mut S| s.nested_presses += 1),
                    ),
                );
        }
        root
    }

    let mut rt = Runtime::new(S::default(), view, (200.0, 120.0), Env::new(false, 1.0));
    rt.frame(0.0);

    rt.update(|s, _| s.shown = true);
    rt.frame(0.0);

    // Visual entrance fade still starts at opacity 0 on frame 0.
    assert_eq!(rt.presented("direct").unwrap().1, 0.0);
    assert_eq!(rt.presented("nested").unwrap().1, 0.0);

    // Clicking the newly mounted button right after frame(0.0) lands immediately.
    let (dx, dy) = rt.find("direct").unwrap();
    rt.pointer_down(dx, dy);
    rt.pointer_up(dx, dy);
    rt.frame(0.0);
    assert_eq!(rt.state.direct_presses, 1, "direct newly mounted button must receive click on frame 0");

    // Clicking a button riding an entering parent container also lands on frame 0.
    let (nx, ny) = rt.find("nested").unwrap();
    rt.pointer_down(nx, ny);
    rt.pointer_up(nx, ny);
    rt.frame(0.0);
    assert_eq!(rt.state.nested_presses, 1, "child riding an entering parent must receive click on frame 0");
}

#[test]
fn sibling_pointer_hover_secondary_wheel_touch_drop_pick_and_modal_paths_work_on_frame_zero() {
    #[derive(Default)]
    struct S {
        shown: bool,
        modal: bool,
        presses: u32,
        hovers: u32,
        hover_enters: u32,
        moves: u32,
        secondary: u32,
        modal_presses: u32,
        modal_dismisses: u32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().w_full().h_full();
        if s.shown {
            root = root
                .child(
                    el().key("btn")
                        .absolute(10.0, 10.0)
                        .size(80.0, 30.0)
                        .hit()
                        .on_press(|s: &mut S| s.presses += 1)
                        .on_hover(|s: &mut S| s.hovers += 1)
                        .on_hover_change(|s: &mut S, on, _| {
                            if on { s.hover_enters += 1; }
                        })
                        .on_pointer_move(|s: &mut S, _| s.moves += 1)
                        .on_secondary_press(|s: &mut S, _, _| s.secondary += 1),
                )
                .child(
                    el().key("drop")
                        .absolute(100.0, 10.0)
                        .size(80.0, 30.0)
                        .on_drop_files(|_: &mut S, _, _| {}),
                )
                .child(
                    col().key("scroll")
                        .absolute(10.0, 50.0)
                        .size(160.0, 60.0)
                        .scroll(ScrollSpec { stick_bottom: false })
                        .child(el().key("tall").size(160.0, 300.0)),
                );
        }
        if s.modal {
            root = root.child(
                col().key("dialog")
                    .absolute(20.0, 20.0)
                    .size(160.0, 80.0)
                    .modal(true, |s: &mut S| {
                        s.modal = false;
                        s.modal_dismisses += 1;
                    })
                    .child(
                        el().key("confirm")
                            .size(100.0, 30.0)
                            .on_press(|s: &mut S| s.modal_presses += 1),
                    ),
            );
        }
        root
    }

    let mut rt = Runtime::new(S::default(), view, (220.0, 140.0), Env::new(true, 1.0));
    rt.host.native_overlay = true;
    rt.frame(0.0);

    rt.update(|s, _| s.shown = true);
    rt.frame(0.0);

    // Pick::interactive, Pick::drop_targets, and overlay hit_regions all see the entering nodes.
    let (bx, by) = rt.find("btn").unwrap();
    let (dx, dy) = rt.find("drop").unwrap();
    assert_eq!(
        rt.element_at_with(bx, by, Pick::new().interactive(true)).as_ref().and_then(|p| p.key()),
        Some("btn"),
        "Pick::interactive finds entering button on frame 0",
    );
    assert_eq!(
        rt.element_at_with(dx, dy, Pick::new().drop_targets(true)).as_ref().and_then(|p| p.key()),
        Some("drop"),
        "Pick::drop_targets finds entering drop zone on frame 0",
    );
    assert!(
        !rt.hit_regions().is_empty(),
        "overlay hit_regions includes entering .hit() element on frame 0",
    );

    // Pointer move + hover + secondary click + touch tap on frame 0.
    rt.pointer_move(bx, by);
    rt.secondary_down(bx, by);
    rt.secondary_up(bx, by);
    rt.frame(0.0);
    assert_eq!(rt.state.moves, 1, "pointer_move reaches entering button on frame 0");
    assert_eq!(rt.state.hovers, 1, "on_hover reaches entering button on frame 0");
    assert_eq!(rt.state.hover_enters, 1, "on_hover_change(true) reaches entering button on frame 0");
    assert_eq!(rt.state.secondary, 1, "secondary press reaches entering button on frame 0");

    rt.touch(1, TouchPhase::Started, bx, by, 0.0);
    rt.touch(1, TouchPhase::Ended, bx, by, 0.01);
    rt.frame(0.0);
    assert_eq!(rt.state.presses, 1, "touch tap reaches entering button on frame 0");

    // Wheel hits a newly mounted scroll view on frame 0.
    let (sx, sy) = rt.find("scroll").unwrap();
    let before_tall = rt.find("tall").unwrap().1;
    rt.wheel(sx, sy, -40.0);
    rt.frame(1.0 / 60.0);
    let after_tall = rt.find("tall").unwrap().1;
    assert!(after_tall < before_tall, "wheel scrolls newly mounted scroll view on frame 0 ({before_tall} -> {after_tall})");

    // Clicking a button inside a newly mounted modal on frame 0 invokes the button and keeps the modal open.
    rt.update(|s, _| s.modal = true);
    rt.frame(0.0);
    let (cx, cy) = rt.find("confirm").unwrap();
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(0.0);
    assert_eq!(rt.state.modal_presses, 1, "button inside newly mounted modal receives click on frame 0");
    assert_eq!(rt.state.modal_dismisses, 0, "clicking modal button on frame 0 does not dismiss modal");
    assert!(rt.state.modal);
}

#[test]
fn authored_opacity_zero_live_opacity_zero_exiting_and_enter_delay_still_ignore_hits() {
    struct S {
        show: bool,
        live_zero: crate::Live<f32>,
        zero_presses: u32,
        parent_zero_presses: u32,
        live_zero_presses: u32,
        delayed_presses: u32,
        frozen_presses: u32,
        exiting_presses: u32,
        exit_now: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().w_full().h_full();
        if s.show {
            root = root
                .child(
                    el().key("zero")
                        .absolute(0.0, 0.0)
                        .size(60.0, 30.0)
                        .opacity(0.0)
                        .on_press(|s: &mut S| s.zero_presses += 1),
                )
                .child(
                    col().key("parent-zero")
                        .absolute(70.0, 0.0)
                        .size(60.0, 30.0)
                        .opacity(0.0)
                        .child(
                            el().key("child-of-zero")
                                .size(60.0, 30.0)
                                .on_press(|s: &mut S| s.parent_zero_presses += 1),
                        ),
                )
                .child(
                    el().key("live-zero")
                        .absolute(140.0, 0.0)
                        .size(60.0, 30.0)
                        .live_opacity(&s.live_zero)
                        .on_press(|s: &mut S| s.live_zero_presses += 1),
                )
                .child(
                    el().key("delayed")
                        .absolute(0.0, 40.0)
                        .size(60.0, 30.0)
                        .enter_delay(0.5)
                        .on_press(|s: &mut S| s.delayed_presses += 1),
                )
                .child(
                    el().key("frozen")
                        .absolute(70.0, 40.0)
                        .size(60.0, 30.0)
                        .time_scale(0.0)
                        .on_press(|s: &mut S| s.frozen_presses += 1),
                );
        }
        if !s.exit_now {
            root = root.child(
                el().key("exiting")
                    .absolute(140.0, 40.0)
                    .size(60.0, 30.0)
                    .on_press(|s: &mut S| s.exiting_presses += 1),
            );
        }
        root
    }

    let mut rt = Runtime::new(
        S {
            show: false,
            live_zero: crate::Live::new(0.0),
            zero_presses: 0,
            parent_zero_presses: 0,
            live_zero_presses: 0,
            delayed_presses: 0,
            frozen_presses: 0,
            exiting_presses: 0,
            exit_now: false,
        },
        view,
        (220.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let click = |rt: &mut Runtime<S>, x: f32, y: f32| {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
    };

    rt.update(|s, _| {
        s.show = true;
        s.exit_now = true;
    });
    rt.frame(0.0);

    click(&mut rt, 30.0, 15.0);
    click(&mut rt, 100.0, 15.0);
    click(&mut rt, 170.0, 15.0);
    click(&mut rt, 30.0, 55.0);
    click(&mut rt, 100.0, 55.0);
    click(&mut rt, 170.0, 55.0);
    assert_eq!(
        (
            rt.state.zero_presses,
            rt.state.parent_zero_presses,
            rt.state.live_zero_presses,
            rt.state.delayed_presses,
            rt.state.frozen_presses,
            rt.state.exiting_presses,
        ),
        (0, 0, 0, 0, 0, 0),
        "opacity(0), parent opacity(0), live_opacity(0), waiting enter_delay, frozen clock, and exiting nodes must not take hits",
    );

    // Once enter_delay elapses, the delayed button becomes clickable.
    rt.frame(0.5);
    click(&mut rt, 30.0, 55.0);
    assert_eq!(rt.state.delayed_presses, 1, "delayed button takes hits once its entrance starts");
}

#[test]
fn hover_group_overhanging_entering_child_and_group_opacity_entering_child() {
    #[derive(Default)]
    struct S {
        shown: bool,
        overhang_presses: u32,
        reveal_presses: u32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut row = col().key("row").absolute(0.0, 0.0).size(100.0, 40.0).hover_group();
        if s.shown {
            row = row
                .child(
                    el().key("overhang")
                        .absolute(110.0, 0.0)
                        .size(40.0, 40.0)
                        .on_press(|s: &mut S| s.overhang_presses += 1),
                )
                .child(
                    el().key("reveal")
                        .absolute(60.0, 0.0)
                        .size(80.0, 40.0)
                        .group_opacity(0.0, 1.0)
                        .on_press(|s: &mut S| s.reveal_presses += 1),
                );
        }
        col().w_full().h_full().child(row)
    }

    let mut rt = Runtime::new(S::default(), view, (200.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.shown = true);
    rt.frame(0.0);

    // While the hover_group is unhovered, the hover-revealed entering child (group_opacity(0, 1))
    // is clipped to the hover_group: its outside-group overhang misses, while its inside-group part hits.
    rt.pointer_down(105.0, 20.0);
    rt.pointer_up(105.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.reveal_presses, 0, "group_opacity entering child outside its hover_group does not take hits while unhovered");

    // Normal newly mounted child overhanging its hover_group is still clickable outside the group's rect.
    rt.pointer_down(130.0, 20.0);
    rt.pointer_up(130.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.overhang_presses, 1, "overhanging entering child without group_opacity is not clipped to hover_group");

    // Inside-group portion of the group_opacity entering child is clickable on frame 0.
    rt.pointer_down(80.0, 20.0);
    rt.pointer_up(80.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.reveal_presses, 1, "group_opacity entering child is clickable inside its hover_group on frame 0");
}
