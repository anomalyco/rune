//! Core text and attribute replacement engine, grapheme stepping, and input sanitization.

use super::*;

/// The previous grapheme boundary, so a caret or backspace takes a ZWJ emoji, flag or skin
/// tone whole.
pub(crate) fn prev_char(s: &str, i: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    s[..i].grapheme_indices(true).next_back().map_or(0, |(j, _)| j)
}

/// Where Backspace deletes back to from `i`, as Chromium's `BackspaceStateMachine` and native
/// fields do: one code point (a combining accent, a Thai or Indic vowel sign, the last jamo of
/// a decomposed syllable), so a typist fixes the last thing they typed without retyping the
/// cluster; but a CR LF and an emoji sequence (a variation selector, a skin tone, a keycap, a
/// flag's pair, a tag sequence, a ZWJ sequence of emoji) go whole.
pub(crate) fn prev_backspace(s: &str, i: usize) -> usize {
    let start = prev_char(s, i);
    let cluster = &s[start..i];
    let emoji = cluster.chars().any(|c| matches!(c as u32, 0xFE0F | 0x20E3 | 0x1F3FB..=0x1F3FF | 0x1F1E6..=0x1F1FF | 0xE0020..=0xE007F))
        || (cluster.contains('\u{200D}') && cluster.chars().any(rune_text::is_pictographic));
    if cluster == "\r\n" || emoji {
        return start;
    }
    s[..i].char_indices().next_back().map_or(0, |(j, _)| j)
}

pub(crate) fn next_char(s: &str, i: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    s[i..].graphemes(true).next().map_or(i, |g| i + g.len())
}

/// `\n` bytes in `s`. Counts into `u8` lanes so it vectorizes: `filter().count()` widens every
/// byte to `usize` and runs several times slower on a long field.
pub(crate) fn count_newlines(s: &[u8]) -> usize {
    s.chunks(255).map(|c| c.iter().fold(0u8, |n, &b| n + (b == b'\n') as u8) as usize).sum()
}

