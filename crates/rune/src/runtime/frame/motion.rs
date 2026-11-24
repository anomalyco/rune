//! Per-frame motion stepping, delayed-motion wakes, loop visibility and layer paint order.
use super::*;

impl RuntimeCore {
    /// frame-profile: the paths of drawn nodes whose loop is running, which wake the window.
    #[cfg(feature = "frame-profile")]
    pub(crate) fn fp_loops(&self) -> Vec<String> {
        self.tree.nodes.iter().enumerate()
            .filter(|(i, n)| n.looping.is_some() && self.paint.was_seen(*i, false) && self.motion.has_loop(n.id))
            .map(|(_, n)| n.path.clone()).collect()
    }

    /// Whether a drawn node's loop runs (a spinner or animated SVG icon): what a window shows
    /// would fall behind it while the window draws nothing.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn loops_shown(&self) -> bool {
        (!self.reduced_motion && self.paint.animated_icons_shown())
            || self.tree.nodes.iter().enumerate().any(|(i, n)| n.looping.is_some() && self.paint.was_seen(i, false) && self.motion.loop_active(n.id, self.scene.store().node_time(n.id)))
    }

    /// Wakes when a delayed start at animation time `at` comes due, on the loops' wake: the
    /// frame then runs at exactly that time, however long the host slept. A hidden window
    /// sleeps through it; showing it settles.
    fn wake_for_motion(&mut self, at: f64) {
        self.paint.note_cycle_other_deadline();
        if !self.frame.live.visible() || self.time_scale <= 0.0 { return; }
        let wake = self.clock + (at - self.t).max(0.0) / self.time_scale as f64;
        self.motion.arm_loop_wake(wake, at);
    }

    /// Wakes for a paced animated SVG icon without marking `cycles.mixed`, so icon-only wakes take
    /// the zero-paint-walk `loop_step` / `arm_loop` fast path.
    fn wake_for_icon_anim(&mut self, at: f64) {
        if !self.frame.live.visible() || self.time_scale <= 0.0 { return; }
        let wake = self.clock + (at - self.t).max(0.0) / self.time_scale as f64;
        self.motion.arm_loop_wake(wake, at);
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
        self.motion.add_caught_up(step as f32);
        self.step_growth(step as f32);
        self.step_scroll_by(step as f32, true);
    }

    pub(in crate::runtime) fn step_motion(&mut self, app: &dyn ViewDriver, dt: f64, instant_scroll: bool) -> bool {
        self.motion.loops_stepped();
        let step = dt * self.time_scale as f64;
        self.motion.begin_slack_step(step as f32);
        self.frame.set_last_dt(step);
        self.t += step;
        self.scene.set_time(self.t);
        if self.scene.tracing() {
            for node in &self.tree.nodes {
                self.scene.trace_name(node.id, node.trace_name);
                self.scene.trace_namespace(node.id, node.trace_namespace);
            }
        }
        self.motion.step_loops_into_scene(&self.tree.nodes, &self.tree.node_index, &mut self.scene, self.reduced_motion);
        self.end_unseen_exits();
        let out = self.scene.frame();
        let (mut animating, exited) = (out.animating, !out.exited.is_empty());
        self.paint.begin_frame();
        // Tracks that only wait for a delayed start (`El::enter_delay`, a delayed `Anim`)
        // draw nothing new until it: sleep, and wake for it after the build.
        let mut waits = if animating { self.scene.store().waiting_until() } else { None };
        if waits.is_some() { animating = false; }
        let mut sources = if animating { AnimSource::TRACKS } else { 0 };
        if exited {
            self.prune_exited();
        }
        land_group_fades(&mut self.scene, &mut self.tree.nodes, &self.tree.node_index);
        if self.motion.prune_landed_flights(&self.scene) {
            self.update_layer_paint_order();
        }
        let mut source = |on: bool, bit: u16| { if on { sources |= bit; } on };
        animating |= source(self.step_growth(step as f32), AnimSource::GROWTH);
        animating |= source(self.step_scroll(step as f32), AnimSource::SCROLL);
        // A hold draws nothing while it waits: its wake asks for the one frame that ends it.
        let hold_wake = self.step_nudges();
        for stack in self.motion.stacks.values_mut() { stack.grown = 0.0; }
        animating |= source(self.motion.reveals_active(&self.scene), AnimSource::REVEAL);
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
        self.paint.record_moved(self.scene.changed().iter().map(|p| p.node));
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
        self.paint.drawn_rect_wrote = self.write_drawn_rects();
        animating |= self.paint.drawn_rect_wrote;
        // A selection's edge scroll stepped: it follows the pointer over this layout, and shows.
        let edge = self.laid_edge_scroll(app);
        if edge.repaint { animating |= self.build(); }
        animating |= edge.queued;
        self.report_at_bottom();
        animating |= self.report_visible_rows();
        self.report_layout();
        // A report waits for the next frame: draw one to deliver it.
        animating |= self.report_drawn() | self.report_visible();
        self.end_exit_mask();
        self.motion.record_rewind(self.clock, self.viewport, &self.tree.nodes, &self.paint);
        animating |= source(self.motion.loops_active(), AnimSource::LOOP);
        animating |= source(self.paint.scale_settling(), AnimSource::SCALE_SETTLE);
        animating |= source(self.paint.videos_active(), AnimSource::VIDEO);
        animating |= source(self.paint.animated_icons(), AnimSource::ICON_ANIM);
        let flights_active = self.paint.draw_text_flights(
            self.motion.flights_mut(), &self.tree.nodes, &self.scene, &mut self.text, &mut self.images,
            &mut self.instances, self.env.scale, self.t, self.reduced_motion, self.viewport,
        );
        animating |= source(flights_active, AnimSource::TEXT_FLIGHT);
        animating |= source(self.text_fill_due(), AnimSource::TEXT_FILL);
        if !animating && raster_check::on() {
            self.check_resting_raster();
        }
        if let Some(at) = waits { self.wake_for_motion(at); }
        if let Some(at) = self.paint.icon_anim_wake() { self.wake_for_icon_anim(at); }
        // An instant scroll's hold draws nothing until it ends: wake for the one frame that
        // resolves hover then, taken from the state each frame so a re-arm moves it on.
        if let Some(due) = hold_wake { self.wake_for_clock(due); }
        self.images.end_frame();
        if self.track_marks {
            self.marks.resize(self.instances.len(), 0);
        }
        self.motion.set_anim_sources(sources);
        animating
    }

    /// Subtree (preorder) paint order, then each flying element's subtree on top. A sticky
    /// element paints after its normal siblings (and before overlay siblings), so it covers the
    /// rows it's stuck over.
    #[inline]
    pub(in crate::runtime) fn update_layer_paint_order(&mut self) {
        let Self { paint, tree, motion, scroll, scene, .. } = self;
        paint.update_layer_paint_order(
            tree,
            &motion.flying,
            |id| motion.launches().launched(id),
            |i| {
                (scroll.requests.has_keeps() && scroll.requests.keeps().any(|k| k.hold.as_ref().is_some_and(|h| h.path == tree.nodes[i].path)))
                    || tree.nodes[i].sticky.as_ref().and_then(|s| s.edge).is_some()
            },
            |i| std::iter::successors(Some(i), |&p| tree.nodes[p].parent).any(|p| matches!(scene.phase(tree.nodes[p].id), Some(PresencePhase::Exiting { .. }))),
        );
    }
}
