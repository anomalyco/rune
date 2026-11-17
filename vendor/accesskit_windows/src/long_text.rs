// Rune (vendor/accesskit_windows.patch): a text field too long for its toolkit to publish text
// runs (Rune stops at 64 KiB) keeps its text ranges. The toolkit marks such a field by putting its
// selection on the field itself, as byte offsets into its value; ranges are then answered from the
// value when a reader asks, a line at a time, with no per-character data published up front. A
// line is what a line break ends. Off-screen geometry is not known, so ranges have no rectangles.
// The same file is in accesskit_windows and accesskit_macos; each uses part of it.
#![allow(dead_code)]

use accesskit::TextSelection;
use accesskit_consumer::NodeRef;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unit {
    Character,
    Word,
    Line,
    Document,
}

/// The value and selection a long field's text ranges are answered from.
pub(crate) fn long_text<'a>(node: &NodeRef<'a>) -> Option<(Text<'a>, &'a TextSelection)> {
    if !node.is_text_input() || node.is_password_input() || node.supports_text_ranges() {
        return None;
    }
    let data = node.data();
    let selection = data.text_selection()?;
    let (id, _) = node.locate();
    if selection.anchor.node != id || selection.focus.node != id {
        return None;
    }
    Some((Text(data.value().unwrap_or("")), selection))
}

fn utf16_len(s: &str) -> usize {
    if s.is_ascii() { s.len() } else { s.chars().map(char::len_utf16).sum() }
}

/// Byte offsets into a field's value.
#[derive(Clone, Copy)]
pub(crate) struct Text<'a>(pub(crate) &'a str);

impl<'a> Text<'a> {
    pub(crate) fn len(self) -> usize {
        self.0.len()
    }

    /// `i`, within the text and on a character's start.
    pub(crate) fn clamp(self, i: usize) -> usize {
        self.0.floor_char_boundary(i.min(self.0.len()))
    }

    pub(crate) fn slice(self, a: usize, b: usize) -> &'a str {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        &self.0[a..b]
    }

    fn line_start(self, i: usize) -> usize {
        self.0[..i].rfind('\n').map_or(0, |p| p + 1)
    }

    pub(crate) fn is_start(self, i: usize, unit: Unit) -> bool {
        let s = self.0;
        match unit {
            Unit::Character => true,
            Unit::Word => {
                i == 0
                    || (s[i..].chars().next().is_some_and(|c| !c.is_whitespace())
                        && s[..i].chars().next_back().is_some_and(char::is_whitespace))
            }
            Unit::Line => i == 0 || s.as_bytes()[i - 1] == b'\n',
            Unit::Document => i == 0,
        }
    }

    /// The start of the unit holding `i`.
    pub(crate) fn start_of(self, i: usize, unit: Unit) -> usize {
        if self.is_start(i, unit) { i } else { self.prev(i, unit) }
    }

    /// The first unit start after `i`, or the text's end.
    pub(crate) fn next(self, i: usize, unit: Unit) -> usize {
        let s = self.0;
        match unit {
            Unit::Character => s[i..].chars().next().map_or(i, |c| i + c.len_utf8()),
            Unit::Word => {
                let mut space = false;
                for (k, c) in s[i..].char_indices() {
                    if c.is_whitespace() {
                        space = true;
                    } else if space {
                        return i + k;
                    }
                }
                s.len()
            }
            Unit::Line => s[i..].find('\n').map_or(s.len(), |p| i + p + 1),
            Unit::Document => s.len(),
        }
    }

    /// The last unit start before `i`, or the text's start.
    pub(crate) fn prev(self, i: usize, unit: Unit) -> usize {
        let s = self.0;
        match unit {
            Unit::Character => s[..i].chars().next_back().map_or(0, |c| i - c.len_utf8()),
            Unit::Word => {
                let mut word = None;
                for (k, c) in s[..i].char_indices().rev() {
                    if !c.is_whitespace() {
                        word = Some(k);
                    } else if let Some(k) = word {
                        return k;
                    }
                }
                0
            }
            Unit::Line => {
                if i == 0 {
                    return 0;
                }
                let at = self.line_start(i);
                if at < i { at } else { self.line_start(i - 1) }
            }
            Unit::Document => 0,
        }
    }

    /// Moves `i` by `count` unit starts (a unit ends where the next starts), stopping at either
    /// end of the text. Returns where it got to and how many units it moved.
    pub(crate) fn step(self, mut i: usize, unit: Unit, count: i32) -> (usize, i32) {
        let mut moved = 0;
        for _ in 0..count.unsigned_abs() {
            if (count > 0 && i == self.len()) || (count < 0 && i == 0) {
                break;
            }
            i = if count > 0 { self.next(i, unit) } else { self.prev(i, unit) };
            moved += 1;
        }
        (i, if count < 0 { -moved } else { moved })
    }

    /// The zero-based line holding byte `i`.
    pub(crate) fn line_of(self, i: usize) -> usize {
        self.0.as_bytes()[..i].iter().filter(|&&b| b == b'\n').count()
    }

    /// Line `n`'s bytes, its line break included.
    pub(crate) fn line(self, n: usize) -> Option<(usize, usize)> {
        let start = if n == 0 { 0 } else { self.0.match_indices('\n').nth(n - 1)?.0 + 1 };
        Some((start, self.next(start, Unit::Line)))
    }

    pub(crate) fn utf16(self, i: usize) -> usize {
        utf16_len(&self.0[..i])
    }

    /// Bytes `a..b` as a UTF-16 start and length.
    pub(crate) fn utf16_span(self, a: usize, b: usize) -> (usize, usize) {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        (self.utf16(a), utf16_len(&self.0[a..b]))
    }

    pub(crate) fn from_utf16(self, units: usize) -> usize {
        if self.0.is_ascii() {
            return units.min(self.0.len());
        }
        let mut seen = 0;
        for (k, c) in self.0.char_indices() {
            if seen >= units {
                return k;
            }
            seen += c.len_utf16();
        }
        self.0.len()
    }
}
