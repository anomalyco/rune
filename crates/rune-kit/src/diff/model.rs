//! What a diff is: files of hunks of lines, parsed from a patch or computed from two texts.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use super::myers::{self, Op};

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
    /// changes), when it pairs with one and they share enough to say.
    pub changes: Vec<Range<usize>>,
    /// The file ends here without a line break (`\ No newline at end of file`).
    pub no_newline: bool,
}

impl Line {
    fn new(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> Line {
        Line { kind, old, new, text: text.to_owned(), changes: Vec::new(), no_newline: false }
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

    /// One past the last line number the hunk covers on `side`.
    pub(crate) fn end(&self, side: Side) -> u32 {
        self.first(side).saturating_add(if side == Side::Old { self.old_count } else { self.new_count })
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
        let path = path.into();
        FileDiff::between(Some(path.clone()), Some(path), old, new)
    }

    /// [`FileDiff::from_texts`] for a file whose path changed (a rename), was added (`old_path`
    /// `None`, `old` empty) or deleted (`new_path` `None`, `new` empty).
    pub fn between(old_path: Option<String>, new_path: Option<String>, old: &str, new: &str) -> FileDiff {
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
        f.hunks = hunks(&a, &b);
        f.old_text = Some(a);
        f.new_text = Some(b);
        f.pair_words();
        f
    }

    /// Attach the whole old and new texts to a parsed diff, so the unchanged ranges between its
    /// hunks can be expanded. One side is enough (unchanged lines read the same on both);
    /// the trailing range after the last hunk needs the side it counts lines on.
    pub fn with_texts(mut self, old: Option<&str>, new: Option<&str>) -> FileDiff {
        self.old_text = old.map(|t| Rc::new(Source::new(t)));
        self.new_text = new.map(|t| Rc::new(Source::new(t)));
        self
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

    /// The highlighter language for the path's extension (or name, for a `Makefile`), as
    /// written: `"rs"`, `"tsx"`.
    pub fn language(&self) -> Option<&str> {
        let name = self.path().rsplit('/').next()?;
        match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => Some(ext),
            _ => Some(name).filter(|n| matches!(*n, "Makefile" | "Dockerfile")),
        }
    }

    /// Whether the unchanged lines between hunks can be shown.
    pub fn expandable(&self) -> bool {
        self.old_text.is_some() || self.new_text.is_some()
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
        Some((i64::from(t.len()) - added + removed).max(0) as u32)
    }

    /// The widest line it can show, in columns with tabs stopping every four.
    pub(crate) fn columns(&self) -> u32 {
        let hunks = self.hunks.iter().flat_map(|h| &h.lines).map(|l| columns(&l.text));
        let texts = [&self.old_text, &self.new_text].into_iter().flatten().flat_map(|t| (1..=t.len()).filter_map(|n| t.line(n))).map(columns);
        hunks.chain(texts).max().unwrap_or(0)
    }

    /// The largest line number either side shows.
    pub(crate) fn last_line(&self) -> u32 {
        let hunks = self.hunks.last().map_or(0, |h| h.end(Side::Old).max(h.end(Side::New)).saturating_sub(1));
        let texts = [&self.old_text, &self.new_text].into_iter().flatten().map(|t| t.len()).max().unwrap_or(0);
        hunks.max(texts)
    }

    /// Word changes for each removed line followed by an added one, pairing them in order.
    fn pair_words(&mut self) {
        for h in &mut self.hunks {
            let mut i = 0;
            while i < h.lines.len() {
                let del = i;
                while i < h.lines.len() && h.lines[i].kind == LineKind::Removed {
                    i += 1;
                }
                let add = i;
                while i < h.lines.len() && h.lines[i].kind == LineKind::Added {
                    i += 1;
                }
                if add == del || i == add {
                    i = i.max(del + 1);
                    continue;
                }
                let pairs = (add - del).min(i - add);
                if pairs <= 128 {
                    for k in 0..pairs {
                        let (a, b) = myers::words(&h.lines[del + k].text, &h.lines[add + k].text);
                        h.lines[del + k].changes = a;
                        h.lines[add + k].changes = b;
                    }
                }
            }
        }
    }
}

/// Columns `line` takes with tabs stopping every four.
pub(crate) fn columns(line: &str) -> u32 {
    line.chars().fold(0, |col, c| if c == '\t' { col + 4 - col % 4 } else { col + 1 })
}

/// Hunks of `old` to `new` with [`CONTEXT`] lines around each change, merged when their
/// context would touch.
fn hunks(old: &Source, new: &Source) -> Vec<Hunk> {
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
            Some((g, last)) if c.0.start - last.0.end <= 2 * CONTEXT => g.push(c),
            _ => groups.push(vec![c]),
        }
    }
    let line = |s: &Source, n: u32| s.line(n + 1).unwrap_or("").to_owned();
    groups
        .into_iter()
        .map(|g| {
            let (first, last) = (&g[0], &g[g.len() - 1]);
            let start = first.0.start.saturating_sub(CONTEXT);
            let shift = first.0.start - start;
            let (os, ns) = (start, first.1.start - shift);
            let end_old = (last.0.end + CONTEXT).min(old.len());
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

/// Every file in a unified diff, as `git diff`, `git show`, `git format-patch` or `diff -u`
/// write them: renames, copies, mode changes, added and deleted files, binary files and
/// `\ No newline at end of file` included. Text around the diffs (a commit message, a mail
/// header) is skipped. Word changes are paired as [`FileDiff::from_texts`] pairs them.
pub fn parse(patch: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let lines: Vec<&str> = patch.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let mut i = 0;
    let mut cur: Option<FileDiff> = None;
    // In a `diff --git` section, before its first hunk.
    let mut in_header = false;
    while i < lines.len() {
        let l = lines[i];
        if let Some(rest) = l.strip_prefix("diff --git ") {
            files.extend(cur.take());
            let (a, b) = git_paths(rest);
            cur = Some(FileDiff::empty(a, b));
            in_header = true;
            i += 1;
            continue;
        }
        if l.starts_with("--- ") && lines.get(i + 1).is_some_and(|n| n.starts_with("+++ ")) && lines.get(i + 2).is_some_and(|n| n.starts_with("@@ ")) {
            let (a, b) = (path_of(&l[4..]), path_of(&lines[i + 1][4..]));
            match cur.as_mut() {
                Some(f) if in_header => {
                    // These name the paths unambiguously (and `/dev/null` for none).
                    f.old_path = a;
                    f.new_path = b;
                }
                _ => {
                    files.extend(cur.take());
                    cur = Some(FileDiff::empty(a, b));
                }
            }
            in_header = false;
            i += 2;
            continue;
        }
        if let Some(f) = cur.as_mut() {
            if l.starts_with("@@ ") {
                in_header = false;
                if let Some(h) = hunk_header(l) {
                    let (h, next) = hunk_body(h, &lines, i + 1);
                    f.hunks.push(h);
                    i = next;
                    continue;
                }
            } else if in_header {
                header_line(f, l);
            }
        }
        i += 1;
    }
    files.extend(cur);
    for f in &mut files {
        if f.status == FileStatus::Modified {
            f.status = match (&f.old_path, &f.new_path) {
                (None, Some(_)) => FileStatus::Added,
                (Some(_), None) => FileStatus::Deleted,
                (Some(a), Some(b)) if a != b => FileStatus::Renamed,
                _ => FileStatus::Modified,
            };
        }
        f.pair_words();
    }
    files
}

fn header_line(f: &mut FileDiff, l: &str) {
    let word = |p: &str| l.strip_prefix(p).map(str::trim);
    if let Some(m) = word("old mode ") {
        f.old_mode = Some(m.to_owned());
    } else if let Some(m) = word("new mode ") {
        f.new_mode = Some(m.to_owned());
    } else if let Some(m) = word("new file mode ") {
        f.new_mode = Some(m.to_owned());
        f.status = FileStatus::Added;
        f.old_path = None;
    } else if let Some(m) = word("deleted file mode ") {
        f.old_mode = Some(m.to_owned());
        f.status = FileStatus::Deleted;
        f.new_path = None;
    } else if let Some(p) = word("rename from ") {
        f.old_path = Some(unquote(p));
        f.status = FileStatus::Renamed;
    } else if let Some(p) = word("rename to ") {
        f.new_path = Some(unquote(p));
        f.status = FileStatus::Renamed;
    } else if let Some(p) = word("copy from ") {
        f.old_path = Some(unquote(p));
        f.status = FileStatus::Copied;
    } else if let Some(p) = word("copy to ") {
        f.new_path = Some(unquote(p));
        f.status = FileStatus::Copied;
    } else if let Some(s) = word("similarity index ") {
        f.similarity = s.trim_end_matches('%').parse().ok();
    } else if let Some(rest) = word("index ") {
        if let Some((_, mode)) = rest.split_once(' ') {
            f.old_mode.get_or_insert_with(|| mode.to_owned());
            f.new_mode.get_or_insert_with(|| mode.to_owned());
        }
    } else if l.starts_with("Binary files ") || l == "GIT binary patch" {
        f.binary = true;
    }
}

/// The two paths of a `diff --git a/x b/y` line. With unquoted paths containing spaces the
/// split is ambiguous; the `---`/`+++` or rename lines that follow correct it.
fn git_paths(rest: &str) -> (Option<String>, Option<String>) {
    if rest.starts_with('"')
        && let Some((a, b)) = split_quoted(rest)
    {
        return (Some(strip_prefix(&a)), Some(strip_prefix(&b)));
    }
    // Equal paths split in the middle; otherwise at the first " b/".
    let half = rest.len() / 2;
    if rest.len() % 2 == 1
        && rest.as_bytes().get(half) == Some(&b' ')
        && rest[..half].strip_prefix("a/").zip(rest[half + 1..].strip_prefix("b/")).is_some_and(|(a, b)| a == b)
    {
        return (Some(strip_prefix(&rest[..half])), Some(strip_prefix(&rest[half + 1..])));
    }
    match rest.find(" b/") {
        Some(at) => (Some(strip_prefix(&rest[..at])), Some(strip_prefix(&rest[at + 1..]))),
        None => (None, None),
    }
}

fn split_quoted(rest: &str) -> Option<(String, String)> {
    let end = quoted_end(rest)?;
    let a = unquote(&rest[..end]);
    let tail = rest[end..].trim_start();
    Some((a, unquote(tail)))
}

/// The byte after the closing quote of the quoted string `s` starts with.
fn quoted_end(s: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in s.char_indices().skip(1) {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// A path as git writes it: bare, or in double quotes with C escapes (octal bytes for
/// non-ASCII).
fn unquote(p: &str) -> String {
    let Some(inner) = p.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else { return p.to_owned() };
    let mut bytes = Vec::new();
    let mut it = inner.bytes().peekable();
    while let Some(c) = it.next() {
        if c != b'\\' {
            bytes.push(c);
            continue;
        }
        match it.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(d @ b'0'..=b'7') => {
                let mut v = u32::from(d - b'0');
                for _ in 0..2 {
                    match it.peek() {
                        Some(&e @ b'0'..=b'7') => {
                            v = v * 8 + u32::from(e - b'0');
                            it.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(v as u8);
            }
            Some(other) => bytes.push(other),
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn strip_prefix(p: &str) -> String {
    let p = unquote(p);
    match p.get(..2) {
        Some("a/" | "b/") => p[2..].to_owned(),
        _ => p,
    }
}

/// The path of a `---` or `+++` line: `None` for `/dev/null`, without a trailing timestamp.
fn path_of(s: &str) -> Option<String> {
    let s = if s.starts_with('"') { s } else { s.split('\t').next().unwrap_or(s) };
    let s = s.trim_end();
    if s == "/dev/null" {
        return None;
    }
    Some(strip_prefix(s))
}

fn hunk_header(l: &str) -> Option<Hunk> {
    let rest = l.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(u32, u32)> {
        match r.split_once(',') {
            Some((s, c)) => Some((s.parse().ok()?, c.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let ((os, oc), (ns, nc)) = (range(old)?, range(new)?);
    Some(Hunk { old_start: os, old_count: oc, new_start: ns, new_count: nc, section: section.trim().to_owned(), lines: Vec::new() })
}

/// The lines of hunk `h` from `lines[i..]`, read until its counts are used up: the index of
/// the line after it.
fn hunk_body(mut h: Hunk, lines: &[&str], mut i: usize) -> (Hunk, usize) {
    let (mut old, mut new) = (h.first(Side::Old), h.first(Side::New));
    let (mut old_left, mut new_left) = (h.old_count, h.new_count);
    while i < lines.len() {
        let l = lines[i];
        if l.starts_with('\\') {
            if let Some(last) = h.lines.last_mut() {
                last.no_newline = true;
            }
            i += 1;
            continue;
        }
        if old_left == 0 && new_left == 0 {
            break;
        }
        let (kind, text) = match l.as_bytes().first() {
            Some(b' ') => (LineKind::Context, &l[1..]),
            // A context line whose space an editor trimmed.
            None if i + 1 < lines.len() => (LineKind::Context, ""),
            Some(b'-') if old_left > 0 => (LineKind::Removed, &l[1..]),
            Some(b'+') if new_left > 0 => (LineKind::Added, &l[1..]),
            _ => break,
        };
        let line = match kind {
            LineKind::Context if old_left > 0 && new_left > 0 => {
                old_left -= 1;
                new_left -= 1;
                let (o, n) = (old, new);
                old = old.saturating_add(1);
                new = new.saturating_add(1);
                Line::new(kind, Some(o), Some(n), text)
            }
            LineKind::Context => break,
            LineKind::Removed => {
                old_left -= 1;
                let o = old;
                old = old.saturating_add(1);
                Line::new(kind, Some(o), None, text)
            }
            LineKind::Added => {
                new_left -= 1;
                let n = new;
                new = new.saturating_add(1);
                Line::new(kind, None, Some(n), text)
            }
        };
        h.lines.push(line);
        i += 1;
    }
    h.old_count -= old_left;
    h.new_count -= new_left;
    (h, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_make_hunks_with_three_lines_of_context() {
        let old: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let new = old.replace("line 5\n", "line five\n").replace("line 17\n", "").replace("line 20\n", "line 20\nline 21\n");
        let f = FileDiff::from_texts("a.txt", &old, &new);
        let headers: Vec<String> = f.hunks.iter().map(Hunk::header).collect();
        assert_eq!(headers, vec!["@@ -2,7 +2,7 @@ line 1", "@@ -14,7 +14,7 @@ line 13"]);
        let second: Vec<(LineKind, Option<u32>, Option<u32>)> = f.hunks[1].lines.iter().map(|l| (l.kind, l.old, l.new)).collect();
        assert_eq!(second[3], (LineKind::Removed, Some(17), None));
        assert_eq!(second.last(), Some(&(LineKind::Added, None, Some(20))));
        assert_eq!((f.additions(), f.deletions()), (2, 2));
        let five = &f.hunks[0].lines;
        let added = five.iter().find(|l| l.kind == LineKind::Added).unwrap();
        assert_eq!(&added.text[added.changes[0].clone()], "five");
    }

    #[test]
    fn a_missing_final_newline_is_a_change() {
        let f = FileDiff::from_texts("a", "one\ntwo", "one\ntwo\n");
        let l: Vec<(LineKind, &str, bool)> = f.hunks[0].lines.iter().map(|l| (l.kind, l.text.as_str(), l.no_newline)).collect();
        assert_eq!(l, vec![(LineKind::Context, "one", false), (LineKind::Removed, "two", true), (LineKind::Added, "two", false)]);
        let both_missing = FileDiff::from_texts("a", "one\ntwo", "one\nthree");
        let l2: Vec<(LineKind, &str, bool)> = both_missing.hunks[0].lines.iter().map(|l| (l.kind, l.text.as_str(), l.no_newline)).collect();
        assert_eq!(l2, vec![(LineKind::Context, "one", false), (LineKind::Removed, "two", true), (LineKind::Added, "three", true)]);
        assert!(FileDiff::from_texts("a", "same\n", "same\n").hunks.is_empty());
        assert!(FileDiff::from_texts("a", "same", "same").hunks.is_empty());
        // CRLF normalizes line endings while preserving changes.
        assert!(FileDiff::from_texts("a", "one\r\ntwo\r\n", "one\ntwo\n").hunks.is_empty());
        let crlf = FileDiff::from_texts("a", "one\r\ntwo\r\n", "one\r\nthree\r\n");
        assert_eq!(crlf.hunks[0].lines[1].text, "two");
        assert_eq!(crlf.hunks[0].lines[2].text, "three");
        // NUL bytes mark a binary file.
        let bin = FileDiff::from_texts("img.bin", "a\0b", "c\0d");
        assert!(bin.binary && bin.hunks.is_empty());
    }

    #[test]
    fn hostile_and_fuzzed_patches_never_panic_or_blow_up() {
        let cases = [
            "",
            "diff --git\n--- \n+++ \n@@ -4294967295,4294967295 +4294967295,4294967295 @@\n-a\n+b\n\\ No newline",
            "diff --git \"a/unclosed b/unclosed\n--- \"a/\\777\\x\n+++ b/\n@@ -0,0 +0,0 @@\n",
            "diff --git 🚀 a/x 🚀 b/y\n@@ -1,999999 +1,999999 @@\n+truncated",
            "--- a/f\n+++ b/f\n@@ -10,2 +10,2 @@\n ctx\n-old\n+new\n@@ -2,1 +2,1 @@\n-out_of_order\n+ok\n",
        ];
        for patch in cases {
            let files = parse(patch);
            let _ = crate::diff::DiffState::new(files);
        }

        // Fuzz-style random line soup of headers, hunks, escapes and markers.
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed as usize) % n
        };
        let fragments = [
            "diff --git a/f.rs b/f.rs\n",
            "diff --git \"a/sp ace\\303\\251\" \"b/sp ace\\n\"\n",
            "--- a/f.rs\n",
            "+++ b/f.rs\n",
            "--- /dev/null\n",
            "+++ /dev/null\n",
            "@@ -1,3 +1,3 @@ fn f()\n",
            "@@ -0,0 +1,5 @@\n",
            "@@ -4294967295,10 +0,4294967295 @@\n",
            "@@ malformed @@\n",
            " context line\n",
            "-removed line\n",
            "+added line\n",
            "\\ No newline at end of file\n",
            "rename from \"a/\\123\"\n",
            "similarity index 999%\n",
            "Binary files a/x and b/x differ\n",
            "\r\n",
        ];
        for _ in 0..300 {
            let len = 1 + next(24);
            let patch: String = (0..len).map(|_| fragments[next(fragments.len())]).collect();
            let files = parse(&patch);
            let mut state = crate::diff::DiffState::new(files);
            state.set_mode(crate::diff::DiffMode::Split);
            for f in 0..state.files().len() {
                let _ = state.side_line(f, Side::Old, 1);
                let _ = state.side_line(f, Side::New, 1);
            }
        }
    }

    #[test]
    fn an_added_file_and_an_insertion_count_from_zero() {
        let f = FileDiff::between(None, Some("new.rs".into()), "", "fn main() {}\n");
        assert_eq!((f.status, f.hunks[0].header()), (FileStatus::Added, "@@ -0,0 +1 @@".to_owned()));
        let reparsed = parse("--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+fn main() {}\n");
        assert_eq!(reparsed[0].hunks[0].lines, f.hunks[0].lines);
        assert_eq!(reparsed[0].status, FileStatus::Added);
    }

    #[test]
    fn languages_come_from_the_extension() {
        let f = |p: &str| FileDiff::from_texts(p, "", "x").language().map(str::to_owned);
        assert_eq!(f("src/lib.rs").as_deref(), Some("rs"));
        assert_eq!(f("web/App.tsx").as_deref(), Some("tsx"));
        assert_eq!(f(".gitignore"), None);
        assert_eq!(f("Makefile").as_deref(), Some("Makefile"));
    }
}
