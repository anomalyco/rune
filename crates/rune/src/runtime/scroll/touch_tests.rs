use super::*;

#[test]
fn a_touch_pan_moves_the_axis_its_first_direction_picks() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let tiles = (0..10).fold(crate::row(), |r, i| r.child(crate::el().key(format!("tile{i}")).w(100.0).h(100.0)));
        let rows = (0..20).fold(crate::col().w_full().child(crate::col().key("strip").w_full().h(100.0).scroll_x().child(tiles)),
            |c, i| c.child(crate::el().key(format!("row{i}")).w_full().h(40.0)));
        crate::col().w_full().h_full().child(crate::col().key("page").w_full().h(300.0)
            .scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
    }
    let mut rt = Runtime::new((), view, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    let id = |rt: &Runtime<()>, key: &str| rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().id;
    let (page, strip) = (id(&rt, "page"), id(&rt, "strip"));

    let vertical = rt.touch_scroll_start(150.0, 50.0);
    assert_eq!(vertical, Some(page));
    assert_eq!(rt.touch_scroll_pan(150.0, 50.0, true, vertical), (Some(strip), true));
    rt.touch_scroll_move(strip, -50.0);
    assert_eq!((rt.scroll.scrolls[&strip].x.offset, rt.scroll.scrolls[&page].offset), (50.0, 0.0));
    rt.touch_scroll_end(Some(strip), 0.0);

    // A vertical pan over the strip passes through to the page.
    let vertical = rt.touch_scroll_start(150.0, 50.0);
    assert_eq!(rt.touch_scroll_pan(150.0, 50.0, false, vertical), (Some(page), false));
    rt.touch_scroll_move(page, -30.0);
    assert_eq!((rt.scroll.scrolls[&strip].x.offset, rt.scroll.scrolls[&page].offset), (50.0, 30.0));
    rt.touch_scroll_end(Some(page), 0.0);

    // A touch on the strip while it springs back past its left edge holds it there, until a
    // tap lets it go or a sideways pan carries on from where it was caught.
    let held = |rt: &Runtime<()>| rt.pointer.touch_scroll().map(|t| (t.id(), t.horizontal(), !t.coasting(), t.origin()));
    rt.scroll.scrolls.get_mut(&strip).unwrap().x.offset = -20.0;
    *rt.pointer.touch_scroll_mut() = Some(TouchScroll::Coasting { id: strip, horizontal: true });
    let vertical = rt.touch_scroll_start(150.0, 50.0);
    assert_eq!(held(&rt), Some((strip, true, true, Some(-20.0))), "caught, not handed to the page");
    rt.touch_scroll_end(vertical, 0.0);
    assert_eq!(held(&rt), Some((strip, true, false, None)), "a tap lets it go back");
    let vertical = rt.touch_scroll_start(150.0, 50.0);
    assert_eq!(rt.touch_scroll_pan(150.0, 50.0, true, vertical), (Some(strip), true));
    rt.touch_scroll_move(strip, 10.0);
    assert!(rt.scroll.scrolls[&strip].x.offset < -20.0 * 0.55, "still rubber-banding: {}", rt.scroll.scrolls[&strip].x.offset);
    rt.touch_scroll_end(Some(strip), 0.0);

    // Caught there, a vertical pan lets the strip go and moves the page.
    rt.touch_scroll_start(150.0, 50.0);
    assert_eq!(rt.touch_scroll_pan(150.0, 50.0, false, vertical), (Some(page), false));
    assert_eq!(held(&rt).map(|h| (h.0, h.1, h.2)), Some((page, false, true)));
}

#[test]
fn edge_return_is_frame_rate_independent() {
    let one = rebound(-50.0, -400.0, 0.0, 0.5);
    let mut many = (-50.0, -400.0);
    for _ in 0..60 { many = rebound(many.0, many.1, 0.0, 0.5 / 60.0); }
    assert!((one.0 - many.0).abs() < 0.001);
    assert!((one.1 - many.1).abs() < 0.01);
    assert!(one.0 < 0.0 && one.0 > -1.0);
}

fn run(state: &mut Option<TouchScroll>, event: TouchEvent) -> Vec<TouchEffect> {
    step(state, event).into_iter().collect()
}

#[test]
fn a_down_holds_the_viewport_under_it_and_stops_its_motion() {
    let (page, strip) = (NodeId(1), NodeId(2));
    let mut state = None;
    let effects = run(&mut state, TouchEvent::Down { across: Some((strip, 5.0)), vertical: Some((page, 7.0)) });
    assert_eq!(state, Some(TouchScroll::Held { id: page, origin: 7.0, horizontal: false }));
    assert_eq!(effects, vec![TouchEffect::StopAcross(strip), TouchEffect::Halt(page)]);

    // A horizontal viewport still settling is caught where the finger lands on it.
    let mut state = Some(TouchScroll::Coasting { id: strip, horizontal: true });
    run(&mut state, TouchEvent::Down { across: Some((strip, -20.0)), vertical: Some((page, 7.0)) });
    assert_eq!(state, Some(TouchScroll::Held { id: strip, origin: -20.0, horizontal: true }), "caught, not handed to the page");

    // A touch outside every viewport leaves the settling one alone.
    let mut state = Some(TouchScroll::Coasting { id: strip, horizontal: false });
    run(&mut state, TouchEvent::Down { across: None, vertical: None });
    assert_eq!(state, Some(TouchScroll::Coasting { id: strip, horizontal: false }));
}

