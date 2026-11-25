//! Opt-in permanent CPU presentation path for transparent overlays on Windows
//! (`ConfiguredOverlay::renderer(RendererKind::Cpu)`).
//!
//! Keeps the default `RendererKind::Gpu` overlay path untouched (`overlay/draw.rs` and
//! `overlay/surface.rs` take zero new branches for GPU overlays). On Windows, a CPU overlay creates
//! zero `wgpu::Instance`/`Adapter`/`Device`/`Surface`/`Renderer` objects, paints premultiplied
//! transparent BGRA frames (`clear = [0.0; 4]`, `Scene::lcd(false)`) through `CpuPainter`, and
//! presents them through documented Win32 `UpdateLayeredWindowIndirect` (`ULW_ALPHA`) via
//! `platform::win32::layered::LayeredPresenter`.

#[cfg(any(target_os = "windows", test))]
use crate::render::cpu_boot::{CpuPainter, NeedsGpu, Scene, Target};
#[cfg(any(target_os = "windows", test))]
use crate::runtime::RuntimeCore;
use crate::RendererKind;

#[cfg(target_os = "windows")]
use crate::platform::win32::layered::{self, LayeredPresenter, LayeredSeam, RealLayeredSeam};
#[cfg(all(test, not(target_os = "windows")))]
use crate::platform::win32_layered::{LayeredPresenter, LayeredSeam};

/// Target OS classification for the pure overlay backend selection policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OverlayTarget {
    Windows,
    MacOs,
    Linux,
    Other,
}

impl OverlayTarget {
    #[inline]
    pub(crate) const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }
}

/// Pure backend selection decision for an overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OverlayBackendChoice {
    /// Default GPU presentation through `wgpu`.
    Gpu,
    /// Opt-in Windows permanent CPU layered presentation (`UpdateLayeredWindowIndirect`).
    WindowsCpuLayered,
    /// `RendererKind::Cpu` was requested alongside `.compositor(...)`; stay on GPU and log once.
    GpuCompositorOverride,
    /// `RendererKind::Cpu` was requested, but Win32 layered setup failed once; stay on GPU.
    GpuAfterLayeredFailure,
    /// `RendererKind::Cpu` was requested on a platform that ignores it in Step 1 (stay on GPU).
    GpuPlatformUnsupported,
}

impl OverlayBackendChoice {
    #[inline]
    pub(crate) const fn is_cpu_layered(self) -> bool {
        matches!(self, Self::WindowsCpuLayered)
    }

    #[inline]
    pub(crate) const fn renderer_kind(self) -> RendererKind {
        if self.is_cpu_layered() {
            RendererKind::Cpu
        } else {
            RendererKind::Gpu
        }
    }
}

/// Pure selection policy deciding whether an overlay enters the Windows CPU layered path.
///
/// An overlay enters `WindowsCpuLayered` if and only if:
/// `target == Windows && requested == RendererKind::Cpu && !has_compositor && !cpu_layered_failed`.
#[inline]
pub(crate) const fn select_overlay_backend(
    target: OverlayTarget,
    requested: RendererKind,
    has_compositor: bool,
    cpu_layered_failed: bool,
) -> OverlayBackendChoice {
    match requested {
        RendererKind::Gpu => OverlayBackendChoice::Gpu,
        RendererKind::Cpu => {
            if !matches!(target, OverlayTarget::Windows) {
                OverlayBackendChoice::GpuPlatformUnsupported
            } else if has_compositor {
                OverlayBackendChoice::GpuCompositorOverride
            } else if cpu_layered_failed {
                OverlayBackendChoice::GpuAfterLayeredFailure
            } else {
                OverlayBackendChoice::WindowsCpuLayered
            }
        }
    }
}

/// How a permanent CPU overlay reacts when `CpuPainter` reports `NeedsGpu`.
///
/// Unlike opaque-window `CpuFirst`, a CPU overlay never silently hands over to the GPU (which
/// would discard its 42-129 MiB memory saving).
#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NeedsGpuAction {
    /// Paint live via `Scene::degraded(true)`, log once, and note the reason in `surface_description`.
    DegradedPaint(NeedsGpu),
    /// Non-degradable reason after a frame was already presented: log once and hold previous pixels.
    HoldPreviousFrame(NeedsGpu),
    /// Non-degradable reason on frame 1 before anything presented: mark `Availability::Unavailable`.
    UnavailableFrameOne(NeedsGpu),
}

