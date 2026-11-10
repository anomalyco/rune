//! A source still arriving ([`Markdown::streaming`]): its tail is parsed as what it is about to
//! become, so no markup flashes as text and no block changes kind under the reader.
//!
//! - An emphasis, strike or code span opened in the last paragraph and not yet closed draws as
//!   already styled: its closers are added at the end for the parse. Only an opener at a word's
//!   start counts (`2 * 3` and `snake_case` stay text), `~` counts only as `~~`, and a parse
//!   that would still show a marker falls back to the source as written.
//! - A link still arriving draws as a link: `[text` and `[text](https://ex` are parsed with their
//!   `](…)` or `)` added, so its brackets never show. A `[` that turns out not to start a link
//!   (`[x] done`, `[1]`) shows as written once the next character after its `]` says so.
//! - A run of markers at the very end, which may yet open or close a span (`and **`), is left
//!   out until the next character says which.
//! - A trailing `|` line that isn't a table yet is left out until it is one, or until the line
//!   after it shows it isn't, so a header never lays out as a full-width paragraph first.
//!
//! Every change is at the end of the source, so source offsets (block starts, highlights, task
//! markers) mean the same in the parse. A finished document never comes here.

use super::*;
use super::parser::{ParsedDoc, Scope, Tail, parse_doc_tail};

/// [`parse_doc`] of `src` as a stream's current text.
pub(super) fn parse_streaming(src: &str, autolink: bool) -> ParsedDoc {
    let (raw, tail) = parse_doc_tail(src, autolink);
    let end = held_table(src, tail, raw.0.last()).unwrap_or(src.len());
    let patch = match tail {
        Tail::Inline(start, scope) if start < end => close_tail(&src[start..end], start, scope),
        _ => None,
    };
    let Some(Patch { cut, closers, markers, code }) = patch else {
        return if end == src.len() { raw } else { parse_doc(&src[..end], autolink) };
    };
    if !closers.is_empty() {
        let kept = src[..cut].trim_end();
        let view = format!("{kept}{closers}");
        let mut doc = parse_doc(&view, autolink);
        let added = kept.len()..view.len();
        if !markers.iter().chain([&added]).any(|r| doc.5.iter().any(|m| shows(m, r))) {
            if code {
                drop_closing_pad(&mut doc);
            }
            return doc;
        }
    }
    let doc = parse_doc(&src[..cut], autolink);
    if columns(&doc) >= columns(&raw) {
        return doc;
    }
    // Leaving markers out of a header's last cell (`| a | *`) left that cell out too: keep it,
    // empty, so the table keeps its columns.
    let doc = parse_doc(&format!("{}|", &src[..cut]), autolink);
    if columns(&doc) >= columns(&raw) { doc } else { raw }
}

/// How many columns the last block has, if it's a table.
fn columns(doc: &ParsedDoc) -> usize {
    match doc.0.last() {
        Some(Block::Table(t)) => t.align.len().max(t.head.len()),
        _ => 0,
    }
}

/// Where the trailing `|` lines start, when they may still become a table's header or row and
/// don't draw as a table yet: a `|` line still arriving, a finished one that may be a header
/// waiting for its delimiter row, or a header with its delimiter row still arriving.
fn held_table(src: &str, tail: Tail, last: Option<&Block>) -> Option<usize> {
    if tail == Tail::Code || matches!(last, Some(Block::Table(_))) {
        return None;
    }
    let body = src.strip_suffix('\n').unwrap_or(src);
    let open = body.len() == src.len();
    let start = |end: usize| body[..end].rfind('\n').map_or(0, |i| i + 1);
    let piped = |line: &str| line.trim_start().starts_with('|');
    let delim = |line: &str| line.trim().chars().all(|c| "|:- ".contains(c));
    // The `|` line above `at`, when it starts a block of them, and so may be a header.
    let header = |at: usize| {
        let s = start(at.checked_sub(1)?);
        let above = s.checked_sub(1).map(start);
        (piped(&body[s..at - 1]) && !delim(&body[s..at - 1]) && above.is_none_or(|a| !piped(&body[a..s - 1]))).then_some(s)
    };
    let line = start(body.len());
    if !piped(&body[line..]) {
        return None;
    }
    if open {
        return Some(header(line).filter(|_| delim(&body[line..])).unwrap_or(line));
    }
    // A finished line under another isn't a header unless that one is, and it isn't.
    (line == 0 || !piped(&body[start(line - 1)..line - 1])).then_some(line)
}

/// How to end a stream's last inline run for its parse.
#[derive(Debug, PartialEq)]
struct Patch {
    /// Source bytes kept.
    cut: usize,
    /// Appended after the kept source (less trailing whitespace) to close what's open.
    closers: String,
    /// The source ranges of the open markers, none of which may show.
    markers: Vec<Range<usize>>,
    /// The closers end a code span.
    code: bool,
}

