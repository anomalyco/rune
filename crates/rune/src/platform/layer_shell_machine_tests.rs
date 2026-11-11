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
