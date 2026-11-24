use winit::dpi::PhysicalPosition;
use winit::event::{MouseScrollDelta, TouchPhase};
use winit::keyboard::ModifiersState;

use super::*;
use crate::{Cx, El, Env, GesturePhase, Runtime, WheelEvent, el};

#[test]
fn gesture_phase_maps_every_winit_touch_phase() {
    assert_eq!(gesture_phase(TouchPhase::Started), GesturePhase::Began);
    assert_eq!(gesture_phase(TouchPhase::Moved), GesturePhase::Moved);
    assert_eq!(gesture_phase(TouchPhase::Ended), GesturePhase::Ended);
    assert_eq!(gesture_phase(TouchPhase::Cancelled), GesturePhase::Ended);
}

#[test]
fn shared_wheel_translates_pixel_and_line_deltas_with_zoom_and_modifiers() {
    #[derive(Default)]
    struct S { events: Vec<WheelEvent> }
    fn view(_: &S, cx: &mut Cx<S>) -> El {
        cx.zoom(2.0);
        el().size(200.0, 200.0)
            .hit()
            .on_wheel(|s: &mut S, ev, _| { s.events.push(ev); true })
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    // View point (50, 50) at scale 2.0, zoom 2.0.
    rt.pointer_move(50.0, 50.0);

    // PixelDelta(0, -40) at scale 2.0 -> window delta -20 pt -> view delta -10 pt at zoom 2.0.
    wheel(rt.core_mut(), MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -40.0)), gesture_phase(TouchPhase::Moved), 2.0, ModifiersState::ALT);
    rt.frame(0.0);
    assert_eq!(rt.state.events.len(), 1);
    let ev = rt.state.events[0];
    assert_eq!((ev.dx, ev.dy), (0.0, -10.0));
    assert!(ev.precise && !ev.notch && ev.mods.alt);
    assert_eq!(ev.phase, GesturePhase::Moved);

    // Magnify / rotate / smart_magnify require a pointer and return false when pointer is None.
    assert!(magnify(rt.core_mut(), 0.25, gesture_phase(TouchPhase::Moved)));
    assert!(rotate(rt.core_mut(), 15.0, gesture_phase(TouchPhase::Moved)));
    assert!(smart_magnify(rt.core_mut()));
    rt.pointer_leave();
    assert!(!magnify(rt.core_mut(), 0.25, gesture_phase(TouchPhase::Moved)));
    assert!(!rotate(rt.core_mut(), 15.0, gesture_phase(TouchPhase::Moved)));
    assert!(!smart_magnify(rt.core_mut()));
}

#[test]
fn shared_modifiers_updates_state_and_notifies_physical_handlers() {
    #[derive(Default)]
    struct S { seen: Vec<crate::Mods> }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().key("root").size(100.0, 100.0).focusable(true).on_physical_input(|s: &mut S, ev, _| {
            if let crate::PhysicalInput::Modifiers(m) = ev {
                s.seen.push(*m);
                return true;
            }
            false
        })
    }
    let mut rt = Runtime::new(S::default(), view, (100.0, 100.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| tx.focus("root", false));
    rt.frame(0.0);
    let mut mods = ModifiersState::empty();
    let redraw = {
        let (core, mut app) = rt.split_mut();
        modifiers(core, &mut app, &mut mods, ModifiersState::SHIFT | ModifiersState::ALT)
    };
    assert!(redraw);
    assert_eq!(mods, ModifiersState::SHIFT | ModifiersState::ALT);
    rt.frame(0.0);
    assert_eq!(rt.state.seen.last().map(|m| (m.shift, m.alt)), Some((true, true)));
}

#[test]
fn pointer_wheel_hover_and_ime_area_scale_by_view_zoom() {
    #[derive(Default)]
    struct S { clicks: Vec<&'static str>, wheels: Vec<WheelEvent>, pinches: Vec<f32>, edit: crate::TextEdit }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        cx.zoom(1.5);
        crate::col().size(200.0, 200.0)
            .on_wheel(|s: &mut S, ev, _| { s.wheels.push(ev); true })
            .on_magnify(|s: &mut S, ev, _| { s.pinches.push(ev.delta); true })
            .child(el().key("top").size(100.0, 50.0).on_press(|s: &mut S| s.clicks.push("top")))
            .child(el().key("mid").size(100.0, 50.0).on_press(|s: &mut S| s.clicks.push("mid")))
            .child(crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0))
                .key("field").size(100.0, 40.0).on_edit(|s: &mut S| &mut s.edit))
    }
    // Scale 2.0, zoom 1.5 -> input_scale = 3.0.
    // "mid" sits at view y in [50, 100), i.e. window y in [75, 150), physical y in [150, 300).
    let mut rt = Runtime::new(S::default(), view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);

    // Physical (120, 180) -> window (60, 90) -> view (40, 60) which hits "mid" (not "top").
    let (vx, vy) = physical_point(rt.core(), 120.0, 180.0);
    assert!((vx - 40.0).abs() < 1e-4 && (vy - 60.0).abs() < 1e-4);
    rt.pointer_move(vx, vy);
    rt.pointer_down(vx, vy);
    rt.pointer_up(vx, vy);
    rt.frame(0.0);
    assert_eq!(rt.state.clicks, ["mid"]);

    // Window point (45, 30) -> view (30, 20) which hits "top".
    let (hx, hy) = window_point(rt.core(), 45.0, 30.0);
    assert!((hx - 30.0).abs() < 1e-4 && (hy - 20.0).abs() < 1e-4);
    rt.pointer_move(hx, hy);
    rt.pointer_down(hx, hy);
    rt.pointer_up(hx, hy);
    rt.frame(0.0);
    assert_eq!(rt.state.clicks, ["mid", "top"]);

    // Wheel delta -30 window points -> -20 view points at zoom 1.5.
    let (redraw, used) = web_wheel(
        rt.core_mut(), 0.0, -30.0, crate::Mods::default(), GesturePhase::Moved, true, false, false,
    );
    assert!(redraw && used.is_none());
    rt.frame(0.0);
    assert_eq!(rt.state.wheels.len(), 1);
    assert!((rt.state.wheels[0].dy - (-20.0)).abs() < 1e-4);

    // Ctrl+wheel pinches in un-zoomed window points; document mode leaves it to the browser.
    let ctrl = crate::Mods { ctrl: true, cmd: true, ..crate::Mods::default() };
    let (redraw, used) = web_wheel(rt.core_mut(), 0.0, -40.0, ctrl, GesturePhase::Moved, true, false, false);
    assert!(redraw && used == Some(true));
    rt.frame(0.0);
    assert_eq!(rt.state.pinches.len(), 1);
    let (redraw, used) = web_wheel(rt.core_mut(), 0.0, -40.0, ctrl, GesturePhase::Moved, true, true, false);
    assert!(!redraw && used == Some(false));

    // fit_height leaves vertical wheel to the page, while keeping horizontal wheel.
    let (redraw, used) = web_wheel(rt.core_mut(), 0.0, -30.0, crate::Mods::default(), GesturePhase::Moved, true, false, true);
    assert!(!redraw && used.is_none());
    let (redraw, used) = web_wheel(rt.core_mut(), -30.0, 0.0, crate::Mods::default(), GesturePhase::Moved, true, false, true);
    assert!(redraw && used.is_none());

    // IME area in window points scales by zoom (1.5x view ime_area).
    let (fx, fy) = rt.find("field").unwrap();
    rt.pointer_down(fx, fy);
    rt.pointer_up(fx, fy);
    rt.frame(0.0);
    let view_ime = rt.ime_area.expect("focused field sets ime_area");
    let win_ime = ime_area_window(rt.core()).expect("window ime_area");
    for i in 0..4 {
        assert!((win_ime[i] - view_ime[i] * 1.5).abs() < 1e-3);
    }
}

