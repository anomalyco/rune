//! Laying out and placing child elements inside a text element's in-flow
//! [`InlineBox`](crate::InlineBox) slots right after `lay_out_text`, without turning the text
//! node into a Taffy container.

use std::borrow::Cow;

use parley::PositionedLayoutItem;

use super::*;
use crate::el::{Flow, Pin};
use crate::text::{INLINE_BOX, InlineBoxRun, TextFlow};

/// Whether node `i` is an elided inline child of a text node (zeroed out by
/// [`place_inline_children`] when `truncate` or `max_lines` cut its box). Paint, hit testing,
/// AccessKit, and the web mirror skip the child and its entire subtree while hidden; widening
/// the text restores it on relayout.
#[inline]
pub(crate) fn is_elided_inline_child(nodes: &[Node], i: usize) -> bool {
    let n = &nodes[i];
    n.local.w <= 0.0
        && n.local.h <= 0.0
        && n.target == [0.0; 4]
        && n.parent.is_some_and(|p| {
            nodes[p]
                .text
                .as_ref()
                .is_some_and(|t| !t.cold.inline_boxes.is_empty())
        })
}

/// Lays out and places every inline child of text node `parent_idx` using its laid-out Parley
/// `PositionedInlineBox` coordinates, and zeroes any child whose box was elided by `truncate`
/// or `max_lines`.
pub(super) fn place_inline_children(
    taffy: &mut LayoutEngine,
    text: &mut TextSystem,
    nodes: &mut [Node],
    parent_idx: usize,
    scale: f32,
) {
    let Some(spec) = nodes[parent_idx].text.as_deref() else { return };
    if spec.cold.inline_boxes.is_empty() {
        return;
    }
    let Some(crate::text::TextLayout::Single(layout)) = nodes[parent_idx].text_layout.as_ref() else {
        return;
    };
    let layout = layout.clone();
    let flow = TextFlow::Single(&layout);
    let x0 = nodes[parent_idx].inset[0];
    let x1 = (nodes[parent_idx].local.w - nodes[parent_idx].inset[2]).max(x0);
    let y0 = nodes[parent_idx].inset[1];
    let y1 = (nodes[parent_idx].local.h - nodes[parent_idx].inset[3]).max(y0);
    let ox = text_x(spec, x0, x1, text_width(layout.full_width()) / scale, flow.flow());
    let oy = text_y(spec, y0, y1, text_height(spec, flow, scale) / scale);
    let parent_target = [nodes[parent_idx].target[0], nodes[parent_idx].target[1]];

    let runs: Cow<'_, [InlineBoxRun]> = match nodes[parent_idx].elided {
        None => Cow::Borrowed(&spec.cold.inline_boxes),
        Some(elision) => Cow::Owned(elision.inline_boxes(&spec.cold.inline_boxes, spec.content.len())),
    };

    // Collect direct children of `parent_idx` in tree order and each child's subtree end.
    let mut direct: Vec<(usize, usize)> = Vec::new();
    let mut scan = parent_idx + 1;
    while scan < nodes.len() && nodes[scan].parent.is_some_and(|p| p >= parent_idx) {
        if nodes[scan].parent == Some(parent_idx) {
            let start = scan;
            scan += 1;
            while scan < nodes.len() && nodes[scan].parent.is_some_and(|p| p >= start) {
                scan += 1;
            }
            direct.push((start, scan));
        } else {
            scan += 1;
        }
    }
    if direct.is_empty() {
        return;
    }

    // Map direct child ordinal -> placed `[x, y, w, h]` in parent-local points.
    let mut placed: Vec<Option<[f32; 4]>> = vec![None; direct.len()];
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::InlineBox(b) = item else { continue };
            if b.id & INLINE_BOX == 0 {
                continue;
            }
            let run_idx = (b.id & !INLINE_BOX) as usize;
            let Some(run) = runs.get(run_idx) else { continue };
            let ord = run.child as usize;
            if let Some(slot) = placed.get_mut(ord) {
                *slot = Some([
                    ox + b.x / scale,
                    oy + b.y / scale,
                    b.width / scale,
                    b.height / scale,
                ]);
            }
        }
    }

    // Solve Taffy layout for each placed child at its fixed `(w, h)` before mutating `nodes`.
    for (&(child_idx, _), rect) in direct.iter().zip(&placed) {
        let Some([_, _, w, h]) = *rect else { continue };
        let room = Size {
            width: AvailableSpace::Definite(w.max(0.0)),
            height: AvailableSpace::Definite(h.max(0.0)),
        };
        compute_layout(taffy, nodes[child_idx].taffy, room, nodes, text, scale);
    }

    // Write `local`, `target`, and `inset` for each child subtree (or zero if elided).
    for (&(child_idx, sub_end), rect) in direct.iter().zip(&placed) {
        if let Some([cx, cy, cw, ch]) = *rect {
            let mut l = taffy.box_layout(nodes[child_idx].taffy);
            if nodes[child_idx].flow == Flow::Rtl {
                l.inset.swap(0, 2);
            }
            let w = if l.w > 0.0 || cw == 0.0 { l.w } else { cw };
            let h = if l.h > 0.0 || ch == 0.0 { l.h } else { ch };
            let tx = parent_target[0] + cx;
            let ty = parent_target[1] + cy;
            nodes[child_idx].local = MRect::new(cx, cy, w, h);
            nodes[child_idx].target = [tx, ty, tx + w, ty + h];
            nodes[child_idx].inset = l.inset;

            for d in child_idx + 1..sub_end {
                let mut dl = taffy.box_layout(nodes[d].taffy);
                if let Some(p) = nodes[d].parent.filter(|&p| nodes[p].flow == Flow::Rtl && nodes[d].pin == Pin::Flowing) {
                    dl.x = nodes[p].local.w - dl.x - dl.w;
                }
                if nodes[d].flow == Flow::Rtl {
                    dl.inset.swap(0, 2);
                }
                nodes[d].local = MRect::new(dl.x, dl.y, dl.w, dl.h);
                let (px, py) = nodes[d].parent.map_or((0.0, 0.0), |p| (nodes[p].target[0], nodes[p].target[1]));
                let dx = px + dl.x;
                let dy = py + dl.y;
                nodes[d].target = [dx, dy, dx + dl.w, dy + dl.h];
                nodes[d].inset = dl.inset;
            }
        } else {
            for n in &mut nodes[child_idx..sub_end] {
                n.local = MRect::ZERO;
                n.target = [0.0; 4];
                n.inset = [0.0; 4];
            }
        }
    }
}

