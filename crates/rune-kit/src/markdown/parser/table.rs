//! Partial GFM table headers arriving at the end of a stream.

use super::super::*;
use super::builder::MAX_TABLE_COLUMNS;
use super::{BlockMap, SpanMap, parse_doc_with};

/// The header (and partial delimiter row) of a table still streaming in, from the source of the
/// document's last paragraph: `| a | b |` alone, or followed by a delimiter row in progress.
pub(super) fn pending_table(src: &str, base_offset: usize, autolink: bool, hard_breaks: bool) -> Option<(Table, BlockMap)> {
    let mut lines = src.trim_end().lines();
    let raw_head = lines.next()?;
    let lead_ws = raw_head.len() - raw_head.trim_start().len();
    let head = raw_head.trim();
    let delim = lines.next().map(str::trim);
    if lines.next().is_some() || !head.starts_with('|') {
        return None;
    }
    let cells = split_row(head, base_offset + lead_ws);
    if cells.len() < 2 || delim.is_some_and(|d| !d.starts_with('|') || !d.chars().all(|c| "|:- ".contains(c))) {
        return None;
    }
    let mut align = vec![Align::Start; cells.len()];
    if let Some(d) = delim {
        for (a, (_, spec)) in align.iter_mut().zip(split_row(d, 0)) {
            *a = match (spec.starts_with(':'), spec.len() > 1 && spec.ends_with(':')) {
                (true, true) => Align::Center,
                (false, true) => Align::End,
                _ => Align::Start,
            };
        }
    }
    let mut head_inlines = Vec::with_capacity(cells.len());
    let mut head_maps = Vec::with_capacity(cells.len());
    for (cell_off, c) in &cells {
        let (inl, map) = inline_of(c, *cell_off, autolink, hard_breaks);
        head_inlines.push(inl);
        head_maps.push(map);
    }
    Some((
        Table { align, head: head_inlines, rows: Vec::new(), open: true },
        BlockMap::Table { head: head_maps, rows: Vec::new() },
    ))
}

/// The cells of a `| a | b |` line (a trailing pipe is optional; `\|` doesn't split, and stays
/// escaped for [`inline_of`] to unescape), with each trimmed cell's starting byte offset.
fn split_row(line: &str, base_offset: usize) -> Vec<(usize, String)> {
    let pipe_off = usize::from(line.starts_with('|'));
    let inner = &line[pipe_off..];
    let mut cells: Vec<(usize, String)> = vec![(base_offset + pipe_off, String::new())];
    let mut chars = inner.char_indices().peekable();
    while let Some((idx, c)) = chars.next() {
        match c {
            '\\' if chars.peek().is_some_and(|&(_, next)| next == '|') => {
                cells.last_mut().unwrap().1.extend(['\\', chars.next().unwrap().1]);
            }
            '|' => {
                if cells.len() > MAX_TABLE_COLUMNS {
                    break;
                }
                cells.push((base_offset + pipe_off + idx + 1, String::new()));
            }
            c => cells.last_mut().unwrap().1.push(c),
        }
    }
    if cells.last().is_some_and(|(_, c)| c.trim().is_empty()) && inner.trim_end().ends_with('|') {
        cells.pop();
    }
    cells.truncate(MAX_TABLE_COLUMNS);
    cells
        .into_iter()
        .map(|(off, c)| {
            let lead = c.len() - c.trim_start().len();
            (off + lead, c.trim().to_owned())
        })
        .collect()
}

fn inline_of(src: &str, offset: usize, autolink: bool, hard_breaks: bool) -> (Inline, SpanMap) {
    let (blocks, _, _, _, _, maps) = parse_doc_with(src, autolink, hard_breaks);
    match (blocks.into_iter().next(), maps.into_iter().next()) {
        (Some(Block::Para(i)), Some(BlockMap::Inline(mut m))) => {
            m.shift_src(offset);
            (i, m)
        }
        _ => {
            let mut m = SpanMap::default();
            m.push(offset..offset + src.len(), 0..src.len());
            (Inline { text: src.to_owned(), ..Inline::default() }, m)
        }
    }
}
