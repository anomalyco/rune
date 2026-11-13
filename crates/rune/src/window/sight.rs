//! Whether a shown window can be seen. The system says so (macOS's occlusion state, which
//! winit sends as `WindowEvent::Occluded`); a drawable refused as occluded says it until its
//! retry looks again; on Windows, other windows over the whole of it, minimising it or another
//! virtual desktop say it (`platform::win32::occlusion`, after Chromium's occlusion tracker).
//! Any one covers it, and it shows again once none does. A covered window draws nothing and
//! runs no motion: `poll`, due timers and queued input still run (`covered_step`), and
//! `App::on_visibility` hears it go and come back, as a page's `visibilitychange` does.

use std::num::NonZeroU8;

use super::*;

/// What says a shown window can't be seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cover {
    /// The system's occlusion notification.
    System = 1,
    /// A drawable refused as occluded, before any notification said so.
    Surface = 2,
    /// Windows: other windows cover it whole, it's minimised, or it's on another virtual desktop.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Windows = 4,
}

/// A shown window's sight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sight {
    Seen,
    /// What covers it: a set of `Cover`s, never empty.
    Covered(NonZeroU8),
}

impl Sight {
    pub(crate) fn covered(self) -> bool { matches!(self, Sight::Covered(_)) }

    /// Whether `cover` is among what covers it.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn has(self, cover: Cover) -> bool { matches!(self, Sight::Covered(set) if set.get() & cover as u8 != 0) }

    /// `cover` starts (`on`) or stops covering the window. A notification, either way,
    /// supersedes a refused drawable's guess, which only stood in for one.
    pub(crate) fn step(self, cover: Cover, on: bool) -> Sight {
        let mut bits = match self {
            Sight::Seen => 0,
            Sight::Covered(set) => set.get(),
        };
        if cover != Cover::Surface { bits &= !(Cover::Surface as u8); }
        bits = if on { bits | cover as u8 } else { bits & !(cover as u8) };
        NonZeroU8::new(bits).map_or(Sight::Seen, Sight::Covered)
    }
}

impl<S: 'static> Host<S> {
    /// `cover` starts or stops covering the window. Covered, it stops pacing and rests, and is
    /// no longer live; uncovered, its first frame paints the settled state of what changed
    /// meanwhile (guarantee 8).
    pub(super) fn set_cover(&mut self, cover: Cover, on: bool) {
        let Some(l) = self.win.as_mut() else { return };
        #[cfg(target_os = "macos")]
        { l.native.prime.stop(); }
        let was = self.life.cover(cover, on);
        l.backoff_step(BackoffEvent::Uncovered);
        l.presented = None;
        self.rt.set_live_visible(self.life.uncovered() && !l.minimized());
        if self.life.covered() {
            self.secure_input.set(false);
            l.stop_pacing();
            self.rt.cancel_timer_frame();
            self.idle = true;
            self.rest_step(RestEvent::Unseen);
            return;
        }
        // Its first frame uncovered draws what a covered one owed.
        #[cfg(target_os = "windows")]
        { l.covered_draw = l.covered_draw.step(CoveredDrawEvent::Drawn); }
        sync_chrome(&mut self.rt, l, self.chrome, self.titlebar);
        // A window opened while the displays slept has no display link yet.
        #[cfg(target_os = "macos")]
        if l.native.pacing.is_none() { l.native.pacing = crate::platform::display_link::DisplayLink::new(&l.window, self.frame_wake.clone()); }
        if was {
            self.rt.settle_all();
            l.request_redraw();
        } else if self.rt.stale() {
            l.request_redraw();
        }
    }
}
