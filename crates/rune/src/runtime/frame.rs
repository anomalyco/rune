//! Frame loop, clocks, wake deadlines, request latching and motion/paint-order stepping.
use super::*;

pub(in crate::runtime) mod drain;
mod latch;
mod motion;
mod mount;
mod wakes;

/// Side effects collected across a frame (`open_urls`, `open_paths`, `haptics`) and committed
/// once at the end of [`RuntimeCore::frame`] so they align with the presented frame.
#[derive(Default)]
pub(in crate::runtime) struct EndOfFrameRequests {
    pub open_urls: Vec<crate::open_url::Link>,
    pub open_paths: Vec<crate::open_path::PathRequest>,
    pub haptics: Vec<crate::Haptic>,
}

impl EndOfFrameRequests {
    pub(in crate::runtime) fn take_from(tx: &mut Tx) -> Self {
        Self {
            open_urls: std::mem::take(&mut tx.open_urls),
            open_paths: std::mem::take(&mut tx.open_paths),
            haptics: std::mem::take(&mut tx.haptics),
        }
    }

    pub(in crate::runtime) fn extend_from(&mut self, tx: &mut Tx) {
        self.open_urls.extend(std::mem::take(&mut tx.open_urls));
        self.open_paths.extend(std::mem::take(&mut tx.open_paths));
        self.haptics.extend(std::mem::take(&mut tx.haptics));
    }

    pub(in crate::runtime) fn extend(&mut self, other: Self) {
        self.open_urls.extend(other.open_urls);
        self.open_paths.extend(other.open_paths);
        self.haptics.extend(other.haptics);
    }

