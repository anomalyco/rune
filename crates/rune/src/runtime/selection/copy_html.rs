//! The selection as HTML, for the clipboard's `text/html` beside its plain text, as a browser
//! copies a page's selection: pasted into a document or a mail it keeps its headings, lists,
//! tables, bold, italic, code and links.

use super::*;

/// The block a selected text belongs to: the nearest heading, list item, table cell or code
/// text around it (by node index), else the paragraph it is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Heading(usize, u8),
    Item { list: usize, item: usize, ordered: bool },
    Cell { table: usize, row: usize, cell: usize, head: bool },
    Code(usize),
    Paragraph(usize),
}

impl RuntimeCore {
    /// The selection as HTML (`None` with nothing selected, or a selection reaching rows that
    /// aren't built). Each selected text sits in its block; its spans are `<strong>`, `<em>` and
    /// `<code>`, its links `<a href>` and its strikes `<s>`.
    pub(crate) fn copy_html(&self) -> Option<String> {
        if self.selection_spans_unmounted() { return None; }
        let ranges = self.selection_ranges();
        if ranges.is_empty() { return None; }
        let mut out = String::new();
        let mut prev: Option<(Block, usize)> = None;
        // The lists open around the last item, outermost first: (list, item, ordered).
        let mut lists: Vec<(usize, usize, bool)> = Vec::new();
        let close_lists = |out: &mut String, lists: &mut Vec<(usize, usize, bool)>, keep: usize| {
            while lists.len() > keep {
                let (_, _, ordered) = lists.pop().unwrap_or_default();
                out.push_str(if ordered { "</li></ol>" } else { "</li></ul>" });
            }
        };
        for &(i, a, b) in &ranges {
            let Some(spec) = self.tree.nodes[i].text.as_ref() else { continue };
            let block = self.block_of(i);
            // A list's markers are the list's own: `<ol>` and `<ul>` draw them.
            if self.tree.nodes[i].a11y.hidden && matches!(block, Block::Item { .. }) { continue; }
            let inline = inline_html(spec, a, b);
            if inline.trim().is_empty() { continue; }
            match prev {
                Some((p, last)) if p == block => {
                    if self.beside(last, i) && !out.ends_with(' ') && !inline.starts_with(' ') { out.push(' '); } else if !self.beside(last, i) { out.push_str("<br>"); }
                }
                _ => {
                    if let Some((p, _)) = prev.filter(|(p, _)| !matches!(p, Block::Item { .. })) { close(&mut out, p, Some(block)); }
                    match block {
                        // A list nested in an item opens inside it; a later item of an outer
                        // list closes the lists inside first.
                        Block::Item { list, item, ordered } => {
                            let keep = lists.iter().rposition(|&(l, it, _)| l == list || self.within(list, it)).map_or(0, |k| k + 1);
                            close_lists(&mut out, &mut lists, keep);
                            match lists.last_mut() {
                                Some(top) if top.0 == list => {
                                    out.push_str("</li><li>");
                                    top.1 = item;
                                }
                                _ => {
                                    out.push_str(if ordered { "<ol><li>" } else { "<ul><li>" });
                                    lists.push((list, item, ordered));
                                }
                            }
                        }
                        _ => {
                            close_lists(&mut out, &mut lists, 0);
                            open(&mut out, prev.map(|(p, _)| p), block);
                        }
                    }
                }
            }
            out.push_str(&inline);
            prev = Some((block, i));
        }
        let (p, _) = prev?;
        if !matches!(p, Block::Item { .. }) { close(&mut out, p, None); }
        close_lists(&mut out, &mut lists, 0);
        Some(out)
    }

    /// Node `j` is inside node `a`'s subtree.
    fn within(&self, j: usize, a: usize) -> bool {
        std::iter::successors(self.tree.nodes[j].parent, |&p| self.tree.nodes[p].parent).any(|p| p == a)
    }

