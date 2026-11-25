//! Where `El::anchored` overlays go, how long an `El::anchored_fit` one may be, and the zoom
//! one inherits from its anchor.
use super::*;
use crate::el::{Anchor, Flow, Pin, Side};

/// Maximum depth of chained anchored overlays followed for zoom or target placement.
pub(crate) const MAX_ANCHOR_CHAIN: usize = 32;

/// Whether `candidate` is `root` or inside `root`'s subtree.
fn in_subtree(nodes: &[Node], mut candidate: usize, root: usize) -> bool {
    loop {
        if candidate == root { return true; }
        let Some(p) = nodes[candidate].parent else { return false };
        candidate = p;
    }
}

/// The node an `El::above` / `El::anchored` anchor `a` names, preferring an earlier node and
/// falling back to a later one (for example a trigger inside a dialog mounted after its portal),
/// never matching `overlay` itself or any of its descendants.
pub(super) fn anchor_index(nodes: &[Node], overlay: usize, a: &Anchor) -> Option<usize> {
    nodes[..overlay].iter().position(|n| n.key.as_ref() == Some(&a.key))
        .or_else(|| {
            nodes[overlay + 1..].iter().enumerate()
                .find(|(off, n)| n.key.as_ref() == Some(&a.key) && !in_subtree(nodes, overlay + 1 + *off, overlay))
                .map(|(off, _)| overlay + 1 + off)
        })
}

/// Like [`anchor_index`], preferring a live (non-exiting) anchor when both a live and an exiting
/// node share the key.
pub(super) fn anchor_index_live(nodes: &[Node], overlay: usize, a: &Anchor, exiting: impl Fn(usize) -> bool) -> Option<usize> {
    let mut fallback = None;
    for idx in (0..overlay).chain(overlay + 1..nodes.len()) {
        if nodes[idx].key.as_ref() != Some(&a.key) { continue; }
        if idx > overlay && in_subtree(nodes, idx, overlay) { continue; }
        if !exiting(idx) { return Some(idx); }
        fallback.get_or_insert(idx);
    }
    fallback
}

/// Where a walk from node `i` goes next: the node it is anchored above, else its parent.
pub(super) fn step_up(nodes: &[Node], i: usize) -> Option<usize> {
    nodes[i].above.as_ref().and_then(|a| anchor_index(nodes, i, a)).or(nodes[i].parent)
}

/// Walks upward from `start` via [`step_up`], stopping before any cycle.
pub(super) fn anchor_ancestors(nodes: &[Node], start: usize) -> impl Iterator<Item = usize> + '_ {
    let limit = nodes.len();
    let mut count = 0usize;
    std::iter::successors(Some(start), move |&p| {
        count += 1;
        (count < limit).then(|| step_up(nodes, p)).flatten()
    })
}

/// The boundary node `El::anchored_in` names for `overlay`, preferring an earlier node and
/// falling back to a later one, never matching `overlay` itself or any of its descendants.
pub(super) fn boundary_index(nodes: &[Node], overlay: usize) -> Option<usize> {
    let key = nodes[overlay].above_boundary.as_deref()?;
    nodes[..overlay].iter().position(|n| n.key.as_deref() == Some(key))
        .or_else(|| {
            nodes[overlay + 1..].iter().enumerate()
                .find(|(off, n)| n.key.as_deref() == Some(key) && !in_subtree(nodes, overlay + 1 + *off, overlay))
                .map(|(off, _)| overlay + 1 + off)
        })
}

/// Like [`boundary_index`], preferring a live (non-exiting) boundary node when both a live and
/// an exiting node share the key.
pub(super) fn boundary_index_live(nodes: &[Node], overlay: usize, exiting: impl Fn(usize) -> bool) -> Option<usize> {
    let key = nodes[overlay].above_boundary.as_deref()?;
    let mut fallback = None;
    for idx in (0..overlay).chain(overlay + 1..nodes.len()) {
        if nodes[idx].key.as_deref() != Some(key) { continue; }
        if idx > overlay && in_subtree(nodes, idx, overlay) { continue; }
        if !exiting(idx) { return Some(idx); }
        fallback.get_or_insert(idx);
    }
    fallback
}

