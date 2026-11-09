//! Word changes on a pair of lines as OpenCode Desktop marks them: Pierre's `word-line` mode
//! (`@pierre/diffs` `renderDiffWithHighlighter.js:176-190`, with `lineDiffType: "word-line"` and
//! `maxLineDiffLength: 1000` from session-ui `pierre/index.ts:314-315`). jsdiff's
//! `diffWordsWithSpace` (`diff` 9.0.0, `libesm/diff/word.js:267-281` over `libesm/diff/base.js`)
//! diffs the lines' words, and each side's change is one range from its first changed word to its
//! last (`pushWordLineDecorations`, `renderDiffWithHighlighter.js:146-175`), so "MAX_RETRIES = 3"
//! against "MAX_ATTEMPTS = 5" is one change, the unchanged " = " inside it.

use std::ops::Range;

/// Lines longer than this (in UTF-16 units, as JavaScript counts) get no word changes.
const MAX_LINE_DIFF_LENGTH: usize = 1000;

/// The byte range of `old` and of `new` that differs, each from its first changed word to its
/// last; `None` on a side nothing was removed from or added to.
pub(crate) fn words(old: &str, new: &str) -> (Option<Range<usize>>, Option<Range<usize>>) {
    let utf16 = |s: &str| s.encode_utf16().count();
    if utf16(old) > MAX_LINE_DIFF_LENGTH || utf16(new) > MAX_LINE_DIFF_LENGTH {
        return (None, None);
    }
    let (ta, tb) = (tokens(old), tokens(new));
    let sa: Vec<&str> = ta.iter().map(|r| &old[r.clone()]).collect();
    let sb: Vec<&str> = tb.iter().map(|r| &new[r.clone()]).collect();
    let (mut a, mut b) = (0, 0);
    let (mut ra, mut rb): (Option<Range<usize>>, Option<Range<usize>>) = (None, None);
    let span = |r: &mut Option<Range<usize>>, toks: &[Range<usize>], from: usize, to: usize| {
        let (start, end) = (toks[from].start, toks[to - 1].end);
        *r = Some(r.as_ref().map_or(start, |r| r.start)..end);
    };
    for c in diff(&sa, &sb) {
        match c.edit {
            Edit::Removed => {
                span(&mut ra, &ta, a, a + c.count);
                a += c.count;
            }
            Edit::Added => {
                span(&mut rb, &tb, b, b + c.count);
                b += c.count;
            }
            Edit::Kept => {
                a += c.count;
                b += c.count;
            }
        }
    }
    (ra, rb)
}

/// jsdiff's `WordsWithSpaceDiff.tokenize` (`word.js:268-276`): a line break, a run of word
/// characters, a run of other whitespace, or any other single character.
fn tokens(s: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq, Clone, Copy)]
    enum K {
        Word,
        Space,
    }
    let kind = |c: char| {
        if word_char(c) {
            Some(K::Word)
        } else if js_space(c) && c != '\n' && c != '\r' {
            Some(K::Space)
        } else {
            None
        }
    };
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut prev = None;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let k = kind(c);
        let end = i + c.len_utf8();
        if c == '\r' && chars.peek().is_some_and(|&(_, n)| n == '\n') {
            chars.next();
            out.push(i..end + 1);
            prev = None;
            continue;
        }
        match out.last_mut() {
            Some(last) if k.is_some() && prev == k => last.end = end,
            _ => out.push(i..end),
        }
        prev = k;
    }
    out
}

/// jsdiff's `extendedWordChars` (`word.js:23`).
fn word_char(c: char) -> bool {
    matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '\u{AD}' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}'
        | '\u{F8}'..='\u{2C6}' | '\u{2C8}'..='\u{2D7}' | '\u{2DE}'..='\u{2FF}' | '\u{1E00}'..='\u{1EFF}')
}

/// JavaScript's `\s`: WhiteSpace and LineTerminator (U+FEFF in, U+0085 out, unlike Rust's).
fn js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}'
        | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    Kept,
    Added,
    Removed,
}

/// A run of tokens one edit covers.
#[derive(Clone, Copy)]
struct Component {
    count: usize,
    edit: Edit,
}

/// A component in the shared list paths grow from, linked to the one before it.
struct Node {
    c: Component,
    prev: Option<usize>,
}