#[test]
fn web_page_wheel_scales_by_viewport_zoom_and_respects_shift_without_double_swapping() {
    #[derive(Default)]
    struct S { wheels: Vec<WheelEvent> }
    fn view(_: &S, cx: &mut Cx<S>) -> El {
        cx.zoom(2.0);
        el().size(200.0, 100.0).hit().on_wheel(|s: &mut S, ev, _| { s.wheels.push(ev); true })
    }
    // Window viewport 400x200 at zoom 2.0 -> view viewport 200x100.
    let mut rt = Runtime::new(S::default(), view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(50.0, 50.0);

    // 1 vertical page down -> -1.0 * 200 * 0.875 = -175 window pt -> -87.5 view pt.
    let (dx, dy) = wheel_page_delta(rt.core(), 0.0, -1.0, false);
    assert!((dx - 0.0).abs() < 1e-4 && (dy - (-175.0)).abs() < 1e-4);
    let (redraw, _) = web_wheel(rt.core_mut(), dx, dy, crate::Mods::default(), GesturePhase::Moved, false, false, false);
    assert!(redraw);
    rt.frame(0.0);
    assert_eq!(rt.state.wheels.len(), 1);
    assert!((rt.state.wheels[0].dy - (-87.5)).abs() < 1e-4);
    assert!(!rt.state.wheels[0].precise && rt.state.wheels[0].phase == GesturePhase::Moved);

    // Shift + vertical page when browser kept dx == 0 turns horizontal using window width (400 * 0.875 = 350 -> 175 view pt).
    let (dx, dy) = wheel_page_delta(rt.core(), 0.0, -1.0, true);
    assert!((dx - (-350.0)).abs() < 1e-4 && dy == 0.0);
    let shift = crate::Mods { shift: true, ..crate::Mods::default() };
    let (redraw, _) = web_wheel(rt.core_mut(), dx, dy, shift, GesturePhase::Moved, false, false, false);
    assert!(redraw);
    rt.frame(0.0);
    assert!((rt.state.wheels[1].dx - (-175.0)).abs() < 1e-4 && rt.state.wheels[1].dy == 0.0);

    // When the browser already swapped axes (dx != 0, dy == 0), Shift must not double-swap.
    let (dx, dy) = wheel_page_delta(rt.core(), -1.0, 0.0, true);
    assert!((dx - (-350.0)).abs() < 1e-4 && dy == 0.0);
    let (dx_px, dy_px) = crate::runtime::wheel_delta(
        MouseScrollDelta::PixelDelta(PhysicalPosition::new(-40.0, 0.0)),
        1.0,
        true,
        40.0,
    );
    assert_eq!((dx_px, dy_px), (-40.0, 0.0));
}

#[test]
fn sync_modifiers_observes_changes_updates_press_mods_and_deduplicates_repeats() {
    #[derive(Default)]
    struct S { physical: Vec<crate::Mods>, click_mods: Vec<crate::Mods> }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().key("btn").size(100.0, 100.0).focusable(true)
            .on_physical_input(|s: &mut S, ev, _| {
                if let crate::PhysicalInput::Modifiers(m) = ev {
                    s.physical.push(*m);
                    return true;
                }
                false
            })
            .on_press_tx(|s: &mut S, tx| s.click_mods.push(tx.mods()))
    }
    let mut rt = Runtime::new(S::default(), view, (100.0, 100.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| tx.focus("btn", false));
    rt.frame(0.0);

    let mut mods = ModifiersState::empty();
    let shift_ctrl = modifiers_from_flags(true, true, false, false);
    assert_eq!(shift_ctrl, ModifiersState::SHIFT | ModifiersState::CONTROL);

    let redraw = {
        let (core, mut app) = rt.split_mut();
        sync_modifiers(core, &mut app, &mut mods, shift_ctrl)
    };
    assert!(redraw);
    rt.frame(0.0);
    assert_eq!(rt.state.physical.len(), 1);
    assert!(rt.state.physical[0].shift && rt.state.physical[0].ctrl);

    // Repeating the same state (winit's ModifiersChanged after DOM capture) is a no-op.
    let dup = {
        let (core, mut app) = rt.split_mut();
        sync_modifiers(core, &mut app, &mut mods, shift_ctrl)
    };
    assert!(!dup);
    rt.frame(0.0);
    assert_eq!(rt.state.physical.len(), 1);

    // A click after modifier refresh sees the held modifiers via tx.mods().
    rt.pointer_move(50.0, 50.0);
    rt.pointer_down(50.0, 50.0);
    rt.pointer_up(50.0, 50.0);
    rt.frame(0.0);
    assert_eq!(rt.state.click_mods.len(), 1);
    assert!(rt.state.click_mods[0].shift && rt.state.click_mods[0].ctrl);
}

#[test]
fn keyboard_core_handles_unidentified_repeat_paste_fallback_and_release_gating() {
    #[derive(Default)]
    struct S {
        edit: crate::TextEdit,
        keys: Vec<(crate::Key, Option<crate::PhysicalKeyCode>, Option<char>, bool)>,
        ups: Vec<crate::Key>,
        listen_up: bool,
        consume_paste: bool,
        input_keys: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let root = crate::col().size(200.0, 100.0)
            .child(crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0))
                .key("field").size(160.0, 40.0).on_edit(|s: &mut S| &mut s.edit)
                .on_input_key(|s: &mut S, ev, _| {
                    s.input_keys += 1;
                    s.consume_paste && ev.key == crate::Key::Command('v')
                }));
        if s.listen_up {
            root.on_key_up(|s: &mut S, ev, _| { s.ups.push(ev.key.clone()); true })
        } else {
            root
        }
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 100.0), Env::default());
    let mem = crate::clipboard::memory();
    mem.set_text("pasted");
    rt.use_clipboard(&mem);
    rt.on_key = Some(|s, ev, tx| {
        s.keys.push((ev.key.clone(), tx.physical_key(), tx.key_base(), tx.key_repeat()));
        true
    });
    rt.frame(0.0);

    // Unidentified physical key with repeat: true uses repeat_key (tx.key_repeat() == true).
    let redraw = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, KeyInput {
            physical: None,
            logical: Some(crate::KeyEvent { key: crate::Key::Down, mods: crate::Mods::default() }),
            base: None,
            mods: crate::Mods::default(),
            pressed: true,
            repeat: true,
            unhandled: None,
        })
    };
    assert!(redraw);
    rt.frame(0.0);
    assert_eq!(rt.state.keys, [(crate::Key::Down, None, None, true)]);

    // Unhandled key release with no on_key_up returns false (no redraw).
    let redraw = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, KeyInput {
            physical: Some(crate::PhysicalKeyCode::ArrowDown),
            logical: None,
            base: None,
            mods: crate::Mods::default(),
            pressed: false,
            repeat: false,
            unhandled: None,
        })
    };
    assert!(!redraw);

    // Physical key press delivers physical_key, key_base, and key_repeat.
    let redraw = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, KeyInput {
            physical: Some(crate::PhysicalKeyCode::KeyA),
            logical: Some(crate::KeyEvent { key: crate::Key::Text("A".into()), mods: crate::Mods { shift: true, ..Default::default() } }),
            base: Some('a'),
            mods: crate::Mods { shift: true, ..Default::default() },
            pressed: true,
            repeat: false,
            unhandled: None,
        })
    };
    assert!(redraw);
    rt.frame(0.0);
    assert_eq!(rt.state.keys.last(), Some(&(crate::Key::Text("A".into()), Some(crate::PhysicalKeyCode::KeyA), Some('a'), false)));

    // Unidentified physical key Command('v') offers on_input_key first, then pastes when declined.
    let (fx, fy) = rt.find("field").unwrap();
    rt.pointer_down(fx, fy);
    rt.pointer_up(fx, fy);
    rt.frame(0.0);
    let cmd = crate::Mods { cmd: true, ..crate::Mods::default() };
    let app_keys_before = rt.state.keys.len();
    rt.state.consume_paste = true;
    let redraw = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, KeyInput {
            physical: None,
            logical: Some(crate::KeyEvent { key: crate::Key::Command('v'), mods: cmd }),
            base: None,
            mods: cmd,
            pressed: true,
            repeat: false,
            unhandled: None,
        })
    };
    assert!(redraw);
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.input_keys, 1);
    assert_eq!(rt.state.edit.text, "", "consuming on_input_key prevents clipboard paste");
    assert_eq!(rt.state.keys.len(), app_keys_before);

    rt.state.consume_paste = false;
    let redraw = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, KeyInput {
            physical: None,
            logical: Some(crate::KeyEvent { key: crate::Key::Command('v'), mods: cmd }),
            base: None,
            mods: cmd,
            pressed: true,
            repeat: false,
            unhandled: None,
        })
    };
    assert!(redraw);
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.input_keys, 2);
    assert_eq!(rt.state.edit.text, "pasted");
    assert_eq!(rt.state.keys.len(), app_keys_before);

    // With an on_key_up listener, unidentified release delivers key_up and requests redraw.
    rt.update(|s, _| s.listen_up = true);
    rt.frame(0.0);
    let redraw = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, KeyInput {
            physical: None,
            logical: Some(crate::KeyEvent { key: crate::Key::Escape, mods: crate::Mods::default() }),
            base: None,
            mods: crate::Mods::default(),
            pressed: false,
            repeat: false,
            unhandled: None,
        })
    };
    assert!(redraw);
    rt.frame(0.0);
    assert_eq!(rt.state.ups, [crate::Key::Escape]);
}

