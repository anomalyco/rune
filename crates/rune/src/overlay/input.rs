//! An overlay's pointer, cursor and input regions, its drag-out, and when it advances, commits,
//! wakes and syncs with its handle's requests.

use super::*;

/// Whether a live Wayland layer-shell surface must be destroyed and recreated for `requests`.
/// `zwlr_layer_surface_v1` fixes its `wl_output` at creation, so only changing the target
/// display requires a new surface; placement changes (size, anchors, margins) update in place.
#[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
pub(super) fn should_recreate_layer(current_display: &DisplayTarget, requests: &Requests, is_layer: bool) -> bool {
    is_layer && requests.display.as_ref().is_some_and(|display| display != current_display)
}

/// An overlay has no window to ask: whatever a step requested of one is dropped.
pub(super) fn drop_window_requests(rt: &mut RuntimeCore) {
    rt.shell_request = None;
    rt.ignore_minimize("an overlay");
    // Overlays size through `OverlayHandle::resize`, not `Tx::resize_window`.
    rt.host.window_frame = None;
}

impl OverlayCore {
    /// The runtime's cursor while the pointer is over a hit area of a shown overlay that takes
    /// input; the arrow once when it leaves, so `Cursor::Hidden` never outlives the overlay or
    /// the hover, and a click-through area leaves the cursor to the window under it. A Wayland
    /// layer overlay sets it through cursor-shape-v1; without that, the pointer keeps the cursor
    /// it arrived with.
    pub(super) fn apply_cursor(&mut self, rt: &RuntimeCore) {
        let input = self.options.input.resolved(self.interactive);
        let over = rt.pointer().is_some_and(|(x, y)| {
            x >= 0.0 && y >= 0.0 && x < self.at.size.0 && y < self.at.size.1
                && (input != OverlayInput::Regions || rt.pointer_captured()
                    || rt.hit_regions().iter().any(|r| crate::overlay::options::contains(*r, x, y)))
        });
        let active = self.presence.shown() && input != OverlayInput::None && over;
        let want = rt.cursor();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Layer(layer), .. }) = self.surface.live_mut() {
            layer.set_cursor(if active { want } else { crate::el::Cursor::Default });
            return;
        }
        let (Some(Live { backing: Backing::Window(window), .. }), Some(cursor)) = (self.surface.live(), &mut self.cursor) else { return };
        cursor.apply(window, want, active);
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_cursor(active.then_some(want)); }
    }

    #[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
    pub(super) fn observe_pointer(&mut self, app: &mut dyn OverlayApp, x: f32, y: f32) {
        if app.rt().pointer_captured() || (x >= 0.0 && y >= 0.0 && x < self.at.size.0 && y < self.at.size.1) {
            self.dirty |= app.observe_pointer(x, y);
            self.begin_drag_out(app.rt_mut());
        } else if app.rt().pointer().is_some() {
            app.rt_mut().pointer_leave();
            self.dirty = true;
        }
    }

    /// Starts the OS drag a pointer move asked for, before the move event returns.
    pub(super) fn begin_drag_out(&mut self, rt: &mut RuntimeCore) {
        let Some(start) = rt.take_drag_out() else { return };
        self.dirty = true;
        let (sink, path) = (rt.drag_end_sink(), start.path.clone());
        #[allow(irrefutable_let_patterns)]
        let Some(Live { backing: Backing::Window(window), config, scale, .. }) = self.surface.live() else {
            sink.borrow_mut().push((path, crate::DragOutcome::Cancelled));
            return;
        };
        let image = match (&self.gpu, &self.renderer) {
            (Some(gpu), Some(renderer)) if start.item.drag_image().is_none() =>
                crate::drag_out::snapshot(gpu, renderer, (config.width, config.height), *scale, rt.clear_color(), start.rect),
            _ => None,
        };
        let wake = self.shared.wake.lock().unwrap().clone();
        crate::drag_out::begin(window, start, image, Box::new(move |outcome| {
            sink.borrow_mut().push((path, outcome));
            if let Some(wake) = wake { wake(); }
        }));
        // The system drag swallows the button release, so let go of the panel's capture now
        // rather than after its missed-release grace: the panel turns click-through outside
        // its regions (so drops land on the windows below) and deferred input changes apply.
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.pointer_released(); }
        self.apply_input(rt);
        self.apply_cursor(rt);
    }

    /// A left press or release, at the panel's click point when it has one.
    pub(super) fn left_button(&mut self, app: &mut dyn OverlayApp, pressed: bool, point: Option<(f32, f32)>) {
        if pressed {
            if let Some((x, y)) = point { app.pointer_move(x, y); }
            if let Some((x, y)) = app.rt().pointer() {
                app.pointer_down(x, y);
                app.rt_mut().physical_input(crate::PhysicalInput::Modifiers(crate::window::map_mods(self.mods)));
            }
        } else if let Some((x, y)) = point.or(app.rt().pointer()) {
            // `pointer_up` moves to the release point itself, after disarming a drag-out: a
            // release never starts an OS drag.
            app.pointer_up(x, y);
        }
        self.dirty = true;
    }

    /// winit reported the pointer at (x, y).
    pub(super) fn cursor_moved(&mut self, app: &mut dyn OverlayApp, x: f32, y: f32) {
        self.dirty |= app.observe_pointer(x, y);
        self.begin_drag_out(app.rt_mut());
    }

    #[cfg(target_os = "macos")]
    pub(super) fn native_pointer(&self) -> Option<Rc<crate::platform::macos::PointerState>> {
        self.panel.as_ref().map(|panel| panel.pointer_state())
    }

    /// A release lost before the click a position belongs to ends first, so the press that lost
    /// it never hears that position.
    #[cfg(target_os = "macos")]
    pub(super) fn before_position(&mut self, app: &mut dyn OverlayApp, pointer: &crate::platform::macos::PointerState) {
        let lost = pointer.take_lost_before_position();
        self.release_lost(app, lost);
    }

    /// winit reported a position on the panel. With the button known up, the held press can't
    /// start an OS drag: the session needs the button held, and this is the Up's own position or
    /// one from before it.
    #[cfg(target_os = "macos")]
    pub(super) fn native_moved(&mut self, app: &mut dyn OverlayApp, pointer: &crate::platform::macos::PointerState, x: f32, y: f32) {
        self.before_position(app, pointer);
        if pointer.button_up() { app.rt_mut().disarm_drag_out(); }
        self.cursor_moved(app, x, y);
    }

    /// winit dispatched a left press or release on the panel.
    #[cfg(target_os = "macos")]
    pub(super) fn native_left(&mut self, app: &mut dyn OverlayApp, pointer: &crate::platform::macos::PointerState, pressed: bool) {
        let (lost, point) = pointer.take_edge(pressed);
        self.left_edge(app, lost, pressed, point);
    }

    /// A turn's pointer work once the panel has sampled: the position to report, if any.
    #[cfg(target_os = "macos")]
    pub(super) fn turn_position(&mut self, app: &mut dyn OverlayApp, pointer: &crate::platform::macos::PointerState) -> Option<(f32, f32)> {
        self.before_position(app, pointer);
        pointer.take_position()
    }

    /// Ends the runtime's press if the panel lost its release: the runtime hears no drop.
    #[cfg(target_os = "macos")]
    pub(super) fn release_lost(&mut self, app: &mut dyn OverlayApp, lost: Option<(f32, f32)>) {
        if let Some((x, y)) = lost && app.rt().pointer_captured() {
            app.pointer_release_lost(x, y);
            self.dirty = true;
        }
    }

    /// A left press or release from the panel. A release it already gave up on lands first, so
    /// it can't end this press, and a late Up can't turn it into a drop.
    #[cfg(target_os = "macos")]
    pub(super) fn left_edge(&mut self, app: &mut dyn OverlayApp, lost: Option<(f32, f32)>, pressed: bool, point: Option<(f32, f32)>) {
        self.release_lost(app, lost);
        if self.options.input.resolved(self.interactive) != OverlayInput::None { self.left_button(app, pressed, point); }
    }

    /// Delivers clipboard replies or secret-copy changes from the worker. A shown overlay steps
    /// them on its next drawn frame; a hidden or occluded one advances now.
    pub(super) fn clipboard_due(&mut self, app: &mut dyn OverlayApp) {
        if !self.clipboard.receive_core(app.rt_mut()) { return; }
        self.dirty = true;
        if self.frameless() { self.advance(app); }
    }

    /// Delivers drag ends a host reported. An `on_drag_start` may have hidden the overlay;
    /// its `on_drag_end` still runs.
    pub(super) fn drag_ends_due(&mut self, app: &mut dyn OverlayApp) {
        if !app.rt().drag_ends_pending() { return; }
        self.dirty = true;
        if self.frameless() { self.advance(app); }
    }

    /// True when a layer surface's input region changed and waits for a commit.
    pub(super) fn publish_regions(&mut self, rt: &RuntimeCore) -> bool {
        // An X11 window's input shape applies at once, with no commit to wait for.
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Xwayland(surface), .. }) = self.surface.live_mut() {
            surface.regions(rt.hit_regions());
            return false;
        }
        let Some(live) = self.surface.live() else { return false };
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_regions(rt.hit_regions()); }
        #[cfg(target_os = "windows")]
        if let Some(native) = &self.native { native.set_regions(rt.hit_regions(), live.scale); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.options.input.resolved(self.interactive) == OverlayInput::Regions && let Some(x11) = &mut self.x11 { x11.regions(rt.hit_regions(), live.scale); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Backing::Layer(layer) = &live.backing { return layer.input_regions(rt.hit_regions(), self.interactive); }
        let _ = (live, rt);
        false
    }

    pub(super) fn apply_input(&mut self, rt: &RuntimeCore) {
        if rt.pointer_captured() { return; }
        #[cfg(target_os = "macos")]
        if self.panel.as_ref().is_some_and(|panel| panel.pointer_captured()) { return; }
        let Some(input) = self.pending_input.take() else { return };
        self.options.input = input;
        let resolved = input.resolved(self.interactive);
        #[cfg(any(target_arch = "wasm32", target_os = "ios", target_os = "android"))]
        let _ = resolved;
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_input(resolved); }
        #[cfg(target_os = "windows")]
        if let Some(native) = &mut self.native { native.set_input(resolved); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            if let Some(x11) = &mut self.x11 { x11.set_input(resolved); }
            match self.surface.live_mut() {
                Some(Live { backing: Backing::Layer(layer), .. }) => layer.set_input(resolved),
                Some(Live { backing: Backing::Xwayland(surface), .. }) => surface.set_input(resolved),
                _ => {}
            }
        }
        self.publish_regions(rt);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Layer(layer), .. }) = self.surface.live_mut() { layer.commit(); }
    }

    pub(super) fn advance(&mut self, app: &mut dyn OverlayApp) { self.advance_at(app, crate::time::now()); }

    /// An overlay that draws nothing delivers queued input without drawing, such as the
    /// releases of the modifier keys held when it hid, rather than when it next shows.
    pub(super) fn hidden_input(&mut self, app: &mut dyn OverlayApp) {
        if !self.presence.drawing() && app.rt().keys_queued() { self.advance(app); }
    }

    /// Fn changes the monitor saw since the loop last turned. A hidden overlay drops them: its
    /// hide released Fn, and it takes no keys.
    #[cfg(target_os = "macos")]
    pub(super) fn fn_key(&mut self, app: &mut dyn OverlayApp) {
        let Some(changes) = self.surface.live().and_then(|l| l.fn_key.as_ref()).map(|f| f.take()) else { return };
        if !self.presence.shown() || self.options.keys != KeyPolicy::OnDemand { return; }
        for pressed in changes {
            self.dirty |= app.physical_with_fallback(crate::PhysicalInput::Key {
                key: winit::keyboard::KeyCode::Fn, mods: crate::window::map_mods(self.mods), pressed, repeat: false,
            }, None);
        }
    }

    /// Handle updates, and the `setup` waker (new data arrived) re-runs the view. A shown
    /// overlay draws a frame, which polls; a hidden one polls now, as for a due timer, and
    /// re-runs the view when it shows (`advance_at`).
    /// Updates that changed nothing (`update_if`) only deliver their requests.
    pub(super) fn apply_wake(&mut self, app: &mut dyn OverlayApp, woken: bool) {
        let (queued, updated) = app.drain_updates();
        self.apply_wake_drained(app, woken, queued, updated);
    }

    pub(super) fn apply_wake_drained(&mut self, app: &mut dyn OverlayApp, woken: bool, queued: bool, updated: bool) {
        if woken { app.rt_mut().invalidate(); }
        if queued || woken { self.clipboard.flush_core(app.rt_mut()); }
        if !woken && !updated {
            if !queued { return; }
            // A pop-up they asked for may be answered at once, running its callback: a change.
            let stale = app.rt().needs_commit();
            self.open_popup(app);
            if (stale || !app.rt().needs_commit()) && !app.rt().has_pending_input() {
                app.rt_mut().schedule_pending_wakes();
                return;
            }
            self.clipboard.flush_core(app.rt_mut());
        }
        self.dirty = true; // Queued updates already invalidate the cache before polling.
        if self.frameless() {
            // Register timer requests even before the first show; hidden or occluded overlays
            // don't draw.
            self.advance(app);
        }
    }

    /// Wayland layer-shell: true while a presented frame's `wl_surface.frame` callback is
    /// outstanding and the GPU device is still live, so the overlay must not acquire or present
    /// again until the compositor answers.
    pub(super) fn awaiting_layer_frame(&self) -> bool {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let surface_ready = match self.surface.live().map(|l| &l.backing) {
            Some(Backing::Layer(layer)) => layer.frame_ready(),
            _ => true,
        };
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        let surface_ready = true;
        #[cfg(test)]
        let surface_ready = surface_ready && !self.layer_frame_pending;
        let lost = self.gpu.as_ref().is_some_and(|gpu| gpu.is_lost());
        frame::is_awaiting_frame(surface_ready, lost)
    }

    /// Hidden, shown but not on screen, or awaiting a Wayland layer frame callback: no frame will
    /// come on its own to run what's due.
    pub(super) fn frameless(&self) -> bool {
        frame::is_frameless(self.presence.drawing(), self.presence.occluded(), self.awaiting_layer_frame())
    }

    pub(super) fn advance_at(&mut self, app: &mut dyn OverlayApp, now: Instant) {
        // Covered, asleep or awaiting a Wayland layer frame callback, a shown overlay only polls.
        // What it would commit waits for the next drawn frame, which resumes running motion and
        // starts new motion fresh (`step`). Hidden, an overlay only polls too, and its view waits
        // for the frame that shows it; a retained one commits at most every `HIDDEN_REFRESH` to
        // keep its cached frame near. Input and drag ends still run their handlers now.
        let handlers = app.rt().has_pending_input() || app.rt().drag_ends_pending();
        let polls_only = !handlers && if self.presence.drawing() {
            frame::polls_only_when_drawing(self.presence.shown(), self.presence.occluded(), self.awaiting_layer_frame())
        } else {
            !self.retained() || !self.hidden_pace.ready(now)
        };
        if !polls_only { return self.commit_at(app, now); }
        app.poll_hidden();
        if app.rt().has_pending_input() { return self.commit_at(app, now); }
        drop_window_requests(app.rt_mut());
        self.clipboard.flush_core(app.rt_mut());
        self.open_popup(app);
        if self.presence.drawing() {
            self.dirty = true;
        } else if app.rt().needs_commit() {
            self.dirty = true;
            if self.retained() { self.hidden_pace.changed(now); }
        }
    }

    /// Runs a frame without drawing: handlers, poll, the view and layout.
    pub(super) fn commit_at(&mut self, app: &mut dyn OverlayApp, now: Instant) {
        let commits = app.rt().stats.commits;
        if self.retained() {
            app.frame_split(now.saturating_duration_since(self.clock.last()).as_secs_f64(), 0.0);
            self.clock.hold(now);
        } else {
            app.frame(0.0);
        }
        drop_window_requests(app.rt_mut());
        self.clipboard.flush_core(app.rt_mut());
        self.open_popup(app);
        let rt = app.rt();
        if !self.presence.drawing() && rt.stats.commits != commits { self.hidden_pace.rendered(now); }
        // Even a transition that finished in this step needs its current pixels submitted.
        // A scheduling-only poll with no motion still preserves the prepared frame.
        if rt.stats.commits != commits || rt.animating || !self.idle {
            self.dirty = true;
            self.idle = false;
        }
    }

    pub(super) fn wake_due(&mut self, app: &mut dyn OverlayApp, now: Instant) {
        self.presence_due(app, now);
        // Paced to a frame interval: a hidden or covered overlay's poll that keeps asking for
        // the next frame runs once a frame, however often other events turn the loop.
        let deadline = frame::timer_deadline(app.rt(), self.awaiting_layer_frame());
        if deadline.is_some_and(|deadline| deadline <= now) {
            // Backing off, the frame waits for the retry and would leave the timer due.
            if !self.frameless() && !self.backing_off(now) {
                // Like the window host, poll once in the upcoming frame, after acquiring
                // the drawable. A false poll result does not keep the display loop alive.
                // Resting, it may draw nothing new (`Quiet`).
                self.quiet.step(frame::QuietEvent::TimerDue { resting: self.idle && !self.dirty });
                self.dirty = true;
            } else {
                // No frame will come to run it in time: a hidden, occluded or backing-off
                // overlay steps without drawing, so its timers fire (and may hide it) on time.
                self.advance_at(app, now);
                self.turn_frame.step(frame::TurnEvent::WakeStepped);
            }
        }
        // A hidden retained overlay's paced commit of what changed since its last.
        if !self.presence.drawing() && self.hidden_pace.take_due(now) { self.advance_at(app, now); }
        if self.backoff.deadline().is_some_and(|at| at <= now) {
            self.backoff_step(crate::window::BackoffEvent::Lapsed);
            self.dirty = true;
        }
    }

    /// A frame with no drawable still runs what it would have (updates, poll, due timers)
    /// without drawing, as a hidden overlay does, unless it already `stepped`. The next try
    /// waits, with the display link stopped so no tick tries sooner.
    pub(super) fn missed(&mut self, app: &mut dyn OverlayApp, missed: crate::window::Missed, paced: bool, stepped: bool) {
        self.missed_at(app, missed, paced, stepped, crate::time::now());
    }

    pub(super) fn missed_at(&mut self, app: &mut dyn OverlayApp, missed: crate::window::Missed, paced: bool, stepped: bool, now: Instant) {
        self.quiet.step(frame::QuietEvent::Forget);
        self.turn_frame.step(frame::TurnEvent::FrameMissed);
        if stepped {
            drop_window_requests(app.rt_mut());
            self.clipboard.flush_core(app.rt_mut());
        } else {
            self.advance_at(app, now);
        }
        self.backoff_step(crate::window::BackoffEvent::Missed { missed, paced, now });
        let pacing = self.pacing_state(missed != crate::window::Missed::Occluded);
        self.meter.set_pacing(pacing);
    }

    /// Steps the overlay's wait after a frame it couldn't draw, and carries out what that asks:
    /// a display link that would draw sooner than the wait stops.
    pub(super) fn backoff_step(&mut self, event: crate::window::BackoffEvent) {
        for effect in self.backoff.step(event) {
            match effect {
                crate::window::BackoffEffect::StopPacing => {
                    #[cfg(target_os = "macos")]
                    if let Some(pacing) = self.surface.live_mut().and_then(|l| l.pacing.as_mut()) { pacing.stop(); }
                }
            }
        }
    }

    /// Waiting for a missed frame's retry, or for the timer pacing unpaced motion.
    pub(super) fn backing_off(&self, now: Instant) -> bool { self.backoff.waiting(now) }

    /// What paces the overlay's frames, for its stats.
    pub(super) fn pacing_state(&self, moving: bool) -> crate::Pacing {
        #[cfg(target_os = "macos")]
        return crate::platform::display_link::pacing(self.surface.live().and_then(|l| l.pacing.as_ref()), moving);
        #[cfg(not(target_os = "macos"))]
        if moving { crate::Pacing::Vsync } else { crate::Pacing::Idle }
    }

    /// Applies requests from other threads: updates, edge, show and hide.
    pub(super) fn sync(&mut self, app: &mut dyn OverlayApp, event_loop: &ActiveEventLoop) {
        self.clipboard_due(app);
        if app.popup_answer(&mut self.popup) {
            self.dirty = true;
            if self.frameless() { self.advance(app); }
        }
        if app.rt().live_pending() { self.dirty = true; }
        self.drag_ends_due(app);
        let requests = std::mem::take(&mut *self.shared.requests.lock().unwrap());
        if let Some(input) = requests.input { self.pending_input = Some(input); }
        self.apply_input(app.rt());
        if let Some(exclude) = requests.exclude_from_capture { self.set_capture_exclusion(exclude); }
        if let Some(allow) = requests.xwayland_input { self.options.xwayland_input = allow; }
        if let Some(follow) = requests.follow_pointer {
            self.options.follow_pointer = follow;
            #[cfg(target_os = "macos")]
            if let Some(panel) = &self.panel { panel.set_follow_pointer(follow); }
            #[cfg(target_os = "windows")]
            if let Some(native) = &mut self.native { native.set_follow_pointer(follow); }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            if let Some(x11) = &self.x11 { x11.set_follow_pointer(follow); }
        }
        let options_changed = requests.placement.is_some() || requests.display.is_some();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let recreate_layer = should_recreate_layer(
            &self.options.display,
            &requests,
            matches!(self.surface.live().map(|l| &l.backing), Some(Backing::Layer(_))),
        );
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let layer_was_visible = self.presence.shown();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if recreate_layer { self.hide(app); }
        if let Some(placement) = requests.placement {
            self.options.placement = placement;
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            if let Some(Live { backing: Backing::Layer(layer), .. }) = self.surface.live_mut() {
                layer.set_placement(placement);
            }
        }
        if let Some(display) = requests.display { self.options.display = display; }
        self.apply_wake(app, requests.woken);
        let mut at = self.at;
        at.edge = requests.edge.unwrap_or(at.edge);
        at.margin = requests.margin.unwrap_or(at.margin);
        if let Some((align, inset)) = requests.align {
            (at.align, at.inset) = (align, inset);
        }
        let old_size = self.at.size;
        if let Some(size) = requests.size { self.canvas_size = size; }
        at.size = requests.size.unwrap_or(at.size);
        if at != self.at || options_changed {
            self.at = at;
            self.resolve_size(event_loop);
            if self.at.size != old_size {
                self.resized(app, event_loop);
            } else {
                match self.surface.live_mut().map(|l| &mut l.backing) {
                    Some(Backing::Window(w)) => {
                        let w = w.clone();
                        self.place_window(app.rt_mut(), event_loop, &w);
                        if let Some(gpu) = self.gpu.clone() { self.rescale(app, &gpu, crate::window::backing_scale(&w)); }
                    }
                    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                    Some(Backing::Layer(layer)) => {
                        layer.place(self.at);
                        layer.commit();
                    }
                    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                    Some(Backing::Xwayland(surface)) => surface.place(self.at, &self.options),
                    None => {}
                }
            }
        }
        if self.options.prewarm && self.shared.availability.load(Ordering::Acquire) == 0 { self.prepare(app, event_loop, false); }
        if let Some(level) = requests.macos_level {
            self.macos_level = Some(level);
            #[cfg(target_os = "macos")]
            if let Some(panel) = &self.panel { panel.set_level(level); }
        }
        match requests.visibility {
            Some(Visibility::Show(fade)) => self.show_fading(app, event_loop, fade),
            Some(Visibility::Hide(fade)) => self.hide_fading(app, fade),
            None => {}
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if recreate_layer && self.surface.live().is_none() {
            if layer_was_visible && !matches!(requests.visibility, Some(Visibility::Hide(_))) { self.show_fading(app, event_loop, Duration::ZERO); }
            else if self.options.prewarm { self.prepare(app, event_loop, false); }
        }
        if requests.refollow && self.presence.shown() && let Some(point) = Self::sample_pointer(event_loop) {
            self.refollow(app, event_loop, point);
        }
        self.apply_input(app.rt());
        if self.presence.shown() {
            #[cfg(target_os = "macos")]
            if let Some(panel) = &self.panel { self.shared.native.lock().unwrap().1.global_pointer = panel.global_pointer(); }
            #[cfg(target_os = "windows")]
            if let Some(native) = &self.native { self.shared.native.lock().unwrap().1.global_pointer = native.global_pointer(); }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            if let Some(x11) = &self.x11 { self.shared.native.lock().unwrap().1.global_pointer = x11.global_pointer(); }
        }
        #[cfg(target_os = "macos")]
        if requests.request_keys && self.presence.shown() && self.options.keys == KeyPolicy::OnDemand
            && let Some(panel) = &self.panel { panel.request_keys(); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if requests.request_keys && self.presence.shown() && self.options.keys == KeyPolicy::OnDemand
            && let Some(Live { backing: Backing::Layer(layer), .. }) = self.surface.live() { layer.request_keys(); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if requests.request_keys && self.presence.shown() && self.options.keys == KeyPolicy::OnDemand
            && let Some(x11) = &self.x11 { x11.request_keys(); }
        #[cfg(target_os = "windows")]
        if requests.request_keys && self.presence.shown() && self.options.keys == KeyPolicy::OnDemand
            && let Some(native) = &self.native { native.request_keys(); }
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
