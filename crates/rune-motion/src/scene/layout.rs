//! Layout frame animation and dual-number kinematics (exact root-space edges and velocities).

use super::*;

/// A small dual number: value and time derivative, for exact velocity of composed geometry.
#[derive(Clone, Copy, Default)]
pub(super) struct D {
    pub(super) x: f32,
    pub(super) v: f32,
}
impl D {
    pub(super) fn c(x: f32) -> D {
        D { x, v: 0.0 }
    }
    pub(super) fn add(self, o: D) -> D {
        D { x: self.x + o.x, v: self.v + o.v }
    }
    pub(super) fn sub(self, o: D) -> D {
        D { x: self.x - o.x, v: self.v - o.v }
    }
    pub(super) fn mul(self, o: D) -> D {
        D { x: self.x * o.x, v: self.v * o.x + self.x * o.v }
    }
}

impl Scene {
    /// The node's layout frame (parent space) changed. Each moving edge animates; a stationary
    /// edge gets no motion at all. With `Travel::Screen` the node takes one straight root-space
    /// path instead, whatever its ancestors do.
    pub fn set_frame(&mut self, id: NodeId, frame: Rect, anim: impl Into<LayoutAnim>) {
        self.set_frame_drawn(id, frame, anim.into(), [Vec2::ZERO; 2]);
    }

    /// [`set_frame`](Self::set_frame) for a node drawn `drawn[0]` away from its scene rect,
    /// moving `drawn[1]` faster: a screen path starts there, at that speed.
    pub(super) fn set_frame_drawn(&mut self, id: NodeId, frame: Rect, anim: LayoutAnim, drawn: [Vec2; 2]) {
        let Some(i) = self.slot(id) else { return };
        let old = self.nodes[i].frame;
        if old == frame && self.nodes[i].path.is_none() && self.nodes[i].tracks[prop::FRAME.id.0 as usize].is_none() {
            return;
        }
        if frame.w != old.w {
            self.rebase_trailing(i, frame.w - old.w);
        }
        let had_path = self.nodes[i].path.is_some();
        // An explicit landing also cancels motion toward an unchanged target. Comparing only
        // target frames here used to leave a spring running after Runtime::settle_all().
        if anim == LayoutAnim::INSTANT && !had_path && self.in_flight_to(id, prop::FRAME, frame) {
            return;
        }
        if anim == LayoutAnim::INSTANT && (had_path || self.store.is_animating(id, prop::FRAME)) {
            self.end_path(i);
            self.nodes[i].frame = frame;
            self.track_set(i, prop::FRAME, frame);
            self.nodes[i].dirty = true;
            return;
        }
        if old == frame && !had_path {
            return;
        }
        let ms = match anim.x.curve {
            Curve::Spring(s) => s.duration,
            c => c.logical_duration(),
        };
        self.intent(id, Intent::POSITION | Intent::SIZE, ms);
        if anim.travel == Travel::Screen && !self.store.reduced_motion() {
            // A path restarts from where it was drawn: under the origin the last frame drew it
            // against, which an ancestor laid out elsewhere in this commit has already moved.
            let (mut start, mut vel) = match self.nodes[i].path {
                Some(path) => {
                    let e = self.path_dual(i, path, path.origin);
                    (e.map(|d| d.x), e.map(|d| d.v))
                }
                None => {
                    let e = self.laid_dual(i);
                    let mut off = [D::c(0.0); 2];
                    self.lanes_dual(i, prop::OFFSET.id, &mut off);
                    let s: [D; 4] = core::array::from_fn(|k| e[k].add(off[k % 2]));
                    (s.map(|d| d.x), s.map(|d| d.v))
                }
            };
            for k in 0..4 {
                let [offset, speed] = drawn.map(|v| if k % 2 == 0 { v.x } else { v.y });
                start[k] += offset;
                vel[k] += speed;
            }
            self.nodes[i].frame = frame;
            self.start_path(i, start, vel, anim);
            return;
        }
        if had_path {
            // Leave screen travel: continue in parent space from the exact current place.
            let (w, wv) = self.exact_world(i);
            let (mut po, mut pv) = self.parent_origin_exact(i);
            let trail = self.trail_dual(i);
            po.x += trail.x;
            pv.x += trail.v;
            let mut off = [D::c(0.0); 2];
            self.lanes_dual(i, prop::OFFSET.id, &mut off);
            po.x += off[0].x;
            po.y += off[1].x;
            pv.x += off[0].v;
            pv.y += off[1].v;
            let local = Rect::from_edges(w[0] - po.x, w[1] - po.y, w[2] - po.x, w[3] - po.y);
            let lv = [wv[0] - pv.x, wv[1] - pv.y, wv[2] - pv.x, wv[3] - pv.y];
            self.end_path(i);
            self.track_set(i, prop::FRAME, local);
            let t = self.nodes[i].tracks[prop::FRAME.id.0 as usize].unwrap();
            // Give the lanes their velocity, then retarget per axis.
            let tid = t.index_usize();
            self.store.animate_raw(tid, &local.edges(), Anim::new(Curve::Spring(Spring::SMOOTH)), Some(&lv));
        } else if self.nodes[i].tracks[prop::FRAME.id.0 as usize].is_none() {
            self.track_set(i, prop::FRAME, old);
        }
        self.nodes[i].frame = frame;
        let t = self.nodes[i].tracks[prop::FRAME.id.0 as usize].unwrap().index_usize();
        let mut cur = self.store.lane_targets_4(t);
        let e = frame.edges();
        if let Some((lead, trail)) = anim.lead {
            for axis in 0..2 {
                let (a, b) = (axis, axis + 2);
                let (sa, sb) = if e[a] + e[b] > cur[a] + cur[b] { (trail, lead) } else { (lead, trail) };
                let mut tg = cur;
                tg[a] = e[a];
                self.store.animate_raw(t, &tg, sa, None);
                cur[a] = e[a];
                let mut tg = cur;
                tg[b] = e[b];
                self.store.animate_raw(t, &tg, sb, None);
                cur[b] = e[b];
            }
        } else {
            // X edges on the X curve, Y edges on the Y curve; unchanged lanes are skipped.
            self.store.animate_raw(t, &[e[0], cur[1], e[2], cur[3]], anim.x, None);
            self.store.animate_raw(t, &e, anim.y, None);
        }
        self.nodes[i].dirty = true;
    }