    fn block_of(&self, i: usize) -> Block {
        let mono = self.tree.nodes[i].text.as_ref().is_some_and(|t| matches!(t.style.family, crate::tok::Family::Mono | crate::tok::Family::SystemMono));
        let up = || std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent);
        let role = |j: usize| self.tree.nodes[j].role;
        for j in up() {
            match role(j) {
                "heading" => return Block::Heading(j, self.tree.nodes[j].a11y.level.unwrap_or(2).clamp(1, 6) as u8),
                "listitem" => {
                    let list = std::iter::successors(self.tree.nodes[j].parent, |&p| self.tree.nodes[p].parent).find(|&p| role(p) == "list").unwrap_or(j);
                    // A list whose first item's marker is a number is ordered.
                    let ordered = self.tree.nodes.get(j + 1).and_then(|n| n.text.as_ref()).is_some_and(|t| t.content.trim_end_matches('.').chars().all(|c| c.is_ascii_digit()) && !t.content.is_empty());
                    return Block::Item { list, item: j, ordered };
                }
                "cell" | "columnheader" | "rowheader" => {
                    let ancestors = || std::iter::successors(self.tree.nodes[j].parent, |&p| self.tree.nodes[p].parent);
                    let row = ancestors().find(|&p| role(p) == "row").unwrap_or(j);
                    let table = ancestors().find(|&p| role(p) == "table").unwrap_or(row);
                    return Block::Cell { table, row, cell: j, head: role(j) != "cell" };
                }
                _ => {}
            }
        }
        if mono { Block::Code(self.tree.nodes[i].parent.unwrap_or(i)) } else { Block::Paragraph(i) }
    }
}

/// Opens `block`'s elements after `prev`'s closed: a list or table already open stays open.
fn open(out: &mut String, prev: Option<Block>, block: Block) {
    match block {
        Block::Heading(_, level) => { write!(out, "<h{level}>").ok(); }
        // Lists open and close in `copy_html`, which nests them.
        Block::Item { .. } => {}
        Block::Cell { table, row, head, .. } => {
            let (same_table, same_row) = match prev {
                Some(Block::Cell { table: t, row: r, .. }) => (t == table, t == table && r == row),
                _ => (false, false),
            };
            if !same_table { out.push_str("<table>"); }
            if !same_row { out.push_str("<tr>"); }
            out.push_str(if head { "<th>" } else { "<td>" });
        }
        Block::Code(_) => out.push_str("<pre><code>"),
        Block::Paragraph(_) => out.push_str("<p>"),
    }
}

/// Closes `block`'s elements before `next`'s open: a list or table `next` continues stays open.
fn close(out: &mut String, block: Block, next: Option<Block>) {
    match block {
        Block::Heading(_, level) => { write!(out, "</h{level}>").ok(); }
        Block::Item { .. } => {}
        Block::Cell { table, row, head, .. } => {
            out.push_str(if head { "</th>" } else { "</td>" });
            let (same_table, same_row) = match next {
                Some(Block::Cell { table: t, row: r, .. }) => (t == table, t == table && r == row),
                _ => (false, false),
            };
            if !same_row { out.push_str("</tr>"); }
            if !same_table { out.push_str("</table>"); }
        }
        Block::Code(_) => out.push_str("</code></pre>"),
        Block::Paragraph(_) => out.push_str("</p>"),
    }
}

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};
use std::fmt::Write;

const MARK_BOLD: u8 = 1 << 0;
const MARK_ITALIC: u8 = 1 << 1;
const MARK_CODE: u8 = 1 << 2;
const MARK_STRIKE: u8 = 1 << 3;

fn add_marks(counts: &mut [usize; 4], bits: u8) {
    for (i, count) in counts.iter_mut().enumerate() {
        if bits & (1 << i) != 0 { *count += 1; }
    }
}

fn sub_marks(counts: &mut [usize; 4], bits: u8) {
    for (i, count) in counts.iter_mut().enumerate() {
        if bits & (1 << i) != 0 { *count -= 1; }
    }
}

