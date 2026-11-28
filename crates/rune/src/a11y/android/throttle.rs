//! Scroll-pacing state machine for Android accessibility tree updates.
//!
//! At rest every frame's tree changes sync to `accesskit_android` immediately. While a scroll
//! viewport is actively moving, syncs are paced to at most once per [`SCROLL_SYNC_INTERVAL`]
//! (100 ms), then flushed at once on the first resting frame (or when the scheduled wake fires
//! if frames stop).
#![cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]

use std::time::Duration;

use crate::machine::{Attempt, Effects, Seq};
use crate::time::Instant;

/// Minimum spacing between Android accessibility tree syncs while a viewport scrolls.
pub(crate) const SCROLL_SYNC_INTERVAL: Duration = Duration::from_millis(100);

/// Mode of the Android accessibility scroll pacer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum State {
    /// At rest: every frame's changes sync immediately.
    #[default]
    Rest,
    /// Synced while scrolling at `last_sync`; no held update is waiting yet.
    Cooldown { last_sync: Instant },
    /// A scroll frame arrived inside the cooldown window; one sync is held until `due`
    /// or until scrolling rests.
    Holding { attempt: Attempt, due: Instant },
}

/// Inputs to [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// End of a UI frame. `scrolling` is true while any viewport has active scroll motion.
    Frame { now: Instant, scrolling: bool },
    /// The host's delayed wake fired for `attempt`.
    Wake {
        attempt: Attempt,
        now: Instant,
        scrolling: bool,
    },
    /// Host paused, surface detached, or shut down: drop any held sync timer.
    Reset,
}

/// Actions the Android host carries out after [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Pull pending tree changes from the runtime and push them to `accesskit_android`.
    SyncNow,
    /// Schedule a delayed wake for `attempt` after `delay` so held scroll changes flush even if
    /// Choreographer frames stop.
    WakeAfter { attempt: Attempt, delay: Duration },
}

/// Pure transition for [`State`].
pub(crate) fn step(seq: &mut Seq, state: &mut State, event: Event) -> Effects<Effect> {
    match event {
        Event::Reset => {
            *state = State::Rest;
            Effects::none()
        }
        Event::Frame { scrolling: false, .. } => {
            *state = State::Rest;
            Effects::one(Effect::SyncNow)
        }
        Event::Frame { now, scrolling: true } => match *state {
            State::Rest => {
                *state = State::Cooldown { last_sync: now };
                Effects::one(Effect::SyncNow)
            }
            State::Cooldown { last_sync } => {
                let due = last_sync + SCROLL_SYNC_INTERVAL;
                if now >= due {
                    *state = State::Cooldown { last_sync: now };
                    Effects::one(Effect::SyncNow)
                } else {
                    let attempt = seq.next();
                    let delay = due.saturating_duration_since(now);
                    *state = State::Holding { attempt, due };
                    Effects::one(Effect::WakeAfter { attempt, delay })
                }
            }
            State::Holding { due, .. } => {
                if now >= due {
                    *state = State::Cooldown { last_sync: now };
                    Effects::one(Effect::SyncNow)
                } else {
                    Effects::none()
                }
            }
        },
        Event::Wake { attempt, now, scrolling } => match *state {
            State::Holding { attempt: expected, due } if attempt == expected => {
                if scrolling && now < due {
                    let next = seq.next();
                    let delay = due.saturating_duration_since(now);
                    *state = State::Holding { attempt: next, due };
                    Effects::one(Effect::WakeAfter { attempt: next, delay })
                } else {
                    *state = if scrolling {
                        State::Cooldown { last_sync: now }
                    } else {
                        State::Rest
                    };
                    Effects::one(Effect::SyncNow)
                }
            }
            _ => Effects::none(),
        },
    }
}
