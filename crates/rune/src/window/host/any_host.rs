//! The event loop's view of a host, whatever its state type.

use std::sync::atomic::Ordering;
use std::time::Duration;

use winit::event::{StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::{Host, WindowApp};
use crate::time::Instant;
use crate::window::file_drop::DROP_SAMPLE;
use crate::window::state::{Cover, Life, RestEvent, Reveal};
use crate::window::surface::{BackoffEvent, Redraw, RedrawEffect, RedrawEvent, Win, timer_deadline_core};
#[cfg(target_os = "windows")]
use crate::window::win32::covered_draw::CoveredDrawEvent;
use crate::window::{AnyHost, WakeEvent};

impl AnyHost for Host {
    fn owns(&self, id: WindowId) -> bool {
        self.win.as_ref().is_some_and(|w| w.window.id() == id)
    }
    fn has_live_wayland_layer(&self) -> bool {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        return self.wayland
            && self
                .win
                .as_ref()
                .and_then(|w| w.native.wayland_backdrop.as_ref())
                .is_some_and(|b| b.needs_wake_on_read());
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        false
    }
    fn has_pending_platform_events(&self) -> bool {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        return self.wayland
            && self
                .win
                .as_ref()
                .and_then(|w| w.native.wayland_backdrop.as_ref())
                .is_some_and(|b| b.has_pending_events());
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        false
    }
    fn finished(&self) -> bool { self.life.finished() }
    fn has_shown(&self) -> bool { self.shown }
    fn take_startup_error(&mut self) -> Option<String> { self.startup_error.take() }
    fn keeps_loop(&self) -> bool { self.keeps_loop }
    fn closing_ends_app(&mut self) { self.ends_app = true; }
    fn key(&self) -> Option<&str> { Some(self.native_handle.key()) }
    fn reopen_candidate(&self) -> crate::platform::reopen::Candidate {
        let shown = self.shown_win();
        let minimized = shown.is_some_and(Win::is_minimized_native);
        crate::platform::reopen::Candidate {
            reopenable: !self.life.finished() && !self.macos_window.nonactivating_panel && !self.no_activate(),
            visible: shown.is_some() && !minimized,
            minimized,
            hidden_at: self.life.hidden_at(),
            main: self.keeps_loop,
        }
    }

    fn wake_deadline(&self) -> Option<Instant> {
        if self.gpu_loss_failed() { return None; }
        // The next display tick consumes due timers too. Leaving an overdue timer
        // armed while its redraw waits for that tick would spin the event loop. Without a
        // running link, a timer due sooner than a frame waits a frame (`timer_deadline`).
        let now = crate::time::now();
        let win = self.win.as_ref();
        // A frame waiting for room in the swap chain comes when there is (`slot`): its timers
        // would turn the loop at once until then.
        let runtime = self.poll_deadline().filter(|_| !win.is_some_and(|w| w.paced() || w.waits_for_slot()));
        // A due timer whose frame is still on its way takes the frame's bound instead
        // (`TimerFrame`), so the loop sleeps rather than waking at once for a frame that can't
        // come.
        let shown = self.shown_win();
        let runtime = shown.map_or(runtime, |w| w.timer_frame.wait(runtime, now));
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let handover = win.and_then(|w| w.handover_deadline(now));
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        let handover = None;
        #[cfg(target_os = "windows")]
        let covered_draw = win.and_then(|w| w.covered_draw.deadline());
        #[cfg(not(target_os = "windows"))]
        let covered_draw = None;
        #[cfg(target_os = "windows")]
        let resize_sync = win.and_then(|w| w.native.resize_sync.deadline());
        #[cfg(not(target_os = "windows"))]
        let resize_sync = None;
        runtime.into_iter()
            .chain(handover)
            .chain(covered_draw)
            .chain(resize_sync)
            .chain(win.and_then(|w| w.first_frame.deadline().map(|(retry, _)| retry)))
            .chain(win.and_then(|w| w.first_frame.ready_deadline()))
            .chain(win.and_then(Win::cloak_deadline))
            .chain(win.and_then(|w| w.backoff.deadline()))
            .chain(win.filter(|_| shown.is_none()).and_then(|w| w.hidden_pace.deadline()))
            .chain(look_deadline(win, self.life))
            .chain(self.files.pointer.as_ref().filter(|_| shown.is_some()).map(|_| now + DROP_SAMPLE))
            .chain(shown.and_then(|w| w.cursor.reassert_at(now)))
            .chain(self.rest.deadline())
            .chain(wayland_drop_deadline(win))
            .chain(self.window_script.as_ref().and_then(|s| s.next_at()))
            .min()
    }

    fn turn_ends(&mut self, event_loop: &ActiveEventLoop, now: Instant) { self.in_turn(|h| h.turn_ends_in_turn(event_loop, now)) }

    // What reaches the app's state runs in its turn scope (`App::turn_scope`).
    fn terminate(&mut self) { self.in_turn(Self::terminate_in_turn) }
    fn has_quit_request(&self) -> bool { !self.life.finished() && self.app.has_quit_request() }
    fn quit_request_hook(&mut self, event_loop: &ActiveEventLoop) -> Option<crate::QuitResponse> {
        self.in_turn(|h| h.quit_request_in_turn(event_loop))
    }
    fn quit_hook(&mut self) { self.in_turn(Self::quit_hook_in_turn) }
    fn vanish(&mut self) {
        if let Some(w) = self.shown_win() { w.hide(); }
    }
    fn cancel_menus(&mut self) { self.in_turn(Self::cancel_menus_in_turn) }
    #[cfg(target_os = "macos")]
    fn stop_pacing(&mut self) {
        if let Some(w) = &mut self.win { w.stop_pacing(); }
    }
    fn reopen_hook(&mut self, event_loop: &ActiveEventLoop, has_visible_windows: bool) -> bool {
        self.in_turn(|h| h.reopen_hook_in_turn(event_loop, has_visible_windows))
    }
    fn apply_pending(&mut self, event_loop: &ActiveEventLoop) { self.in_turn(|h| h.apply_pending_in_turn(event_loop)) }
    fn reopen(&mut self, event_loop: &ActiveEventLoop) { self.in_turn(|h| h.reopen_in_turn(event_loop)) }
    fn apply_app_color_scheme(&mut self) { self.apply_window_color_scheme(); }
    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
    fn on_material_policy_changed(&mut self) { self.in_turn(Self::on_material_policy_changed_in_turn) }
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) { self.in_turn(|h| h.new_events_in_turn(event_loop, cause)) }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) -> bool { self.in_turn(|h| h.about_to_wait_in_turn(event_loop)) }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) { self.in_turn(|h| h.resumed_in_turn(event_loop)) }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) { self.in_turn(|h| h.user_event_in_turn(event_loop, event)) }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.in_turn(|h| h.window_event_in_turn(event_loop, id, event))
    }
    fn pointer_motion(&mut self, dx: f64, dy: f64) {
        if !self.app.rt().pointer_locked() { return; }
        self.in_turn(|h| h.pointer_motion_in_turn(dx, dy));
    }
}

