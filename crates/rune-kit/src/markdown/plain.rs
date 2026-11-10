//! Blocks as the plain text a selection of all of them copies: what [`Markdown::quote_frame`]
//! hands an app for a whole quote. It lays the blocks out as the texts' `copy_breaks` and
//! `copy_prefix` do (`render.rs`).

use super::render::{Flow, Lead};
use super::*;

/// A block's text and the newlines it asks for before and after it.
struct Piece {
    breaks: [u8; 2],
    text: String,
}

impl Renderer {
    /// `blocks`, drawn with their first list item at `tasks[first_task]`, inside `flow`'s lists.
    pub(super) fn plain_text(&self, blocks: &[Block], first_task: usize, flow: Flow) -> String {
        let mut pieces = Vec::new();
        plain(blocks, &self.tasks, &mut { first_task }, flow.cols, flow.lists, None, &mut pieces);
        let mut out = String::new();
        let mut after = 0u8;
        for (i, p) in pieces.iter().enumerate() {
            if i > 0 {
                out.extend(std::iter::repeat_n('\n', usize::from(after.max(p.breaks[0]).max(1))));
            }
            out.push_str(&p.text);
            after = p.breaks[1];
        }
        out
    }
}

/// `lead`: the marker before the first block's text, with the newlines before it.
fn plain(blocks: &[Block], tasks: &[Option<(bool, usize)>], task: &mut usize, cols: u8, lists: u32, lead: Option<(String, u8)>, out: &mut Vec<Piece>) {
    let mut lead = lead;
    for b in blocks {
        let here = lead.take();
        match b {
            Block::Para(inl) | Block::Heading(_, inl) => {
                let text = unpadded(&inl.text);
                out.push(match here {
                    Some((marker, before)) => Piece { breaks: [before, 1], text: marker + &text },
                    None => Piece { breaks: [2, 2], text },
                });
            }
            Block::Code(s) | Block::Diff(s) | Block::Mermaid(s) => out.push(Piece { breaks: [2, 2], text: s.trim_end_matches('\n').to_owned() }),
            Block::Quote(inner) => plain(inner, tasks, task, cols, lists, here, out),
            Block::List { start, items } => {
                for (k, item) in items.iter().enumerate() {
                    let checked = tasks.get(*task).copied().flatten().map(|(checked, _)| checked);
                    *task += 1;
                    let mark = match (checked, start) {
                        (Some(done), _) => Lead::Task(done),
                        (None, Some(n)) => Lead::Number(n + k as u64),
                        (None, None) => Lead::Bullet,
                    };
                    let marker = format!("{}{}", " ".repeat(cols.into()), mark.marker());
                    let before = if k == 0 && lists == 0 { 2 } else { 1 };
                    plain(item, tasks, task, cols + mark.width(), lists + 1, Some((marker, before)), out);
                }
            }
            Block::Table(t) => {
                let row = |cells: &[Inline]| cells.iter().map(|c| unpadded(&c.text)).collect::<Vec<_>>().join("\t");
                let rows: Vec<String> = std::iter::once(row(&t.head)).chain(t.rows.iter().map(|r| row(r))).collect();
                out.push(Piece { breaks: [1, 1], text: rows.join("\n") });
            }
            Block::Image { .. } | Block::Rule => {}
        }
    }
}