/// An opener waiting for its closer: where it starts (in the run), its character and length. A
/// link's text is `[` (with its `!` for an image); once in its destination, its character is
/// what the destination still needs, `(`, or the quote of a title still arriving.
struct Open {
    at: usize,
    ch: u8,
    n: usize,
}

/// What ends `run`, the source from byte `base` of the inline text the stream ends in: nothing
/// when it ends on nothing open.
fn close_tail(run: &str, base: usize, scope: Scope) -> Option<Patch> {
    // A blank line ended the paragraph: what's open in it stays as written.
    if scope == Scope::Para && run.split('\n').rev().skip(1).any(|l| l.trim().is_empty()) {
        return None;
    }
    let b = run.as_bytes();
    let mut stack: Vec<Open> = Vec::new();
    let mut code = None;
    let mut cut = run.len();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'\n' if scope != Scope::Para => {
                stack.clear();
                i += 1;
            }
            b'|' if scope == Scope::Cell => {
                stack.clear();
                i += 1;
            }
            b'`' => {
                let n = run_len(b, i);
                match closing_ticks(b, i + n, n) {
                    Some(j) => i = j + n,
                    None => {
                        code = Some((i, n));
                        break;
                    }
                }
            }
            // At the very end: hidden while it may still open an image.
            b'!' if i + 1 == b.len() && word_start(run[..i].chars().next_back()) => {
                cut = i;
                break;
            }
            b'[' if word_start(run[..i].chars().next_back()) => {
                let at = if i > 0 && b[i - 1] == b'!' { i - 1 } else { i };
                stack.push(Open { at, ch: b'[', n: i + 1 - at });
                i += 1;
            }
            b']' => {
                let Some(p) = stack.iter().rposition(|o| o.ch == b'[') else {
                    i += 1;
                    continue;
                };
                match b.get(i + 1) {
                    // At the very end: hidden while it may still go on to `](`.
                    None => {
                        cut = i;
                        break;
                    }
                    Some(b'(') => match run[i + 2..].find(')') {
                        Some(j) => {
                            stack.truncate(p);
                            i += 2 + j + 1;
                        }
                        None => {
                            stack.truncate(p + 1);
                            let title = run[i + 2..].split_once(char::is_whitespace).map(|t| t.1.trim_start());
                            stack[p].ch = match title.and_then(|t| t.bytes().next()) {
                                Some(q @ (b'"' | b'\'')) if !title.is_some_and(|t| t.len() > 1 && t.ends_with(q as char)) => q,
                                _ => b'(',
                            };
                            break;
                        }
                    },
                    _ => {
                        stack.remove(p);
                        i += 1;
                    }
                }
            }
            b'h' | b'w' if url_at(run, i) => i += run[i..].find(char::is_whitespace).unwrap_or(run.len() - i),
            ch @ (b'*' | b'_' | b'~') => {
                let n = run_len(b, i);
                let before = run[..i].chars().next_back();
                let Some(after) = run[i + n..].chars().next() else {
                    // At the very end: hidden while it may still open or close a span.
                    let may_close = !space(before) && stack.iter().any(|o| o.ch == ch);
                    if word_start(before) || may_close {
                        cut = i;
                    }
                    break;
                };
                if ch == b'~' && n != 2 {
                    i += n;
                    continue;
                }
                let (opens, closes) = flanks(ch, before, after);
                let mut left = n;
                if closes {
                    while left > 0 {
                        let Some(p) = stack.iter().rposition(|o| o.ch == ch) else { break };
                        let used = stack[p].n.min(left);
                        stack[p].n -= used;
                        left -= used;
                        stack.truncate(if stack[p].n == 0 { p } else { p + 1 });
                    }
                }
                if left > 0 && opens {
                    stack.push(Open { at: i + n - left, ch, n: left });
                }
                i += n;
            }
            _ => i += 1,
        }
    }
    if let Some((at, n)) = code {
        let content = run[..cut].trim_end_matches('`').trim_end().len();
        if content <= at + n {
            cut = at;
            code = None;
        } else {
            cut = content;
        }
    }
    // An opener with nothing after it yet is left out with what follows, as is a `[x` that may
    // be a task's box.
    if code.is_none() {
        let empty = |o: &Open, cut: usize| {
            let rest = run.get(o.at + o.n..cut).unwrap_or("").trim_end();
            rest.is_empty() || (o.ch == b'[' && o.n == 1 && (rest == "x" || rest == "X"))
        };
        while let Some(o) = stack.last().filter(|o| empty(o, cut)) {
            cut = o.at;
            stack.pop();
        }
    }
    let mut closers = String::new();
    let mut markers = Vec::new();
    if let Some((at, n)) = code {
        // A span's content loses one space at each end only when it has both.
        if run[at + n..cut].starts_with(' ') {
            closers.push(' ');
        }
        closers.extend(std::iter::repeat_n('`', n));
        markers.push(base + at..base + at + n);
    }
    for o in stack.iter().rev() {
        match o.ch {
            b'[' => closers.push_str("]()"),
            b'(' => closers.push(')'),
            q @ (b'"' | b'\'') => closers.extend([q as char, ')']),
            ch => closers.extend(std::iter::repeat_n(ch as char, o.n)),
        }
        markers.push(base + o.at..base + o.at + o.n);
    }
    (cut < run.len() || !closers.is_empty()).then_some(Patch { cut: base + cut, closers, markers, code: code.is_some() })
}