#[test]
fn mouse_button_core_handles_macos_ctrl_click_left_down_mods_and_middle_buttons() {
    #[derive(Default)]
    struct S {
        primary: Vec<crate::Mods>,
        secondary: usize,
        middle: usize,
        physical_mods: Vec<crate::Mods>,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().key("target").size(100.0, 100.0).focusable(true)
            .on_press_tx(|s: &mut S, tx| s.primary.push(tx.mods()))
            .on_secondary_press(|s: &mut S, _, _| s.secondary += 1)
            .on_middle_press(|s: &mut S, _| s.middle += 1)
            .on_physical_input(|s: &mut S, ev, _| {
                if let crate::PhysicalInput::Modifiers(m) = ev {
                    s.physical_mods.push(*m);
                    return true;
                }
                false
            })
    }
    let mut rt = Runtime::new(S::default(), view, (100.0, 100.0), Env::default());
    rt.frame(0.0);
    rt.pointer_move(50.0, 50.0);

    let ctrl = crate::Mods { ctrl: true, cmd: true, ..Default::default() };
    let mut control_click = false;

    // On macOS (ctrl_click_is_secondary = true), Ctrl+Left is secondary even if Ctrl lifts before mouse-up.
    let down = {
        let (core, mut app) = rt.split_mut();
        mouse_button_core(core, &mut app, MouseButtonInput {
            x: 50.0, y: 50.0, button: crate::PointerButton::Primary, pressed: true,
            mods: ctrl, ctrl_click_is_secondary: true, queue_mods_on_primary_down: true,
        }, &mut control_click)
    };
    assert_eq!(down, ButtonAction::SecondaryDown);
    assert!(control_click);
    let up = {
        let (core, mut app) = rt.split_mut();
        mouse_button_core(core, &mut app, MouseButtonInput {
            x: 50.0, y: 50.0, button: crate::PointerButton::Primary, pressed: false,
            mods: crate::Mods::default(), ctrl_click_is_secondary: true, queue_mods_on_primary_down: true,
        }, &mut control_click)
    };
    assert_eq!(up, ButtonAction::SecondaryUp);
    assert!(!control_click);
    rt.frame(0.0);
    assert_eq!((rt.state.primary.len(), rt.state.secondary), (0, 1));

    // Off macOS (ctrl_click_is_secondary = false), Ctrl+Left stays primary and queues modifiers for the newly focused element.
    let down = {
        let (core, mut app) = rt.split_mut();
        mouse_button_core(core, &mut app, MouseButtonInput {
            x: 50.0, y: 50.0, button: crate::PointerButton::Primary, pressed: true,
            mods: ctrl, ctrl_click_is_secondary: false, queue_mods_on_primary_down: true,
        }, &mut control_click)
    };
    assert_eq!(down, ButtonAction::PrimaryDown);
    let up = {
        let (core, mut app) = rt.split_mut();
        mouse_button_core(core, &mut app, MouseButtonInput {
            x: 50.0, y: 50.0, button: crate::PointerButton::Primary, pressed: false,
            mods: ctrl, ctrl_click_is_secondary: false, queue_mods_on_primary_down: true,
        }, &mut control_click)
    };
    assert_eq!(up, ButtonAction::PrimaryUp);
    rt.frame(0.0);
    assert_eq!((rt.state.primary.len(), rt.state.secondary), (1, 1));
    assert!(rt.state.primary[0].ctrl);
    assert!(rt.state.physical_mods.last().is_some_and(|m| m.ctrl));

    // Middle button routes through button_down / button_up.
    let down = {
        let (core, mut app) = rt.split_mut();
        mouse_button_core(core, &mut app, MouseButtonInput {
            x: 50.0, y: 50.0, button: crate::PointerButton::Middle, pressed: true,
            mods: crate::Mods::default(), ctrl_click_is_secondary: false, queue_mods_on_primary_down: false,
        }, &mut control_click)
    };
    assert_eq!(down, ButtonAction::OtherDown(crate::PointerButton::Middle));
    let up = {
        let (core, mut app) = rt.split_mut();
        mouse_button_core(core, &mut app, MouseButtonInput {
            x: 50.0, y: 50.0, button: crate::PointerButton::Middle, pressed: false,
            mods: crate::Mods::default(), ctrl_click_is_secondary: false, queue_mods_on_primary_down: false,
        }, &mut control_click)
    };
    assert_eq!(up, ButtonAction::OtherUp(crate::PointerButton::Middle));
    rt.frame(0.0);
    assert_eq!(rt.state.middle, 1);
}

