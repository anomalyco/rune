use std::time::Duration;

use super::{Effect, Event, InputEvent, InputSerial, StartFlush, State, TIMEOUT, Wire, begin_request, finish_request, step};
use crate::time::Instant;

#[derive(Debug, PartialEq, Eq)]
enum Op {
    GetToken,
    SetSerial(u32),
    SetSurface,
    Commit,
    FlushStart,
    DestroyToken,
    DestroyActivation,
    DrainPending,
    Flush,
}

struct MockWire {
    ops: Vec<Op>,
    flush_result: StartFlush,
}

impl MockWire {
    fn new(flush_result: StartFlush) -> Self {
        Self { ops: Vec::new(), flush_result }
    }
}

impl Wire for MockWire {
    fn get_activation_token(&mut self) { self.ops.push(Op::GetToken); }
    fn set_serial(&mut self, serial: u32) { self.ops.push(Op::SetSerial(serial)); }
    fn set_surface(&mut self) { self.ops.push(Op::SetSurface); }
    fn commit(&mut self) { self.ops.push(Op::Commit); }
    fn flush_start(&mut self) -> StartFlush {
        self.ops.push(Op::FlushStart);
        self.flush_result
    }
    fn destroy_token(&mut self) { self.ops.push(Op::DestroyToken); }
    fn destroy_activation(&mut self) { self.ops.push(Op::DestroyActivation); }
    fn drain_pending(&mut self) { self.ops.push(Op::DrainPending); }
    fn flush(&mut self) { self.ops.push(Op::Flush); }
}

#[test]
fn begin_request_sets_serial_and_surface_before_commit_and_flushes_without_roundtrip() {
    let mut wire = MockWire::new(StartFlush::Sent);
    assert!(begin_request(&mut wire, Some(42), false));
    assert_eq!(
        wire.ops,
        [
            Op::GetToken,
            Op::SetSerial(42),
            Op::SetSurface,
            Op::Commit,
            Op::FlushStart,
        ]
    );

    finish_request(&mut wire);
    assert_eq!(
        wire.ops[5..],
        [
            Op::DrainPending,
            Op::DestroyToken,
            Op::DestroyActivation,
            Op::DrainPending,
            Op::Flush,
        ]
    );
}

#[test]
fn begin_request_without_serial_omits_set_serial() {
    let mut wire = MockWire::new(StartFlush::Sent);
    assert!(begin_request(&mut wire, None, false));
    assert_eq!(
        wire.ops,
        [
            Op::GetToken,
            Op::SetSurface,
            Op::Commit,
            Op::FlushStart,
        ]
    );
}

#[test]
fn closed_surface_skips_wire_and_fatal_flush_cleans_up_immediately() {
    let mut closed = MockWire::new(StartFlush::Sent);
    assert!(!begin_request(&mut closed, Some(7), true));
    assert!(closed.ops.is_empty(), "a closed surface must not issue wire requests");

    let mut failed = MockWire::new(StartFlush::Fatal);
    assert!(!begin_request(&mut failed, Some(11), false));
    assert_eq!(
        failed.ops,
        [
            Op::GetToken,
            Op::SetSerial(11),
            Op::SetSurface,
            Op::Commit,
            Op::FlushStart,
            Op::DrainPending,
            Op::DestroyToken,
            Op::DestroyActivation,
            Op::DrainPending,
            Op::Flush,
        ]
    );
}

#[test]
fn state_machine_completes_on_done_and_maps_empty_token_to_none() {
    let t0 = Instant::now();
    let deadline = t0 + TIMEOUT;

    let mut state = State::Pending { deadline };
    assert!(state.is_pending());
    assert_eq!(state.deadline(), Some(deadline));
    assert!(step(&mut state, Event::Tick { now: t0 + Duration::from_millis(10) }).is_empty());
    assert!(state.is_pending());

    let effects: Vec<_> = step(&mut state, Event::Done("niri-token-1".into())).into_iter().collect();
    assert_eq!(effects, [Effect::Complete(Some("niri-token-1".into()))]);
    assert_eq!(state, State::Finished);
    assert_eq!(state.deadline(), None);
    // Subsequent events after completion produce no duplicate callback.
    assert!(step(&mut state, Event::Done("dup".into())).is_empty());
    assert!(step(&mut state, Event::Tick { now: deadline + Duration::from_secs(1) }).is_empty());
    assert!(step(&mut state, Event::Cancel).is_empty());

    let mut empty = State::Pending { deadline };
    let effects: Vec<_> = step(&mut empty, Event::Done(String::new())).into_iter().collect();
    assert_eq!(effects, [Effect::Complete(None)]);
    assert_eq!(empty, State::Finished);
}

#[test]
fn state_machine_times_out_at_deadline_and_handles_failed_and_cancel_once() {
    let t0 = Instant::now();
    let deadline = t0 + TIMEOUT;

    let mut timed = State::Pending { deadline };
    assert!(step(&mut timed, Event::Tick { now: deadline - Duration::from_millis(1) }).is_empty());
    let effects: Vec<_> = step(&mut timed, Event::Tick { now: deadline }).into_iter().collect();
    assert_eq!(effects, [Effect::Complete(None)]);
    assert_eq!(timed, State::Finished);
    assert!(step(&mut timed, Event::Done("too-late".into())).is_empty());

    let mut failed = State::Pending { deadline };
    let effects: Vec<_> = step(&mut failed, Event::Failed).into_iter().collect();
    assert_eq!(effects, [Effect::Complete(None)]);
    assert_eq!(failed, State::Finished);

    let mut cancelled = State::Pending { deadline };
    let effects: Vec<_> = step(&mut cancelled, Event::Cancel).into_iter().collect();
    assert_eq!(effects, [Effect::Complete(None)]);
    assert_eq!(cancelled, State::Finished);
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
