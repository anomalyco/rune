//! Surface-local logical region geometry for Wayland behind-window blur.
//!
//! Both `ext_background_effect_v1` and `org_kde_kwin_blur` accept a `wl_region` in
//! surface-local logical coordinates (`physical_size / window_scale`, unaffected by `Cx::zoom`).
//! - Whole-surface blur (`WindowBackdrop::Blur`): square surfaces emit `[0, 0, w, h]`; Wayland
//!   client-side decorated windows with rounded bottom corners
//!   (`WindowExtWayland::content_corner_radius > 0`) decompose the bottom band into symmetric
//!   1-point horizontal span rectangles strictly inside the corner arcs, and the top band the
//!   same way under a frame without a headerbar (`WindowExtWayland::has_headerless_frame`).
//! - Per-element shaped blur (`El::system_material` while `WindowBackdrop::None` is active):
//!   quantizes each element conservatively inward into surface-local logical coordinates,
//!   decomposes all four rounded corners into symmetric 1-point horizontal span rects strictly
//!   inside the quarter-circle arcs, intersects every span with the element's ancestor clip and
//!   surface bounds, and bounds total emitted rectangles per surface to `MAX_BLUR_REGION_RECTS`.

use crate::system_material::{WAYLAND_BLUR_MIN_OPACITY, WaylandMaterialBlur};

/// Upper bound on rectangles emitted into a single `wl_region` per surface so per-frame wire
/// traffic stays compact even with many large-radius cards animating at 120 Hz.
pub(crate) const MAX_BLUR_REGION_RECTS: usize = 512;

/// Float-noise tolerance for conservative inward quantization of logical coordinates.
const INWARD_EPS: f64 = 1e-4;

/// One `wl_region.add(x, y, width, height)` rectangle in surface-local logical coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RegionRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Cached logical geometry key for a whole-surface blur region (`WindowBackdrop::Blur`).
///
/// Steady frames and monitor scale changes that preserve `logical_size` and
/// `bottom_corner_radius` produce an equal `RegionSpec` and emit zero Wayland requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RegionSpec {
    pub logical_size: (i32, i32),
    pub bottom_corner_radius: i32,
    /// The top two corners take `bottom_corner_radius` too: the client-side frame has no
    /// headerbar over them (`with_top_corners`).
    pub top_corners: bool,
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
        Self { logical_size, bottom_corner_radius, top_corners: false }
    }

    /// The same region with its top two corners rounded as its bottom two are, where `top` says
    /// the window's client-side frame has no headerbar. A square region, and one too short for
    /// arcs at both ends, keeps its top as it was.
    pub(crate) fn with_top_corners(mut self, top: bool) -> Self {
        self.top_corners = top && self.bottom_corner_radius > 0 && self.logical_size.1 >= 2 * self.bottom_corner_radius;
        self
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

/// Quantized surface-local logical geometry for one `El::system_material` blur element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ElementRegion {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub radius: i32,
    /// Effective clip rectangle `[x0, y0, x1, y1]` in surface-local logical coordinates,
    /// intersected with the surface bounds and clamped to this element's own bounding box.
    pub clip: [i32; 4],
}

impl ElementRegion {
    /// Converts an element's physical rect, corner radius, and ancestor clip into a
    /// conservatively inward quantized logical `ElementRegion`, or `None` when empty or
    /// clipped completely outside `surface_logical`.
    pub(crate) fn from_physical(
        rect_px: [f32; 4],
        radius_px: f32,
        clip_px: [f32; 4],
        window_scale: f32,
        surface_logical: (i32, i32),
    ) -> Option<Self> {
        let (sw, sh) = surface_logical;
        if sw <= 0
            || sh <= 0
            || !window_scale.is_finite()
            || window_scale <= 0.0
            || !rect_px.iter().all(|v| v.is_finite())
            || rect_px[2] <= 0.0
            || rect_px[3] <= 0.0
        {
            return None;
        }
        let scale = f64::from(window_scale);
        let lx0 = f64::from(rect_px[0]) / scale;
        let ly0 = f64::from(rect_px[1]) / scale;
        let lx1 = f64::from(rect_px[0] + rect_px[2]) / scale;
        let ly1 = f64::from(rect_px[1] + rect_px[3]) / scale;

        let x0 = (lx0 - INWARD_EPS).ceil().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
        let y0 = (ly0 - INWARD_EPS).ceil().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
        let x1 = (lx1 + INWARD_EPS).floor().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
        let y1 = (ly1 + INWARD_EPS).floor().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
        let width = x1.saturating_sub(x0);
        let height = y1.saturating_sub(y0);
        if width <= 0 || height <= 0 {
            return None;
        }

        let max_r = (width / 2).min(height / 2);
        let radius = if radius_px.is_finite() && radius_px > 0.0 {
            ((f64::from(radius_px) / scale - INWARD_EPS).ceil().max(0.0) as i32).min(max_r)
        } else {
            0
        };

        let (cx0_f, cy0_f, cx1_f, cy1_f) = if clip_px.iter().all(|v| v.is_finite()) {
            (
                (f64::from(clip_px[0]) / scale - INWARD_EPS).ceil(),
                (f64::from(clip_px[1]) / scale - INWARD_EPS).ceil(),
                (f64::from(clip_px[2]) / scale + INWARD_EPS).floor(),
                (f64::from(clip_px[3]) / scale + INWARD_EPS).floor(),
            )
        } else {
            (0.0, 0.0, f64::from(sw), f64::from(sh))
        };
        let clip_x0 = (cx0_f.clamp(0.0, f64::from(sw)) as i32).max(x0);
        let clip_y0 = (cy0_f.clamp(0.0, f64::from(sh)) as i32).max(y0);
        let clip_x1 = (cx1_f.clamp(0.0, f64::from(sw)) as i32).min(x1);
        let clip_y1 = (cy1_f.clamp(0.0, f64::from(sh)) as i32).min(y1);
        if clip_x1 <= clip_x0 || clip_y1 <= clip_y0 {
            return None;
        }

        Some(Self {
            x: x0,
            y: y0,
            width,
            height,
            radius,
            clip: [clip_x0, clip_y0, clip_x1, clip_y1],
        })
    }
}

