//! A loop-only step: what a loop changes on screen, without the scene being built again. The
//! ordinary paint records the instances each eligible loop drew: a spinner's dot (a hidden,
//! decorative leaf with one plain fill and an opacity cycle, nothing else about it moving), and
//! a shimmer's sliding window (`slide.rs`). A step samples the loops into those instances, and
//! nothing else runs. Every paint starts the record again. The host draws only what changed
//! (`Renderer::arm_cycle` on the web, `Renderer::arm_loop` in a window).

use super::*;

#[derive(Default)]
pub(in crate::runtime) struct CyclePaint {
    /// Counts paints: a renderer told the instances are the same paint's as last frame's knows
    /// only the slots can differ.
    paint: u64,
    generation: u64,
    slots: Vec<Slot>,
    pub(super) slides: Vec<super::slide::Slide>,
    mixed: bool,
    /// The node paint last reached, and how many instances it had drawn by then.
    pub(super) opened: (usize, usize),
    /// The sliding window whose subtree paints now.
    pub(super) open: Option<usize>,
    /// Why this paint recorded no window for a sliding loop it drew, if it didn't.
    pub(super) missed: Option<&'static str>,
    /// What the last step would have drawn, while `RUNE_LOOP_CHECK=1` runs ordinary frames instead.
    check: Option<Vec<Instance>>,
    /// The clock the checked step sampled at: the ordinary frame that runs instead samples there
    /// too (`Runtime::frame`), so the two compare one instant.
    pub(in crate::runtime) check_clock: Option<f64>,
    /// Sliding windows' anchors from the last paint, those this paint posed from, and the one it
    /// posed last (`slide_anchor`).
    pub(super) anchors: Vec<super::slide::Anchor>,
    pub(super) next_anchors: Vec<super::slide::Anchor>,
    pub(super) anchor: Option<(usize, super::slide::Anchor)>,
}

struct Slot {
    node: usize,
    at: usize,
    curve: crate::Cycle,
    ink: [f32; 4],
    /// What else fades it: its own and its ancestors' opacity besides the cycle.
    scale: f32,
    rect: [f32; 4],
}

impl CyclePaint {
    pub(in crate::runtime) fn begin(&mut self, generation: u64) {
        self.paint += 1;
        self.generation = generation;
        self.slots.clear();
        self.slides.clear();
        self.open = None;
        self.missed = None;
        self.opened = (usize::MAX, 0);
        self.mixed = false;
        self.anchors = std::mem::take(&mut self.next_anchors);
        self.anchor = None;
    }

    pub(in crate::runtime) fn other_deadline(&mut self) { self.mixed = true; }

    pub(in crate::runtime) fn record(&mut self, node: usize, at: usize, curve: crate::Cycle, (ink, scale): ([f32; 4], f32), rect: [f32; 4]) {
        self.slots.push(Slot { node, at, curve, ink, scale, rect });
    }

    /// A glyph at `quad` (x0, y0, x1, y1), drawn or left out: a sliding window over it notes it.
    pub(in crate::runtime) fn glyph(&mut self, quad: [f32; 4], drawn: bool) {
        if let Some(k) = self.open { self.slides[k].glyphs.push((quad, drawn)); }
    }
}

/// `RUNE_LOOP_CHECK=1`: a frame a loop-only step could draw runs as an ordinary one, and the
/// two are compared (`loop_checked`). A headless run (`--frames`, on the virtual clock) asks
/// too, before every frame no script event reached (`tooling::headless`), on any platform.
/// `RUNE_LOOP_CHECK_DETAIL=1`: `loop_checked` prints each differing instance.
fn detail() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUNE_LOOP_CHECK_DETAIL").is_some())
}

pub(crate) fn checking() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUNE_LOOP_CHECK").is_some_and(|v| v == "1"))
}

