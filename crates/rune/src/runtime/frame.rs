//! Frame loop, clocks, wake deadlines, request latching and motion/paint-order stepping.
use super::*;

mod latch;
mod motion;
mod wakes;

impl RuntimeCore {
    /// The last committed frame is out of date: state changed (an update, poll or handler), or
    /// one asked for a scroll or text flight that waits for the next frame.
    pub(crate) fn needs_commit(&self) -> bool {
        self.commit.dirty || self.frame.pending.frame_work() || self.patch_pending()
    }

    /// The last frame no longer shows the window: state, its size or queued input changed since.
    pub(crate) fn stale(&self) -> bool {
        self.needs_commit() || self.commit.relayout || !self.frame.queue.is_empty()
    }

    /// A live-edit patch arrived that the next frame hasn't rebuilt the view with yet.
    pub(crate) fn patch_pending(&self) -> bool {
        self.frame.hot_pending()
    }

    /// Marks app state changed outside a handler (the next frame re-runs the view).
    pub fn invalidate(&mut self) {
        self.commit.dirty = true;
    }

    /// Lands every track before the next frame: growth, reveals and follow-the-end (guarantee 8,
    /// for returning to a window or session).
    pub fn settle_all(&mut self) {
        self.commit.request_settle();
        self.commit.dirty = true;
    }

    /// Advances animation time by `dt` real seconds, runs queued handlers, re-runs the view only
    /// if state changed, steps `Scene`, growth, scroll and reveals, and builds this frame's
    /// instances.
    pub(crate) fn frame(&mut self, app: &mut dyn AppDriver, dt: f64) -> bool {
        self.frame_split(app, 0.0, dt)
    }

    /// A hidden or windowless host's due timer.
    pub(crate) fn hidden_timer_frame(&mut self, app: &mut dyn AppDriver) {
        self.frame(app, 0.0);
    }

    /// Builds what changed while a window that fits its content was hidden, before the host
    /// shows it, so the content's height (`window_frame`, a jump) is known before the window
    /// is ordered in and its first drawable taken. Lands every track, as showing does.
    /// Returns whether it built; the first frame then has nothing left to build.
    pub(crate) fn build_for_show(&mut self, app: &mut dyn AppDriver) -> bool {
        if !self.host.fit_height || self.commit.commits == 0 || !self.stale() { return false; }
        self.settle_all();
        self.hidden_timer_frame(app);
        true
    }

    /// A hidden or covered host's due timer or wake. `poll` and due timers run now; the view
    /// and layout wait, dirty, for the frame that shows the host. Queued input and drag ends take a frame
    /// without drawing (`hidden_timer_frame`) so their handlers hear them now. Returns whether
    /// the last drawn frame went out of date.
    pub(crate) fn hidden_wake(&mut self, app: &mut dyn AppDriver) -> bool {
        if self.has_pending_input() || self.drag_ends_pending() {
            let commits = self.stats.commits;
            self.hidden_timer_frame(app);
            // File steps held for that frame's build land in another.
            if self.files_held() { self.hidden_timer_frame(app); }
            return self.stats.commits != commits;
        }
        self.poll_hidden(app)
    }

    /// [`Runtime::hidden_wake`] for a shown window that can't draw (covered, minimised, on a
    /// locked or sleeping display). While an assistive client reads its tree, what changed is
    /// built at once, without drawing or motion, so the tree it reads and focus asked of what
    /// the change shows (a permission's card) are current whatever the window's motion does.
    /// Returns what `hidden_wake` does.
    pub(crate) fn covered_wake(&mut self, app: &mut dyn AppDriver) -> bool {
        let stale = self.hidden_wake(app);
        if self.host.a11y.is_some() && self.needs_commit() { self.hidden_timer_frame(app); }
        stale
    }

