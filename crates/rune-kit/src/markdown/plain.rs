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

/// Keyed by `(quote_block_ptr, first_task, cols, lists) -> (task_delta, outer_breaks, text)`.
pub(super) type QuotePlainCache = HashMap<(usize, usize, usize, u32), (usize, [u8; 2], Rc<str>)>;

enum PlainFrame<'a> {
    Blocks {
        blocks: &'a [Block],
        maps: &'a [BlockMap],
        idx: usize,
        cols: usize,
        lists: u32,
        lead: Option<(String, u8)>,
    },
    List {
        start: Option<u64>,
        items: &'a [Vec<Block>],
        item_maps: &'a [Vec<BlockMap>],
        idx: usize,
        cols: usize,
        lists: u32,
    },
}

impl Renderer {
    /// `inner`, drawn with its first list item at `tasks[first_task]`, inside `flow`'s lists.
    /// Keyed by `quote_block`'s own address and `first_task` (with `task_delta` stored, and empty
    /// quotes never cached) so empty slices never collide and `task` never rewinds.
    pub(super) fn plain_text_rc(
        &self,
        quote_block: &Block,
        inner: &[Block],
        inner_maps: &[BlockMap],
        first_task: usize,
        flow: Flow,
        quote_plain_cache: &mut QuotePlainCache,
    ) -> Rc<str> {
        if inner.is_empty() {
            return Rc::from("");
        }
        let key = (std::ptr::from_ref(quote_block) as usize, first_task, flow.cols, flow.lists);
        if let Some((_, _, cached)) = quote_plain_cache.get(&key) {
            return cached.clone();
        }
        if let [child @ Block::Quote(grand)] = inner {
            if grand.is_empty() {
                return Rc::from("");
            }
            let child_key = (std::ptr::from_ref(child) as usize, first_task, flow.cols, flow.lists);
            if let Some(&(task_delta, breaks, ref cached)) = quote_plain_cache.get(&child_key) {
                let rc = cached.clone();
                quote_plain_cache.insert(key, (task_delta, breaks, rc.clone()));
                return rc;
            }
        }
        let mut pieces = Vec::new();
        let mut task = first_task;
        plain_iter(inner, inner_maps, &self.tasks, &mut task, flow.cols, flow.lists, None, quote_plain_cache, &mut pieces);
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
        let task_delta = task.saturating_sub(first_task);
        quote_plain_cache.insert(key, (task_delta, bounds, rc.clone()));
        rc
    }
}

#[allow(clippy::too_many_arguments)]
fn plain_iter(
    root_blocks: &[Block],
    root_maps: &[BlockMap],
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
        maps: root_maps,
        idx: 0,
        cols: root_cols,
        lists: root_lists,
        lead: root_lead,
    }];
    while let Some(top) = stack.last_mut() {
        match top {
            PlainFrame::Blocks { blocks, maps, idx, cols, lists, lead } => {
                let blocks_slice = *blocks;
                let maps_slice = *maps;
                if *idx >= blocks_slice.len() {
                    stack.pop();
                    continue;
                }
                let i = *idx;
                let b = &blocks_slice[i];
                let m = maps_slice.get(i);
                *idx += 1;
                let here = lead.take();
                match (b, m) {
                    (Block::Para(inl), Some(BlockMap::DeepTail { .. })) => {
                        let pad = " ".repeat(*cols);
                        let mut text = String::with_capacity(inl.text.len() + 16);
                        for (j, line) in inl.text.lines().enumerate() {
                            if j > 0 {
                                text.push('\n');
                            }
                            text.push_str(&pad);
                            text.push_str(line);
                        }
                        out.push(Piece { breaks: [2, 2], text });
                    }
                    (Block::Para(inl) | Block::Heading(_, inl), _) => {
                        let text = unpadded(&inl.text);
                        out.push(match here {
                            Some((marker, before)) => Piece { breaks: [before, 1], text: marker + &text },
                            None => Piece { breaks: [2, 2], text },
                        });
                    }
                    (Block::Code(s) | Block::Diff(s) | Block::Mermaid(s), _) => {
                        out.push(Piece { breaks: [2, 2], text: s.trim_end_matches('\n').to_owned() });
                    }
                    (Block::Quote(inner), map_opt) => {
                        if inner.is_empty() {
                            continue;
                        }
                        if here.is_none()
                            && let Some(&(task_delta, breaks, ref cached)) =
                                cache.get(&(std::ptr::from_ref(b) as usize, *task, *cols, *lists))
                        {
                            *task += task_delta;
                            if !cached.is_empty() {
                                out.push(Piece { breaks, text: (**cached).to_owned() });
                            }
                            continue;
                        }
                        let inner_maps = match map_opt {
                            Some(BlockMap::Quote(im)) => im.as_slice(),
                            _ => &[][..],
                        };
                        let (c, l) = (*cols, *lists);
                        stack.push(PlainFrame::Blocks { blocks: inner, maps: inner_maps, idx: 0, cols: c, lists: l, lead: here });
                    }
                    (Block::List { start, items }, map_opt) => {
                        let item_maps = match map_opt {
                            Some(BlockMap::List(im)) => im.as_slice(),
                            _ => &[][..],
                        };
                        let (c, l) = (*cols, *lists);
                        stack.push(PlainFrame::List { start: *start, items, item_maps, idx: 0, cols: c, lists: l });
                    }
                    (Block::Table(t), _) => {
                        let row = |cells: &[Inline]| cells.iter().map(|c| unpadded(&c.text)).collect::<Vec<_>>().join("\t");
                        let rows: Vec<String> = std::iter::once(row(&t.head)).chain(t.rows.iter().map(|r| row(r))).collect();
                        out.push(Piece { breaks: [1, 1], text: rows.join("\n") });
                    }
                    (Block::Image { .. } | Block::Rule, _) => {}
                }
            }
            PlainFrame::List { start, items, item_maps, idx, cols, lists } => {
                if *idx >= items.len() {
                    stack.pop();
                    continue;
                }
                let k = *idx;
                *idx += 1;
                let item_blocks = &(*items)[k];
                let imap = (*item_maps).get(k).map_or(&[][..], Vec::as_slice);
                if matches!(imap, [BlockMap::DeepTail { .. }]) {
                    *task += 1;
                    let (c, next_lists) = (*cols, *lists + 1);
                    stack.push(PlainFrame::Blocks { blocks: item_blocks, maps: imap, idx: 0, cols: c, lists: next_lists, lead: None });
                    continue;
                }
                let checked = tasks.get(*task).copied().flatten().map(|(checked, _)| checked);
                *task += 1;
                let mark = match (checked, *start) {
                    (Some(done), _) => Lead::Task(done),
                    (None, Some(n)) => Lead::Number(n + k as u64),
                    (None, None) => Lead::Bullet,
                };
                let marker = format!("{}{}", " ".repeat(*cols), mark.marker());
                let before = if k == 0 && *lists == 0 { 2 } else { 1 };
                let (next_cols, next_lists) = (*cols + mark.width(), *lists + 1);
                stack.push(PlainFrame::Blocks {
                    blocks: item_blocks,
                    maps: imap,
                    idx: 0,
                    cols: next_cols,
                    lists: next_lists,
                    lead: Some((marker, before)),
                });
            }
        }
    }
}
