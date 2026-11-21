//! Hit-test geometry, turned-layer coordinate mapping, target resolution and per-frame hit
//! registration.
use super::*;

/// Per-frame hit-test lists built during paint in paint order and queried by pointer, wheel and
/// overlay hit testing. Kept together under `#[repr(C)]` because the per-node paint walk and
/// `keep_swap` read and swap all five lists together.
#[repr(C)]
pub(in crate::runtime) struct HitState {
    pub(in crate::runtime) hits: Vec<(NodeId, [f32; 4])>,
    /// What a wheel chains up from, in paint order: what takes input, every scroll view and every
    /// painted surface (`Runtime::chain_start`).
    pub(in crate::runtime) wheel_hits: Vec<(NodeId, [f32; 4])>,
    pub(in crate::runtime) overlay_hits: Vec<[f32; 4]>,
    /// This frame's help elements: (node, visible rect in points, `hits.len()` before it, the
    /// link of its text when the help is that link's).
    pub(in crate::runtime) help_hits: Vec<(NodeId, [f32; 4], usize, Option<usize>)>,
    /// The first help element this frame that `El::help_open` holds open, noted as its hit is
    /// pushed, so the tooltip finds it without walking `help_hits`.
    pub(in crate::runtime) help_held: Option<NodeId>,
    /// This frame's drop targets: (node, visible rect in points, `hits.len()` before it). Kept
    /// apart from `hits`, so a drop zone doesn't cover the context menus and help under it.
    pub(in crate::runtime) drop_hits: Vec<(NodeId, [f32; 4], usize)>,
}

