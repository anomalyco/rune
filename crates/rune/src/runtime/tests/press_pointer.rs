use super::*;
use crate::el::{Cursor, PointerEvent, row, text, text_input};
use crate::runtime::motion::props::{P_HOVER, P_PRESS, P_PRESS_SCALE};

#[test]
fn press_cursor_is_off_by_default_and_handles_precedence_disabled_and_text_fields() {
    struct State {
        enabled: bool,
        field: crate::edit::TextEdit,
    }
    fn view(s: &State, cx: &mut Cx<State>) -> El {
        if s.enabled {
            cx.press_cursor(Cursor::Pointer);
        }
        col().size(300.0, 360.0)
            .child(el().key("press").size(200.0, 30.0).on_press(|_: &mut State| {}))
            .child(el().key("double").size(200.0, 30.0).on_double_press(|_: &mut State| {}))
            .child(el().key("explicit").size(200.0, 30.0).cursor(Cursor::Crosshair).on_press(|_: &mut State| {}))
            .child(
                el().key("grab-bar").size(200.0, 60.0).cursor(Cursor::Grab)
                    .child(
                        col().size(200.0, 60.0)
                            .child(el().key("in-grab").size(100.0, 30.0).on_press(|_: &mut State| {}))
                            .child(el().key("plain-in-grab").size(100.0, 30.0).hit()),
                    ),
            )
            .child(
                el().key("disabled-press").size(200.0, 30.0).disabled(true).on_press(|_: &mut State| {}),
            )
            .child(
                el().key("aria-disabled-press").size(200.0, 30.0).aria_disabled(true).on_press(|_: &mut State| {}),
            )
            .child(
                el().key("disabled-explicit").size(200.0, 30.0).disabled(true).cursor(Cursor::NotAllowed).on_press(|_: &mut State| {}),
            )
            .child(
                el().key("card").size(200.0, 30.0).on_press(|_: &mut State| {})
                    .child(el().key("disabled-child-btn").size(100.0, 30.0).role("button").disabled(true).hit()),
            )
            .child(
                text_input(&s.field, crate::tok::TextStyle::new(14.0, 400.0))
                    .key("field").size(200.0, 30.0).on_edit(|s: &mut State| &mut s.field),
            )
            .child(
                el().key("selectable-wrap").size(200.0, 30.0).on_press(|_: &mut State| {})
                    .child(text("Copy me").key("sel").selectable(true)),
            )
            .child(el().key("plain").size(200.0, 30.0).hit())
            .child(el().key("inert-press").size(200.0, 30.0).inert().on_press(|_: &mut State| {}))
    }

    let mut rt = Runtime::new(
        State { enabled: false, field: crate::edit::TextEdit::new("abc") },
        view,
        (300.0, 360.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let at = |rt: &mut Runtime<State>, key: &str| {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing {key}"));
        rt.pointer_move(x, y);
        rt.cursor()
    };

    // Off by default: existing cursor rules are completely unchanged.
    assert_eq!(at(&mut rt, "press"), Cursor::Default);
    assert_eq!(at(&mut rt, "double"), Cursor::Default);
    assert_eq!(at(&mut rt, "explicit"), Cursor::Crosshair);
    assert_eq!(at(&mut rt, "in-grab"), Cursor::Grab, "without press_cursor, inherits ancestor Grab");
    assert_eq!(at(&mut rt, "field"), Cursor::Text);

    // Enable Cx::press_cursor(Cursor::Pointer).
    rt.update(|s, _| s.enabled = true);
    rt.frame(0.0);

    assert_eq!(at(&mut rt, "press"), Cursor::Pointer);
    assert_eq!(at(&mut rt, "double"), Cursor::Pointer);
    assert_eq!(at(&mut rt, "explicit"), Cursor::Crosshair, "explicit .cursor() wins over press_cursor");
    assert_eq!(at(&mut rt, "in-grab"), Cursor::Pointer, "press_cursor wins over inherited ancestor Grab");
    assert_eq!(at(&mut rt, "plain-in-grab"), Cursor::Grab, "non-pressable child still inherits Grab");
    assert_eq!(at(&mut rt, "disabled-press"), Cursor::Default, "disabled(true) keeps default arrow");
    assert_eq!(at(&mut rt, "aria-disabled-press"), Cursor::Default, "aria_disabled(true) keeps default arrow");
    assert_eq!(at(&mut rt, "disabled-explicit"), Cursor::NotAllowed, "explicit cursor on disabled still wins");
    assert_eq!(at(&mut rt, "disabled-child-btn"), Cursor::Default, "disabled button inside pressable card does not inherit press_cursor");
    assert_eq!(at(&mut rt, "field"), Cursor::Text, "text_input keeps I-beam");
    assert_eq!(at(&mut rt, "sel"), Cursor::Text, "selectable text keeps I-beam");
    assert_eq!(at(&mut rt, "plain"), Cursor::Default, "plain element stays default");
    assert_eq!(at(&mut rt, "inert-press"), Cursor::Default, "inert pressable element does not show press_cursor");

    // Held press retains press_cursor while captured.
    let (px, py) = rt.find("press").unwrap();
    rt.pointer_move(px, py);
    rt.pointer_down(px, py);
    let (plain_x, plain_y) = rt.find("plain").unwrap();
    rt.pointer_move(plain_x, plain_y);
    assert_eq!(rt.cursor(), Cursor::Pointer, "held press keeps press_cursor");
    rt.pointer_up(plain_x, plain_y);
    assert_eq!(rt.cursor(), Cursor::Default);

    // Turning Cx::press_cursor off on a later commit reverts to default.
    rt.update(|s, _| s.enabled = false);
    rt.frame(0.0);
    assert_eq!(at(&mut rt, "press"), Cursor::Default);
    assert_eq!(at(&mut rt, "in-grab"), Cursor::Grab);
}

#[test]
fn press_point_reports_local_and_window_coords_across_press_handlers_and_none_for_keyboard_and_a11y() {
    #[derive(Default)]
    struct State {
        events: Vec<(&'static str, Option<PointerEvent>, bool, crate::PointerButton)>,
        outside_press: Vec<(&'static str, Option<PointerEvent>)>,
    }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(300.0, 260.0).padding(10.0).gap(10.0)
            .child(
                el().key("btn").size(120.0, 40.0)
                    .focusable(true).press_on_return()
                    .on_press_tx(|s: &mut State, tx| {
                        s.events.push(("press", tx.press_point(), tx.touch_tap(), tx.button()));
                    })
                    .on_double_press_tx(|s: &mut State, tx| {
                        s.events.push(("double", tx.press_point(), tx.touch_tap(), tx.button()));
                    })
                    .on_middle_press(|s: &mut State, tx| {
                        s.events.push(("middle", tx.press_point(), tx.touch_tap(), tx.button()));
                    })
                    .on_hover_change(|s: &mut State, _, tx| {
                        s.outside_press.push(("hover", tx.press_point()));
                    }),
            )
            .child(
                el().key("bg").size(120.0, 40.0)
                    .on_background_press_tx(|s: &mut State, tx| {
                        s.events.push(("bg", tx.press_point(), tx.touch_tap(), tx.button()));
                    }),
            )
            .child(
                el().key("down").size(120.0, 40.0)
                    .press_on_down()
                    .on_press_tx(|s: &mut State, tx| {
                        s.events.push(("down", tx.press_point(), tx.touch_tap(), tx.button()));
                    }),
            )
            .child(
                el().key("extra").size(120.0, 40.0)
                    .pointer_buttons([crate::PointerButton::Back])
                    .on_press_tx(|s: &mut State, tx| {
                        s.events.push(("extra", tx.press_point(), tx.touch_tap(), tx.button()));
                    })
                    .on_drag(|s: &mut State, _, tx| {
                        s.outside_press.push(("drag", tx.press_point()));
                    }),
            )
            .child(
                el().key("default").size(120.0, 30.0)
                    .default_button()
                    .on_press_tx(|s: &mut State, tx| {
                        s.events.push(("default", tx.press_point(), tx.touch_tap(), tx.button()));
                    }),
            )
    }

    let mut rt = Runtime::new(State::default(), view, (300.0, 260.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // 1. Primary click on btn at rect (10, 10, 130, 50) -> point (42, 27) => local (32, 17).
    rt.pointer_move(42.0, 27.0);
    rt.pointer_down(42.0, 27.0);
    rt.pointer_up(42.0, 27.0);
    rt.frame(0.0);
    assert_eq!(rt.state.outside_press, [("hover", None)]);
    let (kind, pt, touch, btn) = rt.state.events.pop().unwrap();
    let p = pt.expect("mouse on_press_tx carries press_point");
    assert_eq!((kind, touch, btn), ("press", false, crate::PointerButton::Primary));
    assert_eq!((p.x, p.y, p.window_x, p.window_y), (32.0, 17.0, 42.0, 27.0));
    assert_eq!(p.window(), PointerPress { x: 42.0, y: 27.0 });

    // 2. Second click within double-click interval fires both on_press_tx and on_double_press_tx with point.
    rt.pointer_down(43.0, 28.0);
    rt.pointer_up(43.0, 28.0);
    rt.frame(0.1);
    assert_eq!(rt.state.events.len(), 2);
    assert_eq!(rt.state.events[0].0, "press");
    assert_eq!(rt.state.events[1].0, "double");
    let dp = rt.state.events[1].1.expect("on_double_press_tx carries press_point");
    assert_eq!((dp.x, dp.y, dp.window_x, dp.window_y), (33.0, 18.0, 43.0, 28.0));
    rt.state.events.clear();

    // 3. Middle press on btn.
    rt.button_down(50.0, 30.0, crate::PointerButton::Middle);
    rt.button_up(50.0, 30.0, crate::PointerButton::Middle);
    rt.frame(0.0);
    let (kind, pt, _, btn) = rt.state.events.pop().unwrap();
    let mp = pt.expect("on_middle_press carries press_point");
    assert_eq!((kind, btn), ("middle", crate::PointerButton::Middle));
    assert_eq!((mp.x, mp.y, mp.window_x, mp.window_y), (40.0, 20.0, 50.0, 30.0));

    // 4. on_background_press_tx on bg at (10, 60, 130, 100).
    rt.pointer_down(35.0, 82.0);
    rt.pointer_up(35.0, 82.0);
    rt.frame(0.0);
    let (kind, pt, _, _) = rt.state.events.pop().unwrap();
    let bp = pt.expect("on_background_press_tx carries press_point");
    assert_eq!(kind, "bg");
    assert_eq!((bp.x, bp.y, bp.window_x, bp.window_y), (25.0, 22.0, 35.0, 82.0));

    // 5. press_on_down fires on down with the down point.
    rt.pointer_down(28.0, 124.0);
    rt.frame(0.0);
    let (kind, pt, _, _) = rt.state.events.pop().unwrap();
    let down_p = pt.expect("press_on_down carries press_point at down");
    assert_eq!(kind, "down");
    assert_eq!((down_p.x, down_p.y, down_p.window_x, down_p.window_y), (18.0, 14.0, 28.0, 124.0));
    rt.pointer_up(90.0, 130.0);
    rt.frame(0.0);
    assert!(rt.state.events.is_empty(), "press_on_down does not fire again on release");

    // 6. Extra button via pointer_buttons + drag handler sees None.
    rt.state.outside_press.clear();
    rt.button_down(45.0, 175.0, crate::PointerButton::Back);
    rt.button_up(45.0, 175.0, crate::PointerButton::Back);
    rt.frame(0.0);
    let (kind, pt, _, btn) = rt.state.events.pop().unwrap();
    let ep = pt.expect("extra button press carries press_point");
    assert_eq!((kind, btn), ("extra", crate::PointerButton::Back));
    assert_eq!((ep.x, ep.y, ep.window_x, ep.window_y), (35.0, 15.0, 45.0, 175.0));
    assert!(rt.state.outside_press.iter().all(|(k, p)| *k == "drag" && p.is_none()), "drag handler sees press_point() == None");

    // 7. Touch tap carries press_point alongside touch_tap() == true.
    rt.touch(1, crate::TouchPhase::Started, 55.0, 25.0, 0.0);
    rt.touch(1, crate::TouchPhase::Ended, 55.0, 25.0, 0.05);
    rt.frame(0.0);
    let (kind, pt, touch, _) = rt.state.events.pop().unwrap();
    let tp = pt.expect("touch tap carries press_point");
    assert_eq!((kind, touch), ("press", true));
    assert_eq!((tp.x, tp.y, tp.window_x, tp.window_y), (45.0, 15.0, 55.0, 25.0));

    // 8. Keyboard activation (Space and Return on focused button, and default_button Return) -> None.
    rt.update(|_, tx| tx.focus("btn", true));
    rt.frame(0.0);
    rt.key(crate::KeyEvent { key: crate::Key::Text(" ".into()), mods: crate::Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.events.pop(), Some(("press", None, false, crate::PointerButton::Primary)));
    rt.key(crate::KeyEvent { key: crate::Key::Enter, mods: crate::Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.events.pop(), Some(("press", None, false, crate::PointerButton::Primary)));
    rt.update(|_, tx| tx.blur());
    rt.frame(0.0);
    rt.key(crate::KeyEvent { key: crate::Key::Enter, mods: crate::Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.events.pop(), Some(("default", None, false, crate::PointerButton::Primary)));

    // 9. Accessibility Invoke -> None.
    let btn_path = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("btn")).unwrap().path.clone();
    rt.core_mut().queue_a11y_press(btn_path);
    rt.frame(0.0);
    assert_eq!(rt.state.events.pop(), Some(("press", None, false, crate::PointerButton::Primary)));
}

#[test]
fn press_point_follows_scroll_and_zoom() {
    #[derive(Default)]
    struct State {
        point: Option<PointerEvent>,
    }
    fn view(_: &State, cx: &mut Cx<State>) -> El {
        cx.zoom(1.5);
        col().size(200.0, 100.0).scroll(crate::ScrollSpec::default()).key("sc")
            .child(el().size(200.0, 80.0))
            .child(
                el().key("target").size(160.0, 50.0)
                    .on_press_tx(|s: &mut State, tx| s.point = tx.press_point()),
            )
            .child(el().size(200.0, 200.0))
    }

    let mut rt = Runtime::new(State::default(), view, (300.0, 150.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Scroll down by 40 view points: target moves from y=80 to drawn y=40.
    rt.update(|_, tx| tx.scroll_to_instant("sc", 0.0, 40.0));
    rt.frame(0.0);
    rt.pointer_down(25.0, 55.0);
    rt.pointer_up(25.0, 55.0);
    rt.frame(0.0);
    let p = rt.state.point.expect("press_point on zoomed scrolled element");
    assert!((p.x - 25.0).abs() < 0.01 && (p.y - 15.0).abs() < 0.01, "expected local (25, 15), got ({}, {})", p.x, p.y);
    assert!((p.window_x - 25.0).abs() < 0.01 && (p.window_y - 55.0).abs() < 0.01, "expected window (25, 55), got ({}, {})", p.window_x, p.window_y);
}

#[test]
fn hover_tracks_bounds_follows_drag_outside_reenter_and_press_scale_geometry() {
    #[derive(Default)]
    struct State {
        pill_hover: Vec<bool>,
        btn_hover: Vec<bool>,
        presses: usize,
    }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(240.0, 160.0)
            .child(
                // Ancestor WITHOUT hover_tracks_bounds keeps Rune's default held-press retention.
                el().key("pill").size(160.0, 60.0)
                    .on_hover_change(|s: &mut State, inside, _| s.pill_hover.push(inside))
                    .child(
                        el().key("btn").size(100.0, 40.0)
                            .hover(Color::WHITE.alpha(0.1))
                            .press(Color::WHITE.alpha(0.2))
                            .press_scale(0.8)
                            .hover_tracks_bounds()
                            .on_hover_change(|s: &mut State, inside, _| s.btn_hover.push(inside))
                            .on_press(|s: &mut State| s.presses += 1),
                    ),
            )
    }

    let mut rt = Runtime::new(State::default(), view, (240.0, 160.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let btn_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("btn")).unwrap().id;
    let targets = |rt: &Runtime<State>| {
        let store = rt.scene.store();
        (
            store.target(btn_id, P_HOVER).unwrap_or(0.0),
            store.target(btn_id, P_PRESS).unwrap_or(0.0),
            store.target(btn_id, P_PRESS_SCALE).unwrap_or(1.0),
        )
    };

    // Enter near left edge (x=3, y=20) — inside resting [0..100, 0..40], outside 0.8-scaled [10..90, 4..36].
    rt.pointer_move(3.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.pill_hover, [true]);
    assert_eq!(rt.state.btn_hover, [true]);

    // Press down near the edge: press_scale shrinks the drawn rect, but compensated hit geometry keeps it inside.
    rt.pointer_down(3.0, 20.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(targets(&rt), (1.0, 1.0, 0.8));
    rt.pointer_move(5.0, 20.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.btn_hover, [true], "moving inside press_scale resting bounds stays inside");
    assert_eq!(targets(&rt), (1.0, 1.0, 0.8));

    // Drag outside the button and pill within the window: button leaves, pill retains default!
    rt.pointer_move(200.0, 100.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.pressing(), "capture stays active while dragged outside");
    assert_eq!(rt.state.btn_hover, [true, false], "hover_tracks_bounds reports outside during held press");
    assert_eq!(rt.state.pill_hover, [true], "ancestor without hover_tracks_bounds keeps held-press retention");
    assert_eq!(targets(&rt), (0.0, 0.0, 1.0), "hover/press/press_scale tracks retarget off");

    // Drag back inside before release: button re-enters and tracks restore!
    rt.pointer_move(50.0, 20.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.btn_hover, [true, false, true], "re-entering before release reports inside again");
    assert_eq!(rt.state.pill_hover, [true]);
    assert_eq!(targets(&rt), (1.0, 1.0, 0.8));

    // Release inside fires on_press.
    rt.pointer_up(50.0, 20.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.presses, 1);
    assert_eq!(rt.state.btn_hover, [true, false, true]);
    assert_eq!(rt.state.pill_hover, [true]);
}

#[test]
fn hover_tracks_bounds_handles_coordinateless_pointer_leave_during_capture_without_breaking_drag_or_cursor() {
    #[derive(Default)]
    struct State {
        ancestor_hover: Vec<bool>,
        btn_hover: Vec<bool>,
        presses: usize,
        phases: Vec<(Phase, bool)>,
    }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 120.0)
            .child(
                el().key("ancestor").size(160.0, 80.0)
                    .on_hover_change(|s: &mut State, inside, _| s.ancestor_hover.push(inside))
                    .child(
                        el().key("edge-btn").size(80.0, 40.0)
                            .cursor(Cursor::Hidden)
                            .hover(Color::WHITE.alpha(0.1))
                            .press(Color::WHITE.alpha(0.2))
                            .hover_tracks_bounds()
                            .on_hover_change(|s: &mut State, inside, _| s.btn_hover.push(inside))
                            .on_press(|s: &mut State| s.presses += 1),
                    )
                    .child(
                        el().key("drag-btn").size(80.0, 40.0)
                            .cursor(Cursor::Grabbing)
                            .hover(Color::WHITE.alpha(0.1))
                            .press(Color::WHITE.alpha(0.2))
                            .hover_tracks_bounds()
                            .on_hover_change(|s: &mut State, inside, _| s.btn_hover.push(inside))
                            .on_drag(|s: &mut State, d, _| s.phases.push((d.phase, d.cancelled()))),
                    ),
            )
    }

    let mut rt = Runtime::new(State::default(), view, (200.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let edge_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("edge-btn")).unwrap().id;
    let hp = |rt: &Runtime<State>| {
        let store = rt.scene.store();
        (store.target(edge_id, P_HOVER).unwrap_or(0.0), store.target(edge_id, P_PRESS).unwrap_or(0.0))
    };

    // Scenario A: press edge-btn -> coordinate-less pointer_leave -> re-enter at the EXACT same point -> release clicks!
    rt.pointer_move(20.0, 10.0);
    rt.pointer_down(20.0, 10.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.btn_hover, [true]);
    assert_eq!(rt.state.ancestor_hover, [true]);
    assert_eq!(hp(&rt), (1.0, 1.0));

    rt.pointer_leave();
    rt.frame(1.0 / 60.0);
    assert!(rt.pressing(), "coordinate-less pointer_leave preserves capture");
    assert_eq!(rt.cursor(), Cursor::Hidden, "captured cursor stays until release");
    assert_eq!(rt.state.btn_hover, [true, false], "coordinate-less pointer_leave reports outside immediately");
    assert_eq!(rt.state.ancestor_hover, [true], "ancestor without hover_tracks_bounds stays inside");
    assert_eq!(hp(&rt), (0.0, 0.0), "P_HOVER and P_PRESS retarget off on pointer_leave during capture");

    // Re-enter at the exact same coordinates through observe_pointer (host CursorMoved path).
    assert!(rt.observe_pointer(20.0, 10.0), "observe_pointer must not ignore re-entry at the same coordinates");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.btn_hover, [true, false, true], "re-entering window restores hover_change");
    assert_eq!(hp(&rt), (1.0, 1.0), "re-entering window restores P_HOVER and P_PRESS");

    rt.pointer_up(20.0, 10.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.presses, 1, "releasing after re-entry fires on_press");

    // Scenario B: press edge-btn -> coordinate-less pointer_leave -> release outside at stale pre-leave coords!
    rt.state.btn_hover.clear();
    rt.state.ancestor_hover.clear();
    rt.pointer_down(20.0, 10.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_leave();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.btn_hover, [false]);
    assert_eq!(rt.cursor(), Cursor::Hidden);

    rt.pointer_up(20.0, 10.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.presses, 1, "releasing outside at stale pre-leave coords must not fire on_press");
    assert_eq!(rt.state.btn_hover, [false], "stale-coord release after pointer_leave must not flicker hover true");
    assert_eq!(rt.state.ancestor_hover, [false], "ancestor hears leave once capture ends");
    assert_eq!(rt.pointer(), None, "deferred leave clears pointer on release");
    assert_eq!(rt.cursor(), Cursor::Default, "deferred cursor cleanup lands on release");

    // Scenario C: drag continuity across coordinate-less pointer_leave.
    rt.state.btn_hover.clear();
    let (dx, dy) = rt.find("drag-btn").unwrap();
    rt.pointer_move(dx, dy);
    rt.pointer_down(dx, dy);
    rt.frame(1.0 / 60.0);
    rt.pointer_leave();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.btn_hover, [true, false]);
    assert!(rt.pressing());
    rt.pointer_move(dx + 10.0, dy);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.btn_hover, [true, false, true]);
    rt.pointer_up(dx + 10.0, dy);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.phases, [(Phase::Down, false), (Phase::Move, false), (Phase::Up, false)]);
}

#[test]
fn hover_tracks_bounds_respects_clip_bounds_and_memo_reuse() {
    #[derive(Default)]
    struct State {
        track: bool,
        inside: Vec<bool>,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let track = s.track;
        col().size(200.0, 120.0)
            .child(
                // Clip to 50x40; child is 100x40 so x in [50..100) is clipped away.
                row().key("clip-box").size(50.0, 40.0).clip_rounded(0.0)
                    .child(crate::memo("clipped-btn", track, move || {
                        let mut b = el().key("inner").size(100.0, 40.0)
                            .on_press(|_: &mut State| {})
                            .on_hover_change(|s: &mut State, inside, _| s.inside.push(inside));
                        if track {
                            b = b.hover_tracks_bounds();
                        }
                        b
                    })),
            )
    }

    let mut rt = Runtime::new(State { track: false, inside: Vec::new() }, view, (200.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(20.0, 20.0);
    rt.pointer_down(20.0, 20.0);
    rt.frame(0.0);
    // Move into clipped-out region (x=70): without hover_tracks_bounds, stays inside.
    rt.pointer_move(70.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.inside, [true]);
    rt.pointer_up(70.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.inside, [true, false]);

    // Flip track = true (memo version changes): now moving into clipped-out region reports outside.
    rt.state.inside.clear();
    rt.update(|s, _| s.track = true);
    rt.frame(0.0);
    rt.pointer_move(20.0, 20.0);
    rt.pointer_down(20.0, 20.0);
    rt.frame(0.0);
    rt.pointer_move(70.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.inside, [true, false], "clipped-away region counts as outside for hover_tracks_bounds");
    rt.pointer_move(20.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.inside, [true, false, true], "re-entering visible clipped rect reports inside");
    rt.pointer_up(20.0, 20.0);
    rt.frame(0.0);
}