impl TextEdit {
    /// Text typed, pasted or committed into this field. Single-line fields drop control
    /// characters, line breaks included (stripped, as `<input type=text>` does); multi-line
    /// fields fold CRLF to LF (and multi-line secure fields also drop control characters other
    /// than `\n` and `\t`). Sized up front so no partial copy of a secret is left behind by a
    /// reallocation.
    pub(crate) fn sanitize(&self, s: &str) -> Wiped {
        let mut out = Wiped(String::with_capacity(s.len()));
        if self.is_single_line() {
            out.extend(s.chars().filter(|c| !c.is_control() && !matches!(c, '\u{2028}' | '\u{2029}')));
        } else if self.secure {
            let mut chars = s.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\r' {
                    if chars.peek() != Some(&'\n') {
                        out.push('\n');
                    }
                    continue;
                }
                if (!c.is_control() || matches!(c, '\n' | '\t')) && !matches!(c, '\u{2028}' | '\u{2029}') {
                    out.push(c);
                }
            }
        } else if !s.contains('\r') {
            // Most pastes: copied whole, not a character at a time.
            out.push_str(s);
        } else {
            let mut chars = s.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\r' && chars.peek() == Some(&'\n') { continue; }
                out.push(c);
            }
        }
        out
    }

    /// Core replacement engine updating `text`, `marks`, `blocks`, and `soft_breaks` together.
    pub(crate) fn apply_text_and_attr_replace(
        &mut self,
        (a, b): (usize, usize),
        s: &str,
        inserted_marks: &[Mark],
        new_line_block: Option<Block>,
        mark_new_lines_soft: bool,
    ) {
        // Callers sync first; a second whole-text pass here would show on every keystroke.
        self.column = Column::Free;
        self.affinity = parley::Affinity::Downstream;
        let del_len = b - a;
        let ins_len = s.len();
        if self.marks.is_empty()
            && inserted_marks.is_empty()
            && self.soft_breaks.is_empty()
            && !mark_new_lines_soft
            && new_line_block.is_none_or(|b| b == Block::Paragraph)
            && self.blocks.iter().all(|&b| b == Block::Paragraph)
        {
            // Plain text: no per-line passes, so a keystroke in a long plain field costs what
            // it did before blocks and marks existed.
            let removed = count_newlines(&self.text.as_bytes()[a..b]);
            let added = count_newlines(s.as_bytes());
            self.text.replace_range(a..b, s);
            self.blocks.resize(self.blocks.len() + added - removed, Block::Paragraph);
            self.anchor = a + ins_len;
            self.focus = self.anchor;
            self.cancel_composition();
            return;
        }
        let line_a = self.line_of_byte(a);
        let line_b = self.line_of_byte(b);
        let line_a_range = self.line_range(line_a);
        let line_b_range = self.line_range(line_b);

        // Determine the surviving block for `line_a` when `a..b` merges `line_a..=line_b`.
        let merged_block = if line_a < line_b
            && line_a_range.is_empty()
            && a == line_a_range.start
            && b == line_b_range.start
        {
            self.blocks.get(line_b).copied().unwrap_or(Block::Paragraph)
        } else {
            self.blocks.get(line_a).copied().unwrap_or(Block::Paragraph)
        };
        let merged_soft = if line_a < line_b
            && line_a_range.is_empty()
            && a == line_a_range.start
            && b == line_b_range.start
        {
            self.soft_breaks.binary_search(&(line_b as u32)).is_ok() && line_a > 0
        } else {
            self.soft_breaks.binary_search(&(line_a as u32)).is_ok()
        };

        let removed_lines = line_b - line_a;
        let added_lines = count_newlines(s.as_bytes());
        let next_line_blk = new_line_block.unwrap_or(match merged_block {
            Block::Code => Block::Code,
            _ => Block::Paragraph,
        });

        // Update `blocks` for removed and added lines.
        if removed_lines > 0 {
            self.blocks.drain(line_a + 1..=line_b);
        }
        if let Some(slot) = self.blocks.get_mut(line_a) {
            *slot = merged_block;
        }
        if added_lines > 0 {
            let insert_pos = (line_a + 1).min(self.blocks.len());
            self.blocks.splice(insert_pos..insert_pos, std::iter::repeat_n(next_line_blk, added_lines));
        }

        // Update `soft_breaks`.
        let mut next_soft = Vec::new();
        for &sb in &self.soft_breaks {
            let idx = sb as usize;
            if idx < line_a {
                next_soft.push(sb);
            } else if idx > line_b {
                let shifted = idx - removed_lines + added_lines;
                if shifted > 0 {
                    next_soft.push(shifted as u32);
                }
            }
        }
        if merged_soft && line_a > 0 {
            next_soft.push(line_a as u32);
        }
        if mark_new_lines_soft && added_lines > 0 {
            for k in 1..=added_lines {
                next_soft.push((line_a + k) as u32);
            }
        }
        self.soft_breaks = next_soft;

        // Shift and split existing inline marks around `a..b`.
        let mut updated_marks = Vec::with_capacity(self.marks.len() + inserted_marks.len());
        for m in self.marks.drain(..) {
            if m.end <= a {
                updated_marks.push(m);
            } else if m.start >= b {
                updated_marks.push(MarkRange {
                    start: m.start - del_len + ins_len,
                    end: m.end - del_len + ins_len,
                    mark: m.mark,
                });
            } else if m.mark.is_pill() {
                // An edit that cuts into a pill leaves its remains as plain text.
            } else {
                // `m` overlaps `a..b` (or touches inside it).
                if m.start < a {
                    updated_marks.push(MarkRange {
                        start: m.start,
                        end: a,
                        mark: m.mark.clone(),
                    });
                }
                if m.end > b {
                    updated_marks.push(MarkRange {
                        start: a + ins_len,
                        end: m.end - del_len + ins_len,
                        mark: m.mark,
                    });
                }
            }
        }
        if ins_len > 0 {
            // Typed or replacing text never becomes a pill.
            for im in inserted_marks.iter().filter(|m| !m.is_pill()) {
                // Avoid wrapping a pure newline at the trailing edge of a mark unless inside the mark.
                if s == "\n" {
                    let was_inside = updated_marks.iter().any(|m| m.mark == *im && m.start >= a + ins_len);
                    if !was_inside {
                        continue;
                    }
                }
                updated_marks.push(MarkRange {
                    start: a,
                    end: a + ins_len,
                    mark: im.clone(),
                });
            }
        }
        self.marks = updated_marks;
        self.text.replace_range(a..b, s);
        self.anchor = a + ins_len;
        self.focus = self.anchor;
        self.cancel_composition();
        self.sync_invariants();
        self.mark_edited();
    }

    pub(crate) fn replace(&mut self, (mut a, mut b): (usize, usize), s: &str, kind: EditKind) {
        if self.is_hidden_multiline() && self.is_pristine() {
            (a, b) = (0, self.text.len());
        }
        if self.secure {
            let mut next = String::with_capacity(self.text.len() - (b - a) + s.len());
            next.push_str(&self.text[..a]);
            next.push_str(s);
            next.push_str(&self.text[b..]);
            self.replace_text(next);
            self.anchor = a + s.len();
            self.focus = self.anchor;
            self.mark_edited();
            return;
        }
        self.sync_invariants();
        // Backspace or Delete next to a pill, or a selection into one, takes it whole.
        let (a, b) = self.cover_pills(a, b);
        if kind == EditKind::Other { self.break_history_group(); }
        if &self.text[a..b] == s
            && self.typing == Typing::Inherited
            && !self.marks.iter().any(|m| m.mark.is_pill() && m.start < b && a < m.end)
        {
            self.anchor = a + s.len();
            self.focus = self.anchor;
            self.cancel_composition();
            self.mark_edited();
            return;
        }
        // Only the bytes this replaces: a keystroke in a long draft copies none of the rest.
        let before = self.ranged(a, b);
        let inserted_marks: Vec<Mark> = if s.is_empty() {
            self.typing = Typing::Inherited;
            Vec::new()
        } else if let Some(p) = self.typing.take_at((self.anchor, self.focus)) {
            p
        } else if a < b {
            // Replacing a selection inherits marks that covered the first character of the selection.
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

        self.apply_text_and_attr_replace((a, b), s, &inserted_marks, None, false);
        self.record_ranged(before, s, kind);
    }
}
