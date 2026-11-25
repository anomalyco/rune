//! Rest-time compaction of dormant track and lane arrays.

use super::{AnimationStore, Extra, FxMap, NONE, Pending, TrackId};

fn shrink_vec<T>(v: &mut Vec<T>, floor: usize) {
    let cap = (v.len().saturating_mul(2)).max(floor);
    if v.capacity() > cap {
        v.shrink_to(cap);
    }
}

impl AnimationStore {
    pub(crate) fn is_animating_any(&self) -> bool {
        !self.active.is_empty()
    }

    /// Compacts live tracks and lanes when idle (no active tracks) and capacity has grown far
    /// beyond live count. Returns `Some(remap)` mapping old track indices to new ones whenever
    /// any live track moved so `Scene` can rewrite its track handles.
    pub(crate) fn compact_with_remap(&mut self) -> Option<Vec<u32>> {
        if !self.active.is_empty() {
            return None;
        }
        let live_tracks = self.index.len();
        let lane_floor = (live_tracks.saturating_mul(8)).max(256);
        let track_floor = (live_tracks.saturating_mul(2)).max(64);
        let sparse = self.t_meta.capacity() > track_floor
            || self.l_target.capacity() > lane_floor
            || self.track_free.capacity() > 64
            || self.extra.capacity() > 64
            || self.pending.capacity() > 64
            || self.active.capacity() > 64
            || self.touched.capacity() > 64
            || self.set_touched.capacity() > 64
            || self.starts.capacity() > 64
            || self.ticked_starts.capacity() > 64
            || self.index.capacity() > track_floor
            || self.node_head.capacity() > (self.node_head.len().saturating_mul(2)).max(64)
            || self.lane_free.iter().any(|v| v.capacity() > 64);
        if !sparse {
            return None;
        }

        if live_tracks == 0 {
            self.l_target = Vec::new();
            self.l_value = Vec::new();
            self.l_vel = Vec::new();
            self.l_layer = Vec::new();
            self.l_more = Vec::new();
            self.l_pending = Vec::new();
            self.l_curve = Vec::new();
            for free in &mut self.lane_free {
                *free = Vec::new();
            }
            self.t_lane0 = Vec::new();
            self.t_n = Vec::new();
            self.t_settle_at = Vec::new();
            self.t_next_due = Vec::new();
            self.t_slot = Vec::new();
            self.t_meta = Vec::new();
            self.t_clock = Vec::new();
            self.t_held = Vec::new();
            self.track_free = Vec::new();
            self.extra = Vec::new();
            self.extra_free = NONE;
            self.pending = Vec::new();
            self.pending_free = NONE;
            self.active = Vec::new();
            self.touched = Vec::new();
            self.set_touched = Vec::new();
            self.starts = Vec::new();
            self.ticked_starts = Vec::new();
            self.index = FxMap::default();
            self.node_head = FxMap::default();
            self.events.shrink();
            shrink_vec(&mut self.clock_free, 16);
            return Some(Vec::new());
        }

        let old_len = self.t_meta.len();
        let mut live_lanes = 0usize;
        for t in 0..old_len {
            if self.t_meta[t].alive {
                live_lanes += self.t_n[t] as usize;
            }
        }

        let mut remap = vec![NONE; old_len];
        let mut l_target = Vec::with_capacity(live_lanes);
        let mut l_value = Vec::with_capacity(live_lanes);
        let mut l_vel = Vec::with_capacity(live_lanes);
        let mut l_layer = Vec::with_capacity(live_lanes);
        let mut l_more = Vec::with_capacity(live_lanes);
        let mut l_pending = Vec::with_capacity(live_lanes);
        let mut l_curve = Vec::with_capacity(live_lanes);
        let mut extra: Vec<Extra> = Vec::new();
        let mut pending: Vec<Pending> = Vec::new();

        let mut t_lane0 = Vec::with_capacity(live_tracks);
        let mut t_n = Vec::with_capacity(live_tracks);
        let mut t_settle_at = Vec::with_capacity(live_tracks);
        let mut t_next_due = Vec::with_capacity(live_tracks);
        let mut t_slot = Vec::with_capacity(live_tracks);
        let mut t_meta = Vec::with_capacity(live_tracks);
        let mut t_clock = Vec::with_capacity(live_tracks);
        let mut t_held = Vec::with_capacity(live_tracks);
        let mut index = FxMap::with_capacity_and_hasher(live_tracks, Default::default());
        let mut node_head = FxMap::with_capacity_and_hasher(self.node_head.len(), Default::default());

        for old_t in 0..old_len {
            let mut meta = self.t_meta[old_t];
            if !meta.alive {
                continue;
            }
            let new_t = t_meta.len() as u32;
            remap[old_t] = new_t;
            let old_l0 = self.t_lane0[old_t] as usize;
            let n = self.t_n[old_t] as usize;
            let new_l0 = l_target.len() as u32;
            for l in old_l0..old_l0 + n {
                l_target.push(self.l_target[l]);
                l_value.push(self.l_value[l]);
                l_vel.push(self.l_vel[l]);
                l_layer.push(self.l_layer[l]);
                l_curve.push(self.l_curve[l]);
                let mut m_head = NONE;
                let mut m_tail = NONE;
                let mut cur = self.l_more[l];
                while cur != NONE {
                    let e = self.extra[cur as usize];
                    let idx = extra.len() as u32;
                    extra.push(Extra { layer: e.layer, next: NONE });
                    if m_head == NONE {
                        m_head = idx;
                    } else {
                        extra[m_tail as usize].next = idx;
                    }
                    m_tail = idx;
                    cur = e.next;
                }
                l_more.push(m_head);
                let mut p_head = NONE;
                let mut p_tail = NONE;
                let mut cur = self.l_pending[l];
                while cur != NONE {
                    let p = self.pending[cur as usize];
                    let idx = pending.len() as u32;
                    pending.push(Pending { next: NONE, ..p });
                    if p_head == NONE {
                        p_head = idx;
                    } else {
                        pending[p_tail as usize].next = idx;
                    }
                    p_tail = idx;
                    cur = p.next;
                }
                l_pending.push(p_head);
            }
            let prev_head = node_head.insert(meta.node.0, new_t).unwrap_or(NONE);
            meta.node_next = prev_head;
            index.insert((meta.node.0, meta.prop.0), new_t);
            t_lane0.push(new_l0);
            t_n.push(self.t_n[old_t]);
            t_settle_at.push(self.t_settle_at[old_t]);
            t_next_due.push(self.t_next_due[old_t]);
            t_slot.push(self.t_slot[old_t]);
            t_meta.push(meta);
            t_clock.push(self.t_clock[old_t]);
            t_held.push(self.t_held[old_t]);
        }

        let remap_ids = |ids: &mut Vec<TrackId>, old_meta: &[super::Meta]| {
            ids.retain_mut(|id| {
                let old = id.index as usize;
                let Some(&new_t) = remap.get(old) else { return false };
                if new_t == NONE || !old_meta[old].alive || old_meta[old].generation != id.generation {
                    return false;
                }
                id.index = new_t;
                true
            });
            shrink_vec(ids, 16);
        };
        remap_ids(&mut self.touched, &self.t_meta);
        remap_ids(&mut self.set_touched, &self.t_meta);
        remap_ids(&mut self.starts, &self.t_meta);
        remap_ids(&mut self.ticked_starts, &self.t_meta);

        self.l_target = l_target;
        self.l_value = l_value;
        self.l_vel = l_vel;
        self.l_layer = l_layer;
        self.l_more = l_more;
        self.l_pending = l_pending;
        self.l_curve = l_curve;
        for free in &mut self.lane_free {
            *free = Vec::new();
        }
        self.t_lane0 = t_lane0;
        self.t_n = t_n;
        self.t_settle_at = t_settle_at;
        self.t_next_due = t_next_due;
        self.t_slot = t_slot;
        self.t_meta = t_meta;
        self.t_clock = t_clock;
        self.t_held = t_held;
        self.track_free = Vec::new();
        self.extra = extra;
        self.extra_free = NONE;
        self.pending = pending;
        self.pending_free = NONE;
        self.active = Vec::new();
        self.index = index;
        self.node_head = node_head;
        self.events.shrink();
        shrink_vec(&mut self.clock_free, 16);
        Some(remap)
    }
}
