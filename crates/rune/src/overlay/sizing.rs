//! Content-driven overlay sizing helpers: clamping/rounding, `OverlayPlacement::Rect` fitting,
//! an explicit Wayland layer-shell `PendingConfigure` state machine, and `OverlayCore`
//! pre-show measurement and same-frame resize helpers.

use crate::machine::Effects;
use crate::OverlayPlacement;

/// Default GPU physical texture dimension fallback before a GPU adapter is created.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) const DEFAULT_MAX_PHYS_DIM: u32 = 8192;

/// Rounds a logical-point dimension up to a whole number of physical pixels at `scale` so text
/// and borders never clip on fractional DPI scales (`1.0`, `1.25`, `1.5`, `2.0`, `3.0`).
#[inline]
pub(crate) fn ceil_logical_to_pixels(points: f32, scale: f32) -> f32 {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let p = if points.is_finite() && points > 0.0 { points } else { 0.0 };
    (p * s).ceil() / s
}

/// Clamps a fitted height in logical window points to `[1.0, min(work_area_h, max_phys_dim / scale)]`
/// after rounding up to whole physical pixels at `scale`. Zero-height content clamps to `1.0` pt.
pub(crate) fn clamp_fitted_height(
    height: f32,
    work_area: Option<(f32, f32)>,
    scale: f32,
    max_phys_dim: u32,
) -> f32 {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let phys_max = ((max_phys_dim.max(1) as f32) / s).max(1.0);
    let max_h = match work_area.map(|a| a.1) {
        Some(wa) if wa.is_finite() && wa > 0.0 => phys_max.min(wa).max(1.0),
        _ => phys_max,
    };
    ceil_logical_to_pixels(height, s).clamp(1.0, max_h)
}

/// Whether `next` differs materially from `current` at backing `scale`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn size_changed(current: (f32, f32), next: (f32, f32), scale: f32) -> bool {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let cur_px = ((current.0 * s).round() as i64, (current.1 * s).round() as i64);
    let nxt_px = ((next.0 * s).round() as i64, (next.1 * s).round() as i64);
    cur_px != nxt_px || (current.0 - next.0).abs() > 1e-3 || (current.1 - next.1).abs() > 1e-3
}

/// Like `overlay::options::local_rect`, preserving `(x, y)` and fixed width on
/// `OverlayPlacement::Rect` while taking `at.size.1` when `fit_height` is true.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn local_rect_for_fit(
    at: crate::overlay::Placement,
    placement: Option<OverlayPlacement>,
    area: (f32, f32),
    fit_height: bool,
) -> [f32; 4] {
    if let Some(OverlayPlacement::Rect { x, y, width, height }) = placement {
        let h = if fit_height { at.size.1.max(1.0) } else { height.max(1.0) };
        return [x, y, width.max(1.0), h];
    }
    crate::overlay::options::local_rect(at, placement, area)
}

/// Explicit state machine for Wayland `zwlr_layer_surface_v1` asynchronous content-driven resize
/// configures: after sending `set_size` + `commit`, holds the previous frame until the compositor
/// answers with `configure` (including when the compositor clamps or keeps the old size).
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WaylandResize {
    #[default]
    Ready,
    PendingConfigure { target: (u32, u32) },
}

#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaylandResizeEvent {
    RequestResize((u32, u32)),
    Configured((u32, u32)),
    Reset,
}

#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaylandResizeEffect {
    CommitAndHold((u32, u32)),
    ApplyConfigured((u32, u32)),
}

#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
impl WaylandResize {
    pub(crate) fn step(&mut self, event: WaylandResizeEvent) -> Effects<WaylandResizeEffect> {
        match (*self, event) {
            (_, WaylandResizeEvent::Reset) => {
                *self = Self::Ready;
                Effects::none()
            }
            (Self::Ready, WaylandResizeEvent::RequestResize(target)) => {
                let target = (target.0.max(1), target.1.max(1));
                *self = Self::PendingConfigure { target };
                Effects::one(WaylandResizeEffect::CommitAndHold(target))
            }
            (Self::PendingConfigure { target }, WaylandResizeEvent::RequestResize(next)) => {
                let next = (next.0.max(1), next.1.max(1));
                if next == target {
                    Effects::none()
                } else {
                    *self = Self::PendingConfigure { target: next };
                    Effects::one(WaylandResizeEffect::CommitAndHold(next))
                }
            }
            (_, WaylandResizeEvent::Configured(size)) => {
                *self = Self::Ready;
                Effects::one(WaylandResizeEffect::ApplyConfigured(size))
            }
        }
    }

