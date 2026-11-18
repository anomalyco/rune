//! Per-node sub-clocks: scaled or paused time domains that tracks can run on independently of
//! the store's own clock.

use super::{AnimationStore, NONE, SubClock};
use crate::props::NodeId;

impl AnimationStore {
    /// Runs every track of `node`, and any it creates later, at `scale` times the store's speed:
    /// each `set_time` advances them by `dt × scale`. At 0 they hold exactly where they are,
    /// pending delays included, and [`running`](Self::running) ignores them. Changing the scale
    /// keeps every running animation at its exact place. 1 is the store's own clock.
    pub fn set_node_time_scale(&mut self, node: NodeId, scale: f32) {
        let scale = if scale.is_finite() { scale.max(0.0) } else { 1.0 };
        let old = self.node_clock.get(&node.0).copied().unwrap_or(0);
        if self.clock_scale(old) == scale {
            return;
        }
        let new = if scale == 1.0 { 0 } else { self.clock_at(scale) };
        let delta = self.clock_time(new) - self.clock_time(old);
        if new == 0 {
            self.node_clock.remove(&node.0);
        } else {
            self.clocks[new as usize].nodes += 1;
            self.node_clock.insert(node.0, new);
        }
        let mut t = self.node_head.get(&node.0).copied().unwrap_or(NONE);
        while t != NONE {
            self.rebase(t as usize, delta);
            self.t_clock[t as usize] = new;
            t = self.t_meta[t as usize].node_next;
        }
        self.release_clock(old);
    }

    /// The scale `node`'s tracks run at relative to the store's (1 unless set).
    pub fn node_time_scale(&self, node: NodeId) -> f32 {
        if self.node_clock.is_empty() {
            return 1.0;
        }
        self.clock_scale(self.node_clock.get(&node.0).copied().unwrap_or(0))
    }

    /// The time `node`'s tracks are sampled at: `now()` unless its time is scaled.
    pub fn node_time(&self, node: NodeId) -> f64 {
        if self.node_clock.is_empty() {
            return self.now;
        }
        self.clock_time(self.node_clock.get(&node.0).copied().unwrap_or(0))
    }

    /// The live clock running at `scale`, or a new one reading `now`.
    fn clock_at(&mut self, scale: f32) -> u32 {
        if let Some(c) = self.clocks.iter().skip(1).position(|c| c.nodes > 0 && c.scale == scale) {
            return c as u32 + 1;
        }
        if self.clocks.is_empty() {
            self.clocks.push(SubClock { time: 0.0, scale: 1.0, nodes: 0 });
        }
        let clock = SubClock { time: self.now, scale, nodes: 0 };
        match self.clock_free.pop() {
            Some(c) => {
                self.clocks[c as usize] = clock;
                c
            }
            None => {
                self.clocks.push(clock);
                self.clocks.len() as u32 - 1
            }
        }
    }

    pub(super) fn release_clock(&mut self, c: u32) {
        if c == 0 {
            return;
        }
        let clock = &mut self.clocks[c as usize];
        clock.nodes -= 1;
        if clock.nodes == 0 {
            self.clock_free.push(c);
        }
    }

    pub(super) fn clock_scale(&self, c: u32) -> f32 {
        if c == 0 { 1.0 } else { self.clocks[c as usize].scale }
    }

    pub(super) fn clock_time(&self, c: u32) -> f64 {
        if c == 0 { self.now } else { self.clocks[c as usize].time }
    }

    #[inline]
    pub(crate) fn time_of(&self, t: usize) -> f64 {
        self.clock_time(self.t_clock[t])
    }

    #[inline]
    pub(super) fn frozen(&self, t: usize) -> bool {
        let c = self.t_clock[t];
        c != 0 && self.clocks[c as usize].scale == 0.0
    }

    /// Shifts every absolute time of track `t` by `delta`.
    pub(super) fn rebase(&mut self, t: usize, delta: f64) {
        self.resample(t);
        if delta == 0.0 {
            return;
        }
        let l0 = self.t_lane0[t] as usize;
        for l in l0..l0 + self.t_n[t] as usize {
            self.l_layer[l].t0 += delta;
            self.l_layer[l].end += delta;
            let mut m = self.l_more[l];
            while m != NONE {
                let e = &mut self.extra[m as usize];
                e.layer.t0 += delta;
                e.layer.end += delta;
                m = e.next;
            }
            let mut p = self.l_pending[l];
            while p != NONE {
                self.pending[p as usize].at += delta;
                p = self.pending[p as usize].next;
            }
        }
        self.t_settle_at[t] += delta;
        self.t_next_due[t] += delta;
        self.t_meta[t].logical_at += delta;
    }
}
