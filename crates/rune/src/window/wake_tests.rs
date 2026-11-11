//! What a drain asks of the window (`WakeWork`) and what a minimize asked for (`Minimize`),
//! each checked against a literal transcription of the fields and branches they replaced: every
//! combination of the facts for the first, every short sequence of events for the second.

use super::minimize::{step, MinimizeEffect, MinimizeEvent};
use super::{Minimize, WakeFacts, WakeWork};

/// Every sequence of `alphabet` up to `len` long, prefixes first.
fn walk<E: Copy>(alphabet: &[E], len: usize, mut run: impl FnMut(&[E])) {
    let mut seq = Vec::new();
    fn go<E: Copy>(alphabet: &[E], len: usize, seq: &mut Vec<E>, run: &mut impl FnMut(&[E])) {
        run(seq);
        if seq.len() == len { return; }
        for &e in alphabet {
            seq.push(e);
            go(alphabet, len, seq, run);
            seq.pop();
        }
    }
    go(alphabet, len, &mut seq, &mut run);
}

/// `drain_frame` and the `match` it filtered, as dev had them: a hidden launcher draws a change
/// into its cached frame, a shown window draws unless only a quiet turn woke it, and a window
/// that can't draw — covered, or minimised — runs its turn without one.
fn dev_work(launcher: bool, visible: bool, was_shown: bool, changed: bool, quiet: bool, covered: bool, minimized: bool, window: bool) -> WakeWork {
    let framed = if launcher && !visible { changed } else { !quiet && if launcher { changed || was_shown && visible } else { visible } };
    if !window || !framed { return WakeWork::Nothing; }
    if launcher && !visible { return WakeWork::HiddenFrame; }
    if (!launcher && covered) || minimized { WakeWork::CoveredFrame } else { WakeWork::Paint }
}

#[test]
fn a_drain_asks_the_window_for_the_same_work_as_dev_for_every_fact() {
    let mut cases = 0;
    for bits in 0u32..256 {
        let fact = |n: u32| bits & (1 << n) != 0;
        let (launcher, visible, was_shown, changed, quiet, covered, minimized, window) =
            (fact(0), fact(1), fact(2), fact(3), fact(4), fact(5), fact(6), fact(7));
        let work = WakeWork::of(WakeFacts { window, launcher, visible, was_shown, changed, quiet, covered }, || minimized);
        assert_eq!(work, dev_work(launcher, visible, was_shown, changed, quiet, covered, minimized, window), "{bits:#010b}");
        cases += 1;
    }
    assert_eq!(cases, 256, "every combination of the facts");
}

/// Every event a minimize meets as the window's call sites send it.
#[derive(Clone, Copy, Debug)]
enum Op {
    /// `Tx::minimize`, `AppHandle::minimize`, or the standard key.
    Asked,
    /// `poll_hidden` in a turn that shows the window: the poll, then its shell.
    HeldForShow,
    /// `window_request(Show | Open)`.
    Shown,
    /// A shell's minimize step.
    Taken,
    /// `close` or `stop_showing`.
    Lost,
}

/// dev's minimize latch, `Runtime::minimize_request`, as its call sites left it.
#[derive(Default)]
struct DevMinimize {
    /// `minimize_request`.
    latch: bool,
    /// The poll of a hidden window took it, and its own shell saw nothing.
    held: bool,
}

impl DevMinimize {
    /// The call site as dev wrote it, saying whether it called `minimize()`.
    fn apply(&mut self, op: Op) -> bool {
        match op {
            // `latch_requests`: `minimize_request |= std::mem::take(&mut tx.minimize)`.
            Op::Asked => {
                self.latch = true;
                self.held = false;
                false
            }
            // `poll_hidden`: `let minimize = shows && take(); self.shell(..); minimize_request |= minimize`.
            Op::HeldForShow => {
                let minimize = std::mem::take(&mut self.latch);
                self.latch |= minimize;
                self.held = minimize;
                false
            }
            // `window_request`: the show and its reveal never touched the latch.
            Op::Shown => {
                self.held = false;
                false
            }
            // `shell`: `let minimize = std::mem::take(&mut self.minimize_request);` then
            // `None if minimize => self.minimize()`.
            Op::Taken => {
                // differs from dev: a shell that reaches a minimize held for a show still to
                // come leaves it. dev took it and handed a hidden window a `minimize()` it
                // ignored, which dropped the request. The window always sends the show before
                // that shell, so the difference is a request kept, never a frame.
                if self.held { return false; }
                std::mem::take(&mut self.latch)
            }
            // `close` and `stop_showing`: `minimize_request = false`.
            Op::Lost => {
                self.latch = false;
                self.held = false;
                false
            }
        }
    }
}

fn apply_event(state: &mut Minimize, event: MinimizeEvent) -> bool {
    step(state, event).into_iter().any(|effect| effect == MinimizeEffect::Minimize)
}

fn apply_minimize(state: &mut Minimize, op: Op) -> bool {
    let event = match op {
        Op::Asked => MinimizeEvent::Asked,
        Op::HeldForShow => MinimizeEvent::HeldForShow,
        Op::Shown => MinimizeEvent::Shown,
        Op::Taken => MinimizeEvent::Taken,
        Op::Lost => MinimizeEvent::Lost,
    };
    apply_event(state, event)
}

#[test]
fn a_shell_minimizes_once_exactly_where_dev_did_for_every_sequence() {
    let alphabet = [Op::Asked, Op::HeldForShow, Op::Shown, Op::Taken, Op::Lost];
    let mut cases = 0;
    walk(&alphabet, 6, |seq| {
        cases += 1;
        let (mut dev, mut state) = (DevMinimize::default(), Minimize::None);
        for (i, &op) in seq.iter().enumerate() {
            assert_eq!(apply_minimize(&mut state, op), dev.apply(op), "{seq:?} at {i}");
            assert_eq!(state.latched(), dev.latch, "{seq:?} at {i}");
        }
    });
    assert_eq!(cases, (0..=6).map(|k| 5usize.pow(k)).sum::<usize>());
}

#[test]
fn a_minimize_asked_in_a_show_turn_waits_for_the_show() {
    // The turn's own order: poll a hidden window, show it, then shell.
    let (mut state, mut took) = (Minimize::None, Vec::new());
    for event in [MinimizeEvent::Asked, MinimizeEvent::HeldForShow, MinimizeEvent::Taken, MinimizeEvent::Shown, MinimizeEvent::Taken] {
        took.push(apply_event(&mut state, event));
    }
    assert_eq!(took, [false, false, false, false, true], "the poll's shell leaves it, the show's shell takes it");
    assert_eq!(state, Minimize::None);
}

#[test]
fn a_second_ask_after_a_held_minimize_is_due_before_the_show() {
    // `AppHandle::minimize` after the poll: the later ask no longer waits.
    let (mut state, mut took) = (Minimize::None, Vec::new());
    for event in [MinimizeEvent::Asked, MinimizeEvent::HeldForShow, MinimizeEvent::Asked, MinimizeEvent::Taken] {
        took.push(apply_event(&mut state, event));
    }
    assert_eq!(took, [false, false, false, true]);
    assert_eq!(state, Minimize::None);
}

#[test]
fn hiding_and_closing_drop_a_minimize_that_waits() {
    for state in [Minimize::Asked, Minimize::AfterShow] {
        let mut state = state;
        assert!(!apply_event(&mut state, MinimizeEvent::Lost));
        assert_eq!(state, Minimize::None);
    }
}
