//! Whether a panel counts as shown, and so whether its capture exclusion is applied.
//!
//! A prewarmed panel stays ordered in at alpha zero while hidden, so its show is instant. Panels
//! join every Space, and macOS blanks a Space's Mission Control thumbnail while a window with
//! `sharingType` none is on it, even a transparent one. So the exclusion an app requests is
//! applied only while the panel is shown: from `show` (before its alpha starts rising) until a
//! hide's fade-out finishes. A show during that fade bumps the generation, so the stale fade's
//! completion changes nothing.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Presence {
    /// Bumped by every show and hide, so a finished fade acts only if nothing came after it.
    generation: u64,
    shown: bool,
    requested: bool,
}

impl Presence {
    pub(crate) fn new(requested: bool) -> Self { Self { requested, ..Self::default() } }

    /// What the app asked for, shown or not.
    pub(crate) fn requested(self) -> bool { self.requested }

    /// Whether `sharingType` should be none now; read-only otherwise.
    pub(crate) fn excluded(self) -> bool { self.requested && self.shown }

    pub(crate) fn request(&mut self, exclude: bool) { self.requested = exclude; }

    /// A show starts. Returns its fade's generation.
    pub(crate) fn show(&mut self) -> u64 {
        self.shown = true;
        self.bump()
    }

    /// A hide starts. Without a fade it has finished; with one, `fade_done` finishes it.
    pub(crate) fn hide(&mut self, fading: bool) -> u64 {
        if !fading { self.shown = false; }
        self.bump()
    }

    /// The fade started as `generation` finished. False if a later show or hide replaced it.
    pub(crate) fn fade_done(&mut self, generation: u64, hiding: bool) -> bool {
        if generation != self.generation { return false; }
        if hiding { self.shown = false; }
        true
    }

    fn bump(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }
}

#[cfg(test)]
#[path = "presence_tests.rs"]
mod tests;
