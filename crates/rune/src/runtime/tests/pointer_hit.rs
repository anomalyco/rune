use super::*;


#[test]
fn overlay_hit_regions_follow_live_geometry_and_ancestor_clips_in_the_same_frame() {
    struct State { offset: crate::Live<[f32; 2]>, opacity: crate::Live<f32>, pressed: u32, hovered: u32, visible: bool }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        el().child(el().size(60.0, 60.0).clip_soft(0.0)
            .children(s.visible.then(|| el().key("chip").size(40.0, 20.0).pin(Some(30.0), Some(0.0), None, None)
                .live_offset(&s.offset).live_opacity(&s.opacity).hit()
                .on_hover(|s: &mut State| s.hovered += 1).on_press(|s: &mut State| s.pressed += 1))))
            .child(el().size(10.0, 10.0).on_hover(|_: &mut State| {}))
    }
    let offset = crate::Live::new([0.0, 0.0]);
    let opacity = crate::Live::new(1.0);
    let mut rt = Runtime::new(State { offset: offset.clone(), opacity: opacity.clone(), pressed: 0, hovered: 0, visible: true }, view, (100.0, 100.0), Env { system_dark: true, scale: 2.0 });
    rt.host.native_overlay = true;
    rt.frame(0.0);
    assert_eq!(rt.hit_regions(), &[[30.0, 0.0, 60.0, 20.0]]);
    rt.pointer_move(15.0, 10.0);
    offset.set([20.0, 0.0]);
    rt.frame(0.0);
    assert_eq!(rt.hit_regions(), &[[50.0, 0.0, 60.0, 20.0]], "logical regions must use this frame's live offset, not the previous layout");
    offset.set([-20.0, 0.0]);
    rt.frame(0.0);
    assert_eq!(rt.hit_regions(), &[[10.0, 0.0, 50.0, 20.0]]);
    rt.pointer_down(15.0, 10.0);
    rt.pointer_up(15.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.pressed, 1, "the native hit and Rune release must agree on the moved chip");
    assert_eq!(rt.state.hovered, 1, "a chip moving under a stationary pointer enters hover once");
    assert!(!rt.has_pending_input(), "stationary hover must not generate an endless frame loop");
    rt.pointer_down(15.0, 10.0);
    rt.pointer_up(55.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.pressed, 1, "release inside the old layout but outside the drawn chip must cancel");
    opacity.set(0.0);
    rt.frame(0.0);
    assert!(rt.hit_regions().is_empty(), "transparent nodes and hover-only elements do not block native input");
    opacity.set(1.0);
    rt.update(|s, _| s.visible = false);
    rt.frame(0.0);
    assert!(rt.hit_regions().is_empty(), "exiting chips must immediately release native input");
}