    /// One frame in one pass that steps `catch_up + dt` seconds. Motion already running advances
    /// `catch_up` before this frame's handlers and commit, so they retarget it from its pose at
    /// the frame's time. Then everything, including what the commit started, steps `dt`. A late
    /// frame or a sleeping host's gap belongs to old tracks, never to targets first observed
    /// on this frame.
    pub(crate) fn frame_split(&mut self, app: &mut dyn AppDriver, catch_up: f64, dt: f64) -> bool {
        // Like a native shell: the logical clock follows host time (timers, poll), while
        // animations step by the frame's own dt, so idle time never lands on a fresh commit.
        let elapsed = catch_up + dt;
        // A coarse headless step still polls at its timer deadlines, independent of FPS, as an
        // idle window wakes for them. Like a window's timer floor (`window::timer_deadline`),
        // a poll comes at most once a frame interval and takes every deadline due by then, so
        // wakes that polls keep asking for collapse instead of each adding a poll per frame.
        // Input queued for the requested frame stays at that frame's timestamp. The catch-up
        // is the earliest part of the step.
        let target = self.clock + elapsed;
        if self.wall_start.is_none() && self.timer_step(target).is_some() {
            let input = std::mem::take(&mut self.frame.queue);
            let mut catch_up = catch_up;
            while let Some(at) = self.timer_step(target) {
                let step = at - self.clock;
                let old = step.min(catch_up);
                catch_up -= old;
                self.frame_step(app, step, old, step - old);
            }
            self.frame.queue.extend(input);
            let step = target - self.clock;
            let old = step.min(catch_up);
            return self.frame_step(app, step, old, step - old);
        }
        self.frame_step(app, elapsed, catch_up, dt)
    }

    /// When a headless step toward `target` stops early for a timer: the next deadline, but no
    /// sooner than a frame interval after the last poll. None when the step's own poll at
    /// `target` takes everything due.
    pub(super) fn timer_step(&self, target: f64) -> Option<f64> {
        let deadline = self.logical_wake_deadline()?;
        let at = deadline.max(self.clock + crate::window::TIMER_FRAME.as_secs_f64());
        (at < target).then_some(at)
    }