#[test]
fn hide_reset_cancels_drag_commits_ime_releases_keys_and_clears_host_state() {
    #[derive(Default)]
    struct S {
        edit: crate::TextEdit,
        drag_cancelled: bool,
        focus_lost: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        crate::col().size(200.0, 200.0)
            .child(el().key("drag").size(100.0, 50.0).focusable(true)
                .on_drag(|s: &mut S, d: &mut crate::Drag, _| {
                    if d.cancelled { s.drag_cancelled = true; }
                })
                .on_physical_input(|s: &mut S, ev, _| {
                    if matches!(ev, crate::PhysicalInput::FocusLost) { s.focus_lost += 1; return true; }
                    false
                }))
            .child(crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0))
                .key("field").size(160.0, 40.0).on_edit(|s: &mut S| &mut s.edit))
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::default());
    rt.frame(0.0);

    // Focus field and open an IME composition.
    let (fx, fy) = rt.find("field").unwrap();
    rt.pointer_down(fx, fy);
    rt.pointer_up(fx, fy);
    rt.frame(0.0);
    ime_core(rt.core_mut(), None, Ime::Preedit("nihon".into(), Some((5, 5)), Vec::new()));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.preedit().map(|(s, _)| s), Some("nihon"));

    // Start a drag on "drag" (moves focus there, holds pointer capture).
    let (dx, dy) = rt.find("drag").unwrap();
    rt.pointer_move(dx, dy);
    rt.pointer_down(dx, dy);
    rt.frame(0.0);
    assert!(rt.pointer_captured());

    let mut mods = ModifiersState::SHIFT | ModifiersState::CONTROL;
    let mut control_click = true;
    hide_reset(rt.core_mut(), &mut mods, &mut control_click);
    assert!(mods.is_empty() && !control_click);
    let (reset, _) = ime_sync_state(rt.core_mut());
    assert!(reset, "finish_composition sets ime_reset for the host");
    rt.frame(0.0);

    assert!(rt.state.drag_cancelled, "hide cancels active drag with cancelled = true");
    assert_eq!(rt.state.focus_lost, 1, "hide delivers FocusLost to release held keys");
    assert!(!rt.pointer_captured());
    assert_eq!(rt.pointer(), None);
}

