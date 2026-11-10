//! What a scroll bar shows: always, nothing (its content fits), or an overlay with a phase of
//! its own, and the thumb's glide to a new length. A step returns what the bar's opacity should
//! do next; the caller carries it out on the live value.
//!
//! The due times here are deadlines, not wakes: the opacity's own spring and pulse are what ask
//! the runtime for frames, so a step is only run when the pointer, a press or a follow arrives,
//! and it reads the deadline against that event's `now`.

use std::time::Duration;

use rune::machine::Effects;

/// How long after a pulse's hold its fade-out has run.
pub(super) const FADE_TAIL: f64 = 0.35;
/// How long a length glide runs, through which the scrolls that follow it (a feed keeping to its
/// end) glide too rather than cutting it short.
const GLIDE_SPAN: f64 = 0.5;

/// What a bar shows, and the timers that decide when it stops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum BarState {
    /// Not laid out yet: the first follow decides.
    Uninit,
    /// Nothing to show: the content fits, or the track is too short.
    NoOverflow,
    /// Always up, however the pointer moves.
    Always { glide: Glide },
    /// An overlay: shown while scrolling or revealed, then faded.
    Overlay { phase: Phase, glide: Glide },
}

/// An overlay bar's phase.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Phase {
    /// Faded out at rest.
    Hidden,
    /// A scroll's fade: up until `until`, its hold and fade over.
    Pulsing { until: f64 },
    /// Held up by the pointer or a press, until both let it go.
    Held,
}

/// The thumb's glide to a new length, when it has one running.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Glide {
    Idle,
    Running { until: f64 },
}

/// A frame that followed the viewport: what it shows, at the size and scroll it's drawn at.
#[derive(Clone, Copy, Debug)]
pub(super) struct Follow {
    pub shows: bool,
    pub overlay: bool,
    pub scrolled: bool,
    pub instant: bool,
    pub resized: bool,
    pub lengthened: bool,
    pub extent_changed: bool,
    pub pressed: bool,
    pub revealed: bool,
    pub now: f64,
    pub hold: Duration,
}

/// Something that can change what a bar shows.
#[derive(Clone, Copy, Debug)]
pub(super) enum Event {
    /// A frame followed the viewport.
    Follow(Follow),
    /// Something holds an overlay bar up: the pointer at its edge or over its target, or a press.
    Hold,
    /// Nothing holds it any more: it fades, or drops at once where the whole area reveals it.
    LetGo { pressed: bool, revealed: bool, area_reveals: bool, now: f64, hold: Duration },
}

/// What a step asks the bar to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Effect {
    /// Whether the thumb glides to its new length, and whether a glide it was in was cut.
    Glide { glides: bool, cut: bool },
    /// What its opacity does.
    Opacity(Opacity),
}

/// A change to a bar's opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Opacity {
    /// Eases to `to`.
    Set(f32),
    /// Lands at `to` at once.
    Snap(f32),
    /// Holds up for `hold`, then fades out.
    Pulse { hold: Duration },
}

impl BarState {
    /// Whether the thumb is gliding to a new length at `now`.
    pub(super) fn gliding(self, now: f64) -> bool {
        match self.glide() {
            Glide::Idle => false,
            Glide::Running { until } => now < until,
        }
    }

    /// Whether a bar at this state is up at `now`: an always-shown bar, or an overlay whose
    /// pulse hasn't faded out.
    pub(super) fn seen(self, now: f64) -> bool {
        match self {
            Self::Always { .. } => true,
            Self::Overlay { phase, .. } => phase.visible(now),
            Self::Uninit | Self::NoOverflow => false,
        }
    }

    /// When an overlay's pulse ends, or `f64::INFINITY` while held, or `-1.0` when it isn't
    /// pulsing.
    pub(super) fn pulse_until(self) -> f64 {
        match self {
            Self::Overlay { phase: Phase::Pulsing { until }, .. } => until,
            Self::Overlay { phase: Phase::Held, .. } => f64::INFINITY,
            _ => -1.0,
        }
    }

