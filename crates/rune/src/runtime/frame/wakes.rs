//! Clocks, timer wake deadlines, viewport updates and native window focus transitions.
use super::*;

impl RuntimeCore {
    /// Clock seconds now: the wall clock in a window (input arrives between frames), the
    /// virtual clock headless.
    pub(crate) fn now_clock(&self) -> f64 {
        #[cfg(test)]
        if let Some(now) = self.frame.test_clock_now() { return now; }
        self.wall_start.map_or(self.clock, |t| t.elapsed().as_secs_f64())
    }

    /// The window's size in its points and its scale. The view lays out in them over its zoom
    /// ([`Cx::zoom`]), at the scale times the zoom.
    pub fn set_viewport(&mut self, viewport: (f32, f32), scale: f32) {
        (self.host.zoom.window, self.host.zoom.window_scale) = (viewport, scale);
        let (viewport, scale) = self.host.zoom.view();
        if self.fit_resized(viewport.1) {
            self.commit.relayout = true;
            self.commit.set_viewport_stepped(false);
        }
        if self.change_viewport(viewport, scale) { self.commit.set_viewport_stepped(false); }
    }

    /// The viewport one frame of the window's own resize animation moved it to. Unlike a
    /// user's resize it doesn't land running motion: content at rest follows the window and
    /// springs keep going.
    pub(crate) fn step_viewport(&mut self, viewport: (f32, f32), scale: f32) {
        (self.host.zoom.window, self.host.zoom.window_scale) = (viewport, scale);
        let (viewport, scale) = self.host.zoom.view();
        let fresh = !self.commit.relayout;
        if self.change_viewport(viewport, scale) && fresh { self.commit.set_viewport_stepped(true); }
    }

    /// Whether the change needs a layout.
    pub(in crate::runtime) fn change_viewport(&mut self, viewport: (f32, f32), scale: f32) -> bool {
        // A window that fits its content lays out the same at any height: its own resize
        // mustn't cut short the layout springs it is following.
        if self.host.fit_height && viewport.0 == self.viewport.0 && scale == self.env.scale {
            self.viewport = viewport;
            return false;
        }
        let scale_changed = scale != self.env.scale;
        let changed = viewport != self.viewport || scale_changed;
        if changed {
            if scale_changed {
                self.motion.clear_flights();
                self.commit.step_feedback(layout_read::FeedbackEvent::Reopened);
            }
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
        if focused { self.kbd.keys_lost = false; self.kbd.focus_lost_sent = false; }
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
            // Back to the front: a field's keyboard-entry ring stays off until the keys move focus.
            if focused && self.host.window_focused == Some(false) && self.kbd.focus.entered_by_keys() {
                self.kbd.focus.step(crate::runtime::input::focus_machine::FocusEvent::WindowReturned);
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
        if !focused { self.edit.clear_input_click(); self.wheel_lost(); }
    }

    /// The last focus change the window reported was a gain.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn window_has_focus(&self) -> bool { self.host.window_focused == Some(true) }

    /// The window gets no more keys: it lost focus or hid, which may never report a blur. The
    /// focused physical handler hears `FocusLost` and held modifier keys are released, once
    /// until the window has focus again. A hidden host steps `hidden_timer_frame` while
    /// `keys_queued` to deliver them.
    pub(crate) fn keys_lost(&mut self) {
        if std::mem::replace(&mut self.kbd.keys_lost, true) { return; }
        self.physical_input(crate::PhysicalInput::FocusLost);
    }

    /// Keyboard input, a focus change, a file drag step, a file dialog answer or an
    /// `open_url_then` result is queued for the next frame. A browser that comes to the front
    /// can hide the window, and its result still has to reach the app.
    pub(crate) fn keys_queued(&self) -> bool {
        self.frame.queue.iter().any(|a| matches!(a, Action::Physical(..) | Action::Key { .. } | Action::KeyUp(_) | Action::WindowFocus(_) | Action::Files(_) | Action::FileDialog(..) | Action::UrlOpened(..)))
    }

    /// Host time on the logical clock.
    pub(in crate::runtime) fn wall_clock(&self) -> Option<f64> {
        #[cfg(test)]
        if let Some(now) = self.frame.test_clock_now() { return Some(now); }
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
    pub(in crate::runtime) fn schedule_wakes(&mut self, wakes: impl IntoIterator<Item = std::time::Duration>) {
        self.schedule_wakes_from(self.clock, wakes);
    }

    /// Wakes for a frame once the clock reaches `due`, as [`Tx::wake_at`] asks from a handler (a
    /// deadline already past wakes the next frame), for the runtime's own timers.
    #[cfg_attr(feature = "frame-profile", track_caller)]
    pub(in crate::runtime) fn wake_at(&mut self, due: f64) {
        #[cfg(feature = "frame-profile")]
        crate::frame::live_log::waker(std::panic::Location::caller(), std::time::Duration::from_secs_f64((due - self.clock).max(0.0)));
        self.schedule_wakes_from(due, [std::time::Duration::ZERO]);
    }

    #[inline]
    pub(in crate::runtime) fn schedule_wakes_from(&mut self, now: f64, wakes: impl IntoIterator<Item = std::time::Duration>) {
        self.frame.schedule_wakes_from(now, wakes);
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
        self.frame.polled_at(self.clock)
    }

    pub(in crate::runtime) fn logical_wake_deadline(&self) -> Option<f64> {
        self.poll_wake_deadline().into_iter().chain(self.motion.loop_wake().map(|w| w.0)).min_by(f64::total_cmp)
    }

    /// The next due timer of `poll`'s (or an armed touch hold), on `wake_deadline`'s clock.
    pub(crate) fn poll_wake_deadline(&self) -> Option<f64> {
        self.frame.next_wake_deadline().into_iter().chain(self.pointer.touch_hold_deadline()).min_by(f64::total_cmp)
    }

    /// The next frame motion asks for at its own cadence (a loop's next step, a video's next
    /// picture), on `wake_deadline`'s clock.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn motion_wake_deadline(&self) -> Option<f64> {
        self.motion.loop_wake().map(|w| w.0)
    }
}