    pub(super) fn parent_target_origin(&self, i: usize) -> Vec2 {
        let mut o = Vec2::ZERO;
        let mut p = self.nodes[i].parent;
        while p != NONE {
            let f = self.nodes[p as usize].frame;
            o.x += f.x;
            o.y += f.y;
            p = self.nodes[p as usize].parent;
        }
        o
    }

    /// Root-space layout edges of node `i` at `now()`, with velocities, sampled exactly.
    pub(super) fn exact_world(&self, i: usize) -> ([f32; 4], [f32; 4]) {
        let e = self.world_dual(i);
        (e.map(|d| d.x), e.map(|d| d.v))
    }

    /// Root-space presented edges of node `i` at `now()`, including its own offset and presence
    /// shift, with velocities, sampled exactly.
    pub(super) fn exact_shown(&self, i: usize) -> ([f32; 4], [f32; 4]) {
        let w = self.world_dual(i);
        let o = self.origin_dual(i);
        let e = [o[0], o[1], o[0].add(w[2].sub(w[0])), o[1].add(w[3].sub(w[1]))];
        (e.map(|d| d.x), e.map(|d| d.v))
    }

    /// Trace-only check of presented scene edges against unsnapped analytic tracks at `now`.
    /// Returns (edge samples, maximum device-pixel error). Renderer-owned growth/scroll and
    /// post-layout scale transforms are outside this metric.
    pub fn sampling_error(&self) -> (usize, f32) {
        let mut count = 0;
        let mut error = 0.0f32;
        for &slot in &self.order {
            let i = slot as usize;
            let Some(p) = self.nodes[i].last else { continue };
            if !self.nodes[i].alive { continue; }
            let w = self.world_dual(i);
            let o = self.origin_dual(i);
            let ideal = [o[0].x, o[1].x, o[0].x + w[2].x - w[0].x, o[1].x + w[3].x - w[1].x];
            for (shown, exact) in p.rect.edges().into_iter().zip(ideal) {
                error = error.max((shown - exact).abs() * self.store.pixel_scale());
                count += 1;
            }
        }
        (count, error)
    }

    fn parent_origin_exact(&self, i: usize) -> (Vec2, Vec2) {
        let p = self.nodes[i].parent;
        if p == NONE {
            return (Vec2::ZERO, Vec2::ZERO);
        }
        let o = self.origin_dual(p as usize);
        (Vec2::new(o[0].x, o[1].x), Vec2::new(o[0].v, o[1].v))
    }

    /// Overwrites `out` with the property's exact lanes, if its track has a sample.
    pub(super) fn lanes_dual(&self, i: usize, p: PropId, out: &mut [D]) {
        let Some((x, v)) = self.nodes[i].tracks[p.0 as usize].and_then(|t| self.sample(t)) else { return };
        for (k, o) in out.iter_mut().enumerate() {
            *o = D { x: x[k], v: v[k] };
        }
    }

    pub(super) fn world_dual(&self, i: usize) -> [D; 4] {
        let n = &self.nodes[i];
        if let Some(path) = n.path {
            return self.path_dual(i, path, self.parent_target_origin(i));
        }
        self.laid_dual(i)
    }

    /// Where node `i` is on its screen path, ending at its layout plus own offset under parent
    /// origin `origin`.
    pub(super) fn path_dual(&self, i: usize, path: Path, origin: Vec2) -> [D; 4] {
        let mut p = [D::c(1.0); 2];
        self.lanes_dual(i, prop::TRAVEL.id, &mut p);
        let mut off = [D::c(0.0); 2];
        self.lanes_dual(i, prop::OFFSET.id, &mut off);
        let s = path.start.edges();
        let end = path.end_local.translated(origin).edges();
        core::array::from_fn(|k| {
            let e = D::c(end[k]).add(off[k % 2]);
            D::c(s[k]).add(e.sub(D::c(s[k])).mul(p[k % 2]))
        })
    }

    /// The origin children of `i` ride: layout origin plus offsets (own and presence).
    /// When `i` is on a screen path, `world_dual` already blends its own offset by travel.
    pub(super) fn origin_dual(&self, i: usize) -> [D; 2] {
        let w = self.world_dual(i);
        let mut off = [D::c(0.0); 2];
        if self.nodes[i].path.is_none() {
            self.lanes_dual(i, prop::OFFSET.id, &mut off);
        }
        let mut p = [D::c(1.0)];
        self.lanes_dual(i, prop::PRESENCE.id, &mut p);
        let n = &self.nodes[i];
        let (at0, at1) = n.presence.pair();
        let fx = |k: usize| D::c(at0[k]).mul(D::c(1.0).sub(p[0])).add(D::c(at1[k]).mul(p[0]));
        let size = [w[2].sub(w[0]), w[3].sub(w[1])];
        let reduced = self.store.reduced_motion();
        core::array::from_fn(|a| {
            let mut o = w[a].add(off[a]);
            if !reduced {
                o = o.add(fx(2 + a)).add(fx(4 + a).mul(size[a]));
            }
            o
        })
    }
}
