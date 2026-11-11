//! The open window: its surface, native handles, pacing and first-frame state.

use super::*;

/// How long `on_ready` waits for a first frame that never comes, as when the window opens
/// hidden or its drawable never arrives. A covered or occluded window doesn't wait for it.
pub(super) const READY_FALLBACK: Duration = Duration::from_secs(1);

/// What a frame does about a surface that can't draw (`Host::replace_lost_surface`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SurfaceStep {
    /// Try a replacement now.
    Replace,
    /// `SURFACE_TRIES` replacements in a row and no frame presented between them: give the
    /// window up.
    GiveUp,
}

/// How many replacements in a row a window may make before it gives the window up, and how long
/// it waits between ones that failed (`Host::replace_lost_surface`). Every replacement counts,
/// whether or not it worked: only a frame that presents starts the count over (`Host::draw`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct SurfaceRetries {
    replacements: u8,
}

/// Replacements in a row without a presented frame before the window gives up.
pub(super) const SURFACE_TRIES: u8 = 3;
/// The first wait between replacements; each one after waits a further `SURFACE_BACKOFF`.
pub(super) const SURFACE_BACKOFF: Duration = Duration::from_millis(100);

impl SurfaceRetries {
    /// Counts this replacement and says whether it may happen. One that worked counts too: a
    /// surface replaced and lost again before its frame presents has still used it up.
    pub(super) fn attempt(&mut self) -> SurfaceStep {
        if self.replacements >= SURFACE_TRIES { return SurfaceStep::GiveUp; }
        self.replacements = self.replacements.saturating_add(1);
        SurfaceStep::Replace
    }

    /// When to try again after a replacement that failed at `now`, or None when that was the
    /// last one `attempt` allows.
    pub(super) fn retry(&self, now: Instant) -> Option<Instant> {
        (self.replacements < SURFACE_TRIES).then(|| now + SURFACE_BACKOFF * u32::from(self.replacements))
    }

    /// A frame presented: the next loss starts its replacements over.
    pub(super) fn worked(&mut self) { self.replacements = 0; }

    #[cfg(test)]
    pub(super) fn failures(&self) -> u8 { self.replacements }
}

/// The open window. Fields drop in order: what's native to the OS, then the surface before its
/// window.
pub(super) struct Win {
    /// The OS accessibility adapter, which goes before the window it answers for.
    pub(super) a11y: Option<crate::a11y::Adapter>,
    #[cfg_attr(not(any(target_os = "macos", all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(super) native: Native,
    pub(super) first_frame: FirstFrame,
    /// The surface it draws into. None only while `Host::reattach` replaces it: the old surface
    /// has to go before the new one is made (a DX12 surface takes the window's one
    /// DirectComposition target, `Dx12SwapchainKind::DxgiFromVisual`) and before the device it
    /// drew with.
    pub(super) surface: Option<wgpu::Surface<'static>>,
    pub(super) config: wgpu::SurfaceConfiguration,
    /// Whether the surface's last configuration took. False after a refused one, and for a lost
    /// device: no drawable can be taken from it (`acquire`), and its next frame replaces the
    /// surface and the device it drew with (`Host::reattach`).
    pub(super) surface_ok: bool,
    /// How many times in a row a frame tried to replace that surface and device, and the wait
    /// between tries (`SurfaceRetries`).
    pub(super) surface_retries: SurfaceRetries,
    pub(super) window: Arc<Window>,
    /// When to redraw after a frame that had no drawable, or the next frame of motion when no
    /// display link paces it (macOS) (`Backoff`).
    pub(super) backoff: Backoff,
    /// When a hidden launcher renders the changes it hasn't yet.
    pub(super) hidden_pace: HiddenPace,
    pub(super) cursor: crate::cursor::os::OsCursor,
    pub(super) alpha: Option<crate::render::surface_alpha::AlphaSurface>,
    /// The frame a due timer asked for, until the window draws it, and whether only it asked
    /// (`TimerFrame`).
    pub(super) timer_frame: TimerFrame,
    /// The last frame presented, while the window rests.
    pub(super) presented: Option<Presented>,
    /// The window's steady run after a change (`STEADY`).
    #[cfg(target_os = "windows")]
    pub(super) steady: Steady,
    /// Its next frame (`redraw`).
    pub(super) redraw: std::cell::Cell<Redraw>,
}

