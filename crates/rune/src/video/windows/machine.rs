//! The Windows player's phases as plain data: what its decoder is doing and which picture time
//! the screen holds. [`step`] takes one event and returns what to do; the backend (`mod.rs`)
//! carries the effects out on the UI thread, in the frame that draws the clip.
//!
//! Times are unwrapped seconds: a loop keeps counting past the duration, so a clip's pictures
//! always move forward and the clock never jumps back at the loop point.

use crate::machine::Effects;
use crate::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Phase {
    /// No decoder: never asked for, or let go while undrawn. The surface keeps the picture at
    /// `at`, and opening starts there.
    Idle { at: f64 },
    /// The decoder is opening the source.
    Opening { at: f64, play: bool },
    /// Waiting for the picture at `at`: after opening, a seek, or a stall while playing, when
    /// the clock holds. `play`: playback goes on once it lands.
    Buffering { at: f64, play: bool },
    /// Pictures follow the clock.
    Playing { clock: Anchor },
    /// The picture at `at` holds.
    Paused { at: f64 },
    /// The last picture showed and the clip doesn't loop.
    Ended { at: f64 },
    Failed,
}

impl Default for Phase {
    fn default() -> Self {
        Phase::Idle { at: 0.0 }
    }
}

/// The playback clock: `pos` media seconds at `at`, moving `rate` media seconds per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Anchor {
    pub at: Instant,
    pub pos: f64,
    pub rate: f64,
}

impl Anchor {
    pub(crate) fn position(&self, t: Instant) -> f64 {
        match t.checked_duration_since(self.at) {
            Some(d) => self.pos + d.as_secs_f64() * self.rate,
            None => self.pos - self.at.duration_since(t).as_secs_f64() * self.rate,
        }
    }

    /// When the clock reaches `pos`.
    pub(crate) fn when(&self, pos: f64) -> Instant {
        let d = (pos - self.pos) / self.rate;
        if d >= 0.0 { self.at + std::time::Duration::from_secs_f64(d) } else { self.at - std::time::Duration::from_secs_f64(-d) }
    }
}

pub(super) enum Event {
    /// A frame wants the clip's pictures and no decoder runs.
    Open { play: bool },
    Opened,
    Failed,
    /// The first picture at or after the phase's time reached the surface, at presentation `target`.
    Shown { pts: f64, target: Instant, lead: f64, rate: f64 },
    Play { target: Instant, lead: f64, rate: f64 },
    /// Hold the picture on screen, at `at`.
    Pause { at: f64 },
    Seek { to: f64 },
    Rate { target: Instant, rate: f64 },
    /// Playing, with no picture due and the decoder behind: hold the clock at `at`.
    Stalled { at: f64 },
    /// The last picture showed, at `at`.
    Ended { at: f64 },
    /// Drawn again after a gap with nothing heard: the clock picks up at the picture on screen.
    Resumed { target: Instant, at: f64, lead: f64 },
    /// The decoder let go of the clip.
    Released { at: f64 },
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    /// Start a decoder at `at`.
    Open { at: f64 },
    /// Move the decoder to `to`.
    Seek { to: f64 },
    /// The clock changed: sound follows it (`None`: silent).
    Clock(Option<Anchor>),
}

/// The clock that shows `pts` at `target`: `lead` ahead of it, so later presentations land
/// mid-way between picture boundaries and a little jitter never shows a picture twice.
fn anchor(target: Instant, pts: f64, lead: f64, rate: f64) -> Anchor {
    Anchor { at: target, pos: pts + lead, rate }
}

pub(super) fn step(phase: &mut Phase, event: Event) -> Effects<Effect> {
    use Phase as P;
    match (*phase, event) {
        (P::Failed, _) => Effects::none(),
        (_, Event::Failed) => {
            *phase = P::Failed;
            Effects::one(Effect::Clock(None))
        }
        (P::Idle { at }, Event::Open { play }) => {
            *phase = P::Opening { at, play };
            Effects::one(Effect::Open { at })
        }
        (P::Opening { at, play }, Event::Opened) => {
            *phase = P::Buffering { at, play };
            Effects::none()
        }
        (P::Buffering { play, .. }, Event::Shown { pts, target, lead, rate }) => {
            if play {
                let clock = anchor(target, pts, lead, rate);
                *phase = P::Playing { clock };
                Effects::one(Effect::Clock(Some(clock)))
            } else {
                *phase = P::Paused { at: pts };
                Effects::none()
            }
        }
        (P::Opening { at, .. }, Event::Play { .. }) => {
            *phase = P::Opening { at, play: true };
            Effects::none()
        }
        (P::Buffering { at, .. }, Event::Play { .. }) => {
            *phase = P::Buffering { at, play: true };
            Effects::none()
        }
        (P::Paused { at }, Event::Play { target, lead, rate }) => {
            let clock = anchor(target, at, lead, rate);
            *phase = P::Playing { clock };
            Effects::one(Effect::Clock(Some(clock)))
        }
        (P::Opening { at, .. }, Event::Pause { .. }) => {
            *phase = P::Opening { at, play: false };
            Effects::none()
        }
        (P::Buffering { at, .. }, Event::Pause { .. }) => {
            *phase = P::Buffering { at, play: false };
            Effects::none()
        }
        (P::Playing { .. }, Event::Pause { at }) => {
            *phase = P::Paused { at };
            Effects::one(Effect::Clock(None))
        }
        (P::Idle { .. }, Event::Seek { to }) => {
            *phase = P::Idle { at: to };
            Effects::none()
        }
        (P::Opening { play, .. }, Event::Seek { to }) => {
            *phase = P::Opening { at: to, play };
            Effects::one(Effect::Seek { to })
        }
        (P::Buffering { play, .. }, Event::Seek { to }) => {
            *phase = P::Buffering { at: to, play };
            Effects::one(Effect::Seek { to })
        }
        (P::Playing { .. }, Event::Seek { to }) => {
            *phase = P::Buffering { at: to, play: true };
            Effects::two(Effect::Clock(None), Effect::Seek { to })
        }
        (P::Paused { .. } | P::Ended { .. }, Event::Seek { to }) => {
            *phase = P::Buffering { at: to, play: false };
            Effects::one(Effect::Seek { to })
        }
        (P::Playing { clock }, Event::Rate { target, rate }) => {
            let clock = Anchor { at: target, pos: clock.position(target), rate };
            *phase = P::Playing { clock };
            Effects::one(Effect::Clock(Some(clock)))
        }
        (P::Playing { .. }, Event::Stalled { at }) => {
            *phase = P::Buffering { at, play: true };
            Effects::one(Effect::Clock(None))
        }
        (P::Playing { .. }, Event::Ended { at }) => {
            *phase = P::Ended { at };
            Effects::one(Effect::Clock(None))
        }
        (P::Playing { clock }, Event::Resumed { target, at, lead }) => {
            let clock = anchor(target, at, lead, clock.rate);
            *phase = P::Playing { clock };
            Effects::one(Effect::Clock(Some(clock)))
        }
        (P::Opening { .. } | P::Buffering { .. } | P::Playing { .. } | P::Paused { .. } | P::Ended { .. }, Event::Released { at }) => {
            *phase = P::Idle { at };
            Effects::one(Effect::Clock(None))
        }
        _ => Effects::none(),
    }
}