#[test]
fn pointer_cancel_distinguishes_lost_capture_at_rest_from_active_capture_and_cancel() {
    #[derive(Default)]
    struct S { cancelled: usize }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().key("drag").size(100.0, 100.0)
            .on_drag(|s: &mut S, d: &mut crate::Drag, _| {
                if d.cancelled { s.cancelled += 1; }
            })
    }
    let mut rt = Runtime::new(S::default(), view, (100.0, 100.0), Env::default());
    rt.frame(0.0);
    rt.pointer_move(50.0, 50.0);
    rt.frame(0.0);

    // At rest (no capture held, e.g. lostpointercapture after a normal pointerup): no-op, hover preserved.
    let mut control_click = false;
    assert!(!pointer_cancel(rt.core_mut(), &mut control_click, true));
    assert_eq!(rt.pointer(), Some((50.0, 50.0)));

    // With an active drag capture, lostpointercapture cancels the drag and drops pointer.
    rt.pointer_down(50.0, 50.0);
    rt.frame(0.0);
    assert!(rt.pointer_captured());
    assert!(pointer_cancel(rt.core_mut(), &mut control_click, true));
    rt.frame(0.0);
    assert_eq!(rt.state.cancelled, 1);
    assert!(!rt.pointer_captured() && rt.pointer().is_none());

    // Unconditional pointercancel (only_if_captured = false) ends hover even at rest.
    rt.pointer_move(40.0, 40.0);
    rt.frame(0.0);
    assert!(pointer_cancel(rt.core_mut(), &mut control_click, false));
    assert_eq!(rt.pointer(), None);
}

#[test]
fn ime_sync_state_reset_preserves_mirror_diff_when_empty_field_blurs() {
    #[derive(Default)]
    struct S { edit: crate::TextEdit }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        crate::col().size(200.0, 120.0)
            .child(el().key("btn").size(100.0, 40.0).focusable(true))
            .child(crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0))
                .key("field").size(160.0, 40.0).on_edit(|s: &mut S| &mut s.edit))
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 120.0), Env::default());
    rt.frame(0.0);
    let mut mirror = crate::runtime::web_runtime::Mirror::default();

    // Focus empty text field and sync mirror.
    let (fx, fy) = rt.find("field").unwrap();
    rt.pointer_down(fx, fy);
    rt.pointer_up(fx, fy);
    rt.frame(0.0);
    assert!(rt.core().web_context(&mut mirror));
    assert!(!mirror.key.is_empty());

    // Blurring the empty field sets ime_reset and leaves focused_input() == None.
    // web_context must still report a change (key -> "") when mirror.key is not cleared first.
    let (bx, by) = rt.find("btn").unwrap();
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.0);
    let (reset, area) = ime_sync_state(rt.core_mut());
    assert!(area.is_none());
    let changed = rt.core().web_context(&mut mirror);
    assert!(changed || reset, "mirror must sync back to empty key so DOM focus returns to canvas");
    assert!(mirror.key.is_empty());
}

