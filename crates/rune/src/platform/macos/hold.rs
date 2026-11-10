//! The press an overlay's panel holds: from the local monitor seeing a Down until the runtime
//! has its Up. While held, the panel keeps taking mouse events, even off its hit area, so the
//! release reaches it.
//!
//! The monitor sees a click before winit dispatches it, so the seen clicks wait in a queue, and
//! another Down can already be queued when the runtime hears the Up before it. A release can also
//! be lost: the button reads up and winit never dispatches an Up. `Releasing` waits out a grace
//! for one, then gives up and asks the runtime to end the press ([`Effect::ReleaseLost`]).

use std::{collections::VecDeque, time::{Duration, Instant}};
use crate::machine::Effects;

pub(super) type Point = (f32, f32);

/// How long an Up the monitor saw may wait: winit dispatches one within the turn, so one still
/// queued this much later was never delivered (the panel lost its view).
const QUEUED_UP_GRACE: Duration = Duration::from_secs(2);
/// How long the button may read up with no Up seen at all.
const UNSEEN_UP_GRACE: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Click {
    pub(super) down: bool,
    pub(super) at: Point,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Hold {
    /// No press is held. `ups` are releases still to dispatch, of presses the monitor never saw.
    Free { ups: VecDeque<Point> },
    /// A press is held, the button down or its Up still on the way.
    Pressed { clicks: VecDeque<Click> },
    /// The button read up at `at` and `since`, and the runtime hasn't had the Up.
    Releasing { clicks: VecDeque<Click>, since: Instant, at: Point },
}

impl Default for Hold {
    fn default() -> Self { Self::Free { ups: VecDeque::new() } }
}

impl Hold {
    pub(super) fn captured(&self) -> bool { !matches!(self, Self::Free { .. }) }

    /// When a lost release gives up, for the event loop to sample the pointer then even if
    /// nothing else moves.
    pub(super) fn deadline(&self) -> Option<Instant> {
        let Self::Releasing { clicks, since, .. } = self else { return None };
        let queued_up = clicks.iter().any(|click| !click.down);
        Some(*since + if queued_up { QUEUED_UP_GRACE } else { UNSEEN_UP_GRACE })
    }

    pub(super) fn into_clicks(self) -> VecDeque<Click> {
        match self {
            Self::Free { ups } => ups.into_iter().map(|at| Click { down: false, at }).collect(),
            Self::Pressed { clicks } | Self::Releasing { clicks, .. } => clicks,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Event {
    /// The local monitor saw a click; `over` is whether it fell on the hit area.
    Seen { click: Click, over: bool },
    /// winit dispatched a press (`down`) or a release.
    Dispatched { down: bool },
    /// The runtime has had the release, or a system drag took it.
    Released,
    /// A sample of the pointer and the primary button.
    Sampled { button_down: bool, now: Instant, at: Point },
    /// winit's view was out of the panel, so the clicks seen meanwhile never reached it.
    ViewLost { last: Option<Point> },
    /// The panel hid.
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Effect {
    /// Where the click winit just dispatched happened.
    Deliver(Point),
    /// The press ended without an Up: the runtime should give it up at this point.
    ReleaseLost(Point),
}

pub(super) fn step(hold: &mut Hold, event: Event) -> Effects<Effect> {
    match event {
        Event::Seen { click, over } => {
            // A press with nothing queued ahead of it, on one winit already dispatched: that
            // one's Up never came, and it must end before this press reaches the runtime.
            let lost = match hold {
                Hold::Pressed { clicks } if click.down && clicks.is_empty() => Some(click.at),
                Hold::Releasing { clicks, at, .. } if click.down && clicks.is_empty() => Some(*at),
                _ => None,
            };
            match hold {
                Hold::Free { .. } if !over => {}
                Hold::Free { ups } if !click.down => ups.push_back(click.at),
                Hold::Pressed { clicks } | Hold::Releasing { clicks, .. } if !click.down => clicks.push_back(click),
                _ => {
                    let mut clicks = std::mem::take(hold).into_clicks();
                    clicks.push_back(click);
                    *hold = Hold::Pressed { clicks };
                }
            }
            lost.map_or(Effects::none(), |at| Effects::one(Effect::ReleaseLost(at)))
        }
        Event::Dispatched { down } => {
            let point = match hold {
                Hold::Free { ups } if !down => ups.pop_front(),
                Hold::Pressed { clicks } | Hold::Releasing { clicks, .. } if clicks.front().is_some_and(|click| click.down == down) =>
                    clicks.pop_front().map(|click| click.at),
                _ => None,
            };
            point.map_or(Effects::none(), |at| Effects::one(Effect::Deliver(at)))
        }
        Event::Released => {
            // Native callbacks can queue another Down before winit dispatches this Up.
            let clicks = std::mem::take(hold).into_clicks();
            *hold = if clicks.iter().any(|click| click.down) { Hold::Pressed { clicks } }
                else { Hold::Free { ups: clicks.into_iter().map(|click| click.at).collect() } };
            Effects::none()
        }
        Event::Sampled { button_down, now, at } => sampled(hold, button_down, now, at),
        Event::ViewLost { last } => {
            *hold = Hold::default();
            last.map_or(Effects::none(), |at| Effects::one(Effect::ReleaseLost(at)))
        }
        Event::Stopped => {
            *hold = Hold::default();
            Effects::none()
        }
    }
}

fn sampled(hold: &mut Hold, button_down: bool, now: Instant, point: Point) -> Effects<Effect> {
    let due = hold.deadline();
    match hold {
        Hold::Free { .. } => Effects::none(),
        Hold::Pressed { .. } if button_down => Effects::none(),
        Hold::Pressed { clicks } => {
            *hold = Hold::Releasing { clicks: std::mem::take(clicks), since: now, at: point };
            Effects::none()
        }
        Hold::Releasing { clicks, .. } if button_down => {
            *hold = Hold::Pressed { clicks: std::mem::take(clicks) };
            Effects::none()
        }
        Hold::Releasing { at, .. } => {
            if due.is_some_and(|due| now < due) { return Effects::none(); }
            let at = *at;
            *hold = Hold::default();
            Effects::one(Effect::ReleaseLost(at))
        }
    }
}

#[cfg(test)]
#[path = "hold_tests.rs"]
mod tests;