#[test]
fn overlay_pointer_observation_does_not_require_a_native_hit_region() {
    fn view(_: &Vec<PointerPress>, _: &mut Cx<Vec<PointerPress>>) -> El {
        el().on_pointer_move(|s: &mut Vec<PointerPress>, p| s.push(p))
    }
    let mut rt = Runtime::new(Vec::new(), view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    rt.pointer_move(20.0, 40.0);
    rt.frame(0.0);
    assert_eq!(rt.state, [PointerPress { x: 20.0, y: 40.0 }]);
    assert!(rt.hit_regions().is_empty());
}

#[test]
fn overlay_idle_pointer_does_not_request_unchanged_frames() {
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        el().child(el().size(40.0, 20.0).hit().on_hover(|n: &mut u32| *n += 1))
    }
    let mut rt = Runtime::new(0, view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    assert!(rt.observe_pointer(5.0, 5.0), "enter must run the hover callback");
    rt.frame(0.0);
    assert_eq!(rt.state, 1);
    assert!(!rt.observe_pointer(6.0, 5.0), "movement within an unchanged chip needs no frame");
    assert!(!rt.observe_pointer(6.0, 5.0), "duplicate monitor/winit observations need no frame");
    assert!(!rt.observe_pointer(90.0, 90.0));
    assert!(!rt.observe_pointer(200.0, 90.0), "movement outside the canvas needs no frame");
    assert!(!rt.has_pending_input());
}

#[test]
fn hit_tests_leave_out_the_right_and_bottom_edges() {
    fn tile(k: &'static str) -> El {
        el().key(k).size(40.0, 20.0)
            .on_press(move |s: &mut Vec<String>| s.push(format!("press {k}")))
            .on_hover(move |s: &mut Vec<String>| s.push(format!("hover {k}")))
            .on_hover_change(move |s: &mut Vec<String>, inside, _| if inside { s.push(format!("enter {k}")) })
            .on_pointer_move(move |s: &mut Vec<String>, _| s.push(format!("move {k}")))
    }
    fn view(_: &Vec<String>, _: &mut Cx<Vec<String>>) -> El {
        col().gap(20.0).child(crate::row().child(tile("a")).child(tile("b"))).child(tile("lone"))
    }
    let mut rt = Runtime::new(Vec::new(), view, (200.0, 200.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    assert_eq!(rt.presented("lone").unwrap().0, [0.0, 40.0, 40.0, 20.0]);
    let mut click = |x, y| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        let mut events = std::mem::take(&mut rt.state);
        rt.pointer_leave();
        rt.frame(0.0);
        rt.state.clear();
        events.sort();
        events.dedup();
        events
    };
    assert_eq!(click(40.0, 10.0), ["enter b", "hover b", "move b", "press b"], "the shared edge is b's alone");
    assert_eq!(click(40.0, 50.0), Vec::<String>::new(), "a lone node's right edge is outside it");
    assert_eq!(click(20.0, 60.0), Vec::<String>::new(), "and so is its bottom edge");
    assert_eq!(click(0.0, 40.0), ["enter lone", "hover lone", "move lone", "press lone"], "its top-left corner is inside");
}

#[test]
fn deleting_a_focused_row_does_not_call_its_stale_edit_accessor() {
    fn view(s: &Vec<crate::TextEdit>, _: &mut Cx<Vec<crate::TextEdit>>) -> El {
        col().children(s.iter().enumerate().map(|(i, edit)| {
            crate::text_input(edit, crate::TextStyle::new(14.0, 400.0))
                .key(format!("row-{i}"))
                .on_edit(crate::edit_access(move |s: &mut Vec<crate::TextEdit>| &mut s[i]))
        })).child(el().key("delete").size(40.0, 40.0).on_press(|s: &mut Vec<crate::TextEdit>| { s.clear(); }))
    }
    let mut edit = crate::TextEdit::default();
    edit.focused = true;
    let mut rt = Runtime::new(vec![edit], view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let path = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("delete")).unwrap().path.clone();
    rt.frame.queue.push(Action::Press(path, crate::PointerButton::Primary, false));
    rt.frame(0.0);
    assert!(rt.state.is_empty());
    assert!(!rt.has_focus());
}

#[test]
fn secondary_only_children_preserve_primary_click_hover_and_background_blur() {
    type State = (u32, u32, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let parent = el().key("parent").size(80.0, 80.0);
        let parent = if s.2 { parent } else {
            parent.on_press(|s: &mut State| s.0 += 1)
                .on_hover(|s: &mut State| s.1 += 1)
        };
        parent.child(el().key("child").size(40.0, 40.0)
            .on_secondary_press(|_: &mut State, _, _| {}))
    }
    let mut rt = Runtime::new((0, 0, false), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(10.0, 10.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state, (1, 1, false));
    rt.state.2 = true;
    rt.invalidate();
    rt.frame(0.0);
    rt.kbd.focus.focus = crate::runtime::focus_machine::Focus::On { path: "previous-input".into(), hovered: false, from: None };
    rt.pointer_move(10.0, 10.0);
    rt.pointer_down(10.0, 10.0);
    assert!(rt.pointer.hovered.is_none());
    assert!(matches!(rt.frame.queue.iter().last(), Some(Action::Blur)));
    assert!(rt.secondary_target(10.0, 10.0).is_some());
}

#[test]
fn secondary_press_routes_to_ancestor_without_primary_and_cancels_outside() {
    type State = (u32, Vec<PointerPress>, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().child(if s.2 {
            el()
        } else {
            el().key("parent").size(80.0, 80.0)
                .on_secondary_press(|s: &mut State, p, _| s.1.push(p))
                .child(el().key("child").size(40.0, 40.0).on_press(|s: &mut State| s.0 += 1))
        })
    }
    let mut rt = Runtime::new((0, Vec::new(), false), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.secondary_down(10.0, 10.0);
    rt.secondary_up(12.0, 12.0);
    rt.frame(0.0);
    assert_eq!(rt.state.0, 0);
    assert_eq!(rt.state.1, [PointerPress { x: 12.0, y: 12.0 }]);
    rt.secondary_down(10.0, 10.0);
    rt.secondary_up(90.0, 90.0);
    rt.frame(0.0);
    assert_eq!(rt.state.1.len(), 1);
    rt.secondary_down(10.0, 10.0);
    rt.state.2 = true;
    rt.invalidate();
    rt.frame(0.0);
    rt.secondary_up(12.0, 12.0);
    rt.frame(0.0);
    assert_eq!(rt.state.1.len(), 1);
}

#[test]
fn presses_land_inside_a_scrolled_view() {
    struct State { pressed: Vec<usize> }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        let mut list = col().key("list").w_full();
        for i in 0..10 {
            list = list.child(el().key(format!("b{i}")).size(200.0, 40.0).on_press(move |s: &mut State| s.pressed.push(i)));
        }
        col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
    }
    let mut rt = Runtime::new(State { pressed: Vec::new() }, view, (200.0, 200.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    rt.wheel(100.0, 100.0, -120.0);
    rt.frame(1.0 / 120.0);
    let (x, y) = rt.find("b5").expect("b5 is drawn");
    assert!(y < 200.0, "b5 scrolled into view at {y}");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.pressed, [5]);
}

#[test]
fn a_double_press_follows_the_first_press_within_the_interval_and_tolerance() {
    #[derive(Default)]
    struct State { presses: u32, doubles: u32, other: u32 }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        crate::row().size(300.0, 100.0)
            .child(el().key("row").size(100.0, 100.0)
                .on_press(|s: &mut State| s.presses += 1)
                .on_double_press(|s: &mut State| s.doubles += 1))
            .child(el().key("other").size(100.0, 100.0).on_press(|s: &mut State| s.other += 1))
    }
    let tick = |rt: &mut Runtime<State>, seconds: f64| for _ in 0..(seconds * 120.0).round() as usize { rt.frame(1.0 / 120.0); };
    let click = |rt: &mut Runtime<State>, (x, y): (f32, f32)| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.frame(1.0 / 120.0);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 120.0);
    };
    let counts = |rt: &Runtime<State>| (rt.state.presses, rt.state.doubles);
    let mut rt = Runtime::new(State::default(), view, (300.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);

    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (1, 0), "the first press runs on_press right away");
    tick(&mut rt, 0.1);
    click(&mut rt, (53.0, 47.0));
    assert_eq!(counts(&rt), (2, 1), "the second press inside the interval and 4 pt is a double press");
    assert!(rt.events.iter().any(|e| e.starts_with("double press")));
    tick(&mut rt, 0.1);
    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (3, 1), "a third press starts a new pair");
    tick(&mut rt, 0.1);
    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (4, 2));

    tick(&mut rt, 0.1);
    click(&mut rt, (50.0, 50.0));
    let slow = rt.double_click_interval + 0.05;
    tick(&mut rt, slow);
    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (6, 2), "too slow");

    tick(&mut rt, 0.6);
    click(&mut rt, (50.0, 50.0));
    click(&mut rt, (55.0, 50.0));
    assert_eq!(counts(&rt), (8, 2), "moved more than 4 pt");

    tick(&mut rt, 0.6);
    click(&mut rt, (50.0, 50.0));
    click(&mut rt, (150.0, 50.0));
    click(&mut rt, (50.0, 50.0));
    assert_eq!((counts(&rt), rt.state.other), ((10, 2), 1), "a press elsewhere breaks the pair");

    tick(&mut rt, 0.6);
    click(&mut rt, (50.0, 50.0));
    rt.pointer_down(50.0, 50.0);
    rt.frame(1.0 / 120.0);
    rt.pointer_move(250.0, 50.0);
    rt.pointer_up(250.0, 50.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(counts(&rt), (11, 2), "a second press released outside is cancelled");

    // A second press that drags 40 pt (still inside) and lets go 5 s later is no click.
    tick(&mut rt, 0.6);
    click(&mut rt, (50.0, 50.0));
    rt.pointer_down(50.0, 50.0);
    rt.frame(1.0 / 120.0);
    rt.pointer_move(90.0, 50.0);
    rt.pointer_move(60.0, 50.0);
    tick(&mut rt, 5.0);
    rt.pointer_up(60.0, 50.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(counts(&rt), (13, 2), "a drag or a long press doesn't finish a double press");
    // Held without moving past the interval, it's no click either.
    tick(&mut rt, 0.6);
    click(&mut rt, (50.0, 50.0));
    rt.pointer_down(50.0, 50.0);
    let long = rt.double_click_interval + 0.05;
    tick(&mut rt, long);
    rt.pointer_up(50.0, 50.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(counts(&rt), (15, 2), "a long second press");
    // A first press that drags doesn't arm one.
    tick(&mut rt, 0.6);
    rt.pointer_down(50.0, 50.0);
    rt.pointer_move(50.0, 90.0);
    rt.pointer_move(50.0, 50.0);
    rt.pointer_up(50.0, 50.0);
    rt.frame(1.0 / 120.0);
    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (17, 2), "a dragged first press");

    // Headless, a slower system setting is honoured on the same virtual clock.
    rt.double_click_interval = 1.0;
    tick(&mut rt, 1.1);
    click(&mut rt, (50.0, 50.0));
    tick(&mut rt, 0.8);
    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (19, 3));
}

