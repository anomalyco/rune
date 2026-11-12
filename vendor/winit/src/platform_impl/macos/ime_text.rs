//! Rune: what `Ime::Preedit` can't carry, beside it. The app's text around its selection, for
//! `selectedRange` and `attributedSubstringForProposedRange:` (reconversion), a composition's
//! clauses from `setMarkedText:`'s attributes, and the range a reconversion replaces.

use std::cell::RefCell;
use std::ops::Range;

thread_local! {
    /// The focused field's paragraph and its selection, UTF-8 bytes.
    pub(crate) static TEXT: RefCell<Option<(String, Range<usize>)>> = const { RefCell::new(None) };
    /// The last marked text and its clauses (byte ranges, whether each is being converted).
    pub(crate) static CLAUSES: RefCell<Option<(String, Vec<(Range<usize>, bool)>)>> = const { RefCell::new(None) };
    /// The range of `TEXT` a reconversion's marked text replaces, UTF-8 bytes.
    pub(crate) static REPLACEMENT: RefCell<Option<Range<usize>>> = const { RefCell::new(None) };
}

/// `utf16` UTF-16 units into `text` as a UTF-8 byte offset, clamped to its end.
pub(crate) fn byte_of(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= utf16 { return i; }
        units += c.len_utf16();
    }
    text.len()
}

/// UTF-8 byte `byte` of `text` in UTF-16 units.
pub(crate) fn utf16_of(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].encode_utf16().count()
}
