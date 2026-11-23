use super::*;
use crate::{Env, Mods, PhysicalInput, PhysicalKeyCode, el};

#[derive(Default)]
struct State { ups: Vec<crate::Key>, swallow: bool, zoom: Vec<f32>, from_wheel: Vec<bool> }

fn view(_: &State, _: &mut crate::Cx<State>) -> El {
    el().w_full().h_full()
        .on_key_up(|s: &mut State, e, _| { s.ups.push(e.key.clone()); true })
        .on_magnify(|s: &mut State, e, _| { s.zoom.push(e.delta); s.from_wheel.push(e.wheel); true })
        .child(el().key("field").size(40.0, 40.0).focusable(true).on_physical_input(|s: &mut State, _, _| s.swallow))
}

#[test]
fn a_physical_release_carries_its_logical_key_to_on_key_up_after_the_focused_handler() {
    let mut rt = Runtime::new(State::default(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.wants_key_up());
    let release = PhysicalInput::Key { key: PhysicalKeyCode::Space, mods: Mods::default(), pressed: false, repeat: false };
    let space = KeyEvent { key: crate::Key::Text(" ".into()), mods: Mods::default() };
    assert!(rt.physical_with_fallback(release, Some(space.clone())));
    rt.frame(0.0);
    assert_eq!(rt.state.ups, [space.key.clone()]);

    // A focused `on_physical_input` that takes the release keeps it from `on_key_up`.
    let (x, y) = rt.find("field").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.state.swallow = true;
    rt.physical_with_fallback(release, Some(space.clone()));
    rt.frame(0.0);
    assert_eq!(rt.state.ups.len(), 1);
    rt.state.swallow = false;
    rt.physical_with_fallback(release, Some(space));
    rt.frame(0.0);
    assert_eq!(rt.state.ups.len(), 2);
}

#[derive(Default)]
struct Keys { log: Vec<&'static str> }

fn stage(s: &Keys, _: &mut crate::Cx<Keys>) -> El {
    let _ = s;
    el().w_full().h_full()
        .child(el().key("stage").size(40.0, 40.0).on_key_up(|s: &mut Keys, _, _| { s.log.push("stage"); true }))
        .child(el().key("other").size(40.0, 40.0).focusable(true))
}

fn up() -> KeyEvent { KeyEvent { key: crate::Key::Text("a".into()), mods: Mods::default() } }

#[test]
fn an_unfocused_release_reaches_a_nested_handler() {
    let mut rt = Runtime::new(Keys::default(), stage, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.wants_key_up());
    rt.key_up(up());
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["stage"]);
}

fn pair(_: &Keys, _: &mut crate::Cx<Keys>) -> El {
    el().w_full().h_full()
        .child(el().key("a").size(40.0, 40.0).focusable(true).on_key_up(|s: &mut Keys, _, _| { s.log.push("a"); true }))
        .child(el().key("b").size(40.0, 40.0).focusable(true).on_key_up(|s: &mut Keys, _, _| { s.log.push("b"); true }))
}

#[test]
fn a_release_goes_to_the_element_focused_at_the_press_even_after_focus_moves() {
    let mut rt = Runtime::new(Keys::default(), pair, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let click = |rt: &mut Runtime<Keys>, k: &str| { let (x, y) = rt.find(k).unwrap(); rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0); };
    click(&mut rt, "a");
    rt.key(up());
    rt.frame(0.0);
    click(&mut rt, "b");
    rt.key_up(up());
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["a"]);
}

fn modal_view(_: &Keys, _: &mut crate::Cx<Keys>) -> El {
    el().w_full().h_full()
        .on_key_up(|s: &mut Keys, _, _| { s.log.push("root"); true })
        .child(el().key("under").size(40.0, 40.0).on_key_up(|s: &mut Keys, _, _| { s.log.push("under"); true }))
        .child(el().key("sheet").absolute(50.0, 50.0).size(40.0, 40.0).modal(true, |_: &mut Keys| {})
            .child(el().key("inside").size(10.0, 10.0).on_key_up(|s: &mut Keys, _, _| { s.log.push("inside"); true })))
}

#[test]
fn an_unfocused_release_in_a_modal_stays_inside_it() {
    let mut rt = Runtime::new(Keys::default(), modal_view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key_up(up());
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["inside"]);
}

#[test]
fn a_ctrl_wheel_pinches_only_where_a_magnify_handler_is() {
    let mut rt = Runtime::new(State::default(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // The wheel turned away from the user moves content down (positive dy) and zooms in.
    assert!(rt.pinch_wheel(50.0, 50.0, 20.0));
    rt.frame(0.0);
    assert_eq!(rt.state.zoom, [20.0 / PINCH_WHEEL]);
    assert_eq!(rt.state.from_wheel, [true], "a handler tells Ctrl + wheel from a pinch gesture");
    assert!(!rt.pinch_wheel(500.0, 50.0, -20.0), "off the window nothing takes it");
}

#[derive(Default)]
struct Wheeled { heard: Vec<f32> }

fn wheeling(_: &Wheeled, _: &mut crate::Cx<Wheeled>) -> El {
    el().w_full().h_full()
        .child(el().key("list").w_full().h(300.0).scroll(crate::ScrollSpec { stick_bottom: false })
            .child(el().w_full().h(1000.0)
                .child(el().key("takes").absolute(100.0, 40.0).size(40.0, 40.0)
                    .on_wheel(|s: &mut Wheeled, e: crate::WheelEvent, _: &mut Tx| { s.heard.push(e.x); true }))))
}

/// Deltas queued before the platform began the next gesture finish with the owner the
/// gesture picked: the one outside the handler still reaches it, rather than scrolling the
/// viewport under the pointer.
#[test]
fn deltas_queued_before_a_new_gesture_began_finish_with_the_gesture_s_owner() {
    let mut rt = Runtime::new(Wheeled::default(), wheeling, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(110.0, 50.0));
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(290.0, 50.0));
    rt.wheel_began();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.heard, [10.0, 190.0], "both deltas in the handler's own points");
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    assert_eq!(rt.scroll.scrolls[&id].offset, 0.0, "the whole gesture stays with the handler");
}

