//! SVG `stroke-dasharray`, `stroke-dashoffset`, and `pathLength` support for monochrome icons.
//!
//! Matches Chromium/Skia semantics: odd-length dash arrays repeat to even length, all-zero dash
//! arrays draw solid, `pathLength` scales both dash array and dash offset by
//! `actual_length / path_length`, and SMIL interpolation repeats mismatched even lists to their
//! least common multiple (within `MAX_DASH_ARRAY`), falling back to a discrete midpoint step when
//! one keyframe is `none` or the LCM exceeds the cap.

use swash::zeno::Cap;
use tiny_skia_path::{
    NormalizedF32Exclusive, Path, PathSegment, Point, StrokeDash, path_geometry,
};

/// Maximum entries in a normalized (even-length) `stroke-dasharray`.
pub(crate) const MAX_DASH_ARRAY: usize = 32;
/// Maximum finite magnitude for dash lengths, offsets, and `pathLength`.
pub(crate) const MAX_DASH_VAL: f32 = 65_536.0;
/// Cap on estimated dash segments per stroked path so microscopic intervals never stall frames.
pub(crate) const MAX_DASH_SEGMENTS: usize = 2048;

/// Normalized SVG `stroke-dasharray`: `len == 0` means `none`, otherwise even `2..=MAX_DASH_ARRAY`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DashArray {
    pub(crate) vals: [f32; MAX_DASH_ARRAY],
    pub(crate) len: u8,
}

impl DashArray {
    pub(crate) const NONE: Self = Self { vals: [0.0; MAX_DASH_ARRAY], len: 0 };

    #[inline]
    pub(crate) fn is_none(self) -> bool {
        self.len == 0
    }

    #[inline]
    pub(crate) fn zeros(len: u8) -> Self {
        Self { vals: [0.0; MAX_DASH_ARRAY], len }
    }

    #[inline]
    pub(crate) fn as_slice(&self) -> &[f32] {
        &self.vals[..self.len as usize]
    }
}

impl Default for DashArray {
    fn default() -> Self {
        Self::NONE
    }
}

/// Resolved stroke dashing parameters handed to `area::render_aaa`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StrokeDashSpec {
    pub(crate) array: DashArray,
    pub(crate) offset: f32,
    pub(crate) path_length: Option<f32>,
}

/// Normalized SVG viewport diagonal (`sqrt(w^2 + h^2) / sqrt(2)`) used to resolve `%` lengths.
#[inline]
pub(crate) fn view_box_diagonal(vb: [f32; 4]) -> f32 {
    ((vb[2] as f64).hypot(vb[3] as f64) * std::f64::consts::FRAC_1_SQRT_2) as f32
}

/// Parses an SVG `pathLength` attribute (non-negative finite `<number>`).
pub(crate) fn parse_path_length(raw: &str) -> Option<f32> {
    let v = raw.trim().parse::<f32>().ok()?;
    (v.is_finite() && v >= 0.0).then(|| v.min(MAX_DASH_VAL))
}

/// Parses one length token for `stroke-dashoffset` or a `stroke-dasharray` entry (bare number,
/// `px`, or `%` against `vb_diag`). Rejects negative values unless `signed` is true.
pub(crate) fn parse_dash_len(raw: &str, vb_diag: f32, signed: bool) -> Option<f32> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let val = if let Some(p) = t.strip_suffix('%') {
        let n = p.trim().parse::<f32>().ok()?;
        n * (vb_diag / 100.0)
    } else if let Some(p) = t.strip_suffix("px") {
        p.trim().parse::<f32>().ok()?
    } else {
        t.parse::<f32>().ok()?
    };
    if !val.is_finite() {
        return None;
    }
    if !signed && val < 0.0 {
        return None;
    }
    Some(if signed { val.clamp(-MAX_DASH_VAL, MAX_DASH_VAL) } else { val.clamp(0.0, MAX_DASH_VAL) })
}

