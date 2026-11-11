//! Native panel presentation: prewarm, sharing, and fenced fades. `Presence` decides; this
//! carries out what it asks of AppKit.

use std::{cell::Cell, rc::Rc, time::Duration};
use objc2::{msg_send, rc::Retained};
use objc2_app_kit::NSPanel;
use crate::machine::{Attempt, Effects};
use super::{Panel, set_sharing};
use super::presence::{Effect, Event, Presence};

pub(crate) struct Prewarm {
    presence: Rc<Cell<Presence>>,
    panel: Retained<NSPanel>,
}
impl Drop for Prewarm {
    fn drop(&mut self) {
        let effects = step(&self.presence, Event::PrewarmEnds);
        carry_out(&self.panel, &self.presence, effects);
    }
}

/// Copied out and back, so nothing is held while AppKit runs: a fade's completion can step it
/// from inside `carry_out`.
fn step(presence: &Cell<Presence>, event: Event) -> Effects<Effect> {
    let mut now = presence.get();
    let effects = now.step(event);
    presence.set(now);
    effects
}

fn carry_out(panel: &Retained<NSPanel>, presence: &Rc<Cell<Presence>>, effects: Effects<Effect>) {
    for effect in effects {
        match effect {
            Effect::SetSharing(exclude) => set_sharing(panel, exclude),
            Effect::RejoinSpaces => rejoin_spaces(panel),
            Effect::OrderIn { clear_alpha } => if !panel.isVisible() {
                // SAFETY: AppKit calls on the main thread.
                unsafe {
                    if clear_alpha { panel.setAlphaValue(0.0); }
                    panel.orderFrontRegardless();
                }
            },
            Effect::OrderOut => panel.orderOut(None),
            // SAFETY: an AppKit call on the main thread.
            Effect::SetAlpha(alpha) => unsafe { panel.setAlphaValue(alpha) },
            Effect::IgnoreMouse => panel.setIgnoresMouseEvents(true),
            Effect::DrawOccluded(on) => presence.set(presence.get().with_draws_occluded(on)),
            Effect::Fade { alpha, over } => fade(panel, presence, alpha, over),
            // SAFETY: an AppKit call on the main thread.
            Effect::ResignKey => unsafe { panel.resignKeyWindow() },
        }
    }
}

/// Puts a panel that joins every Space back on all of them before it orders in. After the
/// app's activation policy changes (its Dock icon shown or hidden), the window server can
/// tie such a panel to one Space while AppKit still reports the old mask, and setting the
/// same mask again changes nothing. Clearing it and setting it back makes the window
/// server apply it again. A panel still shown on another Space orders out first, so `show`
/// orders it back in on this one.
fn rejoin_spaces(panel: &NSPanel) {
    use objc2_app_kit::NSWindowCollectionBehavior as Behavior;
    // SAFETY: AppKit calls on the main thread.
    unsafe {
        let behavior = panel.collectionBehavior();
        if !behavior.contains(Behavior::CanJoinAllSpaces) { return; }
        if panel.isVisible() {
            if panel.isOnActiveSpace() { return; }
            panel.orderOut(None);
        }
        panel.setCollectionBehavior(Behavior::Default);
        panel.setCollectionBehavior(behavior);
    }
}

/// Animates the panel's alpha to `alpha`, replacing any fade in flight. Over a length, its
/// completion reports back under the fence; with none, it lands at once.
fn fade(panel: &Retained<NSPanel>, presence: &Rc<Cell<Presence>>, alpha: f64, over: Option<(Duration, Attempt)>) {
    use objc2::runtime::{AnyClass, AnyObject};
    // SAFETY: AppKit calls on the main thread; the completion handler runs there too.
    unsafe {
        if let Some(context) = AnyClass::get("NSAnimationContext") {
            let _: () = msg_send![context, beginGrouping];
            let current: *mut AnyObject = msg_send![context, currentContext];
            let _: () = msg_send![current, setDuration: over.map_or(0.0, |(length, _)| length.as_secs_f64())];
            if let Some((_, fence)) = over {
                let (panel, presence) = (panel.clone(), presence.clone());
                let done = block2::RcBlock::new(move || {
                    let effects = step(&presence, Event::FadeEnded { fence });
                    carry_out(&panel, &presence, effects);
                });
                let _: () = msg_send![current, setCompletionHandler: &*done];
            }
            let animator: *mut AnyObject = msg_send![&**panel, animator];
            let _: () = msg_send![animator, setAlphaValue: alpha];
            let _: () = msg_send![context, endGrouping];
        }
        if over.is_none() { panel.setAlphaValue(alpha); }
    }
}

impl Panel {
    /// Reveals the panel, fading its alpha in over `fade` (instantly when zero).
    pub fn show(&self, fade: Duration) {
        self.keep_view();
        let effects = step(&self.presence, Event::Show { fade });
        self.refresh_monitors();
        self.pointer.observe(&self.panel, self.wake.as_ref());
        carry_out(&self.panel, &self.presence, effects);
    }

    pub fn prewarm(&self) -> Prewarm {
        debug_assert!(self.options.prewarm);
        let effects = step(&self.presence, Event::PrewarmBegins);
        carry_out(&self.panel, &self.presence, effects);
        Prewarm { presence: self.presence.clone(), panel: self.panel.clone() }
    }

    /// Hides the panel, fading its alpha out over `fade` (instantly when zero). Input stops at
    /// once; `show` restores hit-testing through `pointer.observe`.
    pub fn hide(&self, fade: Duration) {
        let effects = step(&self.presence, Event::Hide { fade });
        self.stop_pointer();
        carry_out(&self.panel, &self.presence, effects);
    }

    /// `sharingType` none while shown, or read-only so recorders see the panel again. Hidden,
    /// the panel stays read-only and the request applies at its next show.
    pub fn set_capture_exclusion(&self, exclude: bool) -> crate::OverlaySupport {
        let effects = step(&self.presence, Event::Request { exclude });
        carry_out(&self.panel, &self.presence, effects);
        self.capture_exclusion()
    }
}