/// Clamps boundary rect `b` into the window viewport so overlays never escape the window.
fn intersect_window(b: [f32; 4], vp: (f32, f32)) -> [f32; 4] {
    let x0 = b[0].clamp(0.0, vp.0);
    let y0 = b[1].clamp(0.0, vp.1);
    let x1 = b[2].clamp(x0, vp.0);
    let y1 = b[3].clamp(y0, vp.1);
    [x0, y0, x1, y1]
}

/// The bounds rect `[x0, y0, x1, y1]` for boundary node `b`, intersected with the window, or
/// the full window viewport when `b` is `None`.
pub(super) fn anchor_bounds(nodes: &[Node], scrolls: &HashMap<NodeId, ScrollState>, b: Option<usize>, vp: (f32, f32)) -> [f32; 4] {
    anchor_bounds_depth(nodes, scrolls, b, vp, 0)
}

/// Depth-bounded [`anchor_bounds`] for recursive anchored-target walks.
pub(super) fn anchor_bounds_depth(
    nodes: &[Node],
    scrolls: &HashMap<NodeId, ScrollState>,
    b: Option<usize>,
    vp: (f32, f32),
    depth: usize,
) -> [f32; 4] {
    let Some(b) = b else { return [0.0, 0.0, vp.0, vp.1]; };
    if depth >= MAX_ANCHOR_CHAIN {
        return [0.0, 0.0, vp.0, vp.1];
    }
    intersect_window(sticky::anchored_target_inner(nodes, scrolls, b, vp, depth), vp)
}

/// Whether any ancestor from `start` up to root carries a scale or anchor.
fn has_ancestor_scale(nodes: &[Node], mut at: Option<usize>) -> bool {
    let mut limit = nodes.len();
    while let Some(p) = at {
        if limit == 0 { break; }
        limit -= 1;
        let n = &nodes[p];
        if (n.static_scale - 1.0).abs() > 1e-5 || n.above.is_some() {
            return true;
        }
        at = n.parent;
    }
    false
}

/// Node `idx`'s rect after its own and its ancestors' static scales, with a zero-allocation
/// fast path when unscaled.
fn node_zoomed_rect(nodes: &[Node], scrolls: &HashMap<NodeId, ScrollState>, idx: usize, t: [f32; 4], vp: (f32, f32)) -> [f32; 4] {
    let own = nodes[idx].static_scale;
    if (own - 1.0).abs() <= 1e-5 && !has_ancestor_scale(nodes, nodes[idx].parent) {
        return t;
    }
    let mut r = zoomed_rect(nodes, scrolls, idx, t, vp);
    if (own - 1.0).abs() > 1e-5 {
        let [ox, oy] = nodes[idx].vis.origin.unwrap_or([0.5, 0.5]);
        let px = r[0] + ox * (r[2] - r[0]);
        let py = r[1] + oy * (r[3] - r[1]);
        r = [px + (r[0] - px) * own, py + (r[1] - py) * own, px + (r[2] - px) * own, py + (r[3] - py) * own];
    }
    r
}

/// Zoomed boundary rect intersected with the window, for `fit_anchored` and zoomed placement.
fn anchor_bounds_zoomed(nodes: &[Node], scrolls: &HashMap<NodeId, ScrollState>, b: Option<usize>, vp: (f32, f32)) -> [f32; 4] {
    let Some(b) = b else { return [0.0, 0.0, vp.0, vp.1]; };
    let t = sticky::anchored_target(nodes, scrolls, b, vp);
    intersect_window(node_zoomed_rect(nodes, scrolls, b, t, vp), vp)
}

/// How far an anchored overlay stays inside its bounds (window or `El::anchored_in` boundary).
const MARGIN: f32 = 8.0;