/// Parses an SVG `stroke-dasharray` value (or a SMIL `by`/`additive="sum"` delta when `is_delta`
/// is true). Odd-length lists are repeated once to even length per SVG 2 / Chromium.
pub(crate) fn parse_dash_array(raw: &str, vb_diag: f32, is_delta: bool) -> Option<DashArray> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    if t == "none" {
        return (!is_delta).then_some(DashArray::NONE);
    }

    let mut vals = [0.0f32; MAX_DASH_ARRAY];
    let mut count = 0usize;
    let mut seen_comma = false;
    let bytes = t.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b',' {
            if count == 0 || seen_comma {
                return None;
            }
            seen_comma = true;
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b',' {
            i += 1;
        }
        let v = parse_dash_len(&t[start..i], vb_diag, is_delta)?;
        if count >= MAX_DASH_ARRAY {
            return None;
        }
        vals[count] = v;
        count += 1;
        seen_comma = false;
    }
    if count == 0 || seen_comma {
        return None;
    }
    if count % 2 == 1 {
        if count * 2 > MAX_DASH_ARRAY {
            return None;
        }
        for j in 0..count {
            vals[count + j] = vals[j];
        }
        count *= 2;
    }
    Some(DashArray { vals, len: count as u8 })
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

fn lcm_bounded(a: usize, b: usize) -> Option<usize> {
    if a == 0 || b == 0 {
        return None;
    }
    let l = (a / gcd(a, b)).checked_mul(b)?;
    (l <= MAX_DASH_ARRAY).then_some(l)
}

/// True when two normalized dash arrays represent the same pattern within tolerance.
pub(crate) fn dash_eq(a: DashArray, b: DashArray) -> bool {
    if a.len == 0 && b.len == 0 {
        return true;
    }
    let Some(l) = lcm_bounded(a.len as usize, b.len as usize) else {
        return false;
    };
    (0..l).all(|i| (a.vals[i % a.len as usize] - b.vals[i % b.len as usize]).abs() <= 1e-6)
}

/// Euclidean distance between two dash arrays (used by SMIL `calcMode="paced"`).
pub(crate) fn dash_distance(a: DashArray, b: DashArray) -> f32 {
    let Some(l) = lcm_bounded(a.len as usize, b.len as usize) else {
        return 0.0;
    };
    let sq: f32 = (0..l)
        .map(|i| {
            let d = b.vals[i % b.len as usize] - a.vals[i % a.len as usize];
            d * d
        })
        .sum();
    sq.sqrt()
}

/// Interpolates between two dash arrays at `frac`, repeating mismatched even lists to their LCM
/// (matching Chromium), or falling back to a midpoint step for `none` / over-cap LCM.
pub(crate) fn lerp_dash_array(a: DashArray, b: DashArray, frac: f32, is_delta: bool) -> DashArray {
    let Some(l) = lcm_bounded(a.len as usize, b.len as usize) else {
        return if frac < 0.5 { a } else { b };
    };
    let mut vals = [0.0f32; MAX_DASH_ARRAY];
    for (i, slot) in vals.iter_mut().enumerate().take(l) {
        let av = a.vals[i % a.len as usize];
        let bv = b.vals[i % b.len as usize];
        let out = av + (bv - av) * frac;
        let v = if out.is_finite() { out } else { bv };
        *slot = if is_delta { v.clamp(-MAX_DASH_VAL, MAX_DASH_VAL) } else { v.clamp(0.0, MAX_DASH_VAL) };
    }
    DashArray { vals, len: l as u8 }
}

/// Element-wise addition for SMIL `by` and `additive="sum"` on `stroke-dasharray`.
pub(crate) fn add_dash_array(base: DashArray, delta: DashArray, clamp_nonneg: bool) -> Option<DashArray> {
    let l = lcm_bounded(base.len as usize, delta.len as usize)?;
    let mut vals = [0.0f32; MAX_DASH_ARRAY];
    for (i, slot) in vals.iter_mut().enumerate().take(l) {
        let out = base.vals[i % base.len as usize] + delta.vals[i % delta.len as usize];
        if !out.is_finite() {
            return None;
        }
        *slot = if clamp_nonneg { out.clamp(0.0, MAX_DASH_VAL) } else { out.clamp(-MAX_DASH_VAL, MAX_DASH_VAL) };
    }
    Some(DashArray { vals, len: l as u8 })
}

