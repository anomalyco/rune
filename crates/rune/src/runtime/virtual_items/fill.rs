//! Explicit state machine driving multi-frame budgeted mounting for [`El::virtual_items`].
//!
//! Honest budgeted fills (static version, `missing` strictly shrinking every frame) drain to zero
//! without pausing. Runaway version churn (where `version()` keeps changing or `missing` stops
//! shrinking for more than [`STAGNANT_LIMIT`] consecutive frames) backs off with exponentially
//! spaced wake deadlines and stalls after [`RESUME_LIMIT`] fruitless resumes so a misbehaving
//! source cannot pin the host at 100% CPU forever.

use crate::machine::{Attempt, Effects, Seq};

/// Consecutive stagnant or version-churning frames before backing off.
pub(super) const STAGNANT_LIMIT: u16 = 12;

/// First backoff wait on the runtime clock (3 frames at 60 Hz).
const RESUME_WAIT: f64 = 3.0 / 60.0;

/// Fruitless resumes before stalling until the source settles or a full view rebuild runs.
const RESUME_LIMIT: u8 = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum State {
    /// No `virtual_items` container wants another fill frame.
    #[default]
    Idle,
    /// Actively mounting budgeted items; `stagnant` counts frames where `version` churned or
    /// `missing` failed to shrink, `prev_missing` tracks the prior frame's pending count.
    Filling { stagnant: u16, prev_missing: usize, best: usize, resumes: u8 },
    /// Backed off after runaway churn or lack of progress; `attempt` resumes at `due`.
    Waiting { attempt: Attempt, due: f64, best: usize, resumes: u8 },
    /// Exhausted fruitless resumes; stays idle until `wanted == false` or an explicit reset.
    Stalled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Fill {
    state: State,
    seq: Seq,
    wake: Option<(Attempt, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FillEvent {
    /// A frame ended at `now`.
    /// - `wanted`: any live `virtual_items` container still wants another remount frame.
    /// - `missing`: total pending items across live containers.
    /// - `version_changed`: any live container's `version()` differs from its last committed one.
    Frame { now: f64, wanted: bool, missing: usize, version_changed: bool },
    /// The armed wake deadline came due.
    Due { attempt: Attempt, now: f64 },
    /// A full view rebuild occurred; clears a prior stall so fresh app state can fill again.
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FillEffect {
    /// Request another animation frame immediately.
    Draw,
    /// Rest until `due` on the runtime clock, then resume.
    WakeAt(f64),
}

impl Fill {
    /// Takes the armed deadline if `now >= due`.
    pub(crate) fn wake(&mut self, now: f64) -> Option<Attempt> {
        let (attempt, due) = self.wake?;
        if now < due {
            return None;
        }
        self.wake = None;
        Some(attempt)
    }

    /// Whether an armed resume deadline is due at `now`.
    pub(crate) fn resuming(&self, now: f64) -> bool {
        self.wake.is_some_and(|(_, due)| now >= due)
    }

    pub(crate) fn step(&mut self, event: FillEvent) -> Effects<FillEffect> {
        let effects = self.transition(event);
        if !matches!(self.state, State::Waiting { .. }) {
            self.wake = None;
        }
        effects
    }

    fn transition(&mut self, event: FillEvent) -> Effects<FillEffect> {
        match (self.state, event) {
            (_, FillEvent::Reset) | (_, FillEvent::Frame { wanted: false, .. }) => {
                self.state = State::Idle;
                Effects::none()
            }
            (State::Idle, FillEvent::Frame { wanted: true, missing, version_changed, .. }) => {
                let stagnant = u16::from(version_changed);
                self.state = State::Filling { stagnant, prev_missing: missing, best: missing, resumes: 0 };
                Effects::one(FillEffect::Draw)
            }
            (
                State::Filling { stagnant, prev_missing, best, resumes },
                FillEvent::Frame { now, wanted: true, missing, version_changed },
            ) => {
                let draining = !version_changed && missing < prev_missing;
                let stagnant = if draining { 0 } else { stagnant.saturating_add(1) };
                if stagnant <= STAGNANT_LIMIT {
                    let best = best.min(missing);
                    self.state = State::Filling { stagnant, prev_missing: missing, best, resumes };
                    return Effects::one(FillEffect::Draw);
                }
                let (best, resumes) = if missing < best { (missing, 0) } else { (best, resumes) };
                if resumes >= RESUME_LIMIT {
                    self.state = State::Stalled;
                    return Effects::none();
                }
                let attempt = self.seq.next();
                let resumes = resumes + 1;
                let due = now + RESUME_WAIT * f64::from(1u32 << (resumes - 1));
                self.state = State::Waiting { attempt, due, best, resumes };
                self.wake = Some((attempt, due));
                Effects::one(FillEffect::WakeAt(due))
            }
            (State::Waiting { .. } | State::Stalled, FillEvent::Frame { wanted: true, .. }) => {
                Effects::none()
            }
            (State::Waiting { attempt, due, best, resumes }, FillEvent::Due { attempt: a, now })
                if attempt == a && now >= due =>
            {
                self.state = State::Filling { stagnant: 0, prev_missing: usize::MAX, best, resumes };
                Effects::one(FillEffect::Draw)
            }
            _ => Effects::none(),
        }
    }

    #[cfg(test)]
    pub(super) fn state(self) -> State {
        self.state
    }
}

#[cfg(test)]
#[path = "fill_tests.rs"]
mod fill_tests;
