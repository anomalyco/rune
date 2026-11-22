//! Math in Markdown text (`math` feature): `$…$`, `\(…\)` and `$$…$$` in a line, and a
//! paragraph that is one display equation, with `anomalyco/opencode`'s delimiter rules
//! (`packages/ui/src/context/marked-parser.tsx`), so `$0.60 to $12.50`, `$5.00/$6.00` and
//! `$HOME/$USER` stay text. Inline math keeps its source as written in the text, styled
//! [`Style::Math`]: the renderer sets its bitmap over it once ready, and until then (or when
//! it fails) the source shows as it is.
//!
//! A first pass over pulldown's events finds where math may be: runs of inline text, less code
//! spans, raw HTML and autolinks, and never across an emphasis or link's edge. The walk then
//! hands [`Math::claim`] each event before its own handling.

use pulldown_cmark::LinkType;

use super::super::*;
use super::builder::{Builder, Fence};
use super::{SpanMap, Tail};

/// The math found in a document, and the walk's place in it.
#[derive(Default)]
pub(super) struct Math {
    /// Inline math, in source order: each source range, and whether the walk set it down yet.
    spans: Vec<(Range<usize>, bool)>,
    /// Paragraphs that are one display equation: each one's source, and its TeX's source lines.
    blocks: Vec<(Range<usize>, Vec<Range<usize>>)>,
    /// The TeX lines of the display equation the walk is inside.
    block: Option<Vec<Range<usize>>>,
}

impl Math {
    pub(super) fn find(src: &str, opts: Options) -> Math {
        if !crate::math::may_have_inline_math(src) {
            return Math::default();
        }
        let mut found = Found::default();
        let mut code = 0usize;
        for (ev, r) in Parser::new_ext(src, opts).into_offset_iter() {
            let inline = match &ev {
                Event::Start(Tag::CodeBlock(_)) => {
                    code += 1;
                    false
                }
                Event::End(TagEnd::CodeBlock) => {
                    code = code.saturating_sub(1);
                    false
                }
                _ if code > 0 => continue,
                Event::Code(_) | Event::InlineHtml(_) => {
                    found.opaque.push(r.clone());
                    true
                }
                Event::Start(Tag::Link { link_type: LinkType::Autolink | LinkType::Email, .. }) => {
                    found.opaque.push(r.clone());
                    true
                }
                Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }) => {
                    found.elements.push(r.clone());
                    true
                }
                Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image) => true,
                Event::Text(_) | Event::SoftBreak | Event::HardBreak | Event::FootnoteReference(_) => true,
                Event::Start(Tag::Paragraph) => {
                    found.para = Some(Vec::new());
                    false
                }
                Event::End(TagEnd::Paragraph) => {
                    found.end_run();
                    if let Some(events) = found.para.take()
                        && let Some(lines) = block_lines(src, &events)
                    {
                        found.blocks.push((r, lines));
                    }
                    continue;
                }
                _ => false,
            };
            if inline {
                found.run = Some(found.run.map_or(r.clone(), |run| run.start.min(r.start)..run.end.max(r.end)));
                if let Some(events) = &mut found.para {
                    events.push((matches!(ev, Event::SoftBreak | Event::HardBreak), r));
                }
            } else {
                found.end_run();
            }
        }
        found.end_run();
        let Found { runs, opaque, elements, blocks, .. } = found;
        let mut spans = Vec::new();
        for run in runs {
            if !blocks.iter().any(|(p, _)| p.start <= run.start && run.end <= p.end) {
                scan(src, run, &opaque, &mut spans);
            }
        }
        // Never across an emphasis or link's edge: only around one, or inside one.
        let apart = |a: &Range<usize>, b: &Range<usize>| a.end <= b.start || b.end <= a.start;
        let holds = |a: &Range<usize>, b: &Range<usize>| a.start <= b.start && b.end <= a.end;
        spans.retain(|m| elements.iter().all(|e| apart(e, m) || holds(m, e) || holds(e, m)));
        Math { spans: spans.into_iter().map(|m| (m, false)).collect(), blocks, block: None }
    }

    /// Takes event `ev` at source `r` when it is math's: true when the walk should do nothing
    /// more with it.
    pub(super) fn claim(&mut self, b: &mut Builder, src: &str, ev: &Event<'_>, r: &Range<usize>) -> bool {
        if self.block.is_some() {
            if matches!(ev, Event::End(TagEnd::Paragraph)) {
                push_block(b, src, &self.block.take().unwrap_or_default());
            }
            return true;
        }
        if matches!(ev, Event::Start(Tag::Paragraph))
            && let Some(i) = self.blocks.iter().position(|(p, _)| p.start == r.start)
        {
            b.flush_inline();
            b.tail = Tail::Code;
            self.block = Some(self.blocks.swap_remove(i).1);
            return true;
        }
        if self.spans.is_empty() {
            return false;
        }
        let first = self.spans.partition_point(|(m, _)| m.end <= r.start);
        let hit: Vec<usize> = (first..self.spans.len()).take_while(|&i| self.spans[i].0.start < r.end).collect();
        let inside = hit.iter().copied().find(|&i| self.spans[i].0.start <= r.start && r.end <= self.spans[i].0.end);
        if let Some(i) = inside
            && matches!(ev, Event::Start(_) | Event::End(_) | Event::Text(_))
        {
            self.set_down(b, src, i);
            return true;
        }
        match ev {
            Event::Text(t) if !hit.is_empty() => {
                // The text is the source as written, or that after an escape's backslash.
                let shown = &src[r.clone()];
                let from = if shown == &**t {
                    r.start
                } else if shown.ends_with(&**t) {
                    r.end - t.len()
                } else {
                    return false;
                };
                let mut at = from;
                for i in hit {
                    let m = self.spans[i].0.clone();
                    if m.start > at {
                        push_text(b, src, at..m.start);
                    }
                    self.set_down(b, src, i);
                    at = at.max(m.end);
                }
                if at < r.end {
                    push_text(b, src, at..r.end);
                }
                true
            }
            _ => false,
        }
    }

    /// Sets down math `i` as its source, once.
    fn set_down(&mut self, b: &mut Builder, src: &str, i: usize) {
        let (m, done) = &mut self.spans[i];
        if std::mem::replace(done, true) {
            return;
        }
        let at = push_text(b, src, m.clone());
        b.inline().1.spans.push((at.start, at.end, Style::Math));
    }
}

