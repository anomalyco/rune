//! Declarative content-driven sizing for overlays (`ContentSizing`, `ConfiguredOverlay::sizing`,
//! `min_size`, `max_size`, and `ConfiguredOverlay::fit_content_height`), pure clamping/rounding
//! helpers, an explicit Wayland layer-shell `PendingConfigure` state machine, and `OverlayCore`
//! pre-show measurement and same-frame resize integration.

#[cfg(not(target_arch = "wasm32"))]
use crate::machine::Effects;
#[cfg(not(target_arch = "wasm32"))]
use crate::OverlayPlacement;

/// How an overlay derives its surface size from its layout content.
///
/// Pass to [`ConfiguredOverlay::sizing`](crate::ConfiguredOverlay::sizing) (default
/// [`ContentSizing::Fixed`]), optionally bounded by
/// [`ConfiguredOverlay::min_size`](crate::ConfiguredOverlay::min_size) and
/// [`ConfiguredOverlay::max_size`](crate::ConfiguredOverlay::max_size).
/// [`ConfiguredOverlay::fit_content_height`](crate::ConfiguredOverlay::fit_content_height) is
/// shorthand for `.sizing(ContentSizing::FitHeight)`.
///
/// ```no_run
/// use rune::{ContentSizing, OverlayOptions, OverlaySpec, col, text};
///
/// let _candidates = OverlaySpec::new("candidates", (0.0, 0.0), |items: &Vec<String>, _| {
///     col().gap(2.0).padding(8.0).children(items.iter().map(|s| text(s)))
/// })
/// .options(OverlayOptions::default())
/// .sizing(ContentSizing::Fit)
/// .max_size((420.0, 320.0));
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ContentSizing {
    /// Explicit fixed dimensions from [`OverlaySpec::size`](crate::OverlaySpec::size) or
    /// [`OverlayHandle::resize`](crate::OverlayHandle::resize) (the default).
    #[default]
    Fixed,
    /// Fit both width and height to the root element's natural layout size, clamped to
    /// `min_size`, `max_size`, and the target display's sizing bounds (on Windows,
    /// [`Display::work_area`](crate::Display::work_area) for edge placements and full display
    /// bounds for [`OverlayPlacement::Rect`](crate::OverlayPlacement::Rect); full display bounds
    /// on other platforms).
    Fit,
    /// Keep width fixed and fit height to the root element's natural layout height, clamped to
    /// `min_size`, `max_size`, and the target display's sizing bounds (see [`Self::Fit`]).
    FitHeight,
    /// Fit width to the root element's natural layout width and keep height fixed, clamped to
    /// `min_size`, `max_size`, and the target display's sizing bounds (see [`Self::Fit`]).
    FitWidth,
}

impl ContentSizing {
    #[inline]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) const fn is_fixed(self) -> bool {
        matches!(self, Self::Fixed)
    }

    #[inline]
    pub(crate) const fn fits_width(self) -> bool {
        matches!(self, Self::Fit | Self::FitWidth)
    }

    #[inline]
    pub(crate) const fn fits_height(self) -> bool {
        matches!(self, Self::Fit | Self::FitHeight)
    }

    #[inline]
    pub(crate) const fn fits_any(self) -> bool {
        !matches!(self, Self::Fixed)
    }
}

/// Optional `(min_size, max_size)` logical-point bounds for a content-sized root.
pub(crate) type SizeLimits = (Option<(f32, f32)>, Option<(f32, f32)>);

/// Configured content-sizing mode and optional logical-point bounds for an overlay.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SizingConfig {
    pub mode: ContentSizing,
    pub min_size: Option<(f32, f32)>,
    pub max_size: Option<(f32, f32)>,
}

/// Default GPU physical texture dimension fallback before a GPU adapter is created.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const DEFAULT_MAX_PHYS_DIM: u32 = 8192;

/// Rounds a logical-point dimension up to a whole number of physical pixels at `scale` so text
/// and borders never clip on fractional DPI scales (`1.0`, `1.25`, `1.5`, `2.0`, `3.0`).
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) fn ceil_logical_to_pixels(points: f32, scale: f32) -> f32 {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let p = if points.is_finite() && points > 0.0 { points } else { 0.0 };
    (p * s).ceil() / s
}

/// Computes effective logical-point `(min, max)` bounds from `config`, target `work_area`,
/// backing `scale`, and physical surface limit `max_phys_dim`. When `min > max`, `min` clamps to
/// `max`. Always guarantees `1.0 <= min <= max`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn effective_bounds(
    config: SizingConfig,
    work_area: Option<(f32, f32)>,
    scale: f32,
    max_phys_dim: u32,
) -> ((f32, f32), (f32, f32)) {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let phys_max = ((max_phys_dim.max(1) as f32) / s).max(1.0);
    let cap = |limit: f32, opt: Option<f32>| match opt {
        Some(v) if v.is_finite() && v > 0.0 => limit.min(v),
        _ => limit,
    };
    let mut max_w = cap(phys_max, work_area.map(|a| a.0));
    let mut max_h = cap(phys_max, work_area.map(|a| a.1));
    max_w = cap(max_w, config.max_size.map(|m| m.0)).max(1.0);
    max_h = cap(max_h, config.max_size.map(|m| m.1)).max(1.0);

    let clean_min = |opt: Option<f32>, hi: f32| match opt {
        Some(v) if v.is_finite() && v > 0.0 => v.max(1.0).min(hi),
        _ => 1.0_f32.min(hi),
    };
    let min_w = clean_min(config.min_size.map(|m| m.0), max_w);
    let min_h = clean_min(config.min_size.map(|m| m.1), max_h);
    ((min_w, min_h), (max_w, max_h))
}