/// Re-places every inline child of live-updated text node `parent_idx`, refreshes nested text
/// inside placed children, lands each child subtree's updated frame in `scene`, and bumps their
/// paint epochs so `PaintKeep` repaints moved or newly shown/hidden inline children on this frame.
pub(in crate::runtime) fn place_inline_children_live(
    taffy: &mut LayoutEngine,
    text: &mut TextSystem,
    nodes: &mut [Node],
    scene: &mut Scene,
    parent_idx: usize,
    scale: f32,
) {
    place_inline_children(taffy, text, nodes, parent_idx, scale);
    let mut sub_end = parent_idx + 1;
    while sub_end < nodes.len() && nodes[sub_end].parent.is_some_and(|p| p >= parent_idx) {
        sub_end += 1;
    }
    for d in parent_idx + 1..sub_end {
        if nodes[d].text.is_some() && (nodes[d].local.w > 0.0 || nodes[d].local.h > 0.0) {
            let w = (taffy.box_layout(nodes[d].taffy).w - nodes[d].inset[0] - nodes[d].inset[2]).max(0.0);
            super::relayout::lay_out_text(text, &mut nodes[d], w, scale);
            if nodes[d].text.as_ref().is_some_and(|t| !t.cold.inline_boxes.is_empty()) {
                place_inline_children(taffy, text, nodes, d, scale);
            }
        }
        if scene.contains(nodes[d].id) {
            scene.land_frame(nodes[d].id, nodes[d].local);
        }
        nodes[d].epoch = crate::runtime::paint_keep::next_epoch();
    }
}
