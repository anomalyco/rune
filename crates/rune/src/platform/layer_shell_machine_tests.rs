//! Every short sequence through the layer surface's two machines, since both are fed by events
//! whose order the compositor chooses: a configure, a preferred scale and a close can arrive in
//! any order, as can a finger's down, motion, lift and cancel. Touch is checked against a
//! transcription of the code it replaced, so the conversion can't quietly change what an app sees.

use super::{readiness, touch};

/// Every sequence of up to `depth` symbols, shortest first, through `each`.
fn each_sequence<T: Copy>(alphabet: &[T], depth: usize, seq: &mut Vec<T>, each: &mut impl FnMut(&[T])) {
    each(seq);
    if seq.len() == depth {
        return;
    }
    for &symbol in alphabet {
        seq.push(symbol);
        each_sequence(alphabet, depth, seq, each);
        seq.pop();
    }
}

/// The old `touch_id: Option<i32>` and the input it pushed, transcribed.
#[derive(Default)]
struct OldTouch {
    id: Option<i32>,
}

impl OldTouch {
    fn step(&mut self, event: touch::Event) -> Vec<touch::Effect> {
        use touch::Effect;
        match event {
            touch::Event::Down { id, x, y } if self.id.is_none() => {
                self.id = Some(id);
                vec![Effect::Move(x, y), Effect::Press]
            }
            touch::Event::Motion { id, x, y } if self.id == Some(id) => vec![Effect::Move(x, y)],
            touch::Event::Up { id } if self.id == Some(id) => {
                self.id = None;
                vec![Effect::Release, Effect::Leave]
            }
            touch::Event::Cancel if self.id.is_some() => {
                self.id = None;
                vec![Effect::Leave, Effect::Release]
            }
            touch::Event::Gone => {
                self.id = None;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

#[test]
fn every_touch_sequence_matches_what_the_old_finger_did() {
    use touch::{Effect, Event};
    let alphabet = [
        Event::Down { id: 0, x: 1.0, y: 2.0 },
        Event::Down { id: 1, x: 3.0, y: 4.0 },
        Event::Motion { id: 0, x: 5.0, y: 6.0 },
        Event::Motion { id: 1, x: 7.0, y: 8.0 },
        Event::Up { id: 0 },
        Event::Up { id: 1 },
        Event::Cancel,
        Event::Gone,
    ];
    each_sequence(&alphabet, 5, &mut Vec::new(), &mut |seq| {
        let (mut finger, mut old) = (touch::State::default(), OldTouch::default());
        let (mut presses, mut releases, mut orphaned) = (0usize, 0usize, 0usize);
        for (i, &event) in seq.iter().enumerate() {
            // The seat going away lifts the finger without a release: the only press that ends
            // without one.
            if event == Event::Gone && old.id.is_some() {
                orphaned += 1;
            }
            let got: Vec<Effect> = touch::step(&mut finger, event).into_iter().collect();
            assert_eq!(got, old.step(event), "event {i} of {seq:?}");
            for effect in &got {
                match effect {
                    Effect::Press => presses += 1,
                    Effect::Release => releases += 1,
                    Effect::Move(..) | Effect::Leave => {}
                }
            }
            assert_eq!(matches!(finger, touch::State::Down { .. }), old.id.is_some(), "finger after {seq:?}");
            if let (touch::State::Down { id }, Some(old_id)) = (finger, old.id) {
                assert_eq!(id, old_id, "finger after {seq:?}");
            }
            // One finger: it never releases twice, and only a vanished seat leaves a press behind.
            assert!(releases <= presses && presses - releases <= 1 + orphaned, "presses {presses}, releases {releases}, orphaned {orphaned} in {seq:?}");
        }
    });
}

/// The old `configured`/`scale`/`closed` fields, transcribed literally: `configured` was the
/// size the compositor asked for (taken by a read, which is why it couldn't say "ready"), `scale`
/// the last one it sent, and `closed` latched. All three kept taking new values after a close;
/// nothing read them, because the caller hides on `closed`. `ever` is what readiness means, kept
/// beside them so the machine's `configured()` can be compared on every step.
#[derive(Default)]
struct OldReadiness {
    configured: Option<(u32, u32)>,
    scale: Option<i32>,
    closed: bool,
    ever: bool,
}

impl OldReadiness {
    fn step(&mut self, event: readiness::Event) {
        match event {
            readiness::Event::Configure { width, height } => {
                self.configured = Some((width, height));
                self.ever = true;
            }
            readiness::Event::Scale(scale) => self.scale = Some(scale),
            readiness::Event::Closed => self.closed = true,
        }
    }
}

/// What `overlay/draw.rs` uses from one `dispatch`: it hides on `closed`, and reads a size and a
/// scale only while the surface is still open. Both sides are projected through this, so a read
/// after a close is compared like any other instead of being skipped.
#[derive(Debug, PartialEq)]
struct Used {
    closed: bool,
    size: Option<(u32, u32)>,
    scale: Option<i32>,
}

fn used(closed: bool, size: Option<(u32, u32)>, scale: Option<i32>) -> Used {
    match closed {
        true => Used { closed, size: None, scale: None },
        false => Used { closed, size, scale },
    }
}

#[test]
fn every_readiness_sequence_reports_the_same_size_and_scale() {
    #[derive(Clone, Copy, Debug)]
    enum Symbol {
        Event(readiness::Event),
        Read,
    }
    use readiness::Event;
    let alphabet = [
        Symbol::Event(Event::Configure { width: 10, height: 20 }),
        Symbol::Event(Event::Configure { width: 30, height: 40 }),
        Symbol::Event(Event::Scale(2)),
        Symbol::Event(Event::Scale(3)),
        Symbol::Event(Event::Closed),
        Symbol::Read,
    ];
    each_sequence(&alphabet, 6, &mut Vec::new(), &mut |seq| {
        let (mut state, mut old) = (readiness::State::default(), OldReadiness::default());
        for (i, symbol) in seq.iter().enumerate() {
            match symbol {
                Symbol::Event(event) => {
                    readiness::step(&mut state, *event);
                    old.step(*event);
                }
                Symbol::Read => {
                    let got = used(state.closed(), state.take_size(), state.take_scale());
                    let was = used(old.closed, old.configured.take(), old.scale.take());
                    assert_eq!(got, was, "read {i} of {seq:?}");
                }
            }
            assert_eq!(state.closed(), old.closed, "closed after {seq:?}");
            assert_eq!(state.configured(), old.ever || old.closed, "configured after {seq:?}");
        }
    });
}

#[test]
fn a_second_configure_replaces_the_size_nobody_read() {
    let mut state = readiness::State::default();
    readiness::step(&mut state, readiness::Event::Scale(2));
    assert!(!state.configured());
    assert_eq!(state.size(), None, "no size before the first configure");
    readiness::step(&mut state, readiness::Event::Configure { width: 10, height: 20 });
    assert!(state.configured());
    readiness::step(&mut state, readiness::Event::Configure { width: 30, height: 40 });
    assert_eq!(state.size(), Some((30, 40)));
    assert_eq!(state.take_size(), Some((30, 40)));
    assert_eq!(state.take_size(), None, "read once");
    assert!(state.configured(), "reading the size doesn't unconfigure the surface");
    assert_eq!(state.take_scale(), Some(2), "a scale sent before the first configure is kept");
    readiness::step(&mut state, readiness::Event::Closed);
    assert!(state.closed());
    assert_eq!(state.take_size(), None);
}

#[test]
fn keyboard_focus_emits_focus_edges_and_releases_on_seat_loss_or_other_surface() {
    use super::keyboard::{FocusEffect, FocusEvent, FocusState, step};
    let mut state = FocusState::default();
    assert!(step(&mut state, FocusEvent::LeaveOurs).is_empty(), "leave while unfocused is a no-op");
    assert_eq!(
        step(&mut state, FocusEvent::EnterOurs).into_iter().collect::<Vec<_>>(),
        [FocusEffect::Focus(true)]
    );
    assert!(step(&mut state, FocusEvent::EnterOurs).is_empty(), "duplicate enter emits nothing");
    assert_eq!(
        step(&mut state, FocusEvent::EnterOther).into_iter().collect::<Vec<_>>(),
        [FocusEffect::Focus(false)],
        "entering another surface loses focus on ours"
    );
    assert_eq!(
        step(&mut state, FocusEvent::EnterOurs).into_iter().collect::<Vec<_>>(),
        [FocusEffect::Focus(true)]
    );
    assert_eq!(
        step(&mut state, FocusEvent::KeyboardGone).into_iter().collect::<Vec<_>>(),
        [FocusEffect::Focus(false)],
        "seat losing keyboard capability notifies focus loss"
    );
    assert!(step(&mut state, FocusEvent::LeaveOurs).is_empty());
}

#[test]
fn layer_shell_key_translation_handles_ctrl_modifiers_space_and_non_ascii_layouts() {
    use super::keyboard::{can_request_keys, key_event_from, supports_keys};
    use crate::{Key, KeyPolicy, Mods, PhysicalKeyCode};

    let ctrl = Mods { ctrl: true, cmd: true, ..Default::default() };
    // Holding Ctrl and pressing Shift/Alt yields utf32 == 0 -> must return None, never Command('\0').
    assert_eq!(key_event_from(None, 0, String::new(), ctrl, Some(PhysicalKeyCode::ShiftLeft)), None);
    // Ctrl+Space stays text space, matching window/keys.rs.
    assert_eq!(
        key_event_from(None, ' ' as u32, " ".into(), ctrl, Some(PhysicalKeyCode::Space)).map(|e| e.key.clone()),
        Some(Key::Text(" ".into()))
    );
    // Ctrl+V on Russian layout ('м' on KeyV) maps to US Command('v'), matching window/keys.rs.
    assert_eq!(
        key_event_from(None, 'м' as u32, "м".into(), ctrl, Some(PhysicalKeyCode::KeyV)).map(|e| e.key.clone()),
        Some(Key::Command('v'))
    );
    // Plain Ctrl+C maps to Command('c').
    assert_eq!(
        key_event_from(None, 'C' as u32, "C".into(), ctrl, Some(PhysicalKeyCode::KeyC)).map(|e| e.key.clone()),
        Some(Key::Command('c'))
    );
    // KeyPolicy::None must never report or request keyboard interactivity even with a seat.
    assert!(!can_request_keys(KeyPolicy::None, 4));
    assert!(!supports_keys(KeyPolicy::None, 4, true));
    assert!(can_request_keys(KeyPolicy::OnDemand, 4));
    assert!(supports_keys(KeyPolicy::OnDemand, 4, true));
    assert!(!supports_keys(KeyPolicy::OnDemand, 3, true));
    assert!(!supports_keys(KeyPolicy::OnDemand, 4, false));
}

#[test]
fn keymap_decode_reads_at_offset_zero_strips_nul_and_enforces_cap() {
    use super::keyboard::{MAX_KEYMAP_BYTES, decode_keymap};
    use std::io::{Error, ErrorKind};

    let bytes = b"xkb_keymap { xkb_keycodes {}; };\0";
    let mut seen_offset = None;
    let decoded = decode_keymap(bytes.len() as u32, |buf, offset| {
        seen_offset = Some(offset);
        buf.copy_from_slice(bytes);
        Ok(())
    });
    assert_eq!(seen_offset, Some(0));
    assert_eq!(decoded.as_deref(), Some("xkb_keymap { xkb_keycodes {}; };"));

    // Payload without a NUL terminator is accepted verbatim.
    let no_nul = b"xkb_keymap {};";
    let decoded = decode_keymap(no_nul.len() as u32, |buf, offset| {
        assert_eq!(offset, 0);
        buf.copy_from_slice(no_nul);
        Ok(())
    });
    assert_eq!(decoded.as_deref(), Some("xkb_keymap {};"));

    // Trailing padding (even non-UTF-8) after the first NUL terminator is ignored.
    let padded = b"xkb_keymap {};\0\0\xff";
    let decoded = decode_keymap(padded.len() as u32, |buf, _| {
        buf.copy_from_slice(padded);
        Ok(())
    });
    assert_eq!(decoded.as_deref(), Some("xkb_keymap {};"));

    // Empty and over-cap sizes are rejected before calling read_at.
    assert_eq!(decode_keymap(0, |_, _| panic!("must not read empty keymap")), None);
    assert_eq!(decode_keymap(MAX_KEYMAP_BYTES + 1, |_, _| panic!("must not allocate over cap")), None);

    // All-NUL payloads, read errors, and invalid UTF-8 return None.
    assert_eq!(decode_keymap(4, |buf, _| { buf.fill(0); Ok(()) }), None);
    assert_eq!(decode_keymap(8, |_, _| Err(Error::from(ErrorKind::UnexpectedEof))), None);
    assert_eq!(decode_keymap(2, |buf, _| { buf.copy_from_slice(&[0xff, 0xfe]); Ok(()) }), None);
}

#[test]
fn configure_wait_timeout_ms_rounds_up_submillisecond_and_clamps() {
    use super::configure_wait::{TIMEOUT, timeout_ms};
    use std::time::Duration;

    assert!(TIMEOUT >= Duration::from_secs(1) && TIMEOUT <= Duration::from_secs(5));
    assert_eq!(timeout_ms(Duration::from_nanos(1)), 1);
    assert_eq!(timeout_ms(Duration::from_nanos(999_999)), 1);
    assert_eq!(timeout_ms(Duration::from_millis(1)), 1);
    assert_eq!(timeout_ms(Duration::from_nanos(1_000_001)), 2);
    assert_eq!(timeout_ms(Duration::from_secs(2)), 2_000);
    assert_eq!(timeout_ms(Duration::MAX), i32::MAX);
}

#[test]
fn configure_wait_bounds_silent_and_chatty_compositors_and_handles_retries() {
    use super::configure_wait::{Driver, Flush, Outcome, Poll, Read, wait};
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::Duration;
    use crate::time::Instant;

    struct Guard {
        cancelled: Rc<Cell<usize>>,
        consumed: bool,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if !self.consumed {
                self.cancelled.set(self.cancelled.get() + 1);
            }
        }
    }

    enum Step {
        Poll {
            result: Poll,
            advance: Duration,
            read: Option<Read>,
            deliver_on_dispatch: Option<readiness::Event>,
        },
        AlreadyPending(Option<readiness::Event>),
    }

    struct FakeDriver {
        now: Instant,
        readiness: readiness::State,
        pending: VecDeque<readiness::Event>,
        flushes: VecDeque<Flush>,
        steps: VecDeque<Step>,
        polls: Vec<(i32, bool)>,
        reads: usize,
        cancelled: Rc<Cell<usize>>,
        dispatch_fail: bool,
    }

    impl FakeDriver {
        fn new(steps: impl IntoIterator<Item = Step>) -> Self {
            Self {
                now: crate::time::now(),
                readiness: readiness::State::default(),
                pending: VecDeque::new(),
                flushes: VecDeque::new(),
                steps: steps.into_iter().collect(),
                polls: Vec::new(),
                reads: 0,
                cancelled: Rc::new(Cell::new(0)),
                dispatch_fail: false,
            }
        }
    }

    impl Driver for FakeDriver {
        type Guard = Guard;

        fn done(&self) -> bool { self.readiness.configured() }

        fn now(&mut self) -> Instant { self.now }

        fn dispatch_pending(&mut self) -> bool {
            if self.dispatch_fail {
                return false;
            }
            while let Some(event) = self.pending.pop_front() {
                readiness::step(&mut self.readiness, event);
            }
            true
        }

        fn flush(&mut self) -> Flush {
            self.flushes.pop_front().unwrap_or(Flush::Ok)
        }

        fn prepare_read(&mut self) -> Option<Guard> {
            if let Some(Step::AlreadyPending(event)) = self.steps.front() {
                let event = *event;
                self.steps.pop_front();
                if let Some(event) = event {
                    self.pending.push_back(event);
                } else {
                    self.now += Duration::from_millis(50);
                }
                return None;
            }
            Some(Guard { cancelled: Rc::clone(&self.cancelled), consumed: false })
        }

        fn poll(&mut self, _guard: &Guard, timeout_ms: i32, want_write: bool) -> Poll {
            assert!(self.polls.len() < 16, "exceeded poll budget — unbounded configure wait");
            self.polls.push((timeout_ms, want_write));
            let Some(Step::Poll { result, advance, deliver_on_dispatch, .. }) = self.steps.front() else {
                panic!("unexpected poll call #{}: no scripted step left", self.polls.len());
            };
            let result = *result;
            self.now += *advance;
            if matches!(result, Poll::TimedOut) && let Some(event) = *deliver_on_dispatch {
                self.pending.push_back(event);
            }
            if !matches!(result, Poll::Readable) {
                self.steps.pop_front();
            }
            result
        }

        fn read(&mut self, mut guard: Guard) -> Read {
            guard.consumed = true;
            self.reads += 1;
            let Some(Step::Poll { read, deliver_on_dispatch, .. }) = self.steps.pop_front() else {
                panic!("read without matching Poll::Readable step");
            };
            if let Some(event) = deliver_on_dispatch {
                self.pending.push_back(event);
            }
            read.unwrap_or(Read::Ok)
        }
    }

    // 1. Silent compositor: never sends configure -> times out at deadline and cancels guard.
    let mut silent = FakeDriver::new([Step::Poll {
        result: Poll::TimedOut,
        advance: Duration::from_millis(200),
        read: None,
        deliver_on_dispatch: None,
    }]);
    assert_eq!(wait(&mut silent, Duration::from_millis(200)), Outcome::TimedOut);
    assert_eq!(silent.polls, [(200, false)]);
    assert_eq!(silent.reads, 0);
    assert_eq!(silent.cancelled.get(), 1, "timed-out prepare_read guard must be cancelled");
    assert!(!silent.readiness.configured());

    // 2. Chatty compositor: sends unrelated events (scale / other queue traffic) repeatedly
    // without ever configuring -> remaining timeout shrinks monotonically and ends at deadline.
    let mut chatty = FakeDriver::new([
        Step::Poll { result: Poll::Readable, advance: Duration::from_millis(70), read: Some(Read::Ok), deliver_on_dispatch: Some(readiness::Event::Scale(2)) },
        Step::Poll { result: Poll::Readable, advance: Duration::from_millis(80), read: Some(Read::WouldBlock), deliver_on_dispatch: None },
        Step::Poll { result: Poll::Readable, advance: Duration::from_millis(50), read: Some(Read::Ok), deliver_on_dispatch: None },
    ]);
    assert_eq!(wait(&mut chatty, Duration::from_millis(200)), Outcome::TimedOut);
    assert_eq!(chatty.polls, [(200, false), (130, false), (50, false)]);
    assert_eq!(chatty.reads, 3);
    assert_eq!(chatty.cancelled.get(), 0);
    assert_eq!(chatty.readiness.take_scale(), Some(2));

    // 3. EINTR and Flush::WouldBlock retry with the same overall deadline, then configure lands.
    let mut retries = FakeDriver::new([
        Step::Poll { result: Poll::WritableOnly, advance: Duration::from_millis(40), read: None, deliver_on_dispatch: None },
        Step::Poll { result: Poll::Interrupted, advance: Duration::from_millis(30), read: None, deliver_on_dispatch: None },
        Step::Poll {
            result: Poll::Readable,
            advance: Duration::from_millis(25),
            read: Some(Read::Ok),
            deliver_on_dispatch: Some(readiness::Event::Configure { width: 320, height: 80 }),
        },
    ]);
    retries.flushes = VecDeque::from([Flush::WouldBlock, Flush::Ok, Flush::Ok]);
    assert_eq!(wait(&mut retries, Duration::from_millis(200)), Outcome::Done);
    assert_eq!(retries.polls, [(200, true), (160, false), (130, false)]);
    assert_eq!(retries.reads, 1);
    assert_eq!(retries.cancelled.get(), 2, "WritableOnly and Interrupted guards must be cancelled");
    assert_eq!(retries.readiness.size(), Some((320, 80)));

    // 4. Compositor closing the surface ends the wait early as Done (caller inspects closed()).
    let mut closed = FakeDriver::new([Step::Poll {
        result: Poll::Readable,
        advance: Duration::from_millis(10),
        read: Some(Read::Ok),
        deliver_on_dispatch: Some(readiness::Event::Closed),
    }]);
    assert_eq!(wait(&mut closed, Duration::from_millis(200)), Outcome::Done);
    assert!(closed.readiness.closed());

    // 5. prepare_read returning None dispatches newly queued events without polling.
    let mut queued = FakeDriver::new([Step::AlreadyPending(Some(readiness::Event::Configure { width: 100, height: 50 }))]);
    assert_eq!(wait(&mut queued, Duration::from_millis(200)), Outcome::Done);
    assert!(queued.polls.is_empty());
    assert_eq!(queued.readiness.size(), Some((100, 50)));

    // 6. Fatal errors from flush, poll, read, or dispatch end immediately with Outcome::Error.
    let mut flush_err = FakeDriver::new([]);
    flush_err.flushes = VecDeque::from([Flush::Error]);
    assert_eq!(wait(&mut flush_err, Duration::from_millis(200)), Outcome::Error);

    let mut poll_err = FakeDriver::new([Step::Poll { result: Poll::Error, advance: Duration::ZERO, read: None, deliver_on_dispatch: None }]);
    assert_eq!(wait(&mut poll_err, Duration::from_millis(200)), Outcome::Error);
    assert_eq!(poll_err.cancelled.get(), 1);

    let mut read_err = FakeDriver::new([Step::Poll { result: Poll::Readable, advance: Duration::ZERO, read: Some(Read::Error), deliver_on_dispatch: None }]);
    assert_eq!(wait(&mut read_err, Duration::from_millis(200)), Outcome::Error);

    let mut dispatch_err = FakeDriver::new([]);
    dispatch_err.dispatch_fail = true;
    assert_eq!(wait(&mut dispatch_err, Duration::from_millis(200)), Outcome::Error);
}

#[test]
fn fullscreen_placement_uses_exclusive_zone_minus_one_and_others_stay_zero() {
    use super::exclusive_zone;
    use crate::overlay::ScreenEdge;
    use crate::{Align, OverlayPlacement};

    assert_eq!(exclusive_zone(Some(OverlayPlacement::FullScreen)), -1);
    assert_eq!(exclusive_zone(None), 0);
    assert_eq!(
        exclusive_zone(Some(OverlayPlacement::Edge {
            edge: ScreenEdge::Top,
            margin: 8.0,
            align: Align::Center,
            inset: 0.0,
        })),
        0
    );
    assert_eq!(
        exclusive_zone(Some(OverlayPlacement::Rect { x: 10.0, y: 20.0, width: 200.0, height: 100.0 })),
        0
    );
}

#[test]
fn key_repeat_uses_default_delay_and_rate_until_repeat_info_and_ignores_stale_or_early_timers() {
    use super::repeat::{Effect, Event, State, step};
    use crate::machine::Seq;
    use std::time::Duration;

    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = State::default();
    assert_eq!(state.deadline(), None);
    assert_eq!(state.attempt(), None);
    assert_eq!(state.key(), None);

    // Default policy before wl_keyboard.repeat_info arrives: 200 ms delay, 25 Hz (40 ms).
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 14, repeats: true, now: t0 }).into_iter().collect();
    let first_due = t0 + Duration::from_millis(200);
    assert_eq!(effects, [Effect::WakeAt(first_due)]);
    assert_eq!(state.deadline(), Some(first_due));
    assert_eq!(state.key(), Some(14));
    let a1 = state.attempt().expect("armed attempt");

    // Early wake changes nothing.
    assert!(step(&mut seq, &mut state, Event::Due { attempt: a1, now: t0 + Duration::from_millis(199) }).is_empty());
    assert_eq!(state.deadline(), Some(first_due));
    assert_eq!(state.attempt(), Some(a1));

    // At the initial delay: fires once and advances by 40 ms with a fresh attempt.
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Due { attempt: a1, now: first_due }).into_iter().collect();
    let second_due = first_due + Duration::from_millis(40);
    assert_eq!(effects, [Effect::Repeat { key: 14 }, Effect::WakeAt(second_due)]);
    let a2 = state.attempt().expect("second armed attempt");
    assert_ne!(a1, a2);
    assert_eq!(state.deadline(), Some(second_due));

