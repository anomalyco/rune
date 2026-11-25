//! Surface-local logical region geometry for Wayland behind-window blur.
//!
//! Both `ext_background_effect_v1` and `org_kde_kwin_blur` accept a `wl_region` in
//! surface-local logical coordinates (`physical_size / window_scale`, unaffected by `Cx::zoom`).
//! Square surfaces emit one rectangle `[0, 0, w, h]`; Wayland client-side decorated windows with
//! rounded bottom corners (`WindowExtWayland::content_corner_radius > 0`) decompose the bottom
//! band into symmetric 1-point horizontal span rectangles strictly inside the corner arcs so
//! compositor blur never bleeds outside `sctk-adwaita`'s rounded frame.

/// One `wl_region.add(x, y, width, height)` rectangle in surface-local logical coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RegionRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Cached logical geometry key for a surface's blur region.
///
/// Steady frames and monitor scale changes that preserve `logical_size` and
/// `bottom_corner_radius` produce an equal `RegionSpec` and emit zero Wayland requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RegionSpec {
    pub logical_size: (i32, i32),
    pub bottom_corner_radius: i32,
}

impl RegionSpec {
    /// Builds a normalized `RegionSpec` from physical drawable dimensions, the window/output
    /// backing scale factor (independent of `Cx::zoom`), and any CSD bottom corner radius in
    /// logical window points.
    pub(crate) fn from_surface(
        physical_size: (u32, u32),
        window_scale: f32,
        bottom_corner_radius_points: f32,
    ) -> Self {
        let logical_size = logical_surface_size(physical_size, window_scale);
        let bottom_corner_radius =
            normalize_bottom_corner_radius(bottom_corner_radius_points, logical_size);
        Self { logical_size, bottom_corner_radius }
    }

    /// True when either logical dimension is zero or negative (degenerate/unconfigured surface).
    #[inline]
    pub(crate) const fn is_empty(self) -> bool {
        self.logical_size.0 <= 0 || self.logical_size.1 <= 0
    }

    /// True when no bottom-corner rounding is active and a full-surface square region applies.
    #[inline]
    pub(crate) const fn is_square(self) -> bool {
        self.bottom_corner_radius <= 0
    }
}

/// Converts physical pixel size and display backing scale into surface-local logical coordinates.
///
/// Independent of `Cx::zoom`: UI zoom scales Rune's layout inside the window without changing
/// the Wayland surface's logical extent. Returns `(0, 0)` for zero or non-finite inputs.
pub(crate) fn logical_surface_size(physical_size: (u32, u32), window_scale: f32) -> (i32, i32) {
    if physical_size.0 == 0
        || physical_size.1 == 0
        || !window_scale.is_finite()
        || window_scale <= 0.0
    {
        return (0, 0);
    }
    let scale = f64::from(window_scale);
    let w = (f64::from(physical_size.0) / scale).round().clamp(1.0, i32::MAX as f64) as i32;
    let h = (f64::from(physical_size.1) / scale).round().clamp(1.0, i32::MAX as f64) as i32;
    (w, h)
}

/// Normalizes a Wayland CSD bottom corner radius (in logical points) against `logical_size`.
///
/// Rounds fractional radii up so horizontal spans stay conservatively inside the frame border,
/// and clamps to `min(w / 2, h)`.
pub(crate) fn normalize_bottom_corner_radius(
    radius_points: f32,
    logical_size: (i32, i32),
) -> i32 {
    if !radius_points.is_finite()
        || radius_points <= 0.0
        || logical_size.0 <= 0
        || logical_size.1 <= 0
    {
        return 0;
    }
    let max_radius = (logical_size.0 / 2).min(logical_size.1);
    if max_radius <= 0 {
        return 0;
    }
    let rounded = radius_points.ceil().clamp(0.0, max_radius as f32) as i32;
    rounded.min(max_radius)
}

/// Builds the list of surface-local logical rectangles covering `spec`.
///
/// - Empty `spec` -> empty vec.
/// - Square `spec` (`bottom_corner_radius == 0`) -> single rect `[0, 0, w, h]`.
/// - Rounded bottom corners -> one full-width top rect `[0, 0, w, h - r]` (when `h > r`) plus
///   symmetric 1-point horizontal span rects for each bottom row, strictly inside both bottom
///   quarter-circles and strictly clipped inside `[0, 0, w, h]`.
pub(crate) fn build_blur_rects(spec: RegionSpec) -> Vec<RegionRect> {
    let (w, h) = spec.logical_size;
    if w <= 0 || h <= 0 {
        return Vec::new();
    }
    let r = spec.bottom_corner_radius.clamp(0, (w / 2).min(h));
    if r == 0 {
        return vec![RegionRect { x: 0, y: 0, width: w, height: h }];
    }

    let mut rects = Vec::with_capacity(r as usize + 1);
    if h > r {
        rects.push(RegionRect { x: 0, y: 0, width: w, height: h - r });
    }

    let radius_f = f64::from(r);
    let radius_sq = radius_f * radius_f;
    let y_start = h - r;
    for row in 0..r {
        // Measure at the bottom edge of this 1-pt horizontal strip so the entire strip
        // `[y, y + 1]` stays strictly inside the quarter-circle of radius `r`.
        let dy = f64::from(row + 1);
        let dx = (radius_sq - dy * dy).max(0.0).sqrt();
        let inset = ((radius_f - dx).ceil() as i32).clamp(0, w / 2);
        let span_w = w - inset * 2;
        if span_w > 0 {
            rects.push(RegionRect { x: inset, y: y_start + row, width: span_w, height: 1 });
        }
    }

    // Never emit an empty region for a non-empty surface (under `ext_background_effect_v1`,
    // an empty region disables blur).
    if rects.is_empty() {
        rects.push(RegionRect { x: 0, y: 0, width: w, height: h });
    }
    rects
}
