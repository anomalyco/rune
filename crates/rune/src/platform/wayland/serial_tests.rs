use super::{CapturedSerial, InputKind, SerialEffect, SerialEvent, SerialState, step};

#[test]
fn seat_capabilities_request_and_release_pointer_touch_and_keyboard_only_when_advertised() {
    let mut s = SerialState::default();
    assert!(step(&mut s, SerialEvent::SeatAdded { seat: 1 }).is_empty());

    // Empty capabilities bind nothing.
    assert!(step(&mut s, SerialEvent::Capabilities { seat: 1, pointer: false, touch: false, keyboard: false }).is_empty());

    // Pointer added.
    assert_eq!(
        step(&mut s, SerialEvent::Capabilities { seat: 1, pointer: true, touch: false, keyboard: false }),
        vec![SerialEffect::GetPointer { seat: 1 }]
    );
    // Duplicate capability announcement is idempotent.
    assert!(step(&mut s, SerialEvent::Capabilities { seat: 1, pointer: true, touch: false, keyboard: false }).is_empty());

    // Touch and keyboard added alongside pointer.
    assert_eq!(
        step(&mut s, SerialEvent::Capabilities { seat: 1, pointer: true, touch: true, keyboard: true }),
        vec![SerialEffect::GetTouch { seat: 1 }, SerialEffect::GetKeyboard { seat: 1 }]
    );

    // Record pointer press, then lose pointer capability -> releases pointer and clears serial.
    step(&mut s, SerialEvent::PointerEnter { seat: 1, surface: 10 });
    step(&mut s, SerialEvent::PointerButton { seat: 1, serial: 44, pressed: true });
    assert_eq!(s.latest_for_surface(10), Some(CapturedSerial { seat: 1, serial: 44, kind: InputKind::Pointer }));

    assert_eq!(
        step(&mut s, SerialEvent::Capabilities { seat: 1, pointer: false, touch: true, keyboard: true }),
        vec![SerialEffect::ReleasePointer { seat: 1 }]
    );
    assert_eq!(s.latest_for_surface(10), None);

    // Lose keyboard capability -> releases keyboard.
    step(&mut s, SerialEvent::KeyboardKey { seat: 1, serial: 77, pressed: true });
    assert_eq!(s.latest_input(), Some(CapturedSerial { seat: 1, serial: 77, kind: InputKind::Keyboard }));
    assert_eq!(
        step(&mut s, SerialEvent::Capabilities { seat: 1, pointer: false, touch: true, keyboard: false }),
        vec![SerialEffect::ReleaseKeyboard { seat: 1 }]
    );
    assert_eq!(s.latest_input(), None);

    // Seat removal releases remaining touch.
    assert_eq!(
        step(&mut s, SerialEvent::SeatRemoved { seat: 1 }),
        vec![SerialEffect::ReleaseTouch { seat: 1 }]
    );
}

#[test]
fn multi_seat_touch_pointer_and_keyboard_separate_drag_press_from_selection_serials() {
    let mut s = SerialState::default();
    step(&mut s, SerialEvent::Capabilities { seat: 1, pointer: true, touch: true, keyboard: true });
    step(&mut s, SerialEvent::Capabilities { seat: 2, pointer: true, touch: false, keyboard: false });

    // Seat 1 presses surface 100 with serial 11.
    step(&mut s, SerialEvent::PointerEnter { seat: 1, surface: 100 });
    step(&mut s, SerialEvent::PointerButton { seat: 1, serial: 11, pressed: true });
    // Seat 2 presses surface 200 with serial 22.
    step(&mut s, SerialEvent::PointerEnter { seat: 2, surface: 200 });
    step(&mut s, SerialEvent::PointerButton { seat: 2, serial: 22, pressed: true });

    assert_eq!(s.latest_for_surface(100), Some(CapturedSerial { seat: 1, serial: 11, kind: InputKind::Pointer }));
    assert_eq!(s.latest_for_surface(200), Some(CapturedSerial { seat: 2, serial: 22, kind: InputKind::Pointer }));
    assert_eq!(s.latest_for_surface(300), None);

    // Later touch down on surface 100 supersedes the older pointer press on surface 100.
    step(&mut s, SerialEvent::TouchDown { seat: 1, surface: 100, id: 0, serial: 33 });
    assert_eq!(s.latest_for_surface(100), Some(CapturedSerial { seat: 1, serial: 33, kind: InputKind::Touch }));

    // Button release and keyboard keys update latest_input() for selection requests without
    // overwriting the press-only drag serial.
    step(&mut s, SerialEvent::PointerButton { seat: 2, serial: 23, pressed: false });
    assert_eq!(s.latest_for_surface(200), Some(CapturedSerial { seat: 2, serial: 22, kind: InputKind::Pointer }));
    assert_eq!(s.latest_input(), Some(CapturedSerial { seat: 2, serial: 23, kind: InputKind::Pointer }));

    step(&mut s, SerialEvent::KeyboardKey { seat: 1, serial: 99, pressed: true });
    assert_eq!(s.latest_input(), Some(CapturedSerial { seat: 1, serial: 99, kind: InputKind::Keyboard }));
    assert_eq!(s.latest_for_surface(100), Some(CapturedSerial { seat: 1, serial: 33, kind: InputKind::Touch }));

    // Pointer leave clears focus; TouchUp and TouchCancel clear active touches.
    step(&mut s, SerialEvent::PointerLeave { seat: 2, surface: 200 });
    step(&mut s, SerialEvent::TouchUp { seat: 1, id: 0 });
    step(&mut s, SerialEvent::TouchCancel { seat: 1 });
}

#[test]
fn buffered_press_in_private_queue_supersedes_previous_press_when_drained_before_start_drag() {
    let mut s = SerialState::default();
    step(&mut s, SerialEvent::Capabilities { seat: 1, pointer: true, touch: false, keyboard: false });
    step(&mut s, SerialEvent::PointerEnter { seat: 1, surface: 55 });
    step(&mut s, SerialEvent::PointerButton { seat: 1, serial: 10, pressed: true });
    step(&mut s, SerialEvent::PointerButton { seat: 1, serial: 11, pressed: false });

    assert_eq!(s.latest_for_surface(55), Some(CapturedSerial { seat: 1, serial: 10, kind: InputKind::Pointer }));

    let buffered_private_queue = [
        SerialEvent::PointerButton { seat: 1, serial: 20, pressed: true },
    ];
    let used = s.drain_pending_and_read(55, buffered_private_queue);
    assert_eq!(used, Some(CapturedSerial { seat: 1, serial: 20, kind: InputKind::Pointer }));
}
