//! What a diff is: files of hunks of lines, parsed from a patch or computed from two texts.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use super::myers::{self, Op};
use super::pairs;

mod parse;

pub use parse::parse;

/// The two sides of a diff: the text before the change and after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Side {
    Old,
    New,
}

/// What a line of a hunk is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineKind {
    /// Unchanged, on both sides.
    Context,
    /// Only on the new side.
    Added,
    /// Only on the old side.
    Removed,
}

/// One line of a hunk.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Line {
    pub kind: LineKind,
    /// Its 1-based number on the old side, unless it was added.
    pub old: Option<u32>,
    /// Its 1-based number on the new side, unless it was removed.
    pub new: Option<u32>,
    /// The line without its line break.
    pub text: String,
    /// Byte ranges of `text` that differ from the line it replaced or was replaced by (word
    /// changes), when it pairs with one: as OpenCode Desktop marks them, one range from the first
    /// changed word to the last.
    pub changes: Vec<Range<usize>>,
    /// The index in its hunk's `lines` of the line this changed line pairs with: the added line
    /// a removed one sits beside in split view, or the removed line an added one does. Its word
    /// changes are against that line. Lines pair as OpenCode Desktop's do: in order within a
    /// run of changes, unless a run with more lines on one side matches better shifted.
    pub pair: Option<usize>,
    /// The file ends here without a line break (`\ No newline at end of file`).
    pub no_newline: bool,
}

impl Line {
    fn new(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> Line {
        Line { kind, old, new, text: text.to_owned(), changes: Vec::new(), pair: None, no_newline: false }
    }

    /// Its number on `side`.
    pub fn number(&self, side: Side) -> Option<u32> {
        match side {
            Side::Old => self.old,
            Side::New => self.new,
        }
    }
}

/// A run of changed lines with the unchanged lines around them.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Hunk {
    /// Where the hunk starts on the old side, as its `@@` header says: the line after which
    /// lines are inserted when `old_count` is 0.
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// The text after the header's closing `@@`, often the enclosing function.
    pub section: String,
    pub lines: Vec<Line>,
}

impl Hunk {
    /// The first line number the hunk covers on `side` (its start, or the line after it when
    /// it covers none there).
    pub(crate) fn first(&self, side: Side) -> u32 {
        let (start, count) = match side {
            Side::Old => (self.old_start, self.old_count),
            Side::New => (self.new_start, self.new_count),
        };
        if count == 0 { start.saturating_add(1) } else { start.max(1) }
    }

    /// The first line number the hunk covers on `side`, in `u64` so a hunk at `u32::MAX`
    /// never saturates before range checks.
    pub(crate) fn first_u64(&self, side: Side) -> u64 {
        let (start, count) = match side {
            Side::Old => (self.old_start, self.old_count),
            Side::New => (self.new_start, self.new_count),
        };
        if count == 0 { u64::from(start) + 1 } else { u64::from(start.max(1)) }
    }

    /// One past the last line number the hunk covers on `side`, in `u64` so a hunk at
    /// `u32::MAX` still covers line `u32::MAX`.
    pub(crate) fn end_u64(&self, side: Side) -> u64 {
        self.first_u64(side) + u64::from(if side == Side::Old { self.old_count } else { self.new_count })
    }

    /// The header line as a patch writes it, `@@ -1,4 +1,5 @@ section`.
    pub fn header(&self) -> String {
        let range = |start: u32, count: u32| if count == 1 { format!("{start}") } else { format!("{start},{count}") };
        let mut h = format!("@@ -{} +{} @@", range(self.old_start, self.old_count), range(self.new_start, self.new_count));
        if !self.section.is_empty() {
            h.push(' ');
            h.push_str(&self.section);
        }
        h
    }
}

/// What happened to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
}

/// One file's changes.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FileDiff {
    /// The path before the change; `None` for an added file.
    pub old_path: Option<String>,
    /// The path after the change; `None` for a deleted file.
    pub new_path: Option<String>,
    pub status: FileStatus,
    /// A rename's or copy's similarity, in percent.
    pub similarity: Option<u8>,
    /// File modes, as `100644`, when the patch gives them.
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    /// The patch says the file is binary, so it has no lines.
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    old_text: Option<Rc<Source>>,
    new_text: Option<Rc<Source>>,
}