    /// `elapsed` advances the logical clock when no wall clock drives it; `catch_up` steps
    /// running motion before the commit and `dt` steps all motion after it.
    pub(super) fn frame_step(&mut self, app: &mut dyn AppDriver, elapsed: f64, catch_up: f64, dt: f64) -> bool {
        // Each queued action runs with its own input's arrival time; nothing else has one.
        let _input = crate::input::time::scope(None);
        self.interaction_frame_begins();
        let mut live_woke = self.take_live_wake();
        // Another window may have switched the font families since this one's last frame.
        self.follow_font_families();
        for v in self.commit.virtuals.values_mut() { v.frame_begins(); }
        #[cfg(feature = "hot")]
        if self.frame.hot_changed() {
            // Replace handler trait objects before queued input can call an old vtable.
            // A regular commit preserves keyed nodes, scrolls and spring velocities.
            self.commit.clear_memo();
            self.commit(app, &Tx::default());
        }
        let mut tx = std::mem::take(&mut self.frame.pending);
        let mut ran = false;
        let mut why = if tx.polled_change && self.commit.dirty { CommitWhy::POLL } else { 0 };
        #[cfg(debug_assertions)]
        let commits0 = self.commit.commits;
        let shaped0 = self.text.shaped;
        self.settled = false;
        self.motion.clear_landing_holds();
        // `RUNE_LOOP_CHECK=1`: the frame that runs instead of a checked step samples at its clock.
        self.clock = self.paint.take_cycle_check_clock().unwrap_or_else(|| self.wall_clock().unwrap_or(self.clock + elapsed));
        if self.wall_start.is_none() {
            crate::video::headless_frame(self.clock);
            // Fake recorders publish this frame's level to bound meters, drawn this frame.
            crate::recorder::headless_frame(self.clock);
            live_woke |= self.take_live_wake();
        }
        if let Some((deadline, animation_time)) = self.motion.loop_wake()
            && !self.animating
        {
            // Native shells cap dt after idle. Advance the sleeping part of the
            // delay before new handlers retarget, without double-stepping manual time.
            let elapsed = animation_time - (deadline - self.clock).max(0.0) * self.time_scale as f64;
            self.t += (elapsed - self.t - (catch_up + dt) * self.time_scale as f64).max(0.0);
            self.scene.set_time(self.t);
            if self.clock >= deadline { self.motion.clear_loop_wake(); }
        }
        // Bring old tracks, growth and scroll to this frame's time before a poll or update
        // retargets them. Presenting waits for `step_motion`, so the frame is sampled and
        // built once.
        if catch_up > 0.0 { self.catch_up_motion(catch_up); }
        self.motion.keep_loops_on_clock(&self.scene, self.t, self.clock, self.time_scale, dt);
        let (polled, due) = self.frame.begin_frame_poll(self.clock);
        self.frame.idle_frame_begins(self.animating, polled || due > 0, live_woke);
        self.frame.drain_due_wakes(due);
        self.fire_hover_delays();
        self.fire_press_repeat();
        self.fire_edge_scroll();
        self.fire_autoscroll();
        self.fire_touch_hold(app);
        // Marks are paint's (`paint::content`), and this frame paints: a pause that ends or words
        // the checker brought back show without the view running again.
        let (wake, _marked) = crate::spell::pause::tick(&mut self.edit.spell_watch, self.clock);
        if let Some(at) = wake { self.schedule_wakes([std::time::Duration::from_secs_f64((at - self.clock).max(0.0))]); }
        if !polled && app.has_poll() {
            let mut one = self.transaction(app);
            one.input_waiting = self.frame.queue.has_input();
            // Scheduling does not imply a model change.
            if self.frame.record_poll(crate::tooling::perf_warn::timed("poll", || app.poll(self.clock, &mut one))) {
                ran = true;
                why |= CommitWhy::POLL;
                self.commit.watch.meter.poll();
            }
            merge(&mut tx, one);
        }
        if app.playground().is_some_and(|p| p.polls()) {
            let mut one = self.transaction(app);
            let clock = self.clock;
            if self.poll_playground(app, clock, &mut one) {
                ran = true;
                why |= CommitWhy::POLL;
            }
            merge(&mut tx, one);
        }
        if self.commit.commits == 0 && self.frame.queue.iter().any(|a| matches!(a, Action::WindowFocus(_))) {
            self.commit(app, &Tx::default());
        }
        let mut ran_layout = false;
        self.report_visible_rows();
        // Hosts take a drag-out inside the move that asked for it; by now nothing will.
        self.drop_drag_out_request();
        let ended: Vec<_> = self.pointer.drag_ends.borrow_mut().drain(..).collect();
        let queue = order_drag_ends(std::mem::take(&mut self.frame.queue), ended, &mut self.pointer.drags_open);
        let queue = self.hold_files(queue, ran || self.commit.dirty);
        // A host settle (an uncover, a restore, a shown window) lands the tree the frame began
        // with, before this frame's input runs. The input's own commit below then animates, so
        // a dialog the first press closes still plays its exit.
        if self.commit.is_settling() && queue.has_input_action() {
            self.settle_before_input(app, &mut tx, &mut why);
            ran = false;
        }
        // Something besides a checked live-only event wants this frame's commit, so a change
        // it makes isn't the live-only handler's.
        #[cfg(debug_assertions)]
        let mut ran_other = ran;
        let mut pointer_hover = false;
        for Queued { action, at, input } in queue {
            if matches!(action, Action::Key { stage: false, .. } | Action::Physical(crate::PhysicalInput::Key { pressed: true, .. }, _)) { crate::marks::input_handled(true); }
            pointer_hover |= matches!(action, Action::Hover(_));
            // The user's own key or press takes over from a `Tx::focus_when_shown` still waiting.
            if matches!(action, Action::Key { stage: false, .. } | Action::Physical(crate::PhysicalInput::Key { pressed: true, .. }, _) | Action::Press(..) | Action::Focus(..)) {
                self.kbd.focus_owed = None;
            }
            // State may have requested focus on a newly mounted field. Resolve the
            // new tree before routing this frame's text, not after it has gone to
            // the previous field. The same boundary covers an earlier handler, and lets a
            // file drag find only what is mounted and reachable now (a covered window builds
            // nothing until this).
            let routed = matches!(&action, Action::Key { .. } | Action::KeyUp(_) | Action::Physical(..) | Action::Ime(_) | Action::FinishIme | Action::Paste(..) | Action::Files(_));
            // So is a playground an earlier handler opened or moved.
            if routed { self.latch_playground(app, &mut tx); }
            // Both callbacks see ordinary &mut S. After the next mutation a single view
            // cannot tell which one introduced a row. Observe the poll's tree now, without
            // drawing it or advancing time. A key already needed this same routing boundary.
            let split = self.motion.motion_needs_cause() && why & CommitWhy::POLL != 0 && (input || routed);
            if split || (ran || self.commit.dirty || !tx.scroll_to.is_empty()) && routed {
                if split {
                    let links = std::mem::take(&mut tx.open_urls);
                    self.latch_requests(app, &mut tx);
                    tx.open_urls = links;
                }
                if ran || self.commit.dirty {
                    self.motion.capture_flights(&self.tree.nodes, &mut self.paint, &mut self.text, self.env.scale, self.viewport, self.t, &mut tx);
                    if self.commit.is_settling() { tx.settle(); }
                    self.settled |= tx.commit.lands();
                    #[cfg(debug_assertions)]
                    if ran_other || self.commit.dirty || ran_layout { self.frame.clear_live_check(); }
                    self.capture_keeps(&mut tx);
                    tx.unprompted = self.motion.motion_needs_cause() && why & CommitWhy::INPUT == 0 && !tx.asked_motion;
                    if split && tx.unprompted { tx.commit = tx.commit.without_input(); }
                    if routed { why |= CommitWhy::ROUTE; }
                    if self.commit.dirty { why |= CommitWhy::DIRTY; }
                    self.commit_for(app, &tx, std::mem::take(&mut why));
                    #[cfg(debug_assertions)]
                    { let check = self.frame.take_live_check(); self.compare_live_check(check); ran_other = false; }
                    self.apply_keeps(app);
                    ran = false;
                }
                if split {
                    self.finish_scroll_requests(app, &mut tx, false);
                    // The poll's motion policy belongs to the tree just applied, not to
                    // the handler about to run. Shell requests and timers still go out once.
                    tx.commit = Commit::Animate;
                    tx.unprompted = false;
                    tx.asked_motion = false;
                    tx.live_growth.clear(); tx.tail_rooms.clear();
                    tx.motion = None;
                    tx.color_motion = None;
                    tx.follow_edge = false;
                    tx.unclipped_flights = false;
                    ran_layout = false;
                } else if !tx.scroll_to.is_empty() {
                    self.drop_scroll_holds();
                    for r in tx.scroll_to.drain(..) { self.reveal_row(app, &r, tx.commit.snaps()); }
                }
            }
            let _input = crate::input::time::scope(at);
            // A selection an earlier handler asked for is what this key copies.
            if matches!(action, Action::Key { .. } | Action::Physical(..)) && let Some(request) = tx.select.take() {
                let from_change = std::mem::take(&mut tx.select_from_change);
                self.apply_select_request(app, request, from_change);
            }
            if matches!(action, Action::Layout(..) | Action::ScreenLayout(..) | Action::DrawnLayout(_, _, false) | Action::VisibleRows(..)) {
                ran_layout |= self.run_action(app, action, &mut tx);
            } else {
                #[cfg(debug_assertions)]
                let checks = self.frame.live_checks();
                let r = self.run_action(app, action, &mut tx);
                #[cfg(debug_assertions)]
                { ran_other |= r && checks == self.frame.live_checks(); }
                ran |= r;
                if r { why |= if input { CommitWhy::INPUT } else { CommitWhy::DIRTY }; }
            }
            self.run_pick_reads(app, &mut tx);
        }
        // Once a frame, however often a drag moved the selection, and in time for this commit.
        for action in self.selection_changes(app) {
            #[cfg(debug_assertions)]
            let checks = self.frame.live_checks();
            let r = self.run_action(app, action, &mut tx);
            #[cfg(debug_assertions)]
            { ran_other |= r && checks == self.frame.live_checks(); }
            if r { ran = true; why |= CommitWhy::DIRTY; }
        }
        #[cfg(target_os = "linux")]
        self.note_primary_selection(app);
        let mut open_urls = std::mem::take(&mut tx.open_urls);
        self.latch_requests(app, &mut tx);
        // A handler asked for a menu: the hover it takes ends in this commit, not a frame later.
        if self.release_pointer_for_popup() {
            for queued in std::mem::take(&mut self.frame.queue) {
                if !matches!(queued.action, Action::HoverChange(..)) { self.frame.queue.push_queued(queued); continue; }
                #[cfg(debug_assertions)]
                let checks = self.frame.live_checks();
                let r = self.run_action(app, queued.action, &mut tx);
                #[cfg(debug_assertions)]
                { ran_other |= r && checks == self.frame.live_checks(); }
                if r { ran = true; why |= if queued.input { CommitWhy::INPUT } else { CommitWhy::DIRTY }; }
            }
            open_urls.extend(std::mem::take(&mut tx.open_urls));
            self.latch_requests(app, &mut tx);
        }
        self.release_files();
        self.schedule_wakes(tx.wakes.drain(..));
        self.motion.capture_flights(&self.tree.nodes, &mut self.paint, &mut self.text, self.env.scale, self.viewport, self.t, &mut tx);
        if self.edit.caret_watch && self.caret_moved(None) { self.commit.dirty = true; }
        let read_moved = self.layout_reads_moved();
        self.commit.set_read_moved(read_moved);
        if self.tree.has_pixel_aspect && self.pixel_aspect_changed() { self.commit.relayout = true; }
        let settling = self.commit.is_settling();
        let shifted = !(self.commit.dirty || self.commit.relayout || settling) && self.virtual_window_changed();
        let besides = self.commit.dirty || self.commit.relayout || settling || shifted;
        let other = ran || besides;
        self.count_layout_echo(ran_layout || read_moved, other);
        if other || ran_layout || read_moved {
            if self.commit.relayout && !ran && !self.commit.dirty {
                if self.commit.is_viewport_stepped() { tx.window_step = true } else { tx.relayout = true }
            }
            if settling {
                tx.commit = Commit::Settle;
            }
            self.settled = tx.commit.lands();
            self.commit.set_state_unchanged(!ran && !self.commit.dirty && !ran_layout && !settling && self.commit.commits > 0);
            #[cfg(debug_assertions)]
            if ran_other || besides || ran_layout || read_moved { self.frame.clear_live_check(); }
            self.capture_keeps(&mut tx);
            let flag = |on: bool, bit: u16| if on { bit } else { 0 };
            // Motion has a cause: the user, in this window. A commit nothing in this frame's
            // input asked for shows what it brings in its final state (`Tx::unprompted`).
            tx.unprompted = self.motion.motion_needs_cause() && why & CommitWhy::INPUT == 0 && !tx.asked_motion;
            // Its layout lands too, as an instant change's does: a container that grows to fit what a
            // server brought doesn't ease to its height (motion already running keeps on).
            if tx.unprompted { tx.commit = tx.commit.without_input(); }
            self.commit_for(app, &tx, why | flag(self.commit.dirty, CommitWhy::DIRTY) | flag(self.commit.relayout, CommitWhy::RELAYOUT) | flag(settling, CommitWhy::SETTLE)
                | flag(shifted, CommitWhy::VIRTUAL) | flag(ran_layout, CommitWhy::LAYOUT) | flag(read_moved, CommitWhy::READ));
            #[cfg(debug_assertions)]
            { let check = self.frame.take_live_check(); self.compare_live_check(check); }
            self.apply_keeps(app);
        }
        #[cfg(debug_assertions)]
        { self.frame.clear_live_check(); }
        tx.keep_on_screen.clear();
        for (path, v) in &mut self.commit.virtuals {
            if let Some(key) = v.reveal_selected.take() && !pointer_hover {
                // Use the keyed ancestor path so identical row keys in other lists do not
                // intercept the reveal, including when this row is still virtual.
                let scope = path.split('/').filter(|seg| !seg.starts_with('#')).collect::<Vec<_>>().join("/");
                let key = if scope.is_empty() { key } else { format!("{scope}/{key}") };
                tx.scroll_to.push(ScrollReveal::into_view(None, key));
            }
        }
        #[cfg(debug_assertions)]
        {
            let scrolling = self.scrolling();
            self.commit.watch_frame(commits0, scrolling, tx.scroll_offsets.iter().find(|r| tx.commit.snaps() || r.glide.is_none()).map(|r| r.key.as_str()));
        }
        let follow = std::mem::take(&mut self.edit.caret_follow);
        self.finish_scroll_requests(app, &mut tx, follow);
        // Those scrolls moved the caret after the commit a view placed from it saw.
        if self.edit.caret_watch && self.caret_moved(None) { self.commit(app, &Tx { commit: Commit::Instant, ..Tx::default() }); }
        if let Some(request) = tx.select.take() {
            let from_change = std::mem::take(&mut tx.select_from_change);
            self.apply_select_request(app, request, from_change);
        }
        // What the commit or a `Tx::select_text` changed is heard next frame.
        let changes = self.selection_changes(app);
        self.frame.queue.extend(changes);
        self.update_interaction(None);
        #[cfg(test)]
        if let Some(publish) = tests::MID_FRAME.with(|hook| hook.borrow_mut().take()) { publish(); }
        // Poll and input can publish after the initial wake read without committing. Sample
        // those values now, but retain the wake: a picture may still need next frame's layout.
        let live_pending = live_woke || self.live_pending();
        let live_animating = self.latch_live(live_pending);
        if let Some(trace) = tx.trace.take() {
            for event in trace.events { self.trace_event(event); }
            for (name, kind, value) in trace.channels { self.trace_channel(name, kind, value); }
        }
        let dt = if self.fit_frame_holds() { 0.0 } else { dt };
        let instant_scroll = tx.commit.snaps() || self.settled;
        let animating = self.step_motion(app, dt, instant_scroll) || live_animating;
        self.stats.shaped = self.text.shaped - shaped0;
        let filling = self.virtual_filling();
        if live_animating { self.motion.add_anim_sources(AnimSource::LIVE); }
        if self.time_scale <= 0.0 { self.motion.set_anim_sources(0); }
        if filling { self.motion.add_anim_sources(AnimSource::VIRTUAL_FILL); }
        self.animating = (animating && self.time_scale > 0.0) || filling;
        #[cfg(debug_assertions)]
        {
            let clear = self.clear_color();
            let opaque = !self.surfaces.is_empty() || !self.paint.materials().is_empty() || self.paint.videos_active() || self.paint.animated_icons();
            let title = self.host.window_title.current();
            self.frame.idle_frame_ends(self.clock, self.animating, clear, self.viewport, self.env.scale, self.paint.srgb_blend(), self.transparent, self.images.copies, &self.instances, opaque, title);
        }
        self.interaction_built();
        if !open_urls.is_empty() { self.open_urls(open_urls); }
        self.animating
    }

