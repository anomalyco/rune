//! Linux's primary selection, as Chromium keeps it on X11 and Wayland: text selected by the
//! pointer (drag, double/triple click, Shift+click) or keyboard (Shift+Arrows, Ctrl+A), in
//! selectable text, caret browsing, or a text field, goes on the primary selection once per
//! frame; a middle click in a text field places the caret there and pastes the primary
//! selection. A secure field's text never goes on it, and collapsing or blurring never clears
//! it. The Wayland `zwp_primary_selection_v1` backend and arboard fallback worker live in
//! `platform/wayland/primary_selection` and `clipboard/primary.rs`.
use std::hash::BuildHasher;

use super::*;

/// What the runtime asks of the primary selection, for the clipboard's flush to take.
#[derive(Default)]
pub(crate) struct PrimarySelection {
    /// A pointer release or keyboard selection changed what's selected: coalesced once per frame.
    pending: bool,
    /// A hash of the text last put on it, so a click that leaves the selection as it was
    /// writes nothing. Cleared by [`RuntimeCore::primary_selection_lost`] when another app
    /// takes primary selection.
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

    /// A keyboard selection (Shift+Arrows, Ctrl+A) changed what is selected.
    pub(in crate::runtime) fn primary_selection_changed(&mut self) {
        self.edit.primary.pending = true;
    }

    /// Called when the compositor cancels our owned primary-selection source so re-selecting
    /// identical text publishes it again.
    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn primary_selection_lost(&mut self) {
        self.edit.primary.written = None;
    }

    /// Once per frame, after pointer releases and keyboard selection updates have landed: puts
    /// what's selected on the primary selection, if non-empty and changed.
    pub(in crate::runtime) fn note_primary_selection(&mut self, app: &mut dyn AppDriver) {
        if !std::mem::take(&mut self.edit.primary.pending) { return; }
        let focused_secure = self.focused_index().is_some_and(|i| {
            std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent).any(|k| self.tree.nodes[k].is_secure())
        });
        let focused = self.focused_input().map(|i| (i.edit.is_secure(), i.access.clone(), i.pill_copy_text.clone()));
        let text = match focused {
            Some((true, _, _)) => None,
            Some((_, _, _)) if focused_secure => None,
            Some((_, Some(access), pill_copy_text)) => {
                let edit = access(app.states());
                if edit.is_secure() {
                    None
                } else {
                    let text = match &pill_copy_text {
                        Some(f) => edit.selected_with_pills(|id| f(id)),
                        None => edit.selected().to_owned(),
                    };
                    (!text.is_empty()).then_some(text)
                }
            }
            Some((_, None, pill_copy_text)) => {
                let input = self.focused_input().unwrap();
                let text = match &pill_copy_text {
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