/// Whether a host's turn is inside its app's turn scope now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum Turn {
    Between,
    Within,
}

impl Host {
    /// Windows: reconciles `El::system_material` native vs fallback state after a system effects
    /// policy notification (Transparency effects, Energy/Battery Saver, High Contrast, RDP, or
    /// `CompositionCapabilities`).
    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
    fn on_material_policy_changed_in_turn(&mut self) {
        let contrast = crate::platform::preferences::contrast();
        let mut effects = crate::system_material::windows::current_policy();
        effects.high_contrast |= contrast.is_some();
        self.app.rt_mut().set_system_contrast(contrast);
        if let Some(l) = self.win.as_mut() {
            let native = &mut l.native;
            let redraw = native.materials.on_policy_changed(effects, native.mode.handover_mut());
            let now_native = native.materials.native();
            self.app.rt_mut().paint.set_native_materials(now_native);
            if redraw || self.app.rt().stale() {
                l.timer_frame.no_longer_quiet();
                l.loop_frame.touched();
                l.request_redraw();
            }
        }
    }

    /// Raw relative mouse motion while pointer lock is active and this window has focus.
    fn pointer_motion_in_turn(&mut self, dx: f64, dy: f64) {
        if self.poll_gpu_loss() { return; }
        let Some(w) = self.shown_win() else { return };
        if !self.app.rt().pointer_locked() || !w.window.has_focus() {
            return;
        }
        let [mx, my] = crate::window::pointer_lock::scale_motion(
            dx,
            dy,
            w.window.scale_factor(),
            crate::window::pointer_lock::MotionPlatform::for_host(self.wayland),
        );
        if !mx.is_finite() || !my.is_finite() || (mx == 0.0 && my == 0.0) {
            return;
        }
        let _input = crate::input::time::arrived_now();
        self.app.rt_mut().pointer_motion(mx, my);
        if let Some(w) = self.win.as_mut() {
            w.timer_frame.no_longer_quiet();
            w.loop_frame.touched();
            w.request_redraw();
        }
    }

