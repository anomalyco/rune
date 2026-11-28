//! Multi-caret / multi-cursor editing owned by [`TextEdit`].
//!
//! Primary selection stays in [`TextEdit::anchor`] / [`TextEdit::focus`]; extra carets live in a
//! private `Vec` (zero allocation when single-caret). Overlapping or colliding carets merge
//! deterministically, keeping the primary caret primary and secondary carets sorted and disjoint.

use parley::Layout;
use rune_text::{Ink, TextFlow};
use unicode_segmentation::UnicodeSegmentation;

use super::*;

/// One caret or selection in a [`TextEdit`]: byte offsets `(anchor, focus)` on char boundaries
/// of [`TextEdit::text`]. When `anchor == focus` it is a collapsed caret.
///
/// ```
/// use rune_edit::Caret;
///
/// let c = Caret::caret(4);
/// assert!(c.is_empty());
/// assert_eq!(c.range(), (4, 4));
///
/// let sel = Caret::selection(8, 2);
/// assert!(!sel.is_empty());
/// assert_eq!(sel.range(), (2, 8));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Caret {
    pub anchor: usize,
    pub focus: usize,
}

impl Caret {
    /// A collapsed caret at byte offset `at`.
    pub const fn caret(at: usize) -> Self {
        Self { anchor: at, focus: at }
    }

    /// A selection from `anchor` to `focus`.
    pub const fn selection(anchor: usize, focus: usize) -> Self {
        Self { anchor, focus }
    }

