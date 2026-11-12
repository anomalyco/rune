//! Where the arrows move a field's caret. Left and Right go by the text's logical order while it
//! runs only left to right, and across the line as it's drawn once it holds right-to-left runs,
//! as a browser's caret moves through mixed Arabic or Hebrew and Latin; a word back and forward
//! go by the text's order; Up and Down keep the column they started in.

use parley::{Affinity, Cursor, Layout};

use crate::text::Ink;

use super::{TextEdit, next_char, prev_char};

/// Where Up and Down aim: free, or the x (device px in the field's layout) a run of them keeps,
/// so a caret passing through a shorter line (or one of the other direction) comes back to its
/// column, as a browser's does. Any other key, an edit or a press lets it go.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Column {
    #[default]
    Free,
    Kept(f32),
}

impl Column {
    pub(crate) fn goal(self) -> Option<f32> {
        match self {
            Column::Free => None,
            Column::Kept(x) => Some(x),
        }
    }
}

/// A word back and forward in the text's order, by Unicode's word boundaries (UAX #29), as
/// browsers and Word step: `src/util/format.ts` stops at `src`, `util` and `format.ts`, a Persian
/// word joined by a zero-width non-joiner (`می‌شود`) is one word, and punctuation and spaces
/// between words are stepped over. A pill is one word.
impl TextEdit {
    pub(super) fn prev_word(&self, i: usize) -> usize {
        if self.is_masked() {
            return 0;
        }
        let mut j = i.min(self.text.len());
        loop {
            if let Some(start) = self.pill_before(j) {
                return start;
            }
            let Some((start, word)) = self.segment_before(j) else { return 0 };
            if !word {
                // A pill is one word: stepping back over the punctuation before it stops at
                // its end, and the step after that takes the pill whole.
                if self.pill_before(start).is_some() {
                    return start;
                }
                j = start;
                continue;
            }
            // A word runs back to a pill's end at most.
            let pill_end = self.marks.iter().filter(|m| m.mark.is_pill() && start < m.end && m.end < j).map(|m| m.end).max();
            return pill_end.unwrap_or(start);
        }
    }

    pub(super) fn next_word(&self, i: usize) -> usize {
        if self.is_masked() {
            return self.text.len();
        }
        let len = self.text.len();
        let mut j = i.min(len);
        // Off macOS a word forward ends at the next word's start, past the spaces and
        // punctuation after it, as Chromium's (and Windows') Ctrl+Right does; on macOS at the
        // word's end.
        if !cfg!(target_os = "macos") {
            match self.pill_after(j) {
                Some(end) => j = end,
                None => {
                    if let Some((end, true)) = self.segment_after(j) {
                        j = self.clip_to_pill_start(j, end);
                    }
                }
            }
            while j < len && self.pill_after(j).is_none() {
                match self.segment_after(j) {
                    Some((end, false)) => j = end,
                    _ => break,
                }
            }
            return j;
        }
        loop {
            if let Some(end) = self.pill_after(j) {
                return end;
            }
            let Some((end, word)) = self.segment_after(j) else { return len };
            if !word {
                j = end;
                continue;
            }
            return self.clip_to_pill_start(j, end);
        }
    }

    /// The word-boundary segment ending at byte `j` (UAX #29, within its line; a line break is a
    /// segment of its own): its start, and whether it's a word (holds a letter or digit).
    fn segment_before(&self, j: usize) -> Option<(usize, bool)> {
        use unicode_segmentation::UnicodeSegmentation;
        if j == 0 {
            return None;
        }
        let s = &self.text;
        let line = s[..j].rfind('\n').map_or(0, |k| k + 1);
        if line == j {
            return Some((prev_char(s, j), false));
        }
        let (at, seg) = s[line..j].split_word_bound_indices().next_back()?;
        Some((line + at, seg.chars().any(char::is_alphanumeric)))
    }

    /// The word-boundary segment starting at byte `j`: its end, and whether it's a word.
    fn segment_after(&self, j: usize) -> Option<(usize, bool)> {
        use unicode_segmentation::UnicodeSegmentation;
        let s = &self.text;
        if j >= s.len() {
            return None;
        }
        let line = s[j..].find('\n').map_or(s.len(), |k| j + k);
        if line == j {
            return Some((next_char(s, j), false));
        }
        let seg = s[j..line].split_word_bounds().next()?;
        Some((j + seg.len(), seg.chars().any(char::is_alphanumeric)))
    }

    /// `end`, or the start of a pill between `j` and it: a word runs up to a pill at most.
    fn clip_to_pill_start(&self, j: usize, end: usize) -> usize {
        self.marks.iter().filter(|m| m.mark.is_pill() && j < m.start && m.start < end).map(|m| m.start).min().unwrap_or(end)
    }
}
/// How the arrows move through a field's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CaretMotion {
    /// By character or word in the text's order (only left-to-right runs, or a hidden secret).
    Logical,
    /// Across the line as drawn: the text holds right-to-left runs.
    Visual,
}

