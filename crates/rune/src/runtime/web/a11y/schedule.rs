//! Explicit scheduling state machine for the web accessibility mirror.
//!
//! In app mode (`document == false`), scrolling defers all mirror builds until scrolling has
//! rested for [`SCROLL_SETTLE`] (150 ms), and non-scroll updates flush off `rAF` in a post-frame
//! idle task (`requestIdleCallback` bounded by [`IDLE_TIMEOUT`] = 100 ms, falling back to
//! `setTimeout(0)`). Assistive-tech and browser consumer triggers (`a11y_press`, `a11y_link`,
//! `focusin` inside the mirror, `beforeprint`, right-click `armMenu`, `Ctrl/Cmd+F`) flush
//! immediately. Document mode (`App::web_document()`) keeps synchronous first-frame and commit
//! updates so `syncPage`/`dropStatic` see the document mirror immediately.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::time::Duration;

use crate::machine::{Attempt, Effects, Seq};
use crate::time::Instant;

/// How long scrolling must stay idle before the mirror schedules its post-scroll flush.
pub(crate) const SCROLL_SETTLE: Duration = Duration::from_millis(150);

/// Bounded timeout for post-frame `requestIdleCallback` so ambient animation cannot starve updates.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_millis(100);

/// Mode of the web accessibility mirror scheduler.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ScheduleState {
    /// No deferred mirror flush is pending.
    #[default]
    Clean,
    /// A scroll viewport is moving or settling; all mirror builds are held until `due`
    /// unless a consumer demands an immediate flush.
    ScrollDefer {
        attempt: Attempt,
        due: Instant,
    },
    /// The mirror is dirty and waiting for a post-frame idle callback to flush off `rAF`.
    IdlePending {
        attempt: Attempt,
    },
}

/// Inputs to [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScheduleEvent {
    /// End of a runtime step (`sync_input`).
    Frame {
        now: Instant,
        /// `App::web_document()` mode: mirror is in document coordinates and flushes synchronously.
        document: bool,
        /// Mirror differs from what was last built (commits, viewport, zoom, scroll, motion, or first build).
        dirty: bool,
        /// Any Rune-managed scroll viewport is actively moving (`rt.scrolling()`) or only a
        /// scroll / `CommitWhy::VIRTUAL` change occurred since the last mirror build.
        scrolling: bool,
        /// A non-virtual commit occurred since the last mirror build.
        non_virtual_commit: bool,
    },
    /// The post-scroll settle timer fired for `attempt`.
    SettleDue {
        attempt: Attempt,
        now: Instant,
        /// Whether any scroll viewport is still moving when the timer fired.
        scrolling: bool,
    },
    /// The post-frame idle callback fired for `attempt`.
    IdleDue {
        attempt: Attempt,
        now: Instant,
        /// Whether any scroll viewport started moving again before the idle callback ran.
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
                if let ScheduleState::ScrollDefer { attempt, due } = *state && now < due {
                    // Extend the settle deadline without churning JS timers on every rAF frame.
                    *state = ScheduleState::ScrollDefer { attempt, due: now + SCROLL_SETTLE };
                    return Effects::none();
                }
                let attempt = seq.next();
                *state = ScheduleState::ScrollDefer { attempt, due: now + SCROLL_SETTLE };
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
                    *state = ScheduleState::IdlePending { attempt };
                    Effects::one(ScheduleEffect::ScheduleIdle { attempt, timeout: IDLE_TIMEOUT })
                }
                ScheduleState::IdlePending { .. } => Effects::none(),
                ScheduleState::ScrollDefer { due, .. } => {
                    if non_virtual_commit || now >= due {
                        let attempt = seq.next();
                        *state = ScheduleState::IdlePending { attempt };
                        Effects::one(ScheduleEffect::ScheduleIdle { attempt, timeout: IDLE_TIMEOUT })
                    } else {
                        Effects::none()
                    }
                }
            }
        }
        ScheduleEvent::SettleDue { attempt, now, scrolling } => match *state {
            ScheduleState::ScrollDefer { attempt: expected, due } if attempt == expected => {
                if scrolling {
                    let next = seq.next();
                    *state = ScheduleState::ScrollDefer { attempt: next, due: now + SCROLL_SETTLE };
                    Effects::one(ScheduleEffect::ScheduleSettle { attempt: next, delay: SCROLL_SETTLE })
                } else if now < due {
                    let remaining = due.saturating_duration_since(now);
                    let next = seq.next();
                    *state = ScheduleState::ScrollDefer { attempt: next, due };
                    Effects::one(ScheduleEffect::ScheduleSettle { attempt: next, delay: remaining })
                } else {
                    let next = seq.next();
                    *state = ScheduleState::IdlePending { attempt: next };
                    Effects::one(ScheduleEffect::ScheduleIdle { attempt: next, timeout: IDLE_TIMEOUT })
                }
            }
            _ => Effects::none(),
        },
        ScheduleEvent::IdleDue { attempt, now, scrolling } => match *state {
            ScheduleState::IdlePending { attempt: expected } if attempt == expected => {
                if scrolling {
                    let next = seq.next();
                    *state = ScheduleState::ScrollDefer { attempt: next, due: now + SCROLL_SETTLE };
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