impl Win {
    /// Asks for a frame: now, or at the turn's end while the window paints (`Redraw`).
    #[cfg_attr(feature = "frame-profile", track_caller)]
    pub(super) fn request_redraw(&self) { self.redraw_step(RedrawEvent::Ask(Instant::now())); }

    #[cfg_attr(feature = "frame-profile", track_caller)]
    pub(super) fn redraw_step(&self, event: RedrawEvent) {
        let (next, effect) = self.redraw.get().step(event);
        self.redraw.set(next);
        #[cfg(feature = "frame-profile")]
        if matches!(event, RedrawEvent::Ask(_)) || effect != RedrawEffect::None { crate::frame::live_log::asked(std::panic::Location::caller()); }
        if effect == RedrawEffect::Request { self.window.request_redraw(); }
    }

    /// Matches the drawable to the window's size; false when it already did. A surface that
    /// refuses the new size is marked so its next frame replaces it (`Host::reattach`) rather
    /// than drawing into a surface wgpu has no presentation for.
    pub(super) fn resize_surface(&mut self, gpu: Option<&Gpu>, size: winit::dpi::PhysicalSize<u32>) -> bool {
        let (width, height) = (size.width.max(1), size.height.max(1));
        if (self.config.width, self.config.height) == (width, height) { return false; }
        self.config.width = width;
        self.config.height = height;
        // No surface: `Host::reattach` is between the old one and the new, and takes this size
        // from the window itself; or the CPU renderer draws it, whose buffer follows this size.
        let (Some(surface), Some(gpu)) = (&self.surface, gpu) else { return true };
        if let Err(error) = super::configure(surface, gpu, &self.config) {
            eprintln!("rune: a window's surface refused its new size: {error}");
            self.surface_ok = false;
        }
        if let Some(alpha) = &mut self.alpha { alpha.resize(&gpu.device, (width, height)); }
        #[cfg(target_os = "macos")]
        if let Some(shadow) = &self.native.shadow { shadow.redraw(); }
        true
    }
}

#[test]
fn launcher_first_frame_holds_reveal_and_hide_cancels_late_completion() {
    let now = Instant::now();
    let mut frame = FirstFrame::default();
    frame.prepare(now);
    assert!(!frame.request_reveal(false, false, now));
    assert!(!frame.ready());
    assert!(frame.complete());
    assert!(frame.ready() && frame.deadline().is_none());
    assert!(!frame.complete(), "a second callback must not reopen a hidden panel");
    let mut canceled = FirstFrame::default();
    canceled.prepare(now);
    assert!(!canceled.request_reveal(false, false, now));
    canceled.hide();
    assert!(!canceled.complete(), "hide before readiness cancels the pending show");
    assert!(canceled.ready());
    assert!(canceled.request_reveal(false, false, now), "the prepared frame can be revealed without another preparation");
    assert!(canceled.deadline().is_none());
}

#[test]
fn launcher_stale_reveal_waits_for_an_unsettled_hidden_frame() {
    let now = Instant::now();
    let mut frame = FirstFrame::drawn_notified();
    assert!(!frame.complete());
    assert!(frame.request_reveal(false, false, now), "unchanged state reveals the cached frame at once");
    assert!(!frame.pending() && frame.settles());
    assert!(!frame.request_reveal(true, false, now), "changed state renders hidden first");
    assert!(frame.deadline().is_some() && !frame.settles(), "the refresh keeps its new motion");
    frame.gpu_done(); // The hidden frame's fence completed.
    assert!(frame.pending() && frame.complete());
    assert!(!frame.pending() && frame.settles());
    assert!(!frame.request_reveal(true, false, now));
    frame.hide();
    assert!(frame.settles(), "a canceled refresh is an ordinary hidden frame");
    assert!(frame.ready() && frame.deadline().is_none(), "hide stops the refresh retries");
    assert!(!frame.pending() && !frame.complete(), "a late frame does not reveal");

    assert!(!frame.request_reveal(true, false, now));
    assert!(frame.expire(), "a refresh with no drawable reveals the cached frame");
    assert!(frame.ready() && !frame.pending() && frame.deadline().is_none());
    let mut first = FirstFrame::default();
    first.prepare(now);
    assert!(!first.request_reveal(true, false, now));
    assert!(!first.expire() && !first.ready(), "the first frame stays hidden and reports instead");
    first.gpu_done();
    assert!(first.pending() && first.complete(), "a late first frame still reveals");
}