#[test]
fn ime_sync_skips_unchanged_cursor_area_and_resends_on_change_reset_or_reentry() {
    #[derive(Default)]
    struct S { edit: crate::TextEdit }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        crate::col().size(240.0, 120.0)
            .child(el().key("btn").size(100.0, 40.0).focusable(true))
            .child(crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0))
                .key("field").label("Field").size(200.0, 40.0).on_edit(|s: &mut S| &mut s.edit))
    }
    let mut rt = Runtime::new(S::default(), view, (240.0, 120.0), Env::default());
    rt.frame(0.0);

    let mut ime = ImeSync::default();
    let step = ime_sync_step(rt.core_mut(), &mut ime, true);
    assert_eq!(step, ImeStep { cancel: false, set_allowed: None, cursor_area: None });

    // Focusing a text field enables IME and sends the cursor area once.
    let (fx, fy) = rt.find("field").unwrap();
    rt.pointer_down(fx, fy);
    rt.pointer_up(fx, fy);
    rt.frame(0.0);
    let step = ime_sync_step(rt.core_mut(), &mut ime, true);
    let area = step.cursor_area.expect("first sync after focus sends cursor area");
    assert_eq!(step.set_allowed, Some(true));

    // An unchanged frame (including after Wayland's empty Preedit reply) must not resend.
    ime_core(rt.core_mut(), None, Ime::Preedit(String::new(), None, Vec::new()));
    rt.frame(0.0);
    let step = ime_sync_step(rt.core_mut(), &mut ime, true);
    assert_eq!(step, ImeStep { cancel: false, set_allowed: None, cursor_area: None });

    // Moving the caret sends the updated rectangle once, then rests again.
    ime_core(rt.core_mut(), None, Ime::Commit("abc".into()));
    rt.frame(0.0);
    let step = ime_sync_step(rt.core_mut(), &mut ime, true);
    let moved = step.cursor_area.expect("caret advance sends new cursor area");
    assert_ne!(moved, area);
    assert_eq!(step.set_allowed, None);
    rt.frame(0.0);
    assert_eq!(ime_sync_step(rt.core_mut(), &mut ime, true).cursor_area, None);

    // An IME reset toggles allowance and resends the same rectangle.
    rt.core_mut().edit.ime_reset = true;
    let step = ime_sync_step(rt.core_mut(), &mut ime, true);
    assert_eq!(step, ImeStep { cancel: true, set_allowed: Some(true), cursor_area: Some(moved) });
    assert_eq!(ime_sync_step(rt.core_mut(), &mut ime, true).cursor_area, None);

    // A platform Ime::Enabled/Disabled invalidates the sent area so a late text_input enter gets it.
    ime.invalidate_area();
    let step = ime_sync_step(rt.core_mut(), &mut ime, true);
    assert_eq!(step, ImeStep { cancel: false, set_allowed: None, cursor_area: Some(moved) });

    // Scale factor changes resend even when logical coordinates stay identical (Windows/X11 physical coords).
    rt.core_mut().set_viewport((240.0, 120.0), 2.0);
    rt.frame(0.0);
    let step = ime_sync_step(rt.core_mut(), &mut ime, true);
    assert_eq!(step.set_allowed, None);
    let scaled_area = step.cursor_area.expect("scale factor change resends cursor area");
    assert_eq!(ime_sync_step(rt.core_mut(), &mut ime, true).cursor_area, None);

    // Unfocused window disables IME and does not send a cursor area; regaining focus resends.
    let step = ime_sync_step(rt.core_mut(), &mut ime, false);
    assert_eq!(step, ImeStep { cancel: false, set_allowed: Some(false), cursor_area: None });
    assert_eq!(ime_sync_step(rt.core_mut(), &mut ime, false), ImeStep { cancel: false, set_allowed: None, cursor_area: None });
    let step = ime_sync_step(rt.core_mut(), &mut ime, true);
    assert_eq!(step, ImeStep { cancel: false, set_allowed: Some(true), cursor_area: Some(scaled_area) });

    // Direct clear (focus loss / hide) forgets both allowance and area.
    ime.clear();
    let step = ime_sync_step(rt.core_mut(), &mut ime, true);
    assert_eq!(step, ImeStep { cancel: false, set_allowed: Some(true), cursor_area: Some(scaled_area) });

    // Blurring the field disables IME without sending an area; refocusing sends it again.
    let (bx, by) = rt.find("btn").unwrap();
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.0);
    let step = ime_sync_step(rt.core_mut(), &mut ime, true);
    assert_eq!(step.set_allowed, Some(false));
    assert_eq!(step.cursor_area, None);
    rt.pointer_down(fx, fy);
    rt.pointer_up(fx, fy);
    rt.frame(0.0);
    let step = ime_sync_step(rt.core_mut(), &mut ime, true);
    assert_eq!(step.set_allowed, Some(true));
    assert!(step.cursor_area.is_some());
}

#[test]
fn wheel_ctrl_shift_keeps_vertical_delta_for_pinch_and_on_wheel() {
    #[derive(Default)]
    struct S { wheels: Vec<WheelEvent> }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().size(200.0, 200.0).hit()
            .on_wheel(|s: &mut S, ev, _| { s.wheels.push(ev); true })
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(50.0, 50.0);

    wheel(
        rt.core_mut(),
        MouseScrollDelta::LineDelta(0.0, -1.0),
        GesturePhase::Moved,
        1.0,
        ModifiersState::CONTROL | ModifiersState::SHIFT,
    );
    rt.frame(0.0);
    assert_eq!(rt.state.wheels.len(), 1);
    assert_eq!(rt.state.wheels[0].dx, 0.0, "Ctrl+Shift+wheel does not turn horizontal");
    assert!(rt.state.wheels[0].dy < 0.0, "Ctrl+Shift+wheel preserves vertical dy");
}

#[test]
fn ime_sync_skips_unchanged_area_for_app_ime_cursor() {
    struct S { caret_x: f32 }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        el().size(200.0, 100.0)
            .child(el().key("caret").absolute(s.caret_x, 20.0).size(2.0, 18.0).ime_cursor())
    }
    let mut rt = Runtime::new(S { caret_x: 16.0 }, view, (200.0, 100.0), Env::default());
    rt.frame(0.0);

    let mut ime = ImeSync::default();
    let first = ime_sync_step(rt.core_mut(), &mut ime, true);
    assert_eq!(first.set_allowed, Some(true));
    assert_eq!(first.cursor_area, Some([16.0, 20.0, 2.0, 18.0]));

    // Repeated frames with the same El::ime_cursor rect send nothing.
    rt.frame(0.0);
    assert_eq!(ime_sync_step(rt.core_mut(), &mut ime, true), ImeStep { cancel: false, set_allowed: None, cursor_area: None });

    // Moving the app-drawn caret sends the new rect once.
    rt.update(|s, tx| { s.caret_x = 40.0; tx.instant(); });
    rt.frame(0.0);
    assert_eq!(ime_sync_step(rt.core_mut(), &mut ime, true), ImeStep { cancel: false, set_allowed: None, cursor_area: Some([40.0, 20.0, 2.0, 18.0]) });
    rt.frame(0.0);
    assert_eq!(ime_sync_step(rt.core_mut(), &mut ime, true).cursor_area, None);
}

