//! A window's lifecycle: requests from other threads, showing, focusing, draining updates,
//! polling while hidden, closing, hiding, opening and creating.

use super::*;
use super::minimize::{HiddenPoll, shell_take};

impl<S: 'static> Host<S> {
    pub(super) fn window_request(&mut self, event_loop: &ActiveEventLoop, request: WindowRequest) {
        match request {
            WindowRequest::Open(token) => self.open(event_loop, token),
            WindowRequest::Show => self.show(event_loop),
            WindowRequest::Hide => self.hide(event_loop),
            WindowRequest::Close => self.close(event_loop),
        }
    }

    pub(super) fn set_position(&mut self, event_loop: &ActiveEventLoop, position: (i32, i32)) {
        self.placement = None;
        self.options.position = Some(position);
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_outer_position(PhysicalPosition::new(position.0, position.1));
        }
        self.sync_origin();
    }

    pub(super) fn set_always_on_top(&mut self, event_loop: &ActiveEventLoop, on: bool) {
        self.options.always_on_top = on;
        self.macos_window.level = None;
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_window_level(if on { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
        }
    }

    pub(super) fn show(&mut self, event_loop: &ActiveEventLoop) {
        if self.launcher() { self.reveal_launcher(event_loop); return; }
        self.reveal(event_loop, Reveal::Show, None);
    }

    pub(super) fn open(&mut self, event_loop: &ActiveEventLoop, token: Option<String>) {
        if self.launcher() { self.reveal_launcher(event_loop); return; }
        // The launcher's startup token belongs to the first window only.
        let token = token.or_else(|| self.startup_token(event_loop));
        self.reveal(event_loop, Reveal::Open, token);
    }

    /// The window shown, or not, so far: the shown one, and nothing while it's hidden.
    pub(super) fn shown_win(&self) -> Option<&Win> { self.win.as_ref().filter(|_| self.life.shown()) }

    /// The window as a reveal finds it.
    fn found(&self) -> Found {
        let Some(w) = &self.win else { return Found::Missing };
        let minimized = w.window.is_minimized() == Some(true);
        match self.life {
            Life::Shown { sight } => Found::Shown { covered: sight.covered(), minimized },
            _ => Found::Hidden { minimized },
        }
    }

    /// Brings the window on screen (`Reveal`); a window closed for good stays closed.
    pub(super) fn reveal(&mut self, event_loop: &ActiveEventLoop, reveal: Reveal, mut token: Option<String>) {
        if self.life.finished() { return; }
        let mut built = false;
        for effect in reveal.effects(self.found()) {
            match effect {
                RevealEffect::ReadSystem => {
                    self.rt.set_system_accent(crate::system_accent());
                    self.rt.set_system_scroll_bars_state(crate::platform::scroll_bars::current().with_override(self.args.scroll_bars));
                }
                RevealEffect::Build => built = self.build_for_show(),
                RevealEffect::Place => self.apply_placement(event_loop),
                RevealEffect::Create => {
                    // `App::open_inactive`: the first open leaves focus where it is.
                    let focus = reveal == Reveal::Open && !(self.open_inactive && !self.shown);
                    self.create(event_loop, token.take(), focus);
                    // On macOS a new window becomes key without activating the app, so activate
                    // it like a reopen does. The launch open leaves activation to the platform.
                    #[cfg(target_os = "macos")]
                    if focus && self.started && let Some(w) = self.shown_win() { w.focus(); }
                }
                RevealEffect::OrderIn { focus } => if let Some(w) = &self.win { w.show(focus) },
                RevealEffect::Unminimize => if let Some(w) = &self.win { w.window.set_minimized(false) },
                RevealEffect::Settle => {
                    if !built { self.rt.settle_all(); }
                    self.idle = true;
                }
                RevealEffect::Shown { live } => {
                    self.life.show();
                    self.rt.set_live_visible(live);
                }
                RevealEffect::CancelTimerFrame => self.rt.cancel_timer_frame(),
                RevealEffect::Number => if let Some(w) = &self.win {
                    self.native_handle.native.number.store(native_window_number(&w.window).unwrap_or(0), Ordering::Release);
                },
                RevealEffect::Focus => self.activate(event_loop, token.take()),
                RevealEffect::Redraw => if let Some(w) = &self.win { w.request_redraw() },
                // No settle, frame, surface configure or acquisition on a cached reveal, unless
                // the refreshed frame left motion running.
                RevealEffect::RedrawIfMoving => if !self.idle && let Some(w) = &self.win { w.request_redraw() },
            }
        }
    }

    /// The first window's startup token (`XDG_ACTIVATION_TOKEN`, `DESKTOP_STARTUP_ID`), which
    /// later windows don't inherit.
    fn startup_token(&self, event_loop: &ActiveEventLoop) -> Option<String> {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if !self.shown {
            use winit::platform::startup_notify::{EventLoopExtStartupNotify, reset_activation_token_env};
            let token = event_loop.read_token_from_env().map(|t| t.into_raw());
            reset_activation_token_env();
            return token;
        }
        let _ = event_loop;
        None
    }

    /// Focuses a shown window; Wayland's compositor wants the open's token for that.
    fn activate(&mut self, event_loop: &ActiveEventLoop, token: Option<String>) {
        let Some(w) = self.win.as_mut() else { return };
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if is_wayland(event_loop) {
            match token {
                Some(token) => {
                    if w.native.activator.is_none() {
                        w.native.activator = crate::platform::activation::Activator::new(&w.window);
                    }
                    if let Some(a) = w.native.activator.as_mut() {
                        a.activate(&token);
                    }
                }
                // Without a token the compositor may only mark the window urgent.
                None => w.window.request_user_attention(Some(winit::window::UserAttentionType::Informational)),
            }
            return;
        }
        let _ = (event_loop, token);
        w.focus();
    }

    /// A hidden or released window that fits its content builds what changed while it was
    /// away and takes its new height before it is ordered in (`Runtime::build_for_show`).
    /// Returns whether it built, settled.
    pub(super) fn build_for_show(&mut self) -> bool {
        if !self.rt.build_for_show() { return false; }
        self.clipboard.flush(&mut self.rt);
        self.take_frame_request(0.0, false);
        // macOS sets the frame at once: the first drawable and layout take the new size.
        // Elsewhere the window's `Resized` brings it.
        #[cfg(target_os = "macos")]
        if let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) {
            let size = w.window.inner_size();
            w.resize_surface(Some(&**gpu), size);
            let scale = w.window.scale_factor() as f32;
            self.rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
        }
        true
    }

    pub(super) fn focus(&self, event_loop: &ActiveEventLoop) {
        let Some(w) = self.shown_win() else { return };
        if w.window.is_minimized() == Some(true) { w.window.set_minimized(false); }
        if is_wayland(event_loop) {
            w.window.request_user_attention(Some(winit::window::UserAttentionType::Informational));
            return;
        }
        w.focus();
    }

    /// Applies everything other threads asked for since the last wake-up. A `wake` polls a
    /// hidden window first, so a reveal in the same turn sees what `poll` changed.
    pub(super) fn drain(&mut self, event_loop: &ActiveEventLoop, wake: bool) {
        let shown = self.shown_win().is_some();
        let stale_before = self.rt.needs_commit();
        let clipboard = self.clipboard.receive(&mut self.rt);
        let answered = self.popup.answer(&mut self.rt) | self.file_dialog.answer(&mut self.rt);
        self.shortcuts.drain();
        self.menus.drain();
        let (mut had_updates, mut updated) = (false, false);
        let requests = self.shared.drain(|f| {
            had_updates = true;
            updated |= self.rt.update_if(f);
            self.menus.drain();
        });
        let changed = clipboard || answered || updated || self.rt.drag_ends_pending();
        // Only updates that changed nothing (`AppHandle::update_if`), or an empty startup drain
        // after `try_create` already drew the first frame, ran: no poll, no frame.
        let quiet = (had_updates || (!wake && !stale_before)) && !changed && requests.only_updates()
            && !self.rt.patch_pending() && (stale_before || !self.rt.needs_commit()) && !composing(self.comp.as_deref());
        self.clipboard.flush(&mut self.rt);
        // The next frame polls for the wake.
        if wake && !quiet { self.rt.cancel_timer_frame(); }
        if let Some(after) = requests.quit {
            // Quitting from now on: a close later in this turn closes for the quit.
            // `Windows::exiting` carries out its effects.
            let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
            // A tracking menu holds the event loop until it ends.
            self.popup.cancel(&mut self.rt);
            self.file_dialog.cancel(&mut self.rt);
            event_loop.exit();
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(visible) = requests.dock {
            crate::platform::macos::set_dock_icon(visible);
        }
        let shows = matches!(requests.window, Some(WindowRequest::Show | WindowRequest::Open(_)));
        let polled = wake && !quiet && self.poll_hidden(event_loop, shows);
        if let Some(position) = requests.position { self.set_position(event_loop, position); }
        if let Some(place) = requests.place { self.place(event_loop, place); }
        if let Some(request) = requests.window { self.window_request(event_loop, request); }
        if requests.activation == Some(Activation::Focus) { self.focus(event_loop); }
        // After the show it may follow; `focus` and `minimize` are last-wins (`AppHandle`).
        if requests.activation == Some(Activation::Minimize) { self.rt.minimize_step(MinimizeEvent::Asked); }
        self.shell(event_loop);
        // A pop-up or file dialog a quiet update asked for may have been answered at once.
        let answered = quiet && ((!stale_before && self.rt.needs_commit()) || self.rt.has_pending_input());
        if quiet { self.clipboard.flush(&mut self.rt); }
        let (changed, quiet) = (changed || answered, quiet && !answered);
        // A launcher's cached reveal draws nothing; a hidden one renders its changes hidden,
        // paced (`refresh_hidden`). A covered or minimised one runs them without asking for a
        // frame it wouldn't draw.
        let work = WakeWork::of(
            WakeFacts {
                window: self.win.is_some(),
                launcher: self.launcher(),
                visible: self.life.shown(),
                was_shown: shown,
                changed,
                quiet,
                covered: self.life.covered(),
            },
            || self.win.as_ref().is_some_and(Win::minimized),
        );
        let framed = match work {
            WakeWork::Nothing => false,
            WakeWork::HiddenFrame => self.refresh_hidden(),
            WakeWork::CoveredFrame => {
                self.covered_frame();
                self.shell(event_loop);
                true
            }
            WakeWork::Paint => {
                if let Some(w) = &self.win { w.request_redraw(); }
                true
            }
        };
        // Neither a poll nor a frame comes to schedule the updates' `wake_after`s.
        if !polled && !framed { self.rt.schedule_pending_wakes(); }
    }

    /// A hidden, keep-running or windowless window still reads its inbox when woken: `poll` runs
    /// with no drawable, present or animation, and its changes render when the window is next
    /// shown. A hidden launcher renders poll's changes hidden, paced, keeping its cached frame
    /// fresh. A minimize waits for a show in the same turn (`Minimize`). Returns whether it
    /// polled.
    pub(super) fn poll_hidden(&mut self, event_loop: &ActiveEventLoop, shows: bool) -> bool {
        if event_loop.exiting() || self.shown_win().is_some() { return false; }
        let changed = self.rt.poll_hidden();
        self.clipboard.flush(&mut self.rt);
        // A show this turn minimizes after it: this poll holds the ask across its own shell, and
        // a callback's ask during that shell is the one it takes first.
        let poll = HiddenPoll::new(shows);
        poll.enter(&mut self.rt);
        self.shell(event_loop);
        poll.leave(&mut self.rt);
        if changed && self.launcher() { self.refresh_hidden(); }
        true
    }

    /// A hidden launcher renders what changed into its cached frame: at once after a quiet
    /// spell, then at most every `HIDDEN_REFRESH` (`HiddenPace`). A show of state newer than
    /// that frame renders it hidden first (`reveal_launcher`), as for an update in the same
    /// turn. A frame being prepared, or one a reveal waits on, renders at once. Returns whether
    /// it asked for a frame.
    pub(super) fn refresh_hidden(&mut self) -> bool {
        let now_due = self.hidden_launcher_renders();
        let Some(w) = &mut self.win else { return false };
        let render = now_due || w.first_frame.deadline().is_some() || w.hidden_pace.changed(Instant::now());
        if render { w.request_redraw(); }
        render
    }

    /// What a hidden launcher's frame must render as it commits: queued input and drag ends.
    /// Committed without a render, they'd leave a cached frame that looks fresh.
    pub(super) fn hidden_launcher_renders(&self) -> bool {
        self.rt.has_pending_input() || self.rt.drag_ends_pending()
    }

    /// Carries out what a handler or update asked with `Tx::close_window` / `Tx::quit`, then
    /// opens the pop-up menu it asked for, if the window is still there to open it over.
    pub(super) fn shell(&mut self, event_loop: &ActiveEventLoop) {
        self.show_title();
        if self.rt.take_toggle_maximized() && let Some(w) = self.shown_win() {
            chrome::toggle_maximized(&w.window);
        }
        if self.rt.take_toggle_full_screen() && let Some(w) = self.shown_win() {
            chrome::toggle_full_screen(&w.window);
        }
        if let Some(menu) = self.rt.take_window_menu() && let Some(w) = self.shown_win() {
            chrome::window_menu(&w.window, menu, None);
        }
        // A new `Cx::set_titlebar` or `Cx::titlebar_height` resizes the caption buttons and the band
        // the frame hit-tests, and moves the traffic lights.
        if self.rt.take_titlebar_changed() && let Some(w) = &self.win {
            sync_chrome(&mut self.rt, w, self.chrome, self.titlebar);
            if self.rt.stale() { w.request_redraw(); }
        }
        for _ in 0..8 {
            let minimize = shell_take(&mut self.rt);
            match self.rt.shell_request.take() {
                Some(shell @ (Shell::Quit | Shell::Relaunch)) => {
                    let after = shell.after().unwrap_or(crate::quit::After::Exit);
                    let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
                    self.popup.cancel(&mut self.rt);
                    self.file_dialog.cancel(&mut self.rt);
                    event_loop.exit();
                }
                Some(Shell::Close) => self.close(event_loop),
                None if minimize => self.minimize(),
                None => {}
            }
            if self.life.finished() || event_loop.exiting() {
                self.popup.cancel(&mut self.rt);
                self.file_dialog.cancel(&mut self.rt);
                return;
            }
            let paints = self.shown_win().is_some_and(|w| !self.life.covered() && !w.minimized());
            // A painting macOS window moves its frame over its next frames (`redraw`).
            if self.rt.window_frame.is_some() && !(paints && cfg!(target_os = "macos")) {
                self.take_frame_request(self.frame_secs(Instant::now()), false);
            }
            let shown = self.win.as_ref().filter(|_| self.life.shown());
            let window = shown.map(|w| &w.window);
            if self.popup.open(&mut self.rt, window, paints) && let Some(w) = shown { w.request_redraw(); }
            if self.file_dialog.open(&mut self.rt, window.map(|w| &**w)) && let Some(w) = shown { w.request_redraw(); }
            // A callback that ran (a menu that couldn't open) may have asked to close, quit or
            // minimize.
            if self.rt.shell_request.is_none() && !self.rt.minimize.field() { return; }
        }
    }

    pub(super) fn close(&mut self, event_loop: &ActiveEventLoop) {
        self.rt.minimize_step(MinimizeEvent::Lost);
        if let Some(w) = &mut self.win {
            w.first_frame.hide();
            #[cfg(target_os = "macos")]
            { w.native.prime.stop(); }
        }
        self.secure_input.set(false);
        if !self.closes() { return; }
        crate::marks::mark("window.close");
        // Off the screen first: the close looks done at once and `on_close` runs unseen.
        if let Some(w) = self.shown_win() { w.hide(); }
        crate::marks::mark("window.close.hidden");
        self.close_hook();
        crate::marks::mark("window.close.hooked");
        self.rt.set_live_visible(false);
        // While the window still shows its last frames.
        if let Some(trace) = &mut self.trace { trace.settle_presents(crate::frame::present_time::SETTLE); }
        if let Some(after) = self.rt.shell_request.take().and_then(Shell::after) {
            if after == crate::quit::After::Relaunch {
                let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
            }
            self.popup.cancel(&mut self.rt);
            self.file_dialog.cancel(&mut self.rt);
            event_loop.exit();
            return;
        }
        if !self.keep_running {
            self.popup.cancel(&mut self.rt);
            self.file_dialog.cancel(&mut self.rt);
            self.rt.cancel_pointer();
            self.files = FileDrop::default();
            if let Some(w) = &mut self.win { w.apply_cursor(&self.rt, false); w.hide(); }
            crate::marks::mark("window.hidden");
            self.life.finish();
            return;
        }
        self.hide(event_loop);
    }

    /// Whether a close runs `on_close` (`Life::closes`).
    pub(super) fn closes(&self) -> bool { self.life.closes(self.keep_running, self.win.is_some()) }

    pub(super) fn close_hook(&mut self) {
        if let Some(f) = self.on_close {
            self.rt.update(f);
        }
        // Before the window hides or the app exits: a lock or clear must reach the worker.
        self.clipboard.flush(&mut self.rt);
    }

    pub(super) fn hide(&mut self, event_loop: &ActiveEventLoop) {
        self.rest_step(RestEvent::Unseen);
        self.stop_showing();
        self.wind_down();
        let launcher = self.launcher();
        if self.release_gpu && !launcher {
            self.release();
        } else if is_wayland(event_loop) {
            // Wayland has no hidden toplevel: unmap by destroying, recreate on open.
            self.win = None;
            self.native_handle.native.ready.store(false, Ordering::Release);
            self.native_handle.native.number.store(0, Ordering::Release);
        } else if let Some(w) = self.win.as_mut() {
            w.stop_pacing();
            w.backoff_step(BackoffEvent::Lapsed);
            w.presented = None;
            // Its hidden frame delivers the focus loss.
            if launcher { w.request_redraw(); }
        }
    }

    /// What a window does just before it stops drawing: it leaves `Life` (a window that hid can
    /// be reopened), drops a pending minimize, lands a frame move where it was going, and (macOS)
    /// delivers the Fn key change a hide need not report. No event loop is needed, so a window
    /// whose surface can't be replaced does the same before `wind_down`
    /// (`Host::replace_lost_surface`).
    pub(super) fn stop_showing(&mut self) {
        self.life.hide(crate::platform::reopen::stamp);
        self.rt.minimize_step(MinimizeEvent::Lost);
        self.land_frame();
        // Before the window can go: a hide need not report a blur. `about_to_wait` delivers it,
        // after any Fn change the monitor saw first.
        #[cfg(target_os = "macos")]
        self.fn_key();
    }

    /// What a window stops doing when it hides: keys released, secure input and IME off, popups
    /// and file dialogs cancelled, the pointer cancelled, drops dropped, modifiers reset, the
    /// cursor back to normal, and the native window hidden. No event loop is needed, so a window
    /// that loses its surface does the same from `draw` (`Host::replace_lost_surface`).
    pub(super) fn wind_down(&mut self) {
        self.rt.keys_lost();
        if let Some(w) = &mut self.win {
            w.first_frame.hide();
            #[cfg(target_os = "macos")]
            { w.native.prime.stop(); }
        }
        self.secure_input.set(false);
        self.ime_allowed = false;
        if let Some(w) = &self.win { w.window.set_ime_allowed(false); }
        self.rt.set_live_visible(false);
        self.rt.cancel_timer_frame();
        self.popup.cancel(&mut self.rt);
        self.file_dialog.cancel(&mut self.rt);
        self.rt.cancel_pointer();
        self.files = FileDrop::default();
        if let Some(w) = &mut self.win { w.apply_cursor(&self.rt, false); }
        self.mods = ModifiersState::empty();
        if let Some(w) = &self.win { w.hide(); }
    }

    /// Gives up the window and the device and surface it drew with, as `hide` does for an app
    /// that releases its GPU when closed. The next `open` makes a window, a device and a surface
    /// (an app that sees `AppHandle::window_error` and the window gone can ask for one).
    pub(super) fn release(&mut self) {
        self.win = None;
        self.native_handle.native.ready.store(false, Ordering::Release);
        self.native_handle.native.number.store(0, Ordering::Release);
        self.gpu = None;
        self.rt.text.atlas.invalidate();
        self.rt.images.invalidate();
    }

    /// Creates the window, or records why it can't be and leaves the app windowless; a later
    /// open tries again.
    pub(super) fn create(&mut self, event_loop: &ActiveEventLoop, token: Option<String>, focus: bool) {
        let result = self.try_create(event_loop, token, focus);
        if let Err(error) = &result {
            eprintln!("rune: could not create the window {}: {error}", self.native_handle.key());
        }
        *self.shared.window_error.lock().unwrap() = result.err();
    }
}
