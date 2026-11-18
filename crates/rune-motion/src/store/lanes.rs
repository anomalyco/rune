//! Lane segments, additive layer stacks, delayed pending queues, settling and track lifecycle.

use super::{AnimationStore, Extra, MotionEvent, NONE, Pending};
use crate::curve::{Blend, Curve};
use crate::layer::{Eps, Kind, Layer};
use crate::props::NodeId;

/// Most additive layers a lane keeps before it collapses them into one merged segment.
const MAX_ADDITIVE: usize = 8;

impl AnimationStore {
    /// Starts a new segment on lane `l` at time `at` from its exact state there.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_lane(
        &mut self,
        l: usize,
        at: f64,
        target: f32,
        curve: Curve,
        blend: Blend,
        vel: Option<f32>,
        eps: Eps,
    ) {
        let (x, v) = self.sample_lane(l, at);
        self.l_curve[l] = curve;
        match curve {
            Curve::Instant => {
                self.free_extra(l);
                self.l_layer[l] = Layer::REST;
                self.l_target[l] = target;
            }
            Curve::Decay(d) => {
                let v = vel.unwrap_or(v);
                self.free_extra(l);
                self.l_target[l] = x + v / d.k();
                self.l_layer[l] = Layer::decay(d.k(), at, v, eps);
            }
            _ if blend == Blend::Additive => {
                let old = self.l_target[l];
                self.l_target[l] = target;
                let layer = Layer::start(&curve, at, old - target, vel.map_or(0.0, |u| u - v), eps);
                self.push_layer(l, layer, at);
            }
            _ => {
                self.free_extra(l);
                self.l_target[l] = target;
                self.l_layer[l] = Layer::start(&curve, at, x - target, vel.unwrap_or(v), eps);
            }
        }
    }

    fn push_layer(&mut self, l: usize, layer: Layer, at: f64) {
        if layer.kind == Kind::Rest {
            return;
        }
        if self.l_layer[l].kind == Kind::Rest || self.l_layer[l].end <= at {
            self.l_layer[l] = layer;
            return;
        }
        let mut count = 1;
        let mut m = self.l_more[l];
        while m != NONE {
            count += 1;
            m = self.extra[m as usize].next;
        }
        if count >= MAX_ADDITIVE {
            // Too many stacked layers: collapse into one merged segment on the new curve. Exact in
            // position and velocity at `at`.
            // The lane's target already moved; the new layer carries the difference.
            let (x, v) = self.sample_lane(l, at);
            let (o, ov) = layer.sample(at);
            let (x, v) = (x + o, v + ov);
            self.free_extra(l);
            let target = self.l_target[l];
            let curve = self.l_curve[l];
            self.l_layer[l] = Layer::start(&curve, at, x - target, v, Eps { pos: 1e-4, vel: 1e-3 });
            return;
        }
        let idx = self.alloc_extra(Extra { layer, next: self.l_more[l] });
        self.l_more[l] = idx;
    }

    #[inline]
    pub(super) fn sample_lane(&self, l: usize, at: f64) -> (f32, f32) {
        let (mut o, mut v) = self.l_layer[l].sample(at);
        let mut m = self.l_more[l];
        while m != NONE {
            let e = &self.extra[m as usize];
            let (a, b) = e.layer.sample(at);
            o += a;
            v += b;
            m = e.next;
        }
        (self.l_target[l] + o, v)
    }

    /// Sums extra layers and frees the ones that have ended.
    pub(super) fn sample_extra(&mut self, l: usize, now: f64) -> (f32, f32) {
        let (mut o, mut v) = (0.0, 0.0);
        let mut prev = NONE;
        let mut m = self.l_more[l];
        while m != NONE {
            let e = self.extra[m as usize];
            if now >= e.layer.end {
                if prev == NONE {
                    self.l_more[l] = e.next;
                } else {
                    self.extra[prev as usize].next = e.next;
                }
                self.extra[m as usize].next = self.extra_free;
                self.extra_free = m;
            } else {
                let (a, b) = e.layer.sample(now);
                o += a;
                v += b;
                prev = m;
            }
            m = e.next;
        }
        (o, v)
    }

    pub(super) fn final_target(&self, l: usize) -> f32 {
        let mut p = self.l_pending[l];
        let mut target = self.l_target[l];
        while p != NONE {
            target = self.pending[p as usize].target;
            p = self.pending[p as usize].next;
        }
        target
    }

    pub(super) fn apply_due(&mut self, t: usize, now: f64) {
        let l0 = self.t_lane0[t] as usize;
        let n = self.t_n[t] as usize;
        let eps = self.t_meta[t].class.eps(self.pixel_scale);
        for l in l0..l0 + n {
            loop {
                let p = self.l_pending[l];
                if p == NONE || self.pending[p as usize].at > now {
                    break;
                }
                let ev = self.pending[p as usize];
                self.l_pending[l] = ev.next;
                self.pending[p as usize].next = self.pending_free;
                self.pending_free = p;
                let vel = if ev.velocity.is_nan() { None } else { Some(ev.velocity) };
                self.start_lane(l, ev.at, ev.target, ev.curve, ev.blend, vel, eps);
            }
        }
        self.recompute(t);
    }

    /// Recomputes the track's next due time and settle time from its lanes.
    pub(super) fn recompute(&mut self, t: usize) {
        let l0 = self.t_lane0[t] as usize;
        let n = self.t_n[t] as usize;
        let mut due = f64::INFINITY;
        let mut settle = f64::NEG_INFINITY;
        for l in l0..l0 + n {
            let p = self.l_pending[l];
            if p != NONE {
                due = due.min(self.pending[p as usize].at);
            }
            if self.l_layer[l].kind != Kind::Rest {
                settle = settle.max(self.l_layer[l].end);
            }
            let mut m = self.l_more[l];
            while m != NONE {
                settle = settle.max(self.extra[m as usize].layer.end);
                m = self.extra[m as usize].next;
            }
        }
        self.t_next_due[t] = due;
        self.t_settle_at[t] = settle;
        self.resample(t);
    }

    pub(super) fn settle(&mut self, t: usize) {
        let l0 = self.t_lane0[t] as usize;
        let n = self.t_n[t] as usize;
        for l in l0..l0 + n {
            self.free_extra(l);
            self.l_layer[l] = Layer::REST;
            self.l_value[l] = self.l_target[l];
            self.l_vel[l] = 0.0;
        }
        let m = self.t_meta[t];
        self.events.push(MotionEvent::Settled { track: self.id_of(t), node: m.node, prop: m.prop });
        self.take_token(t, true);
    }

    pub(super) fn take_token(&mut self, t: usize, finished: bool) {
        let m = &mut self.t_meta[t];
        if m.token != 0 {
            let token = core::mem::take(&mut m.token);
            let (node, prop) = (m.node, m.prop);
            self.events.push(MotionEvent::Completed { token, node, prop, finished });
        }
    }

    fn activate(&mut self, t: usize) {
        if self.t_slot[t] == NONE {
            self.t_slot[t] = self.active.len() as u32;
            self.active.push(t as u32);
        }
    }

    pub(super) fn deactivate(&mut self, t: usize, slot: usize) {
        self.active.swap_remove(slot);
        if slot < self.active.len() {
            let moved = self.active[slot] as usize;
            self.t_slot[moved] = slot as u32;
        }
        self.t_slot[t] = NONE;
    }

    pub(super) fn push_pending(&mut self, l: usize, mut ev: Pending) {
        // Keep the lane's list sorted by time.
        let mut prev = NONE;
        let mut cur = self.l_pending[l];
        while cur != NONE && self.pending[cur as usize].at <= ev.at {
            prev = cur;
            cur = self.pending[cur as usize].next;
        }
        ev.next = cur;
        let idx = if self.pending_free != NONE {
            let i = self.pending_free;
            self.pending_free = self.pending[i as usize].next;
            self.pending[i as usize] = ev;
            i
        } else {
            self.pending.push(ev);
            (self.pending.len() - 1) as u32
        };
        if prev == NONE {
            self.l_pending[l] = idx;
        } else {
            self.pending[prev as usize].next = idx;
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "Internal lane scheduler takes a track/lane address and the pending animation fields"
    )]
    pub(crate) fn schedule(
        &mut self,
        t: usize,
        lane: usize,
        at: f64,
        target: f32,
        curve: Curve,
        blend: Blend,
        velocity: Option<f32>,
    ) {
        let l = self.t_lane0[t] as usize + lane;
        self.push_pending(l, Pending { at, target, velocity: velocity.unwrap_or(f32::NAN), curve, blend, next: NONE });
    }

    pub(crate) fn cancel_track_pending(&mut self, t: usize) {
        let l0 = self.t_lane0[t] as usize;
        for l in l0..l0 + self.t_n[t] as usize {
            self.cancel_pending(l);
        }
    }

    /// After `schedule` calls: recompute timing and wake the track.
    pub(crate) fn commit_scheduled(&mut self, t: usize, logical_at: f64, token: u64) {
        self.take_token(t, false);
        let m = &mut self.t_meta[t];
        m.token = token;
        m.logical_at = logical_at;
        m.epoch = m.epoch.wrapping_add(1);
        self.recompute(t);
        self.activate(t);
        self.starts.push(self.id_of(t));
    }

    pub(super) fn cancel_pending(&mut self, l: usize) {
        let mut p = self.l_pending[l];
        while p != NONE {
            let next = self.pending[p as usize].next;
            self.pending[p as usize].next = self.pending_free;
            self.pending_free = p;
            p = next;
        }
        self.l_pending[l] = NONE;
    }

    fn alloc_extra(&mut self, e: Extra) -> u32 {
        if self.extra_free != NONE {
            let i = self.extra_free;
            self.extra_free = self.extra[i as usize].next;
            self.extra[i as usize] = e;
            i
        } else {
            self.extra.push(e);
            (self.extra.len() - 1) as u32
        }
    }

    pub(super) fn free_extra(&mut self, l: usize) {
        let mut m = self.l_more[l];
        while m != NONE {
            let next = self.extra[m as usize].next;
            self.extra[m as usize].next = self.extra_free;
            self.extra_free = m;
            m = next;
        }
        self.l_more[l] = NONE;
    }

    pub(super) fn unlink_node(&mut self, node: NodeId, t: u32) {
        let head = self.node_head.get(&node.0).copied().unwrap_or(NONE);
        if head == t {
            let next = self.t_meta[t as usize].node_next;
            if next == NONE {
                self.node_head.remove(&node.0);
            } else {
                self.node_head.insert(node.0, next);
            }
            return;
        }
        let mut cur = head;
        while cur != NONE {
            let next = self.t_meta[cur as usize].node_next;
            if next == t {
                self.t_meta[cur as usize].node_next = self.t_meta[t as usize].node_next;
                return;
            }
            cur = next;
        }
    }

    pub(super) fn free_track(&mut self, t: usize) {
        if !self.t_meta[t].alive {
            return;
        }
        let l0 = self.t_lane0[t] as usize;
        let n = self.t_n[t] as usize;
        for l in l0..l0 + n {
            self.cancel_pending(l);
            self.free_extra(l);
        }
        if self.t_slot[t] != NONE {
            let slot = self.t_slot[t] as usize;
            self.deactivate(t, slot);
        }
        let m = self.t_meta[t];
        self.index.remove(&(m.node.0, m.prop.0));
        self.lane_free[n].push(l0 as u32);
        // Removal cancels the animation: whoever waits on its completion hears so.
        self.take_token(t, false);
        self.t_meta[t].alive = false;
        self.track_free.push(t as u32);
    }
}