#[test]
fn launcher_refresh_with_no_drawable_retries_and_never_reveals_stale_pixels() {
    let now = Instant::now();
    let mut frame = FirstFrame::drawn_notified();
    let mut backoff = Backoff::default();
    let missed = |frame: &mut FirstFrame, backoff: &mut Backoff| {
        frame.missed();
        backoff.step(BackoffEvent::HiddenMissed { preparing: frame.deadline().is_some(), now });
        backoff.deadline()
    };
    assert_eq!(missed(&mut frame, &mut backoff), Some(now + TIMEOUT_RETRY), "a hidden refresh that got no drawable tries again");
    assert!(!frame.ready(), "its cached frame no longer counts as ready");
    assert_eq!(missed(&mut frame, &mut backoff), Some(now + TIMEOUT_RETRY * 2), "and backs off");
    for _ in 0..10 { missed(&mut frame, &mut backoff); }
    assert_eq!(backoff.deadline(), Some(now + OCCLUDED_MAX), "up to a re-check every two seconds");
    assert!(!frame.request_reveal(false, false, now), "a reveal waits for the refresh even with nothing new to commit");
    assert!(frame.deadline().is_some());
    assert_eq!(missed(&mut frame, &mut backoff), None, "the first-frame deadline owns its retries");

    // One already on screen shows at once and redraws, rather than retrying a first frame.
    let mut shown = FirstFrame::drawn_notified();
    assert!(shown.request_reveal(true, true, now));
    assert!(shown.deadline().is_none() && shown.ready());
}

impl Win {
    pub(super) fn show(&self, focus: bool) {
        #[cfg(target_os = "macos")]
        {
            if let Some(shadow) = &self.native.shadow { shadow.shown(); }
            if let Some(panel) = &self.native.panel {
                panel.show(std::time::Duration::ZERO);
                if focus { panel.request_keys(); }
                return;
            }
            if !focus { crate::platform::macos::show_without_focus(&self.window); return; }
        }
        // What it presented before it was hidden is gone, or never was: cloaked until it draws.
        #[cfg(target_os = "windows")]
        {
            self.native.cloak.hold(&self.window, Instant::now() + READY_FALLBACK, false);
            self.request_redraw();
        }
        self.window.set_visible(true);
        if focus { self.window.focus_window(); }
    }

    /// When a cloaked window shows without its frame.
    pub(super) fn cloak_deadline(&self) -> Option<Instant> {
        #[cfg(target_os = "windows")]
        return self.native.cloak.deadline();
        #[cfg(not(target_os = "windows"))]
        None
    }

    pub(super) fn hide(&self) {
        #[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
        crate::file_dialog::window_focus(&self.window, false);
        #[cfg(target_os = "macos")]
        if let Some(shadow) = &self.native.shadow { shadow.hidden(); }
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.native.panel { panel.hide(std::time::Duration::ZERO); return; }
        self.window.set_visible(false);
        // Hidden before its frame: the next show cloaks it again.
        #[cfg(target_os = "windows")]
        self.native.cloak.hidden(&self.window);
    }

    /// Its last frame is on screen and nothing has asked for another since.
    pub(super) fn rests(&self) -> bool {
        #[cfg(target_os = "macos")]
        if self.native.prime.presumes() { return false; }
        self.presented.is_some()
    }

    /// The display link's next tick after `now`, while it runs.
    pub(super) fn next_tick(&self, now: Instant) -> Option<Instant> {
        #[cfg(target_os = "macos")]
        return self.native.pacing.as_ref().filter(|p| p.running()).map(|p| now + Duration::from_secs_f64(1.0 / p.refresh_hz()));
        #[cfg(not(target_os = "macos"))]
        { let _ = now; None }
    }

    /// Whether the display link drives redraws. Other platforms redraw on request.
    pub(super) fn paced(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.native.pacing.as_ref().is_some_and(|p| p.running());
        #[cfg(not(target_os = "macos"))]
        false
    }

    /// Whether something other than events paces the window's redraws: the display link on
    /// macOS, the present queue's vsync elsewhere.
    pub(super) fn paced_for_stats(&self) -> bool { !cfg!(target_os = "macos") || self.paced() }

    /// Steps the window's wait after a frame it couldn't draw, and carries out what that asks: a
    /// display link that would draw sooner than the wait stops.
    pub(super) fn backoff_step(&mut self, event: BackoffEvent) {
        for effect in self.backoff.step(event) {
            match effect { BackoffEffect::StopPacing => self.stop_pacing() }
        }
    }

