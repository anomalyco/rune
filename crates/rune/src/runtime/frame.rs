//! Frame loop, clocks, wake deadlines, request latching and motion/paint-order stepping.
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

    /// Clock seconds now: the wall clock in a window (input arrives between frames), the
    /// virtual clock headless.
    pub(super) fn now_clock(&self) -> f64 {
        self.wall_start.map_or(self.clock, |t| t.elapsed().as_secs_f64())
    }

    /// The window's size in its points and its scale. The view lays out in them over its zoom
    /// ([`Cx::zoom`]), at the scale times the zoom.
    pub fn set_viewport(&mut self, viewport: (f32, f32), scale: f32) {
        (self.host.zoom.window, self.host.zoom.window_scale) = (viewport, scale);
        let (viewport, scale) = self.host.zoom.view();
        if self.fit_resized(viewport.1) {
            self.commit.relayout = true;
            self.commit.viewport_stepped = false;
        }
        if self.change_viewport(viewport, scale) { self.commit.viewport_stepped = false; }
    }

    /// The viewport one frame of the window's own resize animation moved it to. Unlike a
    /// user's resize it doesn't land running motion: content at rest follows the window and
    /// springs keep going.
    pub(crate) fn step_viewport(&mut self, viewport: (f32, f32), scale: f32) {
        (self.host.zoom.window, self.host.zoom.window_scale) = (viewport, scale);
        let (viewport, scale) = self.host.zoom.view();
        let fresh = !self.commit.relayout;
        if self.change_viewport(viewport, scale) && fresh { self.commit.viewport_stepped = true; }
    }

    /// Whether the change needs a layout.
    pub(super) fn change_viewport(&mut self, viewport: (f32, f32), scale: f32) -> bool {
        // A window that fits its content lays out the same at any height: its own resize
        // mustn't cut short the layout springs it is following.
        if self.host.fit_height && viewport.0 == self.viewport.0 && scale == self.env.scale {
            self.viewport = viewport;
            return false;
        }
        let changed = viewport != self.viewport || scale != self.env.scale;
        if changed {
            if scale != self.env.scale { self.motion.flights.clear(); }
            self.viewport = viewport;
            self.env.scale = scale;
            self.scene.set_pixel_scale(scale);
            self.commit.relayout = true;
        }
        changed
    }

    /// Queue a native focus transition. Repeated notifications do not repeat the callback.
    /// Losing focus is also `keys_lost`.
    pub(crate) fn window_focus(&mut self, app: &mut dyn AppDriver, focused: bool) {
        // Every focus gain, even one a hide without blur left unreported.
        if focused { self.kbd.keys_lost = false; }
        if self.host.window_focused != Some(focused) {
            // Refocused, the pointer where it was hovers again; blurred, it waits for a move.
            if focused && self.host.window_focused == Some(false) { self.pointer.hover_paused = false; }
            // A selection takes its active or inactive pair (`El::selection_style`): a focused
            // field's, or a selectable text's.
            if self.host.window_focused.is_some()
                && (self.focused_input().is_some_and(|i| i.selection_style.is_some()) || self.text_selection().is_some_and(|(_, r)| !r.is_empty()))
            {
                self.commit.dirty = true;
            }
            self.host.window_focused = Some(focused);
            if app.has_on_focus() {
                let (mut one, stall) = self.begin_update(app);
                app.on_focus(focused, &mut one);
                self.finish_update(app, stall, one, true, false);
            }
            // Deliver the cancelled Up before FocusLost can commit a view that removes its
            // grip (or the focus callback clears the app's drag state).
            if !focused {
                self.cancel_pointer();
                self.kbd.held = physical_dispatch::Activation::Up;
            }
            if self.commit.commits == 0 || self.commit.dirty || self.tree.nodes.get(self.app_root()).is_some_and(|n| n.handlers.on_window_focus.is_some()) {
                self.frame.queue.push(Action::WindowFocus(focused));
            }
            if !focused {
                self.finish_composition();
                self.keys_lost();
                // Losing the window ends hover, rather than waiting out an exit grace. A window
                // that isn't key still hears the pointer, so its next move starts the wait again.
                self.end_hover_delays();
                self.pointer.hover_paused = true;
            }
        }
        if !focused { self.edit.input_click.clear(); self.wheel_lost(); }
    }

    /// The window gets no more keys: it lost focus or hid, which may never report a blur. The
    /// focused physical handler hears `FocusLost` and held modifier keys are released, once
    /// until the window has focus again. A hidden host steps `hidden_timer_frame` while
    /// `keys_queued` to deliver them.
    pub(crate) fn keys_lost(&mut self) {
        if std::mem::replace(&mut self.kbd.keys_lost, true) { return; }
        self.physical_input(crate::PhysicalInput::FocusLost);
    }

    /// Keyboard input, a focus change, a file drag step or a file dialog answer is queued for the
    /// next frame.
    pub(crate) fn keys_queued(&self) -> bool {
        self.frame.queue.iter().any(|a| matches!(a, Action::Physical(..) | Action::Key { .. } | Action::KeyUp(_) | Action::WindowFocus(_) | Action::Files(_) | Action::FileDialog(..)))
    }

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
        #[cfg(feature = "hot")]
        return self.frame.hot.as_ref().is_some_and(crate::hot::Session::pending);
        #[cfg(not(feature = "hot"))]
        false
    }

    /// Marks app state changed outside a handler (the next frame re-runs the view).
    pub fn invalidate(&mut self) {
        self.commit.dirty = true;
    }

    /// Starts an [`Runtime::update`] transaction with the clock reading now and begins its
    /// stall timer right before the app callback runs.
    pub(super) fn begin_update(&self, app: &dyn ViewDriver) -> (Tx, crate::tooling::perf_warn::Stall) {
        // Its wakes count from now (`schedule_pending_wakes`), so its clock reads now too.
        let one = Tx { clock: self.now_clock(), ..self.transaction(app) };
        (one, crate::tooling::perf_warn::Stall::start())
    }

    /// Finishes an [`Runtime::update`] / `update_if` / `update_if_ordered` call after its
    /// closure ran against `app.state`.
    pub(super) fn finish_update(
        &mut self,
        app: &mut dyn AppDriver,
        stall: crate::tooling::perf_warn::Stall,
        mut one: Tx,
        changed: bool,
        ordered: bool,
    ) -> (bool, Option<std::num::NonZeroU64>) {
        stall.end_named("an `update` closure");
        let stale = changed || one.frame_work() || one.window_frame.is_some() || one.srgb_blend.is_some_and(|b| b != self.paint.srgb_blend) || one.font_smoothing.is_some_and(|m| m != self.text.font_smoothing())
            || self.changes_families(&one)
            || (one.blur && self.kbd.focus.path().is_some());
        let minimize = if ordered {
            let asked = one.minimize_order;
            self.run_pick_reads(app, &mut one);
            let survived = if asked.is_some() && one.minimize_order == asked { one.minimize_order.take() } else { None };
            self.latch_after_picks(app, &mut one);
            survived
        } else {
            self.latch_requests(app, &mut one);
            None
        };
        merge(&mut self.frame.pending, one);
        self.commit.dirty |= changed;
        (stale, minimize)
    }

    /// Takes a `Tx::minimize` that `what` can't carry out, saying so in debug builds.
    pub(crate) fn ignore_minimize(&mut self, what: &str) {
        let field = self.host.minimize.field();
        self.minimize_step(crate::window::MinimizeEvent::Lost);
        if field && cfg!(debug_assertions) {
            eprintln!("rune: minimize ignored: {what} can't minimize");
        }
    }

    /// Latches a Tx's shell and clipboard requests for the shell, so `update`, a hidden poll
    /// and `on_close` reach the clipboard without a frame, and applies clipboard requests to a
    /// memory clipboard at once. A secret clear drops an earlier secret copy not yet written.
    pub(super) fn latch_requests(&mut self, app: &mut dyn AppDriver, tx: &mut Tx) {
        self.run_pick_reads(app, tx);
        self.latch_after_picks(app, tx);
    }

    /// [`Self::latch_requests`] once the pick callbacks have run.
    fn latch_after_picks(&mut self, app: &mut dyn AppDriver, tx: &mut Tx) {
        self.shell_request = self.shell_request.max(tx.shell.take());
        if tx.minimize_order.take().is_some() { self.minimize_step(crate::window::MinimizeEvent::Asked); }
        self.host.titlebar.toggle_maximized |= std::mem::take(&mut tx.toggle_maximized);
        self.host.titlebar.toggle_full_screen |= std::mem::take(&mut tx.toggle_full_screen);
        for ev in std::mem::take(&mut tx.keys) { self.key(ev); }
        self.latch_playground(app, tx);
        for prefix in std::mem::take(&mut tx.evict_memos) { self.invalidate_memos_by_prefix(&prefix); }
        if !tx.open_urls.is_empty() { self.open_urls(std::mem::take(&mut tx.open_urls)); }
        if std::mem::take(&mut tx.clear_secret) {
            if self.copy_request.as_ref().is_some_and(crate::ClipboardWrite::is_secret) { self.copy_request = None; }
            self.edit.clear_secret_request = true;
            self.edit.pill_copy = None;
        }
        if let Some(copy) = tx.copy.take() {
            self.edit.pill_copy = (!copy.is_secret())
                .then(|| copy.content().html())
                .flatten()
                .filter(|h| h.contains("<span data-pill="))
                .map(|h| {
                    let frag = crate::TextEdit::from_html(h);
                    (
                        zeroize::Zeroizing::new(copy.content().text().to_owned()),
                        frag.pills().map(|(r, id)| (r, id.to_owned())).collect::<Vec<_>>(),
                    )
                })
                .filter(|(_, pills)| !pills.is_empty());
            self.copy_request = Some(copy);
        }
        if std::mem::take(&mut tx.paste) { self.edit.paste_request = Some(self.paste_target()); }
        for read in tx.clipboard_reads.drain(..).filter_map(|read| read.take()) {
            self.edit.next_clipboard_read += 1;
            self.edit.clipboard_reads.push((self.edit.next_clipboard_read, read));
            self.edit.read_requests.push(self.edit.next_clipboard_read);
        }
        self.host.popups.extend(tx.popups.drain(..).filter_map(|call| call.take()));
        for request in tx.file_dialogs.drain(..).filter_map(|call| call.take()) {
            self.request_file_dialog(request);
        }
        if let Some(mode) = tx.font_smoothing.take() { self.set_font_smoothing(mode); }
        if let Some((sans, mono)) = tx.font_families.take() { self.set_font_families(&sans, &mono); }
        if let Some(srgb) = tx.srgb_blend.take() { self.set_srgb_blend(srgb); }
        if let Some(rewind) = tx.rewind.take() { self.motion.rewind = rewind; }
        if std::mem::take(&mut tx.blur) {
            if self.kbd.focus.path().is_some() {
                self.set_focus(app, None, self.kbd.focus.keyboard());
                self.commit.dirty = true;
            }
            // An explicit blur is not the temporary pointer-down blur of a scrim, and nothing
            // hands focus on from it: only a control the user blurred and then left does.
            self.kbd.focus.step(focus_machine::FocusEvent::Dropped);
            self.kbd.focus.step(focus_machine::FocusEvent::Settled);
        }
        if let Some(request) = tx.focus_to.take() {
            self.kbd.focus_request = Some(request);
            self.commit.dirty = true;
        }
        if let Some(title) = tx.title.take() { self.host.window_title.set(title); }
        if let Some((text, how)) = tx.announce.take() { self.host.announcement.say(text, how); }
        if let Some(mut request) = tx.window_frame.take() {
            request.spring = (request.animate && !tx.commit.snaps() && !self.reduced_motion).then(|| tx.motion.map_or(motion::DEFAULT, MotionSource::spring));
            self.host.window_frame = Some(self.host.window_frame.unwrap_or_default().merge(request));
        }
        crate::clipboard::flush_memory(self);
    }

    /// Host time on the logical clock.
    pub(super) fn wall_clock(&self) -> Option<f64> {
        #[cfg(test)]
        if let Some(clock) = &self.frame.test_clock { return Some(clock.now()); }
        self.wall_start.map(|t0| t0.elapsed().as_secs_f64())
    }

    /// Schedules the `wake_after`s that updates left for a frame, counting from now, when no
    /// frame or hidden poll comes to. The clock stays at the last poll, so the frame-interval
    /// floor on timers (`window::timer_deadline`) still counts from that poll.
    pub(crate) fn schedule_pending_wakes(&mut self) {
        let wakes = std::mem::take(&mut self.frame.pending.wakes);
        self.schedule_wakes_from(self.now_clock(), wakes);
    }

    /// Sorted, de-duplicated deadlines on the logical clock.
    pub(super) fn schedule_wakes(&mut self, wakes: impl IntoIterator<Item = std::time::Duration>) {
        self.schedule_wakes_from(self.clock, wakes);
    }

    /// Wakes for a frame once the clock reaches `due`, as [`Tx::wake_at`] asks from a handler (a
    /// deadline already past wakes the next frame), for the runtime's own timers.
    #[cfg_attr(feature = "frame-profile", track_caller)]
    pub(super) fn wake_at(&mut self, due: f64) {
        #[cfg(feature = "frame-profile")]
        crate::frame::live_log::waker(std::panic::Location::caller(), std::time::Duration::from_secs_f64((due - self.clock).max(0.0)));
        self.schedule_wakes_from(due, [std::time::Duration::ZERO]);
    }

    pub(super) fn schedule_wakes_from(&mut self, now: f64, wakes: impl IntoIterator<Item = std::time::Duration>) {
        for delay in wakes {
            let deadline = now + delay.as_secs_f64();
            if let Err(index) = self.frame.wake_deadlines.binary_search_by(|t| t.total_cmp(&deadline)) {
                self.frame.wake_deadlines.insert(index, deadline);
            }
        }
    }

    pub fn tracing(&self) -> bool { self.frame.trace_channels.is_some() || self.scene.tracing() }

    pub(super) fn transaction(&self, app: &dyn ViewDriver) -> Tx {
        Tx { trace: self.tracing().then(crate::el::TraceCalls::default), mods: self.kbd.held_mods, selection: self.selection_snapshot(app),
            input_time: crate::input::time::current(), clock: self.clock, focus_visible: self.kbd.focus.keyboard(), focused: self.focused_key(), assistive: self.host.a11y.is_some() || cfg!(target_arch = "wasm32"), ..Tx::default() }
    }

    /// A scalar from the actual draw path (e.g. `shell.width`). Values hold until resampled.
    /// Call before the tooling writer records the frame; scene traces sample in `frame`.
    pub fn trace_channel(&mut self, name: &'static str, kind: &'static str, value: f32) {
        if let Some(channels) = &mut self.frame.trace_channels { channels.insert(name, (kind, value)); }
        if let Some(trace) = self.scene.trace_mut() { trace.channel(name, kind, value); }
    }

    /// Queues an app marker once for each attached writer. Formatting is skipped when off.
    #[inline]
    pub fn trace_event(&mut self, event: impl std::fmt::Display) {
        self.trace_event_dyn(&event);
    }

    pub(crate) fn trace_event_dyn(&mut self, event: &dyn std::fmt::Display) {
        if self.frame.trace_channels.is_some() { event_log::push(&mut self.events, event.to_string()); }
        if let Some(trace) = self.scene.trace_mut() { trace.event(event); }
    }

    /// Next requested wake on the unscaled app clock. None lets the shell sleep indefinitely.
    pub fn wake_deadline(&self) -> Option<f64> {
        self.logical_wake_deadline()
    }

    /// The native window or overlay's frame counts; `None` headless.
    pub fn surface_stats(&self) -> Option<crate::SurfaceStats> {
        self.host.surface_stats.as_deref().map(crate::frame::surface_stats::read)
    }

    /// When `poll` last ran, on `wake_deadline`'s clock.
    pub(crate) fn polled_at(&self) -> f64 {
        self.frame.timer_wake.polled_at().unwrap_or(self.clock)
    }

    pub(super) fn logical_wake_deadline(&self) -> Option<f64> {
        self.frame.wake_deadlines.first().copied().into_iter().chain(self.motion.loop_wake.map(|w| w.0)).min_by(f64::total_cmp)
    }

    /// The next due timer of `poll`'s, on `wake_deadline`'s clock.
    pub(crate) fn poll_wake_deadline(&self) -> Option<f64> {
        self.frame.wake_deadlines.first().copied()
    }

    /// The next frame motion asks for at its own cadence (a loop's next step, a video's next
    /// picture), on `wake_deadline`'s clock.
    pub(crate) fn motion_wake_deadline(&self) -> Option<f64> {
        self.motion.loop_wake.map(|w| w.0)
    }

    /// Lands every track before the next frame: growth, reveals and follow-the-end (guarantee 8,
    /// for returning to a window or session).
    pub fn settle_all(&mut self) {
        self.commit.settle = true;
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
        if self.frame.hot.as_mut().is_some_and(crate::hot::Session::changed) {
            // Replace handler trait objects before queued input can call an old vtable.
            // A regular commit preserves keyed nodes, scrolls and spring velocities.
            self.commit.memo.clear();
            self.commit(app, &Tx::default());
        }
        let mut tx = std::mem::take(&mut self.frame.pending);
        let mut ran = false;
        let mut why = if tx.polled_change && self.commit.dirty { CommitWhy::POLL } else { 0 };
        #[cfg(debug_assertions)]
        let commits0 = self.commit.commits;
        let shaped0 = self.text.shaped;
        self.settled = false;
        self.motion.landing_holds.clear();
        // `RUNE_LOOP_CHECK=1`: the frame that runs instead of a checked step samples at its clock.
        self.clock = self.paint.keep.cycles.check_clock.take().unwrap_or_else(|| self.wall_clock().unwrap_or(self.clock + elapsed));
        if self.wall_start.is_none() {
            crate::video::headless_frame(self.clock);
            // Fake recorders publish this frame's level to bound meters, drawn this frame.
            crate::recorder::headless_frame(self.clock);
            live_woke |= self.take_live_wake();
        }
        if let Some((deadline, animation_time)) = self.motion.loop_wake
            && !self.animating
        {
            // Native shells cap dt after idle. Advance the sleeping part of the
            // delay before new handlers retarget, without double-stepping manual time.
            let elapsed = animation_time - (deadline - self.clock).max(0.0) * self.time_scale as f64;
            self.t += (elapsed - self.t - (catch_up + dt) * self.time_scale as f64).max(0.0);
            self.scene.set_time(self.t);
            if self.clock >= deadline { self.motion.loop_wake = None; }
        }
        // Bring old tracks, growth and scroll to this frame's time before a poll or update
        // retargets them. Presenting waits for `step_motion`, so the frame is sampled and
        // built once.
        if catch_up > 0.0 { self.catch_up_motion(catch_up); }
        self.keep_loops_on_clock(dt);
        let polled = self.frame.timer_wake.frame_polled(self.frame.wake_deadlines.first().is_some_and(|t| *t <= self.clock));
        let due = self.frame.wake_deadlines.partition_point(|t| *t <= self.clock);
        self.idle_frame_begins(polled || due > 0, live_woke);
        self.frame.wake_deadlines.drain(..due);
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
            if self.frame.idle_meter.polled(crate::tooling::perf_warn::timed("poll", || app.poll(self.clock, &mut one))) {
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
            let split = self.motion.motion_needs_cause && why & CommitWhy::POLL != 0 && (input || routed);
            if split || (ran || self.commit.dirty || !tx.scroll_to.is_empty()) && routed {
                if split {
                    let links = std::mem::take(&mut tx.open_urls);
                    self.latch_requests(app, &mut tx);
                    tx.open_urls = links;
                }
                if ran || self.commit.dirty {
                    self.capture_flights(&mut tx);
                    if self.commit.settle { tx.settle(); }
                    self.settled |= tx.commit.lands();
                    #[cfg(debug_assertions)]
                    if ran_other || self.commit.dirty || ran_layout { self.frame.live_check = None; }
                    self.capture_keeps(&mut tx);
                    tx.unprompted = self.motion.motion_needs_cause && why & CommitWhy::INPUT == 0 && !tx.asked_motion;
                    if split && tx.unprompted { tx.commit = tx.commit.without_input(); }
                    if routed { why |= CommitWhy::ROUTE; }
                    if self.commit.dirty { why |= CommitWhy::DIRTY; }
                    self.commit_for(app, &tx, std::mem::take(&mut why));
                    #[cfg(debug_assertions)]
                    { let check = self.frame.live_check.take(); self.compare_live_check(check); ran_other = false; }
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
                let checks = self.frame.live_checks;
                let r = self.run_action(app, action, &mut tx);
                #[cfg(debug_assertions)]
                { ran_other |= r && checks == self.frame.live_checks; }
                ran |= r;
                if r { why |= if input { CommitWhy::INPUT } else { CommitWhy::DIRTY }; }
            }
            self.run_pick_reads(app, &mut tx);
        }
        // Once a frame, however often a drag moved the selection, and in time for this commit.
        for action in self.selection_changes(app) {
            #[cfg(debug_assertions)]
            let checks = self.frame.live_checks;
            let r = self.run_action(app, action, &mut tx);
            #[cfg(debug_assertions)]
            { ran_other |= r && checks == self.frame.live_checks; }
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
                let checks = self.frame.live_checks;
                let r = self.run_action(app, queued.action, &mut tx);
                #[cfg(debug_assertions)]
                { ran_other |= r && checks == self.frame.live_checks; }
                if r { ran = true; why |= if queued.input { CommitWhy::INPUT } else { CommitWhy::DIRTY }; }
            }
            open_urls.extend(std::mem::take(&mut tx.open_urls));
            self.latch_requests(app, &mut tx);
        }
        self.release_files();
        self.schedule_wakes(tx.wakes.drain(..));
        self.capture_flights(&mut tx);
        if self.edit.caret_watch && self.caret_moved(None) { self.commit.dirty = true; }
        let read_moved = self.layout_reads_moved();
        self.commit.reuse.read_moved = read_moved;
        if self.tree.has_pixel_aspect && self.pixel_aspect_changed() { self.commit.relayout = true; }
        let shifted = !(self.commit.dirty || self.commit.relayout || self.commit.settle) && self.virtual_window_changed();
        let besides = self.commit.dirty || self.commit.relayout || self.commit.settle || shifted;
        let other = ran || besides;
        self.count_layout_echo(ran_layout || read_moved, other);
        if other || ran_layout || read_moved {
            if self.commit.relayout && !ran && !self.commit.dirty {
                if self.commit.viewport_stepped { tx.window_step = true } else { tx.relayout = true }
            }
            if self.commit.settle {
                tx.commit = Commit::Settle;
            }
            self.settled = tx.commit.lands();
            self.commit.state_unchanged = !ran && !self.commit.dirty && !ran_layout && !self.commit.settle && self.commit.commits > 0;
            #[cfg(debug_assertions)]
            if ran_other || besides || ran_layout || read_moved { self.frame.live_check = None; }
            self.capture_keeps(&mut tx);
            let flag = |on: bool, bit: u16| if on { bit } else { 0 };
            // Motion has a cause: the user, in this window. A commit nothing in this frame's
            // input asked for shows what it brings in its final state (`Tx::unprompted`).
            tx.unprompted = self.motion.motion_needs_cause && why & CommitWhy::INPUT == 0 && !tx.asked_motion;
            // Its layout lands too, as an instant change's does: a container that grows to fit what a
            // server brought doesn't ease to its height (motion already running keeps on).
            if tx.unprompted { tx.commit = tx.commit.without_input(); }
            self.commit_for(app, &tx, why | flag(self.commit.dirty, CommitWhy::DIRTY) | flag(self.commit.relayout, CommitWhy::RELAYOUT) | flag(self.commit.settle, CommitWhy::SETTLE)
                | flag(shifted, CommitWhy::VIRTUAL) | flag(ran_layout, CommitWhy::LAYOUT) | flag(read_moved, CommitWhy::READ));
            #[cfg(debug_assertions)]
            { let check = self.frame.live_check.take(); self.compare_live_check(check); }
            self.apply_keeps(app);
        }
        #[cfg(debug_assertions)]
        { self.frame.live_check = None; }
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
        self.watch_frame(commits0, tx.scroll_offsets.iter().find(|r| tx.commit.snaps() || r.glide.is_none()).map(|r| r.key.as_str()));
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
        if live_animating { self.motion.anim_sources |= AnimSource::LIVE; }
        if self.time_scale <= 0.0 { self.motion.anim_sources = 0; }
        if filling { self.motion.anim_sources |= AnimSource::VIRTUAL_FILL; }
        self.animating = (animating && self.time_scale > 0.0) || filling;
        self.idle_frame_ends();
        self.interaction_built();
        if !open_urls.is_empty() { self.open_urls(open_urls); }
        self.animating
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
    pub(super) fn wake_for_clock(&mut self, due: f64) {
        let at = self.t + (due - self.clock).max(0.0) * self.time_scale as f64;
        self.wake_for_motion(at);
    }

    pub(super) fn catch_up_motion(&mut self, dt: f64) {
        let step = dt * self.time_scale as f64;
        self.t += step;
        self.scene.set_time(self.t);
        self.motion.caught_up += step as f32;
        self.step_growth(step as f32);
        self.step_scroll_by(step as f32, true);
    }

    pub(super) fn step_motion(&mut self, app: &dyn ViewDriver, dt: f64, instant_scroll: bool) -> bool {
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

    /// Subtree (preorder) paint order, then each flying element's subtree on top. A sticky
    /// element paints after its normal siblings (and before overlay siblings), so it covers the
    /// rows it's stuck over.
    pub(super) fn update_layer_paint_order(&mut self) {
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