#[test]
fn an_element_with_only_a_double_press_is_a_press_target() {
    struct State { doubles: u32, behind: u32 }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        el().size(100.0, 100.0).on_press(|s: &mut State| s.behind += 1)
            .child(el().size(100.0, 100.0).on_double_press(|s: &mut State| s.doubles += 1))
    }
    let mut rt = Runtime::new(State { doubles: 0, behind: 0 }, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..2 {
        rt.pointer_move(50.0, 50.0);
        rt.pointer_down(50.0, 50.0);
        rt.frame(1.0 / 120.0);
        rt.pointer_up(50.0, 50.0);
        rt.frame(1.0 / 120.0);
    }
    assert_eq!((rt.state.doubles, rt.state.behind), (1, 0));
}

#[test]
fn presses_on_a_window_drag_region_move_the_window_but_not_its_buttons() {
    struct State { pressed: u32 }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 200.0)
            .child(crate::el::row().key("titlebar").size(200.0, 40.0).window_drag()
                .child(el().key("button").size(40.0, 40.0).on_press(|s: &mut State| s.pressed += 1)))
            .child(el().key("body").size(200.0, 40.0))
    }
    let mut rt = Runtime::new(State { pressed: 0 }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for (x, y, drags) in [(100.0, 20.0, true), (20.0, 20.0, false), (100.0, 60.0, false)] {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        assert_eq!(rt.take_titlebar_action() == Some(TitlebarAction::Drag), drags, "at {x}, {y}");
        assert!(rt.take_titlebar_action().is_none(), "a drag request is taken once");
        rt.pointer_up(x, y);
    }
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.pressed, 1, "the button inside the title bar still presses");
}