#[test]
fn web_primary_up_activates_only_when_release_focuses_an_editable_field() {
    #[derive(Default)]
    struct S {
        edit: crate::TextEdit,
        secret: crate::TextEdit,
        clicks: u32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        crate::col().size(240.0, 240.0)
            .child(crate::text("Recall").key("recall").size(100.0, 32.0).on_press_tx(|s: &mut S, tx| {
                s.edit.replace_text("Recalled text");
                s.edit.set_caret(s.edit.text.len());
                tx.focus("editor", false);
            }))
            .child(crate::text("Secret").key("focus_secret").size(100.0, 32.0).on_press_tx(|_: &mut S, tx| {
                tx.focus("secret", false);
            }))
            .child(crate::text("Control").key("focus_btn").size(100.0, 32.0).on_press_tx(|_: &mut S, tx| {
                tx.focus("other", false);
            }))
            .child(crate::text("Count").key("plain").size(100.0, 32.0).on_press(|s: &mut S| {
                s.clicks += 1;
            }))
            .child(el().key("other").size(100.0, 24.0).focusable(true))
            .child(crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0))
                .key("editor").label("Editor").size(200.0, 32.0).on_edit(|s: &mut S| &mut s.edit))
            .child(crate::text_input(&s.secret, crate::TextStyle::new(14.0, 400.0))
                .key("secret").label("Secret").secure().size(200.0, 32.0).on_edit(|s: &mut S| &mut s.secret))
    }
    let mut rt = Runtime::new(S::default(), view, (240.0, 240.0), Env::default());
    rt.frame(0.0);
    let mut mirror = crate::runtime::web_runtime::Mirror::default();
    let mut control_click = false;

    let click_release = |rt: &mut Runtime<S>, control_click: &mut bool, mirror: &mut crate::runtime::web_runtime::Mirror, key: &str| {
        let (x, y) = rt.find(key).unwrap();
        let down = {
            let (core, mut app) = rt.split_mut();
            mouse_button_core(core, &mut app, MouseButtonInput {
                x,
                y,
                button: crate::PointerButton::Primary,
                pressed: true,
                mods: crate::Mods::default(),
                ctrl_click_is_secondary: false,
                queue_mods_on_primary_down: false,
            }, control_click)
        };
        assert_eq!(down, ButtonAction::PrimaryDown);
        assert!(web_mouse_activate(down).should_activate(rt.core()));
        rt.frame(0.0);
        rt.core().web_context(mirror);

        let up = {
            let (core, mut app) = rt.split_mut();
            mouse_button_core(core, &mut app, MouseButtonInput {
                x,
                y,
                button: crate::PointerButton::Primary,
                pressed: false,
                mods: crate::Mods::default(),
                ctrl_click_is_secondary: false,
                queue_mods_on_primary_down: false,
            }, control_click)
        };
        assert_eq!(up, ButtonAction::PrimaryUp);
        assert!(rt.core().web_release_needs_frame());
        rt.frame(0.0);
        rt.core().web_context(mirror);
        web_mouse_activate(up).should_activate(rt.core())
    };

    // Release handler focusing a plain editor must activate the DOM editing proxy with synced text and caret.
    assert!(click_release(&mut rt, &mut control_click, &mut mirror, "recall"));
    assert!(!mirror.key.is_empty());
    assert_eq!(mirror.text, "Recalled text");
    assert_eq!((mirror.anchor, mirror.focus), (13, 13));
    assert!(!mirror.secure);

    // Ordinary release handler that does not focus an editor must leave DOM focus alone.
    assert!(!click_release(&mut rt, &mut control_click, &mut mirror, "plain"));
    assert_eq!(rt.state.clicks, 1);
    assert!(mirror.key.is_empty());

    // Release handler focusing a non-input control must not activate the editing proxy.
    assert!(!click_release(&mut rt, &mut control_click, &mut mirror, "focus_btn"));
    assert!(rt.has_focus());
    assert!(mirror.key.is_empty());

    // Release handler focusing a secure field must activate its password proxy too.
    assert!(click_release(&mut rt, &mut control_click, &mut mirror, "focus_secret"));
    assert!(!mirror.key.is_empty());
    assert!(mirror.secure);

    // Non-primary mouse actions never activate even while an editor is focused.
    for action in [
        ButtonAction::SecondaryDown,
        ButtonAction::SecondaryUp,
        ButtonAction::OtherDown(crate::PointerButton::Middle),
        ButtonAction::OtherUp(crate::PointerButton::Middle),
    ] {
        assert!(!web_mouse_activate(action).should_activate(rt.core()));
    }
}

#[test]
fn ctrl_wheel_line_delta_uses_chrome_notch_step_and_keeps_pixel_delta_scale() {
    #[derive(Default)]
    struct S { pinches: Vec<crate::MagnifyEvent> }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().size(200.0, 200.0).hit()
            .on_magnify(|s: &mut S, ev, _| { s.pinches.push(ev); true })
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(50.0, 50.0);

    // One forward notch zooms in by Chrome's 1.2x step (+0.2), independent of OS scroll lines.
    wheel(rt.core_mut(), MouseScrollDelta::LineDelta(0.0, 1.0), GesturePhase::Moved, 1.0, ModifiersState::CONTROL);
    // One backward notch zooms out by 1/1.2x (-1/6) so forward + backward round-trips cleanly.
    wheel(rt.core_mut(), MouseScrollDelta::LineDelta(0.0, -1.0), GesturePhase::Moved, 1.0, ModifiersState::CONTROL);
    // Pixel-precise Ctrl+wheel keeps dy / 200.0 (at scale 2.0, physical 40 -> 20 pt -> 0.1).
    wheel(rt.core_mut(), MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 40.0)), GesturePhase::Moved, 2.0, ModifiersState::CONTROL);
    // Horizontal line-delta wheel does not pinch.
    wheel(rt.core_mut(), MouseScrollDelta::LineDelta(1.0, 0.0), GesturePhase::Moved, 1.0, ModifiersState::CONTROL);
    // Line-delta Ctrl+wheel is independent of scroll-line pixel distance (e.g. WHEEL_PAGESCROLL or 0 lines).
    let ctrl = crate::Mods { ctrl: true, ..crate::Mods::default() };
    assert_eq!(
        wheel_core(rt.core_mut(), WheelInput { dx: 0.0, dy: 700.0, ticks: Some(1.0), mods: ctrl, phase: GesturePhase::Moved, precise: false, notch: true, pinch: true }),
        WheelOutcome::Pinched,
    );
    assert_eq!(
        wheel_core(rt.core_mut(), WheelInput { dx: 0.0, dy: 0.0, ticks: Some(1.0), mods: ctrl, phase: GesturePhase::Moved, precise: false, notch: true, pinch: true }),
        WheelOutcome::Pinched,
    );
    rt.frame(0.0);

    assert_eq!(rt.state.pinches.len(), 5);
    let (fwd, back, px, page, zero_lines) = (
        rt.state.pinches[0],
        rt.state.pinches[1],
        rt.state.pinches[2],
        rt.state.pinches[3],
        rt.state.pinches[4],
    );
    assert!(fwd.wheel && back.wheel && px.wheel && page.wheel && zero_lines.wheel);
    assert!((fwd.delta - 0.2).abs() < 1e-5, "forward notch delta should be +0.2, got {}", fwd.delta);
    assert!(((1.0 + fwd.delta) * (1.0 + back.delta) - 1.0).abs() < 1e-5, "forward + backward must round-trip: fwd={}, back={}", fwd.delta, back.delta);
    assert!((px.delta - 0.1).abs() < 1e-5, "PixelDelta keeps dy / 200.0, got {}", px.delta);
    assert!((page.delta - 0.2).abs() < 1e-5, "page-scroll dy does not inflate Ctrl+wheel zoom, got {}", page.delta);
    assert!((zero_lines.delta - 0.2).abs() < 1e-5, "zero scroll lines still zooms by 1.2x per tick, got {}", zero_lines.delta);
}

