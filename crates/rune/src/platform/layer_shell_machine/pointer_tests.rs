//! `wl_pointer` frames through the pointer machine, checked against the units and signs a normal
//! Wayland window gets from winit.

#[test]
fn pointer_buttons_map_linux_codes_like_winit_wayland() {
    use super::button_from_linux;
    use crate::PointerButton;

    assert_eq!(button_from_linux(0x110), PointerButton::Primary);
    assert_eq!(button_from_linux(0x111), PointerButton::Secondary);
    assert_eq!(button_from_linux(0x112), PointerButton::Middle);
    assert_eq!(button_from_linux(0x113), PointerButton::Back);
    assert_eq!(button_from_linux(0x116), PointerButton::Back);
    assert_eq!(button_from_linux(0x114), PointerButton::Forward);
    assert_eq!(button_from_linux(0x115), PointerButton::Forward);
    assert_eq!(button_from_linux(0x117), PointerButton::Other(0x117));
}

#[test]
fn pointer_wheel_value120_and_discrete_match_wayland_window_units_and_signs() {
    use super::{Axis, AxisSource, Effect, Event, State, WHEEL_LINE_POINTS, step};
    use crate::input::host_events::WheelInput;
    use crate::{GesturePhase, Mods};

    let mut state = State::default();
    let plain = Mods::default();

    // Empty wl_pointer.frame (e.g. after motion or button) emits nothing.
    assert!(step(&mut state, Event::Frame { mods: plain, framed: true }).is_empty());

    // v8 mouse wheel: one notch down (value120 = +120) -> -120 window points, whole notch.
    assert!(step(&mut state, Event::AxisSource(AxisSource::Wheel)).is_empty());
    assert!(step(&mut state, Event::AxisValue120 { axis: Axis::Vertical, value120: 120 }).is_empty());
    assert!(step(&mut state, Event::Axis { axis: Axis::Vertical, value: 15.0 }).is_empty());
    let got: Vec<_> = step(&mut state, Event::Frame { mods: plain, framed: true }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: 0.0, dy: -WHEEL_LINE_POINTS, mods: plain, phase: GesturePhase::Moved,
        precise: false, notch: true, pinch: true,
    })]);

    // Compare directly against a normal window's Linux wheel_delta for LineDelta(0.0, -1.0).
    let win = crate::runtime::wheel_delta(winit::event::MouseScrollDelta::LineDelta(0.0, -1.0), 2.0, false, WHEEL_LINE_POINTS);
    assert_eq!(win, (0.0, -WHEEL_LINE_POINTS));

    // Order of axis / value120 / source inside a frame does not matter; upward notch is positive dy.
    step(&mut state, Event::Axis { axis: Axis::Vertical, value: -15.0 });
    step(&mut state, Event::AxisValue120 { axis: Axis::Vertical, value120: -120 });
    step(&mut state, Event::AxisSource(AxisSource::Wheel));
    let got: Vec<_> = step(&mut state, Event::Frame { mods: plain, framed: true }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: 0.0, dy: WHEEL_LINE_POINTS, mods: plain, phase: GesturePhase::Moved,
        precise: false, notch: true, pinch: true,
    })]);

    // High-resolution wheel (quarter detent = 30/120): -30 pt, not a whole notch.
    step(&mut state, Event::AxisSource(AxisSource::Wheel));
    step(&mut state, Event::AxisValue120 { axis: Axis::Vertical, value120: 30 });
    step(&mut state, Event::Axis { axis: Axis::Vertical, value: 3.75 });
    let got: Vec<_> = step(&mut state, Event::Frame { mods: plain, framed: true }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: 0.0, dy: -30.0, mods: plain, phase: GesturePhase::Moved,
        precise: false, notch: false, pinch: true,
    })]);

    // Pre-v8 axis_discrete: 1 detent matches value120 = 120.
    step(&mut state, Event::AxisSource(AxisSource::Wheel));
    step(&mut state, Event::AxisDiscrete { axis: Axis::Vertical, discrete: 1 });
    step(&mut state, Event::Axis { axis: Axis::Vertical, value: 15.0 });
    let got: Vec<_> = step(&mut state, Event::Frame { mods: plain, framed: true }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: 0.0, dy: -WHEEL_LINE_POINTS, mods: plain, phase: GesturePhase::Moved,
        precise: false, notch: true, pinch: true,
    })]);

    // Shift turns a purely vertical wheel horizontal; Ctrl is preserved for pinch-zoom.
    let shift = Mods { shift: true, ..plain };
    step(&mut state, Event::AxisSource(AxisSource::Wheel));
    step(&mut state, Event::AxisValue120 { axis: Axis::Vertical, value120: 120 });
    let got: Vec<_> = step(&mut state, Event::Frame { mods: shift, framed: true }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: -WHEEL_LINE_POINTS, dy: 0.0, mods: shift, phase: GesturePhase::Moved,
        precise: false, notch: true, pinch: true,
    })]);

    // WheelTilt horizontal scroll stays horizontal even with Shift held.
    step(&mut state, Event::AxisSource(AxisSource::WheelTilt));
    step(&mut state, Event::AxisValue120 { axis: Axis::Horizontal, value120: -120 });
    let got: Vec<_> = step(&mut state, Event::Frame { mods: shift, framed: true }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: WHEEL_LINE_POINTS, dy: 0.0, mods: shift, phase: GesturePhase::Moved,
        precise: false, notch: true, pinch: true,
    })]);
}

