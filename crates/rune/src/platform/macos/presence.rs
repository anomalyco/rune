//! Whether a panel belongs on screen, and whether its capture exclusion is applied.
//!
//! Panels join every Space. An excluded window can blank Mission Control's thumbnails even at
//! alpha zero, and sharing readbacks can remain stale after a reset.
//! Every hidden panel therefore orders out, including prewarmed panels. Its view, surface and
//! cached pixels stay alive. Exclusion holds from show until the hide's fade finishes; ordering
//! out precedes restoring sharing. A show or hide during a fade replaces it, and the replaced
//! fade's completion, which AppKit still delivers, no longer matches the phase's fence.

use std::time::Duration;
use crate::machine::{Attempt, Effects, Seq};

/// Where a panel is between hidden and shown. A prewarm guard lasts one draw and doesn't depend
/// on show or hide, so any phase can hold one: `Prewarmed` is that guard with no show behind it,
/// and `warming` is the same guard over a phase that has one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    OrderedOut,
    /// Ordered in at alpha zero for one surface acquisition.
    Prewarmed,
    Shown { warming: bool },
    FadingIn { fence: Attempt, warming: bool },
    /// `seen`: it was on screen when the hide started. A panel that wasn't still runs the fade,
    /// but holds no place on screen and no exclusion meanwhile.
    FadingOut { fence: Attempt, seen: bool, warming: bool },
}

impl Phase {
    fn at_rest(warming: bool) -> Self { if warming { Self::Prewarmed } else { Self::OrderedOut } }

    fn warming(self) -> bool {
        match self {
            Self::OrderedOut => false,
            Self::Prewarmed => true,
            Self::Shown { warming } | Self::FadingIn { warming, .. } | Self::FadingOut { warming, .. } => warming,
        }
    }

