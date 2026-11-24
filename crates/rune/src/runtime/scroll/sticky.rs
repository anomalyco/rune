//! `El::sticky_top`, `El::sticky_bottom` and `El::sticky_left`: elements held to an edge of
//! their scroll viewport while their parent is in view (CSS `position: sticky`), and
//! `Tx::keep_on_screen`, which holds an element where it shows across a commit.
//!
//! The shift is worked out in the paint pass from the offsets that frame applies, so a stuck
//! header never trails a scroll. Layout is untouched: only drawing, hit testing and what tools
//! read move. A sticky element paints after its siblings (`update_layer_paint_order`).
use super::*;
use crate::el::StickyEdge;

/// No viewport band and no parent box: nothing to stick within.
pub(in crate::runtime) const UNBOUNDED: [f32; 8] = [f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY];

/// Points of shift over which `El::stuck_shadow` fades in.
pub(in crate::runtime) const SHADOW_RAMP: f32 = 8.0;

/// One fallback anchor target for `El::scroll_anchor` when the primary anchor node is removed
/// or begins exiting during the commit.
#[derive(Clone)]
pub(in crate::runtime) struct KeepCandidate {
    pub path: String,
    pub was: f32,
    /// When the element lives inside a virtual row: `(virtual_list_path, row_key, y_in_row)`.
    pub virtual_row: Option<(String, String, f32)>,
}

/// One `Tx::keep_on_screen` or `El::scroll_anchor` anchor captured before a commit.
#[derive(Clone)]
pub(in crate::runtime) struct Keep {
    pub path: String,
    pub viewport: String,
    pub was: f32,
    /// When the element lives inside a virtual row: `(virtual_list_path, row_key, y_in_row)`.
    pub virtual_row: Option<(String, String, f32)>,
    /// The element, or the header it's in, was stuck to the top: it stays there for the commit.
    pub hold: Option<Hold>,
    /// Ordered fallback candidates from `El::scroll_anchor` when `path` is removed by the commit.
    pub fallbacks: Vec<KeepCandidate>,
}

/// A header stuck to the top when a `Tx::keep_on_screen` commit started. Until the motion
/// settles it sticks `inset` points below its viewport's top whether or not it's still sticky,
/// bounded by the bottom of the growth row it's drawn in rather than its parent's new layout, so
/// the rows under it fold up to it before the scroll moves, on the same frame.
#[derive(Clone)]
pub(in crate::runtime) struct Hold {
    pub path: String,
    pub inset: f32,
}

/// One `Tx::scroll_into_view` or `Tx::scroll_center` request that follows its target while the
/// viewport's subtree moves.
#[derive(Clone)]
pub(in crate::runtime) struct Reveal {
    pub scope: Option<String>,
    pub key: String,
    pub viewport: String,
    pub center: bool,
    /// Whether this reveal lands at the bottom of a `stick_bottom` viewport.
    pub at_end: bool,
}

/// Node `i`'s drawn top below viewport `s`'s top, in unscaled viewport points.
pub(in crate::runtime) fn keep_y(drawn: &[[f32; 4]], drawn_scale: &[f32], i: usize, s: usize) -> f32 {
    let scale = drawn_scale.get(s).copied().unwrap_or(1.0).max(1e-4);
    (drawn[i][1] - drawn[s][1]) / scale
}

/// The nearest vertical scroll viewport around node `i` of `nodes`.
fn vertical_viewport(nodes: &[Node], i: usize) -> Option<usize> {
    std::iter::successors(nodes[i].parent, |&p| nodes[p].parent)
        .find(|&p| nodes[p].scroll.is_some() && nodes[p].scroll_axes.y())
}

/// How far a sticky box with edges `top`, `bottom`, `left` and `right` moves from where
/// scrolling put it, along the edge's axis. `frame` holds its viewport's top and bottom, then
/// its parent's content box top and bottom, then the same four for the horizontal axis.
pub(in crate::runtime) fn shift(edge: StickyEdge, inset: f32, [top, bottom, left, right]: [f32; 4], frame: [f32; 8]) -> f32 {
    let [band_top, band_bottom, box_top, box_bottom, band_left, _, _, box_right] = frame;
    let d = match edge {
        StickyEdge::Left => (band_left + inset - left).min(box_right - right).max(0.0),
        StickyEdge::Top => (band_top + inset - top).min(box_bottom - bottom).max(0.0),
        StickyEdge::Bottom => (band_bottom - inset - bottom).max(box_top - top).min(0.0),
    };
    if d.is_finite() { d } else { 0.0 }
}