#[test]
fn a_double_press_on_nothing_in_the_native_titlebar_band_takes_the_titlebar_action() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(200.0, 200.0).child(el().key("button").size(40.0, 40.0).on_press(|_: &mut ()| {}))
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let double = |rt: &mut Runtime<()>, x: f32, y: f32| {
        let mut actions = Vec::new();
        for _ in 0..2 {
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            actions.push(rt.take_titlebar_action());
            rt.pointer_up(x, y);
        }
        actions
    };
    assert_eq!(double(&mut rt, 100.0, 10.0), [None, None], "no native band, nothing to do");
    rt.set_native_titlebar_band(28.0);
    assert_eq!(double(&mut rt, 100.0, 10.0), [None, Some(TitlebarAction::DoubleClick)], "the system drags; Rune adds the double click");
    assert_eq!(double(&mut rt, 100.0, 60.0), [None, None], "below the band");
    assert_eq!(double(&mut rt, 20.0, 10.0), [None, None], "a button in the band keeps its presses");
}

#[test]
fn hover_change_spans_descendants_and_holds_through_a_press() {
    type State = (Vec<bool>, u32, u32);
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 200.0)
            .child(el().key("pill").size(100.0, 40.0).on_hover_change(|s: &mut State, inside, _| s.0.push(inside))
                .child(el().key("button").size(40.0, 40.0)
                    .on_press(|s: &mut State| s.1 += 1).on_hover(|s: &mut State| s.2 += 1)))
    }
    let mut rt = Runtime::new((Vec::new(), 0, 0), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(80.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.0, [true]);
    rt.pointer_move(20.0, 20.0);
    rt.pointer_down(20.0, 20.0);
    rt.pointer_up(20.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state, (vec![true], 1, 1), "the child keeps its hover and press; entering it is not a leave");
    rt.pointer_down(20.0, 20.0);
    rt.pointer_move(20.0, 120.0);
    rt.pointer_leave();
    rt.frame(0.0);
    assert_eq!(rt.state.0, [true], "a held press stays inside");
    rt.pointer_up(20.0, 120.0);
    rt.frame(0.0);
    assert_eq!(rt.state.0, [true, false]);
    rt.pointer_move(50.0, 20.0);
    rt.pointer_leave();
    rt.frame(0.0);
    assert_eq!(rt.state.0, [true, false, true, false]);
}

#[test]
fn interactive_overlay_immediately_after_non_interactive_help_suppresses_tooltip() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(200.0, 100.0)
            .child(el().key("label").size(100.0, 40.0).help("Behind cover"))
            .child(el().key("cover").absolute(0.0, 0.0).size(100.0, 40.0).on_press(|_: &mut ()| {}))
            .child(el().key("own").absolute(100.0, 0.0).size(100.0, 40.0).help("Own tip").on_press(|_: &mut ()| {}))
    }
    let mut rt = Runtime::new((), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(50.0, 20.0);
    rt.frame(0.0);
    rt.frame(rt.help_delay + 0.1);
    assert!(rt.help_tooltip().is_none(), "interactive cover at hit index == seq must suppress underlying tooltip");

    // An interactive element with help of its own (at hit index == seq) still shows its tooltip.
    rt.pointer_move(150.0, 20.0);
    rt.frame(0.0);
    rt.frame(rt.help_delay + 0.1);
    assert!(rt.help_tooltip().is_some(), "uncovered interactive element still shows its own tooltip");
}

