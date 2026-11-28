//! Math in Markdown text (`math` feature): `$…$`, `\(…\)` and `$$…$$` in a line, and a
//! paragraph that is one display equation, with `anomalyco/opencode`'s delimiter rules
//! (`packages/ui/src/context/marked-parser.tsx`), so `$0.60 to $12.50`, `$5.00/$6.00` and
//! `$HOME/$USER` stay text. Inline math keeps its source as written in the text, styled
//! [`Style::Math`]: the renderer sets its bitmap over it once ready, and until then (or when
//! it fails) the source shows as it is.
//!
//! Multi-line display equations (`$$`, `$`, or `\[` alone on a line through their closer) are
//! found first and masked in-place (exact byte length preserved) before pulldown-cmark parses the
//! document, so TeX lines like `=`, `---`, `- x`, or `> 0` cannot split an equation into Setext
//! headings, lists, or quotes. A second pass over the masked parse finds single-line `$$…$$`
//! paragraphs and inline math runs (less code spans, raw HTML, and autolinks, and never across an
//! emphasis or link's edge). The walk then hands [`Math::claim`] each event before its own
//! handling.

use std::borrow::Cow;

use pulldown_cmark::LinkType;

use super::super::*;
use super::builder::{Builder, Fence};
use super::{SpanMap, Tail};

/// One display equation claimed from the event walk.
struct DisplayBlock {
    /// For a masked multi-line equation, the opener token range replaced with `a`s; for a
    /// single-line `$$…$$` paragraph, the paragraph's source range.
    opener: Range<usize>,
    /// True when produced by [`mask_display_blocks`] (may arrive as a `Paragraph` or a tight list
    /// item's `Text`).
    masked: bool,
    /// Source ranges of each TeX body line in the original source.
    lines: Vec<Range<usize>>,
}

/// The math found in a document, and the walk's place in it.
#[derive(Default)]
pub(super) struct Math {
    /// Inline math, in source order: each source range, and whether the walk set it down yet.
    spans: Vec<(Range<usize>, bool)>,
    /// Display equations: each one's opener range, masking flag, and TeX source lines.
    blocks: Vec<DisplayBlock>,
    /// The TeX lines of the display equation the walk is inside.
    block: Option<Vec<Range<usize>>>,
    /// Start of an unclosed multi-line display equation open at EOF, so incremental parsing keeps
    /// its cut at the opener line until the closing delimiter (or a blank line) arrives.
    open_display_from: Option<usize>,
}