    /// Ordered byte range `(start, end)` with `start <= end`.
    pub const fn range(self) -> (usize, usize) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    /// True when `anchor == focus` (no text selected).
    pub const fn is_empty(self) -> bool {
        self.anchor == self.focus
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct SecondaryCaret {
    pub(super) anchor: usize,
    pub(super) focus: usize,
    pub(super) column: Column,
    pub(super) affinity: parley::Affinity,
}

impl SecondaryCaret {
    pub(super) fn new(anchor: usize, focus: usize) -> Self {
        Self { anchor, focus, column: Column::Free, affinity: parley::Affinity::Downstream }
    }

    pub(super) fn range(&self) -> (usize, usize) {
        (self.anchor.min(self.focus), self.anchor.max(self.focus))
    }

    pub(super) fn as_caret(&self) -> Caret {
        Caret { anchor: self.anchor, focus: self.focus }
    }
}

struct Entry {
    primary: bool,
    anchor: usize,
    focus: usize,
    column: Column,
    affinity: parley::Affinity,
}

impl Entry {
    fn range(&self) -> (usize, usize) {
        (self.anchor.min(self.focus), self.anchor.max(self.focus))
    }
}

impl TextEdit {
    /// All active carets/selections: the primary caret (`anchor` / `focus`) first, followed by
    /// any secondary carets in document order.
    ///
    /// ```
    /// use rune_edit::{Caret, TextEdit};
    ///
    /// let mut edit = TextEdit::new("one\ntwo");
    /// edit.set_caret(0);
    /// assert!(edit.add_caret(4));
    /// assert_eq!(edit.carets(), vec![Caret::caret(0), Caret::caret(4)]);
    /// ```
    pub fn carets(&self) -> Vec<Caret> {
        let mut out = Vec::with_capacity(1 + self.secondary.len());
        let (a, b) = self.range();
        let primary = if self.anchor <= self.focus { Caret::selection(a, b) } else { Caret::selection(b, a) };
        out.push(primary);
        for c in &self.secondary {
            let ca = Self::clamp_char_boundary(&self.text, c.anchor);
            let cf = Self::clamp_char_boundary(&self.text, c.focus);
            out.push(Caret::selection(ca, cf));
        }
        out
    }

    /// Secondary carets (excluding primary) as `(anchor, focus, affinity)`, for painting and
    /// layout queries. Empty in single-caret mode.
    #[doc(hidden)]
    pub fn secondary_carets(&self) -> impl Iterator<Item = (usize, usize, parley::Affinity)> + '_ {
        self.secondary.iter().map(|c| (
            Self::clamp_char_boundary(&self.text, c.anchor),
            Self::clamp_char_boundary(&self.text, c.focus),
            c.affinity,
        ))
    }

    /// Secondary carets as raw `(anchor, focus)` pairs for change detection and undo snapshots.
    pub(crate) fn secondary_pairs(&self) -> Vec<(usize, usize)> {
        self.secondary.iter().map(|c| (c.anchor, c.focus)).collect()
    }

    /// Whether this field currently has more than one caret.
    pub fn has_extra_carets(&self) -> bool {
        !self.secondary.is_empty()
    }

    /// Whether any active caret (primary or secondary) has a non-empty selection.
    #[doc(hidden)]
    pub fn has_any_selection(&self) -> bool {
        let (a, b) = self.range();
        if a != b {
            return true;
        }
        self.secondary.iter().any(|c| c.anchor != c.focus)
    }

    /// Selected text across all active carets. For a single caret this equals
    /// [`Self::selected`]; for multiple carets it joins every non-empty selection in document
    /// order with `"\n"`.
    pub fn selected_all(&self) -> String {
        if self.secure {
            return String::new();
        }
        if self.secondary.is_empty() {
            return self.selected().to_owned();
        }
        let mut ranges: Vec<(usize, usize)> = Vec::with_capacity(1 + self.secondary.len());
        let (pa, pb) = self.range();
        if pa < pb {
            ranges.push((pa, pb));
        }
        for c in &self.secondary {
            let a = Self::clamp_char_boundary(&self.text, c.anchor.min(c.focus));
            let b = Self::clamp_char_boundary(&self.text, c.anchor.max(c.focus));
            if a < b {
                ranges.push((a, b));
            }
        }
        ranges.sort_unstable();
        let mut out = String::new();
        for (idx, (a, b)) in ranges.into_iter().enumerate() {
            if idx > 0 {
                out.push('\n');
            }
            out.push_str(&self.text[a..b]);
        }
        out
    }

    /// Add a collapsed secondary caret at `byte` (clamped to a char boundary and snapped out of
    /// pills). Overlapping or duplicate carets merge automatically. No-op on [`secure`](Self::secure)
    /// fields. Returns `true` if the caret set changed.
    pub fn add_caret(&mut self, byte: usize) -> bool {
        self.add_selection(byte, byte)
    }

    /// Add a secondary selection `(anchor, focus)` (clamped to char boundaries and snapped out of
    /// pills). Overlapping carets merge automatically. No-op on [`secure`](Self::secure) fields.
    /// Returns `true` if the caret set changed.
    pub fn add_selection(&mut self, anchor: usize, focus: usize) -> bool {
        if self.secure {
            return false;
        }
        self.sync_invariants();
        let before = self.carets();
        let a = Self::clamp_char_boundary(&self.text, anchor);
        let f = Self::clamp_char_boundary(&self.text, focus);
        self.secondary.push(SecondaryCaret::new(a, f));
        self.normalize_carets();
        self.typing = Typing::Inherited;
        self.carets() != before
    }

    /// Replace the primary and secondary carets at once, clamping to char boundaries and merging
    /// overlaps. On [`secure`](Self::secure) fields, secondary carets are ignored.
    pub fn set_carets(&mut self, primary: Caret, secondary: impl IntoIterator<Item = Caret>) {
        self.anchor = Self::clamp_char_boundary(&self.text, primary.anchor);
        self.focus = Self::clamp_char_boundary(&self.text, primary.focus);
        self.secondary.clear();
        if !self.secure {
            for c in secondary {
                let a = Self::clamp_char_boundary(&self.text, c.anchor);
                let f = Self::clamp_char_boundary(&self.text, c.focus);
                self.secondary.push(SecondaryCaret::new(a, f));
            }
        }
        self.typing = Typing::Inherited;
        self.column = Column::Free;
        self.affinity = parley::Affinity::Downstream;
        self.normalize_carets();
    }

    /// Drop all secondary carets, keeping only the primary `anchor` / `focus`. Returns `true` if
    /// any extra carets were removed.
    pub fn clear_extra_carets(&mut self) -> bool {
        if self.secondary.is_empty() {
            return false;
        }
        self.secondary.clear();
        true
    }

    /// Add a caret one line above the topmost active caret, preserving visual column when
    /// `layout` is provided (or character column per `\n` line when `layout` is `None`).
    /// Returns `true` if a new caret was added.
    pub fn add_caret_above(&mut self, layout: Option<&Layout<Ink>>) -> bool {
        self.add_caret_vertical(layout.map(TextFlow::Single), false)
    }

    /// Add a caret one line below the bottommost active caret, preserving visual column when
    /// `layout` is provided (or character column per `\n` line when `layout` is `None`).
    /// Returns `true` if a new caret was added.
    pub fn add_caret_below(&mut self, layout: Option<&Layout<Ink>>) -> bool {
        self.add_caret_vertical(layout.map(TextFlow::Single), true)
    }

    /// [`Self::add_caret_above`] for a runtime [`TextFlow`].
    #[doc(hidden)]
    pub fn add_caret_above_in(&mut self, layout: Option<TextFlow<'_>>) -> bool {
        self.add_caret_vertical(layout, false)
    }

    /// [`Self::add_caret_below`] for a runtime [`TextFlow`].
    #[doc(hidden)]
    pub fn add_caret_below_in(&mut self, layout: Option<TextFlow<'_>>) -> bool {
        self.add_caret_vertical(layout, true)
    }

    fn add_caret_vertical(&mut self, layout: Option<TextFlow<'_>>, down: bool) -> bool {
        if self.secure || self.is_single_line() {
            return false;
        }
        self.sync_invariants();
        // Pick the topmost (down == false) or bottommost (down == true) caret.
        let mut edge_focus = self.focus;
        let mut edge_col = self.column;
        for c in &self.secondary {
            if (down && c.focus > edge_focus) || (!down && c.focus < edge_focus) {
                edge_focus = c.focus;
                edge_col = c.column;
            }
        }
        let target = if let Some(l) = layout && !self.is_masked() {
            let disp = self.display_index(edge_focus);
            l.move_line(disp, down, edge_col.goal()).and_then(|(j, x)| {
                let m = self.model_index(j);
                (m != edge_focus).then_some((m, Column::Kept(x)))
            })
        } else {
            self.logical_vertical_step(edge_focus, down).map(|m| (m, Column::Free))
        };
        let Some((at, col)) = target else {
            return false;
        };
        let before = self.carets();
        let mut sc = SecondaryCaret::new(at, at);
        sc.column = col;
        self.secondary.push(sc);
        self.normalize_carets();
        self.typing = Typing::Inherited;
        self.carets() != before
    }

    fn logical_vertical_step(&self, byte: usize, down: bool) -> Option<usize> {
        let line_idx = self.line_of_byte(byte);
        let line_count = 1 + count_newlines(self.text.as_bytes());
        let target_line = if down {
            (line_idx + 1 < line_count).then_some(line_idx + 1)?
        } else {
            line_idx.checked_sub(1)?
        };
        let cur_range = self.line_range(line_idx);
        let char_col = self.text[cur_range.start..byte.clamp(cur_range.start, cur_range.end)].chars().count();
        let target_range = self.line_range(target_line);
        let target_slice = &self.text[target_range.clone()];
        let offset = target_slice
            .char_indices()
            .nth(char_col)
            .map_or(target_slice.len(), |(i, _)| i);
        Some(target_range.start + offset)
    }

    /// Select the word under the primary caret if nothing is selected, or add a secondary
    /// selection on the next uncovered occurrence of the primary selection (wrapping around).
    ///
    /// Returns `true` if the selection or caret set changed.
    pub fn add_next_occurrence(&mut self) -> bool {
        if self.secure {
            return false;
        }
        self.sync_invariants();
        let (pa, pb) = self.range();
        if pa == pb {
            let word = self
                .text
                .split_word_bound_indices()
                .map(|(i, w)| (i, i + w.len(), w))
                .find(|&(s, e, w)| !w.trim().is_empty() && s <= pa && pa <= e);
            let Some((ws, we, _)) = word else {
                return false;
            };
            self.anchor = ws;
            self.focus = we;
            self.normalize_carets();
            self.typing = Typing::Inherited;
            return true;
        }
        let needle = &self.text[pa..pb];
        if needle.is_empty() {
            return false;
        }
        let forward = self.focus >= self.anchor;
        let existing: Vec<(usize, usize)> = self.carets().into_iter().map(Caret::range).collect();
        let overlaps_existing = |s: usize, e: usize| {
            existing.iter().any(|&(a, b)| if a == b { s <= a && a <= e } else { s < b && a < e })
        };
        let start_from = existing.iter().map(|&(_, b)| b).max().unwrap_or(pb);
        let found = self.text[start_from..]
            .match_indices(needle)
            .map(|(off, _)| (start_from + off, start_from + off + needle.len()))
            .chain(
                self.text[..start_from]
                    .match_indices(needle)
                    .map(|(off, _)| (off, off + needle.len())),
            )
            .find(|&(s, e)| !overlaps_existing(s, e));
        let Some((s, e)) = found else {
            return false;
        };
        let (a, f) = if forward { (s, e) } else { (e, s) };
        self.add_selection(a, f)
    }

    fn snap_pair_to_pills(&self, mut anchor: usize, mut focus: usize) -> (usize, usize) {
        if !self.marks.iter().any(|m| m.mark.is_pill()) {
            return (anchor, focus);
        }
        if anchor == focus {
            if let Some((s, e)) = self.pill_around(focus) {
                let to = if focus - s < e - focus { s } else { e };
                (anchor, focus) = (to, to);
            }
        } else {
            let forward = focus > anchor;
            anchor = self.out_of_pill(anchor, !forward);
            focus = self.out_of_pill(focus, forward);
        }
        (anchor, focus)
    }

    /// Clamps primary and secondary carets to char boundaries, snaps them out of pills, and
    /// merges overlapping or duplicate carets while keeping primary primary and secondary sorted.
    pub(super) fn normalize_carets(&mut self) {
        self.anchor = Self::clamp_char_boundary(&self.text, self.anchor);
        self.focus = Self::clamp_char_boundary(&self.text, self.focus);
        let (pa, pf) = self.snap_pair_to_pills(self.anchor, self.focus);
        self.anchor = pa;
        self.focus = pf;
        if self.secure {
            self.secondary.clear();
            return;
        }
        if self.secondary.is_empty() {
            return;
        }
        let mut entries = Vec::with_capacity(1 + self.secondary.len());
        entries.push(Entry {
            primary: true,
            anchor: self.anchor,
            focus: self.focus,
            column: self.column,
            affinity: self.affinity,
        });
        let prev_secondary = std::mem::take(&mut self.secondary);
        for mut c in prev_secondary {
            c.anchor = Self::clamp_char_boundary(&self.text, c.anchor);
            c.focus = Self::clamp_char_boundary(&self.text, c.focus);
            let (a, f) = self.snap_pair_to_pills(c.anchor, c.focus);
            entries.push(Entry {
                primary: false,
                anchor: a,
                focus: f,
                column: c.column,
                affinity: c.affinity,
            });
        }
        entries.sort_by_key(|e| {
            let (s, end) = e.range();
            (s, end, !e.primary)
        });
        let mut merged: Vec<Entry> = Vec::with_capacity(entries.len());
        for cur in entries {
            if let Some(prev) = merged.last_mut() {
                let (a1, b1) = prev.range();
                let (a2, b2) = cur.range();
                let overlaps = if a1 == b1 && a2 == b2 {
                    a1 == a2
                } else if a1 < b1 && a2 < b2 {
                    a2 < b1
                } else {
                    a2 <= b1
                };
                if overlaps {
                    let start = a1.min(a2);
                    let end = b1.max(b2);
                    let forward = if cur.primary {
                        cur.focus >= cur.anchor
                    } else {
                        prev.focus >= prev.anchor
                    };
                    if cur.primary {
                        prev.primary = true;
                        prev.column = cur.column;
                        prev.affinity = cur.affinity;
                    }
                    (prev.anchor, prev.focus) = if forward { (start, end) } else { (end, start) };
                    continue;
                }
            }
            merged.push(cur);
        }
        for e in merged {
            if e.primary {
                self.anchor = e.anchor;
                self.focus = e.focus;
                self.column = e.column;
                self.affinity = e.affinity;
            } else {
                self.secondary.push(SecondaryCaret {
                    anchor: e.anchor,
                    focus: e.focus,
                    column: e.column,
                    affinity: e.affinity,
                });
            }
        }
    }

    /// All carets in ascending document order as `(is_primary, range)`.
    fn sorted_caret_ranges(&self) -> Vec<(bool, (usize, usize))> {
        let mut out = Vec::with_capacity(1 + self.secondary.len());
        out.push((true, self.range()));
        for c in &self.secondary {
            out.push((false, c.range()));
        }
        out.sort_unstable_by_key(|&(_, (a, b))| (a, b));
        out
    }

    /// Applies one replacement per active caret as a single atomic undo step.
    /// `edits` must be sorted by ascending range start; overlapping deletion ranges are merged.
    pub(super) fn apply_multi_replace(
        &mut self,
        mut edits: Vec<(bool, (usize, usize), String)>,
        kind: EditKind,
        new_line_block: Option<Block>,
        soft: bool,
    ) {
        self.sync_invariants();
        if kind == EditKind::Other {
            self.break_history_group();
        }
        // Cover pills and merge any ranges that now overlap (e.g., adjacent carets deleting).
        for (_, r, _) in &mut edits {
            *r = self.cover_pills(r.0, r.1);
        }
        edits.sort_by_key(|&(p, (a, b), _)| (a, b, !p));
        let mut merged: Vec<(bool, (usize, usize), String)> = Vec::with_capacity(edits.len());
        for (p, (a, b), s) in edits {
            if let Some(prev) = merged.last_mut() {
                let (pa, pb) = prev.1;
                let overlaps = if pa == pb && a == b {
                    pa == a
                } else if pa < pb && a < b {
                    a < pb
                } else {
                    a <= pb
                };
                if overlaps {
                    prev.0 |= p;
                    prev.1 = (pa.min(a), pb.max(b));
                    if prev.2.is_empty() && !s.is_empty() {
                        prev.2 = s;
                    }
                    continue;
                }
            }
            merged.push((p, (a, b), s));
        }

        let before = self.snapshot();
        self.typing = Typing::Inherited;

        // Precompute new collapsed caret positions from ascending order.
        let mut new_positions: Vec<(bool, usize)> = Vec::with_capacity(merged.len());
        let mut delta: isize = 0;
        for &(is_primary, (a, b), ref s) in &merged {
            let pos = (a as isize + delta + s.len() as isize).max(0) as usize;
            new_positions.push((is_primary, pos));
            delta += s.len() as isize - (b - a) as isize;
        }

        // Apply replacements in descending order so earlier byte offsets remain valid.
        for &(_, (a, b), ref s) in merged.iter().rev() {
            let inserted_marks: Vec<Mark> = if s.is_empty() {
                Vec::new()
            } else if a < b {
                let line_idx = self.line_of_byte(a);
                if self.blocks.get(line_idx) == Some(&Block::Code) {
                    Vec::new()
                } else {
                    self.marks
                        .iter()
                        .filter(|m| m.start <= a && a < m.end)
                        .map(|m| m.mark.clone())
                        .collect()
                }
            } else {
                self.inherited_marks_at(a)
            };
            self.apply_text_and_attr_replace((a, b), s, &inserted_marks, new_line_block, soft);
        }

        self.secondary.clear();
        let mut primary_set = false;
        for (is_primary, pos) in new_positions {
            if is_primary && !primary_set {
                self.anchor = pos;
                self.focus = pos;
                primary_set = true;
            } else {
                self.secondary.push(SecondaryCaret::new(pos, pos));
            }
        }
        self.column = Column::Free;
        self.affinity = parley::Affinity::Downstream;
        self.sync_invariants();
        self.mark_edited();
        self.record_change(before, kind);
    }

    /// Inserts `s` at every active caret as one undo step.
    pub(super) fn multi_insert(&mut self, s: &str, kind: EditKind) {
        let carets = self.sorted_caret_ranges();
        let edits = carets.into_iter().map(|(p, r)| (p, r, s.to_owned())).collect();
        self.apply_multi_replace(edits, kind, None, false);
    }

    /// Pastes `s` across active carets: when `s` has the exact same number of `\n`-delimited
    /// lines as active carets, each caret receives its matching line; otherwise every caret
    /// receives the full `s`.
    pub(super) fn multi_paste(&mut self, s: &str) {
        let carets = self.sorted_caret_ranges();
        let lines: Vec<&str> = s.split('\n').collect();
        let edits = if lines.len() == carets.len() {
            carets
                .into_iter()
                .zip(lines)
                .map(|((p, r), line)| (p, r, line.to_owned()))
                .collect()
        } else {
            carets.into_iter().map(|(p, r)| (p, r, s.to_owned())).collect()
        };
        self.apply_multi_replace(edits, EditKind::Other, None, false);
    }

    /// Multi-caret backspace (char, word, or line-start) as one undo step.
    pub(super) fn multi_backspace(&mut self, word: bool, line: bool) {
        self.sync_invariants();
        let carets = self.sorted_caret_ranges();
        let mut edits = Vec::with_capacity(carets.len());
        let mut any_delete = false;
        for (p, (a, b)) in carets {
            let range = if a != b {
                any_delete = true;
                (a, b)
            } else if a > 0 {
                let from = if line {
                    self.text[..a].rfind('\n').map_or(0, |i| i + 1)
                } else if word {
                    self.prev_word(a)
                } else {
                    prev_backspace(&self.text, a)
                };
                if from < a {
                    any_delete = true;
                }
                (from, a)
            } else {
                (0, 0)
            };
            edits.push((p, range, String::new()));
        }
        if !any_delete {
            return;
        }
        let kind = if line || word { EditKind::DeleteRun } else { EditKind::Delete };
        self.apply_multi_replace(edits, kind, None, false);
    }

    /// Multi-caret forward delete (char or word) as one undo step.
    pub(super) fn multi_delete(&mut self, word: bool) {
        self.sync_invariants();
        let carets = self.sorted_caret_ranges();
        let mut edits = Vec::with_capacity(carets.len());
        let mut any_delete = false;
        for (p, (a, b)) in carets {
            let range = if a != b {
                any_delete = true;
                (a, b)
            } else if a < self.text.len() {
                let to = if word { self.next_word(a) } else { next_char(&self.text, a) };
                if to > a {
                    any_delete = true;
                }
                (a, to)
            } else {
                (a, a)
            };
            edits.push((p, range, String::new()));
        }
        if !any_delete {
            return;
        }
        let kind = if word { EditKind::DeleteRun } else { EditKind::Delete };
        self.apply_multi_replace(edits, kind, None, false);
    }

    /// Runs a navigation key (`Left`, `Right`, `Up`, `Down`, `Home`, `End`) independently on
    /// primary and every secondary caret, preserving each caret's vertical `column` goal, then
    /// merges any colliding carets.
    pub(super) fn multi_navigate(&mut self, ev: &KeyEvent, layout: Option<TextFlow<'_>>, primary_col: Column) {
        let mut secondary = std::mem::take(&mut self.secondary);
        // Move primary using the single-caret read-only navigation path (never mutates text).
        self.column = primary_col;
        let _ = self.key_inner(ev, layout, true);
        let p_anchor = self.anchor;
        let p_focus = self.focus;
        let p_col = self.column;
        let p_aff = self.affinity;

        for c in &mut secondary {
            self.anchor = c.anchor;
            self.focus = c.focus;
            self.column = c.column;
            self.affinity = c.affinity;
            let _ = self.key_inner(ev, layout, true);
            c.anchor = self.anchor;
            c.focus = self.focus;
            c.column = self.column;
            c.affinity = self.affinity;
        }

        self.anchor = p_anchor;
        self.focus = p_focus;
        self.column = p_col;
        self.affinity = p_aff;
        self.secondary = secondary;
        self.normalize_carets();
    }
}

#[cfg(test)]
#[path = "multi_caret_tests.rs"]
mod tests;