    // Stale attempt from before the first repeat is ignored even at or past the new deadline.
    assert!(step(&mut seq, &mut state, Event::Due { attempt: a1, now: second_due }).is_empty());
    assert_eq!(state.deadline(), Some(second_due));

    // Second repeat fires at 240 ms.
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Due { attempt: a2, now: second_due }).into_iter().collect();
    let third_due = second_due + Duration::from_millis(40);
    assert_eq!(effects, [Effect::Repeat { key: 14 }, Effect::WakeAt(third_due)]);
    let a3 = state.attempt().expect("third armed attempt");

    // A stall (wake way past due) emits one repeat without bursting and schedules strictly in the future.
    let stalled_now = t0 + Duration::from_secs(2);
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Due { attempt: a3, now: stalled_now }).into_iter().collect();
    assert_eq!(effects, [Effect::Repeat { key: 14 }, Effect::WakeAt(stalled_now + Duration::from_millis(40))]);
}

#[test]
fn key_repeat_applies_compositor_repeat_info_and_disables_on_rate_zero() {
    use super::repeat::{Effect, Event, Policy, State, step};
    use crate::machine::Seq;
    use std::time::Duration;

    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = State::default();

    // Custom compositor rate (10 Hz -> 100 ms) and delay (500 ms).
    assert!(step(&mut seq, &mut state, Event::RepeatInfo { rate: 10, delay_ms: 500 }).is_empty());
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 105, repeats: true, now: t0 }).into_iter().collect();
    let first_due = t0 + Duration::from_millis(500);
    assert_eq!(effects, [Effect::WakeAt(first_due)]);
    let a1 = state.attempt().unwrap();

    // Dynamic RepeatInfo update while held changes subsequent interval (50 Hz -> 20 ms) and future delay (300 ms).
    assert!(step(&mut seq, &mut state, Event::RepeatInfo { rate: 50, delay_ms: 300 }).is_empty());
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Due { attempt: a1, now: first_due }).into_iter().collect();
    assert_eq!(effects, [Effect::Repeat { key: 105 }, Effect::WakeAt(first_due + Duration::from_millis(20))]);
    let a2 = state.attempt().unwrap();

    // rate == 0 stops an active repeat immediately and prevents future repeats.
    assert!(step(&mut seq, &mut state, Event::RepeatInfo { rate: 0, delay_ms: 200 }).is_empty());
    assert_eq!(state.deadline(), None);
    assert_eq!(state.attempt(), None);
    assert!(step(&mut seq, &mut state, Event::Due { attempt: a2, now: first_due + Duration::from_secs(1) }).is_empty());
    assert!(step(&mut seq, &mut state, Event::Press { key: 105, repeats: true, now: t0 + Duration::from_secs(2) }).is_empty());
    assert_eq!(state.deadline(), None);

    // Re-enabling repeat restores repeating with the new values; negative delay clamps to zero
    // and negative rate disables cleanly.
    assert_eq!(Policy::from_wayland(-1, 200), Policy::Disabled);
    assert_eq!(Policy::from_wayland(25, -50), Policy::Repeat { delay: Duration::ZERO, interval: Duration::from_millis(40) });
    assert_eq!(Policy::from_wayland(i32::MAX, 100), Policy::Repeat { delay: Duration::from_millis(100), interval: Duration::from_nanos(1) });

    assert!(step(&mut seq, &mut state, Event::RepeatInfo { rate: 50, delay_ms: 300 }).is_empty());
    let t3 = t0 + Duration::from_secs(3);
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 106, repeats: true, now: t3 }).into_iter().collect();
    assert_eq!(effects, [Effect::WakeAt(t3 + Duration::from_millis(300))]);
}

