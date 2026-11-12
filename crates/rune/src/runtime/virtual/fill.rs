//! An index-built list's fill: how many frames in a row the host kept drawing because a list
//! still wanted rows, and the wait that resumes a fill it couldn't finish in those frames. A
//! frame that finds no list asking settles it; a list whose heights keep changing for longer
//! than the bound waits, backs off and resumes, so it fills once they settle instead of going
//! blank. A wake whose attempt the fill no longer holds draws nothing and never starts it again.

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
    /// Resumes without progress ran out. Only a frame that moves a viewport or finds nothing to
    /// fill starts the fill again; other frames change nothing, so a redraw or an old deadline
    /// can't put a starved list back to drawing forever.
    Stalled,
}

/// The fill's state and the attempts it hands out, plus the one deadline it armed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Fill {
    state: State,
    seq: Seq,
    /// The deadline this fill armed: the attempt it resumes and when. A wake whose attempt the
    /// state no longer holds is stale, so the fill draws nothing for it.
    wake: Option<(Attempt, f64)>,
}

/// What a finished frame found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FillEvent {
    /// A frame ended at `now`. `wanted` is whether an index-built list still wants its window
    /// moved; `missing` is how many of its rows are still unmounted, read only when the run
    /// reaches its bound ([`Fill::at_bound`]) so a fill that finishes inside the bound counts
    /// nothing; `moved` is whether a scroll viewport moved this frame, which invalidates the
    /// window the fill was chosen for.
    Frame { now: f64, wanted: bool, missing: usize, moved: bool },
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

    /// Takes this fill's armed deadline when it has come due: `None` before it, or once it has
    /// been taken. The fill clears it when it leaves the wait, so this only answers for the run
    /// that armed it.
    pub(super) fn wake(&mut self, now: f64) -> Option<Attempt> {
        let (attempt, due) = self.wake?;
        if now < due { return None; }
        self.wake = None;
        Some(attempt)
    }

    /// Whether an armed deadline is due at `now`, so a resting window draws the frame that
    /// resumes it.
    pub(super) fn resuming(&self, now: f64) -> bool {
        self.wake.is_some_and(|(_, due)| now >= due)
    }

    /// Arms this fill's deadline at `due` for `attempt`.
    fn arm(&mut self, attempt: Attempt, due: f64) {
        self.wake = Some((attempt, due));
    }

    pub(crate) fn step(&mut self, event: FillEvent) -> Effects<FillEffect> {
        let effects = self.transition(event);
        // Only a wait owns the deadline. Once the fill leaves it (nothing wanted, a moved
        // viewport, the bound), that deadline is spent bookkeeping: it can neither draw for this
        // fill nor turn away the first frame a list that replaces it wants.
        if !matches!(self.state, State::Waiting { .. }) { self.wake = None; }
        effects
    }

    fn transition(&mut self, event: FillEvent) -> Effects<FillEffect> {
        match (self.state, event) {
            // No list wants a frame: the run is over, and any wait it left retires.
            (_, FillEvent::Frame { wanted: false, .. }) => {
                self.state = State::Idle;
                Effects::none()
            }
            // // differs from dev: a viewport moved, so the window the fill was chosen for is
            // gone; dev's spent count kept drawing nothing here. A fresh run draws from scratch.
            (_, FillEvent::Frame { wanted: true, moved: true, .. }) => {
                self.state = State::Filling { frames: 1, best: usize::MAX, resumes: 0 };
                Effects::one(FillEffect::Draw)
            }
            (State::Idle, FillEvent::Frame { wanted: true, .. }) => {
                self.state = State::Filling { frames: 1, best: usize::MAX, resumes: 0 };
                Effects::one(FillEffect::Draw)
            }
            (State::Filling { frames, best, resumes }, FillEvent::Frame { now, wanted: true, missing, .. }) => {
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
                if missing == 0 {
                    self.state = State::Idle;
                    return Effects::none();
                }
                if resumes >= RESUME_LIMIT {
                    self.state = State::Stalled;
                    return Effects::none();
                }
                let attempt = self.seq.next();
                let resumes = resumes + 1;
                let due = now + RESUME_WAIT * f64::from(1u32 << (resumes - 1));
                self.state = State::Waiting { attempt, due, best, resumes };
                self.arm(attempt, due);
                Effects::one(FillEffect::WakeAt(due))
            }
            // // differs from dev: a frame drawn for another cause while the fill waits changes
            // nothing at all. It can't spend the run's budget or reset its no-progress count, so
            // an old deadline or a plain redraw can't keep a starved fill drawing.
            (State::Waiting { .. } | State::Stalled, FillEvent::Frame { wanted: true, .. }) => Effects::none(),
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
