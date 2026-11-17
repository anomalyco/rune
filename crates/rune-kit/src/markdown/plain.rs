//! Blocks as the plain text a selection of all of them copies: what [`Markdown::quote_frame`]
//! hands an app for a whole quote. It lays the blocks out as the texts' `copy_breaks` and
//! `copy_prefix` do (`render.rs`), reusing already-formatted inner quotes bottom-up.

use std::collections::HashMap;

use super::render::{Flow, Lead};
use super::*;

/// A block's text and the newlines it asks for before and after it.
struct Piece {
    breaks: [u8; 2],
    text: String,
}

pub(super) type QuotePlainCache = HashMap<(usize, usize, u32), (usize, [u8; 2], Rc<str>)>;

enum PlainFrame<'a> {
    Blocks {
        blocks: &'a [Block],
        idx: usize,
        cols: usize,
        lists: u32,
        lead: Option<(String, u8)>,
    },
    List {
        start: Option<u64>,
        items: &'a [Vec<Block>],
        idx: usize,
        cols: usize,
        lists: u32,
    },
}

impl Renderer {
    /// `blocks`, drawn with their first list item at `tasks[first_task]`, inside `flow`'s lists.
    /// Since quotes finish innermost-first during rendering, nested quotes reuse their inner
    /// quote's formatted `Rc<str>` without re-walking the subtree.
    pub(super) fn plain_text_rc(
        &self,
        blocks: &[Block],
        first_task: usize,
        flow: Flow,
        quote_plain_cache: &mut QuotePlainCache,
    ) -> Rc<str> {
        let key = (blocks.as_ptr() as usize, flow.cols, flow.lists);
        if let Some((_, _, cached)) = quote_plain_cache.get(&key) {
            return cached.clone();
        }
        if let [Block::Quote(child)] = blocks {
            let child_key = (child.as_ptr() as usize, flow.cols, flow.lists);
            if let Some(&(end_task, breaks, ref cached)) = quote_plain_cache.get(&child_key) {
                let rc = cached.clone();
                quote_plain_cache.insert(key, (end_task, breaks, rc.clone()));
                return rc;
            }
        }
        let mut pieces = Vec::new();
        let mut task = first_task;
        plain_iter(blocks, &self.tasks, &mut task, flow.cols, flow.lists, None, quote_plain_cache, &mut pieces);
        let bounds = match (pieces.first(), pieces.last()) {
            (Some(f), Some(l)) => [f.breaks[0], l.breaks[1]],
            _ => [0, 0],
        };
        let cap: usize = pieces.iter().map(|p| p.text.len() + 2).sum();
        let mut out = String::with_capacity(cap);
        let mut after = 0u8;
        for (i, p) in pieces.iter().enumerate() {
            if i > 0 {
                out.extend(std::iter::repeat_n('\n', usize::from(after.max(p.breaks[0]).max(1))));
            }
            out.push_str(&p.text);
            after = p.breaks[1];
        }
        let rc: Rc<str> = out.into();
        quote_plain_cache.insert(key, (task, bounds, rc.clone()));
        rc
    }
}

/// Iterative walk producing the plain-text [`Piece`]s of `root_blocks`, consulting `cache` whenever
/// a nested [`Block::Quote`] with no pending list marker was already formatted bottom-up.
#[allow(clippy::too_many_arguments)]
fn plain_iter(
    root_blocks: &[Block],
    tasks: &[Option<(bool, usize)>],
    task: &mut usize,
    root_cols: usize,
    root_lists: u32,
    root_lead: Option<(String, u8)>,
    cache: &QuotePlainCache,
    out: &mut Vec<Piece>,
) {
    let mut stack = vec![PlainFrame::Blocks {
        blocks: root_blocks,
        idx: 0,
        cols: root_cols,
        lists: root_lists,
        lead: root_lead,
    }];
    while let Some(top) = stack.last_mut() {
        match top {
            PlainFrame::Blocks { blocks, idx, cols, lists, lead } => {
                let blocks_slice = *blocks;
                if *idx >= blocks_slice.len() {
                    stack.pop();
                    continue;
                }
                let b = &blocks_slice[*idx];
                *idx += 1;
                let here = lead.take();
                match b {
                    Block::Para(inl) | Block::Heading(_, inl) => {
                        let text = unpadded(&inl.text);
                        out.push(match here {
                            Some((marker, before)) => Piece { breaks: [before, 1], text: marker + &text },
                            None => Piece { breaks: [2, 2], text },
                        });
                    }
                    Block::Code(s) | Block::Diff(s) | Block::Mermaid(s) => {
                        out.push(Piece { breaks: [2, 2], text: s.trim_end_matches('\n').to_owned() });
                    }
                    Block::Quote(inner) => {
                        if here.is_none()
                            && let Some(&(end_task, breaks, ref cached)) =
                                cache.get(&(inner.as_ptr() as usize, *cols, *lists))
                        {
                            *task = end_task;
                            if !cached.is_empty() {
                                out.push(Piece { breaks, text: (**cached).to_owned() });
                            }
                            continue;
                        }
                        let (c, l) = (*cols, *lists);
                        stack.push(PlainFrame::Blocks { blocks: inner, idx: 0, cols: c, lists: l, lead: here });
                    }
                    Block::List { start, items } => {
                        let (c, l) = (*cols, *lists);
                        stack.push(PlainFrame::List { start: *start, items, idx: 0, cols: c, lists: l });
                    }
                    Block::Table(t) => {
                        let row = |cells: &[Inline]| cells.iter().map(|c| unpadded(&c.text)).collect::<Vec<_>>().join("\t");
                        let rows: Vec<String> = std::iter::once(row(&t.head)).chain(t.rows.iter().map(|r| row(r))).collect();
                        out.push(Piece { breaks: [1, 1], text: rows.join("\n") });
                    }
                    Block::Image { .. } | Block::Rule => {}
                }
            }
            PlainFrame::List { start, items, idx, cols, lists } => {
                if *idx >= items.len() {
                    stack.pop();
                    continue;
                }
                let k = *idx;
                *idx += 1;
                let checked = tasks.get(*task).copied().flatten().map(|(checked, _)| checked);
                *task += 1;
                let mark = match (checked, *start) {
                    (Some(done), _) => Lead::Task(done),
                    (None, Some(n)) => Lead::Number(n + k as u64),
                    (None, None) => Lead::Bullet,
                };
                let marker = format!("{}{}", " ".repeat(*cols), mark.marker());
                let before = if k == 0 && *lists == 0 { 2 } else { 1 };
                let item_blocks = &(*items)[k];
                let (next_cols, next_lists) = (*cols + mark.width(), *lists + 1);
                stack.push(PlainFrame::Blocks {
                    blocks: item_blocks,
                    idx: 0,
                    cols: next_cols,
                    lists: next_lists,
                    lead: Some((marker, before)),
                });
            }
        }
    }
}
