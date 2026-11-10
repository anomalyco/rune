//! Whether an overlay is shown, hidden or fading out, and when a hidden one gives its GPU
//! surface back. The overlay carries the effects out: its display link, native window and
//! surface.

use std::time::Duration;

use crate::machine::Effects;
use crate::time::Instant;

/// What a hidden overlay does with its GPU surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Release {
    /// Keeps it, and its pixels, for the next show.
    Keep,
    /// Gives it back as it hides (`release_gpu_when_hidden`).
    Now,
    /// Gives it back once it has stayed hidden this long (`release_gpu_after`).
    After(Duration),
}

impl Release {
    /// From `ConfiguredOverlay`'s release delay. macOS honours only `release_gpu_when_hidden`:
    /// a prewarmed panel with `release_gpu_after` (a HUD) retains its pixels.
    pub(super) fn new(after: Option<Duration>) -> Release {
        match after {
            Some(after) if after.is_zero() => Release::Now,
            Some(after) if !cfg!(target_os = "macos") => Release::After(after),
            _ => Release::Keep,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Presence {
    /// Takes no input and draws nothing. With [`Release::After`], its GPU surface goes at
    /// `release_at`.
    Hidden { release_at: Option<Instant> },
    /// Takes input and draws. `occluded` (macOS): none of it is on screen, covered or on
    /// sleeping displays.
    Shown { occluded: bool },
    /// Hidden by `OverlayHandle::fade_out`: takes no input but keeps drawing its exit until
    /// `until`. `length` is the whole fade, for its alpha (Windows). A panel fading toward
    /// alpha zero reads as covered, so it never becomes `occluded` mid-fade; one that was
    /// stays so.
    FadingOut { until: Instant, length: Duration, occluded: bool },
}

impl Default for Presence {
    fn default() -> Self { Presence::Hidden { release_at: None } }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    /// The overlay is on screen now.
    Show,
    /// `hide` or `fade_out` at `now`; `fade` is zero where the platform doesn't fade windows.
    Hide { fade: Duration, now: Instant },
    /// A deadline (`Presence::deadline`) may have passed.
    Due { now: Instant },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Effect {
    /// Its display link, timer frames and live values stop.
    StopDrawing,
    /// Input stops and the native window hides, fading over `fade`.
    HideWindow { fade: Duration },
    /// Give the GPU surface back now.
    ReleaseGpu,
}

impl Presence {
    pub(super) fn step(&mut self, event: Event, release: Release) -> Effects<Effect> {
        match (*self, event) {
            (Presence::Shown { .. }, Event::Show) => Effects::none(),
            (_, Event::Show) => {
                *self = Presence::Shown { occluded: false };
                Effects::none()
            }
            // Fading again restarts the fade, and the window's alpha with it.
            (Presence::Shown { occluded } | Presence::FadingOut { occluded, .. }, Event::Hide { fade, now }) if !fade.is_zero() => {
                *self = Presence::FadingOut { until: now + fade, length: fade, occluded };
                Effects::one(Effect::HideWindow { fade })
            }
            (Presence::Hidden { .. }, Event::Hide { fade, .. }) => Effects::two(Effect::StopDrawing, Effect::HideWindow { fade }),
            (_, Event::Hide { now, .. }) => self.hide_now(now, release),
            (Presence::FadingOut { until, .. }, Event::Due { now }) if until <= now => self.hide_now(now, release),
            (Presence::Hidden { release_at: Some(at) }, Event::Due { now }) if at <= now => {
                *self = Presence::Hidden { release_at: None };
                Effects::one(Effect::ReleaseGpu)
            }
            (_, Event::Due { .. }) => Effects::none(),
        }
    }

    /// Shown or fading out, it hides now: drawing stops, then the window goes, and the GPU
    /// surface now or later. A fade-out kept the surface until here, so the exit played out
    /// on the window and its last frames reached the screen.
    fn hide_now(&mut self, now: Instant, release: Release) -> Effects<Effect> {
        let release_at = match release {
            Release::After(after) => Some(now + after),
            Release::Keep | Release::Now => None,
        };
        *self = Presence::Hidden { release_at };
        let mut effects = Effects::two(Effect::StopDrawing, Effect::HideWindow { fade: Duration::ZERO });
        if release == Release::Now { effects.push(Effect::ReleaseGpu); }
        effects
    }

    /// Takes input: shown, not fading out.
    pub(super) fn shown(&self) -> bool { matches!(self, Presence::Shown { .. }) }

    /// Shown, or fading out: frames still draw.
    pub(super) fn drawing(&self) -> bool { !matches!(self, Presence::Hidden { .. }) }

    pub(super) fn fading(&self) -> bool { matches!(self, Presence::FadingOut { .. }) }

    pub(super) fn occluded(&self) -> bool {
        matches!(self, Presence::Shown { occluded: true } | Presence::FadingOut { occluded: true, .. })
    }

    /// Takes an occlusion reading; false when it doesn't apply: a fading-out panel reads as
    /// covered once its alpha nears zero, but it's still drawing its exit. A hidden overlay
    /// keeps none, and a show reads it again.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) fn set_occluded(&mut self, covered: bool) -> bool {
        match self {
            Presence::FadingOut { .. } if covered => false,
            Presence::Shown { occluded } | Presence::FadingOut { occluded, .. } => {
                *occluded = covered;
                true
            }
            Presence::Hidden { .. } => true,
        }
    }

    #[cfg(test)]
    pub(super) fn fade_end(&self) -> Option<Instant> {
        match *self {
            Presence::FadingOut { until, .. } => Some(until),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(super) fn release_at(&self) -> Option<Instant> {
        match *self {
            Presence::Hidden { release_at } => release_at,
            _ => None,
        }
    }

    /// When `step` next wants a `Due`: the end of a fade-out, or a hidden surface's release.
    pub(super) fn deadline(&self) -> Option<Instant> {
        match *self {
            Presence::Hidden { release_at } => release_at,
            Presence::Shown { .. } => None,
            Presence::FadingOut { until, .. } => Some(until),
        }
    }
}

#[cfg(test)]
#[path = "presence_tests.rs"]
mod tests;
