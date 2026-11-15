//! A loop-only step: a spinner's dots change opacity without the scene being built again. The
//! ordinary paint records the instance each eligible dot drew (a hidden, decorative leaf with one
//! plain fill and an opacity cycle, nothing else about it moving); a step on the loop's own wake
//! samples the cycles into those instances. Every paint starts the record again. The host draws
//! only what changed (`Renderer::arm_cycle` on the web).

use super::*;

#[derive(Default)]
pub(in crate::runtime) struct CyclePaint {
    /// Counts paints: a renderer told the instances are the same paint's as last frame's knows
    /// only the slots can differ.
    paint: u64,
    generation: u64,
    slots: Vec<Slot>,
    mixed: bool,
}

struct Slot {
    node: usize,
    at: usize,
    curve: crate::Cycle,
    ink: [f32; 4],
    rect: [f32; 4],
}

impl CyclePaint {
    pub(in crate::runtime) fn begin(&mut self, generation: u64) {
        self.paint += 1;
        self.generation = generation;
        self.slots.clear();
        self.mixed = false;
    }

    pub(in crate::runtime) fn other_deadline(&mut self) { self.mixed = true; }

    pub(in crate::runtime) fn record(&mut self, node: usize, at: usize, curve: crate::Cycle, ink: [f32; 4], rect: [f32; 4]) {
        self.slots.push(Slot { node, at, curve, ink, rect });
    }
}

impl<S: 'static> Runtime<S> {
    /// The paint the instances come from, and the instances a loop-only step changes (ascending).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn cycle_paint(&self) -> (u64, Vec<u32>) {
        let c = &self.paint_keep.cycles;
        (c.paint, c.slots.iter().map(|s| s.at as u32).collect())
    }

    /// A loop's wake on a window that otherwise rests: samples the cycles into their slots of
    /// the last paint, and nothing else runs. False, for an ordinary frame, when anything else
    /// is due: another deadline, the app's timer, input, a commit, a changed scene.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn paint_cycle_tick(&mut self) -> bool {
        let Some((due, phase)) = self.loop_wake else { return false };
        let wall = self.now_clock();
        let keep = &self.paint_keep.cycles;
        if wall < due || keep.mixed || keep.generation != self.commits || keep.slots.is_empty()
            || keep.slots.len() != self.loops.len() || self.reduced_motion || self.time_scale <= 0.0
            || !self.live_visible() || !self.rests_except_loop() || self.tracing()
            || self.poll_wake_deadline().is_some_and(|d| d <= wall)
        { return false; }
        let Some(dt) = self.ambient.map(f64::from) else { return false };
        // A layer may have folded a primitive after its slot was observed. Never patch a
        // different instance, even if the instance count stayed the same.
        if !keep.slots.iter().all(|s| self.instances.get(s.at).is_some_and(|q| q.rect == s.rect && q.params[1] == KIND_RECT)) { return false; }
        if self.virtual_fill.resuming(wall) { return false; }
        // No poll: as on a window resting without a loop, the app's poll runs when its timers,
        // wakes or input ask, each of which takes an ordinary frame.
        self.clock = wall;
        let _ = self.text.mask_cache_frame(false);
        for s in &self.paint_keep.cycles.slots {
            let a = s.curve.sample(self.clock);
            let q = &mut self.instances[s.at];
            q.fill = s.ink.map(|v| v * a);
            q.params[3] = a;
            let inh = &mut self.inh_scratch[s.node];
            inh.opacity = a; inh.paint_opacity = a; inh.hit_opacity = a;
        }
        self.loop_wake = Some((self.clock + dt, phase + (self.clock + dt - due) * f64::from(self.time_scale)));
        true
    }
}
