//! Turning text selection off for a whole subtree or window
//! ([`El::selection_enabled`](crate::El::selection_enabled)), with a way back in for the values
//! under it that people should still be able to copy.

use super::*;

impl El {
    /// Whether the [`El::selectable`] texts in this element and everything under it select,
    /// as SwiftUI's `.textSelection(.disabled)` does for a view's subtree. `false` turns them
    /// off: no I-beam, drag, double- or triple-click, Cmd+A (Ctrl+A elsewhere),
    /// [`Tx::select_text`] or [`Tx::select_all_text`], and no copy. They leave any
    /// [`El::selection_group`] they sit in, and a selection holding one of them clears at the
    /// next commit. `true` under an ancestor's `false` turns them back on for this subtree. A
    /// subtree that sets neither follows its nearest ancestor that did, and selectable texts
    /// select when none did. Unbuilt `virtual_rows` follow it too.
    ///
    /// It doesn't make text selectable by itself: texts still ask with [`El::selectable`]. One
    /// app setting reaches every text by setting this on the view's root:
    ///
    /// ```
    /// use rune::{El, col, text};
    ///
    /// let enabled = false;
    /// let _view: El = col()
    ///     .selection_enabled(enabled)
    ///     .child(text("A message body").selectable(true))
    ///     // An id people copy stays selectable whatever the setting says.
    ///     .child(text("ID 4f2a").selectable(true).selection_enabled(true));
    /// ```
    ///
    /// A text it turns off drops the [`Cursor::Text`] that [`El::selectable`] gave it; a cursor
    /// set with [`El::cursor`] stays. An [`El::selection_group`] with [`El::caret_browsing`]
    /// inside a `false` subtree doesn't take focus, since nothing in it can be selected.
    pub fn selection_enabled(mut self, on: bool) -> Self {
        self.cold_mut().selection_enabled = Some(on);
        self
    }
}

/// Whether an element with `own` ([`El::selection_enabled`]) under a parent with `parent_off`
/// sits in a subtree whose selection is off.
#[inline(always)]
pub(crate) fn selection_off(own: Option<bool>, parent_off: bool) -> bool {
    own.map_or(parent_off, |on| !on)
}

/// Turns off `text`'s selection inside an off subtree, and the I-beam [`El::selectable`] gave
/// it. It writes the text only when it was selectable: an element built fresh each commit owns
/// its text, so only a memo's text flattened again copies it, as a redacted text does.
pub(crate) fn apply_selection_off(
    text: &mut Option<crate::cold::SharedBox<TextSpec>>,
    handlers: &mut crate::cold::Cold<Handlers>,
) {
    let Some(t) = text else { return };
    if !t.selectable { return; }
    let t: &mut TextSpec = t;
    t.selectable = false;
    if t.flags.has(crate::el::text_spec::TextFlags::SELECTABLE_CURSOR) && handlers.cursor == Some(Cursor::Text) {
        handlers.cursor = None;
    }
}