#[test]
fn a_pan_turns_a_held_viewport_into_a_drag_with_the_offset_it_was_caught_at() {
    let (page, strip) = (NodeId(1), NodeId(2));
    let mut state = Some(TouchScroll::Held { id: page, origin: 7.0, horizontal: false });
    run(&mut state, TouchEvent::Pan { horizontal: false, across: None, vertical: Some((page, 9.0)) });
    assert_eq!(state, Some(TouchScroll::Dragging { id: page, origin: 7.0, horizontal: false }), "keeps the offset the finger went down at");

    // A horizontal pan takes the horizontal viewport under the finger, from its offset now.
    let mut state = Some(TouchScroll::Held { id: page, origin: 7.0, horizontal: false });
    run(&mut state, TouchEvent::Pan { horizontal: true, across: Some((strip, 4.0)), vertical: Some((page, 9.0)) });
    assert_eq!(state, Some(TouchScroll::Dragging { id: strip, origin: 4.0, horizontal: true }));

    // A finger down on a horizontal viewport with no vertical one still pans it.
    let mut state = None;
    run(&mut state, TouchEvent::Pan { horizontal: true, across: Some((strip, 4.0)), vertical: None });
    assert_eq!(state, Some(TouchScroll::Dragging { id: strip, origin: 4.0, horizontal: true }));

    // A vertical pan lets a caught horizontal viewport go to the vertical one.
    let mut state = Some(TouchScroll::Held { id: strip, origin: -20.0, horizontal: true });
    run(&mut state, TouchEvent::Pan { horizontal: false, across: Some((strip, -20.0)), vertical: Some((page, 9.0)) });
    assert_eq!(state, Some(TouchScroll::Dragging { id: page, origin: 9.0, horizontal: false }));
    // With no vertical viewport under it, the horizontal one is left settling.
    let mut state = Some(TouchScroll::Held { id: strip, origin: -20.0, horizontal: true });
    run(&mut state, TouchEvent::Pan { horizontal: false, across: None, vertical: None });
    assert_eq!(state, Some(TouchScroll::Coasting { id: strip, horizontal: true }));
}

#[test]
fn a_lift_starts_the_coast_and_flings_only_the_viewport_it_held() {
    let (page, strip) = (NodeId(1), NodeId(2));
    let mut state = Some(TouchScroll::Dragging { id: page, origin: 0.0, horizontal: false });
    let effects = run(&mut state, TouchEvent::Up { id: Some(page), velocity: -1200.0 });
    assert_eq!(state, Some(TouchScroll::Coasting { id: page, horizontal: false }));
    assert_eq!(effects, vec![TouchEffect::Fling { id: page, horizontal: false, velocity: -1200.0 }]);

    // A lift from another viewport still lets this one go, without a fling.
    let mut state = Some(TouchScroll::Dragging { id: page, origin: 0.0, horizontal: false });
    assert_eq!(run(&mut state, TouchEvent::Up { id: Some(strip), velocity: 0.0 }), vec![]);
    assert_eq!(state, Some(TouchScroll::Coasting { id: page, horizontal: false }));
}

#[test]
fn a_move_and_a_frame_tick_go_through_step() {
    let page = NodeId(1);
    let mut state = Some(TouchScroll::Dragging { id: page, origin: 5.0, horizontal: false });
    assert_eq!(
        run(&mut state, TouchEvent::Move { id: page, d: -30.0 }),
        vec![TouchEffect::Pan { id: page, origin: 5.0, horizontal: false, d: -30.0 }]
    );
    assert_eq!(
        run(&mut state, TouchEvent::Tick { id: page, horizontal: false, overflow: 40.0, dt: 1.0 / 60.0 }),
        vec![TouchEffect::Hold { id: page, horizontal: false, overflow: 40.0 }],
        "a held viewport only has its bounds set"
    );
    let mut state = Some(TouchScroll::Coasting { id: page, horizontal: false });
    assert_eq!(
        run(&mut state, TouchEvent::Tick { id: page, horizontal: false, overflow: 40.0, dt: 1.0 / 60.0 }),
        vec![TouchEffect::Coast { id: page, horizontal: false, overflow: 40.0, dt: 1.0 / 60.0 }],
        "a coasting one is stepped"
    );

    // A move of a viewport the finger no longer drags, or a tick for another axis, asks for nothing.
    let mut state = Some(TouchScroll::Held { id: page, origin: 5.0, horizontal: false });
    assert_eq!(run(&mut state, TouchEvent::Move { id: page, d: -30.0 }), vec![]);
    assert_eq!(run(&mut state, TouchEvent::Tick { id: page, horizontal: true, overflow: 40.0, dt: 0.1 }), vec![]);
}

#[test]
fn a_coast_reaches_rest_and_nothing_else_is_cleared() {
    let page = NodeId(1);
    let mut state = Some(TouchScroll::Coasting { id: page, horizontal: false });
    run(&mut state, TouchEvent::Settled { moving: true });
    assert_eq!(state, Some(TouchScroll::Coasting { id: page, horizontal: false }), "still moving");
    run(&mut state, TouchEvent::Settled { moving: false });
    assert_eq!(state, None, "came to rest");

    let mut state = Some(TouchScroll::Dragging { id: page, origin: 0.0, horizontal: false });
    run(&mut state, TouchEvent::Settled { moving: false });
    assert_eq!(state, Some(TouchScroll::Dragging { id: page, origin: 0.0, horizontal: false }), "a drag never settles itself");
}