    fn glide(self) -> Glide {
        match self {
            Self::Always { glide } | Self::Overlay { glide, .. } => glide,
            Self::Uninit | Self::NoOverflow => Glide::Idle,
        }
    }
}

impl Phase {
    fn visible(self, now: f64) -> bool {
        match self {
            Self::Hidden => false,
            Self::Pulsing { until } => now < until,
            Self::Held => true,
        }
    }
}

/// What the bar shows after `event`.
pub(super) fn step(state: &mut BarState, event: Event) -> Effects<Effect> {
    match event {
        Event::Follow(f) => follow(state, f),
        Event::Hold => hold(state),
        Event::LetGo { pressed, revealed, area_reveals, now, hold } => {
            let BarState::Overlay { glide, .. } = *state else { return Effects::none() };
            if pressed || revealed {
                return Effects::none();
            }
            let (phase, opacity) = if area_reveals {
                (Phase::Hidden, Opacity::Set(0.0))
            } else {
                (Phase::Pulsing { until: now + (hold.as_secs_f64() + FADE_TAIL) }, Opacity::Pulse { hold })
            };
            *state = BarState::Overlay { phase, glide };
            Effects::one(Effect::Opacity(opacity))
        }
    }
}

/// The pointer at its edge, over its target or holding it: an overlay stays up.
fn hold(state: &mut BarState) -> Effects<Effect> {
    match *state {
        BarState::Overlay { glide, .. } => {
            *state = BarState::Overlay { phase: Phase::Held, glide };
            Effects::one(Effect::Opacity(Opacity::Set(1.0)))
        }
        _ => Effects::none(),
    }
}

fn follow(state: &mut BarState, f: Follow) -> Effects<Effect> {
    let prev = *state;
    let gliding = prev.gliding(f.now);
    // A shown bar glides when its content changes length; nothing glides where nobody can see
    // it, and a hand scroll or an instant change lands it at once.
    let glides = f.shows && !f.scrolled && !f.instant && !f.resized && (f.lengthened || gliding) && prev.seen(f.now);
    let cut = gliding && !glides;
    let glide = if !glides {
        Glide::Idle
    } else if f.lengthened {
        Glide::Running { until: f.now + GLIDE_SPAN }
    } else {
        prev.glide()
    };
    let mut effects = Effects::one(Effect::Glide { glides, cut });

    if !f.shows {
        *state = BarState::NoOverflow;
        if prev != BarState::NoOverflow {
            effects.push(Effect::Opacity(Opacity::Snap(1.0)));
        }
        return effects;
    }

    if !f.overlay {
        *state = BarState::Always { glide };
        match prev {
            BarState::Overlay { .. } if !f.instant => effects.push(Effect::Opacity(Opacity::Set(1.0))),
            BarState::Always { .. } => {}
            _ => effects.push(Effect::Opacity(Opacity::Snap(1.0))),
        }
        return effects;
    }

    if f.pressed || f.revealed {
        *state = BarState::Overlay { phase: Phase::Held, glide };
        effects.push(Effect::Opacity(Opacity::Set(1.0)));
        return effects;
    }

    let pulse = |f: &Follow| (Phase::Pulsing { until: f.now + (f.hold.as_secs_f64() + FADE_TAIL) }, Some(Opacity::Pulse { hold: f.hold }));
    let (phase, opacity) = match prev {
        BarState::Always { .. } => {
            if f.scrolled && !f.instant {
                pulse(&f)
            } else if f.instant {
                (Phase::Hidden, Some(Opacity::Snap(0.0)))
            } else {
                (Phase::Hidden, Some(Opacity::Set(0.0)))
            }
        }
        BarState::Overlay { phase, .. } => {
            if f.scrolled && !f.instant || (f.scrolled || f.extent_changed) && f.instant && phase.visible(f.now) {
                pulse(&f)
            } else {
                (phase, None)
            }
        }
        _ => {
            if f.scrolled && !f.instant {
                pulse(&f)
            } else {
                (Phase::Hidden, Some(Opacity::Snap(0.0)))
            }
        }
    };
    *state = BarState::Overlay { phase, glide };
    if let Some(opacity) = opacity {
        effects.push(Effect::Opacity(opacity));
    }
    effects
}