    /// The loop's turn ends: the frame the window wants next is asked for, and on Windows a
    /// window whose paint the system keeps putting off behind another's paints now (`Redraw`).
    fn turn_ends_in_turn(&mut self, event_loop: &ActiveEventLoop, now: Instant) {
        if self.poll_gpu_loss() { return; }
        let Some(w) = &self.win else { return };
        let paints = cfg!(target_os = "windows") && self.life.uncovered() && !w.minimized();
        let (next, effect) = w.redraw.get().step(RedrawEvent::TurnEnds { now, paints });
        w.redraw.set(next);
        match effect {
            RedrawEffect::None => {}
            RedrawEffect::Request => w.window.request_redraw(),
            RedrawEffect::PaintNow => {
                #[cfg(target_os = "windows")]
                crate::platform::win32::drop_paint(&w.window);
                self.paint(event_loop);
            }
        }
        // Guard: never let the event loop rest with user input still queued for a drawable
        // window when neither a redraw nor an active display link is already on its way.
        if let Some(w) = self.shown_win()
            && queued_input_needs_redraw(
                self.app.rt().has_input_waiting(),
                self.life.uncovered() && !w.minimized() && !w.backing_off(now) && !w.waits_for_slot(),
                w.paced(),
                w.redraw.get(),
            )
        {
            w.request_redraw();
        }
        #[cfg(target_os = "windows")]
        self.sync_poll_pace(now);
    }

    /// Windows: whether the window is shown, uncovered, not minimized and not waiting on a
    /// swap-chain slot, so poll-driven frames pace to the display's vblank grid.
    #[cfg(target_os = "windows")]
    fn poll_pace_active(&self) -> bool {
        self.shown_win().is_some_and(|w| self.life.uncovered() && !w.minimized() && !w.waits_for_slot())
    }

    /// Windows: syncs the window's poll pacer with the runtime's current poll wake before the
    /// loop sleeps.
    #[cfg(target_os = "windows")]
    fn sync_poll_pace(&mut self, now: Instant) {
        let active = self.poll_pace_active();
        let rt = self.app.rt();
        let Some(w) = self.win.as_mut() else { return };
        let window = &w.window;
        w.native.poll_pace.sync_with(rt, now, active, |previous| {
            let mhz = window.current_monitor().and_then(|m| m.refresh_rate_millihertz());
            crate::platform::win32::vsync_grid(now, mhz, previous)
        });
    }

    /// The runtime's next timer as the loop waits for it and checks it on wake. On Windows, a
    /// shown uncovered window's sub-frame poll wake snaps to the display's vblank grid
    /// (`poll_pace`) instead of the 60 Hz `TIMER_FRAME` floor.
    fn poll_deadline(&self) -> Option<Instant> {
        #[cfg(target_os = "windows")]
        if let Some(w) = &self.win {
            return w.native.poll_pace.deadline_for(self.app.rt(), self.poll_pace_active());
        }
        timer_deadline_core(self.app.rt())
    }

