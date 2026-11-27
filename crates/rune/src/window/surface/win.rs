//! The open window: its surface, native handles, pacing and first-frame state.

use std::sync::Arc;
use std::time::Duration;

use winit::window::Window;

use super::*;
use crate::frame::surface_stats::Pacing;
use crate::render::Gpu;
use crate::time::Instant;
use crate::window::state::FirstFrame;
#[cfg(target_os = "windows")]
use crate::window::win32::covered_draw::CoveredDraw;

/// How long `on_ready` waits for a first frame that never comes, as when the window opens
/// hidden or its drawable never arrives. A covered or occluded window doesn't wait for it.
pub(in crate::window) const READY_FALLBACK: Duration = Duration::from_secs(1);

/// What a frame does about a surface that can't draw (`Host::replace_lost_surface`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum SurfaceStep {
    /// Try a replacement in `mode`. `switched_to_software` is true on the single transition
    /// from hardware to software so the caller logs it once.
    Replace {
        mode: crate::gpu::fallback::GpuMode,
        switched_to_software: bool,
    },
    /// The active fallback chain is exhausted (or pinned/unavailable): give the window up.
    GiveUp,
}

/// Repeated device-loss accounting and fallback mode for one window (`Host::replace_lost_surface`),
/// backed directly by `rune_gpu::fallback::LossState`: 3 losses per mode with 1 forgiven per
/// 5 minutes, hardware -> software -> give up (unless pinned or no software adapter exists).
/// A single healthy frame does not reset the count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::window) struct SurfaceRetries {
    state: crate::gpu::fallback::LossState,
}

/// Losses in one mode within the forgiveness window before falling back or giving up.
pub(in crate::window) const SURFACE_TRIES: u8 = crate::gpu::fallback::LOSS_LIMIT;
/// The first wait between replacements; each one after waits a further `SURFACE_BACKOFF`.
pub(in crate::window) const SURFACE_BACKOFF: Duration = Duration::from_millis(100);

impl SurfaceRetries {
    /// Starts with zero recorded losses in `mode`.
    pub(in crate::window) const fn for_mode(mode: crate::gpu::fallback::GpuMode) -> Self {
        Self { state: crate::gpu::fallback::LossState::for_mode(mode) }
    }

    /// Records a loss at `now` and returns whether to replace (and in which mode) or give up.
    pub(in crate::window) fn attempt(
        &mut self,
        now: Instant,
        policy: crate::gpu::fallback::Policy,
        software_available: bool,
    ) -> SurfaceStep {
        let effect = crate::gpu::fallback::step_loss(
            &mut self.state,
            crate::gpu::fallback::LossEvent::Replace { now, policy, software_available },
        )
        .into_iter()
        .next();
        match effect {
            Some(crate::gpu::fallback::LossEffect::Replace { mode, switched_to_software }) => {
                SurfaceStep::Replace { mode, switched_to_software }
            }
            Some(crate::gpu::fallback::LossEffect::GiveUp) | None => SurfaceStep::GiveUp,
        }
    }

    /// When to try again after a replacement that failed at `now`, or None if the next loss
    /// would exhaust the fallback chain.
    pub(in crate::window) fn retry(
        &self,
        now: Instant,
        policy: crate::gpu::fallback::Policy,
        software_available: bool,
    ) -> Option<Instant> {
        self.state.next_backoff(now, policy, software_available, SURFACE_BACKOFF)
    }

    /// When to retry after a frame whose device was lost during submission or presentation, or
    /// None if the next loss at `now` exhausts the fallback chain.
    pub(in crate::window) fn loss_retry(
        &self,
        now: Instant,
        policy: crate::gpu::fallback::Policy,
        software_available: bool,
    ) -> Option<Instant> {
        self.state.next_backoff(now, policy, software_available, SURFACE_BACKOFF)
    }

    /// Records that a replacement attempt failed at `now` (counts a failed hardware->software
    /// switch as software loss 1).
    pub(in crate::window) fn note_replace_failed(&mut self, now: Instant) {
        let _ = crate::gpu::fallback::step_loss(
            &mut self.state,
            crate::gpu::fallback::LossEvent::ReplaceFailed { now },
        );
    }

    /// Records that the window is now running on a software adapter (`DeviceType::Cpu`).
    pub(in crate::window) fn note_software(&mut self) {
        let _ = crate::gpu::fallback::step_loss(
            &mut self.state,
            crate::gpu::fallback::LossEvent::OnSoftware,
        );
    }