#[test]
fn key_repeat_filters_non_repeating_keys_and_stops_on_release_leave_or_keyboard_loss() {
    use super::repeat::{Effect, Event, State, step};
    use crate::machine::Seq;
    use std::time::Duration;

    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = State::default();
    step(&mut seq, &mut state, Event::RepeatInfo { rate: 20, delay_ms: 250 });

    // Non-repeating key (e.g. Shift) while idle does not arm a timer.
    assert!(step(&mut seq, &mut state, Event::Press { key: 42, repeats: false, now: t0 }).is_empty());
    assert_eq!(state.deadline(), None);

    // Repeating key A arms a timer; pressing and releasing Shift does not interrupt A.
    step(&mut seq, &mut state, Event::Press { key: 30, repeats: true, now: t0 });
    let a_attempt = state.attempt().unwrap();
    let a_due = t0 + Duration::from_millis(250);
    assert!(step(&mut seq, &mut state, Event::Press { key: 42, repeats: false, now: t0 + Duration::from_millis(50) }).is_empty());
    assert!(step(&mut seq, &mut state, Event::Release { key: 42 }).is_empty());
    assert_eq!(state.attempt(), Some(a_attempt));
    assert_eq!(state.deadline(), Some(a_due));

    // Pressing another repeating key B replaces A and invalidates A's attempt; releasing A keeps B repeating.
    let t1 = t0 + Duration::from_millis(100);
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 48, repeats: true, now: t1 }).into_iter().collect();
    let b_due = t1 + Duration::from_millis(250);
    assert_eq!(effects, [Effect::WakeAt(b_due)]);
    let b_attempt = state.attempt().unwrap();
    assert_ne!(a_attempt, b_attempt);
    assert!(step(&mut seq, &mut state, Event::Due { attempt: a_attempt, now: b_due }).is_empty());
    assert!(step(&mut seq, &mut state, Event::Release { key: 30 }).is_empty());
    assert_eq!(state.key(), Some(48));
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Due { attempt: b_attempt, now: b_due }).into_iter().collect();
    assert_eq!(effects, [Effect::Repeat { key: 48 }, Effect::WakeAt(b_due + Duration::from_millis(50))]);

    // Releasing B stops repeating.
    let b_next = state.attempt().unwrap();
    assert!(step(&mut seq, &mut state, Event::Release { key: 48 }).is_empty());
    assert_eq!(state.deadline(), None);
    assert!(step(&mut seq, &mut state, Event::Due { attempt: b_next, now: b_due + Duration::from_secs(1) }).is_empty());

    // Stop (focus leave / enter other / drain) cancels repeat but keeps the compositor's RepeatInfo (250 ms).
    let t2 = t0 + Duration::from_secs(2);
    step(&mut seq, &mut state, Event::Press { key: 14, repeats: true, now: t2 });
    let c_attempt = state.attempt().unwrap();
    assert!(step(&mut seq, &mut state, Event::Stop).is_empty());
    assert_eq!(state.deadline(), None);
    assert!(step(&mut seq, &mut state, Event::Due { attempt: c_attempt, now: t2 + Duration::from_secs(1) }).is_empty());
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 14, repeats: true, now: t2 }).into_iter().collect();
    assert_eq!(effects, [Effect::WakeAt(t2 + Duration::from_millis(250))]);

    // KeyboardGone cancels repeat and resets RepeatInfo back to default (200 ms / 40 ms) for the next wl_keyboard.
    step(&mut seq, &mut state, Event::RepeatInfo { rate: 0, delay_ms: 999 });
    step(&mut seq, &mut state, Event::KeyboardGone);
    assert_eq!(state.deadline(), None);
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 14, repeats: true, now: t2 }).into_iter().collect();
    assert_eq!(effects, [Effect::WakeAt(t2 + Duration::from_millis(200))]);
}

