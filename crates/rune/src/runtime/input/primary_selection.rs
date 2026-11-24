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

impl RuntimeCore {
    /// The primary button came up.
    pub(in crate::runtime) fn primary_released(&mut self) {
        self.edit.primary.pending = true;
    }

    /// After a frame's handlers, once the release's caret and selection landed: puts what's
    /// selected on the primary selection, if it changed.
    pub(in crate::runtime) fn note_primary_selection(&mut self, app: &dyn ViewDriver) {
        if !std::mem::take(&mut self.edit.primary.pending) { return; }
        let focused_secure = self.focused_index().is_some_and(|i| {
            std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent).any(|k| self.tree.nodes[k].is_secure())
        });
        let text = match self.focused_input() {
            Some(input) if focused_secure || input.edit.is_secure() => None,
            Some(input) => {
                let text = match &input.pill_copy_text {
                    Some(f) => input.edit.selected_with_pills(|id| f(id)),
                    None => input.edit.selected().to_owned(),
                };
                (!text.is_empty()).then_some(text)
            }
            None => self.selected_text(app),
        };
        let Some(text) = text else { return };
        let hash = std::hash::BuildHasherDefault::<std::hash::DefaultHasher>::default().hash_one(&text);
        if self.edit.primary.written == Some(hash) { return; }
        self.edit.primary.written = Some(hash);
        self.edit.primary.write = Some(text);
    }

    /// A middle press at (x, y): on a text field that takes text, it focuses the field with its
    /// caret there and asks for the primary selection to paste. True when it did.
    pub(in crate::runtime) fn primary_paste_down(&mut self, x: f32, y: f32) -> bool {
        let Some(i) = self.primary_target(x, y)
            .filter(|&i| self.allowed(i) && !self.subtree_exiting(i))
            .and_then(|i| self.textbox_input(i))
        else { return false };
        let blocked = std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent).any(|k| {
            let n = &self.tree.nodes[k];
            n.is_secure() || n.a11y.disabled || n.a11y.more.as_ref().is_some_and(|m| m.read_only)
        });
        if blocked { return false; }
        let takes = self.tree.nodes[i].input.as_ref().is_some_and(|input| input.access.is_some());
        if !takes { return false; }
        self.place_caret(i, x, y);
        self.edit.primary.paste = Some(self.tree.nodes[i].path.clone());
        true
    }
}

#[cfg(test)]
#[path = "primary_selection_tests.rs"]
mod tests;
