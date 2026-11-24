//! Virtual row layout, row keying, line lookup, gap discovery, and chunked syntax highlighting.

use std::collections::HashMap;
use std::rc::Rc;

use super::{DiffMode, DiffState, Gap, Row};
use crate::diff::model::{FileDiff, LineKind, Side};
use crate::diff::pairs::SplitRow;
use crate::syntax::Highlight;

/// Highlights of 128 lines of one side, by line number.
pub(super) struct Chunk {
    /// Byte offset of each line in the chunk's text, and one past the last.
    starts: Vec<usize>,
    hl: Rc<[Highlight]>,
}

const CHUNK: u32 = 128;

impl DiffState {
    /// Line `n` of `side` of file `f`, from its whole text, its hunks or an unchanged range.
    pub(crate) fn side_line(&self, f: usize, side: Side, n: u32) -> Option<&str> {
        let file = self.files.get(f)?;
        if let Some(text) = file.side_text_line(side, n) {
            return Some(text);
        }
        let hunks = &file.hunks;
        let line_in = |h: usize| -> Option<&str> {
            let hunk = hunks.get(h)?;
            let off = u64::from(n).checked_sub(hunk.first_u64(side))?;
            let &i = self.side_lines[f][h][side as usize].get(usize::try_from(off).ok()?)?;
            hunk.lines.get(i as usize).map(|l| l.text.as_str())
        };
        let n64 = u64::from(n);
        let h = hunks.partition_point(|h| h.end_u64(side) <= n64);
        if let Some(text) = line_in(h) {
            return Some(text);
        }
        if !self.hunks_ordered[f][side as usize] {
            for idx in 0..hunks.len() {
                if let Some(text) = line_in(idx) {
                    return Some(text);
                }
            }
        }
        let g = self.gaps[f].get(h)?;
        let start = if side == Side::Old { g.old } else { g.new };
        let off = n.checked_sub(start)?;
        if g.len.is_some_and(|len| off >= len) {
            return None;
        }
        file.context_line(g.old.checked_add(off)?, g.new.checked_add(off)?)
    }

    /// Colour-only highlights of line `n` of `side` of file `f`, relative to the line.
    pub(crate) fn highlights(&self, f: usize, side: Side, n: u32) -> Vec<(std::ops::Range<usize>, crate::syntax::Paint)> {
        let Some(file) = self.files.get(f) else { return Vec::new() };
        if n == 0 {
            return Vec::new();
        }
        let lang = file.language();
        if self.highlighter.is_builtin() && !lang.is_some_and(crate::syntax::known) {
            return Vec::new();
        }
        let c = (n - 1) / CHUNK;
        let key = (f as u32, side, c);
        let chunk = self.chunks.borrow().get(&key).cloned();
        let chunk = chunk.unwrap_or_else(|| {
            let mut text = String::new();
            let mut starts = Vec::with_capacity(CHUNK as usize + 1);
            let base = c * CHUNK + 1;
            for i in 0..CHUNK {
                starts.push(text.len());
                if let Some(m) = base.checked_add(i) {
                    text.push_str(self.side_line(f, side, m).unwrap_or(""));
                }
                text.push('\n');
            }
            starts.push(text.len());
            let hl = self.highlighter.highlight(lang, &text);
            let chunk = Rc::new(Chunk { starts, hl });
            let mut cache = self.chunks.borrow_mut();
            if cache.len() > 512 {
                cache.clear();
            }
            cache.insert(key, chunk.clone());
            chunk
        });
        let k = ((n - 1) % CHUNK) as usize;
        let (a, b) = (chunk.starts[k], chunk.starts[k + 1] - 1);
        let first = chunk.hl.partition_point(|(r, _)| r.end <= a);
        chunk.hl[first..]
            .iter()
            .take_while(|(r, _)| r.start < b)
            .map(|(r, p)| (r.start.max(a).saturating_sub(a)..r.end.min(b).saturating_sub(a), *p))
            .filter(|(r, _)| !r.is_empty())
            .collect()
    }

