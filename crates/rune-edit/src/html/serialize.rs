//! Safe HTML serializer for a [`TextEdit`] range.

use std::ops::Range;

use rune_text::{Direction, Flow};

use super::{ListKind, escape_html};
use crate::{Block, Mark, MarkRange, TextEdit};

/// Serializes byte range `range` of `edit` to HTML.
pub(crate) fn serialize_html(edit: &TextEdit, range: Range<usize>) -> String {
    let start = range.start.min(edit.text.len());
    let end = range.end.min(edit.text.len()).max(start);
    if start == end && edit.blocks().iter().all(|&b| b == Block::Paragraph) {
        return String::new();
    }

    // Collect paragraphs intersecting `start..end`
    let mut paras: Vec<(usize, Range<usize>, Block, bool)> = Vec::new();
    for p in edit.paragraphs() {
        let p_end_with_nl = if p.range.end < edit.text.len() { p.range.end + 1 } else { p.range.end };
        if p.range.start <= end && p_end_with_nl >= start {
            let sub_start = p.range.start.max(start);
            let sub_end = p.range.end.min(end);
            if sub_start <= sub_end
                && (sub_start < sub_end || start == end || (p.range.start >= start && p.range.end <= end))
            {
                paras.push((p.index, sub_start..sub_end, p.block, p.soft_break && p.range.start > start));
            }
        }
    }

    let dirs = block_dirs(edit, &paras);
    let mut out = String::new();
    let mut idx = 0;
    while idx < paras.len() {
        let (_, _, block, _) = paras[idx];
        match block {
            Block::Code => {
                out.push_str("<pre");
                out.push_str(dirs[idx]);
                out.push_str("><code>");
                let mut first = true;
                while idx < paras.len() && paras[idx].2 == Block::Code {
                    if !first {
                        out.push('\n');
                    }
                    first = false;
                    let r = &paras[idx].1;
                    escape_html(&edit.text[r.clone()], &mut out);
                    idx += 1;
                }
                out.push_str("</code></pre>");
            }
            Block::Bullet(_) | Block::Numbered(_) => {
                // Group consecutive list items and emit properly nested `<ul>` / `<ol>`
                let mut open_lists: Vec<ListKind> = Vec::new();
                while idx < paras.len() && paras[idx].2.is_list() {
                    let (_, ref r, blk, _) = paras[idx];
                    let (kind, level) = match blk {
                        Block::Bullet(l) => (ListKind::Bullet, l as usize),
                        Block::Numbered(l) => (ListKind::Numbered, l as usize),
                        _ => unreachable!(),
                    };
                    let target_depth = level + 1;
                    while open_lists.len() > target_depth {
                        match open_lists.pop().unwrap() {
                            ListKind::Bullet => out.push_str("</ul>"),
                            ListKind::Numbered => out.push_str("</ol>"),
                        }
                    }
                    if open_lists.len() == target_depth && open_lists.last() != Some(&kind) {
                        match open_lists.pop().unwrap() {
                            ListKind::Bullet => out.push_str("</ul>"),
                            ListKind::Numbered => out.push_str("</ol>"),
                        }
                    }
                    while open_lists.len() < target_depth {
                        let k = if open_lists.len() + 1 == target_depth {
                            kind
                        } else {
                            *open_lists.last().unwrap_or(&kind)
                        };
                        out.push_str(if k == ListKind::Bullet { "<ul" } else { "<ol" });
                        out.push_str(dirs[idx]);
                        out.push('>');
                        open_lists.push(k);
                    }
                    out.push_str("<li");
                    out.push_str(dirs[idx]);
                    out.push('>');
                    serialize_inline(edit, r.clone(), &mut out);
                    idx += 1;
                    while idx < paras.len() && paras[idx].3 && paras[idx].2 == blk {
                        out.push_str("<br>");
                        serialize_inline(edit, paras[idx].1.clone(), &mut out);
                        idx += 1;
                    }
                    out.push_str("</li>");
                }
                while let Some(k) = open_lists.pop() {
                    match k {
                        ListKind::Bullet => out.push_str("</ul>"),
                        ListKind::Numbered => out.push_str("</ol>"),
                    }
                }
            }
            Block::Quote => {
                out.push_str("<blockquote");
                out.push_str(dirs[idx]);
                out.push('>');
                serialize_inline(edit, paras[idx].1.clone(), &mut out);
                idx += 1;
                while idx < paras.len() && paras[idx].3 && paras[idx].2 == Block::Quote {
                    out.push_str("<br>");
                    serialize_inline(edit, paras[idx].1.clone(), &mut out);
                    idx += 1;
                }
                out.push_str("</blockquote>");
            }
            Block::Paragraph => {
                out.push_str("<p");
                out.push_str(dirs[idx]);
                out.push('>');
                serialize_inline(edit, paras[idx].1.clone(), &mut out);
                idx += 1;
                while idx < paras.len() && paras[idx].3 && paras[idx].2 == Block::Paragraph {
                    out.push_str("<br>");
                    serialize_inline(edit, paras[idx].1.clone(), &mut out);
                    idx += 1;
                }
                out.push_str("</p>");
            }
        }
    }
    out
}

