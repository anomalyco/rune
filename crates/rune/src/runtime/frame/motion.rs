//! Per-frame motion stepping, delayed-motion wakes, loop visibility and layer paint order.
use super::*;

impl RuntimeCore {
    /// frame-profile: the paths of drawn nodes whose loop is running, which wake the window.
    #[cfg(feature = "frame-profile")]
    pub(crate) fn fp_loops(&self) -> Vec<String> {
        self.tree.nodes.iter().enumerate()
            .filter(|(i, n)| n.looping.is_some() && self.paint.seen.get(*i).copied().unwrap_or(false) && self.motion.loops.contains_key(&n.id))
            .map(|(_, n)| n.path.clone()).collect()
    }

    /// Whether a drawn node's loop runs (a spinner): what a window shows would fall behind it
    /// while the window draws nothing.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn loops_shown(&self) -> bool {
        self.tree.nodes.iter().enumerate().any(|(i, n)| n.looping.is_some() && self.paint.seen.get(i).copied().unwrap_or(false) && self.motion.loops.contains_key(&n.id))
    }

    /// Wakes when a delayed start at animation time `at` comes due, on the loops' wake: the
    /// frame then runs at exactly that time, however long the host slept. A hidden window
    /// sleeps through it; showing it settles.
    fn wake_for_motion(&mut self, at: f64) {
        self.paint.keep.cycles.other_deadline();
        if !self.frame.live.visible() || self.time_scale <= 0.0 { return; }
        let wake = self.clock + (at - self.t).max(0.0) / self.time_scale as f64;
        if self.motion.loop_wake.is_none_or(|w| wake < w.0) { self.motion.loop_wake = Some((wake, at)); }
    }

    /// Wakes for one frame at clock time `due` and draws nothing before it, as a delayed start
    /// does: an instant scroll's hover hold asks for the frame it lets go on this way, so a
    /// resting window draws no frame while the hold waits.
    pub(in crate::runtime) fn wake_for_clock(&mut self, due: f64) {
        let at = self.t + (due - self.clock).max(0.0) * self.time_scale as f64;
        self.wake_for_motion(at);
    }

    pub(in crate::runtime) fn catch_up_motion(&mut self, dt: f64) {
        let step = dt * self.time_scale as f64;
        self.t += step;
        self.scene.set_time(self.t);
        self.motion.caught_up += step as f32;
        self.step_growth(step as f32);
        self.step_scroll_by(step as f32, true);
    }

    pub(in crate::runtime) fn step_motion(&mut self, app: &dyn ViewDriver, dt: f64, instant_scroll: bool) -> bool {
        self.loops_stepped();
        let step = dt * self.time_scale as f64;
        self.motion.slack_dt = std::mem::take(&mut self.motion.caught_up) + step as f32;
        self.frame.last_dt = step;
        self.t += step;
        self.scene.set_time(self.t);
        if self.scene.tracing() {
            for node in &self.tree.nodes {
                self.scene.trace_name(node.id, node.trace_name);
                self.scene.trace_namespace(node.id, node.trace_namespace);
            }
        }
        for (id, track) in &self.motion.loops {
            if let Some([x, y]) = track.offset(self.scene.store().node_time(*id), self.reduced_motion)
                && let Some(n) = self.tree.node_index.get(id).map(|&i| &self.tree.nodes[i])
            {
                let offset = rune_motion::Vec2::new(x * n.local.w, y * n.local.h);
                if self.scene.get(*id, prop::OFFSET) != Some(offset) {
                    self.scene.set(*id, prop::OFFSET, offset);
                }
            }
        }
        self.end_unseen_exits();
        let out = self.scene.frame();
        let (mut animating, exited) = (out.animating, !out.exited.is_empty());
        self.paint.keep.frame += 1;
        // Tracks that only wait for a delayed start (`El::enter_delay`, a delayed `Anim`)
        // draw nothing new until it: sleep, and wake for it after the build.
        let mut waits = if animating { self.scene.store().waiting_until() } else { None };
        if waits.is_some() { animating = false; }
        let mut sources = if animating { AnimSource::TRACKS } else { 0 };
        if exited {
            self.prune_exited();
        }
        self.land_group_fades();
        if !self.motion.flying.is_empty() {
            let scene = &self.scene;
            let before = self.motion.flying.len();
            // Screen travel ends once it lands and no ancestor still moves. An exiting flight
            // stays lifted until it's gone.
            self.motion.flying.retain(|(id, _)| scene.store().has(*id, prop::TRAVEL)
                || matches!(scene.phase(*id), Some(PresencePhase::Exiting { .. })));
            if self.motion.flying.len() != before {
                self.motion.launches.keep(&self.motion.flying);
                self.update_layer_paint_order();
            }
        }
        let mut source = |on: bool, bit: u16| { if on { sources |= bit; } on };
        animating |= source(self.step_growth(step as f32), AnimSource::GROWTH);
        animating |= source(self.step_scroll(step as f32), AnimSource::SCROLL);
        // A hold draws nothing while it waits: its wake asks for the one frame that ends it.
        let hold_wake = self.step_nudges();
        for stack in self.motion.stacks.values_mut() { stack.grown = 0.0; }
        animating |= source(self.reveals_active(), AnimSource::REVEAL);
        // Values set from `on_scroll_offset` show in the frame that scrolled.
        if self.tell_scroll_offsets(instant_scroll) {
            let took = self.take_live_wake();
            let live = self.latch_live(took);
            if live { sources |= AnimSource::LIVE; }
            let running = self.scene.store().running();
            waits = running.then(|| self.scene.store().waiting_until()).flatten();
            if running && waits.is_none() { sources |= AnimSource::TRACKS; } else { sources &= !AnimSource::TRACKS; }
            animating = sources != 0;
            self.scene.present();
        }
        self.paint.keep.moved.extend(self.scene.changed().iter().map(|p| p.node));
        if self.fill_text() { animating = true; sources |= AnimSource::TEXT_FILL; }
        let mut source = |on: bool, bit: u16| { if on { sources |= bit; } on };
        self.begin_exit_mask();
        animating |= self.build();
        // Compensate drawn motion, leaving unrelated layout and growth springs alone.
        for _ in 0..3 {
            if !self.follow_keeps() { break; }
            animating |= self.build();
        }
        self.finish_keeps();
        animating |= self.write_drawn_rects();
        // A selection's edge scroll stepped: it follows the pointer over this layout, and shows.
        if self.laid_edge_scroll(app) { animating |= self.build(); }
        self.report_at_bottom();
        animating |= self.report_visible_rows();
        self.report_layout();
        // A report waits for the next frame: draw one to deliver it.
        animating |= self.report_drawn() | self.report_visible();
        self.end_exit_mask();
        self.record_rewind();
        animating |= source(self.motion.loops_active, AnimSource::LOOP);
        animating |= source(self.paint.scale_frame.is_some() || self.paint.pixel_snap.easing(), AnimSource::SCALE_SETTLE);
        animating |= source(self.paint.videos_active, AnimSource::VIDEO);
        animating |= source(self.draw_text_flights(), AnimSource::TEXT_FLIGHT);
        animating |= source(self.text_fill_due(), AnimSource::TEXT_FILL);
        if !animating && raster_check::on() {
            self.check_resting_raster();
        }
        if let Some(at) = waits { self.wake_for_motion(at); }
        // An instant scroll's hold draws nothing until it ends: wake for the one frame that
        // resolves hover then, taken from the state each frame so a re-arm moves it on.
        if let Some(due) = hold_wake { self.wake_for_clock(due); }
        self.images.end_frame();
        if self.track_marks {
            self.marks.resize(self.instances.len(), 0);
        }
        self.motion.anim_sources = sources;
        animating
    }

    /// Subtree (preorder) paint order, then each flying element's subtree on top. A sticky
    /// element paints after its normal siblings (and before overlay siblings), so it covers the
    /// rows it's stuck over.
    pub(in crate::runtime) fn update_layer_paint_order(&mut self) {
        let n = self.tree.nodes.len();
        let mut first = vec![n; n + 1];
        let mut next = vec![n; n];
        // Siblings paint by `El::paint_order`, then (with sticky elements about) sticky ones over
        // the rows they're stuck over and anchored overlays and modals over both.
        let rank = |i: usize| {
            let nd = &self.tree.nodes[i];
            let place = if !self.tree.has_sticky { 0 }
                else if nd.above.is_some() || nd.handlers.modal.is_some() { 2 }
                else if self.sticky_of(i).is_some() { 1 }
                else { 0 };
            (nd.vis.paint_order, place)
        };
        if self.tree.has_sticky || self.tree.has_paint_order {
            let mut ranks: Vec<_> = (0..n).map(|i| (rank(i), i)).collect();
            ranks.sort_unstable();
            for &(_, i) in ranks.iter().rev() {
                let parent = self.tree.nodes[i].parent.map_or(0, |p| p + 1);
                next[i] = first[parent]; first[parent] = i;
            }
        } else {
            for i in (0..n).rev() {
                let parent = self.tree.nodes[i].parent.map_or(0, |p| p + 1);
                next[i] = first[parent]; first[parent] = i;
            }
        }
        let mut lifted: Vec<usize> = self.motion.flying.iter().filter_map(|(id, _)| self.tree.node_index.get(id).copied()).collect();
        if self.tree.has_raise { lifted.extend((0..n).filter(|&i| self.tree.nodes[i].vis_flags.raise())); }
        lifted.sort_unstable();
        lifted.dedup();
        self.paint.layer_paint_order.clear();
        for top in std::iter::once(None).chain(lifted.iter().map(|&r| Some(r))) {
            let mut i = top.unwrap_or(first[0]);
            while i < n {
                // A flying subtree paints in its own pass, not where the tree reaches it.
                if top == Some(i) || lifted.binary_search(&i).is_err() {
                    self.paint.layer_paint_order.push(i);
                    if first[i + 1] < n { i = first[i + 1]; continue; }
                }
                loop {
                    if top == Some(i) { i = n; break; }
                    if next[i] < n { i = next[i]; break; }
                    match self.tree.nodes[i].parent { Some(parent) => i = parent, None => { i = n; break; } }
                }
            }
        }
        if self.paint.layer_paint_order.iter().copied().eq(0..n) { self.paint.layer_paint_order.clear(); }
    }
}