#[test]
fn dead_key_compose_emits_composed_text_and_suppresses_composing_and_cancelled() {
    use super::keyboard::{ComposeAction, ComposeOutcome, compose_action, compose_locale, key_event_with_compose};
    use crate::{Key, KeyEvent, Mods, PhysicalKeyCode};

    let none = Mods::default();
    let shift = Mods { shift: true, ..Default::default() };
    let ctrl = Mods { ctrl: true, cmd: true, ..Default::default() };
    let alt = Mods { alt: true, ..Default::default() };
    let logo = Mods { logo: true, ..Default::default() };

    // Only non-repeat presses feed compose; command chords reset and bypass; releases and repeats bypass.
    assert_eq!(compose_action(true, false, none), ComposeAction::Feed);
    assert_eq!(compose_action(true, false, shift), ComposeAction::Feed);
    assert_eq!(compose_action(true, false, alt), ComposeAction::Feed);
    assert_eq!(compose_action(true, false, ctrl), ComposeAction::ResetAndBypass);
    assert_eq!(compose_action(true, false, Mods { ctrl: true, ..Default::default() }), ComposeAction::ResetAndBypass);
    assert_eq!(compose_action(true, false, logo), ComposeAction::ResetAndBypass);
    assert_eq!(compose_action(false, false, none), ComposeAction::Bypass);
    assert_eq!(compose_action(true, true, none), ComposeAction::Bypass);
    assert_eq!(compose_action(false, false, ctrl), ComposeAction::Bypass);

    // Dead key press (Composing) suppresses both bare dead keys and mid-sequence Multi_key letters.
    assert_eq!(key_event_with_compose(ComposeOutcome::Composing, None, 0, String::new(), none, Some(PhysicalKeyCode::BracketLeft)), None);
    assert_eq!(key_event_with_compose(ComposeOutcome::Composing, None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)), None);

    // Completing dead_acute + a emits "á" instead of uncomposed "a".
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Composed(Some("á".into())), None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)),
        Some(KeyEvent { key: Key::Text("á".into()), mods: none })
    );
    // Completing dead_acute + Shift+A preserves Shift in mods and emits "Á".
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Composed(Some("Á".into())), None, 'A' as u32, "A".into(), shift, Some(PhysicalKeyCode::KeyA)),
        Some(KeyEvent { key: Key::Text("Á".into()), mods: shift })
    );
    // Double dead key (dead_acute + dead_acute) emits "´" even though raw xkb utf8 is empty.
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Composed(Some("´".into())), None, 0, String::new(), none, Some(PhysicalKeyCode::BracketLeft)),
        Some(KeyEvent { key: Key::Text("´".into()), mods: none })
    );
    // Multi-character compose results emit verbatim; empty, missing, or control results emit None.
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Composed(Some("ff".into())), None, 'f' as u32, "f".into(), none, Some(PhysicalKeyCode::KeyF)),
        Some(KeyEvent { key: Key::Text("ff".into()), mods: none })
    );
    assert_eq!(key_event_with_compose(ComposeOutcome::Composed(None), None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)), None);
    assert_eq!(key_event_with_compose(ComposeOutcome::Composed(Some(String::new())), None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)), None);
    assert_eq!(key_event_with_compose(ComposeOutcome::Composed(Some("\x07".into())), None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)), None);

    // Cancelled compose suppresses uncomposed letters (e.g. dead_acute + q) but still delivers named keys (Escape/Backspace).
    assert_eq!(key_event_with_compose(ComposeOutcome::Cancelled, None, 'q' as u32, "q".into(), none, Some(PhysicalKeyCode::KeyQ)), None);
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Cancelled, Some(Key::Escape), 0x1b, "\x1b".into(), none, Some(PhysicalKeyCode::Escape)),
        Some(KeyEvent { key: Key::Escape, mods: none })
    );
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Cancelled, Some(Key::Backspace), 0x08, "\x08".into(), none, Some(PhysicalKeyCode::Backspace)),
        Some(KeyEvent { key: Key::Backspace, mods: none })
    );

    // Nothing falls back to uncomposed key_event_from (plain text, named keys, and command chords).
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Nothing, None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)),
        Some(KeyEvent { key: Key::Text("a".into()), mods: none })
    );
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Nothing, None, 'v' as u32, "v".into(), ctrl, Some(PhysicalKeyCode::KeyV)),
        Some(KeyEvent { key: Key::Command('v'), mods: ctrl })
    );

    // Locale precedence: LC_ALL > LC_CTYPE > LANG > "C", skipping empty or NUL-containing values.
    assert_eq!(compose_locale(|k| match k { "LC_ALL" => Some("es_AR.UTF-8".into()), "LC_CTYPE" => Some("fr_FR.UTF-8".into()), "LANG" => Some("en_US.UTF-8".into()), _ => None }), "es_AR.UTF-8");
    assert_eq!(compose_locale(|k| match k { "LC_ALL" => Some(String::new()), "LC_CTYPE" => Some("es_ES.UTF-8".into()), "LANG" => Some("en_US.UTF-8".into()), _ => None }), "es_ES.UTF-8");
    assert_eq!(compose_locale(|k| match k { "LC_ALL" => Some("bad\0loc".into()), "LANG" => Some("en_US.UTF-8".into()), _ => None }), "en_US.UTF-8");
    assert_eq!(compose_locale(|_| None), "C");
}

