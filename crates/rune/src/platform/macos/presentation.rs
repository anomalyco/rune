//! Native panel presentation: prewarm, sharing, and generation-fenced fades.

use std::{cell::Cell, rc::Rc, time::Duration};
use objc2::msg_send;
use super::{Panel, set_sharing};
use super::presence::Presence;

pub(crate) struct Prewarm(Rc<Cell<bool>>);
impl Drop for Prewarm { fn drop(&mut self) { self.0.set(false); } }

impl Panel {
    /// Reveals the panel, fading its alpha in over `fade` (instantly when zero).
    pub fn show(&self, fade: Duration) {
        self.keep_view();
        self.visible.set(true);
        self.refresh_monitors();
        self.pointer.observe(&self.panel, self.wake.as_ref());
        // Excluded before any of it can be seen: before it orders in or its alpha rises.
        let generation = self.update_presence(|p| p.show());
        if !self.options.prewarm && !self.panel.isVisible() {
            // SAFETY: AppKit calls on the main thread.
            unsafe {
                if !fade.is_zero() { self.panel.setAlphaValue(0.0); }
                self.panel.orderFrontRegardless();
            }
        }
        self.fade_to(1.0, fade, false, generation);
    }

    /// Changes the panel's presence and applies the capture exclusion it now calls for.
    fn update_presence<R>(&self, change: impl FnOnce(&mut Presence) -> R) -> R {
        let mut presence = self.presence.get();
        let result = change(&mut presence);
        self.presence.set(presence);
        set_sharing(&self.panel, presence.excluded());
        result
    }

    /// Animates the panel's alpha to `alpha` over `duration`, replacing any fade in flight.
    /// A fade to zero is a hide: when it finishes with nothing after it, the panel is hidden,
    /// drops its capture exclusion and, if `order_out`, orders out.
    fn fade_to(&self, alpha: f64, duration: Duration, order_out: bool, generation: u64) {
        use objc2::runtime::{AnyClass, AnyObject};
        let hiding = alpha == 0.0;
        let fading = !duration.is_zero();
        self.revealing.set(fading);
        // SAFETY: AppKit calls on the main thread; the completion handler runs there too.
        unsafe {
            if let Some(context) = AnyClass::get("NSAnimationContext") {
                let _: () = msg_send![context, beginGrouping];
                let current: *mut AnyObject = msg_send![context, currentContext];
                let _: () = msg_send![current, setDuration: duration.as_secs_f64()];
                if fading {
                    let (panel, presence, flag) = (self.panel.clone(), self.presence.clone(), self.revealing.clone());
                    let done = block2::RcBlock::new(move || {
                        let mut now = presence.get();
                        if !now.fade_done(generation, hiding) { return; }
                        presence.set(now);
                        flag.set(false);
                        if order_out { panel.orderOut(None) }
                        if hiding { set_sharing(&panel, now.excluded()); }
                    });
                    let _: () = msg_send![current, setCompletionHandler: &*done];
                }
                let animator: *mut AnyObject = msg_send![&*self.panel, animator];
                let _: () = msg_send![animator, setAlphaValue: alpha];
                let _: () = msg_send![context, endGrouping];
            }
            if duration.is_zero() {
                self.panel.setAlphaValue(alpha);
                if order_out { self.panel.orderOut(None); }
            }
        }
    }

    pub fn prewarm(&self) -> Prewarm {
        debug_assert!(self.options.prewarm);
        unsafe { self.panel.setAlphaValue(0.0); }
        self.panel.setIgnoresMouseEvents(true);
        if !self.panel.isVisible() { unsafe { self.panel.orderFrontRegardless() }; }
        self.warming.set(true);
        Prewarm(self.warming.clone())
    }

    /// Hides the panel, fading its alpha out over `fade` (instantly when zero). Input stops at once.
    pub fn hide(&self, fade: Duration) {
        self.visible.set(false);
        self.revealing.set(false);
        self.pointer_poll.set(None);
        self.monitors.borrow_mut().take();
        self.pointer.captured.set(false);
        self.pointer.pending.set(None);
        self.pointer.last.set(None);
        self.pointer.inside.set(false);
        self.pointer.clicks.borrow_mut().clear();
        self.pointer.release_since.set(None);
        self.pointer.recovered_release.set(None);
        self.set_cursor(None);
        // Clicks and scrolls go to the window below for the whole fade; `show` restores
        // hit-testing through `pointer.observe`.
        self.panel.setIgnoresMouseEvents(true);
        // Shown until the fade-out finishes, so the exclusion holds while it can still be seen.
        let mut presence = self.presence.get();
        let generation = presence.hide(!fade.is_zero());
        self.presence.set(presence);
        if self.options.prewarm {
            self.fade_to(0.0, fade, false, generation);
            unsafe { self.panel.resignKeyWindow(); }
        } else if fade.is_zero() {
            self.panel.orderOut(None);
        } else {
            unsafe { self.panel.resignKeyWindow(); }
            self.fade_to(0.0, fade, true, generation);
        }
        // Read-only once its alpha is zero: a hidden prewarm panel stays on every Space.
        set_sharing(&self.panel, presence.excluded());
    }

    /// `sharingType` none while shown, or read-only so recorders see the panel again. Hidden,
    /// the panel stays read-only and the request applies at its next show.
    pub fn set_capture_exclusion(&self, exclude: bool) -> crate::OverlaySupport {
        self.update_presence(|p| p.request(exclude));
        self.capture_exclusion()
    }
}