impl<S: 'static> Runtime<S> {
    /// The paint the instances come from, and the instances a loop-only step changes (ascending).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn cycle_paint(&self) -> (u64, Vec<u32>) {
        let c = &self.paint_keep.cycles;
        (c.paint, c.slots.iter().map(|s| s.at as u32).collect())
    }

    /// The paint the instances come from.
    pub(crate) fn paint_count(&self) -> u64 {
        self.paint_keep.cycles.paint
    }

    /// A loop's wake on a window that otherwise rests: samples the cycles into their slots of
    /// the last paint, and nothing else runs. False, for an ordinary frame, when anything else
    /// is due: another deadline, the app's timer, input, a commit, a changed scene.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn paint_cycle_tick(&mut self) -> bool {
        let Some((due, _)) = self.loop_wake else { return false };
        let wall = self.now_clock();
        let keep = &self.paint_keep.cycles;
        if wall < due || keep.mixed || keep.generation != self.commits || keep.slots.is_empty()
            || keep.slots.len() != self.loops.len() || self.reduced_motion || self.time_scale <= 0.0
            || !self.live_visible() || !self.rests_except_loop() || self.tracing()
            || self.poll_wake_deadline().is_some_and(|d| d <= wall)
        { return false; }
        if self.ambient.is_none() || !self.slots_hold() { return false; }
        if self.virtual_fill.resuming(wall) { return false; }
        // No poll: as on a window resting without a loop, the app's poll runs when its timers,
        // wakes or input ask, each of which takes an ordinary frame.
        self.clock = wall;
        let _ = self.text.mask_cache_frame(false);
        self.sample_cycles(&mut Vec::new());
        true
    }

    /// A frame that only a window's loops asked for, `dt` seconds after the last: samples every
    /// loop into the instances it drew, and nothing else runs (no poll, view, layout or paint).
    /// Returns the instances it wrote (ascending), or why it takes an ordinary frame: anything
    /// else is due (input, a wake, a timer, a commit, other motion), a loop on screen is one a
    /// step can't draw, or a step would show or hide anything.
    pub(crate) fn loop_step(&mut self, dt: f64) -> Result<Vec<u32>, &'static str> {
        // Checked on the virtual clock (headless), the step is due where the frame will be.
        let virtual_check = checking() && self.wall_start.is_none();
        let wall = if virtual_check { self.clock + dt } else { self.now_clock() };
        let keep = &self.paint_keep.cycles;
        let loops = keep.slots.len() + 2 * keep.slides.len();
        let why = if self.loops.is_empty() { "no loop" } else if loops != self.loops.len() { self.loop_refusal() }
            else if keep.mixed || keep.generation != self.commits { "other motion or a commit" }
            else if (self.wall_start.is_none() && !virtual_check) || self.reduced_motion || self.time_scale <= 0.0 || !self.live_visible() || self.tracing() { "not live" }
            else if !(0.0..=1.0 / 30.0).contains(&dt) { "late" }
            else if self.poll_wake_deadline().is_some_and(|d| d <= wall) { "timer due" }
            else if self.virtual_fill.resuming(wall) { "a list filling" }
            else if let Some(work) = self.waiting_work() { work }
            else { "" };
        if !why.is_empty() { return Err(why); }
        // Running, only loops move; resting, a stepped cycle's wake is due.
        let due = self.loop_wake.is_some_and(|(due, _)| wall >= due);
        if self.animating && self.anim_sources != AnimSource::LOOP {
            let other = [(AnimSource::TRACKS, "motion: a track"), (AnimSource::GROWTH, "motion: growth"), (AnimSource::SCROLL, "motion: a scroll"),
                (AnimSource::REVEAL, "motion: a reveal"), (AnimSource::LIVE, "motion: a live value"), (AnimSource::TEXT_FILL, "motion: text filling"),
                (AnimSource::VIRTUAL_FILL, "motion: a list filling"), (AnimSource::SCALE_SETTLE, "motion: a scale settling"),
                (AnimSource::VIDEO, "motion: a video"), (AnimSource::TEXT_FLIGHT, "motion: a text flight")];
            return Err(other.iter().find(|(bit, _)| self.anim_sources & bit != 0).map_or("motion: unknown", |o| o.1));
        }
        if !self.animating && !due { return Err("no loop due"); }
        if !self.slots_hold() || (due && self.ambient.is_none()) { return Err("a slot moved"); }
        let t = self.t + dt * f64::from(self.time_scale);
        let poses = self.slide_poses(t)?;
        if checking() {
            // What the step would draw, for the ordinary frame that runs instead (`loop_checked`).
            let mut predicted = self.instances.clone();
            for s in &self.paint_keep.cycles.slots {
                let a = s.curve.sample(wall) * s.scale;
                predicted[s.at].fill = s.ink.map(|v| v * a);
                predicted[s.at].params[3] = a;
            }
            for (sl, &(clip, fade)) in self.paint_keep.cycles.slides.iter().zip(&poses) {
                for q in &mut predicted[sl.instances.clone()] { q.clip = clip; q.fade = fade; }
            }
            self.paint_keep.cycles.check = Some(predicted);
            self.paint_keep.cycles.check_clock = Some(wall);
            return Err("checked against an ordinary frame");
        }
        self.clock = wall;
        self.t = t;
        self.scene.set_time(t);
        let _ = self.text.mask_cache_frame(false);
        let mut patched = Vec::new();
        self.sample_cycles(&mut patched);
        self.apply_slides(&poses, &mut patched);
        patched.sort_unstable();
        Ok(patched)
    }

    /// `RUNE_LOOP_CHECK=1` is set (`checking`).
    pub(crate) fn loop_check_on(&self) -> bool { checking() }

    /// Why a loop on screen has no slot or window (`loop_step`'s answer): the first such loop's
    /// reason, from what paint read.
    fn loop_refusal(&self) -> &'static str {
        let c = &self.paint_keep.cycles;
        if let Some(why) = c.missed { return why; }
        for id in self.loops.keys() {
            let Some(&i) = self.node_index.get(id) else { continue };
            if c.slots.iter().any(|s| s.node == i) || c.slides.iter().any(|s| s.window == i || s.counter == i) { continue; }
            let n = &self.nodes[i];
            let Some(rune_motion::LoopKind::OpacityCycle(_)) = n.looping.as_ref().map(|l| l.kind) else { return "a loop that isn't a cycle or a window" };
            let parent = n.parent.map_or_else(|| self.root_inh(), |p| self.inh_scratch[p]);
            return if !self.seen.get(i).copied().unwrap_or(false) { "a cycle out of sight" }
                else if !n.a11y.hidden { "a cycle screen readers see" }
                else if self.nodes.get(i + 1).is_some_and(|m| m.parent == Some(i)) { "a cycle with children" }
                else if n.vis.layer_opacity != 1.0 || n.vis.paint_alpha != 1.0 { "a cycle that fades itself" }
                else if parent.in_cache || parent.skip_paint || parent.turn != 0 { "a cycle in a layer" }
                else if n.handlers.get().is_some() || n.focus.get().is_some() || n.vis_flags.hit() { "a cycle that takes input" }
                else { "a cycle paint didn't record" };
        }
        "a loop it can't draw"
    }

    /// `RUNE_LOOP_CHECK=1`: what the last loop-only step would have drawn against what the
    /// ordinary frame that ran instead drew. The instances that differ, and by how much at most
    /// (the largest difference in any field, in pixels or colour; infinite when their number or
    /// soft edges differ). None when no step was checked.
    pub(crate) fn loop_checked(&mut self) -> Option<(usize, f32)> {
        let predicted = self.paint_keep.cycles.check.take()?;
        if predicted.len() != self.instances.len() { return Some((usize::MAX, f32::INFINITY)); }
        let (mut differ, mut worst) = (0, 0.0f32);
        for (i, (p, q)) in predicted.iter().zip(&self.instances).enumerate() {
            if bytemuck::bytes_of(p) == bytemuck::bytes_of(q) { continue; }
            let fields = |i: &Instance| [i.rect, i.radii, i.fill, i.border, i.params, i.uv, i.clip];
            let d = if p.fade != q.fade { f32::INFINITY } else {
                fields(p).iter().flatten().zip(fields(q).iter().flatten()).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)
            };
            if d > 1e-3 {
                differ += 1;
                // `RUNE_LOOP_CHECK_DETAIL=1`: which instance, and what the step and the frame had.
                if detail() {
                    eprintln!("rune: loop check: instance {i} (kind {}) step {:?} frame {:?}", q.params[1], (p.rect, p.fill, p.params, p.clip, p.uv), (q.rect, q.fill, q.params, q.clip, q.uv));
                }
            }
            worst = worst.max(d);
        }
        Some((differ, worst))
    }

    /// A layer may have folded a primitive after its slot was observed. Never patch a different
    /// instance, even if the instance count stayed the same.
    fn slots_hold(&self) -> bool {
        self.paint_keep.cycles.slots.iter().all(|s| self.instances.get(s.at).is_some_and(|q| q.rect == s.rect && q.params[1] == KIND_RECT))
    }

    /// Samples each cycle at the clock into its slot, and moves a stepped cycle's wake on.
    fn sample_cycles(&mut self, patched: &mut Vec<u32>) {
        for s in &self.paint_keep.cycles.slots {
            let a = s.curve.sample(self.clock) * s.scale;
            let q = &mut self.instances[s.at];
            q.fill = s.ink.map(|v| v * a);
            q.params[3] = a;
            let inh = &mut self.inh_scratch[s.node];
            inh.opacity = a; inh.paint_opacity = a; inh.hit_opacity = a;
            // Every dot, moved or not: what the screen shows is the renderer's to know (a frame
            // that missed its drawable drew nothing), and `arm_loop` redraws only those that
            // differ from what it holds.
            patched.push(s.at as u32);
        }
        if let (Some((due, phase)), Some(dt)) = (self.loop_wake, self.ambient.map(f64::from)) && self.clock >= due {
            self.loop_wake = Some((self.clock + dt, phase + (self.clock + dt - due) * f64::from(self.time_scale)));
        }
    }
}
