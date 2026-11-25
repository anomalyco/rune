//! Switching the families behind the sans and mono slots while the app runs.

use super::*;

impl RuntimeCore {
    /// Put `sans` and `mono` behind the `Family::Sans` and `Family::Mono` slots, as
    /// `Tx::set_font_families` does from a handler. Every runtime on this thread follows on its
    /// next frame (all of an app's windows and overlays), and windows opened later start in
    /// them. This one drops what it shaped and measured in the old families now. Asking for the
    /// families it already has costs nothing.
    pub fn set_font_families(&mut self, sans: &str, mono: &str) {
        crate::text::choose_families(sans, mono);
        self.follow_font_families();
    }

    /// Registers font bytes (TTF, OTF, TTC, or variable font) into the process-wide font
    /// collection at runtime, returning the registered family names in registration order, as
    /// `Tx::add_font` does from a handler. When new faces are added, this runtime invalidates its
    /// shaped layouts, memoized subtrees, and layout measurements immediately; every other window
    /// and overlay follows on its next frame, and `rune::text::measure` follows immediately.
    ///
    /// Empty slices, blobs larger than 64 MiB, and unrecognised font data return an empty list
    /// without invalidating anything. Registering bytes that are already registered returns their
    /// family names without duplicating data or invalidating caches.
    pub fn add_font(&mut self, bytes: impl Into<std::sync::Arc<[u8]>>) -> Vec<String> {
        let (families, added) = crate::text::full_faces::add_font_checked(bytes.into());
        if added {
            self.follow_font_families();
        }
        families
    }

    /// Sets or clears the opt-in preferred emoji font family across every runtime on this thread
    /// (and windows opened later), as `Tx::set_emoji_family` does from a handler. Passing `None`
    /// or an empty string restores default system emoji fallback. Asking for the value already
    /// active costs nothing.
    pub fn set_emoji_family(&mut self, family: Option<&str>) {
        crate::text::choose_emoji_family(family);
        self.follow_font_families();
    }

    /// The sans and mono slots' families.
    pub fn font_families(&self) -> (&str, &str) {
        self.text.families()
    }

    /// The opt-in preferred emoji font family, or `None` when using default emoji fallback.
    pub fn emoji_family(&self) -> Option<&str> {
        self.text.emoji_family()
    }

    /// Whether `tx` switches the families, changes the preferred emoji family, or registers new
    /// runtime font faces.
    pub(super) fn changes_families(&self, tx: &Tx) -> bool {
        tx.fonts_added
            || tx.font_families.as_ref().is_some_and(|(sans, mono)| self.text.families() != (sans.as_str(), mono.as_str()))
            || tx.emoji_family.as_ref().is_some_and(|emoji| self.text.emoji_family() != emoji.as_deref())
    }

    /// Takes up runtime-added faces (including a web app's fetched faces) and families chosen
    /// since this runtime last looked. On a change, everything sized in the old fonts rebuilds on
    /// the next commit: memoized subtrees and every measured leaf's layout result. The text system
    /// has already dropped its caches and bumped its shaped epoch so stale `ParagraphLayout`s are
    /// not reused, while the presented tree keeps its `text_layout`s intact until the commit
    /// replaces them so pre-commit reads (`Tx::read_elements`, input, a11y, flights) stay
    /// consistent. True when the next frame must build.
    pub(crate) fn follow_font_families(&mut self) -> bool {
        let faces = self.text.follow_full_faces();
        let chosen = self.text.follow_chosen();
        if !(faces || chosen) {
            return false;
        }
        self.commit.clear_memo();
        self.commit.layout.mark_all_dirty();
        self.commit.dirty = true;
        self.commit.relayout = true;
        true
    }
}