fn run_len(b: &[u8], i: usize) -> usize {
    b[i..].iter().take_while(|&&c| c == b[i]).count()
}

/// The start of the first run of exactly `n` backticks from `from`.
fn closing_ticks(b: &[u8], mut from: usize, n: usize) -> Option<usize> {
    while from < b.len() {
        if b[from] != b'`' {
            from += 1;
            continue;
        }
        let len = run_len(b, from);
        if len == n {
            return Some(from);
        }
        from += len;
    }
    None
}

/// A bare URL starts at `i`; its `_` and `*` are part of it.
fn url_at(run: &str, i: usize) -> bool {
    let rest = &run[i..];
    (rest.starts_with("https://") || rest.starts_with("http://") || rest.starts_with("www."))
        && run[..i].chars().next_back().is_none_or(|c| c.is_whitespace() || "(<[".contains(c))
}

fn space(c: Option<char>) -> bool {
    c.is_none_or(char::is_whitespace)
}

fn punct(c: Option<char>) -> bool {
    c.is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace())
}

fn word_start(before: Option<char>) -> bool {
    space(before) || punct(before)
}

/// Whether a run of `ch` between `before` and `after` can open and can close a span: the
/// CommonMark flanking rules, with openers held to a word's start.
fn flanks(ch: u8, before: Option<char>, after: char) -> (bool, bool) {
    let after = Some(after);
    let left = !space(after) && (!punct(after) || word_start(before));
    let right = !space(before) && (!punct(before) || word_start(after));
    match ch {
        b'_' => (left && (!right || punct(before)), right && (!left || punct(after))),
        _ => (left && word_start(before), right),
    }
}

/// Drops the padding after a code span the stream is still inside, so its text only grows: the
/// padding comes back with what follows the span.
fn drop_closing_pad(doc: &mut ParsedDoc) {
    let (Some(b), Some(m)) = (doc.0.last_mut(), doc.5.last_mut()) else { return };
    let Some((inl, map)) = last_inline(b, m) else { return };
    let len = inl.text.len();
    let Some(k) = inl.spans.iter().position(|s| s.2 == Style::CodePad && s.1 == len && s.0 + CLOSE.len() == len) else { return };
    inl.spans.remove(k);
    let cut = len - CLOSE.len();
    inl.text.truncate(cut);
    inl.spans.retain_mut(|s| {
        s.1 = s.1.min(cut);
        s.0 < s.1
    });
    inl.links.retain_mut(|s| {
        s.1 = s.1.min(cut);
        s.0 < s.1
    });
    map.truncate_out(cut);
}

/// The last inline text of block `b`, and its map.
fn last_inline<'a>(b: &'a mut Block, m: &'a mut BlockMap) -> Option<(&'a mut Inline, &'a mut SpanMap)> {
    match (b, m) {
        (Block::Para(i) | Block::Heading(_, i), BlockMap::Inline(m)) => Some((i, m)),
        (Block::List { items, .. }, BlockMap::List(maps)) => last_inline(items.last_mut()?.last_mut()?, maps.last_mut()?.last_mut()?),
        (Block::Quote(inner), BlockMap::Quote(maps)) => last_inline(inner.last_mut()?, maps.last_mut()?),
        (Block::Table(t), BlockMap::Table { head, rows }) => match (t.rows.last_mut(), rows.last_mut()) {
            (Some(r), Some(m)) => Some((r.last_mut()?, m.last_mut()?)),
            _ => Some((t.head.last_mut()?, head.last_mut()?)),
        },
        _ => None,
    }
}

/// Whether any of `map`'s text comes from source bytes `r`.
fn shows(map: &BlockMap, r: &Range<usize>) -> bool {
    let hit = |m: &SpanMap| m.0.iter().any(|(s, _)| s.start < r.end && r.start < s.end);
    match map {
        BlockMap::Inline(m) | BlockMap::Code(m) => hit(m),
        BlockMap::List(items) => items.iter().flatten().any(|b| shows(b, r)),
        BlockMap::Quote(inner) => inner.iter().any(|b| shows(b, r)),
        BlockMap::Table { head, rows } => head.iter().chain(rows.iter().flatten()).any(hit),
        BlockMap::None => false,
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
