//! When each window's next frame is asked for, and painted. A frame a window asks for while it
//! paints is asked for at the end of the loop's turn (`Windows::about_to_wait`); on Windows a
//! window whose asked-for paint the system keeps putting off is painted there.
//!
//! Windows makes up a `WM_PAINT` for one window at a time, the top one first, and winit ends its
//! message pass after each paint. A window in front that asks again inside its paint, or from a
//! timer or a wake each turn, is painted every turn, and one behind it, its frame asked for by
//! the same stream event, waited until the front one rested: a second and a half after a short
//! reply, the whole reply while one streams.

use crate::time::Instant;
use std::time::Duration;

/// How long a window's asked-for paint may wait while the loop turns before the loop paints it
/// itself: about a frame.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(super) const STARVE: Duration = Duration::from_millis(16);

/// One window's next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Redraw {
    /// No frame is on its way, and none is wanted.
    Idle,
    /// The system was asked for a paint at `at`, which hasn't come yet.
    Asked { at: Instant },
    /// The window paints, and nothing has asked for another frame since it started.
    Painting,
    /// The window paints, and wants the next frame.
    PaintingAgain,
    /// The window painted and wants the next frame, which the turn's end asks for.
    Wants,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum RedrawEvent {
    /// Something asks for a frame: an input, a wake, a timer, the frame that just drew.
    Ask(Instant),
    /// A paint starts.
    Paint,
    /// The paint ends.
    Painted,
    /// The loop's turn ends. `paints`: the window is on screen, and Windows puts its paint off
    /// behind the others'.
    TurnEnds { now: Instant, paints: bool },
}

/// What the window does for a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RedrawEffect {
    None,
    /// `Window::request_redraw`.
    Request,
    /// Paint now, and drop the system's paint still on its way.
    PaintNow,
}

impl Redraw {
    pub(super) fn step(self, event: RedrawEvent) -> (Redraw, RedrawEffect) {
        use {Redraw::*, RedrawEffect as E, RedrawEvent::*};
        match (self, event) {
            // Asked inside the paint: winit would ask the system again before the paint returns.
            (Painting | PaintingAgain, Ask(_)) => (PaintingAgain, E::None),
            // The first ask's time is how long it has waited.
            (Asked { at }, Ask(_)) => (Asked { at }, E::Request),
            (Idle | Wants, Ask(now)) => (Asked { at: now }, E::Request),
            (_, Paint) => (Painting, E::None),
            (Painting, Painted) => (Idle, E::None),
            (PaintingAgain, Painted) => (Wants, E::None),
            (state, Painted) => (state, E::None),
            (Wants, TurnEnds { now, .. }) => (Asked { at: now }, E::Request),
            (Asked { at }, TurnEnds { now, paints: true }) if now >= at + STARVE => (Painting, E::PaintNow),
            (state, TurnEnds { .. }) => (state, E::None),
        }
    }
}
