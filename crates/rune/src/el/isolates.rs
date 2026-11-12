//! `El::isolate`: a run of a static text that keeps its own order whatever text is around it, as
//! HTML's `<bdi>` and CSS's `unicode-bidi: isolate` keep a name, a path or a command whole.
use std::ops::Range;

use super::*;

/// UTF-8 length of each isolate mark (LRI, RLI, FSI and PDI are all three bytes).
const MARK: u32 = 3;

impl El {
    /// Lays bytes `range` of a [`text`](crate::text) or [`rich`](crate::rich) element out as an
    /// isolate (UAX #9 LRI, RLI or FSI .. PDI): it reads in `dir` (`Auto` from its own first
    /// strong character) and the text around it reads as if it were one neutral character, so
    /// `src/app.ts`, `-v` or `npm install` in an Arabic sentence keeps its own order, and an
    /// Arabic name in an English one doesn't take the punctuation after it. A paragraph's own
    /// direction skips it. The marks draw nothing and copying leaves them out.
    ///
    /// Ranges are authored UTF-8 bytes on char boundaries, as spans and links are; an empty or
    /// out-of-range one, or one that crosses another isolate, is skipped (nest none). A pill
    /// ([`El::pill`]) is an `Auto` isolate unless one is set over it. On a field this does
    /// nothing: a field isolates its own pills.
    ///
    /// ```ignore
    /// rich(text, spans).isolate(code.clone(), Direction::Ltr)
    /// ```
    pub fn isolate(mut self, range: Range<usize>, dir: Direction) -> Self {
        if self.cold.input.is_some() {
            return self;
        }
        let Some(t) = &mut self.text else { return self };
        let ok = range.start < range.end && range.end <= t.content.len() && t.content.is_char_boundary(range.start) && t.content.is_char_boundary(range.end);
        let (a, b) = (range.start as u32, range.end as u32);
        if !ok || t.cold.isolates.iter().any(|&(s, e, _)| s < b && a < e) {
            return self;
        }
        let at = t.cold.isolates.partition_point(|r| r.0 < a);
        t.cold.isolates.insert(at, (a, b, dir));
        t.shape = None;
        self
    }
}

impl TextSpec {
    /// Writes each isolate's marks into `content` (its opening mark before it, PDI after), moves
    /// every byte range onto the marked text, and leaves the marks out of copies. Pills without
    /// an isolate over them become `Auto` isolates. Once applied there are none left to apply.
    pub(crate) fn apply_isolates(&mut self) {
        if self.redacted || self.cold.advance_in.is_some() || (self.cold.isolates.is_empty() && self.cold.pills.iter().all(|p| p.bare.is_some())) {
            return;
        }
        let mut isolates = std::mem::take(&mut self.cold.isolates);
        // A text laid out again keeps the marks it has.
        let marked_pill = |p: &crate::text::PillChip| {
            self.content[..p.start as usize].ends_with(['\u{2066}', '\u{2067}', '\u{2068}']) && self.content[p.end as usize..].starts_with('\u{2069}')
        };
        for p in self.cold.pills.iter().filter(|p| p.bare.is_none() && !marked_pill(p)) {
            if !isolates.iter().any(|&(a, b, _)| a < p.end && p.start < b) {
                let at = isolates.partition_point(|r| r.0 < p.start);
                isolates.insert(at, (p.start, p.end, Direction::Auto));
            }
        }
        if isolates.is_empty() {
            return;
        }
        let marked = Marks(&isolates);
        let mut content = String::with_capacity(self.content.len() + (2 * MARK as usize) * isolates.len());
        let mut at = 0;
        for &(a, b, dir) in &isolates {
            content.push_str(&self.content[at..a as usize]);
            content.push(match dir {
                Direction::Ltr => '\u{2066}',
                Direction::Rtl => '\u{2067}',
                Direction::Auto => '\u{2068}',
            });
            content.push_str(&self.content[a as usize..b as usize]);
            content.push('\u{2069}');
            at = b as usize;
        }
        content.push_str(&self.content[at..]);
        for s in &mut self.spans {
            (s.start, s.end) = marked.range(s.start, s.end);
        }
        let cold = &mut self.cold;
        for l in &mut cold.links {
            (l.start, l.end) = marked.range(l.start, l.end);
        }
        for r in &mut cold.uncopied {
            *r = marked.range(r.0, r.1);
        }
        for r in &mut cold.rings {
            (r.0, r.1) = marked.range(r.0, r.1);
        }
        for r in &mut cold.strikes {
            (r.0, r.1) = marked.range(r.0, r.1);
        }
        for h in &mut cold.highlights {
            (h.start, h.end) = marked.range(h.start, h.end);
        }
        for p in &mut cold.pills {
            (p.start, p.end) = marked.range(p.start, p.end);
        }
        for (k, &(a, b, _)) in isolates.iter().enumerate() {
            // Each mark sits after the marks of the isolates before this one.
            let before = 2 * MARK * k as u32;
            let at = cold.uncopied.partition_point(|r| r.0 < a + before);
            cold.uncopied.insert(at, (a + before, a + before + MARK));
            let at = cold.uncopied.partition_point(|r| r.0 < b + before + MARK);
            cold.uncopied.insert(at, (b + before + MARK, b + before + 2 * MARK));
        }
        self.content = content;
        self.shape = None;
    }
}

/// The sorted, disjoint isolates whose marks a text now holds, to move byte ranges with.
struct Marks<'a>(&'a [(u32, u32, Direction)]);

impl Marks<'_> {
    /// Authored range `start..end` in the marked text. A range inside an isolate stays inside
    /// its marks; one that holds an isolate's end holds its PDI, and one that starts at an
    /// isolate's start without ending inside it starts before the opening mark.
    fn range(&self, start: u32, end: u32) -> (u32, u32) {
        let shift = |p: u32, open_at_p: bool, close_at_p: bool| {
            let opens = self.0.iter().filter(|r| r.0 < p || (open_at_p && r.0 == p)).count() as u32;
            let closes = self.0.iter().filter(|r| r.1 < p || (close_at_p && r.1 == p)).count() as u32;
            p + MARK * (opens + closes)
        };
        let inside = |r: &&(u32, u32, Direction)| r.0 <= start && end <= r.1;
        let within = self.0.iter().find(inside);
        // A start at an isolate's start is inside it only when the range ends inside it; a
        // start at an isolate's end is past its PDI.
        let s = shift(start, within.is_some_and(|r| r.0 == start), true);
        // An end at an isolate's end takes its PDI unless the range lies inside the isolate; an
        // end at an isolate's start stops before its opening mark.
        let e = shift(end, false, within.is_none_or(|r| r.1 != end));
        (s, e.max(s))
    }
}