#[derive(Default)]
struct Found {
    run: Option<Range<usize>>,
    runs: Vec<Range<usize>>,
    opaque: Vec<Range<usize>>,
    elements: Vec<Range<usize>>,
    /// The paragraph being read: each event's range and whether it breaks a line.
    para: Option<Vec<(bool, Range<usize>)>>,
    blocks: Vec<(Range<usize>, Vec<Range<usize>>)>,
}

impl Found {
    fn end_run(&mut self) {
        if let Some(run) = self.run.take() {
            self.runs.push(run);
        }
    }
}

/// Source `r` as text, mapped back to it: where it landed in the inline's text.
fn push_text(b: &mut Builder, src: &str, r: Range<usize>) -> Range<usize> {
    let inl = b.inline();
    let start = inl.1.text.len();
    inl.1.text.push_str(&src[r.clone()]);
    let end = inl.1.text.len();
    inl.3.push(r, start..end);
    start..end
}

/// A display equation's TeX, one line per source line, as a closed ```` ```math ```` fence.
fn push_block(b: &mut Builder, src: &str, lines: &[Range<usize>]) {
    let mut text = String::new();
    let mut map = SpanMap::default();
    for (k, l) in lines.iter().enumerate() {
        if k > 0 {
            text.push('\n');
        }
        let start = text.len();
        text.push_str(&src[l.clone()]);
        map.push(l.clone(), start..text.len());
    }
    b.push_code(text, Fence::Plain(Some("math".into()), true), map, false);
}

/// The TeX source lines of a paragraph that is one display equation: `$$` (or `$`, `\[`) alone
/// on its first line and its closer alone on its last, as OpenCode's block rule reads it, or
/// one line that is a whole `$$…$$`. Each line runs from its first event to the break after
/// it, so a quote's `>` and a list's indent stay out.
fn block_lines(src: &str, events: &[(bool, Range<usize>)]) -> Option<Vec<Range<usize>>> {
    let mut lines: Vec<Range<usize>> = Vec::new();
    let mut open: Option<usize> = None;
    for (brk, r) in events {
        if *brk {
            lines.push(open.take()?..r.start);
        } else if open.is_none() {
            // An escape's text starts after its backslash.
            open = Some(if src[..r.start].ends_with('\\') { r.start - 1 } else { r.start });
        }
    }
    lines.push(open?..events.last()?.1.end);
    let trimmed = |r: &Range<usize>| {
        let t = &src[r.clone()];
        let at = r.start + (t.len() - t.trim_start().len());
        at..at + t.trim().len()
    };
    if let [line] = &lines[..] {
        let t = trimmed(line);
        return display_at(src.as_bytes(), t.start, t.end).filter(|&e| e == t.end).map(|_| vec![t.start + 2..t.end - 2]);
    }
    let text = |r: &Range<usize>| &src[trimmed(r)];
    let closer = match text(&lines[0]) {
        "$$" => "$$",
        "$" => "$",
        "\\[" => "\\]",
        _ => return None,
    };
    if lines.len() < 3 || text(&lines[lines.len() - 1]) != closer {
        return None;
    }
    let body = lines[1..lines.len() - 1].to_vec();
    body.iter().any(|l| !text(l).is_empty()).then_some(body)
}