    /// Lands a host settle for the tree as it stands before this frame's input, then lets the
    /// input's own commit animate what it changes. `apply` clears the settle latch as this
    /// commit applies it: the settle is spent here, so nothing is deferred or dropped.
    fn settle_before_input(&mut self, app: &mut dyn AppDriver, tx: &mut Tx, why: &mut u16) {
        let links = std::mem::take(&mut tx.open_urls);
        self.latch_requests(app, tx);
        tx.open_urls = links;
        self.motion.capture_flights(&self.tree.nodes, &mut self.paint, &mut self.text, self.env.scale, self.viewport, self.t, tx);
        tx.settle();
        self.settled |= tx.commit.lands();
        self.capture_keeps(tx);
        // `settle_all` marks the tree dirty too, and this commit carries that change.
        self.commit_for(app, tx, *why | CommitWhy::SETTLE | CommitWhy::DIRTY);
        self.apply_keeps(app);
        // Scroll requests queued before the input resolve under the settle, as the poll split's
        // do, rather than gliding on the input's own animated commit.
        self.finish_scroll_requests(app, tx, false);
        // The input below starts from the settled tree, as it would on any other frame.
        tx.commit = Commit::Animate;
        tx.unprompted = false;
        tx.asked_motion = false;
        tx.live_growth.clear();
        tx.tail_rooms.clear();
        tx.motion = None;
        tx.color_motion = None;
        tx.follow_edge = false;
        tx.unclipped_flights = false;
        *why = 0;
    }

    /// A picture sizing a side from its image changed shape (decoded, or a stream resized).
    /// The next commit lays those pictures out at once, whatever else it animates.
    pub(super) fn pixel_aspect_changed(&mut self) -> bool {
        for n in &mut self.tree.nodes {
            if let Some(cold) = n.cold.get_mut_if_some()
                && let (Some(ratio), Some(pixels)) = (&mut cold.pixel_aspect, &n.vis.pixels)
            {
                let now = pixel_ratio(pixels);
                if now != *ratio { self.tree.aspect_snap.push(n.id); }
                *ratio = now;
            }
        }
        !self.tree.aspect_snap.is_empty()
    }
}