impl HitState {
    // Single-caller constructor for `RuntimeCore::new`; inlined so it stays in its caller's
    // codegen unit instead of shifting partitioning and dropping inlining in the paint walk.
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            hits: Vec::new(),
            wheel_hits: Vec::new(),
            help_hits: Vec::new(),
            help_held: None,
            drop_hits: Vec::new(),
            overlay_hits: Vec::new(),
        }
    }

    #[inline]
    pub(in crate::runtime) fn overlay_hits(&self) -> &[[f32; 4]] { &self.overlay_hits }

    #[inline]
    pub(in crate::runtime) fn hits(&self) -> &[(NodeId, [f32; 4])] { &self.hits }

    #[inline]
    pub(in crate::runtime) fn hits_len(&self) -> usize { self.hits.len() }

    #[inline]
    pub(in crate::runtime) fn hits_since(&self, seq: usize) -> &[(NodeId, [f32; 4])] {
        &self.hits[seq.min(self.hits.len())..]
    }

    #[inline]
    pub(in crate::runtime) fn help_hits(&self) -> &[(NodeId, [f32; 4], usize, Option<usize>)] { &self.help_hits }

    #[inline]
    pub(in crate::runtime) fn has_help_hits(&self) -> bool { !self.help_hits.is_empty() }

    #[inline]
    pub(in crate::runtime) fn clear_help_hits(&mut self) { self.help_hits.clear(); self.help_held = None; }

    #[inline]
    pub(in crate::runtime) fn clear_drop_hits(&mut self) { self.drop_hits.clear(); }

    #[inline]
    pub(in crate::runtime) fn push_hit(&mut self, id: NodeId, rect: [f32; 4]) {
        self.hits.push((id, rect));
    }

    #[inline]
    pub(in crate::runtime) fn push_wheel_hit(&mut self, id: NodeId, rect: [f32; 4]) {
        if rect[2] > rect[0] && rect[3] > rect[1] {
            self.wheel_hits.push((id, rect));
        }
    }

    #[inline]
    pub(in crate::runtime) fn push_overlay_hit(&mut self, rect: [f32; 4]) {
        self.overlay_hits.push(rect);
    }

    /// During `build`: node `id` (or its `link`) has help and draws visible over `rect`
    /// (points, x0 y0 x1 y1), after the first `seq` hits.
    #[inline]
    pub(in crate::runtime) fn push_help_hit(&mut self, id: NodeId, rect: [f32; 4], seq: usize, link: Option<usize>) {
        if rect[2] > rect[0] && rect[3] > rect[1] {
            self.help_hits.push((id, rect, seq, link));
        }
    }

    /// During `build`: drop target `id` draws visible over `rect`, after the first `seq` hits.
    #[inline]
    pub(in crate::runtime) fn push_drop_hit(&mut self, id: NodeId, rect: [f32; 4], seq: usize) {
        if rect[2] > rect[0] && rect[3] > rect[1] {
            self.drop_hits.push((id, rect, seq));
        }
    }

    /// Topmost hit whose rect passes `contains`.
    #[inline]
    pub(in crate::runtime) fn top_hit(&self, mut contains: impl FnMut(NodeId, [f32; 4]) -> bool) -> Option<NodeId> {
        self.hits.iter().rev().find(|&&(id, r)| contains(id, r)).map(|&(id, _)| id)
    }

    /// Index and node of the topmost hit whose rect passes `contains`.
    #[inline]
    pub(in crate::runtime) fn top_hit_index(&self, mut contains: impl FnMut(NodeId, [f32; 4]) -> bool) -> Option<(usize, NodeId)> {
        self.hits.iter().enumerate().rev().find(|&(_, &(id, r))| contains(id, r)).map(|(k, &(id, _))| (k, id))
    }

    /// Walks hits top-to-bottom and returns the first match from `f`.
    #[inline]
    pub(in crate::runtime) fn find_hit<T>(&self, mut f: impl FnMut(NodeId, [f32; 4]) -> Option<T>) -> Option<T> {
        self.hits.iter().rev().find_map(|&(id, r)| f(id, r))
    }

    /// Walks wheel hits top-to-bottom and returns the first match from `f`.
    #[inline]
    pub(in crate::runtime) fn find_wheel_hit<T>(&self, mut f: impl FnMut(NodeId, [f32; 4]) -> Option<T>) -> Option<T> {
        self.wheel_hits.iter().rev().find_map(|&(id, r)| f(id, r))
    }

    /// Topmost help entry whose rect passes `contains`.
    #[inline]
    pub(in crate::runtime) fn top_help(&self, mut contains: impl FnMut(NodeId, [f32; 4]) -> bool) -> Option<(NodeId, usize, Option<usize>)> {
        self.help_hits.iter().rev().find(|&&(id, r, _, _)| contains(id, r)).map(|&(id, _, seq, link)| (id, seq, link))
    }

    /// Topmost resolved hit or drop zone at the point: whichever painted later wins.
    #[inline]
    pub(in crate::runtime) fn top_drop_or_hit(
        &self,
        mut contains: impl FnMut(NodeId, [f32; 4]) -> bool,
        mut resolve: impl FnMut(NodeId) -> Option<usize>,
    ) -> Option<usize> {
        let hit = self.hits.iter().enumerate().rev()
            .find_map(|(k, &(id, r))| contains(id, r).then(|| resolve(id)).flatten().map(|i| (k, i)));
        let zone = self.drop_hits.iter().rev()
            .find_map(|&(id, r, seq)| contains(id, r).then(|| resolve(id)).flatten().map(|i| (seq, i)));
        match (hit, zone) {
            (Some((k, h)), Some((seq, z))) => Some(if k >= seq { h } else { z }),
            (h, z) => h.or(z).map(|(_, i)| i),
        }
    }

    /// Whether `id` has any hit rects this frame, and if so whether any passes `contains`.
    #[inline]
    pub(in crate::runtime) fn has_node_hit(&self, id: NodeId, mut contains: impl FnMut(NodeId, [f32; 4]) -> bool) -> Option<bool> {
        let mut hits = self.hits.iter().filter(|(hit, _)| *hit == id).peekable();
        hits.peek()?;
        Some(hits.any(|&(id, r)| contains(id, r)))
    }
}