/// Result of dashing a path in user space before stroking.
pub(crate) enum DashOutcome {
    /// Draw the stroke undashed (e.g. all-zero dasharray or microscopic interval cap fallback).
    Solid,
    /// Dash pattern yields zero visible segments (draw nothing).
    Empty,
    /// Dashed subpaths ready to pass to `Path::stroke`.
    Dashed(Path),
}

/// Applies `spec` (including `pathLength` normalization and segment-count bounds) to `user`.
pub(crate) fn apply_dash(user: &Path, spec: &StrokeDashSpec, cap: Cap, res: f32) -> DashOutcome {
    let raw = spec.array.as_slice();
    if raw.is_empty() {
        return DashOutcome::Solid;
    }
    let raw_sum: f64 = raw.iter().map(|&v| v as f64).sum();
    if !raw_sum.is_finite() || raw_sum <= 0.0 {
        return DashOutcome::Solid;
    }

    let total_len = measure_path_length(user, res);
    if !total_len.is_finite() || total_len <= 0.0 {
        return DashOutcome::Solid;
    }

    let scale: f64 = match spec.path_length {
        None => 1.0,
        Some(pl) if pl > 0.0 => total_len / pl as f64,
        // SVG 2: pathLength="0" scales 0 -> 0 and >0 -> +infinity.
        Some(_) => {
            if spec.offset == 0.0 {
                if raw[0] > 0.0 {
                    return DashOutcome::Solid;
                }
                // Zero-length leading dash followed by infinite gap: cap dot only (or empty for Butt).
                if cap == Cap::Butt {
                    return DashOutcome::Empty;
                }
                (total_len * 4.0) / raw_sum
            } else {
                return DashOutcome::Empty;
            }
        }
    };
    if !scale.is_finite() || scale <= 0.0 {
        return DashOutcome::Solid;
    }

    let mut scaled = Vec::with_capacity(raw.len());
    let mut interval_len = 0.0f64;
    for &v in raw {
        let sv = (v as f64 * scale) as f32;
        if !sv.is_finite() || sv < 0.0 {
            return DashOutcome::Solid;
        }
        scaled.push(sv);
        interval_len += sv as f64;
    }
    if !interval_len.is_finite() || interval_len <= 0.0 {
        return DashOutcome::Solid;
    }

    let scaled_offset = (spec.offset as f64 * scale) as f32;
    if !scaled_offset.is_finite() {
        return DashOutcome::Solid;
    }

    // Bound total emitted dash segments to keep icon rasterization fast even for hostile or
    // near-zero animated dash intervals (e.g. from="0" to="10" at t = 0.001).
    let est_segments = (total_len / interval_len) * (scaled.len() / 2) as f64;
    if !est_segments.is_finite() || est_segments > MAX_DASH_SEGMENTS as f64 {
        let all_on_zero = scaled.iter().step_by(2).all(|&v| v == 0.0);
        return if all_on_zero && cap == Cap::Butt { DashOutcome::Empty } else { DashOutcome::Solid };
    }

    let Some(sd) = StrokeDash::new(scaled, scaled_offset) else {
        return DashOutcome::Solid;
    };
    match user.dash(&sd, res) {
        Some(dashed) => DashOutcome::Dashed(dashed),
        None => DashOutcome::Empty,
    }
}

