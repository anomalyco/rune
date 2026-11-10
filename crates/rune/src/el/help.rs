//! Tooltip text, rows and keycaps.

use super::El;

/// Parts a help text from each of its [`El::help_keys`] keycaps.
pub(crate) const HELP_KEY: char = '\u{1f}';

/// Parts a help text's rows ([`El::help_row`]).
pub(crate) const HELP_ROW: char = '\u{1e}';

/// A help text as words: each row's keycaps after it, a space apart, and its rows a line apart.
pub(crate) fn help_words(help: &str) -> std::borrow::Cow<'_, str> {
    if help.contains([HELP_KEY, HELP_ROW]) { help.replace(HELP_KEY, " ").replace(HELP_ROW, "\n").into() } else { help.into() }
}

impl El {
    /// A tooltip shown after the system's hover delay while the pointer rests on this element,
    /// as SwiftUI's `.help`. A press, scroll, key or the pointer leaving hides it; it takes no
    /// input itself. On macOS it is AppKit's own tooltip; elsewhere Rune draws it. The element
    /// tree (`Runtime::elements`, `--tree`) carries it as the element's help text.
    #[inline]
    pub fn help(mut self, text: impl Into<String>) -> Self {
        self.set_help(text);
        self
    }
    /// [`El::help`] in place.
    pub fn set_help(&mut self, text: impl Into<String>) -> &mut Self {
        let text = text.into();
        if !text.is_empty() || !self.cold.is_empty() {
            let cold = self.cold_mut();
            cold.help = (!text.is_empty()).then_some(text);
            cold.help_cut = false;
        }
        self
    }
    /// [`El::help`] that shows only while this text is cut short ([`truncate`](Self::truncate), `max_lines`
    /// or `truncate_fade`), as AppKit's expansion tooltips show a table cell's whole text.
    pub fn help_when_truncated(self, text: impl Into<String>) -> Self {
        let mut e = self.help(text);
        e.cold_mut().help_cut = true;
        e
    }
    /// Another row under this element's [`El::help`] tooltip, as a web tooltip lists two
    /// commands, each with its own keybind:
    /// `.help("Save").help_keys(["Enter"]).help_row("Send").help_keys(["Ctrl", "Enter"])`. Rows
    /// stack [`HelpLook::row_gap`](crate::HelpLook::row_gap) apart, each its words then its
    /// keycaps, from the tooltip's start edge. Call it after `help`, which replaces every row.
    /// The element tree and assistive technology read the rows a line apart. Without `help`
    /// first, it sets the first row.
    pub fn help_row(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        match self.cold.get_mut_if_some().and_then(|c| c.help.as_mut()) {
            Some(h) => {
                h.push(HELP_ROW);
                h.push_str(&text);
            }
            None => {
                self.set_help(text);
            }
        }
        self
    }
    /// Keycaps drawn after this element's [`El::help`] text, as a web tooltip shows its
    /// command's keybind: `.help("Toggle review").help_keys(["Ctrl", "Shift", "R"])`. Call it
    /// after `help`, which replaces them, or after a [`El::help_row`], for that row's keys. The
    /// element tree and assistive technology read them after the text, a space apart.
    pub fn help_keys<K: Into<String>>(mut self, keys: impl IntoIterator<Item = K>) -> Self {
        if let Some(h) = self.cold.get_mut_if_some().and_then(|c| c.help.as_mut()) {
            for k in keys.into_iter().map(Into::into).filter(|k| !k.is_empty()) {
                h.push(HELP_KEY);
                h.push_str(&k);
            }
        }
        self
    }
    /// Shows this element's [`El::help`] tooltip above it, 4 points clear, and below only when
    /// there's no room above, as a web page's `placement="top"` tooltip sits over its button.
    /// Short for `help_side(Side::Top)`.
    pub fn help_above(self) -> Self {
        self.help_side(crate::Side::Top)
    }
    /// Shows this element's [`El::help`] tooltip on `side` of it, as a web page's `placement`
    /// does: under it ([`HelpLook::gap`](crate::HelpLook::gap)'s first gap clear, the default),
    /// or above it, or left or right of it (its second gap clear), on the opposite side when
    /// there's no room on `side`. A tooltip under or above it is centred across it; one left or
    /// right of it, centred down it (`placement="right"`). Each stays inside the window.
    pub fn help_side(mut self, side: crate::Side) -> Self {
        self.cold_mut().help_side = (side != crate::Side::Bottom).then_some(side);
        self
    }
}
