//! Bringing a window on screen. `AppHandle::show`, `AppHandle::open` (and `activate`) and a
//! launcher's cached reveal are one transition that differs only in what each asks for: focus,
//! restoring a minimised window, and whether the frame shown is already drawn.

use crate::machine::Effects;

/// What brings the window on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reveal {
    /// `AppHandle::show`: no focus, and a minimised window stays minimised.
    Show,
    /// `AppHandle::open` or `activate`: restores a minimised window and focuses it.
    Open,
    /// A launcher shows the frame it kept rendering while hidden, and takes the keys.
    Cached,
}

/// The window as a reveal finds it. `minimized` is the platform's answer, which on macOS
/// stays true into a deminiaturize (`Win::minimized`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Found {
    /// No native window: never created, released while hidden, or unmapped (Wayland).
    Missing,
    /// A window off screen: hidden, or not shown yet.
    Hidden { minimized: bool },
    Shown { covered: bool, minimized: bool },
}

/// What a reveal does, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RevealEffect {
    /// Re-reads the accent and scroller style, whose redraw a hidden window may have missed.
    ReadSystem,
    /// Builds what changed while the window was away and takes its new height before it is
    /// ordered in (`Host::build_for_show`).
    Build,
    /// Applies a placement request.
    Place,
    /// Creates the window, which comes on screen as it opens.
    Create,
    OrderIn { focus: bool },
    Unminimize,
    /// Paints the settled state of what moved while the window was away in its first frame
    /// (guarantee 8), unless `Build` already did.
    Settle,
    /// It's on screen and uncovered; `live` unless it stays minimised (`Runtime::set_live_visible`).
    Shown { live: bool },
    /// The frame a resting wake asked for won't stand for its poll.
    CancelTimerFrame,
    /// Publishes the native window number.
    Number,
    /// Focuses it, or on Wayland activates it with the open's token.
    Focus,
    Redraw,
    /// Asks for a frame only when the frame it reveals left motion running.
    RedrawIfMoving,
}

impl Reveal {
    pub(super) fn effects(self, found: Found) -> Effects<RevealEffect> {
        use RevealEffect::*;
        let mut out = Effects::none();
        let (hidden, covered, minimized) = match found {
            Found::Missing => {
                if self != Reveal::Cached { out.extend([ReadSystem, Create]); }
                return out;
            }
            Found::Hidden { minimized } => (true, false, minimized),
            Found::Shown { covered, minimized } => (false, covered, minimized),
        };
        if self == Reveal::Cached {
            out.extend([OrderIn { focus: true }, Shown { live: true }, CancelTimerFrame, Number, RedrawIfMoving]);
            return out;
        }
        let restores = self == Reveal::Open && minimized;
        out.push(ReadSystem);
        if hidden || restores { out.push(Build); }
        out.push(Place);
        if hidden { out.push(OrderIn { focus: false }); }
        if restores { out.push(Unminimize); }
        if hidden || covered || restores { out.push(Settle); }
        // A minimised window doesn't draw (on macOS occlusion says so instead).
        let stays_minimized = minimized && !restores && !cfg!(target_os = "macos");
        out.extend([Shown { live: !stays_minimized }, CancelTimerFrame, Number]);
        if self == Reveal::Open { out.push(Focus); }
        out.push(Redraw);
        out
    }
}

#[cfg(test)]
#[path = "reveal_tests.rs"]
mod tests;