/// Unified Wayland blur region specification cached per surface.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum BlurRegion {
    /// No blur region active.
    #[default]
    Empty,
    /// Whole-surface blur (`WindowBackdrop::Blur`, taking precedence over `El::system_material`).
    WholeSurface(RegionSpec),
    /// Union of per-element shaped blur regions (`El::system_material` while `WindowBackdrop::None`).
    Elements(Vec<ElementRegion>),
}

impl BlurRegion {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::WholeSurface(spec) => spec.is_empty(),
            Self::Elements(elems) => elems.is_empty(),
        }
    }

    /// True when `org_kde_kwin_blur::set_region(None)` (whole square surface) applies.
    #[inline]
    pub(crate) fn is_kde_full_surface(&self) -> bool {
        matches!(self, Self::WholeSurface(spec) if !spec.is_empty() && spec.is_square())
    }
}

/// Builds a `BlurRegion` from the frame's eligible `El::system_material` blurs.
pub(crate) fn build_elements_region(
    materials: &[WaylandMaterialBlur],
    window_scale: f32,
    surface_logical: (i32, i32),
) -> BlurRegion {
    if materials.is_empty() || surface_logical.0 <= 0 || surface_logical.1 <= 0 {
        return BlurRegion::Empty;
    }
    let mut elems = Vec::with_capacity(materials.len());
    for m in materials {
        if m.opacity < WAYLAND_BLUR_MIN_OPACITY {
            continue;
        }
        if let Some(elem) = ElementRegion::from_physical(
            m.rect,
            m.radius,
            m.clip,
            window_scale,
            surface_logical,
        ) {
            elems.push(elem);
        }
    }
    if elems.is_empty() {
        BlurRegion::Empty
    } else {
        BlurRegion::Elements(elems)
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

/// Builds the list of surface-local logical rectangles covering a whole-surface `RegionSpec`.
pub(crate) fn build_blur_rects(spec: RegionSpec) -> Vec<RegionRect> {
    let (w, h) = spec.logical_size;
    if w <= 0 || h <= 0 {
        return Vec::new();
    }
    let r = spec.bottom_corner_radius.clamp(0, (w / 2).min(h));
    if r == 0 {
        return vec![RegionRect { x: 0, y: 0, width: w, height: h }];
    }

    let radius_f = f64::from(r);
    let radius_sq = radius_f * radius_f;
    // The top band's rows, where a frame without a headerbar rounds the top corners: the bottom
    // band's mirror, each row measured at its upper edge, the one farther from the arcs' centres.
    let top = if spec.top_corners && h >= 2 * r { r } else { 0 };
    let mut rects = Vec::with_capacity((r + top) as usize + 1);
    for row in 0..top {
        let dy = f64::from(top - row);
        let dx = (radius_sq - dy * dy).max(0.0).sqrt();
        let inset = ((radius_f - dx).ceil() as i32).clamp(0, w / 2);
        let span_w = w - inset * 2;
        if span_w > 0 {
            rects.push(RegionRect { x: inset, y: row, width: span_w, height: 1 });
        }
    }
    if h > r + top {
        rects.push(RegionRect { x: 0, y: top, width: w, height: h - r - top });
    }

    let y_start = h - r;
    for row in 0..r {
        let dy = f64::from(row + 1);
        let dx = (radius_sq - dy * dy).max(0.0).sqrt();
        let inset = ((radius_f - dx).ceil() as i32).clamp(0, w / 2);
        let span_w = w - inset * 2;
        if span_w > 0 {
            rects.push(RegionRect { x: inset, y: y_start + row, width: span_w, height: 1 });
        }
    }

    if rects.is_empty() {
        rects.push(RegionRect { x: 0, y: 0, width: w, height: h });
    }
    rects
}

fn push_clipped(out: &mut Vec<RegionRect>, x: i32, y: i32, w: i32, h: i32, clip: [i32; 4]) {
    if out.len() >= MAX_BLUR_REGION_RECTS || w <= 0 || h <= 0 {
        return;
    }
    let ix0 = x.max(clip[0]);
    let iy0 = y.max(clip[1]);
    let ix1 = x.saturating_add(w).min(clip[2]);
    let iy1 = y.saturating_add(h).min(clip[3]);
    if ix1 > ix0 && iy1 > iy0 {
        out.push(RegionRect { x: ix0, y: iy0, width: ix1 - ix0, height: iy1 - iy0 });
    }
}

fn emit_element_rects(elem: ElementRegion, step: i32, out: &mut Vec<RegionRect>) {
    let step = step.max(1);
    let (x0, y0, w, h) = (elem.x, elem.y, elem.width, elem.height);
    if w <= 0 || h <= 0 {
        return;
    }
    let r = elem.radius.clamp(0, (w / 2).min(h / 2));
    if r == 0 {
        push_clipped(out, x0, y0, w, h, elem.clip);
        return;
    }

    let radius_f = f64::from(r);
    let radius_sq = radius_f * radius_f;

    // Top rounded band: measure at the top edge of each horizontal strip (`dy = r - row`) so
    // the entire strip stays strictly inside the top-left and top-right quarter-circles.
    let mut row = 0;
    while row < r {
        let strip_h = (r - row).min(step);
        let dy = f64::from(r - row);
        let dx = (radius_sq - dy * dy).max(0.0).sqrt();
        let inset = ((radius_f - dx).ceil() as i32).clamp(0, w / 2);
        let span_w = w - inset * 2;
        if span_w > 0 {
            push_clipped(out, x0 + inset, y0 + row, span_w, strip_h, elem.clip);
        }
        row += strip_h;
    }

    // Full-width middle band between top and bottom corner arcs.
    if h > r * 2 {
        push_clipped(out, x0, y0 + r, w, h - r * 2, elem.clip);
    }

    // Bottom rounded band: measure at the bottom edge of each strip (`dy = row + strip_h`).
    let bottom_y = y0 + h - r;
    let mut row = 0;
    while row < r {
        let strip_h = (r - row).min(step);
        let dy = f64::from(row + strip_h);
        let dx = (radius_sq - dy * dy).max(0.0).sqrt();
        let inset = ((radius_f - dx).ceil() as i32).clamp(0, w / 2);
        let span_w = w - inset * 2;
        if span_w > 0 {
            push_clipped(out, x0 + inset, bottom_y + row, span_w, strip_h, elem.clip);
        }
        row += strip_h;
    }
}

/// Builds the `wl_region` rectangle union for `region`, capped at `MAX_BLUR_REGION_RECTS`.
pub(crate) fn build_blur_region_rects(region: &BlurRegion) -> Vec<RegionRect> {
    match region {
        BlurRegion::Empty => Vec::new(),
        BlurRegion::WholeSurface(spec) => {
            let mut rects = build_blur_rects(*spec);
            rects.truncate(MAX_BLUR_REGION_RECTS);
            rects
        }
        BlurRegion::Elements(elems) => {
            if elems.is_empty() {
                return Vec::new();
            }
            let mut worst_total = 0usize;
            let mut rounded_count = 0usize;
            for elem in elems {
                let r = elem.radius.clamp(0, (elem.width / 2).min(elem.height / 2));
                if r > 0 {
                    worst_total = worst_total.saturating_add(2 * r as usize + 1);
                    rounded_count += 1;
                } else {
                    worst_total = worst_total.saturating_add(1);
                }
            }
            let over_budget = worst_total > MAX_BLUR_REGION_RECTS;
            let per_elem_budget = if over_budget && rounded_count > 0 {
                (MAX_BLUR_REGION_RECTS.saturating_sub(elems.len()) / rounded_count).max(2)
            } else {
                usize::MAX
            };
            let mut out = Vec::with_capacity(worst_total.min(MAX_BLUR_REGION_RECTS));
            for &elem in elems {
                if out.len() >= MAX_BLUR_REGION_RECTS {
                    break;
                }
                let r = elem.radius.clamp(0, (elem.width / 2).min(elem.height / 2));
                let step = if over_budget && r > 0 {
                    ((2 * r as usize).div_ceil(per_elem_budget)).clamp(1, r as usize) as i32
                } else {
                    1
                };
                emit_element_rects(elem, step, &mut out);
            }
            out
        }
    }
}
