//! An app's instant scroll (`Tx::scroll_to_instant` and the like) holds hover under a still
//! pointer: content moved with nothing under the pointer moving, so hover waits out the hold
//! instead of lighting each row an app's own, frame-by-frame glide carries past. The hold is a
//! time on the clock, so it lasts as long on a 30 Hz display as on a 120 Hz one: the frame the
//! scroll lands in holds, and hover resolves once at the pointer when the hold is over
//! (`runtime/input/hover.rs`). A held nudge draws nothing in between: it asks for one frame at
//! its due time, the frame that ends it.
use crate::machine::{Attempt, Effects, Seq};

/// How long an instant scroll holds hover: the two frames at 60 Hz its frame countdown took.
pub(super) const NUDGE_HOLD: f64 = 2.0 / 60.0;

/// A deadline comparison's slack, so a frame that lands on the due time ends the hold.
const SLACK: f64 = 1e-6;

/// A viewport's hold: armed as one [`Attempt`] due at a clock time, so a wake an earlier arm
/// asked for changes nothing.
#[derive(Clone, Debug, Default)]
pub(super) struct Nudge {
    seq: Seq,
    state: Hold,
}

/// Where the hold stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum Hold {
    /// Nothing holds hover.
    #[default]
    Free,
    /// An instant scroll landed, and hover holds until this clock time, as `attempt`.
    Held { attempt: Attempt, due: f64 },
}

pub(super) enum Event {
    /// An app's instant scroll landed at this clock time.
    Arm { now: f64 },
    /// A frame at `now`, carrying the `attempt` its arm asked for: the hold ends only if that
    /// is still the armed one.
    Due { attempt: Attempt, now: f64 },
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    /// Wake at this clock time, when the hold is due.
    WakeAt(f64),
}

pub(super) fn step(seq: &mut Seq, state: &mut Hold, event: Event) -> Effects<Effect> {
    match event {
        Event::Arm { now } => {
            let (attempt, due) = (seq.next(), now + NUDGE_HOLD);
            *state = Hold::Held { attempt, due };
            Effects::one(Effect::WakeAt(due))
        }
        Event::Due { attempt, now } => match state {
            Hold::Held { attempt: armed, due } if *armed == attempt && *due <= now + SLACK => {
                *state = Hold::Free;
                Effects::none()
            }
            _ => Effects::none(),
        },
    }
}

impl Nudge {
    /// An instant scroll landed at `now` from the clock: hover holds, and this asks for the
    /// wake that brings the frame the hold ends on.
    pub(super) fn arm(&mut self, now: f64) -> Effects<Effect> {
        step(&mut self.seq, &mut self.state, Event::Arm { now })
    }

    /// A frame at `now`, the one its wake asked for: a hold that came due ends.
    pub(super) fn tick(&mut self, now: f64) {
        if let Hold::Held { attempt, .. } = self.state {
            let _ = step(&mut self.seq, &mut self.state, Event::Due { attempt, now });
        }
    }

    /// Hover holds at `now`.
    pub(super) fn hold_at(&self, now: f64) -> bool {
        matches!(self.state, Hold::Held { due, .. } if now + SLACK < due)
    }

    /// The clock time a pending hold ends.
    pub(super) fn due(&self) -> Option<f64> {
        match self.state {
            Hold::Held { due, .. } => Some(due),
            Hold::Free => None,
        }
    }
}

#[cfg(test)]
#[path = "nudge_tests.rs"]
mod tests;
