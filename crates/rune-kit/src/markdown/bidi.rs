//! Direction in a document: each block reads the way its own words do, and the runs inside a line
//! that must keep their own order (inline code, a URL) are isolated from the words around them.
//!
//! A block's direction is its first strong character's outside those runs (UAX #9 P2, as
//! `dir="auto"` reads it). A block with none (a number, a code chip alone) takes the direction
//! around it. While a reply streams in, a block's first strong character only ever arrives once
//! and never changes after, so a block turns at most once: from the direction around it to its
//! own, when its first letter arrives.

use super::*;
use rune::Direction;

/// The direction of `inl`'s first strong character outside its isolated runs.
pub(super) fn inline_dir(inl: &Inline) -> Option<Direction> {
    let mut at = 0;
    for r in units(inl).into_iter().chain([inl.text.len()..inl.text.len()]) {
        if let Some(d) = inl.text.get(at..r.start).and_then(Direction::of) {
            return Some(d);
        }
        at = at.max(r.end);
    }
    None
}

/// The direction of the first of `blocks` that has one: a list's first item's, a quote's first
/// block's, a table's.
pub(super) fn blocks_dir(blocks: &[Block]) -> Option<Direction> {
    blocks.iter().find_map(|b| match b {
        Block::Para(inl) | Block::Heading(_, inl) => inline_dir(inl),
        Block::List { items, .. } => items.iter().find_map(|item| blocks_dir(item)),
        Block::Quote(inner) => blocks_dir(inner),
        Block::Table(t) => table_dir(t),
        _ => None,
    })
}

/// A table's direction (which side its first column sits on): its header's, else its rows'.
pub(super) fn table_dir(t: &Table) -> Option<Direction> {
    t.head.iter().chain(t.rows.iter().flatten()).find_map(inline_dir)
}

/// `e` in `dir` when it has one; otherwise it keeps the direction around it.
pub(super) fn directed(e: El, dir: Option<Direction>) -> El {
    match dir {
        Some(d) => e.dir(d),
        None => e,
    }
}

/// `e`, the text of `inl`, with its inline code (and the padding around it) and its URLs read left
/// to right, each one whole, whatever the words around it.
pub(super) fn isolated(e: El, inl: &Inline) -> El {
    units(inl).into_iter().fold(e, |e, r| e.isolate(r, Direction::Ltr))
}

/// The runs of `inl` that keep their own order: each inline code chip with its padding, and each
/// link whose text is its address.
fn units(inl: &Inline) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut code: Vec<Range<usize>> = inl.spans.iter().filter(|s| matches!(s.2, Style::Code | Style::CodePad)).map(|s| s.0..s.1).collect();
    code.sort_by_key(|r| r.start);
    for r in code {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    for (a, b, url) in &inl.links {
        let shown = inl.text.get(*a..*b).unwrap_or_default().trim();
        let address = shown == url.as_str() || ["http://", "https://", "www.", "file://", "mailto:"].iter().any(|p| shown.starts_with(p));
        if address && !out.iter().any(|r| r.start < *b && *a < r.end) {
            out.push(*a..*b);
        }
    }
    // Unmarked paths, calls, flags, file names, references and signed numbers in the prose.
    for r in rune::text::ltr_units(&inl.text) {
        if !out.iter().any(|o| o.start < r.end && r.start < o.end) {
            out.push(r);
        }
    }
    out.sort_by_key(|r| r.start);
    out
}
