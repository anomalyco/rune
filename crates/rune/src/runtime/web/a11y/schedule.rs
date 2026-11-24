//! Explicit scheduling state machine for the web accessibility mirror.
//!
//! In app mode (`document == false`), scrolling defers all mirror builds until scrolling has
//! rested for [`SCROLL_SETTLE`] (150 ms), bounded by [`SCROLL_MAX_DEFER`] (750 ms) so a pending
//! update can never starve, and flushes off `rAF` in a post-frame idle task
//! (`requestIdleCallback` bounded by [`IDLE_TIMEOUT`] = 100 ms, falling back to `setTimeout(0)`).
//! Assistive-tech and browser consumer triggers (`a11y_press`, `a11y_link`, `focusin` inside the
//! mirror, `beforeprint`, right-click `armMenu`, `Ctrl/Cmd+F`) flush immediately. Document mode
//! (`App::web_document()`) keeps synchronous first-frame and commit updates so
//! `syncPage`/`dropStatic` see the document mirror immediately.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::time::Duration;

use crate::machine::{Attempt, Effects, Seq};
use crate::time::Instant;

/// How long scrolling must stay idle before the mirror schedules its post-scroll flush.
pub(crate) const SCROLL_SETTLE: Duration = Duration::from_millis(150);

/// Bounded timeout for post-frame `requestIdleCallback` so ambient animation cannot starve updates.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_millis(100);

/// Hard upper bound on how long a pending mirror update may stay deferred in [`ScheduleState::ScrollDefer`]
/// before scheduling a forced off-`rAF` idle flush.
///
/// A normal swipe (~480 ms) plus its 150 ms settle window completes inside this bound with zero
/// mid-swipe mirror work, while continuous scrolling or a repeated `scrolling: true` signal can
/// delay the mirror by at most `SCROLL_MAX_DEFER + IDLE_TIMEOUT` (850 ms) and never starve it.
pub(crate) const SCROLL_MAX_DEFER: Duration = Duration::from_millis(750);

/// Mode of the web accessibility mirror scheduler.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ScheduleState {
    /// No deferred mirror flush is pending.
    #[default]
    Clean,
    /// A scroll viewport is moving or settling; all mirror builds are held until `due`
    /// (capped at `since + SCROLL_MAX_DEFER`) unless a consumer demands an immediate flush.
    ScrollDefer {
        attempt: Attempt,
        /// When this deferral cycle began, anchoring the [`SCROLL_MAX_DEFER`] bound.
        since: Instant,
        /// Next settle deadline (`<= since + SCROLL_MAX_DEFER`).
        due: Instant,
    },
    /// The mirror is dirty and waiting for a post-frame idle callback to flush off `rAF`.
    IdlePending {
        attempt: Attempt,
        /// True once [`SCROLL_MAX_DEFER`] forced this off-`rAF` flush so incoming scroll signals
        /// cannot defer it again.
        forced: bool,
    },
}

/// Inputs to [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScheduleEvent {
    /// End of a runtime step (`sync_input`), built by `RuntimeCore::web_a11y_frame_event`.
    Frame {
        now: Instant,
        /// `App::web_document()` mode: mirror is in document coordinates and flushes synchronously.
        document: bool,
        /// Mirror differs from what was last built (commits, viewport, zoom, scroll, motion, or first build).
        dirty: bool,
        /// Scroll motion is active now or scroll / `CommitWhy::VIRTUAL` movement occurred on this frame.
        scrolling: bool,
        /// A non-virtual commit occurred since the last mirror build.
        non_virtual_commit: bool,
    },
    /// The post-scroll settle timer fired for `attempt`.
    SettleDue {
        attempt: Attempt,
        now: Instant,
        /// Whether any scroll viewport has active scroll motion when the timer fired.
        scrolling: bool,
    },
    /// The post-frame idle callback fired for `attempt`.
    IdleDue {
        attempt: Attempt,
        now: Instant,
        /// Whether any scroll viewport has active scroll motion when the idle callback ran.
        scrolling: bool,
    },
    /// An assistive-tech or browser consumer needs the mirror immediately.
    ConsumerDemand,
}

/// Actions the host performs after [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScheduleEffect {
    /// Build, diff and send the mirror patch synchronously right now.
    FlushNow,
    /// Arm (or re-arm) the post-scroll settle timer for `attempt` after `delay`.
    ScheduleSettle { attempt: Attempt, delay: Duration },
    /// Schedule a post-frame idle task (`requestIdleCallback` with `timeout`, fallback `setTimeout(0)`).
    ScheduleIdle { attempt: Attempt, timeout: Duration },
    /// Cancel any pending settle timer or idle callback.
    CancelTimers,
}