    /// Records that software fallback found no usable software adapter on this host.
    pub(in crate::window) fn note_no_software(&mut self) {
        let _ = crate::gpu::fallback::step_loss(
            &mut self.state,
            crate::gpu::fallback::LossEvent::NoSoftwareAdapter,
        );
    }

    #[cfg(test)]
    pub(in crate::window) fn failures(&self, now: Instant) -> u8 {
        self.state.effective_losses(now)
    }

    #[cfg(test)]
    pub(in crate::window) fn mode(&self) -> Option<crate::gpu::fallback::GpuMode> {
        self.state.mode()
    }
}

/// The open window. Fields drop in order: what's native to the OS, then the surface before its
/// window.
pub(in crate::window) struct Win {
    /// The OS accessibility adapter, which goes before the window it answers for.
    pub(in crate::window) a11y: Option<crate::a11y::Adapter>,
    #[cfg_attr(not(any(target_os = "macos", all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(in crate::window) native: Native,
    pub(in crate::window) first_frame: FirstFrame,
    /// The surface it draws into. None only while `Host::reattach` replaces it: the old surface
    /// has to go before the new one is made (a DX12 surface takes the window's one
    /// DirectComposition target, `Dx12SwapchainKind::DxgiFromVisual`) and before the device it
    /// drew with.
    pub(in crate::window) surface: Option<wgpu::Surface<'static>>,
    pub(in crate::window) config: wgpu::SurfaceConfiguration,
    /// Whether the surface's last configuration took. False after a refused one, and for a lost
    /// device: no drawable can be taken from it (`acquire`), and its next frame replaces the
    /// surface and the device it drew with (`Host::reattach`).
    pub(in crate::window) surface_ok: bool,
    /// How many times in a row a frame tried to replace that surface and device, and the wait
    /// between tries (`SurfaceRetries`).
    pub(in crate::window) surface_retries: SurfaceRetries,
    pub(in crate::window) window: Arc<Window>,
    /// When to redraw after a frame that had no drawable, or the next frame of motion when no
    /// display link paces it (macOS) (`Backoff`).
    pub(in crate::window) backoff: Backoff,
    /// When a hidden launcher renders the changes it hasn't yet.
    pub(in crate::window) hidden_pace: HiddenPace,
    pub(in crate::window) cursor: crate::cursor::os::OsCursor,
    pub(in crate::window) alpha: Option<crate::render::surface_alpha::AlphaSurface>,
    /// The pass that fades the content outside the arcs a Wayland client-side frame leaves its
    /// bottom corners (`surface::corners`), once a surface here composites alpha.
    pub(in crate::window) corners: Option<crate::render::csd_corners::CsdCorners>,
    /// The frame a due timer asked for, until the window draws it, and whether only it asked
    /// (`TimerFrame`).
    pub(in crate::window) timer_frame: TimerFrame,
    /// Whether only its loops asked for the next frame (`LoopFrame`).
    pub(in crate::window) loop_frame: LoopFrame,
    /// What its last frames changed, so the next redraws only that (`damage`).
    pub(in crate::window) damage: damage::Damage,
    /// The last frame presented, while the window rests.
    pub(in crate::window) presented: Option<Presented>,
    /// A frame asked for while other windows covered it, still to draw (`CoveredDraw`).
    #[cfg(target_os = "windows")]
    pub(in crate::window) covered_draw: CoveredDraw,
    /// Its next frame (`redraw`).
    pub(in crate::window) redraw: std::cell::Cell<Redraw>,
}

impl Win {
    /// Windows: its next frame waits for room in the swap chain, and comes when there is (`slot`).
    pub(in crate::window) fn waits_for_slot(&self) -> bool {
        #[cfg(target_os = "windows")]
        return self.native.slot.waiting();
        #[cfg(not(target_os = "windows"))]
        false
    }

    /// Asks for a frame: now, or at the turn's end while the window paints (`Redraw`).
    #[cfg_attr(feature = "frame-profile", track_caller)]
    pub(in crate::window) fn request_redraw(&self) { self.redraw_step(RedrawEvent::Ask(crate::time::now())); }

    #[cfg_attr(feature = "frame-profile", track_caller)]
    pub(in crate::window) fn redraw_step(&self, event: RedrawEvent) {
        let (next, effect) = self.redraw.get().step(event);
        self.redraw.set(next);
        #[cfg(feature = "frame-profile")]
        if matches!(event, RedrawEvent::Ask(_)) || effect != RedrawEffect::None { crate::frame::live_log::asked(std::panic::Location::caller()); }
        if effect == RedrawEffect::Request { self.window.request_redraw(); }
    }

