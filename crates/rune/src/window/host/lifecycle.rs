//! A window's lifecycle: requests from other threads, showing, focusing, draining updates,
//! polling while hidden, closing, hiding, opening, creating, sight cover transitions and rest.

use std::sync::atomic::Ordering;

use winit::dpi::PhysicalPosition;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;
use winit::window::WindowLevel;

use super::Host;
use crate::app::{Activation, WindowRequest};
use crate::el::Shell;
use crate::window::chrome::{self, sync_chrome_core};
use crate::window::file_drop::FileDrop;
use crate::window::handle::native_window_number;
use crate::window::is_wayland;
use crate::window::state::minimize::{HiddenPoll, shell_take_core};
use crate::window::state::{
    Cover, Found, Life, MinimizeEvent, RestEffect, RestEvent, Reveal, RevealEffect, WakeFacts,
    WakeWork, rest,
};
use crate::window::surface::{BackoffEvent, Win};
#[cfg(target_os = "windows")]
use crate::window::win32::covered_draw::CoveredDrawEvent;

impl Host {
    pub(in crate::window) fn window_request(&mut self, event_loop: &ActiveEventLoop, request: WindowRequest) {
        match request {
            WindowRequest::Open(token) => self.open(event_loop, token),
            WindowRequest::Show => self.show(event_loop),
            WindowRequest::Hide => self.hide(event_loop),
            WindowRequest::Close => self.close(event_loop),
        }
    }