impl FileDiff {
    /// The file's changes as a unified patch, as `git diff` writes one: the `diff --git` header,
    /// modes, rename and binary lines when there are any, then each hunk. `git apply` takes it,
    /// and it's what a chat or an issue wants of a diff dragged out of the app.
    pub fn patch(&self) -> String {
        let old = self.old_path.as_deref().or(self.new_path.as_deref()).unwrap_or_default();
        let new = self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or_default();
        let mut out = format!("diff --git a/{old} b/{new}\n");
        match self.status {
            FileStatus::Added => out.push_str(&format!("new file mode {}\n", self.new_mode.as_deref().unwrap_or("100644"))),
            FileStatus::Deleted => out.push_str(&format!("deleted file mode {}\n", self.old_mode.as_deref().unwrap_or("100644"))),
            FileStatus::Renamed | FileStatus::Copied => {
                let verb = if self.status == FileStatus::Renamed { "rename" } else { "copy" };
                if let Some(s) = self.similarity { out.push_str(&format!("similarity index {s}%\n")); }
                out.push_str(&format!("{verb} from {old}\n{verb} to {new}\n"));
            }
            FileStatus::Modified => {
                if let (Some(a), Some(b)) = (&self.old_mode, &self.new_mode) && a != b {
                    out.push_str(&format!("old mode {a}\nnew mode {b}\n"));
                }
            }
        }
        if self.binary {
            out.push_str(&format!("Binary files a/{old} and b/{new} differ\n"));
            return out;
        }
        if self.hunks.is_empty() { return out; }
        let from = if self.status == FileStatus::Added { "/dev/null".to_owned() } else { format!("a/{old}") };
        let to = if self.status == FileStatus::Deleted { "/dev/null".to_owned() } else { format!("b/{new}") };
        out.push_str(&format!("--- {from}\n+++ {to}\n"));
        for hunk in &self.hunks {
            out.push_str(&hunk.header());
            out.push('\n');
            for line in &hunk.lines {
                out.push(match line.kind { LineKind::Context => ' ', LineKind::Added => '+', LineKind::Removed => '-' });
                out.push_str(&line.text);
                out.push('\n');
                if line.no_newline { out.push_str("\\ No newline at end of file\n"); }
            }
        }
        out
    }
}

/// A whole text, split into lines.
#[derive(Debug)]
pub(crate) struct Source {
    text: String,
    /// Each line's byte range, without its line break.
    lines: Vec<(u32, u32)>,
}

impl Source {
    pub(crate) fn new(text: &str) -> Source {
        let mut lines = Vec::new();
        let mut start = 0;
        for (i, _) in text.match_indices('\n') {
            let end = if text[..i].ends_with('\r') { i - 1 } else { i };
            lines.push((start as u32, end as u32));
            start = i + 1;
        }
        if start < text.len() {
            let end = if text.ends_with('\r') { text.len() - 1 } else { text.len() };
            lines.push((start as u32, end as u32));
        }
        Source { text: text.to_owned(), lines }
    }

    /// Line `n` (1-based).
    pub(crate) fn line(&self, n: u32) -> Option<&str> {
        let &(a, b) = self.lines.get((n as usize).checked_sub(1)?)?;
        Some(&self.text[a as usize..b as usize])
    }

    pub(crate) fn len(&self) -> u32 {
        self.lines.len() as u32
    }

    fn ends_without_newline(&self) -> bool {
        !self.text.is_empty() && !self.text.ends_with('\n')
    }
}

/// Lines of context around each change in a diff computed from texts, as `git diff` keeps.
const CONTEXT: u32 = 3;

impl FileDiff {
    fn empty(old_path: Option<String>, new_path: Option<String>) -> FileDiff {
        FileDiff {
            old_path,
            new_path,
            status: FileStatus::Modified,
            similarity: None,
            old_mode: None,
            new_mode: None,
            binary: false,
            hunks: Vec::new(),
            old_text: None,
            new_text: None,
        }
    }

    /// The diff of `old` to `new`, a file at `path`, with three lines of context around each
    /// change and word changes on paired lines. Both texts stay with it, so every unchanged
    /// range between hunks can be expanded.
    pub fn from_texts(path: impl Into<String>, old: &str, new: &str) -> FileDiff {
        FileDiff::from_texts_with_context(path, old, new, CONTEXT)
    }