    /// Colour-only highlights of `text` at line `n` of `side` of file `f`. Uses the cached
    /// chunk when `side_line` holds `text`, and highlights `text` alone when colliding or
    /// saturated line numbers map `n` to a different line.
    pub(crate) fn highlights_for(&self, f: usize, side: Side, n: u32, text: &str) -> Vec<(std::ops::Range<usize>, crate::syntax::Paint)> {
        if n > 0 && self.side_line(f, side, n) == Some(text) {
            return self.highlights(f, side, n);
        }
        let Some(file) = self.files.get(f) else { return Vec::new() };
        let lang = file.language();
        if self.highlighter.is_builtin() && !lang.is_some_and(crate::syntax::known) {
            return Vec::new();
        }
        self.highlighter.highlight(lang, text).iter().cloned().collect()
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
                let top = g.len.map_or(top, |n| top.min(n));
                let bottom = g.len.map_or(0, |n| bottom.min(n.saturating_sub(top)));
                let ctx = |k: u32| Row::Ctx { f: fu, old: g.old.saturating_add(k), new: g.new.saturating_add(k) };
                rows.extend((0..top).map(ctx));
                let hidden = g.len.map(|n| n.saturating_sub(top.saturating_add(bottom)));
                let first_row = rows.len() as u32;
                if hidden.is_some_and(|n| n > 0) {
                    rows.push(Row::Gap { f: fu, gap: gap as u32 });
                }
                if let Some(n) = g.len {
                    rows.extend((n - bottom..n).map(ctx));
                }
                let Some(h) = file.hunks.get(gap) else { break };
                let hu = gap as u32;
                match self.mode {
                    DiffMode::Split => rows.extend(h.split_rows().map(|r| match r {
                        SplitRow::Pair { del, add } => Row::Pair { f: fu, h: hu, del: del as u32, add: add as u32 },
                        SplitRow::Line(i) => Row::Line { f: fu, h: hu, i: i as u32 },
                    })),
                    DiffMode::Unified => rows.extend((0..h.lines.len() as u32).map(|i| Row::Line { f: fu, h: hu, i })),
                }
                if rows.len() > first_row as usize {
                    self.hunk_rows.push(first_row);
                }
            }
        }
        self.row_of.clear();
        let mut seen: HashMap<(u32, u8, u32), u32> = HashMap::with_capacity(rows.len());
        let keys: Vec<String> = rows
            .iter()
            .map(|&r| {
                let slot = self.key_slot(r);
                let count = seen.entry(slot).or_insert(0);
                *count += 1;
                let base = key_from_slot(slot);
                if *count == 1 { base } else { format!("{base}#{}", *count - 1) }
            })
            .collect();
        if !self.visible_rows.is_empty() {
            if let Some(top) = self.keys.get(self.visible_rows.start)
                && let Some(start) = keys.iter().position(|k| k == top)
            {
                let len = self.visible_rows.len();
                self.visible_rows = start..start.saturating_add(len).min(rows.len());
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

    /// A row's identity tuple: identical for an unchanged or removed line in both modes, so
    /// switching moves it rather than replacing it.
    fn key_slot(&self, r: Row) -> (u32, u8, u32) {
        match r {
            Row::File(f) => (f, 0, 0),
            Row::Note(f) => (f, 1, 0),
            Row::Gap { f, gap } => (f, 2, gap),
            Row::Ctx { f, old, .. } => (f, 3, old),
            Row::Line { f, h, i } => {
                let l = &self.files[f as usize].hunks[h as usize].lines[i as usize];
                match l.kind {
                    LineKind::Context => (f, 3, l.old.unwrap_or(0)),
                    LineKind::Removed => (f, 4, l.old.unwrap_or(0)),
                    LineKind::Added => (f, 5, l.new.unwrap_or(0)),
                }
            }
            Row::Pair { f, h, del, .. } => (f, 4, self.files[f as usize].hunks[h as usize].lines[del as usize].old.unwrap_or(0)),
        }
    }
}

fn key_from_slot((f, kind, n): (u32, u8, u32)) -> String {
    match kind {
        0 => format!("df{f}"),
        1 => format!("df{f}.note"),
        2 => format!("df{f}.g{n}"),
        3 => format!("df{f}.c{n}"),
        4 => format!("df{f}.o{n}"),
        _ => format!("df{f}.n{n}"),
    }
}

/// The unchanged ranges before each hunk of `f` and after its last.
pub(super) fn gaps(f: &FileDiff) -> Vec<Gap> {
    let mut out = Vec::with_capacity(f.hunks.len() + 1);
    let max_end = f.old_len().map(|n| u64::from(n) + 1);
    let clamp = |v: u64| v.min(u64::from(u32::MAX)) as u32;
    let (mut old, mut new) = (1u64, 1u64);
    for h in &f.hunks {
        let first = h.first_u64(Side::Old);
        let target = max_end.map_or(first, |limit| first.min(limit));
        out.push(Gap { old: clamp(old), new: clamp(new), len: Some(clamp(target.saturating_sub(old))) });
        old = h.end_u64(Side::Old);
        new = h.end_u64(Side::New);
    }
    let len = max_end.map(|limit| clamp(limit.saturating_sub(old)));
    out.push(Gap { old: clamp(old), new: clamp(new), len: if f.hunks.is_empty() { Some(0) } else { len } });
    out
}
