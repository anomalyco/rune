//! Password fields: the secure setting, the reveal toggle, and how a masked field maps model
//! bytes to the bullets it draws.

use super::*;

/// Fixed mask shown by a hidden multi-line secure field when non-empty, hiding both byte length
/// and line count.
pub(crate) const MULTILINE_SECURE_MASK: &str = "••••••••••••";

/// Mirrors whether the bound [`text_input`](crate::text_input) has [`El::single_line`](crate::El::single_line).
/// Before a field mounts, standalone [`TextEdit::secure`] defaults to single-line.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum LineMode {
    #[default]
    Default,
    Single,
    Multi,
}

/// How a field draws its text, read from its secure setting, its reveal toggle and its line
/// mode. The toggle is kept on a plain field too, so `.revealed(true).secure()` reveals in
/// either order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Masking {
    /// Not secure: the text as typed.
    Plain,
    /// Secure and revealed: the plain characters, with every other secure protection kept.
    Shown,
    /// Secure and hidden: one bullet per character on one line, or a fixed mask over several.
    Hidden { multiline: bool },
}

impl TextEdit {
    pub(crate) fn masking(&self) -> Masking {
        match (self.secure, self.revealed) {
            (false, _) => Masking::Plain,
            (true, true) => Masking::Shown,
            (true, false) => Masking::Hidden { multiline: !self.is_single_line() },
        }
    }

    /// The text draws as bullets, which caret motion treats as one word on one line.
    pub(crate) fn is_masked(&self) -> bool {
        matches!(self.masking(), Masking::Hidden { .. })
    }

    /// Password editing: masked display, no undo, copy, cut or IME. All model buffers are
    /// wiped on drop; secure replacements also wipe the previous allocation before freeing it.
    pub fn secure(mut self) -> Self { self.set_secure(true); self }

    pub fn is_secure(&self) -> bool { self.secure }

    pub fn set_secure(&mut self, secure: bool) {
        self.secure = secure;
        if secure {
            self.history = Arc::default();
            if self.group.is_open() {
                self.group = Group::Unrecorded;
            }
            self.marks.clear();
            self.blocks.clear();
            self.soft_breaks.clear();
            self.typing = Typing::Inherited;
            self.cancel_composition();
        } else if let Group::Unrecorded = self.group {
            // A group opened while secure starts recording from here.
            self.sync_invariants();
            self.group = Group::Open(self.snapshot_now());
        }
    }

    /// Show the plain characters of a [`secure`](Self::secure) field while keeping all other
    /// secure protections: no undo, copy, cut, IME or accessibility text, redacted [`Debug`],
    /// and zeroized buffers.
    /// Rune-owned source buffers are wiped; third-party shaping scratch used while revealed
    /// is released after shaping but is not guaranteed zeroized.
    pub fn revealed(mut self, revealed: bool) -> Self { self.set_revealed(revealed); self }

    pub fn is_revealed(&self) -> bool { self.revealed }

    pub fn set_revealed(&mut self, revealed: bool) { self.revealed = revealed; }

    pub(crate) fn is_single_line(&self) -> bool {
        match self.line_mode {
            LineMode::Default => self.secure,
            LineMode::Single => true,
            LineMode::Multi => false,
        }
    }

    pub(crate) fn set_single_line(&mut self, single_line: bool) {
        self.line_mode = if single_line { LineMode::Single } else { LineMode::Multi };
    }

    /// Whether this is a multi-line secure field in hidden mode (`secure && !revealed && !single_line`).
    pub(crate) fn is_hidden_multiline(&self) -> bool {
        self.masking() == Masking::Hidden { multiline: true }
    }

    /// The text a secure field draws: its mask while hidden, its characters while revealed.
    pub(super) fn masked_display(&self) -> String {
        match self.masking() {
            Masking::Hidden { multiline: false } => "•".repeat(self.text.chars().count()),
            Masking::Hidden { multiline: true } if !self.text.is_empty() => MULTILINE_SECURE_MASK.to_owned(),
            Masking::Hidden { multiline: true } => String::new(),
            Masking::Plain | Masking::Shown => self.text.clone(),
        }
    }

    /// For a secure field that has started exiting: wipe the secret and keep only its mask, so
    /// the field paints the same dots (no placeholder) through the exit with no plaintext.
    pub(crate) fn conceal(&mut self) {
        if !self.secure { return; }
        self.revealed = false;
        let mask = self.masked_display();
        self.replace_text(mask);
    }

    /// Map model bytes to the bullet layout's bytes and back. Each Unicode scalar gets one
    /// bullet in single-line mode so UTF-8 length never changes caret/selection alignment;
    /// hidden multi-line fields pin the visual caret to the end of the fixed mask and map
    /// pointer hits to the end of the model.
    pub(crate) fn display_index(&self, byte: usize) -> usize {
        match self.masking() {
            Masking::Hidden { multiline: true } => if self.text.is_empty() { 0 } else { MULTILINE_SECURE_MASK.len() },
            Masking::Hidden { multiline: false } => self.text[..byte.min(self.text.len())].chars().count() * '•'.len_utf8(),
            Masking::Plain | Masking::Shown => byte.min(self.text.len()),
        }
    }
    pub(crate) fn model_index(&self, byte: usize) -> usize {
        match self.masking() {
            Masking::Hidden { multiline: true } => self.text.len(),
            Masking::Hidden { multiline: false } => {
                self.text.char_indices().nth(byte / '•'.len_utf8()).map_or(self.text.len(), |(i, _)| i)
            }
            Masking::Plain | Masking::Shown => byte.min(self.text.len()),
        }
    }
    pub(crate) fn display_selection_range(&self, a: usize, b: usize) -> (usize, usize) {
        if self.is_hidden_multiline() {
            if a == b || self.text.is_empty() { (0, 0) } else { (0, MULTILINE_SECURE_MASK.len()) }
        } else {
            (self.display_index(a), self.display_index(b))
        }
    }

    // A fixed mask has no visible interior positions. Preserve the revealed selection until
    // an edit occurs, then treat it as either the whole secret or an insertion at its end.
    pub(super) fn normalize_hidden_selection(&mut self) {
        if self.is_hidden_multiline() {
            let selected = self.anchor != self.focus;
            self.focus = self.text.len();
            self.anchor = if selected { 0 } else { self.focus };
        }
    }
}

#[cfg(test)]
#[path = "secure_tests.rs"]
mod tests;
