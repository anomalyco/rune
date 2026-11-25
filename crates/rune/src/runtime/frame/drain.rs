//! Bounded same-turn follow-up drain: a turn does not end with work that it itself queued still
//! waiting; it repeats follow-up rounds while due work remains, at most [`MAX_POLL_ROUNDS`]
//! times, and then stops, logging a debug-only warning once and scheduling a continuation wake
//! so a self-requeueing handler or poll cannot spin the host.
//!
//! Shared by hidden and covered wakes (draining input that `poll` or a handler queued without
//! drawing) and by the visible frame loop for same-turn `poll <-> handlers` follow-up rounds.

use crate::machine::Effects;

/// Maximum follow-up rounds a single turn may run to drain work that it itself queued
/// (matching [`super::mount::MAX_MOUNT_ROUNDS`] = 4), so a self-requeueing callback cannot
/// spin a window.
pub(in crate::runtime) const MAX_POLL_ROUNDS: usize = super::mount::MAX_MOUNT_ROUNDS;

/// Explicit state of a bounded same-turn follow-up drain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::runtime) enum DrainState {
    /// Initial pass has not yet triggered a follow-up round.
    #[default]
    Active,
    /// Follow-up round `rounds` (`1..=MAX_POLL_ROUNDS`) is running.
    Draining { rounds: usize },
    /// No same-turn work remained, or [`MAX_POLL_ROUNDS`] was reached.
    Done { capped: bool },
}

/// Observation after a turn's initial step or after a follow-up round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum DrainEvent {
    /// `more` is whether the turn left same-turn work waiting.
    StepDone { more: bool },
}

/// Effect emitted by [`step_drain`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum DrainEffect {
    /// Run one bounded follow-up round (`round` is 1-based, up to [`MAX_POLL_ROUNDS`]).
    FollowUp { round: usize },
    /// [`MAX_POLL_ROUNDS`] was exhausted with work still waiting: stop and defer continuation.
    Capped,
}

/// Outcome of [`drain_turn`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime) struct DrainOutcome {
    /// Number of follow-up rounds executed (`0..=MAX_POLL_ROUNDS`).
    pub rounds: usize,
    /// `true` when [`MAX_POLL_ROUNDS`] was reached with work still waiting.
    pub capped: bool,
}

/// Advances [`DrainState`] after a round finishes: emits [`DrainEffect::FollowUp`] while `more`
/// holds and fewer than [`MAX_POLL_ROUNDS`] follow-up rounds have run; emits
/// [`DrainEffect::Capped`] if `more` still holds at the cap, then transitions to
/// [`DrainState::Done`].
pub(in crate::runtime) fn step_drain(
    state: &mut DrainState,
    event: DrainEvent,
) -> Effects<DrainEffect> {
    let DrainEvent::StepDone { more } = event;
    let ran = match *state {
        DrainState::Active => 0,
        DrainState::Draining { rounds } => rounds,
        DrainState::Done { .. } => return Effects::none(),
    };
    if !more {
        *state = DrainState::Done { capped: false };
        Effects::none()
    } else if ran < MAX_POLL_ROUNDS {
        let round = ran + 1;
        *state = DrainState::Draining { rounds: round };
        Effects::one(DrainEffect::FollowUp { round })
    } else {
        *state = DrainState::Done { capped: true };
        Effects::one(DrainEffect::Capped)
    }
}

/// Runs follow-up rounds while `has_more` reports same-turn work queued by the current turn,
/// bounded to [`MAX_POLL_ROUNDS`]. When the cap is reached with work still waiting, logs the
/// debug-only cap warning once and returns `DrainOutcome { capped: true, .. }` so the caller
/// schedules a continuation without spinning.
pub(in crate::runtime) fn drain_turn<C>(
    ctx: &mut C,
    mut has_more: impl FnMut(&C) -> bool,
    mut run_round: impl FnMut(&mut C, usize),
) -> DrainOutcome {
    let mut state = DrainState::Active;
    let mut rounds = 0;
    loop {
        let effects = step_drain(&mut state, DrainEvent::StepDone { more: has_more(ctx) });
        match effects.into_iter().next() {
            Some(DrainEffect::FollowUp { round }) => {
                rounds = round;
                run_round(ctx, round);
            }
            Some(DrainEffect::Capped) => {
                crate::tooling::perf_warn::poll_round_cap(MAX_POLL_ROUNDS);
                return DrainOutcome { rounds, capped: true };
            }
            None => return DrainOutcome { rounds, capped: false },
        }
    }
}
