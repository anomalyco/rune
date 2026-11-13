//! Windows: whether other windows cover a window, found when it's about to draw
//! (`platform::win32::occlusion::look`) rather than heard from every app's window events. A
//! frame looks when the last look is older than `FRESH`, or the window's own move, size, show,
//! focus or input made it stale; the pointer reaching it counts as seen. Covered, it runs no motion
//! (`Cover::Windows`), and a frame asked for draws settled once the changes go quiet
//! (`CoveredDraw`), so what the window shows the moment it's uncovered is current. Only motion
//! falls behind, so only a covered window with a loop on screen looks again, every `AGAIN`, until
//! it's seen. Idle, covered or not, nothing wakes it.

use std::time::Duration;

use crate::time::Instant;

/// How long a look holds while the window draws.
pub(super) const FRESH: Duration = Duration::from_millis(250);
/// How often a covered window with motion on screen looks again.
pub(super) const AGAIN: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Look {
    /// Nothing known since the window last moved, showed, took focus or heard input.
    #[default]
    Due,
    /// Seen at `at`.
    Seen { at: Instant },
    /// Covered. `again`: when to look again, while motion on screen would be out of date.
    Covered { again: Option<Instant> },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum LookEvent {
    /// A frame is about to draw.
    Frame { now: Instant },
    /// A look found the window seen or not; `moving`: motion is on screen.
    Found { seen: bool, moving: bool, now: Instant },
    /// The window's own event: the next frame looks.
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LookEffect {
    None,
    /// Look now (`occlusion::look`), then step `Found`.
    Look,
}

impl Look {
    pub(super) fn step(self, event: LookEvent) -> (Look, LookEffect) {
        match (self, event) {
            (Look::Seen { at }, LookEvent::Frame { now }) if now < at + FRESH => (self, LookEffect::None),
            (_, LookEvent::Frame { .. }) => (self, LookEffect::Look),
            (_, LookEvent::Found { seen: true, now, .. }) => (Look::Seen { at: now }, LookEffect::None),
            (_, LookEvent::Found { seen: false, moving, now }) => (Look::Covered { again: moving.then(|| now + AGAIN) }, LookEffect::None),
            (_, LookEvent::Stale) => (Look::Due, LookEffect::None),
        }
    }

    /// When a covered window looks again.
    pub(super) fn deadline(self) -> Option<Instant> {
        match self {
            Look::Covered { again } => again,
            Look::Due | Look::Seen { .. } => None,
        }
    }

}

impl<S: 'static> super::Host<S> {
    /// Before a frame draws: looks whether other windows cover the window when the last look is
    /// stale, and covers or uncovers it (`Cover::Windows`).
    pub(super) fn look(&mut self) {
        let Some(w) = self.win.as_mut() else { return };
        let now = Instant::now();
        let (look, effect) = w.native.look.step(LookEvent::Frame { now });
        w.native.look = look;
        if effect == LookEffect::None { return; }
        let seen = crate::platform::win32::occlusion::look(&w.window) == crate::platform::win32::occlusion::Found::Seen;
        let moving = self.rt.loops_shown();
        self.look_found(seen, moving, now);
    }

    /// The window's own event: its next frame looks.
    pub(super) fn look_stale(&mut self) {
        if let Some(w) = self.win.as_mut() { w.native.look = w.native.look.step(LookEvent::Stale).0; }
    }

    /// The pointer reached the window, so some of it shows: no look needed.
    pub(super) fn sighted(&mut self) {
        if self.win.is_none() { return; }
        self.look_found(true, false, Instant::now());
    }

    fn look_found(&mut self, seen: bool, moving: bool, now: Instant) {
        let Some(w) = self.win.as_mut() else { return };
        w.native.look = w.native.look.step(LookEvent::Found { seen, moving, now }).0;
        if self.life.covered_by(super::Cover::Windows) != seen { return; }
        // Covered mid-move: land, so the window comes back at its new place.
        if !seen { self.land_frame(); }
        self.set_cover(super::Cover::Windows, !seen);
    }
}
