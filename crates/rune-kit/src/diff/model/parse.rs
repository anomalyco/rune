//! Parsing unified patches (`git diff`, `git show`, `git format-patch`, `diff -u`) into
//! [`FileDiff`] values.

use super::{FileDiff, FileStatus, Hunk, Line, LineKind, Side};
use crate::diff::pairs;

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
        pairs::pair_lines(&mut f.hunks);
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