impl CaretMotion {
    pub(super) fn of(layout: &Layout<Ink>, hidden: bool) -> CaretMotion {
        let bidi = layout.is_rtl() || layout.lines().any(|line| line.runs().any(|r| r.is_rtl()));
        if bidi && !hidden { CaretMotion::Visual } else { CaretMotion::Logical }
    }
}

/// Which way, and how far, one arrow press moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Arrow {
    Left,
    Right,
    WordLeft,
    WordRight,
}

/// Seeking the caret's next place across the drawn line. A caret between two runs has two
/// places, one per affinity: the seek tries the caret's own side, then the other when that one
/// is the line's end, and stops at the first that moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seek {
    /// Trying the caret's place on `side`; `then`, the other side, is still to try.
    Try { side: Affinity, then: Option<Affinity> },
    /// It moves to this display byte, on this side.
    Found(usize, Affinity),
    /// Neither place moves: the caret is at the line's edge that way.
    Edge,
}

/// What trying one place gave.
enum Tried {
    Moved(usize, Affinity),
    Stayed,
}

fn step(state: Seek, event: Tried) -> Seek {
    match (state, event) {
        (Seek::Try { .. }, Tried::Moved(j, side)) => Seek::Found(j, side),
        (Seek::Try { then: Some(side), .. }, Tried::Stayed) => Seek::Try { side, then: None },
        (Seek::Try { then: None, .. }, Tried::Stayed) => Seek::Edge,
        (done, _) => done,
    }
}

/// Where an arrow press lands across the drawn line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Landing {
    /// On this display byte, on this side of it (where two runs meet, the byte has a place at
    /// each).
    At(usize, Affinity),
    /// The caret is at the line's edge that way: it leaves the line the way its paragraph runs
    /// (left out of a right-to-left line goes on to the next line), or stays at the text's end.
    Edge,
}

/// Where `arrow` takes a caret at display byte `at` (on `side` of it) across `text` as drawn,
/// with the direction of the paragraph it's in; `None` where it moves in the text's order (a
/// paragraph of only left-to-right runs, or a `hidden` secret). A pill's isolate marks are
/// stops of their own that leave the caret on the same byte, so it steps on past them.
pub(super) fn visual_in(text: crate::text::TextFlow<'_>, at: usize, side: Affinity, arrow: Arrow, hidden: bool) -> Option<(Landing, crate::el::Flow)> {
    let p = text.piece_at(at);
    if CaretMotion::of(p.layout, hidden) == CaretMotion::Logical {
        return None;
    }
    let (mut from, mut side) = (p.shaped(at), side);
    for _ in 0..4 {
        match visual(p.layout, from, side, arrow) {
            Landing::At(j, s) if p.content(j) == at && j != from => (from, side) = (j, s),
            Landing::At(j, s) => return Some((Landing::At(p.content(j), s), p.flow)),
            Landing::Edge => return Some((Landing::Edge, p.flow)),
        }
    }
    Some((Landing::Edge, p.flow))
}

/// Whether the cluster just `right` (else left) of the caret at display byte `at` (on `side`
/// of it) is a space the text runs on into that way: in a left-to-right run to the right, in a
/// right-to-left run to the left. A word forward goes on past it off macOS.
pub(super) fn space_ahead(text: crate::text::TextFlow<'_>, at: usize, side: Affinity, right: bool) -> bool {
    let p = text.piece_at(at);
    let c = Cursor::from_byte_index(p.layout, p.shaped(at), side);
    c.visual_clusters(p.layout)[usize::from(right)].as_ref().is_some_and(|cl| cl.is_space_or_nbsp() && cl.is_rtl() != right)
}

/// Where `arrow` takes a caret at display byte `at`, on `side` of it, across `layout` as drawn.
fn visual(layout: &Layout<Ink>, at: usize, side: Affinity, arrow: Arrow) -> Landing {
    let other = match side { Affinity::Downstream => Affinity::Upstream, Affinity::Upstream => Affinity::Downstream };
    let mut state = Seek::Try { side, then: Some(other) };
    loop {
        state = match state {
            Seek::Try { side, .. } => {
                let c = Cursor::from_byte_index(layout, at, side);
                let to = match arrow {
                    Arrow::Left => c.previous_visual(layout),
                    Arrow::Right => c.next_visual(layout),
                    Arrow::WordLeft => c.previous_visual_word(layout),
                    Arrow::WordRight => c.next_visual_word(layout),
                };
                let moved = to.index() != at || to.geometry(layout, 0.0).x0 != c.geometry(layout, 0.0).x0;
                step(state, if moved { Tried::Moved(to.index(), to.affinity()) } else { Tried::Stayed })
            }
            Seek::Found(j, s) => return Landing::At(j, s),
            Seek::Edge => return Landing::Edge,
        };
    }
}