    #[inline]
    pub(crate) const fn is_pending(self) -> bool {
        matches!(self, Self::PendingConfigure { .. })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl super::OverlayCore {
    /// Resolves `(work_area_points, backing_scale)` for the overlay's target display.
    pub(super) fn target_metrics(
        &self,
        event_loop: &winit::event_loop::ActiveEventLoop,
    ) -> (Option<(f32, f32)>, f32) {
        #[cfg(target_os = "macos")]
        {
            let scale = self
                .live_window()
                .map(|w| crate::window::backing_scale(w))
                .or_else(|| {
                    super::placement::monitor(event_loop, &self.options.display)
                        .map(|m| m.scale_factor() as f32)
                })
                .unwrap_or(1.0);
            let area = self
                .panel
                .as_ref()
                .and_then(|p| p.screen_area(self.at, &self.options))
                .or_else(|| crate::platform::macos::overlay_screen_area(&self.options))
                .or_else(|| {
                    super::placement::monitor(event_loop, &self.options.display).map(|m| {
                        let l = m.size().to_logical::<f32>(m.scale_factor());
                        (l.width, l.height)
                    })
                });
            return (area, scale.max(0.5));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let monitor = self
                .live_window()
                .and_then(|w| w.current_monitor())
                .or_else(|| super::placement::monitor(event_loop, &self.options.display));
            let scale = self
                .live_window()
                .map(|w| crate::window::backing_scale(w))
                .or_else(|| monitor.as_ref().map(|m| m.scale_factor() as f32))
                .unwrap_or(1.0);
            let area = monitor.map(|m| {
                let s = m.scale_factor() as f32;
                #[cfg(target_os = "windows")]
                let px = if matches!(
                    self.options.placement,
                    Some(OverlayPlacement::FullScreen | OverlayPlacement::Rect { .. })
                ) {
                    (m.size().width, m.size().height)
                } else {
                    crate::platform::win32::work_area(&m)
                        .map_or((m.size().width, m.size().height), |(_, size)| size)
                };
                #[cfg(not(target_os = "windows"))]
                let px = (m.size().width, m.size().height);
                (px.0 as f32 / s, px.1 as f32 / s)
            });
            (area, scale.max(0.5))
        }
    }

    pub(super) fn max_phys_dim(&self) -> u32 {
        #[cfg(target_os = "windows")]
        if self.cpu.is_some() || (self.requested_renderer.is_cpu() && !self.cpu_layered_failed) {
            return crate::platform::win32::layered::MAX_LAYERED_DIM;
        }
        self.gpu
            .as_ref()
            .map_or(DEFAULT_MAX_PHYS_DIM, |g| g.device.limits().max_texture_dimension_2d)
    }

    /// Whether height fitting is active (suppressed under `OverlayPlacement::FullScreen`).
    pub(super) fn fits_height_active(&self, rt: &crate::runtime::RuntimeCore) -> bool {
        rt.host.fit_height && !matches!(self.options.placement, Some(OverlayPlacement::FullScreen))
    }

    /// Non-committing pre-show / prewarm measurement at the target monitor's scale, work area,
    /// and planned `RendererKind`. Updates `self.canvas_size.1` and `self.at.size.1`.
    pub(super) fn fit_for_show_in(
        &mut self,
        app: &mut dyn super::OverlayApp,
        event_loop: Option<&winit::event_loop::ActiveEventLoop>,
    ) {
        if !self.fits_height_active(app.rt()) { return; }
        let (work_area, scale) = match event_loop {
            Some(el) => {
                let (area, scale) = self.target_metrics(el);
                if area.is_some() { self.last_work_area = area; }
                (self.last_work_area, scale)
            }
            None => (self.last_work_area, self.live_scale().unwrap_or_else(|| app.rt().window_scale())),
        };
        #[cfg(target_os = "windows")]
        let cpu_failed = self.cpu_layered_failed;
        #[cfg(not(target_os = "windows"))]
        let cpu_failed = false;
        let planned_kind = super::cpu::select_overlay_backend(
            super::cpu::OverlayTarget::current(),
            self.requested_renderer,
            app.has_compositor(),
            cpu_failed,
        )
        .renderer_kind();
        let width = match self.options.placement {
            Some(OverlayPlacement::Rect { width, .. }) => width.max(1.0),
            _ => self.canvas_size.0.max(1.0),
        };
        self.canvas_size.0 = width;
        let size = (width, self.canvas_size.1.max(1.0));
        {
            let rt = app.rt_mut();
            rt.set_renderer_kind(planned_kind);
            if rt.window_viewport() != size || (rt.window_scale() - scale).abs() > f32::EPSILON {
                rt.set_viewport(size, scale);
            }
        }
        if app.rt().stats.commits == 0 || app.rt().stale() {
            let raw = app.content_height();
            let clamped = clamp_fitted_height(raw, work_area, scale, self.max_phys_dim());
            self.canvas_size.1 = clamped;
            self.at.size = (width, clamped);
            app.rt_mut().set_viewport((width, clamped), scale);
        } else {
            let clamped = clamp_fitted_height(self.canvas_size.1, work_area, scale, self.max_phys_dim());
            self.canvas_size.1 = clamped;
            self.at.size = (width, clamped);
        }
    }

    /// Checks right after a runtime step whether a live `fit_content_height()` overlay needs a
    /// same-frame resize before painting and presenting.
    pub(super) fn take_live_fit_resize(&mut self, rt: &mut crate::runtime::RuntimeCore) -> Option<(f32, f32)> {
        if !self.fits_height_active(rt) {
            rt.host.overlay_fit.take();
            return None;
        }
        let raw = rt.host.overlay_fit.take()?;
        let scale = self.live_scale().unwrap_or_else(|| rt.window_scale());
        let width = match self.options.placement {
            Some(OverlayPlacement::Rect { width, .. }) => width.max(1.0),
            _ => self.at.size.0.max(1.0),
        };
        let height = clamp_fitted_height(raw, self.last_work_area, scale, self.max_phys_dim());
        let next = (width, height);
        self.canvas_size.1 = height;
        size_changed(self.at.size, next, scale).then_some(next)
    }

    /// Same-frame content resize on a live GPU overlay (after the old drawable is dropped and
    /// before acquiring the replacement drawable).
    pub(super) fn apply_content_resize_gpu(
        &mut self,
        app: &mut dyn super::OverlayApp,
        event_loop: Option<&winit::event_loop::ActiveEventLoop>,
        next: (f32, f32),
    ) -> bool {
        self.at.size = next;
        self.canvas_size.1 = next.1;
        let Some(live) = self.surface.live_mut() else { return false };
        let scale = live.scale;
        match &mut live.backing {
            super::Backing::Window(window) => {
                let window = window.clone();
                #[cfg(target_os = "macos")]
                if let Some(panel) = &self.panel {
                    if self.position_owner.is_managed() {
                        panel.place_fit(self.at, &self.options, true);
                    } else {
                        panel.resize(self.at.size);
                    }
                    self.sync_origin(app.rt_mut(), &window);
                } else {
                    let _size = window.request_inner_size(winit::dpi::LogicalSize::new(next.0 as f64, next.1 as f64));
                    if let Some(el) = event_loop && self.position_owner.is_managed() {
                        self.place_window(app.rt_mut(), el, &window);
                    } else {
                        self.sync_origin(app.rt_mut(), &window);
                    }
                }
                #[cfg(target_os = "windows")]
                {
                    let phys = crate::overlay::surface::overlay_rescale_size(next, scale, self.max_phys_dim());
                    if let Some(el) = event_loop && self.position_owner.is_managed() {
                        if let Some(pos) = super::placement::position(el, self.at, &self.options) {
                            crate::platform::win32::set_window_rect(&window, (pos.x, pos.y), phys);
                            self.origin = Some(pos);
                            self.sync_origin(app.rt_mut(), &window);
                        } else {
                            let _size = window.request_inner_size(winit::dpi::LogicalSize::new(next.0 as f64, next.1 as f64));
                            self.sync_origin(app.rt_mut(), &window);
                        }
                    } else {
                        let _size = window.request_inner_size(winit::dpi::LogicalSize::new(next.0 as f64, next.1 as f64));
                        self.sync_origin(app.rt_mut(), &window);
                    }
                    if let Some(native) = &self.native { native.update_geometry(); }
                }
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                {
                    let _size = window.request_inner_size(winit::dpi::LogicalSize::new(next.0 as f64, next.1 as f64));
                    if let Some(el) = event_loop && self.position_owner.is_managed() {
                        self.place_window(app.rt_mut(), el, &window);
                    } else {
                        self.sync_origin(app.rt_mut(), &window);
                        if let Some(x11) = &self.x11 { x11.update_geometry(); }
                    }
                }
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            super::Backing::Layer(_) => return false,
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            super::Backing::Xwayland(surface) => surface.place(self.at, &self.options),
        }
        if let Some(live) = self.surface.live_mut() {
            live.scale = 0.0;
        }
        if let Some(gpu) = self.gpu.clone() {
            self.rescale(app, &gpu, scale);
        }
        self.surface.live().is_some()
    }

    /// Wayland layer-shell: sends `layer.place(at)` + `layer.commit()` and enters
    /// `PendingConfigure` so the previous frame stays on screen until the compositor confirms.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn wayland_request_content_resize(&mut self, next: (f32, f32)) -> bool {
        if !matches!(self.surface.live().map(|l| &l.backing), Some(super::Backing::Layer(_))) {
            return false;
        }
        let target = (next.0.ceil().max(1.0) as u32, next.1.ceil().max(1.0) as u32);
        self.at.size = next;
        self.canvas_size.1 = next.1;
        let effects = self.wayland_resize.step(WaylandResizeEvent::RequestResize(target));
        for effect in effects {
            if let WaylandResizeEffect::CommitAndHold(_) = effect
                && let Some(super::Live { backing: super::Backing::Layer(layer), .. }) = self.surface.live_mut()
            {
                layer.place(self.at);
                layer.commit();
            }
        }
        true
    }

    /// Same-frame content resize on a live Windows CPU layered overlay right after `frame::step`
    /// and before `paint_and_present`, so `UpdateLayeredWindowIndirect` atomically updates
    /// origin, size, and pixels in one Win32 call.
    #[cfg(target_os = "windows")]
    pub(super) fn apply_content_resize_cpu(
        &mut self,
        app: &mut dyn super::OverlayApp,
        event_loop: Option<&winit::event_loop::ActiveEventLoop>,
        next: (f32, f32),
    ) {
        self.at.size = next;
        self.canvas_size.1 = next.1;
        let Some(window) = self.cpu.as_ref().map(|c| c.window.clone()) else { return };
        if let Some(el) = event_loop && self.position_owner.is_managed() {
            if let Some(pos) = super::placement::position(el, self.at, &self.options) {
                self.origin = Some(pos);
                let scale = self.cpu.as_ref().map_or(1.0, |c| c.scale);
                app.rt_mut().set_window_pixels(Some((pos.x, pos.y)));
                self.origin_moved(app.rt_mut(), (pos.x as f32 / scale, pos.y as f32 / scale));
                self.publish_screen_origin((pos.x, pos.y), scale);
            }
        }
        let scale = self.cpu.as_ref().map_or_else(|| crate::window::backing_scale(&window), |c| c.scale);
        if let Some(cpu) = self.cpu.as_mut() {
            cpu.size = crate::platform::win32::layered::bounded_layered_size(next, scale);
            cpu.engine.invalidate_full();
        }
        app.rt_mut().set_viewport(next, scale);
        if let Some(native) = &self.native { native.update_geometry(); }
    }
}
