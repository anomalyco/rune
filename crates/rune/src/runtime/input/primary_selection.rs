//! Linux's primary selection, as Chromium keeps it on X11 and Wayland: the text a pointer
//! selects (a drag, a double or triple click, Shift+click), in selectable text or a text field,
//! goes on the primary selection when the button comes up; a middle click in a text field puts
//! the caret where it landed and pastes the primary selection there. A secure field's text
//! never goes on it. The clipboard's worker beside it writes and reads it
//! (`clipboard/primary.rs`), so Cmd/Ctrl+C and Cmd/Ctrl+V never see it.
use std::hash::BuildHasher;

use super::*;

/// What the runtime asks of the primary selection, for the clipboard's flush to take.
#[derive(Default)]
pub(crate) struct PrimarySelection {
    /// A primary button came up: after this frame's handlers, what's selected goes on it.
    pending: bool,
    /// A hash of the text last put on it, so a click that leaves the selection as it was
    /// writes nothing.
    written: Option<u64>,
    /// Text to put on it.
    pub(crate) write: Option<String>,
    /// The text field a middle click pastes it into.
    pub(crate) paste: Option<String>,
}

impl<S: 'static> Runtime<S> {
    /// The primary button came up.
    pub(super) fn primary_released(&mut self) {
        self.primary.pending = true;
    }

    /// After a frame's handlers, once the release's caret and selection landed: puts what's
    /// selected on the primary selection, if it changed.
    pub(super) fn note_primary_selection(&mut self) {
        if !std::mem::take(&mut self.primary.pending) { return; }
        let text = match self.focused_input() {
            Some(input) if input.edit.is_secure() => None,
            Some(input) => Some(input.edit.selected().to_owned()).filter(|t| !t.is_empty()),
            None => self.selected_text(),
        };
        let Some(text) = text else { return };
        let hash = std::hash::BuildHasherDefault::<std::hash::DefaultHasher>::default().hash_one(&text);
        if self.primary.written == Some(hash) { return; }
        self.primary.written = Some(hash);
        self.primary.write = Some(text);
    }

    /// A middle press at (x, y): on a text field that takes text, it focuses the field with its
    /// caret there and asks for the primary selection to paste. True when it did.
    pub(super) fn primary_paste_down(&mut self, x: f32, y: f32) -> bool {
        let Some(i) = self.primary_target(x, y).filter(|&i| self.allowed(i) && !self.subtree_exiting(i)) else { return false };
        let takes = self.nodes[i].input.as_ref().is_some_and(|input| input.access.is_some() && !input.edit.is_secure());
        if !takes { return false; }
        self.place_caret(i, x, y);
        self.primary.paste = Some(self.nodes[i].path.clone());
        true
    }
}
