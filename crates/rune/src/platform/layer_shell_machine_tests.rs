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
