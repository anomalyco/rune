//! A held press whose drag handler asked to hear it again ([`Tx::drag_repeat_after`]): a frame
//! at or after the delay sends the handler the pointer as a `Phase::Move` its `Tx` marks
//! as the repeat.
use crate::machine::{Attempt, Effects, Seq};

use super::*;

/// The held press's repeat: armed for an element at a clock time, as one attempt, so a wake an
/// earlier arm asked for changes nothing.
#[derive(Debug, Default)]
pub(super) struct PressRepeat {
    seq: Seq,
    state: Repeat,
}

/// Where the repeat stands.
#[derive(Clone, Debug, Default, PartialEq)]
enum Repeat {
    /// No repeat waits.
    #[default]
    Idle,
    /// `path`'s drag hears the pointer again at `due` (clock seconds), as attempt `attempt`.
    Armed { attempt: Attempt, path: String, due: f64 },
}

impl Repeat {
    fn attempt(&self) -> Option<Attempt> {
        match self {
            Repeat::Armed { attempt, .. } => Some(*attempt),
            Repeat::Idle => None,
        }
    }
}

pub(super) enum Event<'a> {
    /// `path`'s drag handler asked for a repeat due at this clock time.
    Arm { path: &'a str, due: f64 },
    /// The release or a cancel ends the repeat.
    End,
    /// A frame at `now`, carrying the `attempt` its wake was asked for: the repeat fires only
    /// if that is still the armed one.
    Due { attempt: Attempt, now: f64 },
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    /// `path`'s drag hears the pointer again.
    Fire(String),
    /// Wake at this clock time, when the repeat is due.
    WakeAt(f64),
}

pub(super) fn step(seq: &mut Seq, state: &mut Repeat, event: Event) -> Effects<Effect> {
    match event {
        Event::Arm { path, due } => {
            let attempt = seq.next();
            *state = Repeat::Armed { attempt, path: path.to_owned(), due };
            Effects::one(Effect::WakeAt(due))
        }
        Event::End => {
            *state = Repeat::Idle;
            Effects::none()
        }
        Event::Due { attempt, now } => match state {
            Repeat::Armed { attempt: armed, path, due } if *armed == attempt && *due <= now + 1e-6 => {
                let path = std::mem::take(path);
                *state = Repeat::Idle;
                Effects::one(Effect::Fire(path))
            }
            _ => Effects::none(),
        },
    }
}

impl PressRepeat {
    fn step(&mut self, event: Event) -> Effects<Effect> {
        step(&mut self.seq, &mut self.state, event)
    }

    /// The attempt a frame's wake for the repeat would carry, while one is armed.
    fn attempt(&self) -> Option<Attempt> { self.state.attempt() }

    /// A repeat waits past `now`.
    fn waiting(&self, now: f64) -> bool {
        matches!(self.state, Repeat::Armed { due, .. } if now + 1e-6 < due)
    }
}

impl<S: 'static> Runtime<S> {
    /// After `path`'s drag handler asked for a repeat `after` from now: due then, while the
    /// press it heard is still held.
    pub(super) fn arm_press_repeat(&mut self, path: &str, after: std::time::Duration) {
        if self.press.path() != Some(path) { return; }
        let due = self.clock + after.as_secs_f64();
        for effect in self.press_repeat.step(Event::Arm { path, due }) {
            if let Effect::WakeAt(due) = effect { self.schedule_wakes_from(due, [std::time::Duration::ZERO]); }
        }
    }

    /// The release or a cancel ends the repeat.
    pub(super) fn end_press_repeat(&mut self) { let _ = self.press_repeat.step(Event::End); }

    pub(super) fn press_repeat_waiting(&self) -> bool { self.press_repeat.waiting(self.clock) }

    /// Sends the repeat due by this frame to the drag still holding its press.
    pub(super) fn fire_press_repeat(&mut self) {
        let Some(attempt) = self.press_repeat.attempt() else { return };
        for effect in self.press_repeat.step(Event::Due { attempt, now: self.clock }) {
            if let Effect::Fire(path) = effect { self.send_press_repeat(&path); }
        }
    }

    /// `path`'s drag, if it still holds the press, hears the pointer again.
    fn send_press_repeat(&mut self, path: &str) {
        let Some(held) = self.press.held().filter(|h| h.path == path) else { return };
        let (d, at, button) = (held.drag, held.at, held.button);
        let Some(i) = self.live_index(path) else { return };
        let (x, y) = self.pointer.unwrap_or(at);
        let [lx, ly] = self.to_local(i, x, y);
        let (dx, dy) = (x - at.0, y - at.1);
        self.queue.push(Action::DragRepeat(path.to_owned(), Drag { phase: Phase::Move, x: lx, y: ly, dx, dy, window: [x, y], screen: self.screen_at(x, y), ..d }, button));
    }
}

#[cfg(test)]
#[path = "press_repeat_tests.rs"]
mod tests;