    fn with_warming(self, warming: bool) -> Self {
        match self {
            Self::OrderedOut | Self::Prewarmed => Self::at_rest(warming),
            Self::Shown { .. } => Self::Shown { warming },
            Self::FadingIn { fence, .. } => Self::FadingIn { fence, warming },
            Self::FadingOut { fence, seen, .. } => Self::FadingOut { fence, seen, warming },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Event {
    /// A show starts, fading in over `fade` (at once when zero).
    Show { fade: Duration },
    /// A hide starts, fading out over `fade` (at once when zero).
    Hide { fade: Duration },
    PrewarmBegins,
    PrewarmEnds,
    /// AppKit finished the fade started under `fence`.
    FadeEnded { fence: Attempt },
    /// The app asked for capture exclusion on or off.
    Request { exclude: bool },
}

/// What a transition asks of AppKit, in order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Effect {
    /// `sharingType` none when true, read-only when false.
    SetSharing(bool),
    /// Puts a panel that joins every Space back on all of them. May order it out.
    RejoinSpaces,
    /// Orders in if AppKit doesn't have it in, dropping its alpha to zero first if asked.
    OrderIn { clear_alpha: bool },
    OrderOut,
    SetAlpha(f64),
    IgnoreMouse,
    /// Switches what `occlusionState` answers (`Presence::draws_occluded`).
    DrawOccluded(bool),
    /// Takes the alpha to `alpha`, replacing any fade in flight: over a length, answering with
    /// `Event::FadeEnded` under its fence, or at once.
    Fade { alpha: f64, over: Option<(Duration, Attempt)> },
    ResignKey,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Presence {
    phase: Phase,
    /// Issues every show's and hide's fence, so a finished fade acts only if nothing came after.
    fences: Seq,
    requested: bool,
    /// What `occlusionState` answers. It trails the phase on purpose: an observer inside
    /// `orderFrontRegardless` reads it, and must see an alpha-zero panel as occluded until a
    /// fade starts or a prewarm has ordered in.
    draws_occluded: bool,
}

impl Presence {
    pub(crate) fn new(requested: bool) -> Self { Self { requested, ..Self::default() } }

    /// What the app asked for, shown or not.
    pub(crate) fn requested(self) -> bool { self.requested }

    /// Whether `sharingType` should be none now; read-only otherwise.
    pub(crate) fn excluded(self) -> bool { self.requested && self.ordered() }

    /// Outside a scoped prewarm acquisition, only a shown or fading-out panel is ordered in.
    pub(crate) fn ordered(self) -> bool {
        match self.phase {
            Phase::OrderedOut | Phase::Prewarmed => false,
            Phase::Shown { .. } | Phase::FadingIn { .. } => true,
            Phase::FadingOut { seen, .. } => seen,
        }
    }

    /// Whether the app wants it seen: from a show until the next hide, so not during a fade-out.
    pub(crate) fn visible(self) -> bool { matches!(self.phase, Phase::Shown { .. } | Phase::FadingIn { .. }) }

    /// Whether `occlusionState` reports the panel visible, so it may draw though AppKit calls
    /// it occluded while its alpha is near zero.
    pub(crate) fn draws_occluded(self) -> bool { self.draws_occluded }

    pub(crate) fn with_draws_occluded(self, draws_occluded: bool) -> Self { Self { draws_occluded, ..self } }

    /// What `draws_occluded` settles on in this phase: a prewarm acquisition or a fade.
    fn wants_draw_occluded(self) -> bool {
        self.phase.warming() || matches!(self.phase, Phase::FadingIn { .. } | Phase::FadingOut { .. })
    }

    pub(crate) fn step(&mut self, event: Event) -> Effects<Effect> {
        match event {
            Event::Show { fade } => self.show(fade),
            Event::Hide { fade } => self.hide(fade),
            Event::PrewarmBegins => {
                self.phase = self.phase.with_warming(true);
                Effects::Many(vec![Effect::SetAlpha(0.0), Effect::IgnoreMouse, Effect::OrderIn { clear_alpha: false }, Effect::DrawOccluded(true)])
            }
            Event::PrewarmEnds => {
                self.phase = self.phase.with_warming(false);
                let mut effects = Effects::one(Effect::DrawOccluded(self.wants_draw_occluded()));
                if !self.ordered() { effects.push(Effect::OrderOut); }
                effects
            }
            Event::FadeEnded { fence } => self.fade_ended(fence),
            Event::Request { exclude } => {
                self.requested = exclude;
                Effects::one(Effect::SetSharing(self.excluded()))
            }
        }
    }

    /// Excluded before any of it can be seen: before it orders in or its alpha rises.
    fn show(&mut self, fade: Duration) -> Effects<Effect> {
        let (fence, warming, fading) = (self.fences.next(), self.phase.warming(), !fade.is_zero());
        self.phase = if fading { Phase::FadingIn { fence, warming } } else { Phase::Shown { warming } };
        Effects::Many(vec![
            Effect::SetSharing(self.excluded()),
            Effect::RejoinSpaces,
            Effect::OrderIn { clear_alpha: fading },
            Effect::DrawOccluded(self.wants_draw_occluded()),
            Effect::Fade { alpha: 1.0, over: fading.then_some((fade, fence)) },
        ])
    }

    /// Clicks and scrolls go to the window below for the whole fade. Shown until the fade-out
    /// finishes, so the exclusion holds while it can still be seen. A fade it replaces stops
    /// counting as drawing during the setup, and its own starts counting with its alpha.
    fn hide(&mut self, fade: Duration) -> Effects<Effect> {
        let (fence, warming, fading, seen) = (self.fences.next(), self.phase.warming(), !fade.is_zero(), self.ordered());
        self.phase = if fading { Phase::FadingOut { fence, seen, warming } } else { Phase::at_rest(warming) };
        let mut effects = Effects::Many(vec![
            Effect::DrawOccluded(warming),
            Effect::IgnoreMouse,
            Effect::DrawOccluded(self.wants_draw_occluded()),
            Effect::Fade { alpha: 0.0, over: fading.then_some((fade, fence)) },
        ]);
        if !fading { effects.push(Effect::OrderOut); }
        effects.push(Effect::ResignKey);
        effects.push(Effect::SetSharing(self.excluded()));
        effects
    }

    /// A fade-out's end orders out before restoring sharing. A fence the phase doesn't hold
    /// belongs to a fade a later show or hide replaced.
    fn fade_ended(&mut self, ended: Attempt) -> Effects<Effect> {
        match self.phase {
            Phase::FadingIn { fence, warming } if fence == ended => {
                self.phase = Phase::Shown { warming };
                Effects::one(Effect::DrawOccluded(warming))
            }
            Phase::FadingOut { fence, warming, .. } if fence == ended => {
                self.phase = Phase::at_rest(warming);
                Effects::Many(vec![Effect::DrawOccluded(warming), Effect::OrderOut, Effect::SetSharing(self.excluded())])
            }
            _ => Effects::none(),
        }
    }
}

#[cfg(test)]
#[path = "presence_tests.rs"]
mod tests;
