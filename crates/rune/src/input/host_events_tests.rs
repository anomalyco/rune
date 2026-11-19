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
