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
    assert!(rt.pinch_wheel(50.0, 50.0, 20.0, None));
    rt.frame(0.0);
    assert_eq!(rt.state.zoom, [20.0 / PINCH_WHEEL]);
    assert_eq!(rt.state.from_wheel, [true], "a handler tells Ctrl + wheel from a pinch gesture");
    assert!(!rt.pinch_wheel(500.0, 50.0, -20.0, None), "off the window nothing takes it");
}

#[test]
fn ctrl_wheel_line_ticks_scale_by_chrome_step_and_round_trip() {
    let mut rt = Runtime::new(State::default(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Line-delta notches use 1.2^ticks - 1 regardless of pixel dy.
    assert!(rt.pinch_wheel(50.0, 50.0, 100.0, Some(1.0)));
    assert!(rt.pinch_wheel(50.0, 50.0, -100.0, Some(-1.0)));
    assert!(!rt.pinch_wheel(50.0, 50.0, 100.0, Some(0.0)), "zero vertical ticks does not pinch");
    assert!(!rt.pinch_wheel(500.0, 50.0, 100.0, Some(1.0)), "off the window nothing takes it");
    rt.frame(0.0);
    assert_eq!(rt.state.zoom.len(), 2);
    assert_eq!(rt.state.from_wheel, [true, true]);
    assert!((rt.state.zoom[0] - 0.2).abs() < 1e-5, "one forward notch is +0.2, got {}", rt.state.zoom[0]);
    assert!(((1.0 + rt.state.zoom[0]) * (1.0 + rt.state.zoom[1]) - 1.0).abs() < 1e-5, "forward + backward round-trips to 1.0");
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

/// An ancestor above a scroll view hears `on_wheel` (bubbling past scroll views as DOM `wheel`
/// does), and a modifier change mid-run breaks the unphased wheel gesture immediately.
#[test]
fn wheel_routing_ancestor_on_wheel_hears_deltas_past_scroll_view_and_on_modifier_change() {
    #[derive(Default)]
    struct S { zoom: u32, plain: u32 }
    fn view(_: &S, _: &mut crate::Cx<S>) -> El {
        crate::col().w_full().h_full()
            .on_wheel(|s: &mut S, e: crate::WheelEvent, _| {
                if e.mods.ctrl && e.mods.shift { s.zoom += 1; true } else { s.plain += 1; false }
            })
            .child(crate::col().key("vp").size(300.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false })
                .child(el().w_full().h(4000.0)))
    }
    let mut rt = Runtime::new(S::default(), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let vp = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("vp")).unwrap().id;
    let ctrl_shift = Mods { ctrl: true, cmd: true, shift: true, ..Mods::default() };

    // Plain wheel bubbles to ancestor (returns false) and scrolls the viewport.
    rt.wheel_event(crate::WheelEvent::new(0.0, -40.0).at(150.0, 100.0));
    rt.frame(0.016);
    assert_eq!((rt.state.plain, rt.state.zoom), (1, 0));
    assert_eq!(rt.scroll.scrolls[&vp].offset, 40.0);

    // Immediately (within WHEEL_IDLE 0.1 s), Ctrl+Shift+wheel breaks the scroll gesture and
    // reaches the ancestor above the scroll view without scrolling.
    rt.wheel_event(crate::WheelEvent::new(0.0, -120.0).at(150.0, 100.0).mods(ctrl_shift));
    rt.frame(0.016);
    assert_eq!(rt.state.zoom, 1, "ancestor above scroll view hears Ctrl+Shift+wheel immediately");
    assert_eq!(rt.scroll.scrolls[&vp].offset, 40.0, "viewport did not scroll on claimed Ctrl+Shift+wheel");

    // Releasing modifiers immediately starts a new gesture that scrolls again.
    rt.wheel_event(crate::WheelEvent::new(0.0, -30.0).at(150.0, 100.0));
    rt.frame(0.016);
    assert_eq!(rt.scroll.scrolls[&vp].offset, 70.0, "plain wheel right after scrolls again");
}

/// When a handler holding a gesture declines a later delta (returns false), the delta is not
/// dropped: it bubbles to outer `on_wheel` ancestors and, if none take it, scrolls as usual.
#[test]
fn wheel_routing_declined_delta_from_owning_handler_bubbles_and_scrolls() {
    #[derive(Default)]
    struct S { inner_takes: bool, outer_takes: bool, log: Vec<&'static str> }
    fn view(_: &S, _: &mut crate::Cx<S>) -> El {
        crate::col().w_full().h_full()
            .on_wheel(|s: &mut S, _, _| { s.log.push("outer"); s.outer_takes })
            .child(crate::col().key("list").size(300.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false })
                .child(el().w_full().h(2000.0)
                    .child(el().key("inner").size(200.0, 100.0)
                        .on_wheel(|s: &mut S, _, _| { s.log.push("inner"); s.inner_takes }))))
    }
    let mut rt = Runtime::new(S { inner_takes: true, outer_takes: true, log: Vec::new() }, view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    let list = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;

    // Tick 1: inner claims the gesture.
    rt.wheel_event(crate::WheelEvent::new(0.0, -20.0).at(50.0, 50.0));
    rt.frame(0.016);
    assert_eq!(rt.state.log, ["inner"]);
    assert_eq!(rt.scroll.scrolls[&list].offset, 0.0);

    // Tick 2 (same gesture): inner declines -> bubbles to outer, which claims it.
    rt.state.inner_takes = false;
    rt.wheel_event(crate::WheelEvent::new(0.0, -20.0).at(50.0, 50.0));
    rt.frame(0.016);
    assert_eq!(rt.state.log, ["inner", "inner", "outer"]);
    assert_eq!(rt.scroll.scrolls[&list].offset, 0.0);

    // Tick 3 (same gesture): outer declines -> falls through to scrolling instead of dropping.
    rt.state.outer_takes = false;
    rt.wheel_event(crate::WheelEvent::new(0.0, -25.0).at(50.0, 50.0));
    rt.frame(0.016);
    assert_eq!(rt.state.log, ["inner", "inner", "outer", "outer"]);
    assert_eq!(rt.scroll.scrolls[&list].offset, 25.0, "declined delta scrolls instead of being dropped");
}

/// A phased vertical trackpad gesture stays latched to the vertical viewport it began on even
/// when the pointer moves across sibling columns (and through its momentum), as Chromium latches
/// phased scroll gestures without applying mouse-wheel slop.
#[test]
fn wheel_routing_phased_vertical_gesture_latches_across_columns_and_momentum() {
    fn tall(key: &str) -> El {
        crate::col().key(key.to_owned()).size(400.0, 300.0).scroll(crate::ScrollSpec { stick_bottom: false })
            .child(el().w_full().h(4000.0))
    }
    fn strip(_: &(), _: &mut crate::Cx<()>) -> El {
        crate::row().key("strip").size(800.0, 300.0).scroll_x().child(
            crate::row().w(1200.0).h_full().child(tall("c0")).child(tall("c1")).child(tall("c2"))
        )
    }
    let mut rt = Runtime::new((), strip, (800.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let id = |k: &str| rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(k)).unwrap().id;
    let (c0, c1, strip_id) = (id("c0"), id("c1"), id("strip"));

    let phases = [crate::GesturePhase::Began, crate::GesturePhase::Moved, crate::GesturePhase::Moved, crate::GesturePhase::Ended];
    for (x, phase) in [300.0, 390.0, 450.0, 500.0].into_iter().zip(phases) {
        rt.wheel_event(crate::WheelEvent::new(0.0, -40.0).at(x, 150.0).phase(phase).precise(true));
        rt.frame(0.016);
    }
    assert_eq!((rt.scroll.scrolls[&c0].offset, rt.scroll.scrolls[&c1].offset), (160.0, 0.0), "whole phased gesture stays on c0");

    // Momentum immediately following Ended stays latched to c0 even over c1.
    rt.wheel_event(crate::WheelEvent::new(0.0, -20.0).at(550.0, 150.0).phase(crate::GesturePhase::Began).precise(true));
    rt.wheel_event(crate::WheelEvent::new(0.0, 0.0).at(550.0, 150.0).phase(crate::GesturePhase::Ended).precise(true));
    rt.frame(0.016);
    assert_eq!((rt.scroll.scrolls[&c0].offset, rt.scroll.scrolls[&c1].offset), (180.0, 0.0), "momentum stays on c0");

    // A Shift + vertical mouse wheel in Runtime::wheel_event turns horizontal and scrolls strip.
    let shift = Mods { shift: true, ..Mods::default() };
    rt.wheel_event(crate::WheelEvent::new(0.0, -120.0).at(600.0, 150.0).mods(shift).precise(false));
    rt.frame(0.016);
    assert_eq!((rt.scroll.scrolls[&strip_id].x.offset, rt.scroll.scrolls[&c1].offset), (120.0, 0.0), "Shift + mouse wheel scrolls strip sideways");

    // A second Shift + wheel tick within WHEEL_IDLE continues horizontal scrolling; releasing
    // Shift immediately scrolls c1 vertically.
    rt.wheel_event(crate::WheelEvent::new(0.0, -60.0).at(600.0, 150.0).mods(shift).precise(false));
    rt.frame(0.016);
    assert_eq!(rt.scroll.scrolls[&strip_id].x.offset, 180.0);
    rt.wheel_event(crate::WheelEvent::new(0.0, -50.0).at(600.0, 150.0).precise(false));
    rt.frame(0.016);
    assert_eq!((rt.scroll.scrolls[&strip_id].x.offset, rt.scroll.scrolls[&c1].offset), (180.0, 50.0), "plain wheel scrolls c1");
}