    /// [`FileDiff::from_texts`] with `context` unchanged lines around each change instead of
    /// three: four is what jsdiff (and so OpenCode Desktop) keeps.
    pub fn from_texts_with_context(path: impl Into<String>, old: &str, new: &str, context: u32) -> FileDiff {
        let path = path.into();
        FileDiff::between_with_context(Some(path.clone()), Some(path), old, new, context)
    }

    /// [`FileDiff::from_texts`] for a file whose path changed (a rename), was added (`old_path`
    /// `None`, `old` empty) or deleted (`new_path` `None`, `new` empty).
    pub fn between(old_path: Option<String>, new_path: Option<String>, old: &str, new: &str) -> FileDiff {
        FileDiff::between_with_context(old_path, new_path, old, new, CONTEXT)
    }

    /// [`FileDiff::between`] with `context` unchanged lines around each change.
    pub fn between_with_context(old_path: Option<String>, new_path: Option<String>, old: &str, new: &str, context: u32) -> FileDiff {
        let mut f = FileDiff::empty(old_path, new_path);
        f.status = match (&f.old_path, &f.new_path) {
            (None, _) => FileStatus::Added,
            (_, None) => FileStatus::Deleted,
            (Some(a), Some(b)) if a != b => FileStatus::Renamed,
            _ => FileStatus::Modified,
        };
        if old.contains('\0') || new.contains('\0') {
            f.binary = true;
            return f;
        }
        let (a, b) = (Rc::new(Source::new(old)), Rc::new(Source::new(new)));
        f.hunks = hunks(&a, &b, context);
        f.old_text = Some(a);
        f.new_text = Some(b);
        pairs::pair_lines(&mut f.hunks);
        f
    }

    /// The path to show: the new one, or the old one for a deleted file.
    pub fn path(&self) -> &str {
        self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or("")
    }

