//! Switching the families behind the sans and mono slots while the app runs.

use super::*;

impl<S: 'static> Runtime<S> {
    /// Put `sans` and `mono` behind the `Family::Sans` and `Family::Mono` slots, as
    /// `Tx::set_font_families` does from a handler. Every runtime on this thread follows on its
    /// next frame (all of an app's windows and overlays), and windows opened later start in
    /// them. This one drops what it shaped and measured in the old families now. Asking for the
    /// families it already has costs nothing.
    pub fn set_font_families(&mut self, sans: &str, mono: &str) {
        crate::text::choose_families(sans, mono);
        self.follow_font_families();
    }

    /// The sans and mono slots' families.
    pub fn font_families(&self) -> (&str, &str) {
        self.text.families()
    }

    /// Whether `tx` switches the families.
    pub(super) fn changes_families(&self, tx: &Tx) -> bool {
        tx.font_families.as_ref().is_some_and(|(sans, mono)| self.text.families() != (sans.as_str(), mono.as_str()))
    }

    /// Takes up families chosen since this runtime last looked. On a change, everything sized
    /// in the old families goes: memoized subtrees, and every measured leaf's layout result.
    /// The text system has already dropped its layouts. True when the next frame must build.
    pub(crate) fn follow_font_families(&mut self) -> bool {
        if !self.text.follow_chosen() {
            return false;
        }
        self.memo.clear();
        for e in self.layout_entries.values().filter(|e| e.measure.is_some()) {
            let _ = self.layout.mark_dirty(e.id);
        }
        self.dirty = true;
        self.relayout = true;
        true
    }
}