    /// Waiting for a missed frame's retry, or for the timer pacing unpaced motion. A hidden
    /// launcher's first-frame deadline owns its own retries, so a wait it leaves behind doesn't
    /// hold the frame back.
    pub(super) fn backing_off(&self, now: Instant) -> bool {
        self.backoff.waiting(now) && self.first_frame.deadline().is_none()
    }

    /// What paces the window's frames, for its stats.
    pub(super) fn pacing_state(&self, moving: bool) -> Pacing {
        #[cfg(target_os = "macos")]
        return crate::platform::display_link::pacing(self.native.pacing.as_ref(), moving);
        #[cfg(not(target_os = "macos"))]
        if moving { Pacing::Vsync } else { Pacing::Idle }
    }

    /// Minimized, as far as drawing goes. On macOS occlusion says it instead (the `Occluded`
    /// event, `natively_occluded`): `isMiniaturized` stays true into a deminiaturize whose
    /// occlusion change and first redraws would otherwise draw nothing, with no event after.
    pub(super) fn minimized(&self) -> bool {
        !cfg!(target_os = "macos") && self.window.is_minimized().unwrap_or(false)
    }

    /// Whether none of the window is on screen, read from AppKit (a prewarm acquisition reads
    /// as visible).
    #[cfg(target_os = "macos")]
    pub(super) fn natively_occluded(&self) -> bool {
        match &self.native.panel {
            Some(panel) => panel.occluded(),
            None => crate::platform::macos::window_occluded(&self.window),
        }
    }

    pub(super) fn stop_pacing(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(pacing) = &mut self.native.pacing { pacing.stop(); }
        #[cfg(target_os = "macos")]
        self.present_with_transaction(false);
    }

    /// Presents drawables inside Core Animation transactions, while the frame moves.
    #[cfg(target_os = "macos")]
    pub(super) fn present_with_transaction(&mut self, on: bool) {
        if let Some(surface) = &self.surface { self.native.materials.set_moving(surface, on); }
    }

    /// Presents the frame in the transaction that resizes the window while the user drags its
    /// edge. Before the drawable is acquired: wgpu reads `presentsWithTransaction` then.
    #[cfg(target_os = "macos")]
    pub(super) fn set_live_resize(&mut self, on: bool) {
        if let Some(surface) = &self.surface { self.native.materials.set_live(&self.window, surface, on); }
    }

    pub(super) fn focus(&self) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.native.panel { panel.request_keys(); return; }
        self.window.focus_window();
    }
}

/// `on_ready` starts an app's live work, so it can't wait on a drawable: a window launched
/// covered, occluded or hidden by the system may not draw for as long as it stays that way.
/// It runs at the first frame skipped for being off screen, or `READY_FALLBACK` after opening
/// when no frame comes at all; a window on screen still runs it after its first frame.
#[test]
fn on_ready_runs_for_a_window_launched_occluded_or_hidden() {
    let now = Instant::now();
    let ms = Duration::from_millis;
    let mut shown = FirstFrame::opened(now);
    assert!(!shown.ready_due(now + ms(16)), "a window on screen waits for its first frame");
    shown.gpu_done();
    assert!(shown.pending() && !shown.complete());
    assert!(shown.take_ready(), "its first frame runs on_ready");
    assert!(!shown.take_ready() && shown.ready_deadline().is_none(), "once");

    let mut covered = FirstFrame::opened(now);
    covered.skipped(now + ms(30));
    assert!(covered.ready_due(now + ms(30)) && covered.take_ready(), "the first covered frame runs on_ready");
    assert!(!covered.ready() && !covered.pending(), "the window isn't ready until it draws");
    covered.skipped(now + ms(60));
    assert_eq!(covered.ready_deadline(), None, "and skipping again doesn't run it again");
    covered.gpu_done(); // Uncovered.
    assert!(covered.pending() && !covered.complete() && !covered.take_ready(), "drawing at last doesn't either");

    let mut hidden = FirstFrame::opened(now);
    assert_eq!(hidden.ready_deadline(), Some(now + READY_FALLBACK), "the event loop wakes for it");
    assert!(!hidden.ready_due(now + READY_FALLBACK - ms(1)));
    assert!(hidden.ready_due(now + READY_FALLBACK) && hidden.take_ready(), "a window that never draws runs it after the fallback");
}