/// Root `(min_size, max_size)` layout constraints passed to Taffy so wrapping text wraps at
/// effective `max_w` instead of laying out on one unbounded line.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn layout_limits(
    config: SizingConfig,
    work_area: Option<(f32, f32)>,
    scale: f32,
    max_phys_dim: u32,
) -> SizeLimits {
    if config.mode.is_fixed() {
        return (None, None);
    }
    let (min, max) = effective_bounds(config, work_area, scale, max_phys_dim);
    (Some(min), Some(max))
}

/// Resolves the final logical overlay size `(width, height)` for a `ContentSizing` overlay.
///
/// - `OverlayPlacement::FullScreen` always overrides content sizing.
/// - `ContentSizing::Fixed` preserves `fixed_size` (clamped `>= 1.0`).
/// - Fitted axes round up to whole physical pixels (`ceil_logical_to_pixels`) and clamp to
///   `[min_size.max(1.0), min(max_size, work_area, max_phys_dim / scale)]`. Zero-size content
///   clamps to the 1 pt floor so transparent overlays stay live and clear.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_content_size(
    config: SizingConfig,
    placement: Option<OverlayPlacement>,
    fixed_size: (f32, f32),
    measured: (f32, f32),
    work_area: Option<(f32, f32)>,
    scale: f32,
    max_phys_dim: u32,
) -> (f32, f32) {
    if matches!(placement, Some(OverlayPlacement::FullScreen)) {
        let full = work_area.unwrap_or(fixed_size);
        return (full.0.max(1.0), full.1.max(1.0));
    }
    let base = match placement {
        Some(OverlayPlacement::Rect { width, height, .. }) => (width.max(1.0), height.max(1.0)),
        _ => (fixed_size.0.max(1.0), fixed_size.1.max(1.0)),
    };
    if config.mode.is_fixed() {
        return base;
    }
    let (min, max) = effective_bounds(config, work_area, scale, max_phys_dim);
    let fit_axis = |v: f32, lo: f32, hi: f32| ceil_logical_to_pixels(v, scale).clamp(lo, hi);
    let w = if config.mode.fits_width() { fit_axis(measured.0, min.0, max.0) } else { base.0 };
    let h = if config.mode.fits_height() { fit_axis(measured.1, min.1, max.1) } else { base.1 };
    (w.max(1.0), h.max(1.0))
}

/// Whether `next` differs materially from `current` at backing `scale` (avoids floating-point
/// churn at repeating fractional scales such as `1.5`).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn size_changed(current: (f32, f32), next: (f32, f32), scale: f32) -> bool {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let cur_px = ((current.0 * s).round() as i64, (current.1 * s).round() as i64);
    let nxt_px = ((next.0 * s).round() as i64, (next.1 * s).round() as i64);
    cur_px != nxt_px || (current.0 - next.0).abs() > 1e-3 || (current.1 - next.1).abs() > 1e-3
}

/// Like `overlay::options::local_rect`, preserving `(x, y)` on `OverlayPlacement::Rect` while
/// substituting `at.size` on axes fitted by `sizing`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn local_rect_for(
    at: crate::overlay::Placement,
    placement: Option<OverlayPlacement>,
    area: (f32, f32),
    sizing: ContentSizing,
) -> [f32; 4] {
    if let Some(OverlayPlacement::Rect { x, y, width, height }) = placement {
        let w = if sizing.fits_width() { at.size.0.max(1.0) } else { width.max(1.0) };
        let h = if sizing.fits_height() { at.size.1.max(1.0) } else { height.max(1.0) };
        return [x, y, w, h];
    }
    crate::overlay::options::local_rect(at, placement, area)
}