impl Math {
    /// Prepares `src` for parsing: masks any closed multi-line display equations (preserving all
    /// byte offsets) and finds all display and inline math ranges.
    pub(super) fn prepare<'a>(src: &'a str, opts: Options) -> (Cow<'a, str>, Math) {
        if !crate::math::may_have_inline_math(src) {
            return (Cow::Borrowed(src), Math::default());
        }
        let (parse_src, mut blocks, open_display_from) = mask_display_blocks(src, opts);
        let mut found = Found::default();
        let mut code = 0usize;
        for (ev, r) in Parser::new_ext(&parse_src, opts).into_offset_iter() {
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
                        && let Some(lines) = block_lines(&parse_src, &events)
                    {
                        found.blocks.push(DisplayBlock { opener: r, masked: false, lines });
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
        let Found { runs, opaque, elements, blocks: single_blocks, .. } = found;
        let mut spans = Vec::new();
        for run in runs {
            if !single_blocks.iter().any(|b| b.opener.start <= run.start && run.end <= b.opener.end) {
                scan(&parse_src, run, &opaque, &mut spans);
            }
        }
        blocks.extend(single_blocks);
        // Never across an emphasis or link's edge: only around one, or inside one.
        let apart = |a: &Range<usize>, b: &Range<usize>| a.end <= b.start || b.end <= a.start;
        let holds = |a: &Range<usize>, b: &Range<usize>| a.start <= b.start && b.end <= a.end;
        spans.retain(|m| elements.iter().all(|e| apart(e, m) || holds(m, e) || holds(e, m)));
        (
            parse_src,
            Math { spans: spans.into_iter().map(|m| (m, false)).collect(), blocks, block: None, open_display_from },
        )
    }

    /// Whether an incremental parse cut at `at` should be suppressed because an unclosed display
    /// equation opened earlier is still waiting for its closer at EOF.
    pub(super) fn holds_cut(&self, at: usize) -> bool {
        self.open_display_from.is_some_and(|start| at > start)
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
            && let Some(i) = self.blocks.iter().position(|blk| {
                if blk.masked {
                    r.start <= blk.opener.start && blk.opener.end <= r.end
                } else {
                    blk.opener.start == r.start
                }
            })
        {
            b.flush_inline();
            b.tail = Tail::Code;
            self.block = Some(self.blocks.swap_remove(i).lines);
            return true;
        }
        if matches!(ev, Event::Text(_))
            && let Some(i) = self.blocks.iter().position(|blk| blk.masked && blk.opener == *r)
        {
            b.flush_inline();
            b.tail = Tail::Code;
            let lines = self.blocks.swap_remove(i).lines;
            push_block(b, src, &lines);
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
    blocks: Vec<DisplayBlock>,
}

/// Container prefix shape of a display equation: `quote_depth` blockquote markers (`>`), up to
/// `pre_quote_indent` columns of list indent before a `>` (for blockquotes nested inside list
/// items), and up to `post_quote_indent` columns of list indent after the last `>`.
#[derive(Clone, Copy)]
struct Prefix {
    quote_depth: usize,
    pre_quote_indent: usize,
    post_quote_indent: usize,
}

enum ScanBlock {
    Closed {
        opener: Range<usize>,
        blanks: Vec<Range<usize>>,
        body: Vec<Range<usize>>,
        end: usize,
    },
    Unclosed {
        stop: usize,
        at_eof: bool,
    },
    Empty,
}

/// Finds closed multi-line display equations (`$$`, `$`, or `\[` alone on a line through their
/// matching closer) outside code and HTML blocks, and masks each one in-place (same byte length:
/// opener bytes replaced with `a`, body and closer bytes replaced with spaces, keeping container
/// prefixes and line breaks intact) so CommonMark block rules (`=`, `---`, `- `, `> `, `# `)
/// cannot break an equation apart.
fn mask_display_blocks<'a>(src: &'a str, opts: Options) -> (Cow<'a, str>, Vec<DisplayBlock>, Option<usize>) {
    if !src.contains('\n') || !(src.contains('$') || src.contains("\\[")) {
        return (Cow::Borrowed(src), Vec::new(), None);
    }
    let mut code = 0usize;
    let mut containers: Vec<(Range<usize>, bool)> = Vec::new();
    let mut item_first = false;
    let mut claimed_until = 0usize;
    // Per closer (`$$`, `$`, `\]`): `(quote_depth, stop)` where no closer exists before `stop`.
    let mut no_close = [(0usize, 0usize); 3];
    let mut masked_ranges: Vec<(Range<usize>, Vec<Range<usize>>)> = Vec::new();
    let mut blocks: Vec<DisplayBlock> = Vec::new();
    let mut open_display_from: Option<usize> = None;

    for (ev, r) in Parser::new_ext(src, opts).into_offset_iter() {
        match &ev {
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) => {
                code += 1;
                item_first = false;
                continue;
            }
            Event::End(TagEnd::CodeBlock | TagEnd::HtmlBlock) => {
                code = code.saturating_sub(1);
                continue;
            }
            _ if code > 0 => continue,
            Event::Start(Tag::BlockQuote(_)) => {
                item_first = false;
                containers.push((r, false));
                continue;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                containers.pop();
                continue;
            }
            Event::Start(Tag::Item) => {
                item_first = true;
                containers.push((r, true));
                continue;
            }
            Event::End(TagEnd::Item) => {
                item_first = false;
                containers.pop();
                continue;
            }
            _ => {}
        }
        let cand = match &ev {
            Event::Start(Tag::Paragraph | Tag::Heading { .. }) => {
                item_first = false;
                Some(r.start)
            }
            Event::Text(_) if item_first => {
                item_first = false;
                Some(if src[..r.start].ends_with('\\') { r.start - 1 } else { r.start })
            }
            Event::Start(_) | Event::Rule | Event::TaskListMarker(_) => {
                item_first = false;
                None
            }
            _ => None,
        };
        let Some(pos) = cand.filter(|&p| p >= claimed_until) else { continue };
        let line_start = src[..pos].rfind('\n').map_or(0, |i| i + 1);
        let limit = containers.last().map_or(src.len(), |(c, _)| c.end);
        let in_item = containers.iter().any(|(_, is_item)| *is_item);
        let quote_depth = containers.iter().filter(|(_, is_item)| !*is_item).count();

        // Scan this block and any back-to-back display equation immediately following its closer.
        let mut cur_line_start = line_start;
        let mut cur_pos = Some(pos);
        let mut prefix = Prefix { quote_depth, pre_quote_indent: 0, post_quote_indent: 0 };
        loop {
            let line_nl = src[cur_line_start..limit].find('\n').map_or(limit, |d| cur_line_start + d);
            let line_end = strip_cr(src, cur_line_start, line_nl);
            let first_in_chain = cur_pos.is_some();
            let content_start = match cur_pos.take() {
                Some(p) => p,
                None => {
                    let Some(after_q) = strip_quotes(src, cur_line_start, line_end, prefix.quote_depth, prefix.pre_quote_indent) else {
                        break;
                    };
                    let body_start = strip_indent(src, after_q, line_end, prefix.post_quote_indent);
                    if extra_indent_cols(&src[body_start..line_end]) >= 4 {
                        break;
                    }
                    body_start
                }
            };
            let Some((opener_range, closer, closer_idx)) = match_opener(src, content_start, line_end) else {
                break;
            };
            if first_in_chain {
                prefix = line_prefix(src, cur_line_start, opener_range.start, quote_depth, in_item);
            }
            if line_nl >= limit {
                if limit == src.len() {
                    open_display_from.get_or_insert(cur_line_start);
                }
                break;
            }
            let (cached_qd, cached_stop) = no_close[closer_idx];
            if prefix.quote_depth == cached_qd && cur_line_start < cached_stop {
                break;
            }
            match scan_display_block(src, opener_range, line_nl + 1, limit, prefix, closer) {
                ScanBlock::Closed { opener, blanks, body, end } => {
                    masked_ranges.push((opener.clone(), blanks));
                    blocks.push(DisplayBlock { opener, masked: true, lines: body });
                    claimed_until = end;
                    if end >= limit {
                        break;
                    }
                    cur_line_start = end;
                }
                ScanBlock::Unclosed { stop, at_eof } => {
                    no_close[closer_idx] = (prefix.quote_depth, stop);
                    if at_eof {
                        open_display_from.get_or_insert(cur_line_start);
                    }
                    break;
                }
                ScanBlock::Empty => break,
            }
        }
    }

    if masked_ranges.is_empty() {
        return (Cow::Borrowed(src), Vec::new(), open_display_from);
    }
    let mut bytes = src.as_bytes().to_vec();
    for (opener, blanks) in masked_ranges {
        bytes[opener].fill(b'a');
        for blank in blanks {
            bytes[blank].fill(b' ');
        }
    }
    let Ok(masked) = String::from_utf8(bytes) else {
        return (Cow::Borrowed(src), Vec::new(), open_display_from);
    };
    (Cow::Owned(masked), blocks, open_display_from)
}

/// Computes the blockquote depth and list-item indents from `src[line_start..pos]`.
fn line_prefix(src: &str, line_start: usize, pos: usize, quote_depth: usize, in_item: bool) -> Prefix {
    let slice = &src[line_start..pos];
    if quote_depth == 0 {
        let post = if in_item { col_width(slice) } else { 0 };
        return Prefix { quote_depth: 0, pre_quote_indent: 0, post_quote_indent: post };
    }
    let pre = if in_item {
        slice.find('>').map_or(0, |i| col_width(&slice[..i]))
    } else {
        0
    };
    let post = if in_item {
        slice.rfind('>').map_or(0, |i| {
            let after = &slice[i + 1..];
            let after = after.strip_prefix(' ').unwrap_or(after);
            col_width(after)
        })
    } else {
        0
    };
    Prefix { quote_depth, pre_quote_indent: pre, post_quote_indent: post }
}

/// Matches a display equation opener (`$$`, `$`, or `\[`) alone on `src[start..end]`.
fn match_opener(src: &str, start: usize, end: usize) -> Option<(Range<usize>, &'static str, usize)> {
    let raw = &src[start..end];
    let lead = raw.len() - raw.trim_start_matches([' ', '\t']).len();
    let trimmed = raw[lead..].trim_end_matches([' ', '\t']);
    let (closer, idx) = match trimmed {
        "$$" => ("$$", 0),
        "$" => ("$", 1),
        "\\[" => ("\\]", 2),
        _ => return None,
    };
    let a = start + lead;
    Some((a..a + trimmed.len(), closer, idx))
}

/// Scans body lines starting at `from` for a matching `closer` line before `limit`.
fn scan_display_block(
    src: &str,
    opener: Range<usize>,
    from: usize,
    limit: usize,
    prefix: Prefix,
    closer: &str,
) -> ScanBlock {
    let mut body = Vec::new();
    let mut blanks = Vec::new();
    let mut cur = from;
    while cur < limit {
        let nl = src[cur..limit].find('\n').map_or(limit, |d| cur + d);
        let line_end = strip_cr(src, cur, nl);
        let Some(after_q) = strip_quotes(src, cur, line_end, prefix.quote_depth, prefix.pre_quote_indent) else {
            return ScanBlock::Unclosed { stop: cur, at_eof: false };
        };
        if src[after_q..line_end].bytes().all(|b| b.is_ascii_whitespace()) {
            return ScanBlock::Unclosed { stop: cur, at_eof: false };
        }
        let body_start = strip_indent(src, after_q, line_end, prefix.post_quote_indent);
        let content = &src[body_start..line_end];
        if content.trim_matches([' ', '\t']) == closer {
            if body.is_empty() {
                return ScanBlock::Empty;
            }
            blanks.push(after_q..line_end);
            let end = if nl < limit { nl + 1 } else { nl };
            return ScanBlock::Closed { opener, blanks, body, end };
        }
        if opens_code_fence(content) {
            return ScanBlock::Unclosed { stop: cur, at_eof: false };
        }
        blanks.push(after_q..line_end);
        body.push(body_start..line_end);
        cur = if nl < limit { nl + 1 } else { limit };
    }
    ScanBlock::Unclosed { stop: limit, at_eof: limit == src.len() }
}

fn strip_cr(src: &str, start: usize, end: usize) -> usize {
    if end > start && src.as_bytes()[end - 1] == b'\r' { end - 1 } else { end }
}

/// Strips `quote_depth` leading `>` markers (each with optional single space), allowing up to
/// `pre_indent + 3` leading columns before a `>`.
fn strip_quotes(src: &str, start: usize, end: usize, quote_depth: usize, pre_indent: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut i = start;
    let max_lead = pre_indent + 3;
    for q in 0..quote_depth {
        let mut cols = 0usize;
        let limit_cols = if q == 0 { max_lead } else { 3 };
        while i < end && cols <= limit_cols {
            match bytes[i] {
                b' ' => {
                    cols += 1;
                    i += 1;
                }
                b'\t' => {
                    cols = (cols / 4 + 1) * 4;
                    i += 1;
                }
                _ => break,
            }
        }
        if cols > limit_cols || i >= end || bytes[i] != b'>' {
            return None;
        }
        i += 1;
        if i < end && bytes[i] == b' ' {
            i += 1;
        }
    }
    Some(i)
}

/// Strips up to `max_cols` columns of leading spaces/tabs from `src[start..end]`.
fn strip_indent(src: &str, start: usize, end: usize, max_cols: usize) -> usize {
    let bytes = src.as_bytes();
    let mut i = start;
    let mut cols = 0usize;
    while i < end && cols < max_cols {
        match bytes[i] {
            b' ' => {
                cols += 1;
                i += 1;
            }
            b'\t' => {
                let next = (cols / 4 + 1) * 4;
                if next > max_cols {
                    break;
                }
                cols = next;
                i += 1;
            }
            _ => break,
        }
    }
    i
}

fn col_width(s: &str) -> usize {
    let mut cols = 0usize;
    for b in s.bytes() {
        if b == b'\t' {
            cols = (cols / 4 + 1) * 4;
        } else {
            cols += 1;
        }
    }
    cols
}

fn extra_indent_cols(s: &str) -> usize {
    let mut cols = 0usize;
    for b in s.bytes() {
        match b {
            b' ' => cols += 1,
            b'\t' => cols = (cols / 4 + 1) * 4,
            _ => break,
        }
    }
    cols
}

/// Whether `line` opens a CommonMark fenced code block (0–3 spaces, then `>= 3` backticks with
/// no backticks in the info string, or `>= 3` tildes).
fn opens_code_fence(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && i < 3 && bytes[i] == b' ' {
        i += 1;
    }
    let Some(&ch @ (b'`' | b'~')) = bytes.get(i) else { return false };
    let n = bytes[i..].iter().take_while(|&&b| b == ch).count();
    n >= 3 && (ch == b'~' || !bytes[i + n..].contains(&b'`'))
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
