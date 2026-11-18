//! Render-side growth, scroll and streaming word-reveal tracks, plus animated paint samples.
use super::*;
use crate::runtime::scroll_mode::{self, Event as ModeEvent};

/// Scroll momentum decay (iOS normal: 0.998 per ms).
pub(super) const SCROLL_DECAY: f32 = 2.002;

/// The (x, y) scroll offsets of node `i`'s scroll viewports, innermost first.
pub(super) fn scroll_offsets<'a>(scrolls: &'a HashMap<NodeId, ScrollState>, nodes: &'a [Node], i: usize) -> impl Iterator<Item = [f32; 2]> + 'a {
    std::iter::successors(Some(i), |&p| nodes[p].parent.filter(|_| nodes[p].above.is_none()))
        .skip(1)
        .filter_map(|p| scrolls.get(&nodes[p].id).map(|st| [st.x.shift() as f32, st.offset as f32]))
}

/// Marks every ancestor of a marked node.
pub(super) fn mark_ancestors(nodes: &[Node], marked: &mut [bool]) {
    for i in 0..nodes.len() {
        if !marked[i] { continue; }
        let mut p = nodes[i].parent;
        while let Some(j) = p {
            if marked[j] { break; }
            marked[j] = true;
            p = nodes[j].parent;
        }
    }
}