/// Inline math in source `run`, skipping `opaque` ranges (code spans, raw HTML, autolinks) and
/// never reaching into one.
///
/// One pass: `opaque` is in source order and its ranges never overlap (each is one event: a
/// code span, an inline HTML tag, an autolink), so `next`, the first range that ends after `i`,
/// only moves forward. A `\(` whose line holds no `\)` is looked for once per line, not once per
/// `\(` (see [`paren_at`]).
fn scan(src: &str, run: Range<usize>, opaque: &[Range<usize>], out: &mut Vec<Range<usize>>) {
    let s = src.as_bytes();
    let end = run.end.min(s.len());
    let mut i = run.start;
    let mut next = opaque.partition_point(|o| o.end <= i);
    // Every `\(` at `i` with `i + 2 <= no_close` has no `\)` before its line ends.
    let mut no_close = 0;
    while i < end {
        while opaque.get(next).is_some_and(|o| o.end <= i) {
            next += 1;
        }
        if let Some(o) = opaque.get(next).filter(|o| o.start <= i) {
            i = o.end;
            continue;
        }
        // An escape's text starts after its backslash, so a run can begin just past one.
        if i == run.start && escaped(s, i) {
            i += 1;
            continue;
        }
        let found = match s[i] {
            b'\\' if s.get(i + 1) == Some(&b'(') && i + 2 <= no_close => None,
            b'\\' if s.get(i + 1) == Some(&b'(') => paren_at(s, i, end).unwrap_or_else(|stop| {
                no_close = stop;
                None
            }),
            b'\\' => {
                // An escape: the next character is text.
                i += if s.get(i + 1).is_some_and(u8::is_ascii_punctuation) { 2 } else { 1 };
                continue;
            }
            b'$' if s.get(i + 1) == Some(&b'$') => display_at(s, i, end),
            b'$' => inline_at(s, i, end),
            _ => None,
        };
        match found {
            // Ranges before `next` end at or before `i`; from `next` on, the first starts first.
            Some(e) if !opaque.get(next).is_some_and(|o| o.start < e) => {
                out.push(i..e);
                i = e;
            }
            // A run of `$` that opens nothing is text, all of it.
            _ if s[i] == b'$' => i += s[i..end].iter().take_while(|&&c| c == b'$').count(),
            _ => i += 1,
        }
    }
}

/// Whether the byte at `i` follows an odd run of backslashes.
fn escaped(s: &[u8], i: usize) -> bool {
    s[..i].iter().rev().take_while(|&&c| c == b'\\').count() % 2 == 1
}

/// `\(…\)` at `i`, on one line and not empty: its end. `Err(stop)` when no `\)` follows before
/// the line or `end` does, `stop` being where the search ended. A later `\(` at `k` with
/// `k + 2 <= stop` has none either: this search passes through `k + 2` (it steps onto the `\` or
/// over it as an escaped character, then onto the `(`), so a search from there sees the same bytes.
fn paren_at(s: &[u8], i: usize, end: usize) -> Result<Option<usize>, usize> {
    let mut j = i + 2;
    while j < end {
        match s[j] {
            b'\n' => return Err(j),
            b'\\' if s.get(j + 1) == Some(&b')') => return Ok(s[i + 2..j].iter().any(|c| !c.is_ascii_whitespace()).then_some(j + 2)),
            b'\\' => j += 2,
            _ => j += 1,
        }
    }
    Err(j)
}

/// `$$…$$` at `i`, on one line, its TeX not empty: its end.
fn display_at(s: &[u8], i: usize, end: usize) -> Option<usize> {
    if s.get(i + 2) == Some(&b'$') {
        return None;
    }
    let mut j = i + 2;
    while j + 1 < end {
        match s[j] {
            b'\n' => return None,
            b'\\' if s[j + 1] == b'\n' => return None,
            b'\\' => j += 2,
            b'$' if s[j + 1] == b'$' => return (j > i + 2).then_some(j + 2),
            _ => j += 1,
        }
    }
    None
}

/// `$…$` at `i`: opened before a non-space and not after a letter or digit, closed on the
/// same line after a non-space and not before a word character, with no `$` between. Its end.
fn inline_at(s: &[u8], i: usize, end: usize) -> Option<usize> {
    if i > 0 && s[i - 1].is_ascii_alphanumeric() {
        return None;
    }
    let first = *s.get(i + 1)?;
    if first.is_ascii_whitespace() || first == b'$' {
        return None;
    }
    let mut j = i + 1;
    // Whether the TeX so far ends in a non-space (or an escaped character).
    let mut solid = false;
    while j < end {
        match s[j] {
            b'\n' => return None,
            b'\\' => {
                let next = *s.get(j + 1).filter(|_| j + 1 < end)?;
                if next == b'\n' {
                    return None;
                }
                solid = !next.is_ascii_whitespace();
                j += 2;
            }
            b'$' => {
                let word = s.get(j + 1).is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80);
                return (solid && !word).then_some(j + 1);
            }
            c => {
                solid = !c.is_ascii_whitespace();
                j += 1;
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "math_tests.rs"]
mod tests;