/// Target rect of node `j` after its scroll viewports' offsets and any sticky shift on `j` or
/// its ancestors, for `El::anchored`.
pub(in crate::runtime) fn anchored_target(nodes: &[Node], scrolls: &HashMap<NodeId, ScrollState>, j: usize, viewport: (f32, f32)) -> [f32; 4] {
    anchored_target_inner(nodes, scrolls, j, viewport, 0)
}

pub(in crate::runtime) fn anchored_target_inner(nodes: &[Node], scrolls: &HashMap<NodeId, ScrollState>, j: usize, viewport: (f32, f32), depth: usize) -> [f32; 4] {
    let mut t = nodes[j].target;
    for [ox, oy] in scroll_offsets(scrolls, nodes, j) {
        t[0] -= ox;
        t[2] -= ox;
        t[1] -= oy;
        t[3] -= oy;
    }
    let [mut dx, mut dy] = target_shift(nodes, scrolls, j);
    if depth < crate::runtime::anchor::MAX_ANCHOR_CHAIN
        && let Some(anc) = std::iter::successors(Some(j), |&p| nodes[p].parent).find(|&p| nodes[p].above.is_some())
        && let Some(a) = &nodes[anc].above
        && let Some(k) = super::anchor_index(nodes, anc, a)
    {
        let at = anchored_target_inner(nodes, scrolls, k, viewport, depth + 1);
        let b = crate::runtime::anchor::boundary_index(nodes, anc);
        let bounds = crate::runtime::anchor::anchor_bounds_depth(nodes, scrolls, b, viewport, depth + 1);
        let (w, h) = (nodes[anc].target[2] - nodes[anc].target[0], nodes[anc].target[3] - nodes[anc].target[1]);
        let (ax, ay, _, _) = super::anchor_position(a, at, (w, h), bounds);
        dx += ax - nodes[anc].target[0];
        dy += ay - nodes[anc].target[1];
    }
    t[0] += dx;
    t[2] += dx;
    t[1] += dy;
    t[3] += dy;
    t
}

/// Total sticky shift `[dx, dy]` on node `j` and its ancestors from `nodes`'s laid-out rects and
/// `scrolls`.
fn target_shift(nodes: &[Node], scrolls: &HashMap<NodeId, ScrollState>, j: usize) -> [f32; 2] {
    let mut total = [0.0; 2];
    for a in std::iter::successors(Some(j), |&p| nodes[p].parent.filter(|_| nodes[p].above.is_none())) {
        let Some((edge, inset, tail)) = nodes[a].sticky.as_ref().and_then(|s| Some((s.edge?, s.inset, s.tail))) else { continue };
        let Some(p) = nodes[a].parent else { continue };
        if edge == StickyEdge::Left {
            let Some(s) = std::iter::successors(Some(p), |&u| nodes[u].parent)
                .find(|&u| nodes[u].scroll.is_some() && nodes[u].scroll_axes.x()) else { continue };
            let ox: f32 = std::iter::successors(Some(p), |&u| nodes[u].parent)
                .take_while(|&u| u != s)
                .chain(std::iter::once(s))
                .filter_map(|u| scrolls.get(&nodes[u].id).map(|st| st.x.shift() as f32))
                .sum();
            let st_x = nodes[s].target[0];
            let band_w = nodes[s].target[2] - st_x;
            let (box_left, box_right) = if p == s {
                (f32::NEG_INFINITY, f32::INFINITY)
            } else {
                (nodes[p].target[0] - st_x - ox + nodes[p].inset[0], nodes[p].target[2] - st_x - ox - nodes[p].inset[2])
            };
            let left = nodes[a].target[0] - st_x - ox;
            let right = nodes[a].target[2] - st_x - ox;
            let frame = [f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, 0.0, band_w, box_left, box_right];
            total[0] += shift(edge, inset, [0.0, 0.0, left, right], frame);
        } else {
            let Some(s) = std::iter::successors(Some(p), |&u| nodes[u].parent)
                .find(|&u| nodes[u].scroll.is_some() && nodes[u].scroll_axes.y()) else { continue };
            let oy: f32 = std::iter::successors(Some(p), |&u| nodes[u].parent)
                .take_while(|&u| u != s)
                .chain(std::iter::once(s))
                .filter_map(|u| scrolls.get(&nodes[u].id).map(|st| st.offset as f32))
                .sum();
            let st_y = nodes[s].target[1];
            let band_h = nodes[s].target[3] - st_y;
            let (box_top, box_bottom) = if p == s {
                (f32::NEG_INFINITY, f32::INFINITY)
            } else {
                (nodes[p].target[1] - st_y - oy + nodes[p].inset[1], nodes[p].target[3] - st_y - oy - nodes[p].inset[3])
            };
            let top = nodes[a].target[1] - st_y - oy;
            let bottom = nodes[a].target[3] - st_y - oy;
            let frame = [0.0, band_h, box_top, box_bottom, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY];
            total[1] += shift(edge, if tail { inset - (bottom - top) } else { inset }, [top, bottom, 0.0, 0.0], frame);
        }
    }
    total
}