    /// Runs `f` inside the app's turn scope, entered once however deep the turn's calls go.
    fn in_turn<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        if self.turn != Turn::Between || !self.app.enter_turn() { return f(self); }
        self.turn = Turn::Within;
        let result = f(self);
        self.app.leave_turn();
        self.turn = Turn::Between;
        result
    }

    fn terminate_in_turn(&mut self) {
        // Once: a window that already closed ran its hook then.
        if !self.closes() { return; }
        self.life.finish();
        self.close_hook();
        self.app.popup_cancel(&mut self.popup);
        self.app.file_dialog_cancel(&mut self.file_dialog);
    }

    fn quit_hook_in_turn(&mut self) {
        if quit_hook_core(&mut *self.app, &mut self.clipboard, self.life) && let Some(w) = self.shown_win() {
            w.request_redraw();
        }
    }

    fn cancel_menus_in_turn(&mut self) {
        self.app.popup_cancel(&mut self.popup);
        self.app.file_dialog_cancel(&mut self.file_dialog);
    }

    fn reopen_hook_in_turn(&mut self, event_loop: &ActiveEventLoop, has_visible_windows: bool) -> bool {
        if !self.started || self.life.finished() || self.poll_gpu_loss() { return false; }
        // As an update, so its frame, wakes, window requests and quit go as an update's do.
        let Some(handled) = self.app.queue_on_reopen(has_visible_windows) else { return false };
        self.drain(event_loop, false);
        self.publish();
        handled.load(Ordering::Relaxed)
    }

    fn apply_pending_in_turn(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started || self.life.finished() || (!self.shared.asks_window() && !self.app.has_updates()) { return; }
        self.drain(event_loop, false);
        self.publish();
    }

    fn reopen_in_turn(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started || self.life.finished() || self.poll_gpu_loss() { return; }
        self.open(event_loop, None);
        self.shell(event_loop);
        // winit's focus does nothing while the window is still deminiaturizing.
        #[cfg(target_os = "macos")]
        if let Some(w) = self.shown_win() { crate::platform::reopen::front(&w.window); }
        self.publish();
    }

    fn new_events_in_turn(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if self.poll_gpu_loss() { return; }
        #[cfg(target_os = "macos")]
        {
            if let Some(shadow) = self.win.as_ref().and_then(|w| w.native.shadow.as_ref()) { shadow.turn(); }
            self.frame_presented.drain();
        }
        self.meter.turn(matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll), crate::frame::surface_stats::slept(cause));
        if matches!(cause, StartCause::ResumeTimeReached { .. }) && let Some(w) = &self.win { w.cursor.reassert(); }
        if self.window_script.is_some() { self.replay_window_script(crate::time::now()); }
        #[cfg(target_os = "windows")]
        if let Some(w) = &self.win { w.native.cloak.lift_if_due(&w.window, crate::time::now()); }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        self.watch_handover(crate::time::now());
        #[cfg(target_os = "windows")]
        self.watch_resize_sync(crate::time::now());
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(attempt) = self.win.as_ref().and_then(|w| w.native.drop_binding.grace_attempt())
            && self.win.as_ref().and_then(|w| w.native.drop_binding.deadline()).is_some_and(|due| due <= crate::time::now())
        {
            self.step_wayland_drop_binding(crate::platform::wayland::drop::binding::Event::GraceDue {
                attempt,
                now: crate::time::now(),
            });
        }
        #[cfg(target_os = "windows")]
        if let Some(w) = &mut self.win && w.covered_draw.due(crate::time::now()) {
            w.covered_draw = w.covered_draw.step(CoveredDrawEvent::Due);
            w.request_redraw();
        }
        if let Some(w) = &mut self.win && let Some((retry, until)) = w.first_frame.deadline() && retry <= crate::time::now() {
            if until <= crate::time::now() {
                let notified = w.first_frame.notified();
                if w.first_frame.expire() {
                    self.reveal(event_loop, Reveal::Cached, None);
                    self.shell(event_loop);
                    if let Some(w) = &self.win { w.request_redraw(); }
                    self.publish();
                } else if !notified {
                    self.shared.set_window_error(Some("launcher first frame was not ready within two seconds".into()));
                }
            } else {
                // Bounded retries while the frame prepares, whatever stops each one (occlusion,
                // no drawable). A retry left due would turn the loop at once until it gives up.
                w.first_frame.retry_at(crate::time::now() + Duration::from_millis(16));
                w.request_redraw();
            }
        }
        let launcher = self.launcher();
        if let Some(w) = &mut self.win && w.backoff.deadline().is_some_and(|at| at <= crate::time::now()) {
            w.backoff_step(BackoffEvent::Lapsed);
            // A missed frame marked it occluded; look at the surface again.
            self.life.cover(Cover::Surface, false);
            if launcher || self.life.shown() { w.request_redraw(); }
        }
        self.rest_step(RestEvent::Woke { now: crate::time::now() });
        if let Some(w) = &mut self.win && !self.life.shown() && w.hidden_pace.take_due(crate::time::now()) {
            w.request_redraw();
        }
        // A covered window with motion on screen looks again whether it's still covered (`look`),
        // without a frame: asked for while covered, one would draw (`CoveredDraw`) for no one.
        #[cfg(target_os = "windows")]
        if look_deadline(self.win.as_ref(), self.life).is_some_and(|at| at <= crate::time::now()) {
            self.look();
        }
        if matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll)
            && self.poll_deadline().is_some_and(|deadline| deadline <= crate::time::now())
            && !self.win.as_ref().is_some_and(Win::waits_for_slot)
        {
            let hidden = self.shown_win().is_none();
            if hidden && !(launcher && self.hidden_launcher_renders()) {
                // Hidden or windowless: poll and due timers run, and the view waits for the
                // frame that shows the window. A launcher's cached frame follows, paced.
                let changed = self.app.hidden_wake();
                self.clipboard.flush_core(self.app.rt_mut());
                self.shell(event_loop);
                if changed && launcher { self.refresh_hidden(); }
            // A redraw while the window backs off would draw nothing and leave the timer due.
            } else if self.win.as_ref().is_some_and(|w| (launcher || self.life.uncovered()) && !w.minimized()
                && !w.backing_off(crate::time::now())) {
                let now = crate::time::now();
                // A frame the due timer asked for that the window never drew (a Wayland surface
                // covered before its frame callback fires) would come back here for the same
                // overdue timer and turn the loop at once. Let the timer run covered, and give
                // the next frame it asks for the stall bound afresh.
                if self.win.as_ref().is_some_and(|w| w.timer_frame.stalled(now)) {
                    if let Some(w) = &mut self.win {
                        w.timer_frame.stalled_again(now);
                    }
                    self.app.covered_wake();
                    self.clipboard.flush_core(self.app.rt_mut());
                    self.shell(event_loop);
                } else {
                    // A resting window polls first and draws only for what that changed.
                    let rests = self.idle && !self.app.compositor_animating() && self.win.as_ref().is_some_and(Win::rests);
                    let frame = match rests.then(|| self.app.resting_wake()).flatten() {
                        Some(stale) => {
                            self.clipboard.flush_core(self.app.rt_mut());
                            self.shell(event_loop);
                            // A close the poll asked for may have hidden the window.
                            let frame = (stale || self.app.rt().stale()) && self.shown_win().is_some();
                            self.app.rt_mut().resting_wake_frame(frame);
                            frame
                        }
                        None => true,
                    };
                    if frame && let Some(w) = &mut self.win {
                        w.timer_frame.asked_quiet(now);
                        w.request_redraw();
                    }
                }
            } else {
                self.app.covered_wake();
                self.clipboard.flush_core(self.app.rt_mut());
                self.shell(event_loop);
            }
        }
    }

    fn about_to_wait_in_turn(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if self.poll_gpu_loss() { return false; }
        if self.idle_geometry(event_loop) { return false; }
        #[cfg(target_os = "macos")]
        {
            self.fn_key();
            self.media_keys();
        }
        self.flush_files(event_loop);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        self.poll_wayland_backdrop();
        // Another window switched the font families (`Tx::set_font_families`).
        if self.app.rt_mut().follow_font_families() && let Some(w) = &mut self.win {
            w.loop_frame.touched();
            w.request_redraw();
        }
        // Process queued hide/close requests before completing a pending first reveal.
        self.first_frame_ready(event_loop);
        if !self.win.as_ref().is_some_and(|w| self.life.shown() || w.first_frame.revealing()) {
            self.app.rt_mut().refuse_file_dialog("cannot open a file dialog while the window is hidden");
        }
        self.hidden_input(event_loop);
        self.a11y_turn();
        // After the first frame: its on_ready, hooks, the GPU's start and the accessibility
        // adapter, one a turn, none while input waits; the loop turns again while some are owed.
        #[cfg(target_os = "windows")]
        return self.post_frame_turn();
        // Elsewhere `on_ready` ran with the first frame; a view asked to be prepared since runs now.
        // Linux: a window that opened on the CPU starts its GPU in the first of these turns.
        #[cfg(target_os = "linux")]
        self.cpu_idle();
        #[cfg(not(target_os = "windows"))]
        { !crate::marks::counting() && self.app.text_idle_owed() && self.app.text_idle_turn() }
    }

    fn resumed_in_turn(&mut self, event_loop: &ActiveEventLoop) {
        self.resume(event_loop);
        self.publish();
    }

    fn user_event_in_turn(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        #[cfg(feature = "frame-profile")]
        let logged = Some(crate::frame::live_log::now());
        self.meter.waited();
        self.handle_user_event(event_loop, event);
        self.publish();
        #[cfg(feature = "frame-profile")]
        if let Some(start) = logged { crate::frame::live_log::wake(start); }
    }

    fn window_event_in_turn(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        #[cfg(feature = "frame-profile")]
        let logged = (!matches!(event, WindowEvent::RedrawRequested)).then(crate::frame::live_log::now);
        if !matches!(event, WindowEvent::RedrawRequested) { self.meter.waited(); }
        self.handle_window_event(event_loop, id, event);
        self.publish();
        #[cfg(feature = "frame-profile")]
        if let Some(start) = logged { crate::frame::live_log::input(start); }
    }
}