    /// Matches the drawable to the window's size; false when it already did or when the reported
    /// size is zero or exceeds the texture limit (keeping the previous drawable until a valid size
    /// arrives). A surface that refuses the new size is marked so its next frame replaces it
    /// (`Host::reattach`) rather than drawing into a surface wgpu has no presentation for.
    pub(in crate::window) fn resize_surface(&mut self, gpu: Option<&Gpu>, size: winit::dpi::PhysicalSize<u32>) -> bool {
        let (width, height) = super::drawable_size(Some((self.config.width, self.config.height)), (size.width, size.height), super::max_surface_dim(gpu));
        if (self.config.width, self.config.height) == (width, height) { return false; }
        self.config.width = width;
        self.config.height = height;
        self.damage.reset();
        // No surface: `Host::reattach` is between the old one and the new, and takes this size
        // from the window itself; or the CPU renderer draws it, whose buffer follows this size.
        let (Some(surface), Some(gpu)) = (&self.surface, gpu) else { return true };
        let configured = super::configure(surface, gpu, &self.config);
        // DX12 `configure` calls `ResizeBuffers` on the same swap chain, preserving its waitable
        // semaphore and count; only a failed configure drops the swap chain.
        #[cfg(target_os = "windows")]
        self.native.slot.reconfigured(configured.is_ok());
        if let Err(error) = configured {
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
    let now = crate::time::now();
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
    let now = crate::time::now();
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
    let now = crate::time::now();
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

#[test]
fn launcher_hidden_resize_holds_reveal_until_the_resized_frame_draws() {
    let now = crate::time::now();
    let mut frame = FirstFrame::drawn_notified();
    assert!(!frame.request_reveal(true, false, now), "changed state refreshes hidden first");
    let deadline = frame.deadline();
    assert!(deadline.is_some());
    frame.gpu_done(); // Hidden refresh drew at the old height and queued a fit_content_height resize.
    assert!(frame.pending());
    frame.missed(); // The hidden surface resized, invalidating the old-height cached pixels.
    assert!(!frame.pending() && !frame.ready() && frame.revealing(), "reveal waits for the resized draw");
    assert_eq!(frame.deadline(), deadline, "keeps the original reveal deadline");
    frame.gpu_done(); // Hidden frame drew at the new height.
    assert!(frame.pending() && frame.complete(), "reveals once the new-height frame finishes");
}

impl Win {
    pub(in crate::window) fn show(&mut self, focus: bool) {
        #[cfg(target_os = "macos")]
        {
            if let Some(shadow) = &self.native.shadow { shadow.shown(); }
            if let Some(panel) = &self.native.panel {
                panel.show(std::time::Duration::ZERO);
                if focus { panel.request_keys(); }
                // A frame right after order-in can read as occluded before AppKit's commit
                // lands, and launchers drop native occlusion events.
                self.native.prime.arm();
                return;
            }
            if !focus { crate::platform::macos::show_without_focus(&self.window); return; }
        }
        // What it presented before it was hidden is gone, or never was: cloaked until it draws.
        #[cfg(target_os = "windows")]
        {
            self.native.cloak.hold(&self.window, crate::time::now() + READY_FALLBACK, false);
            self.request_redraw();
        }
        self.window.set_visible(true);
        if focus { self.window.focus_window(); }
    }

    /// When a cloaked window shows without its frame.
    pub(in crate::window) fn cloak_deadline(&self) -> Option<Instant> {
        #[cfg(target_os = "windows")]
        return self.native.cloak.deadline();
        #[cfg(not(target_os = "windows"))]
        None
    }

    pub(in crate::window) fn hide(&self) {
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
    pub(in crate::window) fn rests(&self) -> bool {
        #[cfg(target_os = "macos")]
        if self.native.prime.presumes() { return false; }
        self.presented.is_some()
    }

    /// The display link's next tick after `now`, while actively driving redraws.
    pub(in crate::window) fn next_tick(&self, now: Instant) -> Option<Instant> {
        #[cfg(target_os = "macos")]
        return self.native.pacing.as_ref().filter(|p| p.paced()).map(|p| p.tick_after(now));
        #[cfg(not(target_os = "macos"))]
        { let _ = now; None }
    }

    /// Whether the display link actively drives redraws (false while coasting or stopped). Other
    /// platforms redraw on request.
    pub(in crate::window) fn paced(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.native.pacing.as_ref().is_some_and(|p| p.paced());
        #[cfg(not(target_os = "macos"))]
        false
    }

    /// Arms one-shot urgent frame admission on macOS display-link pacing.
    pub(in crate::window) fn admit_urgent_frame(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(pacing) = &mut self.native.pacing { pacing.admit_once(); }
    }

    #[cfg(target_os = "macos")]
    pub(in crate::window) fn has_urgent_admission(&self) -> bool {
        self.native.pacing.as_ref().is_some_and(|p| p.is_urgent())
    }

    /// Consumes a macOS `RedrawRequested` on entry to `paint`, returning `true` when a prior
    /// `DisplayTick` or synchronous draw already satisfied it.
    #[cfg(target_os = "macos")]
    pub(in crate::window) fn take_redraw_on_paint(&self) -> bool {
        let (next, satisfied) = self.redraw.get().take_on_paint();
        self.redraw.set(next);
        satisfied
    }

    /// Whether something other than events paces the window's redraws: the display link on
    /// macOS, the present queue's vsync elsewhere.
    pub(in crate::window) fn paced_for_stats(&self) -> bool { !cfg!(target_os = "macos") || self.paced() }

    /// Steps the window's wait after a frame it couldn't draw, and carries out what that asks: a
    /// display link that would draw sooner than the wait stops.
    pub(in crate::window) fn backoff_step(&mut self, event: BackoffEvent) {
        for effect in self.backoff.step(event) {
            match effect { BackoffEffect::StopPacing => self.stop_pacing() }
        }
    }

    /// Waiting for a missed frame's retry, or for the timer pacing unpaced motion. A hidden
    /// launcher's first-frame deadline owns its own retries, so a wait it leaves behind doesn't
    /// hold the frame back.
    pub(in crate::window) fn backing_off(&self, now: Instant) -> bool {
        self.backoff.waiting(now) && self.first_frame.deadline().is_none()
    }

    /// What paces the window's frames, for its stats.
    pub(in crate::window) fn pacing_state(&self, moving: bool) -> Pacing {
        #[cfg(target_os = "macos")]
        return crate::platform::display_link::pacing(self.native.pacing.as_ref(), moving);
        #[cfg(not(target_os = "macos"))]
        if moving { Pacing::Vsync } else { Pacing::Idle }
    }

    /// Minimized, as far as drawing goes. On macOS occlusion says it instead (the `Occluded`
    /// event, `natively_occluded`): `isMiniaturized` stays true into a deminiaturize whose
    /// occlusion change and first redraws would otherwise draw nothing, with no event after.
    pub(in crate::window) fn minimized(&self) -> bool {
        !cfg!(target_os = "macos") && self.window.is_minimized().unwrap_or(false)
    }

    /// Whether none of the window is on screen, read from AppKit (a prewarm acquisition reads
    /// as visible).
    #[cfg(target_os = "macos")]
    pub(in crate::window) fn natively_occluded(&self) -> bool {
        match &self.native.panel {
            Some(panel) => panel.occluded(),
            None => crate::platform::macos::window_occluded(&self.window),
        }
    }

    /// Keeps the macOS display link coasting briefly (~150 ms) with zero draws and zero presents
    /// when a visible window settles idle after presenting.
    pub(in crate::window) fn coast_pacing(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(pacing) = &mut self.native.pacing { pacing.coast(); }
        #[cfg(target_os = "macos")]
        self.present_with_transaction(false);
    }

    /// Stops a coasting macOS display link once its keep-alive budget hits zero (zero redraws).
    #[cfg(target_os = "macos")]
    pub(in crate::window) fn stop_expired_pacing(&mut self) {
        if let Some(pacing) = &mut self.native.pacing { pacing.stop_expired(); }
    }

    pub(in crate::window) fn stop_pacing(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(pacing) = &mut self.native.pacing { pacing.stop(); }
        #[cfg(target_os = "macos")]
        self.present_with_transaction(false);
    }

    /// Presents drawables inside Core Animation transactions, while the frame moves.
    #[cfg(target_os = "macos")]
    pub(in crate::window) fn present_with_transaction(&mut self, on: bool) {
        if let Some(surface) = &self.surface { self.native.materials.set_moving(surface, on); }
    }

    /// Presents the frame in the transaction that resizes the window while the user drags its
    /// edge. Before the drawable is acquired: wgpu reads `presentsWithTransaction` then.
    #[cfg(target_os = "macos")]
    pub(in crate::window) fn set_live_resize(&mut self, on: bool) {
        if let Some(surface) = &self.surface { self.native.materials.set_live(&self.window, surface, on); }
    }

    pub(in crate::window) fn focus(&self) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.native.panel { panel.request_keys(); return; }
        self.window.focus_window();
    }

    /// Whether this window is backed by an adopted macOS `NSPanel`.
    pub(in crate::window) fn is_panel(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.native.panel.is_some();
        #[cfg(not(target_os = "macos"))]
        false
    }

    /// Whether the native window is currently minimized. Adopted macOS panels never minimize,
    /// avoiding a `isMiniaturized` query on winit's hidden source `NSWindow`.
    pub(in crate::window) fn is_minimized_native(&self) -> bool {
        !self.is_panel() && self.window.is_minimized() == Some(true)
    }

    /// Whether the native window is currently maximized. Adopted macOS panels never maximize,
    /// avoiding winit's borderless `is_zoomed` style-mask mutation on the hidden source `NSWindow`.
    pub(in crate::window) fn is_maximized(&self) -> bool {
        if self.is_panel() { return false; }
        #[cfg(target_os = "macos")]
        { crate::window::chrome::mac::is_maximized(&self.window) }
        #[cfg(not(target_os = "macos"))]
        { self.window.is_maximized() }
    }

    /// Reconciles the macOS full-screen watch against the live window's winit and AppKit flags.
    #[cfg(target_os = "macos")]
    pub(in crate::window) fn sync_full_screen(&self) {
        if self.is_panel() { return; }
        let winit_full_screen = self.window.fullscreen().is_some();
        let style_mask_full_screen = crate::window::chrome::mac::style_mask_full_screen(&self.window);
        self.native.full_screen.sync(winit_full_screen, style_mask_full_screen);
    }

    /// Current macOS full-screen transition phase. Adopted panels always stay `Windowed`.
    #[cfg(target_os = "macos")]
    pub(in crate::window) fn full_screen_state(&self) -> crate::window::chrome::full_screen::State {
        if self.is_panel() { return crate::window::chrome::full_screen::State::Windowed; }
        self.native.full_screen.state()
    }

    /// Whether the native window is currently full screen. Adopted macOS panels never enter
    /// full screen. On macOS this reads the window's transition watch so full-screen state flips
    /// at AppKit's `WillEnter` / `WillExit` notifications instead of waiting for winit's `DidExit`.
    pub(in crate::window) fn is_full_screen(&self) -> bool {
        #[cfg(target_os = "macos")]
        { crate::window::chrome::full_screen::window_is_full_screen(self.native.full_screen.state(), self.is_panel()) }
        #[cfg(not(target_os = "macos"))]
        { !self.is_panel() && self.window.fullscreen().is_some() }
    }

    /// Snapshot of the live native window's state and bounds for recreating it the same way
    /// after hide/release or Wayland unmapping. Adopted macOS panels ignore maximize, minimize,
    /// and full screen.
    pub(in crate::window) fn live_state(&self, wayland: bool) -> crate::window::state::LiveWindowState {
        let panel = self.is_panel();
        let (maximized, full_screen, minimized) = if panel {
            (false, false, false)
        } else {
            (self.is_maximized(), self.is_full_screen(), self.is_minimized_native())
        };
        let inner = self.window.inner_size();
        let scale = self.window.scale_factor();
        let (logical_size, physical_size) = if inner.width > 0 && inner.height > 0 && scale > 0.0 && scale.is_finite() {
            (
                Some((inner.width as f32 / scale as f32, inner.height as f32 / scale as f32)),
                Some((inner.width, inner.height)),
            )
        } else {
            (None, None)
        };
        let position = if wayland {
            None
        } else if let Ok(pos) = self.window.outer_position() {
            #[cfg(target_os = "macos")]
            let pos = if scale > 0.0 && scale.is_finite() {
                winit::dpi::Position::Logical(winit::dpi::LogicalPosition::new(
                    f64::from(pos.x) / scale,
                    f64::from(pos.y) / scale,
                ))
            } else {
                winit::dpi::Position::Physical(pos)
            };
            #[cfg(not(target_os = "macos"))]
            let pos = winit::dpi::Position::Physical(pos);
            Some(pos)
        } else {
            None
        };
        #[cfg(target_os = "windows")]
        let restore_frame = ((maximized || minimized) && !full_screen)
            .then(|| crate::platform::win32::restore_frame(&self.window))
            .flatten();
        #[cfg(not(target_os = "windows"))]
        let restore_frame = None;
        crate::window::state::LiveWindowState {
            maximized,
            full_screen,
            minimized,
            logical_size,
            physical_size,
            position,
            restore_frame,
        }
    }

    /// Carries out a titlebar action (`Drag` or `DoubleClick`) on the window's live native
    /// target. Adopted macOS panels drag the `NSPanel` directly and ignore `DoubleClick` so
    /// neither `miniaturize:` nor `zoom:` (nor winit's borderless `setStyleMask:` / frame
    /// fallback) can reach winit's hidden source `NSWindow`.
    pub(in crate::window) fn titlebar_action(&self, action: crate::runtime::TitlebarAction) {
        match crate::window::chrome::titlebar_target(action, self.is_panel()) {
            crate::window::chrome::TitlebarTarget::DragPanel => {
                #[cfg(target_os = "macos")]
                if let Some(panel) = &self.native.panel { panel.drag_window(); }
            }
            crate::window::chrome::TitlebarTarget::DragWindow => {
                self.window.drag_window().ok();
            }
            crate::window::chrome::TitlebarTarget::DoubleClickWindow => {
                crate::window::chrome::titlebar_double_click(&self.window);
            }
            crate::window::chrome::TitlebarTarget::Ignore => {}
        }
    }

    /// Carries out a titlebar action while fencing `TitlebarAction::DoubleClick` in `rt`'s press
    /// machine so clicks whose native timestamps precede the end of a blocking toggle (macOS
    /// `NSWindow::zoom:`) are dropped instead of replaying queued drags and zooms.
    pub(in crate::window) fn run_titlebar_action(
        &self,
        rt: &mut crate::runtime::RuntimeCore,
        action: crate::runtime::TitlebarAction,
    ) {
        rt.run_titlebar_action_with(action, |a| self.titlebar_action(a));
    }



    /// Maximizes or restores this window. Adopted macOS panels ignore it.
    pub(in crate::window) fn toggle_maximized(&self) {
        if self.is_panel() { return; }
        crate::window::chrome::toggle_maximized(&self.window);
    }

    /// Sets this window's full-screen state (`true` to enter, `false` to exit). Adopted macOS
    /// panels ignore it.
    pub(in crate::window) fn set_full_screen(&self, full_screen: bool) {
        if self.is_panel() { return; }
        #[cfg(target_os = "windows")]
        let changing = full_screen != self.is_full_screen();
        #[cfg(target_os = "windows")]
        if changing {
            // Cloak with DWM transitions disabled until the resized frame lands so DWM never
            // composes the classic frame, a transition ghost, or a stale-size swap chain.
            self.native.cloak.hold_transition(&self.window, crate::time::now() + READY_FALLBACK);
        }
        crate::window::chrome::set_full_screen(&self.window, full_screen);
        #[cfg(target_os = "windows")]
        if changing {
            self.request_redraw();
        }
    }

    /// Takes this window full screen or brings it back. Adopted macOS panels ignore it.
    pub(in crate::window) fn toggle_full_screen(&self) {
        #[cfg(target_os = "macos")]
        {
            if let Some(target) = crate::window::chrome::full_screen::toggle_target(self.native.full_screen.state(), self.is_panel()) {
                self.set_full_screen(target);
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            if self.is_panel() { return; }
            self.set_full_screen(!self.is_full_screen());
        }
    }

    /// Starts an interactive OS/compositor window resize from `edge`, returning whether the
    /// platform accepted the resize request. Ignored on macOS and on adopted panels.
    pub(in crate::window) fn drag_resize_window(&self, edge: crate::ResizeEdge) -> bool {
        if self.is_panel() { return false; }
        #[cfg(not(target_os = "macos"))]
        {
            self.window.drag_resize_window(edge.into()).is_ok()
        }
        #[cfg(target_os = "macos")]
        {
            let _ = edge;
            false
        }
    }
}

/// `on_ready` starts an app's live work, so it can't wait on a drawable: a window launched
/// covered, occluded or hidden by the system may not draw for as long as it stays that way.
/// It runs at the first frame skipped for being off screen, or `READY_FALLBACK` after opening
/// when no frame comes at all; a window on screen still runs it after its first frame.
#[test]
fn on_ready_runs_for_a_window_launched_occluded_or_hidden() {
    let now = crate::time::now();
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