    pub(in crate::runtime) fn restore_into(self, tx: &mut Tx) {
        tx.open_urls.extend(self.open_urls);
        tx.open_paths.extend(self.open_paths);
        tx.haptics.extend(self.haptics);
    }
}

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

    /// Whether layout callbacks, a drawn-rect live write, or a pending commit/relayout are
    /// waiting for another pass. Used by headless `--settle` to stop early once layout settles.
    pub(crate) fn layout_settle_pending(&self) -> bool {
        self.needs_commit()
            || self.commit.relayout
            || self.paint.drawn_rect_wrote
            || self.mounts_pending()
            || self.frame.queue.iter().any(|a| matches!(
                a,
                Action::Layout(..)
                    | Action::ScreenLayout(..)
                    | Action::DrawnLayout(..)
                    | Action::Visible(..)
                    | Action::VisibleRows(..)
                    | Action::AtBottom(..)
                    | Action::FarFromBottom(..)
            ))
    }

    /// Runs up to `max_passes` zero-dt passes while [`Self::layout_settle_pending`] holds,
    /// returning the passes run and the latest animating flag.
    pub(crate) fn settle_layout_passes(&mut self, app: &mut dyn AppDriver, max_passes: usize) -> (usize, Option<bool>) {
        let mut animating = None;
        for pass in 0..max_passes {
            if !self.layout_settle_pending() {
                return (pass, animating);
            }
            animating = Some(self.frame(app, 0.0));
        }
        (max_passes, animating)
    }

    /// A live-edit patch arrived that the next frame hasn't rebuilt the view with yet.
    pub(crate) fn patch_pending(&self) -> bool {
        self.frame.hot_pending()
    }

    /// Marks app state changed outside a handler (the next frame re-runs the view).
    pub fn invalidate(&mut self) {
        self.commit.dirty = true;
    }

    /// Drops uploaded image atlas pages and every `Pixels` reference kept in the last committed
    /// tree and memo cache, marking the view dirty so the next frame rebuilds and re-uploads any
    /// image the view still shows.
    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn drop_images(&mut self) {
        let mut dropped = self.images.clear();
        let has_tree_pixels = self.tree.nodes.iter().any(|n| {
            n.vis.cold.get().is_some_and(|v| v.pixels.is_some())
                || n.text.as_ref().is_some_and(|t| t.cold.pills.iter().any(crate::text::PillChip::has_image))
        });
        if has_tree_pixels {
            // Clear memos first so `Cold` / `SharedBox` refcounts drop to 1 before mutating in place.
            self.commit.clear_memo();
            for node in &mut self.tree.nodes {
                if node.vis.cold.get().is_some_and(|v| v.pixels.is_some())
                    && let Some(vis) = node.vis.cold.get_mut_if_some()
                {
                    vis.pixels = None;
                }
                if node.text.as_ref().is_some_and(|t| t.cold.pills.iter().any(crate::text::PillChip::has_image))
                    && let Some(text) = node.text.as_mut()
                    && let Some(cold) = text.cold.get_mut_if_some()
                {
                    for pill in &mut cold.pills {
                        pill.image = None;
                        if let Some(bare) = pill.bare.as_mut() {
                            bare.image = None;
                        }
                    }
                }
            }
            dropped = true;
        }
        if dropped {
            self.commit.clear_memo();
            self.commit.dirty = true;
        }
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
        if !self.host.content_sizing.fits_any() || self.commit.commits == 0 || !self.stale() { return false; }
        self.settle_all();
        self.hidden_timer_frame(app);
        true
    }

    /// A hidden or covered host's due timer or wake. `poll` and due timers run now; the view
    /// and layout wait, dirty, for the frame that shows the host. Queued input and drag ends—
    /// whether already waiting at entry or queued by `poll` (such as `Tx::send_key`,
    /// `Play::press`, or a paste)—take a zero-dt frame without drawing (`hidden_timer_frame`)
    /// so their handlers hear them in this same wake with the poll's clock, then drain chained
    /// input through [`drain::drain_turn`]. Returns whether the last drawn frame went out of date.
    pub(crate) fn hidden_wake(&mut self, app: &mut dyn AppDriver) -> bool {
        let commits = self.stats.commits;
        let mut poll_stale = false;
        let wake_clock = if self.has_pending_input() || self.drag_ends_pending() {
            self.hidden_timer_frame(app);
            self.polled_at()
        } else {
            let (stale, pending) = self.poll_hidden_or_arm_follow_up(app);
            poll_stale = stale;
            if !pending {
                return poll_stale;
            }
            let polled = self.clock;
            self.hidden_timer_frame(app);
            polled
        };
        self.drain_hidden_input(app, wake_clock);
        poll_stale || self.stats.commits != commits
    }

    /// Drains chained user input, drag ends, or held file steps left queued after a non-drawable
    /// frame through [`drain::drain_turn`], preserving `wake_clock` without re-polling. When
    /// [`drain::MAX_POLL_ROUNDS`] is hit with work still waiting, schedules a continuation wake
    /// (paced by `TIMER_FRAME` on the host) instead of spinning.
    pub(crate) fn drain_hidden_input(&mut self, app: &mut dyn AppDriver, wake_clock: f64) {
        if self.frame.poll_capped() {
            return;
        }
        let outcome = drain::drain_turn(
            self,
            |rt| !rt.frame.poll_capped() && rt.hidden_follow_up_waiting(),
            |rt, _| {
                rt.frame.set_follow_up_poll(wake_clock);
                rt.hidden_timer_frame(app);
            },
        );
        if outcome.capped {
            self.schedule_wakes_from(wake_clock, [std::time::Duration::ZERO]);
        }
    }

    #[inline]
    fn hidden_follow_up_waiting(&self) -> bool {
        self.has_input_waiting() || self.drag_ends_pending() || self.files_held()
    }

    /// [`Runtime::hidden_wake`] for a shown window that can't draw (covered, minimised, on a
    /// locked or sleeping display). Like Chrome Blink's occluded lifecycle, ordinary updates do
    /// not rebuild the view while covered even with an assistive client attached; the tree
    /// catches up in one frame when the window uncovers. Three exceptions still build at once,
    /// without drawing or motion: an assistive client's own action (handled by `hidden_wake`
    /// via pending input), the window's first activation when out of date, and a programmatic
    /// focus request (`Tx::focus`, `Tx::focus_when_shown`, `Tx::return_focus`, `Tx::blur`).
    /// Returns what `hidden_wake` does.
    pub(crate) fn covered_wake(&mut self, app: &mut dyn AppDriver) -> bool {
        let stale = self.hidden_wake(app);
        if self.host.a11y.is_some()
            && (self.a11y_activation_build_pending() || self.has_focus_request())
            && (self.needs_commit() || self.commit.commits == 0)
        {
            let wake_clock = self.polled_at();
            self.frame.set_follow_up_poll(wake_clock);
            self.hidden_timer_frame(app);
            self.drain_hidden_input(app, wake_clock);
        }
        stale
    }

    /// Whether the app has a latched or pending programmatic focus request.
    fn has_focus_request(&self) -> bool {
        self.kbd.focus_request.is_some()
            || self.frame.pending.focus_to.is_some()
            || self.frame.pending.blur
    }

    /// One frame in one pass that steps `catch_up + dt` seconds. Motion already running advances
    /// `catch_up` before this frame's handlers and commit, so they retarget it from its pose at
    /// the frame's time. Then everything, including what the commit started, steps `dt`. A late
    /// frame or a sleeping host's gap belongs to old tracks, never to targets first observed
    /// on this frame.
    pub(crate) fn frame_split(&mut self, app: &mut dyn AppDriver, catch_up: f64, dt: f64) -> bool {
        self.motion.clear_handovers();
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
            // Immediate hover changes were already emitted by the pointer move that scheduled
            // any hover-delay timer, so deliver them before a later leave deadline fires.
            let mut input = Vec::new();
            for q in std::mem::take(&mut self.frame.queue) {
                if matches!(q.action, Action::HoverChange(..) | Action::LinkHover(..)) {
                    self.frame.queue.push_queued(q);
                } else {
                    input.push(q);
                }
            }
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
        let commits0 = self.commit.commits;
        #[cfg(debug_assertions)]
        let view_commits0 = self.commit.view_commits();
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
        let due_touch = self.pointer.touch_hold_deadline().is_some_and(|d| d <= self.clock);
        self.frame.idle_frame_begins(self.animating, polled || due > 0 || due_touch, live_woke);
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
        self.frame.set_poll_capped(false);
        let mut frame_had_input = self.frame.queue.has_input();
        if !polled && app.has_poll() {
            tx.request_poll = false;
            let mut one = self.transaction(app);
            one.input_waiting = frame_had_input;
            // Scheduling does not imply a model change.
            if self.frame.record_poll(crate::tooling::perf_warn::timed("poll", || app.poll(self.clock, &mut one))) {
                ran = true;
                why |= CommitWhy::POLL;
                self.commit.watch.meter.poll();
            }
            merge(&mut tx, one);
        }
        if !polled && app.playground().is_some_and(|p| p.polls()) {
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
        // Something besides a checked live-only event wants this frame's commit, so a change
        // it makes isn't the live-only handler's.
        #[cfg(debug_assertions)]
        let mut ran_other = ran;
        let mut handler_ran = false;
        let mut drag_cancelled = self.flush_cancel_pointer(app, &mut tx, &mut ran, &mut why, &mut handler_ran, #[cfg(debug_assertions)] &mut ran_other);
        // A host settle (an uncover, a restore, a shown window) lands the tree the frame began
        // with, before this frame's input runs. The input's own commit below then animates, so
        // a dialog the first press closes still plays its exit.
        if self.commit.is_settling() && queue.has_input() {
            self.settle_before_input(app, &mut tx, &mut why);
            ran = false;
            #[cfg(debug_assertions)]
            { ran_other = false; }
        }
        let mut pointer_hover = false;
        self.run_action_queue(
            app,
            queue,
            &mut tx,
            &mut ran,
            &mut ran_layout,
            &mut why,
            &mut pointer_hover,
            &mut handler_ran,
            &mut drag_cancelled,
            #[cfg(debug_assertions)]
            &mut ran_other,
        );
        // Once a frame, however often a drag moved the selection, and in time for this commit.
        for action in self.selection_changes(app) {
            #[cfg(debug_assertions)]
            let checks = self.frame.live_checks();
            let r = self.run_action(app, action, &mut tx);
            #[cfg(debug_assertions)]
            let non_live = r && checks == self.frame.live_checks();
            #[cfg(not(debug_assertions))]
            let non_live = r;
            #[cfg(debug_assertions)]
            { ran_other |= non_live; }
            handler_ran |= non_live;
            if r { ran = true; why |= CommitWhy::DIRTY; }
            self.flush_cancel_pointer(app, &mut tx, &mut ran, &mut why, &mut handler_ran, #[cfg(debug_assertions)] &mut ran_other);
        }
        if app.has_poll() {
            self.run_poll_follow_ups(
                app,
                &mut tx,
                &mut ran,
                &mut ran_layout,
                &mut why,
                &mut pointer_hover,
                &mut handler_ran,
                &mut frame_had_input,
                #[cfg(debug_assertions)]
                &mut ran_other,
            );
        }
        #[cfg(target_os = "linux")]
        self.note_primary_selection(app);
        let mut end_of_frame = EndOfFrameRequests::take_from(&mut tx);
        self.latch_requests(app, &mut tx);
        // A handler asked for a menu: the hover it takes ends in this commit, not a frame later.
        if self.release_pointer_for_popup() {
            let mut popup_handler_ran = false;
            for queued in std::mem::take(&mut self.frame.queue) {
                if !matches!(queued.action, Action::HoverChange(..) | Action::LinkHover(..)) { self.frame.queue.push_queued(queued); continue; }
                #[cfg(debug_assertions)]
                let checks = self.frame.live_checks();
                let r = self.run_action(app, queued.action, &mut tx);
                #[cfg(debug_assertions)]
                let non_live = r && checks == self.frame.live_checks();
                #[cfg(not(debug_assertions))]
                let non_live = r;
                #[cfg(debug_assertions)]
                { ran_other |= non_live; }
                popup_handler_ran |= non_live;
                if r { ran = true; why |= if queued.input { CommitWhy::INPUT } else { CommitWhy::DIRTY }; }
            }
            if app.has_poll() && !self.frame.poll_capped() && (popup_handler_ran || tx.request_poll) {
                self.run_poll_follow_ups(
                    app,
                    &mut tx,
                    &mut ran,
                    &mut ran_layout,
                    &mut why,
                    &mut pointer_hover,
                    &mut popup_handler_ran,
                    &mut frame_had_input,
                    #[cfg(debug_assertions)]
                    &mut ran_other,
                );
            }
            end_of_frame.extend_from(&mut tx);
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
        let has_scroll_request = !tx.scroll_to.is_empty() || !tx.scroll_bottom.is_empty()
            || !tx.scroll_pages.is_empty() || !tx.scroll_offsets.is_empty() || self.edit.caret_follow;
        let shifted = !(self.commit.dirty || self.commit.relayout || settling) && self.virtual_window_changed();
        // When a scroll request is about to move viewports and nothing else needs a commit,
        // wait for the request so we remount once at the new offset instead of filling the old one.
        let defer_shift = shifted && !ran && !ran_layout && !read_moved && !tx.commit.lands() && has_scroll_request;
        let besides = self.commit.dirty || self.commit.relayout || settling || (shifted && !defer_shift);
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
        self.finish_deferred_field_menu(app, &mut tx, Some(&mut end_of_frame));
        self.finish_deferred_popup_menu(app, &mut tx, Some(&mut end_of_frame));
        #[cfg(debug_assertions)]
        { self.frame.clear_live_check(); }
        let mount_rounds = self.deliver_mounts(
            app,
            &mut tx,
            commits0,
            &mut end_of_frame,
            mount::MAX_MOUNT_ROUNDS,
        );
        if std::mem::take(&mut tx.request_poll) {
            self.schedule_wakes([std::time::Duration::ZERO]);
        }
        tx.keep_on_screen.clear();
        self.queue_reveal_selected(&mut tx, pointer_hover);
        #[cfg(debug_assertions)]
        let instant_scroll = tx.scroll_offsets.iter().find(|r| tx.commit.snaps() || r.glide.is_none()).map(|r| r.key.clone());
        self.edit.caret_follow_check = None;
        let follow = std::mem::take(&mut self.edit.caret_follow);
        let commits_before_scroll = self.commit.commits;
        self.finish_scroll_requests(app, &mut tx, follow);
        if self.edit.selectable.pending_move.is_seeking() {
            self.drive_pending_caret(app);
        }
        if defer_shift && self.commit.commits == commits_before_scroll && self.virtual_window_changed() {
            self.commit_virtual_scroll(app, &Tx::default());
        }
        // Those scrolls moved the caret or field selection after the commit a view placed from it saw.
        if (self.edit.caret_watch && self.caret_moved(None)) || self.field_selection_read_moved() {
            self.commit(app, &Tx { commit: Commit::Instant, ..Tx::default() });
        }
        self.deliver_visible_rows(app, &tx, commits0, pointer_hover);
        if self.commit.commits != commits_before_scroll && mount_rounds < mount::MAX_MOUNT_ROUNDS {
            let late_rounds = self.deliver_mounts(
                app,
                &mut tx,
                commits0,
                &mut end_of_frame,
                mount::MAX_MOUNT_ROUNDS - mount_rounds,
            );
            if late_rounds > 0 {
                let late_follow = std::mem::take(&mut self.edit.caret_follow);
                self.finish_scroll_requests(app, &mut tx, late_follow);
            }
        }
        #[cfg(debug_assertions)]
        {
            let scrolling = self.scrolling();
            self.commit.watch_frame(view_commits0, scrolling, instant_scroll.as_deref());
        }
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
        // Poll and input can publish after the initial wake read without committing. When no
        // picture sizes itself from its image (`has_pixel_aspect`), take the wake before latching
        // so this frame draws the published value without asking for a second, identical frame;
        // any publication after the take still wakes the next frame. With `has_pixel_aspect`,
        // retain the wake so a picture whose size changed after `pixel_aspect_changed` still gets
        // next frame's layout.
        if !self.tree.has_pixel_aspect { live_woke |= self.take_live_wake(); }
        let live_pending = live_woke || self.live_pending();
        let live_animating = self.latch_live(live_pending);
        if let Some(trace) = tx.trace.take() {
            for event in trace.events { self.trace_event(event); }
            for (name, kind, value) in trace.channels { self.trace_channel(name, kind, value); }
        }
        let dt = if self.fit_frame_holds() { 0.0 } else { dt };
        let instant_scroll = tx.commit.snaps() || self.settled;
        let mut animating = self.step_motion(app, dt, instant_scroll) || live_animating;
        if self.deliver_popup_menu_fallback_after_paint(app, &mut tx, Some(&mut end_of_frame)) {
            animating |= self.step_motion(app, 0.0, true);
        }
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
        if !end_of_frame.open_urls.is_empty() { self.open_urls(end_of_frame.open_urls); }
        if !end_of_frame.open_paths.is_empty() { self.open_paths(end_of_frame.open_paths); }
        if !end_of_frame.haptics.is_empty() {
            let now = self.clock;
            self.host.haptics.push_at(end_of_frame.haptics, now);
        }
        self.animating
    }

    /// Carries out a deferred `Tx::cancel_pointer` once its calling handler or poll has
    /// returned: ends pointer capture/hover without re-entrancy and delivers the cancelled
    /// `Phase::Up` drag and hover-leave actions in this same frame. Returns whether a held
    /// drag was cancelled.
    pub(super) fn flush_cancel_pointer(
        &mut self,
        app: &mut dyn AppDriver,
        tx: &mut Tx,
        ran: &mut bool,
        why: &mut u16,
        handler_ran: &mut bool,
        #[cfg(debug_assertions)] ran_other: &mut bool,
    ) -> bool {
        if !std::mem::take(&mut tx.cancel_pointer) { return false; }
        let had_drag = self.pointer.press_held().is_some_and(|h| h.drag_handler.is_some());
        let start = self.frame.queue.len();
        self.cancel_pointer();
        let queued = self.frame.queue.split_off(start);
        for q in queued {
            let _input = crate::input::time::scope(q.at);
            #[cfg(debug_assertions)]
            let checks = self.frame.live_checks();
            let r = self.run_action(app, q.action, tx);
            #[cfg(debug_assertions)]
            let non_live = r && checks == self.frame.live_checks();
            #[cfg(not(debug_assertions))]
            let non_live = r;
            #[cfg(debug_assertions)]
            { *ran_other |= non_live; }
            *handler_ran |= non_live;
            if r {
                *ran = true;
                *why |= if q.input { CommitWhy::INPUT } else { CommitWhy::DIRTY };
            }
            self.run_pick_reads(app, tx);
        }
        if std::mem::take(&mut tx.cancel_pointer) { self.cancel_pointer(); }
        had_drag && !self.pointer.pressing()
    }

    /// Lands a host settle for the tree as it stands before this frame's input, then lets the
    /// input's own commit animate what it changes. `apply` clears the settle latch as this
    /// commit applies it: the settle is spent here, so nothing is deferred or dropped.
    fn settle_before_input(&mut self, app: &mut dyn AppDriver, tx: &mut Tx, why: &mut u16) {
        let deferred = EndOfFrameRequests::take_from(tx);
        self.latch_requests(app, tx);
        deferred.restore_into(tx);
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