/// Whether a window whose turn is ending must request a redraw rather than rest: user input is
/// still queued (`RuntimeCore::has_input_waiting`), the window can draw now, and neither a
/// redraw nor an active display link is already on its way.
pub(in crate::window) fn queued_input_needs_redraw(
    has_input: bool,
    can_draw: bool,
    paced: bool,
    redraw: Redraw,
) -> bool {
    has_input && can_draw && !paced && matches!(redraw, Redraw::Idle | Redraw::Satisfied)
}

/// Runs a window's `App::on_quit`, flushes clipboard requests made there, and reports whether
/// a still-shown window should request a redraw. Runs even when `life` is `Life::Finished`
/// because the window whose close ended the app has already finished when the quit starts.
pub(in crate::window) fn quit_hook_core(app: &mut dyn WindowApp, clipboard: &mut crate::clipboard::Client, life: Life) -> bool {
    if !app.run_on_quit() { return false; }
    // Before the loop ends or the session does: a lock or clear must reach the worker.
    clipboard.flush_core(app.rt_mut());
    life.shown()
}

/// When a covered window looks again whether it's still covered (`look`).
fn look_deadline(win: Option<&Win>, life: Life) -> Option<Instant> {
    #[cfg(target_os = "windows")]
    return win.and_then(|w| w.native.look.deadline(life));
    #[cfg(not(target_os = "windows"))]
    { let _ = (win, life); None }
}

/// When a Wayland window's lazy `wl_data_device` teardown grace timer expires.
fn wayland_drop_deadline(win: Option<&Win>) -> Option<Instant> {
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    return win.and_then(|w| w.native.drop_binding.deadline());
    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    { let _ = win; None }
}