    /// Lines added.
    pub fn additions(&self) -> usize {
        self.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind == LineKind::Added).count()
    }

    /// Lines removed.
    pub fn deletions(&self) -> usize {
        self.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind == LineKind::Removed).count()
    }

    /// The highlighter language for the path, as Desktop names it from the file name
    /// ([`lang_for_path`](crate::syntax::lang_for_path)): `"rust"`, `"tsx"`, `"makefile"`.
    pub fn language(&self) -> Option<&str> {
        crate::syntax::lang_for_path(self.path())
    }

    /// Whether the unchanged lines between hunks can be shown.
    pub fn expandable(&self) -> bool {
        self.old_text.is_some() || self.new_text.is_some()
    }

    /// The only side the file has: `New` for an added file, `Old` for a deleted one, `None`
    /// when it has both. A file diffed from texts counts as added when its old text is empty
    /// and its new one isn't (and the other way round as deleted), unless it moved.
    pub fn sole_side(&self) -> Option<Side> {
        match self.status {
            FileStatus::Added => Some(Side::New),
            FileStatus::Deleted => Some(Side::Old),
            FileStatus::Modified => {
                let empty = |t: &Option<Rc<Source>>| t.as_ref().map(|t| t.text.is_empty());
                match (empty(&self.old_text), empty(&self.new_text)) {
                    (Some(true), Some(false)) => Some(Side::New),
                    (Some(false), Some(true)) => Some(Side::Old),
                    _ => None,
                }
            }
            FileStatus::Renamed | FileStatus::Copied => None,
        }
    }

    /// The whole text of `side`, when the diff has it ([`FileDiff::from_texts`],
    /// [`FileDiff::fold`]).
    pub fn text(&self, side: Side) -> Option<&str> {
        let t = match side {
            Side::Old => &self.old_text,
            Side::New => &self.new_text,
        };
        t.as_deref().map(|t| t.text.as_str())
    }

    /// A diff whose one hunk holds the whole file, as a patch made with full context does,
    /// folded as a diff of the two texts is: the hunk splits around its changes with `context`
    /// unchanged lines each side, and the texts it holds stay with the diff, so every folded
    /// range can be expanded. The lines pair as they did in the patch. A diff of any other shape
    /// (more than one hunk, or one that starts past the first line) comes back as it was.
    pub fn fold(mut self, context: u32) -> FileDiff {
        let [h] = self.hunks.as_slice() else { return self };
        if self.binary || h.first(Side::Old) > 1 || h.first(Side::New) > 1 {
            return self;
        }
        let text = |side: Side| {
            let kept = |l: &&Line| l.kind == LineKind::Context || l.kind == if side == Side::Old { LineKind::Removed } else { LineKind::Added };
            h.lines.iter().filter(kept).fold(String::new(), |mut s, l| {
                s.push_str(&l.text);
                if !l.no_newline {
                    s.push('\n');
                }
                s
            })
        };
        let (old, new) = (Rc::new(Source::new(&text(Side::Old))), Rc::new(Source::new(&text(Side::New))));
        let lines = std::mem::take(&mut self.hunks).pop().map(|h| h.lines).unwrap_or_default();
        let ranges = lines.iter().enumerate().filter(|(_, l)| l.kind != LineKind::Context).fold(Vec::<(usize, usize)>::new(), |mut out, (i, _)| {
            let (start, end) = (i.saturating_sub(context as usize), i.saturating_add(context as usize).min(lines.len() - 1));
            match out.last_mut() {
                Some(r) if start <= r.1.saturating_add(1) => r.1 = end,
                _ => out.push((start, end)),
            }
            out
        });
        let before = |side: Side, i: usize| lines[..i].iter().filter(|l| l.number(side).is_some()).count() as u32;
        self.hunks = ranges
            .into_iter()
            .map(|(start, end)| {
                let lines = lines[start..=end].to_vec();
                let count = |side: Side| lines.iter().filter(|l| l.number(side).is_some()).count() as u32;
                let (ob, nb, oc, nc) = (before(Side::Old, start), before(Side::New, start), count(Side::Old), count(Side::New));
                Hunk {
                    old_start: if oc == 0 { ob } else { ob.saturating_add(1) },
                    old_count: oc,
                    new_start: if nc == 0 { nb } else { nb.saturating_add(1) },
                    new_count: nc,
                    section: section(&old, ob),
                    lines,
                }
            })
            .collect();
        // A run of changes never splits across hunks, so pairing again pairs the same lines;
        // it only renumbers each line's partner within its new hunk.
        pairs::pair_lines(&mut self.hunks);
        self.old_text = Some(old);
        self.new_text = Some(new);
        self
    }

    /// Unchanged line `n` of the old side (with `new` its number on the new side), from
    /// whichever whole text the diff has.
    pub(crate) fn context_line(&self, old: u32, new: u32) -> Option<&str> {
        self.old_text.as_ref().and_then(|t| t.line(old)).or_else(|| self.new_text.as_ref().and_then(|t| t.line(new)))
    }

    /// How many lines the old side has, when the diff knows: from the old text, or the new
    /// text and the hunks' line counts.
    pub(crate) fn old_len(&self) -> Option<u32> {
        if let Some(t) = &self.old_text {
            return Some(t.len());
        }
        let t = self.new_text.as_ref()?;
        let (added, removed) = (self.additions() as i64, self.deletions() as i64);
        Some((i64::from(t.len()) - added + removed).clamp(0, i64::from(u32::MAX)) as u32)
    }

    /// The widest line it can show, in columns with tabs stopping every four.
    pub(crate) fn columns(&self) -> u32 {
        let hunks = self.hunks.iter().flat_map(|h| &h.lines).map(|l| columns(&l.text));
        let texts = [&self.old_text, &self.new_text].into_iter().flatten().flat_map(|t| (1..=t.len()).filter_map(|n| t.line(n))).map(columns);
        hunks.chain(texts).max().unwrap_or(0)
    }

    /// The largest line number either side shows.
    pub(crate) fn last_line(&self) -> u32 {
        let hunks = self
            .hunks
            .iter()
            .map(|h| {
                let end = h.end_u64(Side::Old).max(h.end_u64(Side::New)).saturating_sub(1).min(u64::from(u32::MAX)) as u32;
                let lines = h.lines.iter().filter_map(|l| l.old.max(l.new)).max().unwrap_or(0);
                end.max(lines)
            })
            .max()
            .unwrap_or(0);
        let texts = [&self.old_text, &self.new_text].into_iter().flatten().map(|t| t.len()).max().unwrap_or(0);
        hunks.max(texts)
    }
}

/// Columns `line` takes with tabs stopping every four.
pub(crate) fn columns(line: &str) -> u32 {
    line.chars().fold(0, |col, c| if c == '\t' { col + 4 - col % 4 } else { col + 1 })
}

