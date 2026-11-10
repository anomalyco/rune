//! Which tracks moved in the last tick, and which are set to move.
use super::{AnimationStore, TrackId};
use crate::props::{NodeId, PropId};

/// What a track is doing at this frame's time. The one place a hold is decided, so the tick,
/// [`running`](AnimationStore::running) and
/// [`waiting_until`](AnimationStore::waiting_until) can't disagree about whether frames are
/// needed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Mode {
    /// It moves now: sample it.
    Moving,
    /// Its delayed start is due, but the tick that applies it hasn't run: it moves now, and
    /// still wants frames this instant.
    Due,
    /// It holds until a delayed start comes due at `due`, on its own clock's time.
    Waiting { due: f64 },
    /// It holds with nothing due: settled, and the tick retires it.
    Settled,
    /// It holds because its clock is stopped. It moves again when the clock does.
    Frozen,
}

impl Mode {
    /// Nothing on screen changes until something moves it, so no sample is needed.
    pub(super) fn holds(self) -> bool {
        matches!(self, Mode::Waiting { .. } | Mode::Settled | Mode::Frozen)
    }
}

impl AnimationStore {
    /// What track `t` is doing at this frame's time: the one derivation of its hold.
    #[inline]
    pub(super) fn mode(&self, t: usize) -> Mode {
        if self.frozen(t) {
            return Mode::Frozen;
        }
        let now = self.time_of(t);
        let due = self.t_next_due[t];
        if self.t_settle_at[t] > now {
            return Mode::Moving;
        }
        if due > now {
            return if due.is_finite() { Mode::Waiting { due } } else { Mode::Settled };
        }
        Mode::Due
    }

    /// The track's timing or pose just changed, so the next tick samples it again even though
    /// it holds.
    pub(super) fn resample(&mut self, t: usize) {
        self.t_held[t] = false;
    }
}

impl AnimationStore {
    /// Tracks sampled (or `set`) in the last tick: what changed on screen.
    pub fn touched(&self) -> impl Iterator<Item = (NodeId, PropId)> + '_ {
        self.touched_tracks().map(|(_, node, prop)| (node, prop))
    }

    /// Tracks animating now, ticked yet or not: what's set to move.
    pub fn active(&self) -> impl Iterator<Item = (NodeId, PropId)> + '_ {
        self.active.iter().map(|&t| {
            let m = &self.t_meta[t as usize];
            (m.node, m.prop)
        })
    }

    /// When everything running only waits for a delayed start, holding still until then: the
    /// store time the first of them starts. `None` while anything moves, or nothing waits. A
    /// host can sleep until then rather than draw frames that show nothing new. Tracks on a
    /// stopped clock don't count, as in [`running`](Self::running).
    pub fn waiting_until(&self) -> Option<f64> {
        let mut first = f64::INFINITY;
        for &t in &self.active {
            let t = t as usize;
            match self.mode(t) {
                Mode::Waiting { due } => {
                    let now = self.time_of(t);
                    let k = self.clock_scale(self.t_clock[t]) as f64;
                    first = first.min(self.now + (due - now).max(0.0) / k);
                }
                // Due but not yet applied: wake now, as a delayed start that came due.
                Mode::Due => first = first.min(self.now),
                // A stopped clock never comes due on its own.
                Mode::Frozen => {}
                // Moving, or settled and about to be retired: frames are needed now.
                Mode::Moving | Mode::Settled => return None,
            }
        }
        first.is_finite().then_some(first)
    }

    /// Like [`touched`](Self::touched), with track ids.
    pub fn touched_tracks(&self) -> impl Iterator<Item = (TrackId, NodeId, PropId)> + '_ {
        self.touched.iter().filter(|&&id| self.contains(id)).map(|&id| {
            let m = &self.t_meta[id.index as usize];
            (id, m.node, m.prop)
        })
    }
}

impl AnimationStore {
    /// Moves track `t`'s lanes by `d` (value, target and anything scheduled) and leaves the motion
    /// on them as it is: the same motion in coordinates shifted under it.
    pub(crate) fn translate(&mut self, t: usize, d: &[f32]) {
        let l0 = self.t_lane0[t] as usize;
        for (i, &d) in d.iter().enumerate().take(self.t_n[t] as usize) {
            if d == 0.0 {
                continue;
            }
            let l = l0 + i;
            self.l_target[l] += d;
            self.l_value[l] += d;
            let mut p = self.l_pending[l];
            while p != super::NONE {
                self.pending[p as usize].target += d;
                p = self.pending[p as usize].next;
            }
        }
        self.set_touched.push(TrackId { index: t as u32, generation: self.t_meta[t].generation });
    }
}