/// How far along a diagonal a path has reached, and its last component.
#[derive(Clone, Copy)]
struct Path {
    old_pos: isize,
    last: Option<usize>,
}

/// jsdiff's `Diff.diffWithOptionsObj` (`base.js:20-133`) without its time and edit limits,
/// which Pierre leaves unset: Myers' greedy search that branches from the path furthest along
/// the old side, preferring a removal when both reach as far, and stops considering diagonals
/// past the edit graph's edges.
fn diff(old: &[&str], new: &[&str]) -> Vec<Component> {
    let (old_len, new_len) = (old.len() as isize, new.len() as isize);
    let mut nodes: Vec<Node> = Vec::new();
    let off = old_len + new_len + 1;
    let mut best: Vec<Option<Path>> = vec![None; (2 * off + 1) as usize];
    let at = |d: isize| (d + off) as usize;
    let mut first = Path { old_pos: -1, last: None };
    let new_pos = extract_common(&mut first, &mut nodes, old, new, 0);
    if first.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
        return build(&nodes, first.last);
    }
    best[at(0)] = Some(first);
    let (mut min_d, mut max_d) = (isize::MIN, isize::MAX);
    for edit_len in 1..=old_len + new_len {
        let mut d = min_d.max(-edit_len);
        while d <= max_d.min(edit_len) {
            let remove = best[at(d - 1)].take();
            let add = best[at(d + 1)];
            let can_add = add.is_some_and(|p| (0..new_len).contains(&(p.old_pos - d)));
            let can_remove = remove.is_some_and(|p| p.old_pos + 1 < old_len);
            let mut base = match (add, remove) {
                (Some(a), r) if can_add && (!can_remove || r.is_some_and(|r| r.old_pos < a.old_pos)) => add_to_path(&mut nodes, a, Edit::Added, 0),
                (_, Some(r)) if can_remove => add_to_path(&mut nodes, r, Edit::Removed, 1),
                _ => {
                    best[at(d)] = None;
                    d += 2;
                    continue;
                }
            };
            let new_pos = extract_common(&mut base, &mut nodes, old, new, d);
            if base.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                return build(&nodes, base.last);
            }
            best[at(d)] = Some(base);
            if base.old_pos + 1 >= old_len {
                max_d = max_d.min(d - 1);
            }
            if new_pos + 1 >= new_len {
                min_d = min_d.max(d + 1);
            }
            d += 2;
        }
    }
    Vec::new()
}

/// `base.js:134-147`: one more token added or removed, extending the last component when it is
/// the same edit.
fn add_to_path(nodes: &mut Vec<Node>, path: Path, edit: Edit, old_inc: isize) -> Path {
    let node = match path.last.map(|i| (i, &nodes[i])) {
        Some((_, last)) if last.c.edit == edit => Node { c: Component { count: last.c.count + 1, edit }, prev: last.prev },
        _ => Node { c: Component { count: 1, edit }, prev: path.last },
    };
    nodes.push(node);
    Path { old_pos: path.old_pos + old_inc, last: Some(nodes.len() - 1) }
}

/// `base.js:148-166`: follows the diagonal while the tokens match, and returns the new side's
/// position.
fn extract_common(path: &mut Path, nodes: &mut Vec<Node>, old: &[&str], new: &[&str], d: isize) -> isize {
    let (old_len, new_len) = (old.len() as isize, new.len() as isize);
    let mut old_pos = path.old_pos;
    let mut new_pos = old_pos - d;
    let mut common = 0;
    while new_pos + 1 < new_len && old_pos + 1 < old_len && old[(old_pos + 1) as usize] == new[(new_pos + 1) as usize] {
        new_pos += 1;
        old_pos += 1;
        common += 1;
    }
    if common > 0 {
        nodes.push(Node { c: Component { count: common, edit: Edit::Kept }, prev: path.last });
        path.last = Some(nodes.len() - 1);
    }
    path.old_pos = old_pos;
    new_pos
}

/// The components from the first to `last` (`base.js:197-213`).
fn build(nodes: &[Node], mut last: Option<usize>) -> Vec<Component> {
    let mut out = Vec::new();
    while let Some(i) = last {
        out.push(nodes[i].c);
        last = nodes[i].prev;
    }
    out.reverse();
    out
}