impl RuntimeCore {
    /// Node `i`'s edges as drawn this frame (points, x0 y0 x1 y1), or its layout box before
    /// it draws.
    pub(in crate::runtime) fn drawn_edges(&self, i: usize) -> [f32; 4] {
        self.paint.drawn.get(i).map_or(self.tree.nodes[i].target, |d| [d[0], d[1], d[0] + d[2], d[1] + d[3]])
    }

    /// Window point (x, y) as node `i`'s handlers see it.
    pub(crate) fn pointer_event(&self, i: usize, x: f32, y: f32) -> PointerEvent {
        let [lx, ly] = self.to_local(i, x, y);
        PointerEvent { x: lx, y: ly, window_x: x, window_y: y }
    }

    /// Window point (x, y) in node `i`'s own layout points: from its top-left as drawn, divided
    /// by the scale it draws at, so it follows scrolling, `live_offset`, `live_scale`,
    /// `El::scale` and `El::rotate`. From its layout box before it draws.
    pub(crate) fn to_local(&self, i: usize, x: f32, y: f32) -> [f32; 2] {
        let Some(d) = self.paint.drawn.get(i) else {
            let t = self.tree.nodes[i].target;
            return [x - t[0], y - t[1]];
        };
        let k = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0).max(1e-4);
        if self.paint.turn_scratch.is_empty() {
            return [(x - d[0]) / k, (y - d[1]) / k];
        }
        let (b, ux, uy) = self.unturned_box_and_point(i, x, y);
        [(ux - b[0]) / k, (uy - b[1]) / k]
    }

    /// The turn node `i` draws inside (`Inh::turn` is 1-based, 0 for none).
    pub(in crate::runtime) fn turn_of(&self, i: usize) -> Option<Turn> {
        self.paint.inh_scratch.get(i).and_then(|h| h.turn.checked_sub(1)).and_then(|k| self.paint.turn_scratch.get(k as usize)).copied()
    }

    /// Node `i`'s unturned border box (points, x0 y0 x1 y1) and window point (x, y) unturned into
    /// that layer's points.
    pub(in crate::runtime) fn unturned_box_and_point(&self, i: usize, x: f32, y: f32) -> ([f32; 4], f32, f32) {
        if let Some(t) = self.turn_of(i)
            && let Some(&(b, _)) = self.paint.unturned_drawn.get(i)
        {
            let s = self.env.scale;
            let [ux, uy] = t.unturn([x * s, y * s]).map(|v| v / s);
            return (b, ux, uy);
        }
        (self.drawn_box(i), x, y)
    }

    #[inline]
    pub(in crate::runtime) fn hit_contains(&self, id: NodeId, r: [f32; 4], x: f32, y: f32) -> bool {
        if self.paint.turn_scratch.is_empty() {
            return contains(r, x, y);
        }
        self.hit_contains_turned(id, r, x, y)
    }

    #[cold]
    #[inline(never)]
    fn hit_contains_turned(&self, id: NodeId, r: [f32; 4], x: f32, y: f32) -> bool {
        let Some(&i) = self.tree.node_index.get(&id) else { return contains(r, x, y) };
        let Some(t) = self.turn_of(i) else {
            return contains(r, x, y);
        };
        let s = self.env.scale;
        if !contains(t.outer_clip.map(|v| v / s), x, y) {
            return false;
        }
        let [ux, uy] = t.unturn([x * s, y * s]).map(|v| v / s);
        contains(r, ux, uy)
    }

    pub(in crate::runtime) fn hit_screen_rect(&self, id: NodeId, r: [f32; 4]) -> [f32; 4] {
        if self.paint.turn_scratch.is_empty() {
            return r;
        }
        let Some(&i) = self.tree.node_index.get(&id) else { return r };
        let Some(t) = self.turn_of(i) else {
            return r;
        };
        let s = self.env.scale;
        let b = t.bbox(r.map(|v| v * s)).map(|v| v / s);
        let c = t.outer_clip.map(|v| v / s);
        intersect(b, c)
    }

    pub(in crate::runtime) fn node_contains(&self, i: usize, x: f32, y: f32) -> bool {
        if !self.paint.turn_scratch.is_empty()
            && let Some(turn_k) = self.paint.inh_scratch.get(i).map(|h| h.turn).filter(|&k| k != 0)
            && let Some(&t) = self.paint.turn_scratch.get(turn_k as usize - 1)
            && let Some(&(b, _)) = self.paint.unturned_drawn.get(i)
        {
            let s = self.env.scale;
            if !contains(t.outer_clip.map(|v| v / s), x, y) {
                return false;
            }
            let lc = self.tree.nodes[i].parent
                .filter(|&p| self.paint.inh_scratch.get(p).is_some_and(|h| h.turn == turn_k))
                .and_then(|p| self.paint.unturned_drawn.get(p))
                .map_or(NO_CLIP, |u| u.1)
                .map(|v| v / s);
            let shown = intersect(b, lc);
            let [ux, uy] = t.unturn([x * s, y * s]).map(|v| v / s);
            return contains(shown, ux, uy);
        }
        let r = self.drawn_edges(i);
        let c = self.tree.nodes[i].parent.and_then(|p| self.paint.inh_scratch.get(p)).map_or(NO_CLIP, |h| h.clip).map(|v| v / self.env.scale);
        let shown = intersect(r, c);
        contains(shown, x, y)
    }

    pub(in crate::runtime) fn screen_clip_of_parent(&self, parent: Option<usize>) -> [f32; 4] {
        screen_clip_of(parent, &self.paint.inh_scratch, &self.paint.turn_scratch)
    }

    /// Unmounted in this commit (an instant removal), though still listed until the next.
    pub(in crate::runtime) fn subtree_unmounted(&self, mut i: usize) -> bool {
        loop {
            if !self.scene.contains(self.tree.nodes[i].id) { return true; }
            let Some(parent) = self.tree.nodes[i].parent else { return false };
            i = parent;
        }
    }

    pub(crate) fn is_exiting(&self, id: NodeId) -> bool {
        matches!(self.scene.phase(id), Some(PresencePhase::Exiting { .. }))
    }

    /// The single reachable editor in a textbox. Only resolve descendants on presses;
    /// hover hit testing and cursor updates use the container's role directly.
    pub(in crate::runtime) fn textbox_input(&self, i: usize) -> Option<usize> {
        if self.tree.nodes[i].input.is_some() { return self.can_focus(i).then_some(i); }
        if self.tree.nodes[i].role != "textbox" { return None; }
        let mut inputs = (0..self.tree.nodes.len()).filter(|&j| {
            self.tree.nodes[j].input.is_some()
                && self.can_focus(j)
                && std::iter::successors(self.tree.nodes[j].parent, |&p| self.tree.nodes[p].parent).any(|p| p == i)
        });
        let first = inputs.next()?;
        inputs.next().is_none().then_some(first)
    }

    /// The topmost element under (x, y) that takes primary input, selectable text included.
    pub(in crate::runtime) fn top_target(&self, x: f32, y: f32) -> Option<usize> {
        self.hit.find_hit(|id, r| {
            if !self.hit_contains(id, r, x, y) { return None; }
            let i = *self.tree.node_index.get(&id)?;
            let n = &self.tree.nodes[i];
            // A secondary-only child must not obscure primary input or background blur.
            (!self.subtree_exiting(i) && self.allowed(i) && (n.has_primary_handler() || n.input.is_some() || n.role == "textbox"
                || (n.role == "button" && n.vis_flags.hit()) || n.window_drag != crate::el::WindowDrag::None || n.focus.enabled || n.handlers.modal.is_some()
                || self.is_selectable(i))).then_some(i)
        })
    }

    /// The element a press, hover or drag at (x, y) goes to. Selectable text passes them to the
    /// nearest ancestor that handles them (a clickable or draggable row), unless the text has
    /// its own handlers or the point is on one of its links; selecting goes on beside that
    /// (`selectable_at`).
    pub(in crate::runtime) fn primary_target(&self, x: f32, y: f32) -> Option<usize> {
        let i = self.top_target(x, y)?;
        let n = &self.tree.nodes[i];
        if !self.is_selectable(i) || n.has_primary_handler() || self.link_at(i, x, y).is_some() { return Some(i); }
        let handles = |n: &Node| n.has_primary_handler() || n.focus.enabled;
        let mut p = n.parent;
        while let Some(a) = p {
            if self.subtree_exiting(a) || !self.allowed(a) || self.tree.nodes[a].handlers.modal.is_some() { break; }
            if handles(&self.tree.nodes[a]) { return Some(a); }
            p = self.tree.nodes[a].parent;
        }
        Some(i)
    }

    /// A focusable scroll region, which a press on plain content inside it focuses, as a web
    /// page focuses a `tabIndex` scroller.
    pub(in crate::runtime) fn focuses_on_click(&self, i: usize) -> bool {
        self.tree.nodes[i].scroll.is_some() && self.tree.nodes[i].focus.enabled
    }

    /// The focusable scroll region a press at (x, y) lands in beneath `layer`
    /// (`El::press_focuses_below`): the topmost element there outside the layer, or the nearest
    /// region holding it.
    pub(in crate::runtime) fn scroll_region_below(&self, layer: usize, x: f32, y: f32) -> Option<usize> {
        let in_layer = |i: usize| std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent).any(|p| p == layer);
        let under = self.hit.find_hit(|id, r| {
            if !self.hit_contains(id, r, x, y) { return None; }
            let i = *self.tree.node_index.get(&id)?;
            (!in_layer(i) && !self.subtree_exiting(i) && self.allowed(i)).then_some(i)
        })?;
        std::iter::successors(Some(under), |&p| self.tree.nodes[p].parent).find(|&p| self.focuses_on_click(p) && self.can_focus(p))
    }

    /// The selectable text under (x, y), if it's the topmost thing there that takes input.
    pub(in crate::runtime) fn selectable_at(&self, x: f32, y: f32) -> Option<usize> {
        self.top_target(x, y).filter(|&i| self.is_selectable(i))
    }

    /// The node at `path`, unless it is on its way out.
    pub(crate) fn live_index(&self, path: &str) -> Option<usize> {
        self.tree.nodes.iter().enumerate().position(|(i, n)| n.path == path && !self.subtree_exiting(i))
    }

    pub(in crate::runtime) fn in_viewport(&self, x: f32, y: f32) -> bool {
        contains([0.0, 0.0, self.viewport.0, self.viewport.1], x, y)
    }

    pub(in crate::runtime) fn register_node_hits(
        &mut self,
        i: usize,
        pinh_clip: [f32; 4],
        (effective_opacity, visible): (f32, bool),
        rect_pt: [f32; 4],
        (press_k, press_anchor): (f32, [f32; 2]),
    ) {
        let s = self.env.scale;
        let n = &self.tree.nodes[i];
        let hits_before = self.hit.hits_len();
        if (n.vis_flags.hit() || n.handlers.get().is_some_and(crate::el::Handlers::takes_hit) || n.input.is_some() || n.role == "textbox" || n.window_drag != crate::el::WindowDrag::None || n.focus.get().is_some_and(|f| f.enabled)
            || n.text.as_ref().is_some_and(|t| t.selectable))
            && !self.subtree_exiting(i)
            && self.allowed(i) && effective_opacity > 0.001 && visible
        {
            let c = if n.parent.is_some() { pinh_clip } else { self.paint.inh_scratch[0].clip };
            let rect_pt = if (press_k - 1.0).abs() > 1e-5 {
                let [ax, ay] = press_anchor;
                [ax + (rect_pt[0] - ax) / press_k, ay + (rect_pt[1] - ay) / press_k,
                    ax + (rect_pt[2] - ax) / press_k, ay + (rect_pt[3] - ay) / press_k]
            } else { rect_pt };
            let cr = [
                rect_pt[0].max(c[0] / s),
                rect_pt[1].max(c[1] / s),
                rect_pt[2].min(c[2] / s),
                rect_pt[3].min(c[3] / s),
            ];
            let rect = if n.parent.is_some() { cr } else { rect_pt };
            if !self.push_link_hits(i, rect_pt, rect) { self.hit.push_hit(self.tree.nodes[i].id, rect); }
            let n = &self.tree.nodes[i];
            if n.vis_flags.hit() && effective_opacity > 0.0 && rect[2] > rect[0] && rect[3] > rect[1] {
                let screen_rect = self.hit_screen_rect(n.id, rect);
                self.hit.push_overlay_hit(screen_rect);
            }
        }
        // What a wheel chains up from, in the same paint order: what takes input, every scroll
        // view, and the roots of what is drawn over others (an anchored overlay, a `raise()`d
        // panel, a sticky element), whatever their fill, so a wheel over any of them starts there,
        // not on what lies beneath. Other boxes without a handler let the wheel through, as an
        // element with `pointer-events: none` does.
        let n = &self.tree.nodes[i];
        if (self.hit.hits_len() > hits_before || n.scroll.is_some() || n.above.is_some() || n.vis_flags.raise() || n.sticky.is_some())
            && !self.subtree_exiting(i) && self.allowed(i) && effective_opacity > 0.001 && visible
        {
            let c = if n.parent.is_some() { pinh_clip } else { self.paint.inh_scratch[0].clip };
            let r = if n.parent.is_some() {
                [rect_pt[0].max(c[0] / s), rect_pt[1].max(c[1] / s), rect_pt[2].min(c[2] / s), rect_pt[3].min(c[3] / s)]
            } else { rect_pt };
            self.hit.push_wheel_hit(n.id, r);
        }
        if self.tree.nodes[i].cold.get().is_some_and(|c| c.help.is_some() && (!c.help_cut || self.tree.nodes[i].text_cut(s))) && !self.subtree_exiting(i) && self.allowed(i) && effective_opacity > 0.001 && visible {
            let c = if self.tree.nodes[i].parent.is_some() { pinh_clip } else { NO_CLIP };
            let r = [rect_pt[0].max(c[0] / s), rect_pt[1].max(c[1] / s), rect_pt[2].min(c[2] / s), rect_pt[3].min(c[3] / s)];
            self.hit.push_help_hit(self.tree.nodes[i].id, r, hits_before, None);
            // The cold block the test above read: a held-open tooltip costs nothing to find later.
            if self.hit.help_held.is_none() && r[2] > r[0] && r[3] > r[1] && self.tree.nodes[i].cold.get().is_some_and(|c| c.help_open) { self.hit.help_held = Some(self.tree.nodes[i].id); }
        }
        if self.tree.nodes[i].text.as_ref().is_some_and(|t| t.cold.links.iter().any(|l| l.help.is_some())) && !self.subtree_exiting(i) && self.allowed(i) && effective_opacity > 0.001 && visible {
            let c = if self.tree.nodes[i].parent.is_some() { pinh_clip } else { NO_CLIP };
            self.push_link_help_hits(i, rect_pt, [c[0] / s, c[1] / s, c[2] / s, c[3] / s], hits_before);
        }
        if self.tree.nodes[i].handlers.get().is_some_and(|h| h.drop.is_some()) && !self.subtree_exiting(i) && self.allowed(i) && effective_opacity > 0.001 && visible {
            let c = if self.tree.nodes[i].parent.is_some() { pinh_clip } else { NO_CLIP };
            let r = [rect_pt[0].max(c[0] / s), rect_pt[1].max(c[1] / s), rect_pt[2].min(c[2] / s), rect_pt[3].min(c[3] / s)];
            self.hit.push_drop_hit(self.tree.nodes[i].id, r, hits_before);
        }
    }
}
