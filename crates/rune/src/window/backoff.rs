//! The window's wait after a frame it couldn't draw: when it may try again, and how long a miss
//! of each kind waits. An occluded surface re-checks on a wait that doubles while it stays
//! covered, so a window nobody sees costs a few failed frames a second instead of a spinning
//! core; a hidden launcher's refresh backs off likewise. A timer pacing motion no display link
//! drives, and a surface being replaced after a failed attempt, wait too, but with no backoff of
//! their own. A frame that presents starts both over.
//!
//! One `step` takes every event that moves them, so the doubling and the resets sit in one place
//! instead of in the draw, the event handlers and the surface replacement.

use super::*;
use crate::machine::Effects;

/// When the window may draw next, and the waits a miss of each kind gets. One miss is one
/// `step`; the waits behind `at` are its only state that the deadline does not show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Backoff {
    /// When the window may draw again; `None` when nothing waits.
    at: Option<Instant>,
    /// The wait an occluded surface's miss gets, held to `OCCLUDED_RETRY..=OCCLUDED_MAX`.
    occluded: Duration,
    /// The wait a hidden launcher's refresh takes after no drawable, doubling likewise.
    hidden: Duration,
}

impl Default for Backoff {
    fn default() -> Self { Backoff { at: None, occluded: OCCLUDED_RETRY, hidden: TIMEOUT_RETRY } }
}

/// What happened to the window's next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackoffEvent {
    /// A shown frame got no drawable at `now`. `paced` is a running display link, whose next tick
    /// draws the frame itself.
    Missed { missed: Missed, paced: bool, now: Instant },
    /// A hidden launcher's refresh got no drawable at `now`. `preparing` is a first frame whose
    /// own deadline retries it, which the window waits on once rather than twice.
    HiddenMissed { preparing: bool, now: Instant },
    /// The window is tried again at `at` for a reason with no backoff of its own: a surface being
    /// replaced, or a timer pacing motion no display link drives (macOS).
    At(Instant),
    /// The wait ran out, or the window takes the frame anyway: nothing waits now. Both waits keep
    /// their backoff: nothing has shown that the surface can draw.
    Lapsed,
    /// The window's occlusion changed: nothing waits, and the occluded wait starts over.
    Uncovered,
    /// A frame presented: nothing waits, and both waits start over.
    Presented,
}

/// What a transition asks the host to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackoffEffect {
    /// No display tick may draw sooner than the wait (`Win::stop_pacing`).
    StopPacing,
}

impl Backoff {
    pub(crate) fn step(&mut self, event: BackoffEvent) -> Effects<BackoffEffect> {
        match event {
            BackoffEvent::Missed { missed, paced, now } => {
                self.at = missed.retry(paced, now, &mut self.occluded);
                match self.at {
                    Some(_) => Effects::one(BackoffEffect::StopPacing),
                    None => Effects::none(),
                }
            }
            BackoffEvent::HiddenMissed { preparing, now } => {
                // A first frame's deadline owns its retries; the window waits for none of its own.
                self.at = (!preparing).then(|| now + self.hidden);
                if !preparing { self.hidden = (self.hidden * 2).min(OCCLUDED_MAX); }
                Effects::none()
            }
            BackoffEvent::At(at) => {
                self.at = Some(at);
                Effects::none()
            }
            BackoffEvent::Lapsed => {
                self.at = None;
                Effects::none()
            }
            BackoffEvent::Uncovered => {
                self.at = None;
                self.occluded = OCCLUDED_RETRY;
                Effects::none()
            }
            BackoffEvent::Presented => {
                self.at = None;
                self.occluded = OCCLUDED_RETRY;
                self.hidden = TIMEOUT_RETRY;
                Effects::none()
            }
        }
    }

    /// When the window may draw again, until it does.
    pub(crate) fn deadline(&self) -> Option<Instant> { self.at }

    /// Whether the window is still waiting out a frame it couldn't draw at `now`.
    pub(crate) fn waiting(&self, now: Instant) -> bool { self.at.is_some_and(|at| at > now) }

    /// A window whose occluded and hidden waits start at `occluded` and `hidden`, as a long
    /// occluded spell or a test leaves them.
    #[cfg(test)]
    pub(crate) fn seeded(occluded: Duration, hidden: Duration) -> Self { Backoff { at: None, occluded, hidden } }
}