/// Each copied block's ` dir` attribute, by `paras` index: the direction the field showed it in
/// (its first strong character's, else the paragraph before it's, or the one Ctrl+Shift set),
/// and code left to right. Word, Mail and browsers then lay a right-to-left copy out as it was
/// shown. A copy with no right-to-left block has none, so it is as it always was.
fn block_dirs(edit: &TextEdit, paras: &[(usize, Range<usize>, Block, bool)]) -> Vec<&'static str> {
    let forced = edit.forced_dir().map(|f| if f == Flow::Rtl { Direction::Rtl } else { Direction::Ltr });
    let mut last = None;
    let shown: Vec<Option<Direction>> = edit
        .paragraphs()
        .iter()
        .map(|p| {
            last = forced.or_else(|| Direction::of(&edit.text[p.range.clone()])).or(last);
            last
        })
        .collect();
    let at = |i: usize| shown.get(i).copied().flatten();
    let rtl = paras.iter().any(|p| p.2 != Block::Code && at(p.0) == Some(Direction::Rtl));
    paras
        .iter()
        .map(|p| match (p.2, at(p.0)) {
            _ if !rtl => "",
            (Block::Code, _) | (_, Some(Direction::Ltr)) => " dir=\"ltr\"",
            (_, Some(Direction::Rtl)) => " dir=\"rtl\"",
            _ => "",
        })
        .collect()
}

/// Serializes inline marks within `range` (a single line/paragraph slice) to HTML. A pill
/// wholly inside `range` is one `<span data-pill>` around its label's own marks, so marks
/// that cross its edges never split it; a pill cut by `range` is plain text.
fn serialize_inline(edit: &TextEdit, range: Range<usize>, out: &mut String) {
    let mut at = range.start;
    for m in edit.marks().iter().filter(|m| range.start <= m.start && m.end <= range.end) {
        let Some(id) = m.mark.pill_id() else { continue };
        serialize_marks(edit, at..m.start, out);
        out.push_str("<span data-pill=\"");
        escape_html(id, out);
        out.push_str("\">");
        serialize_marks(edit, m.range(), out);
        out.push_str("</span>");
        at = m.end;
    }
    serialize_marks(edit, at..range.end, out);
}

/// Serializes the inline marks other than pills within `range`.
fn serialize_marks(edit: &TextEdit, range: Range<usize>, out: &mut String) {
    if range.is_empty() {
        return;
    }
    let overlapping: Vec<&MarkRange> = edit
        .marks()
        .iter()
        .filter(|m| m.start < range.end && m.end > range.start && !m.mark.is_pill())
        .collect();
    if overlapping.is_empty() {
        escape_html(&edit.text[range], out);
        return;
    }
    let mut bounds = vec![range.start, range.end];
    for m in &overlapping {
        bounds.push(m.start.clamp(range.start, range.end));
        bounds.push(m.end.clamp(range.start, range.end));
    }
    bounds.sort_unstable();
    bounds.dedup();

    // `overlapping` is sorted by start (as `marks()` is): sweep it instead of rescanning.
    let mut covering: Vec<&MarkRange> = Vec::new();
    let mut next = 0;
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b {
            continue;
        }
        covering.retain(|m| m.end > a);
        while next < overlapping.len() && overlapping[next].start <= a {
            if overlapping[next].end > a {
                covering.push(overlapping[next]);
            }
            next += 1;
        }
        let mut active: Vec<&Mark> = covering.iter().map(|m| &m.mark).collect();
        // Canonical nesting order: Link, Bold, Italic, Underline, Strike, Code
        active.sort_by_key(|m| match m {
            Mark::Link(_) => 0u8,
            Mark::Bold => 1,
            Mark::Italic => 2,
            Mark::Underline => 3,
            Mark::Strike => 4,
            Mark::Code => 5,
            Mark::Pill(_) => 6,
        });
        for m in &active {
            match m {
                Mark::Bold => out.push_str("<strong>"),
                Mark::Italic => out.push_str("<em>"),
                Mark::Underline => out.push_str("<u>"),
                Mark::Strike => out.push_str("<s>"),
                Mark::Code => out.push_str("<code>"),
                Mark::Link(url) => {
                    out.push_str("<a href=\"");
                    escape_html(url, out);
                    out.push_str("\">");
                }
                Mark::Pill(_) => {}
            }
        }
        escape_html(&edit.text[a..b], out);
        for m in active.iter().rev() {
            match m {
                Mark::Bold => out.push_str("</strong>"),
                Mark::Italic => out.push_str("</em>"),
                Mark::Underline => out.push_str("</u>"),
                Mark::Strike => out.push_str("</s>"),
                Mark::Code => out.push_str("</code>"),
                Mark::Link(_) => out.push_str("</a>"),
                Mark::Pill(_) => {}
            }
        }
    }
}
