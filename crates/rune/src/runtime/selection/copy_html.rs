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

impl<S: 'static> Runtime<S> {
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
            let Some(spec) = self.nodes[i].text.as_ref() else { continue };
            let block = self.block_of(i);
            // A list's markers are the list's own: `<ol>` and `<ul>` draw them.
            if self.nodes[i].a11y.hidden && matches!(block, Block::Item { .. }) { continue; }
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
        std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == a)
    }

    fn block_of(&self, i: usize) -> Block {
        let mono = self.nodes[i].text.as_ref().is_some_and(|t| matches!(t.style.family, crate::tok::Family::Mono | crate::tok::Family::SystemMono));
        let up = || std::iter::successors(Some(i), |&p| self.nodes[p].parent);
        let role = |j: usize| self.nodes[j].role;
        for j in up() {
            match role(j) {
                "heading" => return Block::Heading(j, self.nodes[j].a11y.level.unwrap_or(2).clamp(1, 6) as u8),
                "listitem" => {
                    let list = std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).find(|&p| role(p) == "list").unwrap_or(j);
                    // A list whose first item's marker is a number is ordered.
                    let ordered = self.nodes.get(j + 1).and_then(|n| n.text.as_ref()).is_some_and(|t| t.content.trim_end_matches('.').chars().all(|c| c.is_ascii_digit()) && !t.content.is_empty());
                    return Block::Item { list, item: j, ordered };
                }
                "cell" | "columnheader" | "rowheader" => {
                    let ancestors = || std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent);
                    let row = ancestors().find(|&p| role(p) == "row").unwrap_or(j);
                    let table = ancestors().find(|&p| role(p) == "table").unwrap_or(row);
                    return Block::Cell { table, row, cell: j, head: role(j) != "cell" };
                }
                _ => {}
            }
        }
        if mono { Block::Code(self.nodes[i].parent.unwrap_or(i)) } else { Block::Paragraph(i) }
    }
}

/// Opens `block`'s elements after `prev`'s closed: a list or table already open stays open.
fn open(out: &mut String, prev: Option<Block>, block: Block) {
    match block {
        Block::Heading(_, level) => { let _ = write!(out, "<h{level}>"); }
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
        Block::Heading(_, level) => { let _ = write!(out, "</h{level}>"); }
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

use std::fmt::Write;

/// Bytes `a..b` of `spec` as inline HTML: split where a span, a link or a strike starts or ends.
fn inline_html(spec: &crate::el::TextSpec, a: usize, b: usize) -> String {
    let len = spec.content.len();
    let (a, b) = (a.min(len), b.min(len));
    let links = &spec.cold.links;
    let strikes = &spec.cold.strikes;
    let mut cuts: Vec<usize> = vec![a, b];
    cuts.extend(spec.spans.iter().flat_map(|s| [s.start as usize, s.end as usize]));
    cuts.extend(links.iter().flat_map(|l| [l.start as usize, l.end as usize]));
    cuts.extend(strikes.iter().flat_map(|s| [s.0 as usize, s.1 as usize]));
    cuts.retain(|&c| c >= a && c <= b && spec.content.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = String::new();
    let mut href: Option<&str> = None;
    for w in cuts.windows(2) {
        let (s, e) = (w[0], w[1]);
        let Some(text) = spec.copy_slice(s as u32, e as u32) else { continue };
        if text.is_empty() { continue; }
        let inside = |x: u32, y: u32| (x as usize) <= s && e <= y as usize;
        let bold = spec.spans.iter().any(|sp| inside(sp.start, sp.end) && sp.weight.is_some_and(|w| w >= 600.0)) || spec.style.weight >= 600.0;
        let italic = spec.spans.iter().any(|sp| inside(sp.start, sp.end) && sp.italic) && !spec.style.italic;
        let code = spec.spans.iter().any(|sp| inside(sp.start, sp.end) && sp.mono);
        let struck = strikes.iter().any(|st| inside(st.0, st.1));
        let link = links.iter().find(|l| inside(l.start, l.end)).map(|l| l.id.as_str()).filter(|id| id.contains("://") || id.starts_with("mailto:"));
        if href != link {
            if href.is_some() { out.push_str("</a>"); }
            if let Some(h) = link { let _ = write!(out, "<a href=\"{}\">", escape(h)); }
            href = link;
        }
        let mut piece = escape(&text).replace('\n', "<br>");
        if code { piece = format!("<code>{piece}</code>"); }
        if struck { piece = format!("<s>{piece}</s>"); }
        if italic { piece = format!("<em>{piece}</em>"); }
        // A heading's own weight isn't markup.
        if bold && spec.style.weight < 600.0 { piece = format!("<strong>{piece}</strong>"); }
        out.push_str(&piece);
    }
    if href.is_some() { out.push_str("</a>"); }
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
