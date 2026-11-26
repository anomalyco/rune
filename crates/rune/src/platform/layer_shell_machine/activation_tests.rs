use std::collections::VecDeque;

use super::{InputEvent, InputSerial, Wire, run_request};

#[derive(Debug, PartialEq, Eq)]
enum Op {
    GetToken,
    SetSerial(u32),
    SetSurface,
    Commit,
    Roundtrip,
    DestroyToken,
    DestroyActivation,
    DrainPending,
    Flush,
}

struct MockWire {
    ops: Vec<Op>,
    replies: VecDeque<Result<Option<String>, ()>>,
}

impl MockWire {
    fn new(replies: impl IntoIterator<Item = Result<Option<String>, ()>>) -> Self {
        Self { ops: Vec::new(), replies: replies.into_iter().collect() }
    }
}

impl Wire for MockWire {
    fn get_activation_token(&mut self) { self.ops.push(Op::GetToken); }
    fn set_serial(&mut self, serial: u32) { self.ops.push(Op::SetSerial(serial)); }
    fn set_surface(&mut self) { self.ops.push(Op::SetSurface); }
    fn commit(&mut self) { self.ops.push(Op::Commit); }
    fn roundtrip(&mut self) -> Result<Option<String>, ()> {
        self.ops.push(Op::Roundtrip);
        self.replies.pop_front().unwrap_or(Ok(None))
    }
    fn destroy_token(&mut self) { self.ops.push(Op::DestroyToken); }
    fn destroy_activation(&mut self) { self.ops.push(Op::DestroyActivation); }
    fn drain_pending(&mut self) { self.ops.push(Op::DrainPending); }
    fn flush(&mut self) { self.ops.push(Op::Flush); }
}

#[test]
fn request_order_sets_serial_and_surface_before_commit_then_destroys_and_drains() {
    let mut wire = MockWire::new([Ok(Some("kwin-token-1".into()))]);
    let token = run_request(&mut wire, Some(42), false);
    assert_eq!(token.as_deref(), Some("kwin-token-1"));
    assert_eq!(
        wire.ops,
        [
            Op::GetToken,
            Op::SetSerial(42),
            Op::SetSurface,
            Op::Commit,
            Op::Roundtrip,
            Op::DestroyToken,
            Op::DestroyActivation,
            Op::DrainPending,
            Op::Flush,
        ]
    );
}

#[test]
fn request_without_serial_omits_set_serial_and_late_done_uses_second_roundtrip() {
    let mut wire = MockWire::new([Ok(None), Ok(Some("late-token".into()))]);
    let token = run_request(&mut wire, None, false);
    assert_eq!(token.as_deref(), Some("late-token"));
    assert_eq!(
        wire.ops,
        [
            Op::GetToken,
            Op::SetSurface,
            Op::Commit,
            Op::Roundtrip,
            Op::Roundtrip,
            Op::DestroyToken,
            Op::DestroyActivation,
            Op::DrainPending,
            Op::Flush,
        ]
    );
}

#[test]
fn closed_surface_skips_wire_and_empty_or_error_answers_return_none_with_cleanup() {
    let mut closed = MockWire::new([Ok(Some("should-not-run".into()))]);
    assert_eq!(run_request(&mut closed, Some(7), true), None);
    assert!(closed.ops.is_empty(), "a closed surface must not issue wire requests or roundtrips");

    let mut empty = MockWire::new([Ok(Some(String::new()))]);
    assert_eq!(run_request(&mut empty, Some(9), false), None);
    assert!(empty.ops.ends_with(&[Op::DestroyToken, Op::DestroyActivation, Op::DrainPending, Op::Flush]));

    let mut failed = MockWire::new([Err(()), Ok(Some("unreachable".into()))]);
    assert_eq!(run_request(&mut failed, Some(11), false), None);
    assert_eq!(
        failed.ops,
        [
            Op::GetToken,
            Op::SetSerial(11),
            Op::SetSurface,
            Op::Commit,
            Op::Roundtrip,
            Op::DestroyToken,
            Op::DestroyActivation,
            Op::DrainPending,
            Op::Flush,
        ]
    );
}

#[test]
fn input_serial_captures_pointer_key_and_touch_presses_and_keeps_newest() {
    let mut serial = InputSerial::default();
    assert_eq!(serial.get(), None);

    // Events outside/unfocused or releases must not record or overwrite a serial.
    serial.step(InputEvent::PointerButton { serial: 1, inside: false, pressed: true });
    serial.step(InputEvent::PointerButton { serial: 2, inside: true, pressed: false });
    serial.step(InputEvent::Key { serial: 3, focused: false, pressed: true });
    serial.step(InputEvent::Key { serial: 4, focused: true, pressed: false });
    serial.step(InputEvent::TouchDown { serial: 5, ours: false });
    assert_eq!(serial.get(), None);

    // Pointer press -> key press -> touch down -> pointer press: newest valid serial wins.
    serial.step(InputEvent::PointerButton { serial: 10, inside: true, pressed: true });
    assert_eq!(serial.get(), Some(10));
    serial.step(InputEvent::PointerButton { serial: 11, inside: true, pressed: false });
    assert_eq!(serial.get(), Some(10), "pointer release must not overwrite the press serial");

    serial.step(InputEvent::Key { serial: 20, focused: true, pressed: true });
    assert_eq!(serial.get(), Some(20));
    serial.step(InputEvent::Key { serial: 21, focused: true, pressed: false });
    assert_eq!(serial.get(), Some(20), "key release must not overwrite the press serial");

    serial.step(InputEvent::TouchDown { serial: 30, ours: true });
    assert_eq!(serial.get(), Some(30), "touch down on our surface captures its serial");
    serial.step(InputEvent::TouchDown { serial: 31, ours: false });
    assert_eq!(serial.get(), Some(30), "touch down on another surface must not overwrite ours");

    serial.step(InputEvent::PointerButton { serial: 40, inside: true, pressed: true });
    assert_eq!(serial.get(), Some(40));

    serial.step(InputEvent::Reset);
    assert_eq!(serial.get(), None);
}

#[test]
fn input_serial_captures_keyboard_enter_for_our_surface_only() {
    let mut serial = InputSerial::default();
    assert_eq!(serial.get(), None);

    // Keyboard focus entering another surface must not record a serial.
    serial.step(InputEvent::KeyboardEnter { serial: 40, ours: false });
    assert_eq!(serial.get(), None);

    // Keyboard focus entering our surface records its serial before any press happens.
    serial.step(InputEvent::KeyboardEnter { serial: 41, ours: true });
    assert_eq!(serial.get(), Some(41));

    // A later enter on another surface must not overwrite ours.
    serial.step(InputEvent::KeyboardEnter { serial: 42, ours: false });
    assert_eq!(serial.get(), Some(41));

    // A newer press overwrites the focus serial, and a later enter on our surface updates it.
    serial.step(InputEvent::Key { serial: 43, focused: true, pressed: true });
    assert_eq!(serial.get(), Some(43));
    serial.step(InputEvent::KeyboardEnter { serial: 44, ours: true });
    assert_eq!(serial.get(), Some(44));

    serial.step(InputEvent::Reset);
    assert_eq!(serial.get(), None);
}