/// Bytes `a..b` of `spec` as inline HTML: split where a markup-emitting span, a link or a strike
/// starts or ends, sweeping active intervals in O(N log N) instead of rescanning every span at
/// every boundary.
fn inline_html(spec: &crate::el::TextSpec, a: usize, b: usize) -> String {
    let len = spec.content.len();
    let (a, b) = (a.min(len), b.min(len));
    if spec.redacted || a >= b { return String::new(); }
    let links = &spec.cold.links;
    let strikes = &spec.cold.strikes;
    // A heading's own weight and an italic base style aren't inline markup.
    let emit_bold = spec.style.weight < 600.0;
    let emit_italic = !spec.style.italic;

    let mut cuts: Vec<usize> = vec![a, b];
    let mut marks: Vec<(usize, usize, u8)> = Vec::new();
    for sp in &spec.spans {
        #[cfg(test)]
        note_span_visit();
        let mut bits = 0u8;
        if emit_bold && sp.weight.is_some_and(|w| w >= 600.0) { bits |= MARK_BOLD; }
        if emit_italic && sp.italic { bits |= MARK_ITALIC; }
        if sp.mono { bits |= MARK_CODE; }
        if bits == 0 { continue; }
        let (start, end) = (sp.start as usize, sp.end as usize);
        if start >= end || end <= a || start >= b { continue; }
        marks.push((start, end, bits));
        cuts.push(start);
        cuts.push(end);
    }
    for &(start, end, _) in strikes {
        let (start, end) = (start as usize, end as usize);
        if start >= end || end <= a || start >= b { continue; }
        marks.push((start, end, MARK_STRIKE));
        cuts.push(start);
        cuts.push(end);
    }
    let mut link_ranges: Vec<(usize, usize, usize)> = Vec::new();
    for (idx, l) in links.iter().enumerate() {
        let (start, end) = (l.start as usize, l.end as usize);
        if start >= end || end <= a || start >= b { continue; }
        link_ranges.push((start, end, idx));
        cuts.push(start);
        cuts.push(end);
    }

    cuts.retain(|&c| c >= a && c <= b && spec.content.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();
    marks.sort_unstable_by_key(|&(start, _, _)| start);
    link_ranges.sort_unstable_by_key(|&(start, _, _)| start);

    let mut active_marks: BinaryHeap<Reverse<(usize, u8)>> = BinaryHeap::new();
    let mut mark_counts = [0usize; 4];
    let mut mark_idx = 0usize;

    let mut active_link_ends: BinaryHeap<Reverse<(usize, usize)>> = BinaryHeap::new();
    let mut active_link_ids: BTreeSet<usize> = BTreeSet::new();
    let mut link_idx = 0usize;

    let mut out = String::new();
    let mut href: Option<&str> = None;
    for w in cuts.windows(2) {
        let (s, e) = (w[0], w[1]);
        while mark_idx < marks.len() && marks[mark_idx].0 <= s {
            #[cfg(test)]
            note_span_visit();
            let (_, end, bits) = marks[mark_idx];
            mark_idx += 1;
            if end >= e {
                add_marks(&mut mark_counts, bits);
                active_marks.push(Reverse((end, bits)));
            }
        }
        while let Some(&Reverse((end, bits))) = active_marks.peek() {
            if end < e {
                #[cfg(test)]
                note_span_visit();
                active_marks.pop();
                sub_marks(&mut mark_counts, bits);
            } else {
                break;
            }
        }
        while link_idx < link_ranges.len() && link_ranges[link_idx].0 <= s {
            let (_, end, idx) = link_ranges[link_idx];
            link_idx += 1;
            if end >= e {
                active_link_ends.push(Reverse((end, idx)));
                active_link_ids.insert(idx);
            }
        }
        while let Some(&Reverse((end, idx))) = active_link_ends.peek() {
            if end < e {
                active_link_ends.pop();
                active_link_ids.remove(&idx);
            } else {
                break;
            }
        }
        let Some(text) = spec.copy_slice(s as u32, e as u32) else { continue };
        if text.is_empty() { continue; }
        let bold = mark_counts[0] > 0;
        let italic = mark_counts[1] > 0;
        let code = mark_counts[2] > 0;
        let struck = mark_counts[3] > 0;
        let link = active_link_ids
            .first()
            .map(|&idx| links[idx].id.as_str())
            .filter(|id| id.contains("://") || id.starts_with("mailto:"));
        if href != link {
            if href.is_some() { out.push_str("</a>"); }
            if let Some(h) = link { write!(out, "<a href=\"{}\">", escape(h)).ok(); }
            href = link;
        }
        let mut piece = escape(&text).replace('\n', "<br>");
        if code { piece = format!("<code>{piece}</code>"); }
        if struck { piece = format!("<s>{piece}</s>"); }
        if italic { piece = format!("<em>{piece}</em>"); }
        if bold { piece = format!("<strong>{piece}</strong>"); }
        out.push_str(&piece);
    }
    if href.is_some() { out.push_str("</a>"); }
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
thread_local! {
    static SPAN_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn note_span_visit() {
    SPAN_VISITS.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
fn count_span_visits<R>(f: impl FnOnce() -> R) -> (R, usize) {
    SPAN_VISITS.with(|c| c.set(0));
    let out = f();
    let count = SPAN_VISITS.with(|c| c.get());
    (out, count)
}

#[cfg(test)]
#[path = "copy_html_tests.rs"]
mod tests;