/// A delta queued over the handler, before it has the gesture, still reaches it when a newer
/// gesture scrolls at once outside every handler: the newer one's own delta scrolls once.
#[test]
fn a_queued_delta_picks_its_handler_before_a_newer_gesture_s_scroll() {
    let mut rt = Runtime::new(Wheeled::default(), wheeling, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(110.0, 50.0));
    rt.wheel_began();
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(290.0, 50.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.heard, [10.0], "the queued delta reaches the handler it was offered to");
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    assert_eq!(rt.scroll.scrolls[&id].offset, 10.0, "only the newer gesture's own delta scrolled");
}

/// A delta queued for the handler that has the gesture keeps reaching it when the gesture
/// ends and the next one scrolls at once, before the frame drains the queue.
#[test]
fn a_queued_delta_keeps_its_owner_when_the_next_gesture_scrolls_first() {
    let mut rt = Runtime::new(Wheeled::default(), wheeling, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    // The handler takes the gesture.
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(110.0, 50.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.heard, [10.0]);
    // A delta outside it is queued for that owner; the gesture ends, and the next one
    // scrolls at once outside every handler.
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(290.0, 50.0));
    rt.wheel_ended();
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(290.0, 50.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.heard, [10.0, 190.0], "the queued delta still reaches the gesture's owner");
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    assert_eq!(rt.scroll.scrolls[&id].offset, 10.0, "only the newer gesture's own delta scrolled");
}

#[derive(Default)]
struct Two { heard: Vec<&'static str> }

fn two_handlers(_: &Two, _: &mut crate::Cx<Two>) -> El {
    el().w_full().h_full()
        .child(el().key("list").w_full().h(300.0).scroll(crate::ScrollSpec { stick_bottom: false })
            .child(el().w_full().h(1000.0)
                .child(el().key("a").absolute(20.0, 40.0).size(40.0, 40.0)
                    .on_wheel(|s: &mut Two, _: crate::WheelEvent, _: &mut Tx| { s.heard.push("a"); true }))
                .child(el().key("b").absolute(200.0, 40.0).size(40.0, 40.0)
                    .on_wheel(|s: &mut Two, _: crate::WheelEvent, _: &mut Tx| { s.heard.push("b"); true }))))
}

/// A gesture whose queued deltas have all run gives its owner's place up: the next gesture's
/// queued delta still reaches that gesture's handler instead of scrolling.
#[test]
fn a_drained_gesture_gives_its_owner_up_to_the_next_queued_one() {
    let mut rt = Runtime::new(Two::default(), two_handlers, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    // A delta over `a`, then a gesture begins and a delta over `b`: each reaches its own.
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(30.0, 50.0));
    rt.wheel_began();
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(210.0, 50.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.heard, ["a", "b"]);
    // `b` owns the gesture now; a delta outside it is queued for `b`, and the next gesture
    // begins before the frame drains it.
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(290.0, 50.0));
    rt.wheel_began();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.heard, ["a", "b", "b"], "the queued delta still reaches b");
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    assert_eq!(rt.scroll.scrolls[&id].offset, 0.0, "every delta reached a handler");
}

/// With a delta queued for a handler under one gesture and another under the next, each
/// reaches its own gesture's handler, however many gestures began before the frame.
#[test]
fn a_delta_queued_for_each_of_two_gestures_reaches_its_own_handler() {
    let mut rt = Runtime::new(Two::default(), two_handlers, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(30.0, 50.0));
    rt.wheel_began();
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(210.0, 50.0));
    rt.wheel_began();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.heard, ["a", "b"], "each delta goes to the handler its own gesture found");
}

/// Three gestures begin inside one frame, each with a delta queued for the handler under the
/// pointer: each delta reaches its own gesture's handler.
#[test]
fn three_gestures_queued_before_one_frame_each_reach_their_own_handler() {
    let mut rt = Runtime::new(Two::default(), two_handlers, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(30.0, 50.0));
    rt.wheel_began();
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(210.0, 50.0));
    rt.wheel_began();
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(30.0, 50.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.heard, ["a", "b", "a"], "each delta goes to the handler its own gesture found");
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    assert_eq!(rt.scroll.scrolls[&id].offset, 0.0, "every delta reached a handler");
}

/// The third gesture scrolls at once outside every handler: the deltas queued for the first
/// two gestures still reach their own handlers, and only the third scrolls.
#[test]
fn a_third_gesture_scrolling_before_the_frame_keeps_both_queued_owners() {
    let mut rt = Runtime::new(Two::default(), two_handlers, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(30.0, 50.0));
    rt.wheel_began();
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(210.0, 50.0));
    rt.wheel_began();
    rt.wheel_event(crate::WheelEvent::new(0.0, -10.0).at(290.0, 50.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.heard, ["a", "b"], "the first two gestures' deltas still reach their handlers");
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    assert_eq!(rt.scroll.scrolls[&id].offset, 10.0, "only the third gesture's own delta scrolled");
}