/// Where an anchored overlay of `size` goes next to `t` (x0, y0, x1, y1) inside `bounds`
/// (x0, y0, x1, y1), flipped to the opposite side and alignment when it doesn't fit, then kept
/// 8 points inside `bounds`. Returns `(x, y, resolved_side, resolved_align)`.
pub(crate) fn anchor_position(a: &Anchor, t: [f32; 4], size: (f32, f32), bounds: [f32; 4]) -> (f32, f32, Side, Align) {
    debug_assert!(t.iter().all(|v| v.is_finite()), "anchor_position called with non-finite target {t:?}");
    debug_assert!(bounds.iter().all(|v| v.is_finite()), "anchor_position called with non-finite bounds {bounds:?}");
    let (w, h) = size;
    let margin = MARGIN;
    let (left_min, top_min) = (bounds[0] + margin, bounds[1] + margin);
    let (right_max, bottom_max) = (bounds[2] - margin, bounds[3] - margin);
    let along = |lo: f32, hi: f32, len: f32| match a.align {
        Align::Start => lo,
        Align::Center => (lo + hi - len) * 0.5,
        Align::End => hi - len,
    };
    let (mut side, mut align) = (a.side, a.align);
    let (mut x, mut y) = match a.side {
        Side::Top => (along(t[0], t[2], w), t[1] - h - a.gap),
        Side::Bottom => (along(t[0], t[2], w), t[3] + a.gap),
        Side::Left => (t[0] - w - a.gap, along(t[1], t[3], h)),
        Side::Right => (t[2] + a.gap, along(t[1], t[3], h)),
    };
    // Flip the side when the other one fits (Floating UI's `flip`).
    match a.side {
        Side::Bottom if y + h > bottom_max && t[1] - h - a.gap >= top_min => {
            y = t[1] - h - a.gap;
            side = Side::Top;
        }
        Side::Top if y < top_min && t[3] + a.gap + h <= bottom_max => {
            y = t[3] + a.gap;
            side = Side::Bottom;
        }
        Side::Right if x + w > right_max && t[0] - w - a.gap >= left_min => {
            x = t[0] - w - a.gap;
            side = Side::Left;
        }
        Side::Left if x < left_min && t[2] + a.gap + w <= right_max => {
            x = t[2] + a.gap;
            side = Side::Right;
        }
        _ => {}
    }
    // Flip the alignment when it overflows and the other end fits, then shift along the side.
    let vertical = matches!(a.side, Side::Top | Side::Bottom);
    let (pos, lo, hi, len, b_lo, b_hi) = if vertical {
        (&mut x, t[0], t[2], w, left_min, right_max)
    } else {
        (&mut y, t[1], t[3], h, top_min, bottom_max)
    };
    match a.align {
        Align::End if *pos < b_lo && lo + len <= b_hi => {
            *pos = lo;
            align = Align::Start;
        }
        Align::Start if *pos + len > b_hi && hi - len >= b_lo => {
            *pos = hi - len;
            align = Align::End;
        }
        _ => {}
    }
    *pos = pos.clamp(b_lo, (b_hi - len).max(b_lo));
    // Along the main axis only keep the start edge inside; a long list may run off the far end
    // (`El::anchored_fit` bounds it instead).
    if vertical { y = y.max(top_min) } else { x = x.max(left_min) }
    if a.round {
        // `Math.round`: halves go up.
        return ((x + 0.5).floor(), (y + 0.5).floor(), side, align);
    }
    (x, y, side, align)
}

/// The longest an `anchored_fit` overlay `len` long may be next to `t` inside `bounds`, or `None`
/// when it fits on its side or the other one as it is. The bound is the larger room (its own
/// side's on a tie), a little short of it so [`anchor_position`] puts the bounded overlay on
/// that side.
fn fit_bound(a: &Anchor, t: [f32; 4], len: f32, bounds: [f32; 4]) -> Option<f32> {
    let (before, after) = match a.side {
        Side::Top | Side::Bottom => (t[1] - a.gap - (bounds[1] + MARGIN), bounds[3] - MARGIN - (t[3] + a.gap)),
        Side::Left | Side::Right => (t[0] - a.gap - (bounds[0] + MARGIN), bounds[2] - MARGIN - (t[2] + a.gap)),
    };
    let (own, other) = if matches!(a.side, Side::Top | Side::Left) { (before, after) } else { (after, before) };
    if len <= own || len <= other {
        return None;
    }
    Some((own.max(other) - 0.5).floor().max(0.0))
}

/// The longest an `anchored_fit` overlay `cross_len` long may be along the cross axis inside
/// `bounds`, or `None` when it already fits with [`MARGIN`] on both ends.
fn cross_fit_bound(a: &Anchor, cross_len: f32, bounds: [f32; 4]) -> Option<f32> {
    let room = match a.side {
        Side::Top | Side::Bottom => bounds[2] - bounds[0] - 2.0 * MARGIN,
        Side::Left | Side::Right => bounds[3] - bounds[1] - 2.0 * MARGIN,
    };
    let room = room.max(0.0);
    if cross_len <= room {
        return None;
    }
    Some(room.floor().max(0.0))
}

