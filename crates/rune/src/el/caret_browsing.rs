//! [`El::caret_browsing`]: a keyboard caret in selectable text, as a browser's caret browsing
//! (F7) gives a page.
use super::El;

impl El {
    /// Gives this [`El::selectable`] text a keyboard caret once a click (or
    /// [`Tx::select_text`](crate::Tx::select_text)) places one in it, as a browser's caret
    /// browsing does and as GTK's read-only `TextView` with a visible cursor does. The caret
    /// draws in the text's colour while the selection is empty, and the keys move it while no
    /// text field has the keyboard:
    ///
    /// - Left and Right by character, and by word with Option (Ctrl elsewhere). Without Shift
    ///   they collapse a selection to its start or end.
    /// - Up and Down by line, keeping to the caret's column across wrapped lines.
    /// - Home and End to the visual line's edges (⌘Left and ⌘Right on macOS), and ⌘Home and
    ///   ⌘End (Ctrl elsewhere, or ⌘Up and ⌘Down on macOS) to the start and end of the text or
    ///   of its [`El::selection_group`].
    /// - Shift with any of them extends the selection instead.
    ///
    /// Inside a selection group the caret steps from one text into the next, so set this on each
    /// of its texts (or on those the caret should stop in); a text without it ignores the keys.
    /// Rows of an [`El::virtual_rows`] list are reached as they're mounted: each step scrolls
    /// the caret into view, which mounts the rows around it. Cmd+C (Ctrl+C) and Cmd+A (Ctrl+A)
    /// work as they do for any selectable text. Text cut short (`El::truncate`, `El::max_lines`)
    /// takes no caret. Off by default; it does nothing on text that isn't selectable.
    #[inline]
    pub fn caret_browsing(mut self, on: bool) -> Self {
        if let Some(t) = &mut self.text && t.cold.caret_browsing != on { t.cold.caret_browsing = on; }
        self
    }
}
