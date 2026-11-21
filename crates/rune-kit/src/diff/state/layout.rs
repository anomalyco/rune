//! Virtual row layout, row keying, line lookup, gap discovery, and chunked syntax highlighting.

use std::rc::Rc;

use super::{DiffMode, DiffState, Gap, Row};
use crate::diff::model::{FileDiff, LineKind, Side};
use crate::diff::pairs::SplitRow;
use crate::syntax::Highlight;

/// Highlights of 128 lines of one side, by line number.
pub(super) struct Chunk {
    /// Byte offset of each line in the chunk's text, and one past the last.
    starts: Vec<u32>,
    hl: Rc<[Highlight]>,
}

const CHUNK: u32 = 128;

impl DiffState {
    /// Line `n` of `side` of file `f`, from its whole text, its hunks or an unchanged range.
    pub(crate) fn side_line(&self, f: usize, side: Side, n: u32) -> Option<&str> {
        let file = &self.files[f];
        let hunks = &file.hunks;
        let h = hunks.partition_point(|h| h.end(side) <= n);
        if let Some(hunk) = hunks.get(h).filter(|h| h.first(side) <= n) {
            let i = self.side_lines[f][h][side as usize].get((n - hunk.first(side)) as usize)?;
            return Some(&hunk.lines[*i as usize].text);
        }
        let g = self.gaps[f][h];
        let off = n.checked_sub(if side == Side::Old { g.old } else { g.new })?;
        file.context_line(g.old.saturating_add(off), g.new.saturating_add(off))
    }

    /// Colour-only highlights of line `n` of `side` of file `f`, relative to the line.
    pub(crate) fn highlights(&self, f: usize, side: Side, n: u32) -> Vec<(std::ops::Range<usize>, crate::syntax::Paint)> {
        let lang = self.files[f].language();
        if self.highlighter.is_builtin() && !lang.is_some_and(crate::syntax::known) {
            return Vec::new();
        }
        let c = n.saturating_sub(1) / CHUNK;
        let key = (f as u32, side, c);
        let chunk = self.chunks.borrow().get(&key).cloned();
        let chunk = chunk.unwrap_or_else(|| {
            let mut text = String::new();
            let mut starts = Vec::with_capacity(CHUNK as usize + 1);
            for m in c * CHUNK + 1..=(c + 1) * CHUNK {
                starts.push(text.len() as u32);
                text.push_str(self.side_line(f, side, m).unwrap_or(""));
                text.push('\n');
            }
            starts.push(text.len() as u32);
            let hl = self.highlighter.highlight(lang, &text);
            let chunk = Rc::new(Chunk { starts, hl });
            let mut cache = self.chunks.borrow_mut();
            if cache.len() > 512 {
                cache.clear();
            }
            cache.insert(key, chunk.clone());
            chunk
        });
        let k = (n.saturating_sub(1) % CHUNK) as usize;
        let (a, b) = (chunk.starts[k] as usize, chunk.starts[k + 1] as usize - 1);
        let first = chunk.hl.partition_point(|(r, _)| r.end <= a);
        chunk.hl[first..]
            .iter()
            .take_while(|(r, _)| r.start < b)
            .map(|(r, p)| (r.start.max(a) - a..r.end.min(b) - a, *p))
            .filter(|(r, _)| !r.is_empty())
            .collect()
    }

    /// Row `r`'s line number on `side`.
    pub(crate) fn row_line(&self, r: Row, side: Side) -> Option<u32> {
        match r {
            Row::Ctx { old, new, .. } => Some(if side == Side::Old { old } else { new }),
            Row::Line { f, h, i } => self.files[f as usize].hunks[h as usize].lines[i as usize].number(side),
            Row::Pair { f, h, del, add } => {
                let i = if side == Side::Old { del } else { add };
                self.files[f as usize].hunks[h as usize].lines[i as usize].number(side)
            }
            _ => None,
        }
    }