/// Pure transition for [`ScheduleState`].
pub(crate) fn step(
    seq: &mut Seq,
    state: &mut ScheduleState,
    event: ScheduleEvent,
) -> Effects<ScheduleEffect> {
    match event {
        ScheduleEvent::ConsumerDemand => {
            if *state == ScheduleState::Clean {
                Effects::one(ScheduleEffect::FlushNow)
            } else {
                *state = ScheduleState::Clean;
                Effects::two(ScheduleEffect::CancelTimers, ScheduleEffect::FlushNow)
            }
        }
        ScheduleEvent::Frame { document: true, dirty, .. } => {
            let was_pending = *state != ScheduleState::Clean;
            *state = ScheduleState::Clean;
            match (was_pending, dirty) {
                (false, false) => Effects::none(),
                (true, false) => Effects::one(ScheduleEffect::CancelTimers),
                (false, true) => Effects::one(ScheduleEffect::FlushNow),
                (true, true) => Effects::two(ScheduleEffect::CancelTimers, ScheduleEffect::FlushNow),
            }
        }
        ScheduleEvent::Frame { document: false, now, dirty, scrolling, non_virtual_commit } => {
            if scrolling {
                if !dirty && *state == ScheduleState::Clean {
                    return Effects::none();
                }
                if matches!(*state, ScheduleState::IdlePending { forced: true, .. }) {
                    return Effects::none();
                }
                if let ScheduleState::ScrollDefer { attempt, since, due } = *state {
                    let cap = since + SCROLL_MAX_DEFER;
                    if now >= cap {
                        let next = seq.next();
                        *state = ScheduleState::IdlePending { attempt: next, forced: true };
                        return Effects::one(ScheduleEffect::ScheduleIdle { attempt: next, timeout: IDLE_TIMEOUT });
                    }
                    let next_due = (now + SCROLL_SETTLE).min(cap);
                    if now < due {
                        // Extend the settle deadline up to the cap without churning JS timers on every rAF frame.
                        *state = ScheduleState::ScrollDefer { attempt, since, due: next_due };
                        return Effects::none();
                    }
                    let next = seq.next();
                    let delay = next_due.saturating_duration_since(now);
                    *state = ScheduleState::ScrollDefer { attempt: next, since, due: next_due };
                    return Effects::one(ScheduleEffect::ScheduleSettle { attempt: next, delay });
                }
                let attempt = seq.next();
                *state = ScheduleState::ScrollDefer { attempt, since: now, due: now + SCROLL_SETTLE };
                return Effects::one(ScheduleEffect::ScheduleSettle { attempt, delay: SCROLL_SETTLE });
            }
            if !dirty {
                if *state == ScheduleState::Clean {
                    return Effects::none();
                }
                *state = ScheduleState::Clean;
                return Effects::one(ScheduleEffect::CancelTimers);
            }
            match *state {
                ScheduleState::Clean => {
                    let attempt = seq.next();
                    *state = ScheduleState::IdlePending { attempt, forced: false };
                    Effects::one(ScheduleEffect::ScheduleIdle { attempt, timeout: IDLE_TIMEOUT })
                }
                ScheduleState::IdlePending { .. } => Effects::none(),
                ScheduleState::ScrollDefer { since, due, .. } => {
                    let cap = since + SCROLL_MAX_DEFER;
                    if now >= cap {
                        let attempt = seq.next();
                        *state = ScheduleState::IdlePending { attempt, forced: true };
                        Effects::one(ScheduleEffect::ScheduleIdle { attempt, timeout: IDLE_TIMEOUT })
                    } else if non_virtual_commit || now >= due {
                        let attempt = seq.next();
                        *state = ScheduleState::IdlePending { attempt, forced: false };
                        Effects::one(ScheduleEffect::ScheduleIdle { attempt, timeout: IDLE_TIMEOUT })
                    } else {
                        Effects::none()
                    }
                }
            }
        }
        ScheduleEvent::SettleDue { attempt, now, scrolling } => match *state {
            ScheduleState::ScrollDefer { attempt: expected, since, due } if attempt == expected => {
                let cap = since + SCROLL_MAX_DEFER;
                if now >= cap {
                    let next = seq.next();
                    *state = ScheduleState::IdlePending { attempt: next, forced: true };
                    Effects::one(ScheduleEffect::ScheduleIdle { attempt: next, timeout: IDLE_TIMEOUT })
                } else if scrolling {
                    let next_due = (now + SCROLL_SETTLE).min(cap);
                    let delay = next_due.saturating_duration_since(now);
                    let next = seq.next();
                    *state = ScheduleState::ScrollDefer { attempt: next, since, due: next_due };
                    Effects::one(ScheduleEffect::ScheduleSettle { attempt: next, delay })
                } else if now < due {
                    let remaining = due.saturating_duration_since(now);
                    let next = seq.next();
                    *state = ScheduleState::ScrollDefer { attempt: next, since, due };
                    Effects::one(ScheduleEffect::ScheduleSettle { attempt: next, delay: remaining })
                } else {
                    let next = seq.next();
                    *state = ScheduleState::IdlePending { attempt: next, forced: false };
                    Effects::one(ScheduleEffect::ScheduleIdle { attempt: next, timeout: IDLE_TIMEOUT })
                }
            }
            _ => Effects::none(),
        },
        ScheduleEvent::IdleDue { attempt, now, scrolling } => match *state {
            ScheduleState::IdlePending { attempt: expected, forced } if attempt == expected => {
                if scrolling && !forced {
                    let next = seq.next();
                    *state = ScheduleState::ScrollDefer { attempt: next, since: now, due: now + SCROLL_SETTLE };
                    Effects::one(ScheduleEffect::ScheduleSettle { attempt: next, delay: SCROLL_SETTLE })
                } else {
                    *state = ScheduleState::Clean;
                    Effects::one(ScheduleEffect::FlushNow)
                }
            }
            _ => Effects::none(),
        },
    }
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