#[test]
fn pinned_sibling_blocks_pointer_move_hover_change_and_hover_delays_underneath() {
    #[derive(Default)]
    struct State {
        moves: Vec<&'static str>,
        hovers: Vec<(&'static str, bool)>,
        delayed: Vec<bool>,
        parent_moves: usize,
        parent_hovers: Vec<bool>,
    }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(300.0, 200.0)
            // Parent container with an interactive child: moves and hover over the child still reach the parent.
            .child(
                el().key("parent").size(300.0, 100.0)
                    .on_pointer_move_local(|s: &mut State, _, _| s.parent_moves += 1)
                    .on_hover_change(|s: &mut State, inside, _| s.parent_hovers.push(inside))
                    .child(
                        el().key("canvas").size(300.0, 100.0)
                            .on_press(|_: &mut State| {})
                            .on_pointer_move_local(|s: &mut State, _, _| s.moves.push("canvas"))
                            .on_hover_change(|s: &mut State, inside, _| s.hovers.push(("canvas", inside))),
                    ),
            )
            // Delayed-hover row underneath a scrim/card.
            .child(
                el().key("row").size(300.0, 40.0)
                    .on_press(|_: &mut State| {})
                    .hover_delays(std::time::Duration::from_millis(50), std::time::Duration::ZERO)
                    .on_hover_change(|s: &mut State, inside, _| s.delayed.push(inside)),
            )
            // Pinned interactive card over part of canvas and row.
            .child(
                el().key("card").pin(Some(50.0), Some(0.0), None, None).size(100.0, 140.0)
                    .on_press(|_: &mut State| {})
                    .on_pointer_move_local(|s: &mut State, _, _| s.moves.push("card"))
                    .on_hover_change(|s: &mut State, inside, _| s.hovers.push(("card", inside))),
            )
            // Pinned hover/move-only sibling over another part of canvas (no on_press).
            .child(
                el().key("observer").pin(Some(180.0), Some(0.0), None, None).size(80.0, 100.0)
                    .on_pointer_move_local(|s: &mut State, _, _| s.moves.push("observer"))
                    .on_hover_change(|s: &mut State, inside, _| s.hovers.push(("observer", inside))),
            )
    }
    let mut rt = Runtime::new(State::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Over uncovered canvas: parent and canvas both hear move and hover.
    rt.pointer_move(20.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.moves, ["canvas"]);
    assert_eq!(rt.state.hovers, [("canvas", true)]);
    assert_eq!(rt.state.parent_moves, 1);
    assert_eq!(rt.state.parent_hovers, [true]);

    // Move onto pinned interactive card: canvas and parent underneath leave and stop hearing moves.
    rt.state.moves.clear();
    rt.state.hovers.clear();
    rt.pointer_move(80.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.moves, ["card"], "card blocks pointer moves to canvas underneath");
    assert_eq!(rt.state.hovers, [("canvas", false), ("card", true)], "canvas leaves hover under card");
    assert_eq!(rt.state.parent_moves, 1, "parent underneath card stops hearing moves");
    assert_eq!(rt.state.parent_hovers, [true, false], "parent underneath card leaves hover");

    // Hover over the card where it covers the delayed-hover row: row's hover_delays must not fire.
    rt.pointer_move(80.0, 120.0);
    rt.frame(0.1);
    assert!(rt.state.delayed.is_empty(), "delayed hover under pinned card must not fire");

    // Move onto uncovered part of the delayed-hover row: hover_delays fires after 50 ms.
    rt.pointer_move(20.0, 120.0);
    rt.frame(0.06);
    assert_eq!(rt.state.delayed, [true]);

    // Move onto move/hover-only pinned sibling: it receives events and blocks canvas underneath.
    rt.state.moves.clear();
    rt.state.hovers.clear();
    rt.pointer_move(200.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.moves, ["observer"], "observer blocks canvas underneath and is not blocked by it");
    assert_eq!(rt.state.hovers, [("observer", true)]);
}

#[test]
fn hoverable_child_with_own_cursor_overrides_draggable_parent() {
    use crate::el::Cursor;
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(300.0, 260.0)
            .child(col().key("page").size(300.0, 200.0).cursor(Cursor::Grab).on_drag(|_: &mut (), _: &mut crate::Drag, _| {})
                .child(el().key("text-band").size(300.0, 40.0).cursor(Cursor::Text).on_hover_change(|_: &mut (), _, _| {}))
                .child(el().key("default-band").size(300.0, 40.0).cursor(Cursor::Default).on_hover_change(|_: &mut (), _, _| {}))
                .child(el().key("wrapped").size(300.0, 40.0).cursor(Cursor::Crosshair)
                    .child(el().key("wrapped-hover").size(150.0, 40.0).on_hover_change(|_: &mut (), _, _| {})))
                .child(el().key("row").size(300.0, 40.0)
                    .child(el().key("btn").size(100.0, 40.0).cursor(Cursor::Pointer).on_press(|_: &mut ()| {}))
                    .child(el().key("observer").absolute(0.0, 0.0).size(100.0, 40.0).on_pointer_move(|_: &mut (), _| {}))))
            .child(el().key("handle").size(300.0, 60.0).cursor(Cursor::Move)
                .child(el().key("hover-only").size(100.0, 60.0).on_hover_change(|_: &mut (), _, _| {})))
    }
    let mut rt = Runtime::new((), view, (300.0, 260.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 20.0);
    rt.frame(0.016);
    assert_eq!(rt.cursor(), Cursor::Text, "hoverable child with Cursor::Text overrides draggable parent");
    rt.pointer_down(100.0, 20.0);
    assert_eq!(rt.cursor(), Cursor::Grab, "while dragging, the captured draggable parent owns the cursor");
    rt.pointer_up(100.0, 20.0);
    assert_eq!(rt.cursor(), Cursor::Text, "after release, the child's cursor returns");
    rt.pointer_move(100.0, 60.0);
    assert_eq!(rt.cursor(), Cursor::Default, "explicit Cursor::Default overrides parent's Grab");
    rt.pointer_move(50.0, 100.0);
    assert_eq!(rt.cursor(), Cursor::Crosshair, "hoverable child inherits cursor from wrapper inside parent");
    rt.pointer_move(200.0, 100.0);
    assert_eq!(rt.cursor(), Cursor::Grab, "outside the hoverable child, the draggable parent's cursor shows");
    rt.pointer_move(50.0, 140.0);
    assert_eq!(rt.cursor(), Cursor::Pointer, "a pointer-move observer without a cursor does not mask a pressable sibling");
    rt.pointer_move(100.0, 180.0);
    assert_eq!(rt.cursor(), Cursor::Grab, "bare area of the draggable parent shows Grab");
    rt.pointer_move(50.0, 230.0);
    assert_eq!(rt.cursor(), Cursor::Move, "hoverable child with no primary ancestor inherits cursor from its wrapper");
}

#[test]
fn preserve_focus_on_press_keeps_focus_on_focusable_rows_and_on_key_ancestors() {
    #[derive(Default)]
    struct State {
        field: crate::TextEdit,
        row_presses: u32,
        child_presses: u32,
        scroll_btn_presses: u32,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(240.0, 320.0)
            .child(
                crate::text_input(&s.field, crate::TextStyle::new(14.0, 400.0))
                    .key("field")
                    .size(200.0, 30.0)
                    .on_edit(|s: &mut State| &mut s.field),
            )
            .child(
                crate::row().key("kept-row").size(200.0, 60.0).padding(10.0)
                    .focusable(true)
                    .on_key(|_: &mut State, _, _| false)
                    .preserve_focus_on_press(true)
                    .on_press(|s: &mut State| s.row_presses += 1)
                    .child(el().key("bare-child").size(40.0, 40.0))
                    .child(el().key("plain-child").size(40.0, 40.0).on_press(|s: &mut State| s.child_presses += 1))
                    .child(el().key("focusable-child").size(40.0, 40.0).focusable(true)),
            )
            .child(
                crate::row().key("normal-row").size(200.0, 60.0).padding(10.0)
                    .focusable(true)
                    .on_key(|_: &mut State, _, _| false)
                    .child(el().key("normal-child").size(40.0, 40.0).on_press(|_: &mut State| {})),
            )
            .child(
                col().key("scroller").size(200.0, 60.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .focusable(true)
                    .child(
                        el().key("scroll-btn").size(80.0, 30.0)
                            .preserve_focus_on_press(true)
                            .on_press(|s: &mut State| s.scroll_btn_presses += 1),
                    ),
            )
    }
    let mut field = crate::TextEdit::new("draft");
    field.focused = true;
    let mut rt = Runtime::new(State { field, ..State::default() }, view, (240.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let focused_key = |rt: &Runtime<State>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    let click = |rt: &mut Runtime<State>, x: f32, y: f32| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
    };
    let refocus_field = |rt: &mut Runtime<State>| {
        let (x, y) = rt.find("field").unwrap();
        click(rt, x, y);
        assert_eq!(focused_key(rt).as_deref(), Some("field"));
    };

    assert_eq!(focused_key(&rt).as_deref(), Some("field"));

    // Pressing the focusable, on_key row's padding keeps focus on the field.
    let kept_rect = rt.presented("kept-row").unwrap().0;
    click(&mut rt, kept_rect[0] + 5.0, kept_rect[1] + 5.0);
    assert_eq!(rt.state.row_presses, 1, "the press still reaches the row's on_press");
    assert_eq!(focused_key(&rt).as_deref(), Some("field"), "a focusable row with preserve_focus_on_press must not take focus on click");
    assert!(rt.state.field.focused);

    // Pressing a bare child inside the row keeps focus on the field.
    let (x, y) = rt.find("bare-child").unwrap();
    click(&mut rt, x, y);
    assert_eq!(focused_key(&rt).as_deref(), Some("field"), "clicking a bare child must not focus its preserve_focus_on_press ancestor");

    // Pressing an unfocusable interactive child keeps focus instead of focusing the on_key ancestor.
    let (x, y) = rt.find("plain-child").unwrap();
    click(&mut rt, x, y);
    assert_eq!(rt.state.child_presses, 1, "the press still reaches the child's on_press");
    assert_eq!(focused_key(&rt).as_deref(), Some("field"), "clicking an unfocusable child must not focus its on_key ancestor");

    // Pressing a preserve_focus_on_press button inside a focusable scroll region does not focus the scroller.
    let (x, y) = rt.find("scroll-btn").unwrap();
    click(&mut rt, x, y);
    assert_eq!(rt.state.scroll_btn_presses, 1);
    assert_eq!(focused_key(&rt).as_deref(), Some("field"), "clicking a preserve_focus_on_press child must not focus a focusable scroll ancestor");

    // An explicitly focusable descendant inside the row still takes focus normally.
    let (x, y) = rt.find("focusable-child").unwrap();
    click(&mut rt, x, y);
    assert_eq!(focused_key(&rt).as_deref(), Some("focusable-child"), "focusable descendants still take focus on click");

    // Keyboard focus (Tab) still reaches the focusable row.
    refocus_field(&mut rt);
    rt.key(crate::KeyEvent { key: crate::Key::Tab, mods: crate::Mods::default() });
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("kept-row"), "Tab still reaches a focusable preserve_focus_on_press row");

    // Without preserve_focus_on_press, clicking the row's padding or its unfocusable child still focuses the row.
    refocus_field(&mut rt);
    let normal_rect = rt.presented("normal-row").unwrap().0;
    click(&mut rt, normal_rect[0] + 5.0, normal_rect[1] + 5.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("normal-row"), "a row without preserve_focus_on_press still takes focus on click");

    refocus_field(&mut rt);
    let (x, y) = rt.find("normal-child").unwrap();
    click(&mut rt, x, y);
    assert_eq!(focused_key(&rt).as_deref(), Some("normal-row"), "an unfocusable child still focuses its on_key ancestor without preserve_focus_on_press");
}

#[test]
fn observe_pointer_skips_unchanged_moves_and_repaints_hover_groups_links_live_and_drags() {
    struct State {
        live_pos: crate::Live<[f32; 2]>,
        drags: u32,
        moves: u32,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(300.0, 320.0)
            .hover_motion_within(crate::Spring::NONE, crate::Spring::NONE)
            .motion(crate::Spring::NONE)
            // Rows 0..3: 24 pt each, plain hover fill, row 0 has a hover_alone child.
            .child(
                col().w_full()
                    .child(
                        crate::row().key("row-0").size(300.0, 24.0).hover(Color::WHITE.alpha(0.1))
                            .child(el().key("alone").size(40.0, 24.0).hover_alone()),
                    )
                    .child(el().key("row-1").size(300.0, 24.0).hover(Color::WHITE.alpha(0.1)))
                    .child(el().key("row-2").size(300.0, 24.0).hover(Color::WHITE.alpha(0.1))),
            )
            // Hover group with no hover fill of its own (y = 72..102).
            .child(
                el().key("group").size(300.0, 30.0).hover_group()
                    .child(el().key("reveal").size(20.0, 20.0).fill(Color::WHITE).group_opacity(0.0, 1.0)),
            )
            // Shared hover group across two siblings (y = 102..132).
            .child(
                crate::row().size(300.0, 30.0)
                    .child(el().key("share-a").size(100.0, 30.0).hover_group_shared("pair"))
                    .child(el().key("share-b").size(100.0, 30.0).hover_group_shared("pair").hover(Color::WHITE.alpha(0.2))),
            )
            // Selectable text with two links (y = 132..162).
            .child(
                el().key("link-box").size(300.0, 30.0)
                    .child(
                        crate::text("aaaa bbbb cccc")
                            .key("linked")
                            .font(crate::TextStyle::new(14.0, 400.0))
                            .selectable(true)
                            .link(crate::Link::new(0..4, "one"))
                            .link(crate::Link::new(10..14, "two"))
                            .on_link(|_: &mut State, _, _| {}),
                    ),
            )
            // Element asking for pointer positions via on_pointer_move_local + live_only (y = 162..202).
            .child(
                el().key("live-canvas").size(300.0, 40.0)
                    .on_pointer_move_local(|s: &mut State, p, tx| {
                        s.moves += 1;
                        s.live_pos.set([p.x, p.y]);
                        tx.live_only();
                    })
                    .child(el().key("dot").size(8.0, 8.0).fill(Color::WHITE).live_offset(&s.live_pos)),
            )
            // Draggable element (y = 202..242).
            .child(
                el().key("drag-box").size(300.0, 40.0)
                    .on_drag(|s: &mut State, d, _| {
                        if d.phase == crate::Phase::Move { s.drags += 1; }
                    }),
            )
            // Plain text for drag selection (y = 242..272).
            .child(
                crate::text("select me across chars")
                    .key("sel-text")
                    .size(300.0, 30.0)
                    .font(crate::TextStyle::new(14.0, 400.0))
                    .selectable(true),
            )
    }
    let live_pos = crate::Live::new([0.0, 0.0]);
    let mut rt = Runtime::new(
        State { live_pos: live_pos.clone(), drags: 0, moves: 0 },
        view,
        (300.0, 320.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // 1. Sweep across 3 rows of 24 pt, stepping 4 pt at a time (6 moves per row):
    //    only the move entering each row should ask for a repaint (3 total, not 18).
    let mut repaints = 0u32;
    for step in 0..18 {
        let y = 2.0 + step as f32 * 4.0;
        if rt.observe_pointer(100.0, y) {
            repaints += 1;
            rt.frame(0.016);
        }
    }
    assert_eq!(repaints, 3, "staying inside the same hovered row must not repaint; only row transitions repaint");

    // 2. hover_alone child shadows row-0's hover; moving inside it repaints once on enter and once on exit.
    assert!(rt.observe_pointer(100.0, 12.0), "re-enter row-0 outside alone child");
    rt.frame(0.016);
    assert!(rt.observe_pointer(20.0, 12.0), "entering hover_alone child shadows row-0");
    rt.frame(0.016);
    assert!(!rt.observe_pointer(24.0, 12.0), "moving inside hover_alone child changes nothing");
    assert!(rt.observe_pointer(100.0, 12.0), "leaving hover_alone child restores row-0 hover");
    rt.frame(0.016);

    // 3. hover_group without its own hover fill repaints on enter and leave, not on interior moves.
    assert!(rt.observe_pointer(100.0, 84.0), "entering hover_group lights its track");
    rt.frame(0.016);
    assert!(!rt.observe_pointer(110.0, 84.0), "moving inside hover_group changes nothing");
    assert!(rt.observe_pointer(100.0, 300.0), "leaving hover_group clears its track");
    rt.frame(0.016);

    // 4. hover_group_shared lights its peer on enter and clears on leave, not on interior moves.
    assert!(rt.observe_pointer(20.0, 115.0), "entering share-a lights shared peer share-b");
    rt.frame(0.016);
    assert!(!rt.observe_pointer(40.0, 115.0), "moving inside share-a changes nothing");
    assert!(rt.observe_pointer(250.0, 115.0), "leaving share-a clears shared peer");
    rt.frame(0.016);

    // 5. Links inside a single selectable text node repaint on link enter, switch, and leave.
    let linked_idx = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("linked")).unwrap();
    let linked_rect = rt.core().drawn_edges(linked_idx);
    let boxes = rt.core().link_boxes(linked_idx, linked_rect);
    let l0 = boxes.iter().find(|(k, _)| *k == 0).unwrap().1;
    let l1 = boxes.iter().find(|(k, _)| *k == 1).unwrap().1;
    let mid_y = (l0[1] + l0[3]) * 0.5;
    let plain_x = (l0[2] + l1[0]) * 0.5;
    assert!(rt.observe_pointer((l0[0] + l0[2]) * 0.5, mid_y), "entering link 0 repaints");
    rt.frame(0.016);
    assert!(!rt.observe_pointer((l0[0] + l0[2]) * 0.5 + 1.0, mid_y), "moving within link 0 needs no repaint");
    assert!(rt.observe_pointer(plain_x, mid_y), "moving off link 0 onto plain text repaints");
    rt.frame(0.016);
    assert!(!rt.observe_pointer(plain_x + 1.0, mid_y), "moving within plain text needs no repaint");
    assert!(rt.observe_pointer((l1[0] + l1[2]) * 0.5, mid_y), "moving onto link 1 repaints");
    rt.frame(0.016);
    assert!(rt.observe_pointer(100.0, 300.0), "leaving text off link 1 repaints");
    rt.frame(0.016);

    // 6. Element with on_pointer_move_local updating a Live value repaints on every move with 0 commits.
    rt.check_live_only(false);
    let commits_before = rt.stats.commits;
    for step in 0..4 {
        let x = 20.0 + step as f32 * 10.0;
        assert!(rt.observe_pointer(x, 180.0), "on_pointer_move element must repaint on every move");
        rt.frame(0.016);
        assert_eq!(live_pos.get()[0], x);
    }
    assert_eq!(rt.state.moves, 4);
    assert_eq!(rt.stats.commits, commits_before, "live_only pointer moves need no view commits");

    // 7. Held on_drag repaints on every move.
    rt.pointer_move(20.0, 220.0);
    rt.pointer_down(20.0, 220.0);
    rt.frame(0.016);
    for step in 1..=3 {
        assert!(rt.observe_pointer(20.0 + step as f32 * 8.0, 220.0), "held drag must repaint on every move");
        rt.frame(0.016);
    }
    assert_eq!(rt.state.drags, 3);
    rt.pointer_up(44.0, 220.0);
    rt.frame(0.016);

    // 8. Held text selection drag repaints as selection extends.
    rt.pointer_move(4.0, 252.0);
    rt.pointer_down(4.0, 252.0);
    rt.frame(0.016);
    assert!(rt.observe_pointer(90.0, 252.0), "extending a text selection drag must repaint");
    rt.frame(0.016);
    assert!(rt.text_selection().is_some_and(|(_, r)| !r.is_empty()));
    rt.pointer_up(90.0, 252.0);
    rt.frame(0.016);
}
