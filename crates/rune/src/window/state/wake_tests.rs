//! What a drain asks of the window (`WakeWork`) and what a minimize asked for (`Minimize`),
//! each checked against a literal transcription of the fields and branches they replaced: every
//! combination of the facts for the first, every short sequence of events for the second.

use super::minimize::{HiddenPoll, MinimizeEffect, MinimizeEvent, shell_take, step};
use super::{Minimize, WakeFacts, WakeWork};
use crate::Cx;
use crate::el::Tx;
use crate::popup_menu::{PopupAnchor, PopupItem, PopupMenu, Presenter};
use crate::runtime::Runtime;

/// Every sequence of `alphabet` up to `len` long that `allowed` lets be extended, prefixes first.
fn walk<E: Copy>(alphabet: &[E], len: usize, mut allowed: impl FnMut(&[E], E) -> bool, mut run: impl FnMut(&[E])) {
    let mut seq = Vec::new();
    fn go<E: Copy>(alphabet: &[E], len: usize, seq: &mut Vec<E>, allowed: &mut impl FnMut(&[E], E) -> bool, run: &mut impl FnMut(&[E])) {
        run(seq);
        if seq.len() == len { return; }
        for &e in alphabet {
            if !allowed(seq, e) { continue; }
            seq.push(e);
            go(alphabet, len, seq, allowed, run);
            seq.pop();
        }
    }
    go(alphabet, len, &mut seq, &mut allowed, &mut run);
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
    /// `poll_hidden` in a turn that shows the window: `let minimize = shows && take(...)`.
    PollEnter,
    /// A shell's minimize step.
    Taken,
    /// The poll's shell is done: `minimize_request |= minimize`.
    PollReturn,
    /// `close` or `stop_showing`: `minimize_request = false`.
    Lost,
}

/// dev's minimize latch, `Runtime::minimize_request`, and the local `poll_hidden` kept across
/// its own shell, as dev's call sites left them.
#[derive(Default)]
struct DevMinimize {
    /// `minimize_request`.
    latch: bool,
    /// `let minimize = shows && std::mem::take(&mut self.rt.minimize_request);`, the local the
    /// whole shell ran under, callbacks included.
    saved: Option<bool>,
}

impl DevMinimize {
    /// The call site as dev wrote it, saying whether it called `minimize()`.
    fn apply(&mut self, op: Op) -> bool {
        match op {
            // `latch_requests`, `AppHandle::minimize`, the standard key: `latch = true`.
            Op::Asked => {
                self.latch = true;
                false
            }
            // `poll_hidden`, before its shell.
            Op::PollEnter => {
                self.saved = Some(std::mem::take(&mut self.latch));
                false
            }
            // `shell`: `let minimize = std::mem::take(&mut self.minimize_request);` then
            // `None if minimize => self.minimize()`.
            Op::Taken => std::mem::take(&mut self.latch),
            // `poll_hidden`, after its shell: `self.rt.minimize_request |= minimize;`.
            Op::PollReturn => {
                self.latch |= self.saved.take().unwrap_or(false);
                false
            }
            // `close` and `stop_showing`: `minimize_request = false`.
            Op::Lost => {
                self.latch = false;
                false
            }
        }
    }

    /// The ask waiting: in the field, or in the local this poll took out of it.
    fn latched(&self) -> bool { self.latch || self.saved.unwrap_or(false) }

    /// What a shell would take now.
    fn field(&self) -> bool { self.latch }
}

fn apply_event(state: &mut Minimize, event: MinimizeEvent) -> bool {
    step(state, event).into_iter().any(|effect| effect == MinimizeEffect::Minimize)
}

fn apply_minimize(state: &mut Minimize, op: Op) -> bool {
    let event = match op {
        Op::Asked => MinimizeEvent::Asked,
        Op::PollEnter => MinimizeEvent::HeldForShow,
        Op::Taken => MinimizeEvent::Taken,
        Op::PollReturn => MinimizeEvent::Returned,
        Op::Lost => MinimizeEvent::Lost,
    };
    apply_event(state, event)
}

#[test]
fn a_shell_minimizes_once_exactly_where_dev_did_for_every_sequence() {
    let alphabet = [Op::Asked, Op::PollEnter, Op::Taken, Op::PollReturn, Op::Lost];
    // A poll's shell can't re-enter `Host::poll_hidden`, so no poll starts inside another's
    // shell and none returns that never started. A sequence may end with a poll still open: the
    // shell after the show runs it later, which is the case r274 is about.
    let allowed = |seq: &[Op], op: Op| {
        let mut dev = DevMinimize::default();
        for &o in seq {
            dev.apply(o);
        }
        match op {
            Op::PollEnter => dev.saved.is_none(),
            Op::PollReturn => dev.saved.is_some(),
            _ => true,
        }
    };
    let mut cases = 0;
    walk(&alphabet, 6, allowed, |seq| {
        cases += 1;
        let (mut dev, mut state) = (DevMinimize::default(), Minimize::None);
        for (i, &op) in seq.iter().enumerate() {
            assert_eq!(apply_minimize(&mut state, op), dev.apply(op), "{seq:?} at {i}");
            assert_eq!(state.latched(), dev.latched(), "{seq:?} at {i}");
            assert_eq!(state.field(), dev.field(), "{seq:?} at {i}");
        }
    });
    // Both a closed and an open poll allow four of the five ops to come next.
    assert_eq!(cases, (0..=6).map(|k| 4usize.pow(k)).sum::<usize>());
}