#[cfg(any(target_os = "windows", test))]
#[inline]
pub(crate) const fn classify_needs_gpu(reason: NeedsGpu, has_presented: bool) -> NeedsGpuAction {
    if reason.is_degradable() {
        NeedsGpuAction::DegradedPaint(reason)
    } else if has_presented {
        NeedsGpuAction::HoldPreviousFrame(reason)
    } else {
        NeedsGpuAction::UnavailableFrameOne(reason)
    }
}

/// Formats `OverlayHandle::surface_description()` for a Windows CPU layered overlay.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn format_surface_description(degraded: Option<NeedsGpu>) -> String {
    match degraded {
        None => "CpuPainter (Win32 layered, ULW_ALPHA)".to_owned(),
        Some(reason) => format!("CpuPainter (Win32 layered, degraded: {reason:?})"),
    }
}

/// Bit index for once-per-reason logging of `NeedsGpu` variants.
#[cfg(any(target_os = "windows", test))]
const fn needs_gpu_bit(reason: NeedsGpu) -> u16 {
    match reason {
        NeedsGpu::LinearBlend => 1 << 0,
        NeedsGpu::NativeSurface => 1 << 1,
        NeedsGpu::SlugText => 1 << 2,
        NeedsGpu::LayerBlur => 1 << 3,
        NeedsGpu::TurnedLayer => 1 << 4,
        NeedsGpu::BackdropBlur => 1 << 5,
        NeedsGpu::Effect => 1 << 6,
        NeedsGpu::OutlineGlyph => 1 << 7,
        NeedsGpu::UnknownKind => 1 << 8,
        NeedsGpu::Target => 1 << 9,
    }
}

#[cfg(target_os = "windows")]
use crate::platform::win32::backdrop::BackdropInputs as WinBackdropInputs;
#[cfg(all(test, not(target_os = "windows")))]
use crate::platform::win32_backdrop::BackdropInputs as WinBackdropInputs;

/// Builds Windows DWM backdrop inputs for a CPU overlay (`cpu_painter: true`, `alpha_mode: None`,
/// `device_type: None`) so `surface_supports_dwm` is always `false` and zero DWM calls are issued.
#[cfg(any(target_os = "windows", test))]
pub(crate) const fn cpu_backdrop_inputs(requested: crate::WindowBackdrop, build: Option<u32>) -> WinBackdropInputs {
    WinBackdropInputs {
        requested, build, transparent: true,
        swapchain: wgpu::Dx12SwapchainKind::DxgiFromVisual,
        alpha_mode: None, cpu_painter: true, device_type: None,
    }
}

/// Result of painting and presenting one CPU overlay frame through [`CpuPaintEngine`].
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CpuPaintOutcome {
    /// Painted and presented to the layered window (optionally degraded).
    Presented {
        degraded: Option<NeedsGpu>,
        newly_logged: Option<NeedsGpu>,
    },
    /// Steady frame with zero dirty pixels; skipped `UpdateLayeredWindowIndirect`.
    Unchanged {
        degraded: Option<NeedsGpu>,
    },
    /// Non-degradable `NeedsGpu` mid-session: held the previously presented pixels untouched.
    HeldPreviousFrame {
        reason: NeedsGpu,
        newly_logged: bool,
    },
    /// Non-degradable `NeedsGpu` on frame 1: overlay must report `Availability::Unavailable`.
    FrameOneUnavailable {
        reason: NeedsGpu,
        message: String,
    },
    /// Win32 DIB creation or `UpdateLayeredWindowIndirect` failed.
    SeamFailed {
        error: String,
        initial: bool,
    },
}