#[test]
fn web_touch_end_does_not_activate_while_popup_menu_is_open_or_touch_ended_in_menu() {
    #[derive(Default)]
    struct S { edit: crate::TextEdit, secondary: u32 }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        crate::col().size(240.0, 200.0)
            .child(crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0))
                .key("editor").label("Editor").size(200.0, 32.0).on_edit(|s: &mut S| &mut s.edit))
            .child(crate::text("Custom").key("custom").size(120.0, 32.0)
                .on_secondary_press(|s: &mut S, _, _| { s.secondary += 1; }))
            .child(crate::text("Menu").key("open_menu").size(120.0, 32.0)
                .on_press_tx(|_: &mut S, tx| {
                    tx.popup_menu(crate::PopupMenu::new().item(crate::PopupItem::new("a", "A")), "open_menu", |_: &mut S, _, _| {});
                }))
    }
    let mut rt = Runtime::new(S::default(), view, (240.0, 200.0), Env::default());
    rt.frame(0.0);

    // 1. Long press on a focused text field opens field_menu and enters Mode::Menu.
    let (ex, ey) = rt.find("editor").unwrap();
    rt.touch(1, crate::TouchPhase::Started, ex, ey, 0.0);
    rt.frame(0.0);
    assert!(rt.core().web_has_editable_focus());
    rt.frame(0.6);
    assert!(rt.popup_menu().is_some(), "long press on text field must open field_menu");
    assert!(rt.core().touch_in_menu(), "long press must transition touch into Mode::Menu");

    // Lifting the finger must not activate the editing proxy while the menu is open.
    let ended_in_menu = rt.core().touch_in_menu();
    rt.touch(1, crate::TouchPhase::Ended, ex, ey, 0.6);
    let act_app = web_touch_end_activate(rt.core(), false, ended_in_menu);
    let act_doc = web_touch_end_activate(rt.core(), true, ended_in_menu);
    rt.frame(0.0);
    assert!(!act_app.should_activate(rt.core()));
    assert!(!act_doc.should_activate(rt.core()));
    assert!(!WebActivate::Always.should_activate(rt.core()), "popup menu must block activation");

    // 2. Once dismissed, a normal short tap on the field activates again in both modes.
    rt.resolve_popup(None).unwrap();
    rt.frame(0.0);
    assert!(rt.popup_menu().is_none());
    rt.touch(2, crate::TouchPhase::Started, ex, ey, 1.0);
    assert!(!rt.core().touch_in_menu());
    let ended_in_menu = rt.core().touch_in_menu();
    rt.touch(2, crate::TouchPhase::Ended, ex, ey, 1.05);
    let act_app = web_touch_end_activate(rt.core(), false, ended_in_menu);
    let act_doc = web_touch_end_activate(rt.core(), true, ended_in_menu);
    rt.frame(0.0);
    assert!(act_app.should_activate(rt.core()));
    assert!(act_doc.should_activate(rt.core()));

    // 3. Long press on a custom secondary-press element (no PopupMenu) still ends in Mode::Menu
    // and suppresses touch-end activation.
    let (cx, cy) = rt.find("custom").unwrap();
    rt.touch(3, crate::TouchPhase::Started, cx, cy, 2.0);
    rt.frame(0.6);
    assert_eq!(rt.state.secondary, 1);
    assert!(rt.popup_menu().is_none());
    assert!(rt.core().touch_in_menu());
    let ended_in_menu = rt.core().touch_in_menu();
    rt.touch(3, crate::TouchPhase::Ended, cx, cy, 2.6);
    assert!(!web_touch_end_activate(rt.core(), false, ended_in_menu).should_activate(rt.core()));

    // 4. A normal tap whose press handler opens a PopupMenu suppresses post-frame activation.
    let (mx, my) = rt.find("open_menu").unwrap();
    rt.touch(4, crate::TouchPhase::Started, mx, my, 3.0);
    let ended_in_menu = rt.core().touch_in_menu();
    rt.touch(4, crate::TouchPhase::Ended, mx, my, 3.05);
    let act = web_touch_end_activate(rt.core(), false, ended_in_menu);
    rt.frame(0.0);
    assert!(rt.popup_menu().is_some());
    assert!(!act.should_activate(rt.core()));
}