    /// Lay the rows out again from the files, modes and expanded ranges.
    pub(super) fn rebuild(&mut self) {
        self.rev += 1;
        self.nav = None;
        let mut rows = Vec::new();
        self.file_rows.clear();
        self.hunk_rows.clear();
        for (f, file) in self.files.iter().enumerate() {
            let fu = f as u32;
            self.file_rows.push(rows.len() as u32);
            rows.push(Row::File(fu));
            if self.ui[f].collapsed {
                continue;
            }
            if file.hunks.is_empty() {
                rows.push(Row::Note(fu));
                continue;
            }
            for gap in 0..=file.hunks.len() {
                let g = self.gaps[f][gap];
                let (top, bottom) = self.ui[f].revealed[gap];
                let ctx = |k: u32| Row::Ctx { f: fu, old: g.old + k, new: g.new + k };
                rows.extend((0..top).map(ctx));
                let hidden = g.len.map(|n| n.saturating_sub(top + bottom));
                let first_row = rows.len() as u32;
                if hidden.is_some_and(|n| n > 0) {
                    rows.push(Row::Gap { f: fu, gap: gap as u32 });
                }
                if let Some(n) = g.len {
                    rows.extend((n - bottom.min(n)..n).map(ctx));
                }
                let Some(h) = file.hunks.get(gap) else { break };
                self.hunk_rows.push(first_row);
                let hu = gap as u32;
                match self.mode {
                    DiffMode::Split => rows.extend(h.split_rows().map(|r| match r {
                        SplitRow::Pair { del, add } => Row::Pair { f: fu, h: hu, del: del as u32, add: add as u32 },
                        SplitRow::Line(i) => Row::Line { f: fu, h: hu, i: i as u32 },
                    })),
                    DiffMode::Unified => rows.extend((0..h.lines.len() as u32).map(|i| Row::Line { f: fu, h: hu, i })),
                }
            }
        }
        self.row_of.clear();
        let keys: Vec<String> = rows.iter().map(|&r| self.key_of(r)).collect();
        if !self.visible_rows.is_empty() {
            if let Some(top) = self.keys.get(self.visible_rows.start)
                && let Some(start) = keys.iter().position(|k| k == top)
            {
                let len = self.visible_rows.len();
                self.visible_rows = start..(start + len).min(rows.len());
            } else {
                self.visible_rows = 0..0;
            }
        }
        self.keys = keys;
        static KEYS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        self.keys_rev = KEYS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        for (i, &r) in rows.iter().enumerate() {
            for side in [Side::Old, Side::New] {
                if let Some(n) = self.row_line(r, side) {
                    self.row_of.insert((r.file() as u32, side, n), i as u32);
                }
            }
        }
        self.rows = rows;
    }

    /// A row's key: the same for an unchanged or removed line in both modes, so switching
    /// moves it rather than replacing it.
    fn key_of(&self, r: Row) -> String {
        match r {
            Row::File(f) => format!("df{f}"),
            Row::Note(f) => format!("df{f}.note"),
            Row::Gap { f, gap } => format!("df{f}.g{gap}"),
            Row::Ctx { f, old, .. } => format!("df{f}.c{old}"),
            Row::Line { f, h, i } => {
                let l = &self.files[f as usize].hunks[h as usize].lines[i as usize];
                match l.kind {
                    LineKind::Context => format!("df{f}.c{}", l.old.unwrap_or(0)),
                    LineKind::Removed => format!("df{f}.o{}", l.old.unwrap_or(0)),
                    LineKind::Added => format!("df{f}.n{}", l.new.unwrap_or(0)),
                }
            }
            Row::Pair { f, h, del, .. } => format!("df{f}.o{}", self.files[f as usize].hunks[h as usize].lines[del as usize].old.unwrap_or(0)),
        }
    }
}

/// The unchanged ranges before each hunk of `f` and after its last.
pub(super) fn gaps(f: &FileDiff) -> Vec<Gap> {
    let mut out = Vec::with_capacity(f.hunks.len() + 1);
    let (mut old, mut new) = (1, 1);
    for h in &f.hunks {
        let first = h.first(Side::Old);
        out.push(Gap { old, new, len: Some(first.saturating_sub(old)) });
        old = h.end(Side::Old);
        new = h.end(Side::New);
    }
    let len = f.old_len().map(|n| (n + 1).saturating_sub(old));
    out.push(Gap { old, new, len: if f.hunks.is_empty() { Some(0) } else { len } });
    out
}
