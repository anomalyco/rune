//! Where the arrows move a field's caret. Left and Right go by the text's logical order while it
//! runs only left to right, and across the line as it's drawn once it holds right-to-left runs,
//! as a browser's caret moves through mixed Arabic or Hebrew and Latin; a word back and forward
//! go by the text's order; Up and Down keep the column they started in.

use parley::{Affinity, Layout};

use rune_text::Ink;

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

/// A word back and forward in the text's order, as Chromium steps on each platform: words by
/// Unicode's word boundaries with the platform's dictionaries (`日本語` is one word) and
/// Chromium's tailoring (`rune_text::segmenter::word_ends`: `foo.bar` is `foo`, `.`, `bar`), a run
/// of punctuation is a stop of its own (`src/util/format.ts` stops at `src`, `/`, `util`, `/`,
/// `format`, `.` and `ts`), and spaces are stepped over. A word back goes to the start of the
/// word or punctuation before the caret. A word forward goes to its end on macOS; elsewhere past
/// the spaces after it too, to the next word's start on the same line, as Ctrl+Right does. A pill
/// is one word.
fn line_start_in(s: &str, j: usize) -> usize {
    s[..j].rfind('\n').map_or(0, |k| k + 1)
}

/// The run ending at byte `j` in `s` (a line break is a run of its own): its start, and what it is.
fn run_before_in(s: &str, j: usize) -> Option<(usize, Run)> {
    if j == 0 {
        return None;
    }
    let line = line_start_in(s, j);
    if line == j {
        return Some((prev_char(s, j), Run::Break));
    }
    let end = s[j..].find('\n').map_or(s.len(), |k| j + k);
    let rel = j - line;
    runs(&s[line..end]).into_iter().find(|r| r.0 < rel && rel <= r.1).map(|r| (line + r.0, r.2))
}

/// The run starting at or holding byte `j` in `s`: its end, and what it is.
fn run_after_in(s: &str, j: usize) -> Option<(usize, Run)> {
    if j >= s.len() {
        return None;
    }
    let end = s[j..].find('\n').map_or(s.len(), |k| j + k);
    if end == j {
        return Some((next_char(s, j), Run::Break));
    }
    let line = line_start_in(s, j);
    let rel = j - line;
    runs(&s[line..end]).into_iter().find(|r| r.0 <= rel && rel < r.1).map(|r| (line + r.1, r.2))
}

/// One word back from byte `i` in `s`, treating each byte range in `pills` as an atomic word.
/// Shared by `TextEdit` and selectable-text caret browsing.
#[doc(hidden)]
pub fn prev_word_in(s: &str, i: usize, pills: &[(usize, usize)]) -> usize {
    let pill_before = |at: usize| pills.iter().find(|&&(a, b)| a < at && at <= b).map(|&(a, _)| a);
    let mut j = i.min(s.len());
    loop {
        if let Some(start) = pill_before(j) {
            return start;
        }
        let Some((start, run)) = run_before_in(s, j) else { return 0 };
        match run {
            // Spaces back to the line's start stop there, as Chromium's caret does.
            Run::Space if line_start_in(s, start) == start && start > 0 => return start,
            Run::Space | Run::Break => {
                // A pill is one word: stepping back over the spaces before it stops at its
                // end, and the step after that takes the pill whole.
                if pill_before(start).is_some() {
                    return start;
                }
                j = start;
            }
            Run::Word | Run::Punct => {
                // A word runs back to a pill's end at most.
                let pill_end = pills.iter().filter(|&&(_, b)| start < b && b < j).map(|&(_, b)| b).max();
                return pill_end.unwrap_or(start);
            }
        }
    }
}