#[test]
fn watch_loop_wakes_on_prequeued_and_read_events_releases_guard_before_wait_and_stops_cleanly() {
    use super::watch_loop::{Dispatch, Driver, Poll, run};
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    struct Guard {
        live: Rc<Cell<usize>>,
        cancelled: Rc<Cell<usize>>,
        consumed: bool,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.live.set(self.live.get() - 1);
            if !self.consumed {
                self.cancelled.set(self.cancelled.get() + 1);
            }
        }
    }

    enum Step {
        PreQueued(Dispatch),
        Poll { result: Poll, read: Option<Dispatch> },
    }

    struct FakeDriver {
        steps: VecDeque<Step>,
        stopped: bool,
        stop_on_wake: Option<usize>,
        live_guards: Rc<Cell<usize>>,
        cancelled: Rc<Cell<usize>>,
        polls: usize,
        reads: usize,
        inner_dispatches: usize,
        wakes: usize,
        guard_held_during_wake: bool,
    }

    impl FakeDriver {
        fn new(steps: impl IntoIterator<Item = Step>) -> Self {
            Self {
                steps: steps.into_iter().collect(),
                stopped: false,
                stop_on_wake: None,
                live_guards: Rc::new(Cell::new(0)),
                cancelled: Rc::new(Cell::new(0)),
                polls: 0,
                reads: 0,
                inner_dispatches: 0,
                wakes: 0,
                guard_held_during_wake: false,
            }
        }
    }

    impl Driver for FakeDriver {
        type Guard = Guard;

        fn stopped(&self) -> bool { self.stopped }

        fn prepare_read(&mut self) -> Option<Guard> {
            assert_eq!(self.live_guards.get(), 0, "must not nest read guards");
            match self.steps.front() {
                Some(Step::PreQueued(_)) => None,
                Some(Step::Poll { .. }) => {
                    self.live_guards.set(self.live_guards.get() + 1);
                    Some(Guard {
                        live: Rc::clone(&self.live_guards),
                        cancelled: Rc::clone(&self.cancelled),
                        consumed: false,
                    })
                }
                None => {
                    self.stopped = true;
                    None
                }
            }
        }

        fn dispatch_inner_queue(&mut self) -> Dispatch {
            self.inner_dispatches += 1;
            match self.steps.pop_front() {
                Some(Step::PreQueued(d)) => d,
                _ => panic!("dispatch_inner_queue without matching PreQueued step"),
            }
        }

        fn poll(&mut self, _guard: &Guard) -> Poll {
            assert_eq!(self.live_guards.get(), 1, "poll must hold prepared read guard");
            self.polls += 1;
            let Some(Step::Poll { result, .. }) = self.steps.front() else {
                panic!("poll without matching Poll step");
            };
            let result = *result;
            if !matches!(result, Poll::Readable) {
                self.steps.pop_front();
            }
            result
        }

        fn read(&mut self, mut guard: Guard) -> Dispatch {
            guard.consumed = true;
            drop(guard);
            self.reads += 1;
            match self.steps.pop_front() {
                Some(Step::Poll { read: Some(d), .. }) => d,
                _ => panic!("read without matching Poll::Readable step"),
            }
        }

        fn wake_and_wait(&mut self) {
            if self.live_guards.get() != 0 {
                self.guard_held_during_wake = true;
            }
            self.wakes += 1;
            if self.stop_on_wake == Some(self.wakes) {
                self.stopped = true;
            }
        }
    }

    // 1. Events already left in the layer's queue by winit or Mesa (prepare_read == None):
    //    dispatches inner queue and wakes when n > 0; ignores n == 0 (main thread drained first).
    let mut prequeued = FakeDriver::new([
        Step::PreQueued(Dispatch::Events(2)),
        Step::PreQueued(Dispatch::Events(0)),
        Step::Poll { result: Poll::Stop, read: None },
    ]);
    run(&mut prequeued);
    assert_eq!(prequeued.inner_dispatches, 2);
    assert_eq!(prequeued.wakes, 1, "must wake once for pre-queued events and skip Events(0)");
    assert_eq!(prequeued.polls, 1);
    assert_eq!(prequeued.cancelled.get(), 1, "Poll::Stop must cancel the prepared read guard");
    assert!(!prequeued.guard_held_during_wake);
    assert_eq!(prequeued.live_guards.get(), 0);

    // 2. Socket traffic: events for another queue (Events(0)), WouldBlock, and EINTR do not wake;
    //    events for this layer wake without holding a read guard while waiting for the loop turn.
    let mut socket = FakeDriver::new([
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(0)) },
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::WouldBlock) },
        Step::Poll { result: Poll::Interrupted, read: None },
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(3)) },
        Step::Poll { result: Poll::Stop, read: None },
    ]);
    run(&mut socket);
    assert_eq!(socket.polls, 5);
    assert_eq!(socket.reads, 3);
    assert_eq!(socket.wakes, 1);
    assert_eq!(socket.cancelled.get(), 2, "EINTR and Stop must cancel their prepared read guards");
    assert!(!socket.guard_held_during_wake, "must not hold a read guard while waiting for a turn");
    assert_eq!(socket.live_guards.get(), 0);

    // 3. Drop while waiting for a turn exits immediately without calling prepare_read again.
    let mut stop_in_wait = FakeDriver::new([
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(1)) },
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(1)) },
    ]);
    stop_in_wait.stop_on_wake = Some(1);
    run(&mut stop_in_wait);
    assert_eq!(stop_in_wait.polls, 1);
    assert_eq!(stop_in_wait.wakes, 1);
    assert_eq!(stop_in_wait.steps.len(), 1, "must stop right after wake_and_wait observes stop");
    assert_eq!(stop_in_wait.live_guards.get(), 0);

    // 4. Compositor hangup (read Error), inner-queue Error, and Poll::Error exit cleanly.
    let mut hangup = FakeDriver::new([
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Error) },
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(1)) },
    ]);
    run(&mut hangup);
    assert_eq!(hangup.reads, 1);
    assert_eq!(hangup.wakes, 0);
    assert_eq!(hangup.steps.len(), 1);
    assert_eq!(hangup.live_guards.get(), 0);

    let mut inner_err = FakeDriver::new([
        Step::PreQueued(Dispatch::Error),
        Step::PreQueued(Dispatch::Events(1)),
    ]);
    run(&mut inner_err);
    assert_eq!(inner_err.inner_dispatches, 1);
    assert_eq!(inner_err.wakes, 0);
    assert_eq!(inner_err.steps.len(), 1);

    let mut poll_err = FakeDriver::new([
        Step::Poll { result: Poll::Error, read: None },
        Step::Poll { result: Poll::Readable, read: Some(Dispatch::Events(1)) },
    ]);
    run(&mut poll_err);
    assert_eq!(poll_err.polls, 1);
    assert_eq!(poll_err.reads, 0);
    assert_eq!(poll_err.wakes, 0);
    assert_eq!(poll_err.cancelled.get(), 1);
    assert_eq!(poll_err.live_guards.get(), 0);
}