/// Hunks of `old` to `new` with `context` lines around each change, merged when their
/// context would touch.
fn hunks(old: &Source, new: &Source, context: u32) -> Vec<Hunk> {
    // Intern lines; a last line without its break differs from the same text with one.
    let mut ids: HashMap<(&str, bool), u32> = HashMap::new();
    let (a, b) = (intern(old, &mut ids), intern(new, &mut ids));
    let ops = myers::diff(&a, &b);
    // Changes as (old range, new range), 0-based.
    let changes: Vec<(Range<u32>, Range<u32>)> = {
        let mut out: Vec<(Range<u32>, Range<u32>)> = Vec::new();
        let (mut i, mut j) = (0u32, 0u32);
        for op in &ops {
            match *op {
                Op::Equal { len, .. } => {
                    i += len as u32;
                    j += len as u32;
                }
                Op::Delete { len, .. } => {
                    match out.last_mut() {
                        Some(c) if c.0.end == i && c.1.end == j => c.0.end += len as u32,
                        _ => out.push((i..i + len as u32, j..j)),
                    }
                    i += len as u32;
                }
                Op::Insert { len, .. } => {
                    match out.last_mut() {
                        Some(c) if c.0.end == i && c.1.end == j => c.1.end += len as u32,
                        _ => out.push((i..i, j..j + len as u32)),
                    }
                    j += len as u32;
                }
            }
        }
        out
    };
    let mut groups: Vec<Vec<(Range<u32>, Range<u32>)>> = Vec::new();
    for c in changes {
        match groups.last_mut().and_then(|g| g.last().cloned().map(|l| (g, l))) {
            Some((g, last)) if c.0.start - last.0.end <= 2 * context => g.push(c),
            _ => groups.push(vec![c]),
        }
    }
    let line = |s: &Source, n: u32| s.line(n + 1).unwrap_or("").to_owned();
    groups
        .into_iter()
        .map(|g| {
            let (first, last) = (&g[0], &g[g.len() - 1]);
            let start = first.0.start.saturating_sub(context);
            let shift = first.0.start - start;
            let (os, ns) = (start, first.1.start - shift);
            let end_old = (last.0.end + context).min(old.len());
            let mut lines = Vec::new();
            let (mut i, mut j) = (os, ns);
            for c in &g {
                while i < c.0.start {
                    lines.push(Line { text: line(old, i), ..Line::new(LineKind::Context, Some(i + 1), Some(j + 1), "") });
                    i += 1;
                    j += 1;
                }
                for n in c.0.clone() {
                    lines.push(Line { text: line(old, n), ..Line::new(LineKind::Removed, Some(n + 1), None, "") });
                }
                for n in c.1.clone() {
                    lines.push(Line { text: line(new, n), ..Line::new(LineKind::Added, None, Some(n + 1), "") });
                }
                i = c.0.end;
                j = c.1.end;
            }
            while i < end_old {
                lines.push(Line { text: line(old, i), ..Line::new(LineKind::Context, Some(i + 1), Some(j + 1), "") });
                i += 1;
                j += 1;
            }
            for l in &mut lines {
                l.no_newline = match l.kind {
                    LineKind::Removed => l.old == Some(old.len()) && old.ends_without_newline(),
                    LineKind::Added => l.new == Some(new.len()) && new.ends_without_newline(),
                    LineKind::Context => l.old == Some(old.len()) && old.ends_without_newline(),
                };
            }
            let (oc, nc) = (i - os, j - ns);
            Hunk {
                old_start: if oc == 0 { os } else { os + 1 },
                old_count: oc,
                new_start: if nc == 0 { ns } else { ns + 1 },
                new_count: nc,
                section: section(old, os),
                lines,
            }
        })
        .collect()
}

fn intern<'a>(s: &'a Source, ids: &mut HashMap<(&'a str, bool), u32>) -> Vec<u32> {
    let last = s.len();
    (1..=last)
        .map(|n| {
            let key = (s.line(n).unwrap_or(""), n == last && s.ends_without_newline());
            let next = ids.len() as u32;
            *ids.entry(key).or_insert(next)
        })
        .collect()
}

/// The nearest line above 0-based line `before` that starts with a letter, `_` or `$` (a
/// function or type at the left margin), as `git diff` names a hunk by default.
fn section(s: &Source, before: u32) -> String {
    (1..=before.min(s.len()))
        .rev()
        .take(2000)
        .filter_map(|n| s.line(n))
        .find(|l| l.starts_with(|c: char| c.is_alphabetic() || c == '_' || c == '$'))
        .map(|l| l.trim_end().chars().take(80).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