impl<S: 'static> Runtime<S> {
    pub(super) fn set_or_anim<T: rune_motion::Animatable>(&mut self, id: NodeId, p: Prop<T>, val: T, s: Spring, instant: bool) {
        if instant {
            self.scene.set(id, p, val);
        } else {
            self.scene.animate(id, p, val, s);
        }
    }

    pub(super) fn color(&self, id: NodeId, prop: Prop<Color>, rest: Color) -> Color {
        if self.scene.store().is_animating(id, prop) { self.scene.get(id, prop).unwrap_or(rest) } else { rest }
    }

    /// Shadow `k` of node `id` with its geometry where its spring has it.
    pub(super) fn shadow_at(&self, id: NodeId, k: usize, sh: &crate::el::Shadow) -> crate::el::Shadow {
        match P_SHADOW_GEO.get(k) {
            Some(&prop) if self.scene.store().active_len() > 0 && self.scene.store().is_animating(id, prop) => match self.scene.get(id, prop) {
                Some(g) => crate::el::Shadow { x: g.x, y: g.y, blur: g.w.max(0.0), spread: g.h, ..*sh },
                None => *sh,
            },
            _ => *sh,
        }
    }

    pub(super) fn step_growth(&mut self, dt: f32) -> bool {
        if self.stacks.is_empty() {
            return false;
        }
        // Frame time, stepped in whole substeps and the rest (`growth::substeps`).
        let steps = crate::growth::substeps(dt);
        let now = self.t;
        let mut moving = false;
        let mut placed = false;
        let (mut restored, mut gave) = (Vec::new(), Vec::new());
        for (cid, stack) in self.stacks.iter_mut() {
            let Some(&ci) = self.node_index.get(cid) else { continue };
            let Some(spec) = self.nodes[ci].growth.as_deref().copied() else { continue };
            let n_rows = stack.order.len();
            let mut st_growing = !stack.pending_heights.is_empty();
            let mut st_moving = st_growing;
            let (in_runs, run_grown) = Self::step_growth_runs(stack, &mut self.grows, steps, spec);
            stack.grown += run_grown;
            for (k, id) in stack.order.iter().enumerate() {
                let Some(g) = self.grows.get_mut(id) else { continue };
                let h0 = g.h;
                let in_run = in_runs.contains(id);
                if let Some(&at) = stack.closing.get(id)
                    && now >= at
                {
                    if self.reduced_motion {
                        *g = GrowTrack::settled(0.0);
                    } else {
                        g.retarget(0.0);
                    }
                }
                // A row above the tail (the last row) carries it.
                let carrying = k + 2 == n_rows;
                let mut left = dt.min(1.0);
                for step in steps.filter(|_| !in_run) {
                    if !stack.pending_heights.is_empty() {
                        let at = now - f64::from(left);
                        if !stack.width_hold.defers(at) && let Some(height) = stack.pending_heights.remove(id) {
                            g.retarget(height);
                        }
                    }
                    g.step_by(step, spec.spring, &spec.edge, carrying);
                    left -= step;
                }
                // A row nobody saw last frame draws no feather: it has none to ease, and grows one
                // again as it comes into view.
                if g.mask > 0.0 && self.built
                    && self.node_index.get(id).is_some_and(|&i| !self.seen.get(i).copied().unwrap_or(true))
                {
                    g.mask = 0.0;
                }
                let r = std::mem::take(&mut g.restore);
                if r > 0.0 { restored.push((ci, *id, r)); }
                if let Some(share) = g.give { gave.push((*id, (h0 - g.h) * share)); }
                if !g.moving() { g.give = None; }
                stack.grown += (g.h - h0).max(0.0);
                st_growing |= g.target > g.h + 1.0 / 128.0;
                st_moving |= g.moving() || (g.h - h0).abs() > 1e-5;
                st_moving |= stack.slides.step(*id, g, spec.spring, steps);
            }
            stack.order.retain(|id| {
                let keep = !stack.closing.contains_key(id) || self.grows.get(id).is_some_and(|g| g.h > 0.01 || g.target > 0.0);
                if !keep {
                    self.grows.remove(id);
                }
                keep
            });
            stack.closing.retain(|id, _| stack.order.contains(id));
            stack.slides.keep(&stack.order);
            stack.growing = st_growing;
            stack.moving = st_moving;
            placed |= stack.handoff.place(&mut self.grows, &mut self.scene, &self.nodes, &self.node_index, &mut self.scrolls);
            // Keep the shared clock awake through the hold. No timer thread or remeasurement.
            moving |= st_moving;
        }
        // Growth steps after Scene::frame. Scroll follow and painting must see this step's
        // shared edge, even in a viewport with no scroll-offset callback to present it.
        if placed { self.scene.present(); }
        for (ci, id, r) in restored { self.restore_above(ci, id, r); }
        for (id, d) in gave { self.lower_hold(id, d); }
        moving
    }

    /// Presented content extent of node `c` if it is a growth stack (points).
    pub(super) fn stack_extent(&self, c: usize) -> Option<f64> {
        let n = &self.nodes[c];
        n.growth.as_ref()?;
        let stack = self.stacks.get(&n.id)?;
        let mut count = 0usize;
        let rows: f64 = stack.order.iter().filter_map(|id| self.grows.get(id)).map(|g| { count += 1; f64::from(g.h) }).sum();
        let gaps = f64::from(stack.gap) * count.saturating_sub(1) as f64;
        Some(f64::from(stack.pad_top) + rows + gaps + f64::from(stack.pad_bottom))
    }

    /// How far scroll viewport `i`'s content reaches past its bottom, negative while it fits.
    pub(super) fn scroll_overflow(&self, i: usize) -> f64 {
        let view = self.nodes[i].target;
        let content_h = self.scroll_children(i).map(|c| {
            let n = &self.nodes[c];
            f64::from(n.target[1] - view[1]) + self.stack_extent(c).unwrap_or(f64::from(n.target[3] - n.target[1]))
        }).reduce(f64::max).unwrap_or(0.0);
        content_h - f64::from(view[3] - view[1])
    }

    pub(super) fn step_scroll(&mut self, dt: f32) -> bool { self.step_scroll_by(dt, false) }

    /// Steps every scroll viewport `dt` seconds on: a frame's own step, or (`catch_up`) the time
    /// it charges to motion that was already running (`frame::clock::split`).
    pub(super) fn step_scroll_by(&mut self, dt: f32, catch_up: bool) -> bool {
        let slack_dt = std::mem::take(&mut self.slack_dt);
        let scale = f64::from(self.env.scale * self.zoom());
        // Motion reduced mid-ease: a wheel's ease lands where it was going, at once.
        let land_wheels = !self.wheel_glides();
        let mut moving = false;
        for i in 0..self.nodes.len() {
            let Some(sp) = self.nodes[i].scroll else { continue };
            if self.subtree_exiting(i) { continue; }
            let id = self.nodes[i].id;
            let axes = self.nodes[i].scroll_axes;
            if axes.x() { moving |= self.step_scroll_x(i, dt, catch_up); }
            // A horizontal viewport doesn't move vertically, even with content taller than it.
            let overflow = if axes.y() { self.scroll_overflow(i) } else { 0.0 };
            if let Some(active) = self.step_touch_scroll(id, overflow, dt, false) {
                moving |= active;
                continue;
            }
            let held = !self.requests.is_empty() && {
                let path = &self.nodes[i].path;
                self.requests.owns(path)
                    && self.scroll_stack(i).and_then(|c| self.stacks.get(&self.nodes[c].id)).is_some_and(|s| s.growing)
            };
            let laid = if axes.y() { self.ease_to_shrunk_end(i, overflow) } else { None };
            let Some(st) = self.scrolls.get_mut(&id) else { continue };
            let _ = scroll_mode::step(&mut st.mode, ModeEvent::Step { x_v: st.x.coast() });
            st.max = overflow.max(0.0);
            match st.mode {
                ScrollMode::Pinned(_) if sp.stick_bottom => {
                    // A reader stuck on a shrinking stack's end doesn't rise with its rows' last fraction.
                    st.offset = laid.filter(|&end| st.offset <= end + 1e-6).map_or(st.max, |end| st.max.min(end));
                }
                ScrollMode::Glide { .. } => moving |= scroll_bottom::glide(st, dt),
                ScrollMode::Momentum { v, .. } if v.abs() > 1.0 => {
                    // Exact for exponential decay, so a long frame carries the same distance as
                    // the short frames it stands for.
                    let decay = (-SCROLL_DECAY * dt).exp();
                    st.offset += f64::from(v * (1.0 - decay) / SCROLL_DECAY);
                    let edge = st.offset <= 0.0 || st.offset >= st.max;
                    st.mode = if edge { st.mode.to_rest() } else { st.mode.to_momentum(v * decay) };
                    moving = true;
                }
                ScrollMode::Momentum { .. } => st.mode = st.mode.to_rest(),
                ScrollMode::Wheel { mut curve, .. } => {
                    curve.follow(st.offset);
                    // An end that shrank under the ease takes its target with it; one that grew does
                    // too when the wheel was sent to the end of a viewport that follows its bottom
                    // (a reply streaming in), so it lands there and pins.
                    if curve.target() > st.max || (sp.stick_bottom && curve.to_end() && curve.target() < st.max) { curve.retarget(st.max, scale); }
                    if land_wheels { curve.advance(f64::INFINITY, false) } else { curve.advance(f64::from(dt), catch_up) }
                    // On whole device pixels, as Chromium's scroll offsets are: everything inside
                    // moves by the same whole pixels each frame and draws as it did, rather than
                    // landing at a new fraction of a pixel every frame.
                    st.offset = scroll_wheel::on_grid_or_end(curve.value(), st.max, scale);
                    curve.applied(st.offset);
                    // The hand moved it: the app hears a wheel's ease as the wheel.
                    st.moved();
                    // An ease sent to a following viewport's end pins the frame it gets there, though
                    // the end growing under it restarts its segment every frame a reply grows.
                    let pins = sp.stick_bottom && curve.to_end() && st.offset == st.max;
                    if curve.done() || pins { st.land(sp.stick_bottom) } else { st.mode = st.mode.to_wheel(curve); moving = true; }
                }
                ScrollMode::Pinned(_) | ScrollMode::Rest(_) => {}
            }
            st.offset = st.offset.clamp(0.0, st.max);
            // Landed on a shrinking stack's new end: stick once its rows are within half a point
            // of it, not while they're still short of it. A glide that reaches the end hands it
            // to the pin.
            let shrinking = laid.is_some_and(|end| end < st.max - 0.5 && st.offset < st.max - 1e-3);
            if sp.stick_bottom && !held && !shrinking && st.glide().is_none() && st.mode.wheel().is_none() && st.offset >= st.max - 0.5 && st.coast().abs() <= 1.0 {
                st.pin();
            }
            let _ = scroll_mode::step(&mut st.mode, ModeEvent::Laid(laid));
            if sp.stick_bottom && axes.y() && slack_dt > 0.0 { moving |= self.step_slack(i, slack_dt); }
        }
        moving
    }

    pub(super) fn reveals_active(&mut self) -> bool {
        let store = self.scene.store();
        let mut any = false;
        for (id, st) in self.reveals.iter_mut() {
            let now = store.node_time(*id);
            st.step(RevealEvent::Tick { now });
            any |= st.fading() && store.node_time_scale(*id) > 0.0;
        }
        any
    }
}