/// Clamps `style`'s width ceiling to `max_w`, lowering any definite `min_width` above `max_w` so
/// CSS/Taffy min-over-max precedence does not let a floored surface escape its bounds.
fn clamp_max_width(style: &mut rune_taffy::CompactStyle, max_w: f32) {
    use taffy::util::MaybeResolve;
    style.set_max_width(length(max_w));
    if style.min_size().width.maybe_resolve(None, |_, _| 0.0).is_some_and(|min_w| min_w > max_w) {
        style.set_min_width(length(max_w));
    }
}

/// Clamps `style`'s height ceiling to `max_h`, lowering any definite `min_height` above `max_h` so
/// CSS/Taffy min-over-max precedence does not let a floored surface escape its bounds.
fn clamp_max_height(style: &mut rune_taffy::CompactStyle, max_h: f32) {
    use taffy::util::MaybeResolve;
    style.set_max_height(length(max_h));
    if style.min_size().height.maybe_resolve(None, |_, _| 0.0).is_some_and(|min_h| min_h > max_h) {
        style.set_min_height(length(max_h));
    }
}

/// Resolved `(Side, Align)` of anchored overlay `i` against anchor `j` and optional boundary `b`,
/// accounting for inherited zoom when present.
pub(super) fn resolved_placement(
    nodes: &[Node],
    scrolls: &HashMap<NodeId, ScrollState>,
    i: usize,
    j: usize,
    b: Option<usize>,
    a: &Anchor,
    size: (f32, f32),
    vp: (f32, f32),
) -> (Side, Align) {
    let target = sticky::anchored_target(nodes, scrolls, j, vp);
    let k = inherited_zoom(nodes, i, &|m: &Node| m.static_scale).max(MIN_ZOOM);
    if (k - 1.0).abs() <= 1e-5 {
        let bounds = anchor_bounds(nodes, scrolls, b, vp);
        let (_, _, side, align) = anchor_position(a, target, size, bounds);
        return (side, align);
    }
    let target = zoomed_rect(nodes, scrolls, j, target, vp);
    let bounds = anchor_bounds_zoomed(nodes, scrolls, b, vp);
    let zoomed = zoomed_gap(a, k);
    let (_, _, side, align) = anchor_position(&zoomed, target, (size.0 * k, size.1 * k), bounds);
    (side, align)
}