/// One word forward from byte `i` in `s`, treating each byte range in `pills` as an atomic word.
/// Shared by `TextEdit` and selectable-text caret browsing.
#[doc(hidden)]
pub fn next_word_in(s: &str, i: usize, pills: &[(usize, usize)]) -> usize {
    let pill_after = |at: usize| pills.iter().find(|&&(a, b)| a <= at && at < b).map(|&(_, b)| b);
    let clip_to_pill_start = |from: usize, end: usize| {
        pills.iter().filter(|&&(a, _)| from < a && a < end).map(|&(a, _)| a).min().unwrap_or(end)
    };
    let len = s.len();
    let mut j = i.min(len);
    if !cfg!(target_os = "macos") {
        match pill_after(j) {
            Some(end) => j = end,
            None => match run_after_in(s, j) {
                Some((end, Run::Word | Run::Punct)) => j = clip_to_pill_start(j, end),
                // From spaces (or a line's end), the next word's start (or the next line's).
                Some((end, Run::Space | Run::Break)) => return end,
                None => return len,
            },
        }
        // Past the spaces after it when a word or pill follows on the line.
        if let Some((end, Run::Space)) = run_after_in(s, j)
            && (pill_after(end).is_some() || matches!(run_after_in(s, end), Some((_, Run::Word | Run::Punct))))
        {
            j = end;
        }
        return j;
    }
    loop {
        if let Some(end) = pill_after(j) {
            return end;
        }
        let Some((end, run)) = run_after_in(s, j) else { return len };
        match run {
            Run::Space | Run::Break => j = end,
            Run::Word | Run::Punct => return clip_to_pill_start(j, end),
        }
    }
}

impl TextEdit {
    fn pill_ranges(&self) -> Vec<(usize, usize)> {
        self.marks.iter().filter(|m| m.mark.is_pill()).map(|m| (m.start, m.end)).collect()
    }

    pub(super) fn prev_word(&self, i: usize) -> usize {
        if self.is_masked() {
            return 0;
        }
        let pills = self.pill_ranges();
        prev_word_in(&self.text, i, &pills)
    }

    pub(super) fn next_word(&self, i: usize) -> usize {
        if self.is_masked() {
            return self.text.len();
        }
        let pills = self.pill_ranges();
        next_word_in(&self.text, i, &pills)
    }
}

/// What a stretch of a line is to a word move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Run {
    /// Holds a letter, a digit or an underscore.
    Word,
    /// Punctuation or symbols, all of a run together (`()`, `--`, `🙂🙂`).
    Punct,
    Space,
    /// A line break.
    Break,
}

/// The runs of `line` (no line break in it), in order: (start, end, what).
fn runs(line: &str) -> Vec<(usize, usize, Run)> {
    let mut out: Vec<(usize, usize, Run)> = Vec::new();
    let mut start = 0;
    for end in rune_text::segmenter::word_ends(line) {
        let seg = &line[start..end];
        let run = if seg.chars().all(char::is_whitespace) {
            Run::Space
        } else if seg.chars().any(|c| c.is_alphanumeric() || c == '_') {
            Run::Word
        } else {
            Run::Punct
        };
        match out.last_mut() {
            Some(last) if last.2 == run && run != Run::Word => last.1 = end,
            _ => out.push((start, end, run)),
        }
        start = end;
    }
    out
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
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrow {
    Left,
    Right,
    WordLeft,
    WordRight,
}

/// Where an arrow press lands across the drawn line.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landing {
    /// On this display byte, on this side of it.
    At(usize, Affinity),
    /// The caret is at the line's edge that way: it leaves the line the way its paragraph runs
    /// (left out of a right-to-left line goes on to the next line), or stays at the text's end.
    Edge,
}

/// One caret navigation command classified from a `KeyEvent`, shared between read-only
/// `TextEdit` and selectable-text caret browsing.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretMove {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

/// Whether `Home`/`End` with modifiers `m` on `macos` targets the document ends (rather than
/// visual line edges). Shared by `TextEdit::home_end_target` and caret browsing.
#[doc(hidden)]
pub fn home_end_is_doc(m: super::Mods, macos: bool, line_home_end: bool) -> bool {
    m.ctrl || (macos && (m.cmd || !line_home_end))
}