impl requests::ScrollTables {
    /// Node `i`, or the header holding it, if it's drawn stuck to the top of viewport `s`.
    pub(super) fn stuck_header(&self, nodes: &[Node], has_sticky: bool, i: usize, s: usize) -> Option<usize> {
        if !has_sticky || self.stuck.len() != nodes.len() { return None; }
        std::iter::successors(Some(i), |&p| nodes[p].parent)
            .take_while(|&p| p != s)
            .find(|&h| self.stuck[h] > 0.0 && self.sticky_of(nodes, h).is_some_and(|(edge, ..)| edge == StickyEdge::Top))
    }

    /// Node `i`'s sticky edge, inset and whether the inset counts from its bottom: a `Hold`'s,
    /// else its own.
    #[inline]
    pub(in crate::runtime) fn sticky_of(&self, nodes: &[Node], i: usize) -> Option<(StickyEdge, f32, bool)> {
        if let Some(h) = self.hold_of(&nodes[i].path) { return Some((StickyEdge::Top, h.inset, false)); }
        nodes[i].sticky.as_ref().and_then(|s| Some((s.edge?, s.inset, s.tail)))
    }
}

impl RuntimeCore {
    /// Replay just one inherited fade edge without the nearest viewport's content-aware
    /// fade. Explicit clips inside it and fades outside it still apply. Recursion follows
    /// parents already painted this frame and needs no scratch allocation.
    pub(in crate::runtime) fn sticky_fade(&self, i: usize, viewport: usize, slot: usize, paint: bool) -> (f32, f32) {
        if i == viewport {
            let up = self.tree.nodes[i].parent.map_or(Inh::default(), |p| self.paint.inh_scratch[p]);
            return if paint { (up.paint_fade[slot], up.paint_fade_origin[slot]) }
                else { (up.fade[slot], up.fade_origin[slot]) };
        }
        let n = &self.tree.nodes[i];
        let (width, origin) = self.sticky_fade(n.parent.unwrap(), viewport, slot, paint);
        let mut widths = [0.0; 4];
        let mut origins = NO_CLIP;
        widths[slot] = width;
        origins[slot] = origin;
        let r = self.paint.drawn[i].map(|v| v * self.env.scale);
        let r = [r[0], r[1], r[0] + r[2], r[1] + r[3]];
        if let Some(clip) = n.vis.clip {
            inherit_fade(&mut widths, &mut origins, r, clip.map(|v| (v * self.paint.inh_scratch[i].scale * self.env.scale).clamp(-65504.0, 65504.0)));
        }
        if self.paint.row_mask[i] >= 0.0 {
            inherit_fade(&mut widths, &mut origins, r, [0.0, 0.0, 0.0, self.growth_feather(i)]);
        }
        // An isolation layer already consumed the parent's paint fade. Its descendants
        // must not apply that same fade a second time.
        if paint && self.paint.inh_scratch[i].paint_fade[slot] == 0.0 { return (0.0, NO_CLIP[slot]); }
        (widths[slot], origins[slot])
    }