#[test]
fn a_poll_that_holds_a_minimize_lets_the_shell_after_the_show_take_it() {
    // The turn's own order: the poll saves its ask, its shell leaves it, then the show's shell
    // takes it, and the window is minimized once — after it is shown.
    let mut rt = Runtime::new(0u32, |_: &u32, _: &mut Cx<u32>| crate::el(), (100.0, 100.0), crate::Env::default());
    rt.update(|_, tx| tx.minimize());
    assert!(!rt.minimize_step(MinimizeEvent::HeldForShow), "the poll's shell doesn't take it");
    assert!(!rt.minimize_step(MinimizeEvent::Taken));
    assert!(!rt.minimize_step(MinimizeEvent::Returned), "nothing to carry out at the return");
    assert!(rt.minimize_step(MinimizeEvent::Taken), "the shell after the show takes it");
    assert_eq!(rt.minimize, Minimize::None);
}

#[test]
fn a_hidden_polls_minimize_survives_its_popup_callback_asking_again() {
    // The turn a hidden window is woken by Show: its poll asks to minimize and opens a pop-up.
    // With no shown window the pop-up answers `None` at once and its callback asks again, so
    // the hidden shell's retry takes the callback's ask and the poll's own is still due after
    // the show. Drives the real poll, the real pop-up callback and the shell's take/return
    // wiring, so a shell that drops the return loses the final minimize here.
    let mut rt = Runtime::new(0u32, |_: &u32, _: &mut Cx<u32>| crate::el(), (100.0, 100.0), crate::Env::default());
    rt.poll = Some(|asked: &mut u32, _, tx: &mut Tx| {
        if *asked > 0 { return false; }
        *asked += 1;
        tx.minimize();
        tx.popup_menu(
            PopupMenu::new().item(PopupItem::new("ok", "OK")),
            PopupAnchor::Point(10.0, 20.0),
            |_: &mut u32, picked, tx: &mut Tx| {
                assert_eq!(picked, None, "a pop-up with no window answers None");
                tx.minimize();
            },
        );
        true
    });
    let poll = HiddenPoll::new(true);
    assert!(rt.poll_hidden(), "the hidden poll asked for a change");
    poll.enter(&mut rt);

    // The hidden shell: take, open the waiting pop-up (which asks again), retry.
    let mut popup = Presenter::default();
    let mut took = Vec::new();
    loop {
        took.push(shell_take(&mut rt));
        let opened = popup.open(&mut rt, None, false);
        if !opened && !rt.minimize.field() { break; }
    }
    // The show happens, and the shell after it takes what the poll held.
    poll.leave(&mut rt);
    took.push(shell_take(&mut rt));

    assert_eq!(took, [false, true, true], "the retry takes the callback's ask, the show's shell the poll's");
    assert_eq!(rt.minimize, Minimize::None);
}

#[test]
fn a_second_ask_during_a_hidden_poll_is_due_before_the_show() {
    let (mut state, mut took) = (Minimize::None, Vec::new());
    for event in [MinimizeEvent::Asked, MinimizeEvent::HeldForShow, MinimizeEvent::Asked, MinimizeEvent::Taken] {
        took.push(apply_event(&mut state, event));
    }
    assert_eq!(took, [false, false, false, true]);
    assert!(!state.field());
}

#[test]
fn hiding_and_closing_take_a_minimize_back_out_but_not_one_a_poll_holds() {
    let mut state = Minimize::Asked;
    assert!(!apply_event(&mut state, MinimizeEvent::Lost));
    assert_eq!(state, Minimize::None);
    // dev's local outlived a close inside the poll's shell, and was restored after it.
    let mut state = Minimize::None;
    assert!(!apply_event(&mut state, MinimizeEvent::Asked));
    assert!(!apply_event(&mut state, MinimizeEvent::HeldForShow));
    assert!(!apply_event(&mut state, MinimizeEvent::Lost));
    assert!(state.latched(), "the held ask is not the field's to take");
    assert!(!apply_event(&mut state, MinimizeEvent::Returned));
    assert_eq!(state, Minimize::Asked);
}