/// Classifies a logical key event into a caret-navigation movement and whether `Shift` extends
/// the selection, following the same platform modifier and macOS Cocoa control-key rules as
/// `TextEdit::key_in_read_only`.
#[doc(hidden)]
pub fn classify_caret_key(ev: &super::KeyEvent, macos: bool, line_home_end: bool) -> Option<(CaretMove, bool)> {
    use super::Key;
    if matches!(ev.key, Key::Modifier { .. } | Key::Media(_)) {
        return None;
    }
    let m = ev.mods;
    if let Some(c) = ev.control_binding() {
        let mov = match c {
            'a' => CaretMove::LineStart,
            'e' => CaretMove::LineEnd,
            'b' => CaretMove::Left,
            'f' => CaretMove::Right,
            'p' => CaretMove::Up,
            'n' => CaretMove::Down,
            _ => return None,
        };
        return Some((mov, m.shift));
    }
    if ev.is_command_chord() || ev.is_window_menu_chord() {
        return None;
    }
    let word = m.alt || (m.ctrl && !macos);
    let line = m.cmd && (macos || !m.ctrl);
    let mov = match &ev.key {
        Key::Left if line => CaretMove::LineStart,
        Key::Left if word => CaretMove::WordLeft,
        Key::Left => CaretMove::Left,
        Key::Right if line => CaretMove::LineEnd,
        Key::Right if word => CaretMove::WordRight,
        Key::Right => CaretMove::Right,
        Key::Up if line => CaretMove::DocStart,
        Key::Up => CaretMove::Up,
        Key::Down if line => CaretMove::DocEnd,
        Key::Down => CaretMove::Down,
        Key::Home => if home_end_is_doc(m, macos, line_home_end) { CaretMove::DocStart } else { CaretMove::LineStart },
        Key::End => if home_end_is_doc(m, macos, line_home_end) { CaretMove::DocEnd } else { CaretMove::LineEnd },
        Key::PageUp if !m.cmd && !m.ctrl && !m.alt && !m.logo => CaretMove::PageUp,
        Key::PageDown if !m.cmd && !m.ctrl && !m.alt && !m.logo => CaretMove::PageDown,
        _ => return None,
    };
    Some((mov, m.shift))
}

/// The end of a single-layout selection `a..b` (with `focus`) that plain Left (`!right`) or
/// Right collapses it to: the end drawn further that way when both sit on one line (so Left in an
/// Arabic line goes to its logical end), else the end the arrow steps toward in the focus's
/// paragraph direction. Shared by `TextEdit::collapse_to` and caret browsing.
#[doc(hidden)]
pub fn collapse_edge(layout: Option<rune_text::TextFlow<'_>>, a: usize, b: usize, focus: usize, right: bool) -> usize {
    let Some(l) = layout else { return if right { b } else { a } };
    let (ca, cb) = (l.caret(a, Affinity::Downstream, 0.0), l.caret(b, Affinity::Upstream, 0.0));
    if (ca.y0 - cb.y0).abs() < 1.0 {
        return if (ca.x0 <= cb.x0) != right { a } else { b };
    }
    let forward = right == (l.piece_at(focus).flow == rune_text::Flow::Ltr);
    if forward { b } else { a }
}

/// Where `arrow` takes a caret at display byte `at` (on `side` of it) across `text` as drawn,
/// with the direction of the paragraph it's in; `None` where it moves in the text's order (a
/// paragraph of only left-to-right runs, or a `hidden` secret). A pill's isolate marks are
/// stops of their own that leave the caret on the same byte, so it steps on past them.
#[doc(hidden)]
pub fn visual_in(text: rune_text::TextFlow<'_>, at: usize, side: Affinity, arrow: Arrow, hidden: bool) -> Option<(Landing, rune_text::Flow)> {
    let p = text.piece_at(at);
    if CaretMotion::of(p.layout, hidden) == CaretMotion::Logical {
        return None;
    }
    let (right, word) = match arrow {
        Arrow::Left => (false, false),
        Arrow::Right => (true, false),
        Arrow::WordLeft => (false, true),
        Arrow::WordRight => (true, true),
    };
    let (mut from, mut side) = (p.shaped(at), side);
    for _ in 0..4 {
        match rune_text::visual_step(p.layout, from, side, right, word) {
            Some((j, s)) if p.content(j) == at && j != from => (from, side) = (j, s),
            Some((j, s)) => return Some((Landing::At(p.content(j), s), p.flow)),
            None => return Some((Landing::Edge, p.flow)),
        }
    }
    Some((Landing::Edge, p.flow))
}

/// Whether the cluster just `right` (else left) of the caret at display byte `at` (on `side`
/// of it) is a space the text runs on into that way: in a left-to-right run to the right, in a
/// right-to-left run to the left. A word forward goes on past it off macOS.
#[doc(hidden)]
pub fn space_ahead(text: rune_text::TextFlow<'_>, at: usize, side: Affinity, right: bool) -> bool {
    let p = text.piece_at(at);
    rune_text::space_ahead(p.layout, p.shaped(at), side, right)
}