/// Explicit state machine for Wayland `zwlr_layer_surface_v1` asynchronous content-driven resize
/// configures: after sending `set_size` + `commit`, holds the previous frame until the compositor
/// answers with `configure` (including when the compositor clamps or keeps the old size).
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WaylandResize {
    #[default]
    Ready,
    PendingConfigure { target: (u32, u32) },
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaylandResizeEvent {
    RequestResize((u32, u32)),
    Configured((u32, u32)),
    Reset,
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaylandResizeEffect {
    CommitAndHold((u32, u32)),
    ApplyConfigured((u32, u32)),
}

#[cfg(not(target_arch = "wasm32"))]
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

/// Selects the physical pixel bounds `(width, height)` that cap fitted axes on a display.
///
/// Mirrors `overlay::placement::position_in`: edge placements (`None` and
/// `OverlayPlacement::Edge`) use `work_area_size` when available (Windows `rcWork`), while
/// `OverlayPlacement::Rect` and `OverlayPlacement::FullScreen` stay in full `display_size`
/// coordinates.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub(crate) fn target_area_px(
    placement: Option<OverlayPlacement>,
    display_size: (u32, u32),
    work_area_size: Option<(u32, u32)>,
) -> (u32, u32) {
    match placement {
        Some(OverlayPlacement::FullScreen | OverlayPlacement::Rect { .. }) => display_size,
        Some(OverlayPlacement::Edge { .. }) | None => work_area_size.unwrap_or(display_size),
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl super::OverlayCore {
    /// Resolves `(display_bounds_points, backing_scale)` for the overlay's target display.
    ///
    /// On Windows, edge placements (`None` and `OverlayPlacement::Edge`) use `rcWork`
    /// (`Display::work_area`), whereas `OverlayPlacement::Rect` and `OverlayPlacement::FullScreen`
    /// use full display bounds to match `overlay::placement::position_in`. Other platforms use
    /// full display bounds for all placements.
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
                let px = target_area_px(
                    self.options.placement,
                    (m.size().width, m.size().height),
                    crate::platform::win32::work_area(&m).map(|(_, size)| size),
                );
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

    /// Whether content sizing is active (suppressed under `OverlayPlacement::FullScreen`).
    pub(super) fn sizing_active(&self) -> bool {
        self.sizing.mode.fits_any()
            && !matches!(self.options.placement, Some(OverlayPlacement::FullScreen))
    }

    /// Non-committing pre-show / prewarm measurement at the target monitor's scale, work area,
    /// bounds, and planned `RendererKind`. Updates `self.canvas_size` and `self.at.size`.
    pub(super) fn fit_for_show_in(
        &mut self,
        app: &mut dyn super::OverlayApp,
        event_loop: Option<&winit::event_loop::ActiveEventLoop>,
    ) {
        if !self.sizing_active() {
            app.rt_mut().host.content_sizing = ContentSizing::Fixed;
            app.rt_mut().host.content_limits = (None, None);
            return;
        }
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
        let max_phys = self.max_phys_dim();
        let limits = layout_limits(self.sizing, work_area, scale, max_phys);
        let base = match self.options.placement {
            Some(OverlayPlacement::Rect { width, height, .. }) => (width.max(1.0), height.max(1.0)),
            _ => (self.canvas_size.0.max(1.0), self.canvas_size.1.max(1.0)),
        };
        {
            let rt = app.rt_mut();
            rt.set_renderer_kind(planned_kind);
            rt.host.content_sizing = self.sizing.mode;
            rt.host.content_limits = limits;
            if rt.window_viewport() != base || (rt.window_scale() - scale).abs() > f32::EPSILON {
                rt.set_viewport(base, scale);
            }
        }
        let fitted = if app.rt().stats.commits == 0 || app.rt().stale() {
            let raw = app.measure_content();
            resolve_content_size(self.sizing, self.options.placement, base, raw, work_area, scale, max_phys)
        } else {
            resolve_content_size(self.sizing, self.options.placement, base, self.canvas_size, work_area, scale, max_phys)
        };
        self.canvas_size = fitted;
        self.at.size = fitted;
        app.rt_mut().set_viewport(fitted, scale);
    }

    /// Checks right after a runtime step whether a live content-sized overlay needs a
    /// same-frame resize before painting and presenting.
    pub(super) fn take_live_fit_resize(&mut self, rt: &mut crate::runtime::RuntimeCore) -> Option<(f32, f32)> {
        if !self.sizing_active() {
            rt.host.overlay_fit.take();
            return None;
        }
        let raw = rt.host.overlay_fit.take()?;
        let scale = self.live_scale().unwrap_or_else(|| rt.window_scale());
        let next = resolve_content_size(
            self.sizing,
            self.options.placement,
            self.at.size,
            raw,
            self.last_work_area,
            scale,
            self.max_phys_dim(),
        );
        self.canvas_size = next;
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
        self.canvas_size = next;
        let Some(live) = self.surface.live_mut() else { return false };
        let scale = live.scale;
        match &mut live.backing {
            super::Backing::Window(window) => {
                let window = window.clone();
                #[cfg(target_os = "macos")]
                if let Some(panel) = &self.panel {
                    if self.position_owner.is_managed() {
                        panel.place_sized(self.at, &self.options, self.sizing.mode);
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
        self.canvas_size = next;
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
        self.canvas_size = next;
        let Some(window) = self.cpu.as_ref().map(|c| c.window.clone()) else { return };
        if let Some(el) = event_loop
            && self.position_owner.is_managed()
            && let Some(pos) = super::placement::position(el, self.at, &self.options)
        {
            self.origin = Some(pos);
            let scale = self.cpu.as_ref().map_or(1.0, |c| c.scale);
            app.rt_mut().set_window_pixels(Some((pos.x, pos.y)));
            self.origin_moved(app.rt_mut(), (pos.x as f32 / scale, pos.y as f32 / scale));
            self.publish_screen_origin((pos.x, pos.y), scale);
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

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "sizing_tests.rs"]
mod tests;