#[test]
fn pointer_touchpad_axis_frames_apply_finger_gain_and_track_phases() {
    use super::{Axis, AxisSource, Effect, Event, FINGER_POINTS_PER_AXIS_UNIT, State, step};
    use crate::input::host_events::WheelInput;
    use crate::{GesturePhase, Mods};

    let mut state = State::default();
    let plain = Mods::default();

    // First finger frame begins a phased gesture with 120/10 gain and diagonal accumulation.
    step(&mut state, Event::AxisSource(AxisSource::Finger));
    step(&mut state, Event::Axis { axis: Axis::Vertical, value: 2.5 });
    step(&mut state, Event::Axis { axis: Axis::Horizontal, value: -1.0 });
    let got: Vec<_> = step(&mut state, Event::Frame { mods: plain, framed: true }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: FINGER_POINTS_PER_AXIS_UNIT as f32,
        dy: (-2.5 * FINGER_POINTS_PER_AXIS_UNIT) as f32,
        mods: plain, phase: GesturePhase::Began, precise: true, notch: false, pinch: true,
    })]);

    // Stopping one axis while the other still moves keeps the gesture Moved.
    step(&mut state, Event::AxisSource(AxisSource::Finger));
    step(&mut state, Event::Axis { axis: Axis::Vertical, value: 3.0 });
    step(&mut state, Event::AxisStop { axis: Axis::Horizontal });
    let got: Vec<_> = step(&mut state, Event::Frame { mods: plain, framed: true }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: 0.0, dy: (-3.0 * FINGER_POINTS_PER_AXIS_UNIT) as f32,
        mods: plain, phase: GesturePhase::Moved, precise: true, notch: false, pinch: true,
    })]);

    // Final axis_stop ends the gesture with zero delta; a duplicate stop while idle emits nothing.
    step(&mut state, Event::AxisStop { axis: Axis::Vertical });
    let got: Vec<_> = step(&mut state, Event::Frame { mods: plain, framed: true }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: 0.0, dy: 0.0, mods: plain, phase: GesturePhase::Ended, precise: true, notch: false, pinch: true,
    })]);
    step(&mut state, Event::AxisStop { axis: Axis::Horizontal });
    assert!(step(&mut state, Event::Frame { mods: plain, framed: true }).is_empty());

    // Wheel axis-only frame between detents uses gain 1.0, stays unphased (Moved), and does not
    // stop the next touchpad scroll from starting with Began.
    step(&mut state, Event::AxisSource(AxisSource::Wheel));
    step(&mut state, Event::Axis { axis: Axis::Vertical, value: 5.0 });
    let got: Vec<_> = step(&mut state, Event::Frame { mods: plain, framed: true }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: 0.0, dy: -5.0, mods: plain, phase: GesturePhase::Moved, precise: true, notch: false, pinch: true,
    })]);

    // Continuous source uses the same finger gain and starts with Began; Reset clears active gesture
    // and any pending frame so re-entry starts cleanly with Began.
    step(&mut state, Event::AxisSource(AxisSource::Continuous));
    step(&mut state, Event::Axis { axis: Axis::Vertical, value: 1.0 });
    let got: Vec<_> = step(&mut state, Event::Frame { mods: plain, framed: true }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: 0.0, dy: -FINGER_POINTS_PER_AXIS_UNIT as f32,
        mods: plain, phase: GesturePhase::Began, precise: true, notch: false, pinch: true,
    })]);
    step(&mut state, Event::Axis { axis: Axis::Vertical, value: 4.0 });
    assert!(step(&mut state, Event::Reset).is_empty());
    assert!(step(&mut state, Event::Frame { mods: plain, framed: true }).is_empty());

    step(&mut state, Event::AxisSource(AxisSource::Finger));
    step(&mut state, Event::Axis { axis: Axis::Vertical, value: 1.0 });
    let got: Vec<_> = step(&mut state, Event::Frame { mods: plain, framed: true }).into_iter().collect();
    assert_eq!(got[0], Effect::Wheel(WheelInput {
        dx: 0.0, dy: -FINGER_POINTS_PER_AXIS_UNIT as f32,
        mods: plain, phase: GesturePhase::Began, precise: true, notch: false, pinch: true,
    }));

    // Unframed wl_pointer (< v5) uses gain 1.0 and unphased Moved.
    step(&mut state, Event::Reset);
    step(&mut state, Event::Axis { axis: Axis::Vertical, value: 15.0 });
    let got: Vec<_> = step(&mut state, Event::Frame { mods: plain, framed: false }).into_iter().collect();
    assert_eq!(got, [Effect::Wheel(WheelInput {
        dx: 0.0, dy: -15.0, mods: plain, phase: GesturePhase::Moved, precise: true, notch: false, pinch: true,
    })]);
}