impl Measurement<'_> {
    /// Places every node from `taffy`'s layout: ids, local and window rects, insets, and
    /// anchored overlays beside their anchors.
    pub(super) fn place(&mut self, taffy: &LayoutEngine, nodes: &mut [Node]) {
        self.node_index.rebuild(nodes.len());
        let mut deferred = Vec::new();
        for i in 0..nodes.len() {
            let id = if nodes[i].id != NodeId(0) {
                debug_assert_eq!(self.path_ids.get(&nodes[i].path).copied(), Some(nodes[i].id));
                nodes[i].id
            } else {
                self.node_id(&nodes[i].path)
            };
            nodes[i].id = id;
            self.node_index.insert(id, i);
            let mut l = taffy.box_layout(nodes[i].taffy);
            // Right to left: laid out left to right, then mirrored inside the parent (`El::dir`).
            if let Some(p) = nodes[i].parent.filter(|&p| nodes[p].flow == Flow::Rtl && nodes[i].pin == Pin::Flowing) {
                l.x = nodes[p].local.w - l.x - l.w;
            }
            if nodes[i].flow == Flow::Rtl {
                l.inset.swap(0, 2);
            }
            nodes[i].local = MRect::new(l.x, l.y, l.w, l.h);
            let (px, py) = nodes[i].parent.map_or((0.0, 0.0), |p| (nodes[p].target[0], nodes[p].target[1]));
            let mut x = px + l.x;
            let mut y = py + l.y;
            let mut resolved = true;
            if let Some(a) = &nodes[i].above {
                if let Some(j) = anchor_index(nodes, i, a) {
                    let b = boundary_index(nodes, i);
                    if j < i && b.is_none_or(|b| b < i) && deferred.is_empty() {
                        // Anchors inside scroll viewports sit where they are scrolled (and stuck) to.
                        let target = sticky::anchored_target(nodes, self.scrolls, j, self.viewport);
                        let bounds = anchor_bounds(nodes, self.scrolls, b, self.viewport);
                        let (ax, ay, mut side, mut align) = anchor_position(a, target, (l.w, l.h), bounds);
                        let k = inherited_zoom(nodes, i, &|m: &Node| m.static_scale).max(MIN_ZOOM);
                        if (k - 1.0).abs() > 1e-5 {
                            let zt = zoomed_rect(nodes, self.scrolls, j, target, self.viewport);
                            let zb = anchor_bounds_zoomed(nodes, self.scrolls, b, self.viewport);
                            let za = zoomed_gap(a, k);
                            (_, _, side, align) = anchor_position(&za, zt, (l.w * k, l.h * k), zb);
                        }
                        (x, y) = (ax, ay);
                        nodes[i].local.x = x - px;
                        nodes[i].local.y = y - py;
                        nodes[i].cold_mut().resolved_anchor = Some((side, align));
                    } else {
                        deferred.push((i, j, b));
                    }
                } else {
                    // Never leave an unresolved anchored overlay at (0, 0) of the window.
                    nodes[i].local.x = 0.0;
                    nodes[i].local.y = 0.0;
                    if nodes[i].cold.get().is_some_and(|c| c.resolved_anchor.is_some()) {
                        nodes[i].cold_mut().resolved_anchor = None;
                    }
                    resolved = false;
                }
            }
            nodes[i].target = if resolved { [x, y, x + l.w, y + l.h] } else { [0.0; 4] };
            nodes[i].inset = l.inset;
        }
        if !deferred.is_empty() {
            self.place_deferred(nodes, &mut deferred);
        }
    }

    /// Places anchored overlays whose anchor or boundary appeared later in `nodes` (or inside
    /// another deferred overlay), then updates their descendants' target rects to match. Cyclic
    /// overlays that never resolve are suppressed at `[0.0; 4]`.
    fn place_deferred(&self, nodes: &mut [Node], deferred: &mut Vec<(usize, usize, Option<usize>)>) {
        let mut remaining = std::mem::take(deferred);
        while !remaining.is_empty() {
            let mut next = Vec::new();
            let mut progressed = false;
            for &(i, j, b) in &remaining {
                if remaining.iter().any(|&(k, _, _)| k != i && (in_subtree(nodes, j, k) || b.is_some_and(|b| in_subtree(nodes, b, k)))) {
                    next.push((i, j, b));
                    continue;
                }
                let a = nodes[i].above.as_ref().unwrap().clone();
                let target = sticky::anchored_target(nodes, self.scrolls, j, self.viewport);
                let bounds = anchor_bounds(nodes, self.scrolls, b, self.viewport);
                let (w, h) = (nodes[i].local.w, nodes[i].local.h);
                let (x, y, mut side, mut align) = anchor_position(&a, target, (w, h), bounds);
                let k = inherited_zoom(nodes, i, &|m: &Node| m.static_scale).max(MIN_ZOOM);
                if (k - 1.0).abs() > 1e-5 {
                    let zt = zoomed_rect(nodes, self.scrolls, j, target, self.viewport);
                    let zb = anchor_bounds_zoomed(nodes, self.scrolls, b, self.viewport);
                    let za = zoomed_gap(&a, k);
                    (_, _, side, align) = anchor_position(&za, zt, (w * k, h * k), zb);
                }
                let (px, py) = nodes[i].parent.map_or((0.0, 0.0), |p| (nodes[p].target[0], nodes[p].target[1]));
                nodes[i].local.x = x - px;
                nodes[i].local.y = y - py;
                nodes[i].target = [x, y, x + w, y + h];
                nodes[i].cold_mut().resolved_anchor = Some((side, align));
                for c in i + 1..nodes.len() {
                    let Some(p) = nodes[c].parent else { break };
                    if p < i { break; }
                    if nodes[c].above.is_some() { continue; }
                    let (cpx, cpy) = (nodes[p].target[0], nodes[p].target[1]);
                    let cx = cpx + nodes[c].local.x;
                    let cy = cpy + nodes[c].local.y;
                    nodes[c].target = [cx, cy, cx + nodes[c].local.w, cy + nodes[c].local.h];
                }
                progressed = true;
            }
            if !progressed {
                for (i, _, _) in next {
                    nodes[i].local.x = 0.0;
                    nodes[i].local.y = 0.0;
                    nodes[i].target = [0.0; 4];
                    if nodes[i].cold.get().is_some_and(|c| c.resolved_anchor.is_some()) {
                        nodes[i].cold_mut().resolved_anchor = None;
                    }
                }
                break;
            }
            remaining = next;
        }
    }

    /// Bounds each placed `anchored_fit` overlay that overflows its bounds on the main axis
    /// (neither side of its anchor fits) or the cross axis (wider/taller than the bounds minus
    /// margins), through its style. Records each clamped node's unclamped style in `saved` so
    /// a later `Relayout` can restore it before re-fitting. True when any changed.
    pub(super) fn fit_anchored(&self, taffy: &mut LayoutEngine, nodes: &[Node], saved: &mut Vec<(usize, CompactStyle)>) -> bool {
        let mut changed = false;
        for (i, n) in nodes.iter().enumerate() {
            let Some(a) = n.above.as_ref().filter(|a| a.fit) else { continue };
            let Some(j) = anchor_index(nodes, i, a) else { continue };
            let b = boundary_index(nodes, i);
            let target = sticky::anchored_target(nodes, self.scrolls, j, self.viewport);
            // It draws `k` times its layout size beside where the anchor draws.
            let k = inherited_zoom(nodes, i, &|m: &Node| m.static_scale).max(MIN_ZOOM);
            let target = if k == 1.0 { target } else { zoomed_rect(nodes, self.scrolls, j, target, self.viewport) };
            let bounds = anchor_bounds_zoomed(nodes, self.scrolls, b, self.viewport);
            let zoomed;
            let a = if k == 1.0 { a } else { zoomed = zoomed_gap(a, k); &zoomed };
            let vertical = matches!(a.side, Side::Top | Side::Bottom);
            let (main_len, cross_len) = if vertical { (n.local.h, n.local.w) } else { (n.local.w, n.local.h) };
            let main_bound = fit_bound(a, target, main_len * k, bounds);
            let cross_bound = cross_fit_bound(a, cross_len * k, bounds);
            if main_bound.is_none() && cross_bound.is_none() { continue; }
            let mut style = taffy.style(n.taffy).clone();
            saved.push((i, style.clone()));
            if let Some(bound) = main_bound {
                if vertical {
                    clamp_max_height(&mut style, bound / k);
                } else {
                    clamp_max_width(&mut style, bound / k);
                }
            }
            if let Some(bound) = cross_bound {
                if vertical {
                    clamp_max_width(&mut style, bound / k);
                } else {
                    clamp_max_height(&mut style, bound / k);
                }
            }
            taffy.set_style_and_mark_ancestors(
                n.taffy,
                style,
                std::iter::successors(n.parent, |&p| nodes[p].parent).map(|p| nodes[p].taffy),
            );
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
#[path = "anchor_boundary_tests.rs"]
mod boundary_tests;
#[cfg(test)]
#[path = "anchor_order_tests.rs"]
mod order_tests;
#[cfg(test)]
#[path = "anchor_placement_tests.rs"]
mod placement_tests;
#[cfg(test)]
#[path = "anchor_zoom_tests.rs"]
mod zoom_tests;
#[cfg(test)]
#[path = "anchor_zoom_motion_tests.rs"]
mod zoom_motion_tests;

/// The smallest zoom an anchored overlay is placed at, so placing it never divides by zero.
const MIN_ZOOM: f32 = 1e-3;

/// The zoom anchored overlay `i` draws at: its anchor's, the product of the `El::scale`s on the
/// anchor's ancestors (through any anchored overlay the anchor is in). It is 1 when the overlay
/// opts out with `El::anchored_unscaled`, or scales itself with `El::scale`, as an app that
/// already zooms its popovers by hand does. `own` reads a node's own `El::scale`.
pub(super) fn inherited_zoom(nodes: &[Node], i: usize, own: &impl Fn(&Node) -> f32) -> f32 {
    inherited_zoom_inner(nodes, i, own, 0)
}

fn inherited_zoom_inner(nodes: &[Node], i: usize, own: &impl Fn(&Node) -> f32, depth: usize) -> f32 {
    if depth >= MAX_ANCHOR_CHAIN { return 1.0; }
    let n = &nodes[i];
    let Some(a) = n.above.as_ref().filter(|a| !a.unscaled) else { return 1.0 };
    if n.scale_track || n.static_scale != 1.0 {
        return 1.0;
    }
    let Some(j) = anchor_index(nodes, i, a) else { return 1.0 };
    let mut k = 1.0;
    let mut at = nodes[j].parent;
    while let Some(p) = at {
        k *= own(&nodes[p]);
        if nodes[p].above.is_some() {
            return k * inherited_zoom_inner(nodes, p, own, depth + 1);
        }
        at = nodes[p].parent;
    }
    k
}

/// `t`, node `j`'s layout rect, where its ancestors' `El::scale`s draw it at rest. An anchored
/// ancestor zooms about its own top-left corner.
fn zoomed_rect(nodes: &[Node], scrolls: &HashMap<NodeId, ScrollState>, j: usize, t: [f32; 4], vp: (f32, f32)) -> [f32; 4] {
    let chain: Vec<usize> = std::iter::successors(nodes[j].parent, |&p| nodes[p].parent).collect();
    // A point `x` draws at `to + (x - from) * k`, as `Inh` composes scales down the tree.
    let (mut k, mut from, mut to) = (1.0f32, [0.0f32; 2], [0.0f32; 2]);
    for &p in chain.iter().rev() {
        let n = &nodes[p];
        if n.above.is_some() {
            // An overlay drops its ancestors' scales.
            (k, from, to) = (1.0, [0.0; 2], [0.0; 2]);
        }
        let own = n.static_scale * inherited_zoom(nodes, p, &|m: &Node| m.static_scale);
        if (own - 1.0).abs() <= 1e-5 {
            continue;
        }
        let r = sticky::anchored_target(nodes, scrolls, p, vp);
        let [ox, oy] = match (n.above.is_some(), n.vis.origin) {
            (true, _) => [0.0, 0.0],
            (false, Some(o)) => o,
            (false, None) => [0.5, 0.5],
        };
        let pivot = [r[0] + ox * (r[2] - r[0]), r[1] + oy * (r[3] - r[1])];
        to = [to[0] + (pivot[0] - from[0]) * k, to[1] + (pivot[1] - from[1]) * k];
        from = pivot;
        k *= own;
    }
    [to[0] + (t[0] - from[0]) * k, to[1] + (t[1] - from[1]) * k, to[0] + (t[2] - from[0]) * k, to[1] + (t[3] - from[1]) * k]
}

/// `a` with its gap at zoom `k`; the window margin stays in window points.
fn zoomed_gap(a: &Anchor, k: f32) -> Anchor {
    Anchor { gap: a.gap * k, ..a.clone() }
}

/// The point on drawn box `d` that touches its anchor `t`: the middle of the edge facing the
/// anchor, moved along it to the end `a.align` names. The overlay zooms about it, so it grows
/// away from its anchor.
fn attach_point(a: &Anchor, t: [f32; 4], d: [f32; 4]) -> [f32; 2] {
    let along = |lo: f32, hi: f32| match a.align {
        Align::Start => lo,
        Align::Center => (lo + hi) * 0.5,
        Align::End => hi,
    };
    let facing = |lo: f32, hi: f32, t_lo: f32, t_hi: f32| if lo + hi > t_lo + t_hi { lo } else { hi };
    match a.side {
        Side::Top | Side::Bottom => [along(d[0], d[2]), facing(d[1], d[3], t[1], t[3])],
        Side::Left | Side::Right => [facing(d[0], d[2], t[0], t[2]), along(d[1], d[3])],
    }
}

impl RuntimeCore {
    /// Whether node `i` carries an `El::above` / `El::anchored` anchor.
    pub(crate) fn has_anchor(&self, i: usize) -> bool {
        self.tree.nodes.get(i).is_some_and(|n| n.above.is_some())
    }

    /// The live anchor element `overlay`'s `El::above` / `El::anchored` points to, if any.
    pub(crate) fn anchor_of(&self, overlay: usize) -> Option<usize> {
        let a = self.tree.nodes.get(overlay)?.above.as_ref()?;
        anchor_index_live(&self.tree.nodes, overlay, a, |j| self.subtree_exiting(j))
    }

    /// Resolved boundary rect `[x0, y0, x1, y1]` for anchored overlay `i` during paint,
    /// intersected with the window, or the full window viewport when no boundary is set/found.
    fn anchored_bounds_drawn(&self, i: usize) -> [f32; 4] {
        let vp = self.viewport;
        let Some(b) = boundary_index_live(&self.tree.nodes, i, |idx| self.subtree_exiting(idx)) else {
            return [0.0, 0.0, vp.0, vp.1];
        };
        let d = self.paint.drawn[b];
        let raw = if d != [0.0; 4] {
            [d[0], d[1], d[0] + d[2], d[1] + d[3]]
        } else {
            let t = sticky::anchored_target(&self.tree.nodes, &self.scroll.scrolls, b, vp);
            node_zoomed_rect(&self.tree.nodes, &self.scroll.scrolls, b, t, vp)
        };
        intersect_window(raw, vp)
    }

    /// Sets up anchored overlay `i`'s inherited paint state beside anchor `j`: at the anchor's
    /// zoom (see [`inherited_zoom`]), placed and flipped by its zoomed size where the anchor was
    /// drawn, and zooming about the point it attaches at. Its own `prop::OFFSET` and transition
    /// still apply on top; its ancestors' positions and scales don't. Returns
    /// `(target, zoom, bounds)` so an exiting overlay can hold its last valid position if its
    /// anchor unmounts.
    pub(super) fn anchored_inh(&self, i: usize, j: usize, a: &Anchor, p: &rune_motion::Presented, pinh: &mut Inh, pcold: &mut InhCold) -> ([f32; 4], f32, [f32; 4]) {
        let tracked = |m: &Node| if m.scale_track { self.scene.get(m.id, P_SCALE).unwrap_or(m.static_scale) } else { m.static_scale };
        let k = inherited_zoom(&self.tree.nodes, i, &tracked).max(MIN_ZOOM);
        let d = self.paint.drawn[j];
        let target = if d != [0.0; 4] { [d[0], d[1], d[0] + d[2], d[1] + d[3]] } else {
            let t = sticky::anchored_target(&self.tree.nodes, &self.scroll.scrolls, j, self.viewport);
            if k == 1.0 { t } else { zoomed_rect(&self.tree.nodes, &self.scroll.scrolls, j, t, self.viewport) }
        };
        let bounds = self.anchored_bounds_drawn(i);
        let motion = self.tree.nodes[j].parent.and_then(|jp| self.paint.inh_scratch.get(jp)).map_or(ScaleMotion::Still, |h| h.scale_motion);
        self.anchored_inh_at(i, a, target, k, bounds, motion, p, pinh, pcold);
        (target, k, bounds)
    }

    /// Places anchored overlay `i` against a resolved `(target, k, bounds)` rect, zoom and bounds.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn anchored_inh_at(&self, i: usize, a: &Anchor, target: [f32; 4], k: f32, bounds: [f32; 4], motion: ScaleMotion, p: &rune_motion::Presented, pinh: &mut Inh, pcold: &mut InhCold) {
        let zoomed;
        let a = if k == 1.0 { a } else { zoomed = zoomed_gap(a, k); &zoomed };
        let (w, h) = (p.rect.w * k, p.rect.h * k);
        let (ax, ay, _, _) = anchor_position(a, target, (w, h), bounds);
        let pivot = attach_point(a, target, [ax, ay, ax + w, ay + h]);
        let world = self.scene.world(self.tree.nodes[i].id).unwrap_or(p.rect);
        (pinh.rigid, pcold.snap_steady, pcold.snap_layer) = ([0.0; 2], [0.0; 2], 0);
        if k == 1.0 {
            (pinh.dx, pinh.dy) = (ax - world.x, ay - world.y);
            (pinh.scale, pcold.anchor, pcold.anchor_to) = (1.0, [0.0; 2], [0.0; 2]);
            pcold.still = (1.0, [0.0; 2], [0.0; 2]);
            pcold.held = (1.0, [0.0; 2], [0.0; 2]);
            pinh.set_held_moving(false);
            pcold.scale_target = 1.0;
            pinh.set_scale_tracked(false);
            return;
        }
        pinh.dx = pivot[0] + (ax - pivot[0]) / k - world.x;
        pinh.dy = pivot[1] + (ay - pivot[1]) / k - world.y;
        (pinh.scale, pcold.anchor, pcold.anchor_to) = (k, pivot, pivot);
        // Text rasterizes as the anchor's does: crisp at the zoom once it rests.
        pinh.scale_motion = pinh.scale_motion.max(motion);
        pcold.still = if pinh.scale_motion == ScaleMotion::Still { (k, pivot, pivot) } else { (1.0, [0.0; 2], [0.0; 2]) };
        pcold.held = if pinh.scale_motion != ScaleMotion::Moving { (k, pivot, pivot) } else { (1.0, [0.0; 2], [0.0; 2]) };
        pinh.set_held_moving(pinh.scale_motion == ScaleMotion::Moving);
        pcold.scale_target = k;
        pinh.set_scale_tracked(false);
    }
}
