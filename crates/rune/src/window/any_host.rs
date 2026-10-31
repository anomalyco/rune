//! The event loop's view of a host, whatever its state type.

use super::*;

impl<S: 'static> Host<S> {
    fn timer_deadline(&self) -> Option<Instant> {
        let runtime = timer_deadline(&self.rt);
        self.win.as_ref().map_or(runtime, |w| w.timer_frame.deadline(runtime))
    }
}

impl<S: 'static> AnyHost for Host<S> {
    fn owns(&self, id: WindowId) -> bool {
        self.win.as_ref().is_some_and(|w| w.window.id() == id)
    }
    fn finished(&self) -> bool { self.finished }
    fn keeps_loop(&self) -> bool { self.keeps_loop }
    fn terminate(&mut self) {
        // Once: a window that already closed ran its hook then.
        if self.finished || !self.closes() { return; }
        self.finished = true;
        self.close_hook();
        self.popup.cancel(&mut self.rt);
        self.file_dialog.cancel(&mut self.rt);
    }

    fn cancel_menus(&mut self) {
        self.popup.cancel(&mut self.rt);
        self.file_dialog.cancel(&mut self.rt);
    }

    fn reopen_candidate(&self) -> crate::platform::reopen::Candidate {
        let shown = self.win.as_ref().filter(|w| w.visible);
        let minimized = shown.is_some_and(|w| w.window.is_minimized() == Some(true));
        crate::platform::reopen::Candidate {
            reopenable: !self.finished && !self.macos_window.nonactivating_panel,
            visible: shown.is_some() && !minimized,
            minimized,
            hidden_at: self.hidden_at,
            main: self.keeps_loop,
        }
    }

    fn reopen_hook(&mut self, event_loop: &ActiveEventLoop, has_visible_windows: bool) -> bool {
        let Some(f) = self.on_reopen.filter(|_| self.started && !self.finished) else { return false };
        // As an update, so its frame, wakes, window requests and quit go as an update's do.
        let handled = Arc::new(AtomicBool::new(false));
        let answer = handled.clone();
        self.shared.queue_update(Box::new(move |s, tx| { answer.store(f(s, has_visible_windows, tx), Ordering::Relaxed); true }));
        self.drain(event_loop, false);
        self.publish();
        handled.load(Ordering::Relaxed)
    }

    fn apply_pending(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started || self.finished || !self.shared.window_pending() { return; }
        self.drain(event_loop, false);
        self.publish();
    }

    fn reopen(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started || self.finished { return; }
        self.open(event_loop, None);
        self.shell(event_loop);
        // winit's focus does nothing while the window is still deminiaturizing.
        #[cfg(target_os = "macos")]
        if let Some(w) = self.win.as_ref().filter(|w| w.visible) { crate::platform::reopen::front(&w.window); }
        self.publish();
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        #[cfg(target_os = "macos")]
        if let Some(shadow) = self.win.as_ref().and_then(|w| w.native.shadow.as_ref()) { shadow.turn(); }
        self.meter.turn(matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll), crate::frame::surface_stats::slept(cause));
        if matches!(cause, StartCause::ResumeTimeReached { .. }) && let Some(w) = &self.win { w.cursor.reassert(); }
        if let Some(w) = &mut self.win && let Some((retry, until)) = w.first_frame.deadline && retry <= Instant::now() {
            if until <= Instant::now() {
                let notified = w.first_frame.notified;
                if w.first_frame.expire() {
                    self.show_cached();
                    self.shell(event_loop);
                    if let Some(w) = &self.win { w.window.request_redraw(); }
                    self.publish();
                } else if !notified {
                    *self.shared.window_error.lock().unwrap() = Some("launcher first frame was not ready within two seconds".into());
                }
            } else {
                // Bounded retries while the frame prepares, whatever stops each one (occlusion,
                // no drawable). A retry left due would turn the loop at once until it gives up.
                w.first_frame.deadline = Some((Instant::now() + Duration::from_millis(16), until));
                w.window.request_redraw();
            }
        }
        let launcher = self.launcher();
        if let Some(w) = &mut self.win && w.retry.is_some_and(|retry| retry <= Instant::now()) {
            w.retry = None;
            // A missed frame marked it occluded; look at the surface again.
            w.occluded = false;
            if launcher || w.visible { w.window.request_redraw(); }
        }
        if let Some(w) = &mut self.win && !w.visible && w.hidden_pace.take_due(Instant::now()) {
            w.window.request_redraw();
        }
        if matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll)
            && self.timer_deadline().is_some_and(|deadline| deadline <= Instant::now())
        {
            let hidden = self.win.as_ref().is_none_or(|w| !w.visible);
            if self.win.as_mut().is_some_and(|w| w.timer_frame.poll_runtime(&mut self.rt)) {
                self.clipboard.flush(&mut self.rt);
                self.shell(event_loop);
            } else if hidden && !(launcher && self.hidden_launcher_renders()) {
                // Hidden or windowless: poll and due timers run, and the view waits for the
                // frame that shows the window. A launcher's cached frame follows, paced.
                let changed = self.rt.hidden_wake();
                self.clipboard.flush(&mut self.rt);
                self.shell(event_loop);
                if changed && launcher { self.refresh_hidden(); }
            // A redraw while the window backs off would draw nothing and leave the timer due.
            } else if self.win.as_ref().is_some_and(|w| (launcher || (w.visible && !w.occluded)) && !w.minimized()
                && !w.backing_off(Instant::now())) {
                // A resting window polls first and draws only for what that changed.
                let rests = self.idle && !composing(self.comp.as_deref()) && self.win.as_ref().is_some_and(Win::rests);
                let frame = match rests.then(|| self.rt.resting_wake()).flatten() {
                    Some(stale) => {
                        self.clipboard.flush(&mut self.rt);
                        self.shell(event_loop);
                        // A close the poll asked for may have hidden the window.
                        let frame = (stale || self.rt.stale()) && self.win.as_ref().is_some_and(|w| w.visible);
                        self.rt.resting_wake_frame(frame);
                        frame
                    }
                    None => true,
                };
                if frame && let Some(w) = &mut self.win {
                    if self.wayland { w.timer_frame.requested(Instant::now()); }
                    w.quiet = true;
                    w.window.request_redraw();
                }
            } else {
                covered_step(&mut self.rt);
                self.clipboard.flush(&mut self.rt);
                self.shell(event_loop);
            }
        }
    }

    fn wake_deadline(&self) -> Option<Instant> {
        // The next display tick consumes due timers too. Leaving an overdue timer
        // armed while its redraw waits for that tick would spin the event loop. Without a
        // running link, a timer due sooner than a frame waits a frame (`timer_deadline`).
        let runtime = self.timer_deadline().filter(|_| !self.win.as_ref().is_some_and(Win::paced));
        let win = self.win.as_ref();
        runtime.into_iter()
            .chain(win.and_then(|w| w.first_frame.deadline.map(|(retry, _)| retry)))
            .chain(win.and_then(|w| w.first_frame.ready_deadline()))
            .chain(win.and_then(|w| w.retry))
            .chain(win.filter(|w| !w.visible).and_then(|w| w.hidden_pace.deadline()))
            .chain(self.files.pointer.as_ref().filter(|_| win.is_some_and(|w| w.visible)).map(|_| Instant::now() + DROP_SAMPLE))
            .chain(win.filter(|w| w.visible).and_then(|w| w.cursor.reassert_at(Instant::now())))
            .min()
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) -> bool {
        #[cfg(target_os = "macos")]
        self.fn_key();
        self.flush_files(event_loop);
        // Another window switched the font families (`Tx::set_font_families`).
        if self.rt.follow_font_families() && let Some(w) = &self.win { w.window.request_redraw(); }
        // Process queued hide/close requests before completing a pending first reveal.
        self.first_frame_ready(event_loop);
        if !self.win.as_ref().is_some_and(|w| w.visible || w.first_frame.reveal) {
            self.rt.refuse_file_dialog("cannot open a file dialog while the window is hidden");
        }
        self.hidden_input(event_loop);
        false
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.resume(event_loop);
        self.publish();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        self.meter.waited();
        self.handle_user_event(event_loop, event);
        self.publish();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if !matches!(event, WindowEvent::RedrawRequested) { self.meter.waited(); }
        self.handle_window_event(event_loop, id, event);
        self.publish();
    }
}
