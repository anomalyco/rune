//! Windows: whether other windows cover a window, found when it's about to draw
//! (`platform::win32::occlusion::look`) rather than heard from every app's window events. A
//! frame looks when the last look is older than `FRESH`, or the window's own move, size, show,
//! focus or input made it stale; the pointer reaching it counts as seen. Covered, it runs no motion
//! (`Cover::Windows`), and a frame asked for draws settled once the changes go quiet
//! (`CoveredDraw`), so what the window shows the moment it's uncovered is current. Only motion
//! falls behind, so only a covered window with a loop on screen looks again, after `AGAIN` and then
//! less often (`AGAIN_MAX`), until it's seen. Idle, covered or not, nothing wakes it.

use std::time::Duration;

use crate::time::Instant;

/// How long a look holds while the window draws.
pub(super) const FRESH: Duration = Duration::from_millis(250);
/// How soon a covered window with motion on screen first looks again; each look that finds it
/// still covered doubles the wait, up to `AGAIN_MAX`. Windows says nothing when a window is
/// uncovered (no message, and on an Iris Xe DXGI's occlusion status stays clear for a windowed
/// flip-model swap chain, covered whole or minimised: a test present returns `S_OK`, the
/// occlusion-status event doesn't fire), so a spinner that
/// comes back into view resumes within that wait; what else it shows is current (`CoveredDraw`).
pub(super) const AGAIN: Duration = Duration::from_millis(250);
pub(super) const AGAIN_MAX: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Look {
    /// Nothing known since the window last moved, showed, took focus or heard input.
    #[default]
    Due,
    /// Seen at `at`.
    Seen { at: Instant },
    /// Covered. `again`: when to look again, and the wait it came after, while motion on screen
    /// would be out of date.
    Covered { again: Option<(Instant, Duration)> },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum LookEvent {
    /// A frame is about to draw, or a covered window's look again is due.
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
            (state, LookEvent::Found { seen: false, moving, now }) => {
                let wait = match state { Look::Covered { again: Some((_, wait)) } => (wait * 2).min(AGAIN_MAX), _ => AGAIN };
                (Look::Covered { again: moving.then(|| (now + wait, wait)) }, LookEffect::None)
            }
            (_, LookEvent::Stale) => (Look::Due, LookEffect::None),
        }
    }

    /// When a covered window looks again.
    pub(super) fn deadline(self) -> Option<Instant> {
        match self {
            Look::Covered { again } => again.map(|(at, _)| at),
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