/// Measures the total arc length of `path` using the exact same contour subdivision tolerance and
/// per-contour `f32` accumulation as `tiny_skia_path::Path::dash`, so `pathLength="1"` +
/// `stroke-dasharray="1"` + `stroke-dashoffset="1"` lands cleanly on the contour boundary with
/// zero stray end-cap dots.
pub(crate) fn measure_path_length(path: &Path, res_scale: f32) -> f64 {
    const MAX_T_VALUE: u32 = 0x3FFF_FFFF;
    let tolerance = 0.5 / res_scale;
    let mut total = 0.0f64;
    let mut contour_len = 0.0f32;
    let mut first_pt = Point::zero();
    let mut prev_p = Point::zero();
    let mut has_contour = false;

    for seg in path.segments() {
        match seg {
            PathSegment::MoveTo(p0) => {
                if has_contour {
                    total += contour_len as f64;
                }
                contour_len = 0.0;
                first_pt = p0;
                prev_p = p0;
                has_contour = true;
            }
            PathSegment::LineTo(p0) => {
                let d = prev_p.distance(p0);
                let next = contour_len + d;
                if next > contour_len {
                    contour_len = next;
                }
                prev_p = p0;
            }
            PathSegment::QuadTo(p0, p1) | PathSegment::ConicTo(p0, p1, _) => {
                contour_len = measure_quad([prev_p, p0, p1], contour_len, 0, MAX_T_VALUE, tolerance);
                prev_p = p1;
            }
            PathSegment::CubicTo(p0, p1, p2) => {
                contour_len = measure_cubic([prev_p, p0, p1, p2], contour_len, 0, MAX_T_VALUE, tolerance);
                prev_p = p2;
            }
            PathSegment::Close => {
                if has_contour {
                    let d = prev_p.distance(first_pt);
                    let next = contour_len + d;
                    if next > contour_len {
                        contour_len = next;
                    }
                    total += contour_len as f64;
                    contour_len = 0.0;
                    prev_p = first_pt;
                    has_contour = false;
                }
            }
        }
    }
    if has_contour {
        total += contour_len as f64;
    }
    total
}

fn measure_quad(pts: [Point; 3], mut dist: f32, min_t: u32, max_t: u32, tol: f32) -> f32 {
    if ((max_t - min_t) >> 10) != 0 && quad_too_curvy(pts[0], pts[1], pts[2], tol) {
        let mut tmp = [Point::zero(); 5];
        let half_t = (min_t + max_t) >> 1;
        path_geometry::chop_quad_at(&pts, NormalizedF32Exclusive::HALF, &mut tmp);
        dist = measure_quad([tmp[0], tmp[1], tmp[2]], dist, min_t, half_t, tol);
        dist = measure_quad([tmp[2], tmp[3], tmp[4]], dist, half_t, max_t, tol);
    } else {
        let next = dist + pts[0].distance(pts[2]);
        if next > dist {
            dist = next;
        }
    }
    dist
}

fn measure_cubic(pts: [Point; 4], mut dist: f32, min_t: u32, max_t: u32, tol: f32) -> f32 {
    if ((max_t - min_t) >> 10) != 0 && cubic_too_curvy(pts[0], pts[1], pts[2], pts[3], tol) {
        let mut tmp = [Point::zero(); 7];
        let half_t = (min_t + max_t) >> 1;
        path_geometry::chop_cubic_at2(&pts, NormalizedF32Exclusive::HALF, &mut tmp);
        dist = measure_cubic([tmp[0], tmp[1], tmp[2], tmp[3]], dist, min_t, half_t, tol);
        dist = measure_cubic([tmp[3], tmp[4], tmp[5], tmp[6]], dist, half_t, max_t, tol);
    } else {
        let next = dist + pts[0].distance(pts[3]);
        if next > dist {
            dist = next;
        }
    }
    dist
}

fn quad_too_curvy(p0: Point, p1: Point, p2: Point, tol: f32) -> bool {
    let dx = (p1.x * 0.5) - ((p0.x + p2.x) * 0.5) * 0.5;
    let dy = (p1.y * 0.5) - ((p0.y + p2.y) * 0.5) * 0.5;
    dx.abs().max(dy.abs()) > tol
}

fn cubic_too_curvy(p0: Point, p1: Point, p2: Point, p3: Point, tol: f32) -> bool {
    let n0 = ((p0.x + (p3.x - p0.x) * (1.0 / 3.0)) - p1.x)
        .abs()
        .max(((p0.y + (p3.y - p0.y) * (1.0 / 3.0)) - p1.y).abs())
        > tol;
    let n1 = ((p0.x + (p3.x - p0.x) * (2.0 / 3.0)) - p2.x)
        .abs()
        .max(((p0.y + (p3.y - p0.y) * (2.0 / 3.0)) - p2.y).abs())
        > tol;
    n0 || n1
}