    pub(in crate::window) fn set_position(&mut self, event_loop: &ActiveEventLoop, position: (i32, i32)) {
        self.placement = None;
        self.options.position = Some(position);
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_outer_position(PhysicalPosition::new(position.0, position.1));
        }
        self.sync_origin();
    }

    pub(in crate::window) fn set_always_on_top(&mut self, event_loop: &ActiveEventLoop, on: bool) {
        self.options.always_on_top = on;
        self.macos_window.level = None;
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_window_level(if on { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
        }
    }

    pub(in crate::window) fn show(&mut self, event_loop: &ActiveEventLoop) {
        if self.launcher() { self.reveal_launcher(event_loop); return; }
        self.reveal(event_loop, Reveal::Show, None);
    }

    pub(in crate::window) fn open(&mut self, event_loop: &ActiveEventLoop, token: Option<String>) {
        if self.launcher() { self.reveal_launcher(event_loop); return; }
        // The launcher's startup token belongs to the first window only.
        let token = token.or_else(|| self.startup_token(event_loop));
        self.reveal(event_loop, Reveal::Open, token);
    }

    /// The window shown, or not, so far: the shown one, and nothing while it's hidden.
    pub(in crate::window) fn shown_win(&self) -> Option<&Win> { self.win.as_ref().filter(|_| self.life.shown()) }

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
    pub(in crate::window) fn reveal(&mut self, event_loop: &ActiveEventLoop, reveal: Reveal, mut token: Option<String>) {
        if self.life.finished() { return; }
        let mut built = false;
        for effect in reveal.effects(self.found()) {
            match effect {
                RevealEffect::ReadSystem => {
                    let rt = self.app.rt_mut();
                    rt.set_system_accent(crate::system_accent());
                    rt.set_system_scroll_bars_state(crate::platform::scroll_bars::current().with_override(self.args.scroll_bars));
                }
                RevealEffect::Build => built = self.build_for_show(),
                RevealEffect::ApplyFrame => {
                    if self.take_frame_request(self.frame_secs(crate::time::now()), false) {
                        self.idle = false;
                    }
                }
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
                RevealEffect::OrderIn { focus } => if let Some(w) = &mut self.win { w.show(focus) },
                RevealEffect::Unminimize => if let Some(w) = &self.win { w.window.set_minimized(false) },
                RevealEffect::Settle => {
                    if !built { self.app.rt_mut().settle_all(); }
                    self.idle = true;
                }
                RevealEffect::Shown { live } => {
                    self.life.show();
                    self.app.set_live_visible(live);
                }
                RevealEffect::CancelTimerFrame => self.app.rt_mut().cancel_timer_frame(),
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

    /// Focuses a shown window; Wayland and X11 use the open's startup token for that.
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
        } else if let Some(token) = token.as_deref()
            && crate::platform::x11::activation::activate(&w.window, token)
        {
            return;
        }
        let _ = (event_loop, token);
        w.focus();
    }

    /// A hidden or released window that fits its content builds what changed while it was
    /// away and takes its new height before it is ordered in (`Runtime::build_for_show`).
    /// Returns whether it built, settled.
    pub(in crate::window) fn build_for_show(&mut self) -> bool {
        if !self.app.build_for_show() { return false; }
        self.clipboard.flush_core(self.app.rt_mut());
        // macOS sets the frame and resizes the surface at once; elsewhere `Resized` brings it.
        self.take_frame_request(0.0, false);
        true
    }

    pub(in crate::window) fn focus(&self, event_loop: &ActiveEventLoop) {
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
    pub(in crate::window) fn drain(&mut self, event_loop: &ActiveEventLoop, wake: bool) {
        let shown = self.shown_win().is_some();
        let stale_before = self.app.rt().needs_commit();
        let clipboard = self.clipboard.receive_core(self.app.rt_mut());
        let answered = self.app.popup_answer(&mut self.popup) | self.file_dialog.answer_core(self.app.rt_mut());
        let (requests, had_updates, updated) = self.app.drain_window(&self.shortcuts, &mut self.menus);
        let changed = clipboard || answered || updated || self.app.rt().drag_ends_pending();
        // Only updates that changed nothing (`AppHandle::update_if`), or an empty startup drain
        // after `try_create` already drew the first frame, ran: no poll, no frame.
        let quiet = (had_updates || (!wake && !stale_before)) && !changed && requests.only_updates()
            && !self.app.rt().patch_pending() && (stale_before || !self.app.rt().needs_commit()) && !self.app.compositor_animating();
        self.clipboard.flush_core(self.app.rt_mut());
        // The next frame polls for the wake.
        if wake && !quiet { self.app.rt_mut().cancel_timer_frame(); }
        if let Some(after) = requests.quit {
            // Quitting from now on: a close later in this turn closes for the quit.
            // `Windows::exiting` carries out its effects.
            let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
            // A tracking menu holds the event loop until it ends.
            self.app.popup_cancel(&mut self.popup);
            self.app.file_dialog_cancel(&mut self.file_dialog);
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
        if requests.activation == Some(Activation::Minimize) { self.app.rt_mut().minimize_step(MinimizeEvent::Asked); }
        self.shell(event_loop);
        // A pop-up or file dialog a quiet update asked for may have been answered at once.
        let answered = quiet && ((!stale_before && self.app.rt().needs_commit()) || self.app.rt().has_pending_input());
        if quiet { self.clipboard.flush_core(self.app.rt_mut()); }
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
        if !polled && !framed { self.app.rt_mut().schedule_pending_wakes(); }
    }

    /// A hidden, keep-running or windowless window still reads its inbox when woken: `poll` runs
    /// with no drawable, present or animation, and its changes render when the window is next
    /// shown. A hidden launcher renders poll's changes hidden, paced, keeping its cached frame
    /// fresh. A minimize waits for a show in the same turn (`Minimize`). Returns whether it
    /// polled.
    pub(in crate::window) fn poll_hidden(&mut self, event_loop: &ActiveEventLoop, shows: bool) -> bool {
        if event_loop.exiting() || self.shown_win().is_some() { return false; }
        let changed = self.app.poll_hidden();
        self.clipboard.flush_core(self.app.rt_mut());
        // A show this turn minimizes after it: this poll holds the ask across its own shell, and
        // a callback's ask during that shell is the one it takes first.
        let poll = HiddenPoll::new(shows);
        poll.enter_core(self.app.rt_mut());
        self.shell(event_loop);
        poll.leave_core(self.app.rt_mut());
        if changed && self.launcher() { self.refresh_hidden(); }
        true
    }

    /// A hidden launcher renders what changed into its cached frame: at once after a quiet
    /// spell, then at most every `HIDDEN_REFRESH` (`HiddenPace`). A show of state newer than
    /// that frame renders it hidden first (`reveal_launcher`), as for an update in the same
    /// turn. A frame being prepared, or one a reveal waits on, renders at once. Returns whether
    /// it asked for a frame.
    pub(in crate::window) fn refresh_hidden(&mut self) -> bool {
        let now_due = self.hidden_launcher_renders();
        let Some(w) = &mut self.win else { return false };
        let render = now_due || w.first_frame.deadline().is_some() || w.hidden_pace.changed(crate::time::now());
        if render { w.request_redraw(); }
        render
    }

    /// What a hidden launcher's frame must render as it commits: queued input and drag ends.
    /// Committed without a render, they'd leave a cached frame that looks fresh.
    pub(in crate::window) fn hidden_launcher_renders(&self) -> bool {
        self.app.rt().has_pending_input() || self.app.rt().drag_ends_pending()
    }

    /// Carries out what a handler or update asked with `Tx::close_window` / `Tx::quit`, then
    /// opens the pop-up menu it asked for, if the window is still there to open it over.
    pub(in crate::window) fn shell(&mut self, event_loop: &ActiveEventLoop) {
        self.show_title();
        if self.app.rt_mut().take_toggle_maximized() && let Some(w) = self.shown_win() {
            chrome::toggle_maximized(&w.window);
        }
        if self.app.rt_mut().take_toggle_full_screen() && let Some(w) = self.shown_win() {
            chrome::toggle_full_screen(&w.window);
        }
        if let Some(menu) = self.app.rt_mut().take_window_menu() && let Some(w) = self.shown_win() {
            chrome::window_menu(&w.window, menu, None);
        }
        // A new `Cx::set_titlebar` or `Cx::titlebar_height` resizes the caption buttons and the band
        // the frame hit-tests, and moves the traffic lights.
        if self.app.rt_mut().take_titlebar_changed() && let Some(w) = &self.win {
            sync_chrome_core(self.app.rt_mut(), w, self.chrome, self.titlebar);
            if self.app.rt().stale() { w.request_redraw(); }
        }
        for _ in 0..8 {
            let minimize = shell_take_core(self.app.rt_mut());
            match self.app.rt_mut().shell_request.take() {
                Some(shell @ (Shell::Quit | Shell::Relaunch)) => {
                    let after = shell.after().unwrap_or(crate::quit::After::Exit);
                    let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
                    self.app.popup_cancel(&mut self.popup);
                    self.app.file_dialog_cancel(&mut self.file_dialog);
                    event_loop.exit();
                }
                Some(Shell::Close) => self.close(event_loop),
                None if minimize => self.minimize(),
                None => {}
            }
            if self.life.finished() || event_loop.exiting() {
                self.app.popup_cancel(&mut self.popup);
                self.app.file_dialog_cancel(&mut self.file_dialog);
                return;
            }
            let paints = self.shown_win().is_some_and(|w| !self.life.covered() && !w.minimized());
            // A painting macOS window moves its frame over its next frames (`redraw`).
            if self.app.rt().host.window_frame.is_some() && !(paints && cfg!(target_os = "macos")) {
                let resized = self.take_frame_request(self.frame_secs(crate::time::now()), false);
                if resized && !self.life.shown() && self.launcher() && let Some(w) = &mut self.win {
                    // The hidden surface resized, invalidating its old-size cached pixels; draw at
                    // the new height before any pending reveal orders it in.
                    w.first_frame.missed();
                    w.request_redraw();
                }
            }
            let shown = self.win.as_ref().filter(|_| self.life.shown());
            let window = shown.map(|w| &w.window);
            if self.app.popup_open(&mut self.popup, window, paints) && let Some(w) = shown { w.request_redraw(); }
            if self.file_dialog.open_core(self.app.rt_mut(), window.map(|w| &**w)) && let Some(w) = shown { w.request_redraw(); }
            // A callback that ran (a menu that couldn't open) may have asked to close, quit or
            // minimize.
            if self.app.rt().shell_request.is_none() && !self.app.rt().host.minimize.field() { return; }
        }
    }

    pub(in crate::window) fn close(&mut self, event_loop: &ActiveEventLoop) {
        self.app.rt_mut().minimize_step(MinimizeEvent::Lost);
        if let Some(w) = &mut self.win {
            w.first_frame.hide();
            #[cfg(target_os = "macos")]
            { w.native.prime.stop(); }
        }
        self.secure_input.set(false);
        if !self.closes() { return; }
        crate::marks::mark("window.close");
        // The last window of an app that ends fast goes as a bare window's close does: its hook,
        // then the window destroyed at once, with no hide before it. The quit runs after it and
        // the process ends without dropping the window.
        let ends = std::mem::take(&mut self.ends_app) && !self.keep_running && cfg!(target_os = "windows");
        // Off the screen first: the close looks done at once and `on_close` runs unseen.
        if !ends && let Some(w) = self.shown_win() { w.hide(); }
        crate::marks::mark("window.close.hidden");
        self.close_hook();
        crate::marks::mark("window.close.hooked");
        #[cfg(target_os = "windows")]
        if ends && let Some(w) = &mut self.win {
            w.native.drop_target = None;
            crate::platform::win32::destroy_window_now(&w.window);
            crate::marks::mark("window.destroyed");
        }
        self.app.set_live_visible(false);
        // While the window still shows its last frames.
        if let Some(trace) = &mut self.trace { trace.settle_presents(crate::frame::present_time::SETTLE); }
        if let Some(after) = self.app.rt_mut().shell_request.take().and_then(Shell::after) {
            if after == crate::quit::After::Relaunch {
                let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
            }
            self.app.popup_cancel(&mut self.popup);
            self.app.file_dialog_cancel(&mut self.file_dialog);
            event_loop.exit();
            return;
        }
        if !self.keep_running {
            self.app.popup_cancel(&mut self.popup);
            self.app.file_dialog_cancel(&mut self.file_dialog);
            self.app.rt_mut().cancel_pointer();
            self.files = FileDrop::default();
            if !ends && let Some(w) = &mut self.win { w.apply_cursor_core(self.app.rt(), false); w.hide(); }
            crate::marks::mark("window.hidden");
            self.life.finish();
            return;
        }
        self.hide(event_loop);
    }

    /// Whether a close runs `on_close` (`Life::closes`).
    pub(in crate::window) fn closes(&self) -> bool { self.life.closes(self.keep_running, self.win.is_some()) }

    pub(in crate::window) fn close_hook(&mut self) {
        self.app.run_on_close();
        // Before the window hides or the app exits: a lock or clear must reach the worker.
        self.clipboard.flush_core(self.app.rt_mut());
    }

    pub(in crate::window) fn hide(&mut self, event_loop: &ActiveEventLoop) {
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
    pub(in crate::window) fn stop_showing(&mut self) {
        self.life.hide(crate::platform::reopen::stamp);
        self.app.rt_mut().minimize_step(MinimizeEvent::Lost);
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
    pub(in crate::window) fn wind_down(&mut self) {
        self.app.rt_mut().keys_lost();
        if let Some(w) = &mut self.win {
            w.first_frame.hide();
            #[cfg(target_os = "macos")]
            { w.native.prime.stop(); }
        }
        #[cfg(target_os = "windows")]
        self.look_stale();
        self.secure_input.set(false);
        self.ime.clear();
        if let Some(w) = &self.win { w.window.set_ime_allowed(false); }
        self.app.set_live_visible(false);
        self.app.rt_mut().cancel_timer_frame();
        self.app.popup_cancel(&mut self.popup);
        self.app.file_dialog_cancel(&mut self.file_dialog);
        self.app.rt_mut().cancel_pointer();
        self.files = FileDrop::default();
        if let Some(w) = &mut self.win { w.apply_cursor_core(self.app.rt(), false); }
        self.mods = ModifiersState::empty();
        if let Some(w) = &self.win { w.hide(); }
    }

    /// Gives up the window and the device and surface it drew with, as `hide` does for an app
    /// that releases its GPU when closed. The next `open` makes a window, a device and a surface
    /// (an app that sees `AppHandle::window_error` and the window gone can ask for one).
    pub(in crate::window) fn release(&mut self) {
        self.win = None;
        self.native_handle.native.ready.store(false, Ordering::Release);
        self.native_handle.native.number.store(0, Ordering::Release);
        self.gpu = None;
        let rt = self.app.rt_mut();
        rt.text.atlas.invalidate();
        rt.images.invalidate();
    }

    /// Creates the window, or records why it can't be and leaves the app windowless; a later
    /// open tries again.
    pub(in crate::window) fn create(&mut self, event_loop: &ActiveEventLoop, token: Option<String>, focus: bool) {
        let result = self.try_create(event_loop, token, focus);
        if let Err(error) = &result {
            eprintln!("rune: could not create the window {}: {error}", self.native_handle.key());
        }
        self.shared.set_window_error(result.err());
    }

    /// `cover` starts or stops covering the window. Covered, it stops pacing and rests, and is
    /// no longer live; uncovered, its first frame paints the settled state of what changed
    /// meanwhile (guarantee 8).
    pub(in crate::window) fn set_cover(&mut self, cover: Cover, on: bool) {
        let Some(l) = self.win.as_mut() else { return };
        #[cfg(target_os = "macos")]
        { l.native.prime.stop(); }
        let was = self.life.cover(cover, on);
        l.backoff_step(BackoffEvent::Uncovered);
        l.presented = None;
        let live = self.life.uncovered() && !l.minimized();
        if self.life.covered() {
            self.secure_input.set(false);
            l.stop_pacing();
            self.idle = true;
            self.app.set_live_visible(live);
            self.app.rt_mut().cancel_timer_frame();
            self.rest_step(RestEvent::Unseen);
            return;
        }
        // Its first frame uncovered draws what a covered one owed.
        #[cfg(target_os = "windows")]
        { l.covered_draw = l.covered_draw.step(CoveredDrawEvent::Drawn); }
        self.app.set_live_visible(live);
        let Some(l) = self.win.as_mut() else { return };
        sync_chrome_core(self.app.rt_mut(), l, self.chrome, self.titlebar);
        // A window opened while the displays slept has no display link yet.
        #[cfg(target_os = "macos")]
        if l.native.pacing.is_none() { l.native.pacing = crate::platform::display_link::DisplayLink::new(&l.window, self.frame_wake.clone()); }
        if was {
            self.app.rt_mut().settle_all();
            l.request_redraw();
        } else if self.app.rt().stale() {
            l.request_redraw();
        }
    }

    pub(in crate::window) fn rest_step(&mut self, event: RestEvent) {
        for effect in rest::step(&mut self.rest, event) {
            match effect {
                RestEffect::GiveBackUnused => self.give_back(false),
                RestEffect::GiveBackAll => self.give_back(true),
            }
        }
    }

    fn give_back(&mut self, all: bool) {
        {
            let rt = self.app.rt_mut();
            rt.text.rest();
            rt.images.rest();
            if let Some(rewind) = rt.motion.rewind() { rewind.rest(); }
        }
        self.app.run_on_rest();
        if let Some((gpu, renderer)) = &mut self.gpu {
            renderer.give_back(all, &mut self.app.rt_mut().text.atlas.resident_caches);
            // Out of sight, what the last frame used goes too, and the staging chunks: an empty
            // submission and a wait land those frees now. At an idle rest the device isn't
            // polled: a poll costs the next two frames' submits 4-9 ms each on an Iris Xe.
            if all {
                gpu.queue.submit(std::iter::empty());
                rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok();
                renderer.trim_uploads();
            }
        }
    }
}