    /// The nearest vertical scroll viewport around node `i`.
    pub(in crate::runtime) fn viewport_of(&self, i: usize) -> Option<usize> {
        vertical_viewport(&self.tree.nodes, i)
    }

    pub(super) fn keep_virtual_row(&self, i: usize, s: usize) -> Option<(String, String, f32)> {
        std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent)
            .take_while(|&p| p != s)
            .find_map(|r| {
                let c = self.tree.nodes[r].parent?;
                self.commit.virtuals.contains_key(&self.tree.nodes[c].path).then(|| (
                    self.tree.nodes[c].path.clone(),
                    self.tree.nodes[r].key.clone().unwrap_or_default(),
                    self.tree.nodes[i].target[1] - self.tree.nodes[r].target[1],
                ))
            })
    }

    pub(super) fn build_keep_candidate(&self, i: usize, s: usize) -> KeepCandidate {
        KeepCandidate {
            path: self.tree.nodes[i].path.clone(),
            was: keep_y(&self.paint.drawn, &self.paint.drawn_scale, i, s),
            virtual_row: self.keep_virtual_row(i, s),
        }
    }

    pub(super) fn build_keep(&self, i: usize, s: usize, fallbacks: Vec<KeepCandidate>) -> Keep {
        let hold = self.scroll.stuck_header(&self.tree.nodes, self.tree.has_sticky, i, s).map(|h| Hold {
            path: self.tree.nodes[h].path.clone(),
            inset: keep_y(&self.paint.drawn, &self.paint.drawn_scale, h, s),
        });
        Keep {
            path: self.tree.nodes[i].path.clone(),
            viewport: self.tree.nodes[s].path.clone(),
            was: keep_y(&self.paint.drawn, &self.paint.drawn_scale, i, s),
            virtual_row: self.keep_virtual_row(i, s),
            hold,
            fallbacks,
        }
    }

    /// Records where each `El::scroll_anchor` and `Tx::keep_on_screen` element showed on the
    /// last frame, before the commit moves it. Explicit `Tx::keep_on_screen` entries arrive
    /// after `El::scroll_anchor` captures and replace them on the same viewport.
    pub(in crate::runtime) fn capture_keeps(&mut self, tx: &mut Tx) {
        // The last frame's rects describe these nodes only until a commit replaces them.
        if !self.tree.built || self.paint.drawn.len() != self.tree.nodes.len() {
            tx.keep_on_screen.clear();
            return;
        }
        self.capture_anchor_keeps();
        if tx.keep_on_screen.is_empty() { return; }
        let keys = std::mem::take(&mut tx.keep_on_screen);
        for key in keys {
            let Some(i) = self.live_named(None, &key) else { continue };
            let Some(s) = self.viewport_of(i) else { continue };
            let keep = self.build_keep(i, s, Vec::new());
            self.scroll.requests.step(requests::Event::Arrive(requests::Request::Capture(keep)));
        }
    }

    /// Whether a `Tx::keep_on_screen` holds a stuck header (`Hold`).
    #[inline]
    pub(in crate::runtime) fn holding(&self) -> bool {
        self.scroll.holding()
    }

    /// Node `i`'s sticky edge, inset and whether the inset counts from its bottom: a `Hold`'s,
    /// else its own.
    #[inline]
    pub(in crate::runtime) fn sticky_of(&self, i: usize) -> Option<(StickyEdge, f32, bool)> {
        self.scroll.sticky_of(&self.tree.nodes, i)
    }

    /// `El::stuck_fade`: how much of node `i`'s children show, drawn with its top at `drawn`
    /// (a frame point, `dy` the offset its siblings draw at): whole until the next sticky-top
    /// sibling's top comes within `lead + span` of it, gone at `lead`.
    pub(in crate::runtime) fn stuck_fade(&self, i: usize, drawn: f32, dy: f32) -> f32 {
        let n = &self.tree.nodes[i];
        let Some([lead, span]) = n.sticky.as_ref().and_then(|s| s.fade) else { return 1.0 };
        let next = (i + 1..self.tree.nodes.len())
            .filter(|&j| self.tree.nodes[j].parent == n.parent)
            .find(|&j| self.tree.nodes[j].sticky.as_ref().is_some_and(|s| s.edge == Some(StickyEdge::Top)));
        let Some(p) = next.and_then(|j| self.scene.presented(self.tree.nodes[j].id).map(|p| (j, p))) else { return 1.0 };
        let (j, p) = p;
        let own = self.scene.get(self.tree.nodes[j].id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
        let top = p.rect.y - own.y + dy + self.paint.row_dy[j] - self.paint.row_dy[i];
        ((top - drawn - lead) / span.max(1e-3)).clamp(0.0, 1.0)
    }

    /// The bands and box node `i` sticks within along `edge` (see `shift`), from its ancestors'
    /// drawn places this frame, and the viewport it sticks to.
    pub(in crate::runtime) fn sticky_frame_of(&self, i: usize, edge: StickyEdge) -> ([f32; 8], Option<usize>) {
        let n = &self.tree.nodes[i];
        let mut frame = n.parent.map_or(UNBOUNDED, |pr| self.scroll.sticky_frame[pr]);
        let horizontal = edge == StickyEdge::Left;
        let mut ancestor = n.parent;
        let mut viewport = None;
        while let Some(a) = ancestor {
            let nd = &self.tree.nodes[a];
            if nd.scroll.is_some() && if horizontal { nd.scroll_axes.x() } else { nd.scroll_axes.y() } {
                viewport = Some(a);
                break;
            }
            // A section nested inside a growth row must stop at that row's presented
            // bottom, not its descendants' final layout height. Only sticky nodes
            // need this ancestor walk; ordinary painting keeps its existing fast path.
            if !horizontal && self.paint.row_h[a] >= 0.0 {
                frame[2] = frame[2].max(self.scroll.sticky_frame[a][2]);
                frame[3] = frame[3].min(self.scroll.sticky_frame[a][3]);
            }
            ancestor = nd.parent;
        }
        // A held header's parent may have shrunk in the commit while the growth row it's drawn
        // in folds: it stops at that row's drawn bottom, less what the row lays out below the
        // parent, so it rests where its parent ends once the row settles.
        if !horizontal && self.scroll.hold_of(&n.path).is_some() && let Some(p) = n.parent {
            let r = std::iter::successors(Some(p), |&a| self.tree.nodes[a].parent)
                .take_while(|&a| Some(a) != viewport)
                .find(|&a| self.paint.row_h[a] >= 0.0)
                .unwrap_or(p);
            let content_bottom = |a: usize| self.tree.nodes[a].target[3] - self.tree.nodes[a].inset[3];
            frame[2] = f32::NEG_INFINITY;
            frame[3] = self.scroll.sticky_frame[r][3] - (content_bottom(r) - content_bottom(p));
        }
        (frame, viewport)
    }

    /// Scrolls each kept element's viewport so the element's top lands where it showed before
    /// the commit, clamped to the content, re-pinning a `stick_bottom` end, and remounting any
    /// virtual rows the jump brings into view.
    pub(in crate::runtime) fn apply_keeps(&mut self, app: &mut dyn AppDriver) {
        if !self.scroll.requests.has_keeps() { return; }
        let mut keeps = self.scroll.requests.take_keeps();
        for pass in 0..4 {
            // Step with the keeps in place: they own their viewports (`ease_to_shrunk_end`).
            self.scroll.requests.restore_keeps(keeps);
            self.step_scroll(0.0);
            keeps = self.scroll.requests.take_keeps();
            let mut moved = false;
            for k in &mut keeps {
                let Some(s) = self.tree.nodes.iter().position(|n| n.path == k.viewport && n.scroll.is_some() && n.scroll_axes.y()) else { continue };
                let mounted = self.live_in_viewport(&k.path, s).or_else(|| {
                    let pos = k.fallbacks.iter().position(|fb| self.live_in_viewport(&fb.path, s).is_some())?;
                    let fb = k.fallbacks[pos].clone();
                    k.path = fb.path;
                    k.was = fb.was;
                    k.virtual_row = fb.virtual_row;
                    k.fallbacks.clear();
                    self.live_in_viewport(&k.path, s)
                });
                let laid = if mounted.is_some() {
                    if self.scroll_stack(s).and_then(|c| self.motion.stacks.get(&self.tree.nodes[c].id)).is_some_and(|st| st.growing)
                        && let Some(st) = self.scroll.scrolls.get_mut(&self.tree.nodes[s].id)
                    {
                        st.unpin();
                    }
                    // Mounted anchors follow drawn geometry after the motion step.
                    continue;
                } else if let Some(laid) = self.keep_virtual_laid(k) {
                    laid
                } else {
                    continue;
                };
                let stick = self.tree.nodes[s].scroll.is_some_and(|sc| sc.stick_bottom);
                let id = self.tree.nodes[s].id;
                let Some(st) = self.scroll.scrolls.get_mut(&id) else { continue };
                st.moved();
                let to = (laid - f64::from(k.was)).clamp(0.0, st.max);
                if pass > 0 && (to - st.offset).abs() < 0.5 { continue; }
                let before = st.offset;
                st.offset = to;
                if (st.offset - before).abs() > 1e-6 { st.note_source(crate::ScrollSource::Programmatic); }
                st.keep_land(stick);
                moved = true;
            }
            if !moved || !self.virtual_window_changed() { break; }
            self.commit_virtual_scroll(app, &Tx::default());
        }
        self.scroll.requests.restore_keeps(keeps);
    }

    fn live_in_viewport(&self, path: &str, s: usize) -> Option<usize> {
        self.tree.nodes.iter().enumerate()
            .position(|(i, n)| n.path == path && !self.subtree_exiting(i))
            .filter(|&i| self.viewport_of(i) == Some(s))
    }

    fn keep_virtual_laid(&self, k: &mut Keep) -> Option<f64> {
        let lookup = |vr: &Option<(String, String, f32)>| {
            let (v_path, row_key, in_row) = vr.as_ref()?;
            let v = self.commit.virtuals.get(v_path)?;
            let idx = v.index_of(row_key)?;
            Some(f64::from(v.top + v.pad_top + v.prefix(idx) + *in_row))
        };
        if let Some(laid) = lookup(&k.virtual_row) {
            return Some(laid);
        }
        let (pos, laid) = k.fallbacks.iter().enumerate().find_map(|(i, fb)| Some((i, lookup(&fb.virtual_row)?)))?;
        let fb = k.fallbacks[pos].clone();
        k.path = fb.path;
        k.was = fb.was;
        k.virtual_row = fb.virtual_row;
        k.fallbacks.clear();
        Some(laid)
    }

    /// Compensate this frame's drawn displacement, in viewport coordinates. Sticky
    /// clamping may absorb part of a correction; the caller permits a bounded repaint.
    pub(in crate::runtime) fn follow_keeps(&mut self) -> bool {
        if self.scroll.requests.is_empty() { return false; }
        self.end_takes_over();
        let mut moved = false;
        for k in self.scroll.requests.keeps() {
            let Some(i) = self.tree.nodes.iter().enumerate().position(|(i, n)| n.path == k.path && !self.subtree_exiting(i)) else { continue };
            let Some(s) = self.viewport_of(i).filter(|&s| self.tree.nodes[s].path == k.viewport) else { continue };
            let scale = self.paint.drawn_scale[s];
            if scale <= 1e-4 { continue; }
            let delta = (self.paint.drawn[i][1] - self.paint.drawn[s][1]) / scale - k.was;
            if delta.abs() < 1.0 / 64.0 { continue; }
            let stick = self.tree.nodes[s].scroll.is_some_and(|s| s.stick_bottom);
            let Some(st) = self.scroll.scrolls.get_mut(&self.tree.nodes[s].id) else { continue };
            let to = (st.offset + f64::from(delta)).clamp(0.0, st.max);
            if (to - st.offset).abs() < 1e-5 { continue; }
            st.offset = to;
            st.keep_land(stick && delta >= 0.0);
            st.moved();
            st.note_source(crate::ScrollSource::Programmatic);
            moved = true;
        }
        for r in self.scroll.requests.reveals() {
            let target = (0..self.tree.nodes.len()).find(|&i| self.reveal_names(r, i))
                .map(|i| (i, self.paint.drawn[i][1], self.paint.drawn[i][3]))
                .or_else(|| {
                    let (i, [_, y0, _, y1]) = self.highlight_target_in(&self.tree.nodes, r.scope.as_deref(), &r.key)?;
                    if self.subtree_exiting(i) { return None; }
                    let scale = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
                    let dy = (y0 - self.tree.nodes[i].target[1]) * scale;
                    Some((i, self.paint.drawn[i][1] + dy, (y1 - y0).max(0.0) * scale))
                });
            let Some((i, drawn_top, drawn_h)) = target else { continue };
            let Some(s) = self.viewport_of(i).filter(|&s| self.tree.nodes[s].path == r.viewport) else { continue };
            let scale = self.paint.drawn_scale[s];
            if scale <= 1e-4 { continue; }
            let Some(cover) = self.stuck_cover(&r.viewport, r.scope.as_deref(), &r.key) else { continue };
            let cover = cover / f64::from(scale);
            let rel_top = f64::from((drawn_top - self.paint.drawn[s][1]) / scale) - cover;
            let height = f64::from(drawn_h / scale) + cover;
            let viewport = f64::from(self.paint.drawn[s][3] / scale);
            let margin = f64::from(self.tree.nodes[s].scroll_edges.margin);
            let stick = self.tree.nodes[s].scroll.is_some_and(|s| s.stick_bottom);
            let growing = self.scroll_stack(s).and_then(|c| self.motion.stacks.get(&self.tree.nodes[c].id)).is_some_and(|st| st.growing);
            let Some(st) = self.scroll.scrolls.get_mut(&self.tree.nodes[s].id) else { continue };
            let top = st.offset + rel_top;
            let to = reveal::place(r.center, st.offset, top, height, viewport, margin).clamp(0.0, st.max);
            // A `Tx::scroll_center` glide follows its target once the target moves.
            if let ScrollMode::Glide { v, to: aim, .. } = st.mode {
                if aim.offset().is_none_or(|g| (to - g).abs() >= 1.0 / 64.0) {
                    let aim = if let GlideTo::ShrunkEnd(_) = aim { GlideTo::ShrunkEnd(to) } else { GlideTo::Offset(to) };
                    st.mode = st.mode.to_glide(scroll_offset::retarget_v(st.glide_spring(), st.offset, v, to), aim);
                }
                continue;
            }
            if (to - st.offset).abs() < 1.0 / 64.0 {
                if growing { st.unpin(); }
                continue;
            }
            st.offset = to;
            st.keep_land(stick && !growing);
            st.moved();
            st.note_source(crate::ScrollSource::Programmatic);
            moved = true;
        }
        // Once the content end prevents holding the anchor, let stick-bottom own the
        // viewport again, including growth committed while the collapse is still settling.
        self.scroll.requests.step(requests::EndClamps { shown: |k: &Keep| {
            let Some(i) = self.tree.nodes.iter().position(|n| n.path == k.path) else { return requests::Shown::Unmounted };
            let Some(s) = vertical_viewport(&self.tree.nodes, i) else { return requests::Shown::Loose };
            if !self.scroll.scrolls.get(&self.tree.nodes[s].id).is_some_and(|st| st.pinned()) { return requests::Shown::Free; }
            requests::Shown::Pinned(keep_y(&self.paint.drawn, &self.paint.drawn_scale, i, s))
        } });
        moved
    }

    /// A user or explicit scroll ends the holds and reveals that would pull the view back.
    #[inline]
    pub(in crate::runtime) fn drop_scroll_holds(&mut self) {
        self.scroll.drop_holds();
    }

    /// Colour, hover or decorative motion must not extend the anchor's lifetime.
    pub(in crate::runtime) fn finish_keeps(&mut self) {
        if self.scroll.requests.is_empty() { return; }
        let mut requests = std::mem::take(&mut self.scroll.requests);
        let animating = |viewport: &str| self.tree.nodes.iter().any(|n| {
            let within = n.path == viewport || (n.path.starts_with(viewport)
                && n.path.as_bytes().get(viewport.len()) == Some(&b'/'));
            within && (self.scene.store().is_animating(n.id, prop::FRAME)
                || self.scene.store().is_animating(n.id, prop::TRAVEL)
                || self.motion.grows.get(&n.id).is_some_and(|g| g.moving())
                || self.scroll.scrolls.get(&n.id).is_some_and(|st| st.slack.moving()))
        });
        let gliding = |viewport: &str| self.tree.path_ids.get(viewport)
            .and_then(|id| self.scroll.scrolls.get(id))
            .is_some_and(|st| st.glide().is_some());
        // A glide keeps its reveal until it lands; otherwise a reveal lasts while its viewport's
        // subtree moves with its target in it.
        requests.step(requests::Finish { animating, gliding, mounted: |r: &Reveal| self.reveal_mounted(r) });
        self.scroll.requests = requests;
    }

    /// Whether node `i` is reveal `r`'s target element.
    fn reveal_names(&self, r: &Reveal, i: usize) -> bool {
        let n = &self.tree.nodes[i];
        n.named(&r.key) && !self.subtree_exiting(i)
            && n.parent.is_none_or(|p| super::in_scope(&self.tree.nodes[p].path, r.scope.as_deref()))
    }

    /// Whether reveal `r`'s target element or highlight is mounted in its viewport.
    fn reveal_mounted(&self, r: &Reveal) -> bool {
        let in_viewport = |i: usize| self.viewport_of(i).is_some_and(|s| self.tree.nodes[s].path == r.viewport);
        (0..self.tree.nodes.len()).any(|i| self.reveal_names(r, i) && in_viewport(i))
            || self.highlight_target_in(&self.tree.nodes, r.scope.as_deref(), &r.key)
                .is_some_and(|(i, _)| !self.subtree_exiting(i) && in_viewport(i))
    }

    /// What a reveal of row `key` (within `scope`, when given) in viewport `path` must leave to
    /// stuck headers: `None` when the row is, or is inside, a header stuck in view (it shows
    /// already), else the height stuck headers cover at the viewport's top, which the reveal keeps
    /// the row clear of.
    pub(in crate::runtime) fn stuck_cover(&self, path: &str, scope: Option<&str>, key: &str) -> Option<f64> {
        if !self.tree.has_sticky || self.scroll.stuck.len() != self.tree.nodes.len() || self.paint.drawn.len() != self.tree.nodes.len() { return Some(0.0) }
        let Some(s) = self.tree.nodes.iter().position(|n| n.path == path) else { return Some(0.0) };
        let view = self.paint.drawn[s];
        let row = self.live_named(scope, key).filter(|&r| self.viewport_of(r) == Some(s));
        let holder = row.and_then(|r| std::iter::successors(Some(r), |&p| self.tree.nodes[p].parent).take_while(|&p| p != s).find(|&p| self.scroll.stuck[p] != 0.0 && self.tree.nodes[p].sticky.as_ref().is_none_or(|s| s.edge != Some(StickyEdge::Left))));
        if let (Some(r), Some(_)) = (row, holder) {
            let d = self.paint.drawn[r];
            if d[1] >= view[1] - 0.5 && d[1] + d[3] <= view[1] + view[3] + 0.5 { return None; }
        }
        let cover = (0..self.tree.nodes.len())
            .filter(|&j| self.scroll.stuck[j] > 0.0 && self.tree.nodes[j].sticky.as_ref().is_none_or(|s| s.edge != Some(StickyEdge::Left)) && Some(j) != holder && self.viewport_of(j) == Some(s))
            .map(|j| self.paint.drawn[j][1] + self.paint.drawn[j][3] - view[1])
            .fold(0.0f32, f32::max);
        Some(f64::from(cover))
    }
}

#[cfg(test)]
#[path = "sticky_tests.rs"]
mod tests;