#[test]
fn pointer_wheel_and_extra_buttons_drive_runtime_handlers() {
    use super::{Axis, AxisSource, Effect, Event, State, button_from_linux, step};
    use crate::input::host_events::wheel_core;
    use crate::{Cx, El, Env, GesturePhase, Mods, PointerButton, Runtime, WheelEvent, el};

    #[derive(Default)]
    struct S {
        wheels: Vec<WheelEvent>,
        middle: usize,
        side: usize,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().size(200.0, 200.0)
            .hit()
            .pointer_buttons([PointerButton::Back, PointerButton::Forward])
            .on_wheel(|s: &mut S, ev, _| { s.wheels.push(ev); true })
            .on_middle_press(|s: &mut S, _| { s.middle += 1; })
            .on_press(|s: &mut S| { s.side += 1; })
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(50.0, 50.0);

    // One wheel notch down reaches on_wheel with -120 pt.
    let mut ptr = State::default();
    step(&mut ptr, Event::AxisSource(AxisSource::Wheel));
    step(&mut ptr, Event::AxisValue120 { axis: Axis::Vertical, value120: 120 });
    for Effect::Wheel(wheel) in step(&mut ptr, Event::Frame { mods: Mods::default(), framed: true }) {
        wheel_core(rt.core_mut(), wheel);
    }
    rt.frame(0.0);
    assert_eq!(rt.state.wheels.len(), 1);
    assert_eq!((rt.state.wheels[0].dx, rt.state.wheels[0].dy), (0.0, -120.0));
    assert_eq!(rt.state.wheels[0].phase, GesturePhase::Moved);
    assert!(!rt.state.wheels[0].precise && rt.state.wheels[0].notch);

    // Middle (0x112) and side buttons (0x113 Back, 0x114 Forward) reach runtime button dispatch.
    let middle = button_from_linux(0x112);
    rt.button_down(50.0, 50.0, middle);
    rt.button_up(50.0, 50.0, middle);
    rt.frame(0.0);
    assert_eq!(rt.state.middle, 1);

    for code in [0x113u32, 0x114] {
        let btn = button_from_linux(code);
        rt.button_down(50.0, 50.0, btn);
        rt.button_up(50.0, 50.0, btn);
        rt.frame(0.0);
    }
    assert_eq!(rt.state.side, 2);
}
