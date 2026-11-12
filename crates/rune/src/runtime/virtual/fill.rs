//! An index-built list's fill: how many frames in a row the host kept drawing because a list
//! still wanted rows, and the wait that resumes a fill it couldn't finish in those frames. A
//! frame that finds no list asking settles it; a list whose heights keep changing for longer
//! than the bound waits, backs off and resumes, so it fills once they settle instead of going
//! blank.

use crate::machine::{Attempt, Effects, Seq};

/// The most frames in a row an index-built list keeps the host drawing to fill its overscan.
/// Filling takes four at most (half a viewport per side each); this bounds a list whose
/// heights keep changing.
pub(super) const FILL_FRAMES: u16 = 12;

/// How long the first resume waits, on the runtime clock: a few frames at 60 Hz. Each resume
/// without progress waits twice as long as the one before it.
const RESUME_WAIT: f64 = 3.0 / 60.0;

/// Resumes without the unfilled range shrinking before the fill gives up. A list whose heights
/// never settle goes blank rather than waking forever.
const RESUME_LIMIT: u8 = 3;

/// Where a fill's run stands.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum State {
    /// No list wants another frame.
    #[default]
    Idle,
    /// This many frames in a row found a list wanting one. `best` is the smallest unfilled
    /// range measured at the bound; `resumes` is how many resumes have run without it shrinking.
    Filling { frames: u16, best: usize, resumes: u8 },
    /// The bound stopped a fill that still had rows to mount: `attempt` resumes it at `due`.
    Waiting { attempt: Attempt, due: f64, best: usize, resumes: u8 },
}

/// The fill's state, and the attempts it hands out so an old wake never resumes a newer run.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Fill {
    state: State,
    seq: Seq,
}

/// What a finished frame found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FillEvent {
    /// A frame ended at `now`. `wanted` is whether an index-built list still wants its window
    /// moved; `missing` is how many of its rows are still unmounted, read only when the run
    /// reaches its bound ([`Fill::at_bound`]) so a fill that finishes inside the bound counts
    /// nothing.
    Frame { now: f64, wanted: bool, missing: usize },
    /// The wake `attempt` asked for came due.
    Due { attempt: Attempt, now: f64 },
}

/// What to do about it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FillEffect {
    /// Draw another frame now: a commit of that frame mounts more of the overscan.
    Draw,
    /// Draw nothing before `due` (runtime clock seconds), then resume the fill there.
    WakeAt(f64),
}

impl Fill {
    /// Whether the next frame reaches this run's bound, so the caller reads `missing` for it.
    pub(super) fn at_bound(&self) -> bool {
        matches!(self.state, State::Filling { frames, .. } if frames >= FILL_FRAMES)
    }

    /// The attempt to resume at `now`, when a wait has come due.
    pub(super) fn due(&self, now: f64) -> Option<Attempt> {
        match self.state {
            State::Waiting { attempt, due, .. } if now >= due => Some(attempt),
            _ => None,
        }
    }

    /// Whether a wait is due at `now`, so a resting window draws the frame that resumes it.
    pub(super) fn resuming(&self, now: f64) -> bool {
        self.due(now).is_some()
    }

    pub(crate) fn step(&mut self, event: FillEvent) -> Effects<FillEffect> {
        match (self.state, event) {
            // No list wants a frame: the run is over, and any wait it left retires.
            (_, FillEvent::Frame { wanted: false, .. }) => {
                self.state = State::Idle;
                Effects::none()
            }
            (State::Idle, FillEvent::Frame { wanted: true, .. }) => {
                self.state = State::Filling { frames: 1, best: usize::MAX, resumes: 0 };
                Effects::one(FillEffect::Draw)
            }
            (State::Filling { frames, best, resumes }, FillEvent::Frame { now, wanted: true, missing }) => {
                let frames = frames.saturating_add(1).min(FILL_FRAMES + 1);
                if frames <= FILL_FRAMES {
                    self.state = State::Filling { frames, best, resumes };
                    return Effects::one(FillEffect::Draw);
                }
                // // differs from dev: at the bound the unfilled range decides between waiting
                // for more frames, backing off, and giving up. Dev stopped at the bound for
                // good, so a list whose heights kept changing went blank until something else
                // asked for a frame.
                let (best, resumes) = if missing < best { (missing, 0) } else { (best, resumes) };
                if missing == 0 || resumes >= RESUME_LIMIT {
                    self.state = State::Idle;
                    return Effects::none();
                }
                let attempt = self.seq.next();
                let resumes = resumes + 1;
                let due = now + RESUME_WAIT * f64::from(1u32 << (resumes - 1));
                self.state = State::Waiting { attempt, due, best, resumes };
                Effects::one(FillEffect::WakeAt(due))
            }
            // // differs from dev: a frame drawn for another cause (a scroll, input, another
            // timer) while the fill waits retires its wake and starts a fresh run, where dev
            // kept its spent count. The frame is already drawn, so this costs no extra one.
            (State::Waiting { .. }, FillEvent::Frame { wanted: true, .. }) => {
                self.state = State::Filling { frames: 1, best: usize::MAX, resumes: 0 };
                Effects::one(FillEffect::Draw)
            }
            (State::Waiting { attempt, due, best, resumes }, FillEvent::Due { attempt: a, now })
                if attempt == a && now >= due =>
            {
                self.state = State::Filling { frames: 1, best, resumes };
                Effects::one(FillEffect::Draw)
            }
            // A due time whose attempt the state no longer holds is stale and changes nothing.
            _ => Effects::none(),
        }
    }

    /// Where the run stands, for tests.
    #[cfg(test)]
    pub(super) fn state(self) -> State {
        self.state
    }
}

#[cfg(test)]
#[path = "fill_tests.rs"]
mod tests;
