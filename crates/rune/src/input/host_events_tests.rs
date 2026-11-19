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