/// Paint + layered-present state machine generic over [`LayeredSeam`], tested directly off screen
/// on every platform and instantiated with `RealLayeredSeam` on Windows.
#[cfg(any(target_os = "windows", test))]
pub(crate) struct CpuPaintEngine<S: LayeredSeam> {
    painter: CpuPainter,
    presenter: LayeredPresenter<S>,
    logged_needs_gpu: u16,
    active_degraded: Option<NeedsGpu>,
}

#[cfg(any(target_os = "windows", test))]
impl<S: LayeredSeam> CpuPaintEngine<S> {
    pub(crate) fn new(seam: S, threads: usize) -> Self {
        Self {
            painter: CpuPainter::new(threads),
            presenter: LayeredPresenter::new(seam),
            logged_needs_gpu: 0,
            active_degraded: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn presenter(&self) -> &LayeredPresenter<S> {
        &self.presenter
    }


    pub(crate) fn active_degraded(&self) -> Option<NeedsGpu> {
        self.active_degraded
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn invalidate_full(&mut self) {
        self.presenter.invalidate_full();
    }

    /// Frees the retained DIB buffer and forgets painter tile hashes when the overlay hides.
    pub(crate) fn on_hide(&mut self) {
        self.presenter.release_surface();
        self.painter.forget();
    }

    /// Re-presents the already-painted DIB at `alpha` during `Presence::FadingOut` (or restores
    /// `255` on cancel) without repainting `CpuPainter`.
    pub(crate) fn present_fade(
        &mut self,
        hwnd: isize,
        origin: (i32, i32),
        alpha: u8,
    ) -> Result<bool, String> {
        self.presenter.present_fade(hwnd, origin, alpha)
    }

    fn note_logged_bit(mask: &mut u16, reason: NeedsGpu) -> bool {
        let bit = needs_gpu_bit(reason);
        let first = (*mask & bit) == 0;
        *mask |= bit;
        first
    }

    /// Paints `rt` into the retained premultiplied BGRA surface (`clear = [0.0; 4]`, `lcd = false`)
    /// and presents through `LayeredPresenter`. Never hands over to the GPU on `NeedsGpu`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_and_present(
        &mut self,
        name: &str,
        hwnd: isize,
        origin: (i32, i32),
        size: (u32, u32),
        alpha: u8,
        rt: &mut RuntimeCore,
    ) -> CpuPaintOutcome {
        let has_presented = self.presenter.has_presented();
        let initial = !has_presented;
        self.painter.fill_masks(&mut rt.text.atlas);
        let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, size, [0.0; 4])
            .lcd(false)
            .srgb_blend(rt.srgb_blend())
            .fade_stops(rt.fade_stops())
            .native_surfaces(!rt.surfaces.is_empty())
            .compositor(false)
            .font_smoothing(rt.font_smoothing());
        let paint_step = {
            let (pixels, stride, reallocated) = match self.presenter.ensure_surface(size) {
                Ok(surf) => surf,
                Err(error) => return CpuPaintOutcome::SeamFailed { error, initial },
            };
            if reallocated {
                self.painter.forget();
            }
            let mut target = Target::with_stride(pixels, size.0, size.1, stride);
            match self.painter.paint(&scene, &mut target) {
                Ok(painted) => Ok((painted, None, None)),
                Err(why) => match classify_needs_gpu(why, has_presented) {
                    NeedsGpuAction::DegradedPaint(reason) => {
                        let first = Self::note_logged_bit(&mut self.logged_needs_gpu, reason);
                        if first {
                            eprintln!(
                                "rune: overlay {name} on CPU renderer degraded GPU-only feature {reason:?}; \
                                 check Cx::renderer_kind() to adapt or use RendererKind::Gpu"
                            );
                        }
                        let degraded_scene = scene.degraded(true);
                        match self.painter.paint(&degraded_scene, &mut target) {
                            Ok(painted) => Ok((painted, Some(reason), first.then_some(reason))),
                            Err(why2) => Err(why2),
                        }
                    }
                    NeedsGpuAction::HoldPreviousFrame(reason)
                    | NeedsGpuAction::UnavailableFrameOne(reason) => Err(reason),
                },
            }
        };
        let (painted, degraded, newly_logged) = match paint_step {
            Ok(ok) => ok,
            Err(reason) => return self.handle_non_degradable(name, reason),
        };
        self.active_degraded = degraded;
        match self.presenter.present_frame(hwnd, origin, size, alpha, &painted.rects) {
            Ok(true) => CpuPaintOutcome::Presented { degraded, newly_logged },
            Ok(false) => CpuPaintOutcome::Unchanged { degraded },
            Err(error) => CpuPaintOutcome::SeamFailed { error, initial },
        }
    }

    fn handle_non_degradable(&mut self, name: &str, reason: NeedsGpu) -> CpuPaintOutcome {
        let first = Self::note_logged_bit(&mut self.logged_needs_gpu, reason);
        if first {
            eprintln!(
                "rune: overlay {name} on CPU renderer hit non-degradable GPU-only feature {reason:?}; \
                 switch to RendererKind::Gpu or avoid {reason:?} on CPU overlays"
            );
        }
        if self.presenter.has_presented() {
            CpuPaintOutcome::HeldPreviousFrame { reason, newly_logged: first }
        } else {
            CpuPaintOutcome::FrameOneUnavailable {
                reason,
                message: format!(
                    "CPU overlay cannot render frame 1: {reason:?} requires RendererKind::Gpu"
                ),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Windows live overlay integration (zero wgpu/GPU objects created)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn cpu_overlay_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4)
}

/// Live state of a Windows transparent overlay running permanently on `CpuPainter` +
/// `UpdateLayeredWindowIndirect` (`ULW_ALPHA`).
#[cfg(target_os = "windows")]
pub(super) struct CpuOverlay {
    pub(super) a11y: Option<crate::a11y::Adapter>,
    pub(super) poll_pace: crate::window::poll_pace::PollPace,
    pub(super) pace_grid: Option<crate::window::poll_pace::VSyncGrid>,
    pub(super) engine: CpuPaintEngine<RealLayeredSeam>,
    pub(super) window: std::sync::Arc<winit::window::Window>,
    pub(super) scale: f32,
    pub(super) size: (u32, u32),
}

#[cfg(target_os = "windows")]
impl super::OverlayCore {
    /// Whether this overlay should run on the Windows CPU layered path right now.
    pub(super) fn uses_cpu_layered(&mut self, app: &dyn super::OverlayApp) -> bool {
        let choice = select_overlay_backend(
            OverlayTarget::current(),
            self.requested_renderer,
            app.has_compositor(),
            self.cpu_layered_failed,
        );
        if choice == OverlayBackendChoice::GpuCompositorOverride && !self.compositor_override_logged {
            self.compositor_override_logged = true;
            eprintln!(
                "rune: overlay {} requested RendererKind::Cpu together with a Compositor; \
                 ignoring RendererKind::Cpu and running on GPU",
                self.name
            );
        }
        choice.is_cpu_layered()
    }

    pub(super) fn has_cpu(&self) -> bool {
        self.cpu.is_some()
    }

    fn cpu_window_origin(&self, window: &winit::window::Window) -> (i32, i32) {
        window
            .outer_position()
            .ok()
            .or(self.origin)
            .map_or((0, 0), |p| (p.x, p.y))
    }

    /// Creates the Windows layered overlay window and CPU painter without touching `gpu_pool`.
    /// Returns `true` if CPU setup succeeded, or `false` if it failed or fell back to GPU.
    pub(super) fn prepare_cpu(
        &mut self,
        app: &mut dyn super::OverlayApp,
        event_loop: &winit::event_loop::ActiveEventLoop,
        showing: bool,
    ) -> bool {
        use winit::dpi::LogicalSize;
        use winit::platform::windows::WindowAttributesExtWindows;
        use winit::window::{Window, WindowLevel};

        if self.cpu.is_some() || (!showing && self.defers_gpu()) { return true; }
        self.fit_for_show_in(app, Some(event_loop));
        self.resolve_size(event_loop);
        let flags = layered::window_attrs_for_cpu_layered();
        let mut attrs = Window::default_attributes()
            .with_title(self.name)
            .with_inner_size(LogicalSize::new(self.at.size.0 as f64, self.at.size.1 as f64))
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(flags.transparent)
            .with_active(false)
            .with_visible(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_skip_taskbar(true)
            .with_undecorated_shadow(false)
            .with_no_redirection_bitmap(flags.no_redirection_bitmap);
        if let Some(p) = super::placement::position(event_loop, self.at, &self.options) {
            attrs = attrs.with_position(p);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(window) => std::sync::Arc::new(window),
            Err(error) => {
                self.fail(app, format!("could not create its window: {error}"));
                return false;
            }
        };
        crate::window::displays::track(&window);
        self.dark = window.theme().map_or_else(
            || crate::platform::preferences::desktop_dark(&window).unwrap_or(true),
            |t| t == winit::window::Theme::Dark,
        );
        app.rt_mut().set_system_dark(self.dark);
        if self.cursor.is_none() {
            self.cursor = Some(crate::cursor::os::OsCursor::new());
        }
        window
            .set_cursor_hittest(self.options.input.resolved(self.interactive) == crate::OverlayInput::All)
            .ok();
        let wake = self.shared.wake.lock().unwrap().clone();
        let native = match crate::platform::win32::OverlayWindow::new_layered_cpu(
            &window,
            self.interactive,
            &self.options,
            wake.clone(),
        ) {
            Ok(native) => native,
            Err(error) => {
                drop(window);
                self.fallback_from_cpu_layered(app, event_loop, showing, error);
                return false;
            }
        };
        let a11y = wake.and_then(|w| crate::a11y::Adapter::new(&window, w));
        let scale = crate::window::backing_scale(&window);
        let size = layered::bounded_layered_size(self.at.size, scale);
        let mut engine = CpuPaintEngine::new(RealLayeredSeam, cpu_overlay_threads());
        if let Err(error) = engine.presenter.ensure_surface(size) {
            drop(native);
            drop(a11y);
            drop(window);
            self.fallback_from_cpu_layered(app, event_loop, showing, error);
            return false;
        }
        self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities::placed(
            native.capture_exclusion(),
            super::keys::keys_support(self.options.keys, true),
            crate::OverlaySupport::Supported,
        );
        self.native = Some(native);
        {
            let rt = app.rt_mut();
            crate::render::cpu_boot::attach_text(&mut rt.text.atlas);
            rt.transparent = true;
            rt.set_viewport(self.at.size, scale);
            rt.set_renderer_kind(RendererKind::Cpu);
            #[cfg(feature = "windows-system-material")]
            rt.paint.set_windows_acrylic_materials(false);
        }
        *self.shared.surface.lock().unwrap() = Some(format_surface_description(None));
        self.place_window(app.rt_mut(), event_loop, &window);
        self.cpu = Some(CpuOverlay {
            a11y,
            poll_pace: crate::window::poll_pace::PollPace::default(),
            pace_grid: None,
            engine,
            window,
            scale,
            size,
        });
        self.sync_window_backdrop(app.rt_mut(), false);
        self.shared.failure.lock().unwrap().take();
        self.set_availability_notify(app, super::Availability::Available);
        if !self.animate_reveal {
            app.rt_mut().settle_all();
        }
        self.dirty = true;
        self.idle = true;
        if self.options.prewarm && !(showing && self.defers_gpu()) {
            app.frame(0.0);
            app.rt_mut().settle_all();
            app.frame(0.0);
            self.clock.hold(crate::time::now());
            self.publish_regions(app.rt());
        }
        if !self.surface.announced() && let Some(created) = &mut self.on_created {
            self.surface.announce();
            let (window_number, capabilities) = *self.shared.native.lock().unwrap();
            created(crate::OverlayCreated { window_number, capabilities });
        }
        true
    }

    /// One-shot fallback from a failed Windows CPU layered setup to the normal GPU overlay path.
    fn fallback_from_cpu_layered(
        &mut self,
        app: &mut dyn super::OverlayApp,
        event_loop: &winit::event_loop::ActiveEventLoop,
        showing: bool,
        error: String,
    ) {
        eprintln!(
            "rune: overlay {} CPU layered setup failed ({error}); falling back to GPU",
            self.name
        );
        self.cpu_layered_failed = true;
        self.release_cpu();
        app.rt_mut().set_renderer_kind(RendererKind::Gpu);
        self.prepare(app, event_loop, showing);
    }

    /// Shows a Windows CPU layered overlay, painting frame 1 into the DIB before revealing so
    /// initial `UpdateLayeredWindowIndirect` errors fall back cleanly before showing a blank window.
    /// Returns `true` if handled on CPU, or `false` if it fell back to the GPU path.
    pub(super) fn show_cpu(
        &mut self,
        app: &mut dyn super::OverlayApp,
        event_loop: &winit::event_loop::ActiveEventLoop,
        fade: std::time::Duration,
    ) -> bool {
        let fading = self.presence.fading();
        let prev_size = self.at.size;
        self.resolve_size(event_loop);
        if self.cpu.is_none() && !self.prepare_cpu(app, event_loop, true) {
            return !self.cpu_layered_failed;
        }
        let Some(window) = self.cpu.as_ref().map(|c| c.window.clone()) else { return true };
        self.resume_live(app);
        if prev_size != self.at.size { self.resized(app, event_loop); }
        if !fading { self.settle_before_reveal(app.rt_mut()); }
        if self.position_owner.is_managed() { self.place_window(app.rt_mut(), event_loop, &window); }
        else { self.sync_origin(app.rt_mut(), &window); }
        self.rescale_cpu(app, crate::window::backing_scale(&window));
        if let Some(cpu) = self.cpu.as_mut() { cpu.engine.invalidate_full(); }
        let now = crate::time::now();
        let (animating, _dt) = super::frame::step(app, &mut self.clock, now, false);
        if let Some(next) = self.take_live_fit_resize(app.rt_mut()) {
            self.apply_content_resize_cpu(app, Some(event_loop), next);
        }
        self.sync_cpu_after_step(app);
        crate::system_material::announce(&mut self.on_materials, app.rt());
        let origin = self.origin.map_or_else(|| self.cpu_window_origin(&window), |p| (p.x, p.y));
        let Some(hwnd) = crate::platform::win32::hwnd(&window) else {
            self.fallback_from_cpu_layered(app, event_loop, true, "overlay window has no HWND".into());
            return false;
        };
        let outcome = {
            let Some(cpu) = self.cpu.as_mut() else { return true };
            let size = cpu.size;
            cpu.engine.paint_and_present(self.name, hwnd as isize, origin, size, 255, app.rt_mut())
        };
        match outcome {
            CpuPaintOutcome::Presented { degraded, .. } | CpuPaintOutcome::Unchanged { degraded } => {
                *self.shared.surface.lock().unwrap() = Some(format_surface_description(degraded));
            }
            CpuPaintOutcome::HeldPreviousFrame { .. } => {}
            CpuPaintOutcome::FrameOneUnavailable { message, .. } => {
                self.fail(app, message);
                return true;
            }
            CpuPaintOutcome::SeamFailed { error, .. } => {
                self.fallback_from_cpu_layered(app, event_loop, true, error);
                return false;
            }
        }
        self.show_window(app, &window, fade);
        self.sync_window_backdrop(app.rt_mut(), true);
        super::frame::record_presented(false);
        let effects = self.presence.step(super::presence::Event::Show, self.release);
        self.carry_out(app, effects);
        self.publish_screen_transform(app.rt());
        self.hidden_pace.reset();
        self.backoff_step(crate::window::BackoffEvent::Revealed);
        let submitted = crate::time::now();
        self.meter.presented();
        self.note_presented(submitted);
        self.dirty = false;
        self.idle = super::frame::idle_after(app.rt(), animating, None);
        self.runtime_idle = self.idle;
        self.pace_cpu_after_present(submitted);
        true
    }

    /// Steps a fading-out Windows CPU overlay's `SourceConstantAlpha` without repainting `CpuPainter`.
    pub(super) fn fade_cpu(&mut self, alpha: u8) {
        let Some(window) = self.cpu.as_ref().map(|c| c.window.clone()) else { return };
        let Some(hwnd) = crate::platform::win32::hwnd(&window) else { return };
        let origin = self.cpu_window_origin(&window);
        if let Some(cpu) = self.cpu.as_mut()
            && let Err(error) = cpu.engine.present_fade(hwnd as isize, origin, alpha)
        {
            eprintln!("rune: overlay {} CPU layered fade failed: {error}", self.name);
        }
    }

    /// Frees the retained DIB buffer when a CPU overlay finishes hiding.
    pub(super) fn on_hide_cpu(&mut self) {
        if let Some(cpu) = self.cpu.as_mut() {
            cpu.engine.on_hide();
            let _ = cpu.poll_pace.step(crate::window::poll_pace::PollPaceEvent::Reset);
            cpu.pace_grid = None;
        }
    }

    /// Rescales a live CPU overlay after a size or monitor DPI change.
    pub(super) fn rescale_cpu(&mut self, app: &mut dyn super::OverlayApp, scale: f32) {
        let Some(cpu) = self.cpu.as_mut() else { return };
        let next_size = layered::bounded_layered_size(self.at.size, scale);
        if (cpu.scale - scale).abs() < f32::EPSILON && cpu.size == next_size { return; }
        (cpu.scale, cpu.size) = (scale, next_size);
        cpu.engine.invalidate_full();
        app.rt_mut().set_viewport(self.at.size, scale);
        self.publish_screen_transform(app.rt());
        self.dirty = true;
    }

    /// Resizes a live CPU overlay window and refreshes placement/origin/scale.
    pub(super) fn resized_cpu(&mut self, app: &mut dyn super::OverlayApp, event_loop: &winit::event_loop::ActiveEventLoop) {
        let Some(window) = self.cpu.as_ref().map(|c| c.window.clone()) else { return };
        let _size = window.request_inner_size(winit::dpi::LogicalSize::new(self.at.size.0 as f64, self.at.size.1 as f64));
        if self.position_owner.is_managed() { self.place_window(app.rt_mut(), event_loop, &window); }
        else { self.sync_origin(app.rt_mut(), &window); }
        self.rescale_cpu(app, crate::window::backing_scale(&window));
    }

    /// Releases the CPU overlay's DIB, a11y adapter, Win32 subclass, and window in safe order.
    pub(super) fn release_cpu(&mut self) {
        let cpu = self.cpu.take();
        self.clear_screen_transform();
        if let Some(CpuOverlay { a11y, engine, window, .. }) = cpu {
            drop(a11y);
            drop(engine);
            self.native.take();
            self.backdrop_tracker = Default::default();
            drop(window);
            *self.shared.surface.lock().unwrap() = None;
        }
    }

    fn sync_cpu_after_step(&mut self, app: &mut dyn super::OverlayApp) {
        if let Some(backdrop) = app.rt_mut().host.window_backdrop_request.take() {
            self.window_backdrop = backdrop;
            self.sync_window_backdrop(app.rt_mut(), false);
        }
        self.take_window_requests(app.rt_mut());
        self.clipboard.flush_core(app.rt_mut());
        if self.options.keys.takes_keys() && let Some(window) = self.cpu.as_ref().map(|c| c.window.clone()) {
            let focused = window.has_focus();
            crate::input::host_events::sync_ime(&window, app.rt_mut(), &mut self.ime, focused);
        }
        self.publish_regions(app.rt());
    }

    fn pace_cpu_after_present(&mut self, submitted: crate::time::Instant) {
        if self.idle {
            self.backoff.step(crate::window::BackoffEvent::Lapsed);
            self.meter.set_pacing(crate::Pacing::Idle);
            return;
        }
        let next_tick = if let Some(cpu) = self.cpu.as_mut() {
            let mhz = cpu
                .window
                .current_monitor()
                .and_then(|m| m.refresh_rate_millihertz());
            let grid = crate::platform::win32::vsync_grid(submitted, mhz, cpu.pace_grid);
            cpu.pace_grid = grid;
            grid.map_or(submitted + crate::window::TIMER_FRAME, |g| {
                crate::window::poll_pace::next_tick(submitted, g)
            })
        } else {
            submitted + crate::window::TIMER_FRAME
        };
        self.backoff.step(crate::window::BackoffEvent::At(next_tick));
        self.meter.set_pacing(crate::Pacing::Unpaced);
    }

    /// Draws one frame on the Windows CPU layered path and paces animating frames via
    /// `PollPace` + `vsync_grid` (stopping all wakes when idle). Always returns `false` so the
    /// event loop sleeps until the next vblank tick instead of spinning `ControlFlow::Poll`.
    pub(super) fn draw_cpu(&mut self, app: &mut dyn super::OverlayApp, event_loop: Option<&winit::event_loop::ActiveEventLoop>) -> bool {
        let now = crate::time::now();
        self.presence_due(app, now);
        if !self.presence.drawing() {
            crate::haptics::flush_live(app.rt_mut(), false);
            return false;
        }
        self.apply_input(app.rt());
        if self.cpu.is_none() { return false; }
        let gate = super::frame::Gate {
            prewarm: false, occluded: false, awaiting_frame: false,
            dirty: self.dirty || app.rt().has_input_waiting(),
            idle: self.idle, retry: self.backoff.deadline(),
            missed_this_turn: self.turn_frame.missed(),
        };
        if !matches!(super::frame::turn(gate, crate::time::now), super::frame::Turn::Draw) {
            self.open_popup(app);
            return false;
        }
        self.backoff_step(crate::window::BackoffEvent::Lapsed);
        self.meter.redraw(true);
        let frame_now = crate::time::now();
        let (animating, _dt) = super::frame::step(app, &mut self.clock, frame_now, false);
        if let Some(next) = self.take_live_fit_resize(app.rt_mut()) {
            self.apply_content_resize_cpu(app, event_loop, next);
        }
        self.sync_cpu_after_step(app);
        crate::system_material::announce(&mut self.on_materials, app.rt());
        let alpha = match self.presence {
            super::presence::Presence::FadingOut { until, length, .. } => {
                super::fade_alpha(until, length, frame_now)
            }
            _ => 255,
        };
        let Some(window) = self.cpu.as_ref().map(|c| c.window.clone()) else { return false };
        let origin = self.origin.map_or_else(|| self.cpu_window_origin(&window), |p| (p.x, p.y));
        let Some(hwnd) = crate::platform::win32::hwnd(&window) else { return false };
        let prev_degraded = self.cpu.as_ref().and_then(|c| c.engine.active_degraded());
        let outcome = {
            let Some(cpu) = self.cpu.as_mut() else { return false };
            let size = cpu.size;
            cpu.engine.paint_and_present(self.name, hwnd as isize, origin, size, alpha, app.rt_mut())
        };
        let mut presented = false;
        match outcome {
            CpuPaintOutcome::Presented { degraded, .. } => {
                presented = true;
                if degraded != prev_degraded {
                    *self.shared.surface.lock().unwrap() = Some(format_surface_description(degraded));
                }
                super::frame::record_presented(false);
            }
            CpuPaintOutcome::Unchanged { degraded } => {
                if degraded != prev_degraded {
                    *self.shared.surface.lock().unwrap() = Some(format_surface_description(degraded));
                }
            }
            CpuPaintOutcome::HeldPreviousFrame { .. } => {}
            CpuPaintOutcome::FrameOneUnavailable { message, .. } => {
                self.fail(app, message);
            }
            CpuPaintOutcome::SeamFailed { error, .. } => {
                eprintln!("rune: overlay {} CPU layered present failed: {error}", self.name);
            }
        }
        let submitted = crate::time::now();
        crate::haptics::flush_live(app.rt_mut(), true);
        if presented {
            self.meter.presented();
            self.note_presented(submitted);
        }
        self.dirty = false;
        self.idle = super::frame::idle_after(app.rt(), animating, None);
        self.runtime_idle = self.idle;
        self.pace_cpu_after_present(submitted);
        self.open_popup(app);
        false
    }
}

#[cfg(test)]
#[path = "cpu_tests.rs"]
mod tests;
