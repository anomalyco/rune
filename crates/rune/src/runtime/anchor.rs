//! Where `El::anchored` overlays go, and how long an `El::anchored_fit` one may be.
use super::*;
use crate::el::{Anchor, Side};

/// How far an anchored overlay stays inside the window.
const MARGIN: f32 = 8.0;

/// Where an anchored overlay of `size` goes next to `t` (x0, y0, x1, y1), flipped to the
/// opposite side when it doesn't fit, then kept 8 points inside the viewport.
pub(super) fn anchor_position(a: &Anchor, t: [f32; 4], size: (f32, f32), vp: (f32, f32)) -> (f32, f32) {
    let (w, h) = size;
    let margin = MARGIN;
    let along = |lo: f32, hi: f32, len: f32| match a.align {
        Align::Start => lo,
        Align::Center => (lo + hi - len) * 0.5,
        Align::End => hi - len,
    };
    let (mut x, mut y) = match a.side {
        Side::Top => (along(t[0], t[2], w), t[1] - h - a.gap),
        Side::Bottom => (along(t[0], t[2], w), t[3] + a.gap),
        Side::Left => (t[0] - w - a.gap, along(t[1], t[3], h)),
        Side::Right => (t[2] + a.gap, along(t[1], t[3], h)),
    };
    // Flip the side when the other one fits (Floating UI's `flip`).
    match a.side {
        Side::Bottom if y + h > vp.1 - margin && t[1] - h - a.gap >= margin => y = t[1] - h - a.gap,
        Side::Top if y < margin && t[3] + a.gap + h <= vp.1 - margin => y = t[3] + a.gap,
        Side::Right if x + w > vp.0 - margin && t[0] - w - a.gap >= margin => x = t[0] - w - a.gap,
        Side::Left if x < margin && t[2] + a.gap + w <= vp.0 - margin => x = t[2] + a.gap,
        _ => {}
    }
    // Flip the alignment when it overflows and the other end fits, then shift along the side.
    let vertical = matches!(a.side, Side::Top | Side::Bottom);
    let (pos, lo, hi, len, room) = if vertical { (&mut x, t[0], t[2], w, vp.0) } else { (&mut y, t[1], t[3], h, vp.1) };
    match a.align {
        Align::End if *pos < margin && lo + len <= room - margin => *pos = lo,
        Align::Start if *pos + len > room - margin && hi - len >= margin => *pos = hi - len,
        _ => {}
    }
    *pos = pos.clamp(margin, (room - len - margin).max(margin));
    // Along the main axis only keep the start edge inside; a long list may run off the far end
    // (`El::anchored_fit` bounds it instead).
    if vertical { y = y.max(margin) } else { x = x.max(margin) }
    (x, y)
}

/// The longest an `anchored_fit` overlay `len` long may be next to `t`, or `None` when it fits
/// on its side or the other one as it is. The bound is the larger room (its own side's on a
/// tie), a little short of it so [`anchor_position`] puts the bounded overlay on that side.
fn fit_bound(a: &Anchor, t: [f32; 4], len: f32, vp: (f32, f32)) -> Option<f32> {
    let (before, after) = match a.side {
        Side::Top | Side::Bottom => (t[1] - a.gap - MARGIN, vp.1 - MARGIN - (t[3] + a.gap)),
        Side::Left | Side::Right => (t[0] - a.gap - MARGIN, vp.0 - MARGIN - (t[2] + a.gap)),
    };
    let (own, other) = if matches!(a.side, Side::Top | Side::Left) { (before, after) } else { (after, before) };
    if len <= own || len <= other {
        return None;
    }
    Some((own.max(other) - 0.5).floor().max(0.0))
}

impl<S: 'static> Measurement<'_, S> {
    /// Places every node from `taffy`'s layout: ids, local and window rects, insets, and
    /// anchored overlays beside their anchors.
    pub(super) fn place(&mut self, taffy: &TaffyTree<usize>, nodes: &mut [Node]) {
        self.node_index.clear();
        self.node_index.reserve(nodes.len());
        for i in 0..nodes.len() {
            let id = self.node_id(&nodes[i].path);
            nodes[i].id = id;
            self.node_index.insert(id, i);
            let l = taffy.layout(nodes[i].taffy).unwrap();
            nodes[i].local = MRect::new(l.location.x, l.location.y, l.size.width, l.size.height);
            let (px, py) = nodes[i].parent.map_or((0.0, 0.0), |p| (nodes[p].target[0], nodes[p].target[1]));
            let mut x = px + l.location.x;
            let mut y = py + l.location.y;
            if let Some(a) = &nodes[i].above
                && let Some(j) = nodes[..i].iter().position(|n| n.key.as_ref() == Some(&a.key))
            {
                // Anchors inside scroll viewports sit where they are scrolled (and stuck) to.
                let target = sticky::anchored_target(nodes, self.scrolls, j, self.viewport);
                (x, y) = anchor_position(a, target, (l.size.width, l.size.height), self.viewport);
                nodes[i].local.x = x - px;
                nodes[i].local.y = y - py;
            }
            nodes[i].target = [x, y, x + l.size.width, y + l.size.height];
            nodes[i].inset = [
                l.padding.left + l.border.left,
                l.padding.top + l.border.top,
                l.padding.right + l.border.right,
                l.padding.bottom + l.border.bottom,
            ];
        }
    }

    /// Bounds each placed `anchored_fit` overlay that fits on neither side of its anchor to the
    /// larger room, through its style. True when any changed, so layout runs again.
    pub(super) fn fit_anchored(&self, taffy: &mut TaffyTree<usize>, nodes: &[Node]) -> bool {
        let mut changed = false;
        for (i, n) in nodes.iter().enumerate() {
            let Some(a) = n.above.as_ref().filter(|a| a.fit) else { continue };
            let Some(j) = nodes[..i].iter().position(|m| m.key.as_ref() == Some(&a.key)) else { continue };
            let target = sticky::anchored_target(nodes, self.scrolls, j, self.viewport);
            let vertical = matches!(a.side, Side::Top | Side::Bottom);
            let len = if vertical { n.local.h } else { n.local.w };
            let Some(bound) = fit_bound(a, target, len, self.viewport) else { continue };
            let mut style: Style = taffy.style(n.taffy).unwrap().clone().into();
            let max = if vertical { &mut style.max_size.height } else { &mut style.max_size.width };
            *max = length(bound);
            taffy.set_style(n.taffy, style).unwrap();
            changed = true;
        }
        changed
    }
}
