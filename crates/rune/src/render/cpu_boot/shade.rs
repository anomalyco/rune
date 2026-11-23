//! shader.wgsl and clip.wgsl on the CPU, function for function in f32: one pixel's premultiplied
//! colour for each instance kind `cpu_boot` draws. The names are the shader's, so a change there
//! has one obvious place to land here.

use crate::render::instance::shader_consts;
use crate::render::{CONTINUOUS_K, Instance, Kind, unpack_clip_radii};

fn clamp01(x: f32) -> f32 { x.clamp(0.0, 1.0) }

fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] { std::array::from_fn(|c| a[c] * (1.0 - t) + b[c] * t) }

fn scale4(a: [f32; 4], k: f32) -> [f32; 4] { a.map(|v| v * k) }

/// `sd_continuous`.
fn sd_continuous(p: [f32; 2], half: [f32; 2], r: f32) -> f32 {
    let [c0, c1, c2, c3] = shader_consts::CONTINUOUS_POLY;
    let e = CONTINUOUS_K * r;
    let q = [half[0] - p[0].abs(), half[1] - p[1].abs()];
    let v = [e - q[0], e - q[1]];
    if v[0] <= 0.0 || v[1] <= 0.0 { return (-q[0]).max(-q[1]); }
    let u = v[1].atan2(v[0]) * (4.0 / shader_consts::CONTINUOUS_PI) - 1.0;
    let u2 = u * u;
    (v[0] * v[0] + v[1] * v[1]).sqrt() - e * (c0 + u2 * (c1 + u2 * (c2 + c3 * u2)))
}

/// `sd_rounded`: radii top-left, top-right, bottom-right, bottom-left (y down); negative is
/// continuous.
pub(super) fn sd_rounded(p: [f32; 2], half: [f32; 2], radii: [f32; 4]) -> f32 {
    let r = if p[0] > 0.0 { if p[1] > 0.0 { radii[2] } else { radii[1] } } else if p[1] > 0.0 { radii[3] } else { radii[0] };
    let m = half[0].min(half[1]);
    if r < 0.0 { return sd_continuous(p, half, (-r).min(m)); }
    let r = r.min(m);
    let q = [p[0].abs() - half[0] + r, p[1].abs() - half[1] + r];
    let (a, b) = (q[0].max(0.0), q[1].max(0.0));
    q[0].max(q[1]).min(0.0) + (a * a + b * b).sqrt() - r
}

/// `ramp`.
fn ramp(d: f32, fade: f32) -> f32 {
    if fade == 0.0 { return clamp01(d + 0.5); }
    let x = clamp01(d / fade.abs());
    if fade < 0.0 { x } else { x * x * (3.0 - 2.0 * x) }
}

/// `ramp_stops`: matches `clip.wgsl`'s `ramp_stops`.
fn ramp_stops(d: f32, fade: f32, ts: [f32; 4], ys: [f32; 4]) -> f32 {
    if fade == 0.0 { return clamp01(d + 0.5); }
    let w = fade.abs();
    if d >= w { return 1.0; }
    if d <= 0.0 { return 0.0; }
    let x = d / w;
    let (mut px, mut py) = (0.0f32, 0.0f32);
    for k in 0..4 {
        let (nx, ny) = (ts[k], ys[k]);
        if x <= nx {
            let span = nx - px;
            return if span > 1e-6 { py + (ny - py) * ((x - px) / span) } else { ny };
        }
        px = nx;
        py = ny;
    }
    1.0
}

/// An instance's clip, decoded once: `clip_alpha_stops`'s inputs.
#[derive(Clone, Copy)]
pub(super) struct Clip {
    pub rect: [f32; 4],
    fade: [f32; 4],
    offset: [f32; 4],
    is_stops: [bool; 4],
    ts: [f32; 4],
    ys: [f32; 4],
    pub radii: Option<[f32; 4]>,
}

impl Clip {
    pub(super) fn of(i: &Instance) -> Clip {
        Self::of_stops(i, [0.0; 2])
    }

    pub(super) fn of_stops(i: &Instance, stops: [f32; 2]) -> Clip {
        let f = i.fade;
        let half = |w: u32, hi: bool| half::f16::from_bits((if hi { w >> 16 } else { w }) as u16).to_f32();
        let (ts, ys) = if (f[2] & 0x8080_8080) != 0 {
            let lo = (stops[0] + 0.5) as u32;
            let hi = (stops[1] + 0.5) as u32;
            (
                [(lo & 0xFF) as f32 * (1.0 / 200.0), ((lo >> 16) & 0xFF) as f32 * (1.0 / 200.0), ((hi >> 8) & 0xFF) as f32 * (1.0 / 200.0), 1.0],
                [((lo >> 8) & 0xFF) as f32 * (1.0 / 200.0), (hi & 0xFF) as f32 * (1.0 / 200.0), ((hi >> 16) & 0xFF) as f32 * (1.0 / 200.0), 1.0],
            )
        } else {
            ([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0])
        };
        Clip {
            rect: i.clip,
            fade: [half(f[0], false), half(f[0], true), half(f[1], false), half(f[1], true)],
            offset: std::array::from_fn(|e| ((f[2] >> (e * 8)) & 0x7f) as f32 / 127.0),
            is_stops: std::array::from_fn(|e| ((f[2] >> (e * 8)) & 0x80) != 0),
            ts,
            ys,
            radii: (f[3] != 0).then(|| unpack_clip_radii(f[3])),
        }
    }

    /// `clip_alpha_stops`.
    pub(super) fn alpha(&self, p: [f32; 2]) -> f32 {
        let c = self.rect;
        let hard = [p[0] - c[0], p[1] - c[1], c[2] - p[0], c[3] - p[1]];
        let mut alpha = 1.0;
        for edge in 0..4 {
            let (f, o, is_stops) = (self.fade[edge], self.offset[edge], self.is_stops[edge]);
            let a = if o >= 1.0 {
                ramp(hard[edge], 0.0)
            } else if o > 0.0 {
                let shifted = hard[edge] + o * f.abs();
                let r = if is_stops { ramp_stops(shifted, f, self.ts, self.ys) } else { ramp(shifted, f) };
                ramp(hard[edge], 0.0).min(r)
            } else if is_stops {
                ramp_stops(hard[edge], f, self.ts, self.ys)
            } else {
                ramp(hard[edge], f)
            };
            alpha *= a;
        }
        if let Some(radii) = self.radii {
            let half = [(c[2] - c[0]) * 0.5, (c[3] - c[1]) * 0.5];
            alpha *= clamp01(0.5 - sd_rounded([p[0] - c[0] - half[0], p[1] - c[1] - half[1]], half, radii));
        }
        alpha
    }

    /// How far in from each edge (`[left, top, right, bottom]`) the clip is whole: a hard edge
    /// half a pixel, a soft one its whole fade.
    pub(super) fn inset(&self) -> [f32; 4] {
        std::array::from_fn(|e| if self.offset[e] >= 1.0 || self.fade[e] == 0.0 { 0.5 } else { self.fade[e].abs().max(0.5) })
    }
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    (-(x * x) / (2.0 * sigma * sigma)).exp() / ((2.0 * shader_consts::CONTINUOUS_PI).sqrt() * sigma)
}

fn erf(x: f32) -> f32 {
    let [a1, a2, a3] = shader_consts::ERF_POLY;
    let s = if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 };
    let a = x.abs();
    // From 5.5 on, `s / r⁴` is under half an ulp of 1: the result is `s` exactly.
    if a >= 5.5 { return s; }
    let r = 1.0 + (a1 + (a2 + a3 * (a * a)) * a) * a;
    let r = r * r;
    s - s / (r * r)
}

fn shadow_radii(i: &Instance, half: [f32; 2]) -> [f32; 4] {
    let m = half[0].min(half[1]);
    let raw = if i.uv[2] > 0.0 { [i.radii[0]; 4] } else { i.radii };
    raw.map(|r| r.abs().min(m))
}

fn shadow_x(x: f32, y: f32, sigma: f32, radii: [f32; 4], half: [f32; 2]) -> f32 {
    let [cl, cr] = if y > 0.0 { [radii[3], radii[2]] } else { [radii[0], radii[1]] };
    let dl = (half[1] - cl - y.abs()).min(0.0);
    let dr = (half[1] - cr - y.abs()).min(0.0);
    let left = half[0] - cl + (cl * cl - dl * dl).max(0.0).sqrt();
    let right = half[0] - cr + (cr * cr - dr * dr).max(0.0).sqrt();
    let k = 0.5f32.sqrt() / sigma;
    (0.5 + 0.5 * erf((x + left) * k)) - (0.5 + 0.5 * erf((x - right) * k))
}

/// shader.wgsl's `SHADOW_INSIDE`.
pub(super) use shader_consts::SHADOW_INSIDE;

/// `shadow`.
fn shadow(p: [f32; 2], half: [f32; 2], sigma: f32, radii: [f32; 4]) -> f32 {
    let corner = radii[0].max(radii[1]).max(radii[2]).max(radii[3]);
    if p[0].abs() <= half[0] - shader_consts::SHADOW_INSIDE_SIGMA * sigma
        && p[1].abs() <= half[1] - corner - shader_consts::SHADOW_INSIDE_SIGMA * sigma
    {
        return SHADOW_INSIDE;
    }
    let (lo, hi) = (p[1] - half[1], p[1] + half[1]);
    if !(lo <= hi) || !lo.is_finite() || !hi.is_finite() { return 0.0; }
    let start = (-shader_consts::SHADOW_WINDOW_SIGMA * sigma).clamp(lo, hi);
    let end = (shader_consts::SHADOW_WINDOW_SIGMA * sigma).clamp(lo, hi);
    let step = (end - start) / shader_consts::SHADOW_SAMPLES as f32;
    let mut y = start + step * 0.5;
    let mut v = 0.0;
    for _ in 0..shader_consts::SHADOW_SAMPLES {
        v += shadow_x(p[0], p[1] - y, sigma, radii, half) * gaussian(y, sigma) * step;
        y += step;
    }
    v
}

/// `shadow` for one row of a `KIND_SHADOW`: what its four y samples take from the row alone
/// (each sample's left and right curved extents and Gaussian weight), so a pixel costs only its
/// erf pairs. The arithmetic and its order are `shadow`'s.
pub(super) struct ShadowRow {
    /// The row's centre relative to the shadow's centre.
    y: f32,
    py: f32,
    samples: [(f32, f32, f32); shader_consts::SHADOW_SAMPLES],
    step: f32,
    half: [f32; 2],
    sigma: f32,
    corner: f32,
    k: f32,
    /// Within `|x| <= mid.0` every sample's erf pair is exactly `1 - 0`: the row's value there is
    /// `mid.1` whatever `x` is.
    pub mid: (f32, f32),
}

impl ShadowRow {
    /// What a pixel's sum depends on besides its `x`: rows that share it share every column's.
    pub(super) fn key(&self) -> [u32; shader_consts::SHADOW_SAMPLES * 3 + 1] {
        let mut k = [self.step.to_bits(); shader_consts::SHADOW_SAMPLES * 3 + 1];
        for (n, &(l, r, g)) in self.samples.iter().enumerate() {
            k[n * 3] = l.to_bits();
            k[n * 3 + 1] = r.to_bits();
            k[n * 3 + 2] = g.to_bits();
        }
        k
    }

    /// Whether `x` (from the shadow's centre) is in the interior `shadow` returns
    /// `SHADOW_INSIDE` for.
    pub(super) fn inside(&self, x: f32) -> bool {
        x.abs() <= self.half[0] - shader_consts::SHADOW_INSIDE_SIGMA * self.sigma
            && self.y.abs() <= self.half[1] - self.corner - shader_consts::SHADOW_INSIDE_SIGMA * self.sigma
    }

    /// The run of `x` (from the shadow's centre, `|x| <= reach`) where this row's colour doesn't
    /// change, and its blur there: the interior, on a row that crosses it, else the middle.
    pub(super) fn flat(&self) -> Option<(f32, f32)> {
        if self.y.abs() <= self.half[1] - self.corner - shader_consts::SHADOW_INSIDE_SIGMA * self.sigma {
            return Some((self.half[0] - shader_consts::SHADOW_INSIDE_SIGMA * self.sigma, SHADOW_INSIDE));
        }
        (self.mid.0 > 0.0).then_some(self.mid)
    }

    /// The erf sum at `x` (from the shadow's centre), outside the interior.
    pub(super) fn sum(&self, x: f32) -> f32 {
        if self.step == 0.0 { return 0.0; }
        if x.abs() <= self.mid.0 { return self.mid.1; }
        let mut v = 0.0;
        for &(left, right, g) in &self.samples {
            let sx = (0.5 + 0.5 * erf((x + left) * self.k)) - (0.5 + 0.5 * erf((x - right) * self.k));
            v += sx * g * self.step;
        }
        v
    }
}

pub(super) fn shadow_row(i: &Instance, py: f32) -> ShadowRow {
    let half = [i.rect[2] * 0.5, i.rect[3] * 0.5];
    let y = py - (i.rect[1] + half[1]);
    let sigma = i.params[2].max(shader_consts::SHADOW_MIN_SIGMA);
    let radii = shadow_radii(i, half);
    let corner = radii[0].max(radii[1]).max(radii[2]).max(radii[3]);
    let (lo, hi) = (y - half[1], y + half[1]);
    if !(lo <= hi) || !lo.is_finite() || !hi.is_finite() {
        return ShadowRow { y, py, samples: [(0.0, 0.0, 0.0); shader_consts::SHADOW_SAMPLES], step: 0.0, half: [0.0, 0.0], sigma, corner: 0.0, k: 0.0, mid: (-1.0, 0.0) };
    }
    let start = (-shader_consts::SHADOW_WINDOW_SIGMA * sigma).clamp(lo, hi);
    let end = (shader_consts::SHADOW_WINDOW_SIGMA * sigma).clamp(lo, hi);
    let step = (end - start) / shader_consts::SHADOW_SAMPLES as f32;
    let mut s = start + step * 0.5;
    let samples = std::array::from_fn(|_| {
        let at = s;
        s += step;
        let ry = y - at;
        let [cl, cr] = if ry > 0.0 { [radii[3], radii[2]] } else { [radii[0], radii[1]] };
        let dl = (half[1] - cl - ry.abs()).min(0.0);
        let dr = (half[1] - cr - ry.abs()).min(0.0);
        let left = half[0] - cl + (cl * cl - dl * dl).max(0.0).sqrt();
        let right = half[0] - cr + (cr * cr - dr * dr).max(0.0).sqrt();
        (left, right, gaussian(at, sigma))
    });
    let k = 0.5f32.sqrt() / sigma;
    // `erf` is exactly ±1 from 5.5 out; `(x + left) * k` and `(x - right) * k` must reach it for every sample.
    let reach = samples.iter().map(|s| s.0.min(s.1)).fold(f32::INFINITY, f32::min) - 5.5 / k;
    let mut v = 0.0;
    for &(_, _, g) in &samples { v += 1.0 * g * step; }
    // Shrunk by a part in a million, so rounding in `x ± reach` can't land a pixel short of 5.5.
    let mid = (reach - reach.abs() * 1e-6 - 1e-3, v);
    ShadowRow { y, py, samples, step, half, sigma, corner, k, mid }
}


/// `KIND_SHADOW`'s colour at `px` given its blur `cov` there: cut by its hole, if it has one.
pub(super) fn shadow_cov(i: &Instance, row: &ShadowRow, px: f32, mut cov: f32) -> [f32; 4] {
    if i.uv[2] > 0.0 {
        let hole = [i.uv[2] * 0.5, i.uv[3] * 0.5];
        let at = [px - (i.rect[0] + i.uv[0] + hole[0]), row.py - (i.rect[1] + i.uv[1] + hole[1])];
        cov *= clamp01(0.5 + sd_rounded(at, hole, [i.radii[3]; 4]));
    }
    scale4(i.fill, cov)
}

/// `segment`.
fn segment(p: [f32; 2], a: [f32; 2], b: [f32; 2], hw: f32, round: bool) -> f32 {
    let w = hw.max(0.5);
    let weight = (hw * 2.0).min(1.0);
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len = (ab[0] * ab[0] + ab[1] * ab[1]).sqrt();
    if !len.is_finite() { return 0.0; }
    let ap = [p[0] - a[0], p[1] - a[1]];
    let d = if len < shader_consts::SEGMENT_MIN_LEN {
        if !round { return 0.0; }
        (ap[0] * ap[0] + ap[1] * ap[1]).sqrt() - w
    } else {
        let dir = [ab[0] / len, ab[1] / len];
        let along = ap[0] * dir[0] + ap[1] * dir[1];
        if round {
            let t = along.clamp(0.0, len);
            let (dx, dy) = (ap[0] - dir[0] * t, ap[1] - dir[1] * t);
            (dx * dx + dy * dy).sqrt() - w
        } else {
            let across = (dir[0] * ap[1] - dir[1] * ap[0]).abs();
            let q = [(along - len * 0.5).abs() - len * 0.5, across - w];
            let (mx, my) = (q[0].max(0.0), q[1].max(0.0));
            q[0].max(q[1]).min(0.0) + (mx * mx + my * my).sqrt()
        }
    };
    clamp01(0.5 - d) * weight
}

fn edge(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    let e = [b[0] - a[0], b[1] - a[1]];
    let l = (e[0] * e[0] + e[1] * e[1]).sqrt().max(shader_consts::EDGE_EPSILON);
    let n = [-e[1] / l, e[0] / l];
    let s = if (c[0] - a[0]) * n[0] + (c[1] - a[1]) * n[1] >= 0.0 { 1.0 } else { -1.0 };
    ((p[0] - a[0]) * n[0] + (p[1] - a[1]) * n[1]) * s
}

fn owns(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let swap = a[0] > b[0] || (a[0] == b[0] && a[1] > b[1]);
    let (lo, hi) = if swap { (b, a) } else { (a, b) };
    let e = [hi[0] - lo[0], hi[1] - lo[1]];
    let side = e[0] * (c[1] - lo[1]) - e[1] * (c[0] - lo[0]);
    let v = e[0] * (p[1] - lo[1]) - e[1] * (p[0] - lo[0]);
    if side > 0.0 { v >= 0.0 } else { v < 0.0 }
}

fn soft(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    let d = edge(p, a, b, c);
    let e = [b[0] - a[0], b[1] - a[1]];
    let t = ((p[0] - a[0]) * e[0] + (p[1] - a[1]) * e[1]) / (e[0] * e[0] + e[1] * e[1]).max(shader_consts::EDGE_EPSILON);
    if !(0.0..=1.0).contains(&t) { return if d >= 0.0 { 1.0 } else { 0.0 }; }
    clamp01(d + 0.5)
}

/// `triangle`.
fn triangle(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2], aa: u32) -> f32 {
    let mut cov = 1.0;
    for (bit, (u, v, w)) in [
        (shader_consts::TRIANGLE_AA_AB, (a, b, c)),
        (shader_consts::TRIANGLE_AA_BC, (b, c, a)),
        (shader_consts::TRIANGLE_AA_CA, (c, a, b)),
    ] {
        if aa & bit != 0 { cov *= soft(p, u, v, w); } else if !owns(p, u, v, w) { return 0.0; }
    }
    cov
}

/// `ring_arc`.
fn ring_arc(p: [f32; 2], size: [f32; 2], radii: [f32; 4], bw: f32) -> (f32, f32) {
    let h = bw * 0.5;
    let half = [size[0] * 0.5, size[1] * 0.5];
    let m = half[0].min(half[1]);
    let rc = radii.map(|r| (r.abs().min(m) - h).max(0.0));
    let e = rc.map(|r| r + h);
    let arc = rc.map(|r| r * shader_consts::QUARTER_TURN);
    let s_tr = (size[0] - bw - rc[0] - rc[1]).max(0.0);
    let s_right = s_tr + arc[1];
    let s_br = s_right + (size[1] - bw - rc[1] - rc[2]).max(0.0);
    let s_bottom = s_br + arc[2];
    let s_bl = s_bottom + (size[0] - bw - rc[2] - rc[3]).max(0.0);
    let s_left = s_bl + arc[3];
    let s_tl = s_left + (size[1] - bw - rc[3] - rc[0]).max(0.0);
    let top = p[0] - e[0];
    let right = s_right + p[1] - e[1];
    let bottom = s_bottom + size[0] - e[2] - p[0];
    let left = s_left + size[1] - e[3] - p[1];
    let s = if p[0] > half[0] {
        if p[1] > half[1] {
            let v = [p[0] - (size[0] - e[2]), p[1] - (size[1] - e[2])];
            if v[0] > 0.0 && v[1] > 0.0 { s_br + v[1].atan2(v[0]) * rc[2] } else if size[0] - p[0] < size[1] - p[1] { right } else { bottom }
        } else {
            let v = [p[0] - (size[0] - e[1]), p[1] - e[1]];
            if v[0] > 0.0 && v[1] < 0.0 { s_tr + v[0].atan2(-v[1]) * rc[1] } else if size[0] - p[0] < p[1] { right } else { top }
        }
    } else if p[1] > half[1] {
        let v = [p[0] - e[3], p[1] - (size[1] - e[3])];
        if v[0] < 0.0 && v[1] > 0.0 { s_bl + (-v[0]).atan2(v[1]) * rc[3] } else if p[0] < size[1] - p[1] { left } else { bottom }
    } else {
        let v = [p[0] - e[0], p[1] - e[0]];
        if v[0] < 0.0 && v[1] < 0.0 { s_tl + (-v[1]).atan2(-v[0]) * rc[0] } else if p[0] < p[1] { left } else { top }
    };
    (s, s_tl + arc[0])
}

/// `dash_cov`.
fn dash_cov(s: f32, total: f32, d: [f32; 3]) -> f32 {
    let nominal = d[0] + d[1];
    let k = total.max(shader_consts::DASH_MIN_TOTAL) / ((total / nominal + 0.5).floor().max(1.0) * nominal);
    let period = nominal * k;
    let dash = d[0] * k;
    let a = s + d[2] * k - dash * 0.5;
    let m = a - period * (a / period + 0.5).floor();
    clamp01(dash * 0.5 - m.abs() + 0.5)
}

/// `border_over`.
#[allow(clippy::too_many_arguments)]
fn border_over(fill: [f32; 4], edge: [f32; 4], d: f32, inner_d: f32, soft: f32, bw: f32, opacity: f32, shrink: bool) -> [f32; 4] {
    let cov = clamp01(0.5 - d / (soft * 2.0));
    let ring = (cov - clamp01(0.5 - inner_d / (soft * 2.0))).max(0.0);
    let hide = clamp01(edge[3] / opacity.clamp(shader_consts::OPACITY_EPSILON, 1.0));
    let under = clamp01(
        0.5 - (d + if shrink && hide > shader_consts::OPAQUE_HIDE_THRESHOLD { bw * 0.5 } else { 0.0 }) / (soft * 2.0),
    );
    std::array::from_fn(|c| edge[c] * ring + fill[c] * (under * (1.0 - hide * ring)))
}

/// `box_cov`.
fn box_cov(p: [f32; 2], half: [f32; 2], radii: [f32; 4]) -> f32 {
    let r = if p[0] > 0.0 { if p[1] > 0.0 { radii[2] } else { radii[1] } } else if p[1] > 0.0 { radii[3] } else { radii[0] };
    if r == 0.0 {
        let e = |a: usize| clamp01(half[a].min(p[a] + 0.5) - (-half[a]).max(p[a] - 0.5));
        return e(0) * e(1);
    }
    clamp01(0.5 - sd_rounded(p, half, radii))
}

/// The per-side widths' inner box of a `KIND_RECT` border: its centre, half size and radii.
pub(super) fn sides_inner(i: &Instance, center: [f32; 2], half: [f32; 2]) -> ([f32; 2], [f32; 2], [f32; 4]) {
    let w = i.uv;
    let ih = [(half[0] - (w[1] + w[3]) * 0.5).max(0.0), (half[1] - (w[0] + w[2]) * 0.5).max(0.0)];
    let ic = [center[0] + (w[3] - w[1]) * 0.5, center[1] + (w[0] - w[2]) * 0.5];
    let cut = [w[0].max(w[3]), w[0].max(w[1]), w[2].max(w[1]), w[2].max(w[3])];
    let r = i.radii;
    (ic, ih, std::array::from_fn(|n| (r[n].abs() - cut[n]).max(0.0).copysign(r[n])))
}

/// Whether a `KIND_RECT` border has per-side widths.
pub(super) fn has_sides(i: &Instance) -> bool {
    i.uv[2] >= 0.0 && i.uv != [0.0; 4]
}

/// The colour (premultiplied, before the clip) of one of the box kinds (`KIND_RECT`,
/// `KIND_SHADOW`, `KIND_INSET`, `KIND_GRADIENT`, `KIND_SEGMENT`, `KIND_TRIANGLE`, `KIND_DASHED`,
/// `KIND_RADIAL`) at pixel centre `p`: the `fs` branch for its kind.
pub(super) fn boxes(i: &Instance, kind: Kind, p: [f32; 2]) -> [f32; 4] {
    let half = [i.rect[2] * 0.5, i.rect[3] * 0.5];
    let center = [i.rect[0] + half[0], i.rect[1] + half[1]];
    let q = [p[0] - center[0], p[1] - center[1]];
    let soft = i.params[2].max(shader_consts::SOFT_MIN);
    let t_y = || clamp01((p[1] - i.rect[1]) / i.rect[3].max(1.0));
    match kind {
        Kind::Rect => {
            let d = sd_rounded(q, half, i.radii);
            let bw = i.params[0];
            if bw <= 0.0 { return scale4(i.fill, clamp01(0.5 - d / (soft * 2.0))); }
            let sides = has_sides(i);
            let inner_d = if sides {
                let (ic, ih, ir) = sides_inner(i, center, half);
                sd_rounded([p[0] - ic[0], p[1] - ic[1]], ih, ir)
            } else {
                d + bw
            };
            border_over(i.fill, i.border, d, inner_d, soft, bw, i.params[3], !sides && i.radii != [0.0; 4])
        }
        Kind::Shadow => {
            let sigma = i.params[2].max(shader_consts::SHADOW_MIN_SIGMA);
            let mut cov = shadow(q, half, sigma, shadow_radii(i, half));
            if i.uv[2] > 0.0 {
                let hole = [i.uv[2] * 0.5, i.uv[3] * 0.5];
                let at = [p[0] - (i.rect[0] + i.uv[0] + hole[0]), p[1] - (i.rect[1] + i.uv[1] + hole[1])];
                cov *= clamp01(0.5 + sd_rounded(at, hole, [i.radii[3]; 4]));
            }
            scale4(i.fill, cov)
        }
        Kind::Inset => {
            let spread = i.params[0];
            let ih = half.map(|v| (v - spread).max(0.0));
            let qs = [q[0] - i.uv[0], q[1] - i.uv[1]];
            let lit = if i.params[2] < shader_consts::INSET_SHARP_SIGMA {
                box_cov(qs, ih, i.radii.map(|r| (r.abs() - spread).max(0.0).copysign(r)))
            } else {
                let m = ih[0].min(ih[1]);
                shadow(qs, ih, i.params[2], i.radii.map(|r| (r.abs() - spread).min(m).max(0.0)))
            };
            scale4(i.fill, box_cov(q, half, i.radii) * (1.0 - clamp01(lit)))
        }
        Kind::Gradient => {
            let d = sd_rounded(q, half, i.radii);
            let t = t_y();
            let bw = i.params[0];
            if bw > 0.0 {
                border_over(i.fill, mix4(i.border, i.uv, t), d, d + bw, soft, bw, i.params[3], i.radii != [0.0; 4])
            } else {
                scale4(mix4(i.fill, i.uv, t), clamp01(0.5 - d / (soft * 2.0)))
            }
        }
        Kind::Segment => scale4(i.fill, segment(p, [i.radii[0], i.radii[1]], [i.radii[2], i.radii[3]], i.params[0], i.params[2] > shader_consts::FLAG_ON_THRESHOLD)),
        Kind::Triangle => scale4(i.fill, triangle(p, [i.radii[0], i.radii[1]], [i.radii[2], i.radii[3]], [i.uv[0], i.uv[1]], (i.params[0] + 0.5) as u32)),
        Kind::Dashed => {
            let d = sd_rounded(q, half, i.radii);
            let bw = i.params[0];
            let ring = clamp01(0.5 - d / (soft * 2.0)) - clamp01(0.5 - (d + bw) / (soft * 2.0));
            let (s, total) = ring_arc([p[0] - i.rect[0], p[1] - i.rect[1]], [i.rect[2], i.rect[3]], i.radii, bw);
            scale4(mix4(i.fill, i.border, t_y()), ring * dash_cov(s, total, [i.uv[0], i.uv[1], i.uv[2]]))
        }
        Kind::Radial => radial(i, p, false),
        Kind::Glyph | Kind::Image | Kind::Layer | Kind::LayerEnd | Kind::Effect | Kind::BackdropBlur => [0.0; 4],
    }
}

/// `KIND_RADIAL` at `p`: a radial or angled linear stop segment. `inside` says `p` is where the
/// rounded rect covers the pixel whole, so its distance needn't be taken (its coverage is 1).
pub(super) fn radial(i: &Instance, p: [f32; 2], inside: bool) -> [f32; 4] {
    let half = [i.rect[2] * 0.5, i.rect[3] * 0.5];
    let q = [p[0] - (i.rect[0] + half[0]), p[1] - (i.rect[1] + half[1])];
    let soft = i.params[2].max(shader_consts::SOFT_MIN);
    let (t0_raw, t1_raw) = (i.params[0], i.params[3]);
    let (lower_open, upper_open) = (
        t0_raw >= shader_consts::RADIAL_OPEN_THRESHOLD,
        t1_raw >= shader_consts::RADIAL_OPEN_THRESHOLD,
    );
    let t0 = if lower_open { t0_raw - shader_consts::RADIAL_OPEN_OFFSET } else { t0_raw };
    let t1 = if upper_open { t1_raw - shader_consts::RADIAL_OPEN_OFFSET } else { t1_raw };
    let t = if i.uv[2] > 0.0 {
        let (dx, dy) = ((p[0] - i.rect[0] - i.uv[0]) / i.uv[2], (p[1] - i.rect[1] - i.uv[1]) / i.uv[3]);
        (dx * dx + dy * dy).sqrt()
    } else {
        0.5 + (q[0] * i.uv[0] + q[1] * i.uv[1])
    };
    // Outside its stops a segment draws nothing, whatever its coverage: decided first.
    if (lower_open && t < t0) || (upper_open && t >= t1) { return [0.0; 4]; }
    let cov = if inside { 1.0 } else { clamp01(0.5 - sd_rounded(q, half, i.radii) / (soft * 2.0)) };
    let u = if t1 <= t0 + shader_consts::RADIAL_STOP_EPSILON { if t >= t0 { 1.0 } else { 0.0 } } else { clamp01((t - t0) / (t1 - t0)) };
    scale4(mix4(i.fill, i.border, u), cov)
}

/// Whether row `py`, from pixel centre `x0` to `x1`, is all outside a linear `KIND_RADIAL`
/// segment's open stops: `t` is linear along the row, so both ends on one side means every
/// pixel between is.
pub(super) fn linear_row_outside(i: &Instance, py: f32, x0: f32, x1: f32) -> bool {
    if i.uv[2] > 0.0 { return false; }
    let (cx, cy) = (i.rect[0] + i.rect[2] * 0.5, i.rect[1] + i.rect[3] * 0.5);
    let t = |px: f32| 0.5 + ((px - cx) * i.uv[0] + (py - cy) * i.uv[1]);
    let (ta, tb) = (t(x0), t(x1));
    let (t0_raw, t1_raw) = (i.params[0], i.params[3]);
    let (thr, off) = (shader_consts::RADIAL_OPEN_THRESHOLD, shader_consts::RADIAL_OPEN_OFFSET);
    (t0_raw >= thr && ta < t0_raw - off && tb < t0_raw - off) || (t1_raw >= thr && ta >= t1_raw - off && tb >= t1_raw - off)
}

/// `image_texel` in an sRGB-blending frame: one texel of an `Rgba8UnormSrgb` image page,
/// premultiplied in sRGB encoding. The page holds straight alpha unless `premultiplied`.
pub(super) fn image_texel(page: &super::Image<'_>, t: [i32; 2], premultiplied: bool) -> [f32; 4] {
    if t[0] < 0 || t[1] < 0 || t[0] as u32 >= page.width { return [0.0; 4]; }
    let Some(at) = (t[1] as usize)
        .checked_mul(page.width as usize)
        .and_then(|row| row.checked_add(t[0] as usize))
        .and_then(|px| px.checked_mul(4))
    else { return [0.0; 4] };
    let Some(c) = page.data.get(at..at + 4) else { return [0.0; 4] };
    let a = c[3] as f32 / 255.0;
    if a <= 0.0 { return [0.0; 4]; }
    let (decoded, round_trip) = srgb_tables();
    if !premultiplied {
        // The texture decodes sRGB to linear and the shader encodes it back: a round trip per
        // byte value, taken from a table.
        return [round_trip[c[0] as usize] * a, round_trip[c[1] as usize] * a, round_trip[c[2] as usize] * a, a];
    }
    // Opaque, the texel divides by exactly 1: the round trip again.
    if c[3] == 255 { return [round_trip[c[0] as usize], round_trip[c[1] as usize], round_trip[c[2] as usize], 1.0]; }
    let e = |v: u8| crate::color::srgb_encode(decoded[v as usize] / a) * a;
    [e(c[0]), e(c[1]), e(c[2]), a]
}

/// Each byte's sRGB decode (what an `Rgba8UnormSrgb` texel reads as) and its encode back, built
/// the first time an image draws.
fn srgb_tables() -> &'static ([f32; 256], [f32; 256]) {
    static TABLES: std::sync::OnceLock<([f32; 256], [f32; 256])> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| {
        let decoded: [f32; 256] = std::array::from_fn(|v| crate::color::srgb_decode(v as f32 / 255.0));
        (decoded, decoded.map(crate::color::srgb_encode))
    })
}

/// `KIND_IMAGE` at pixel centre `p`: bilinear, clamped to the image's own texels.
pub(super) fn image(i: &Instance, page: &super::Image<'_>, p: [f32; 2]) -> [f32; 4] {
    if !(i.rect[2] > 0.0 && i.rect[3] > 0.0 && i.uv[2] > 0.0 && i.uv[3] > 0.0)
        || !i.rect[2].is_finite() || !i.rect[3].is_finite()
        || !i.uv[0].is_finite() || !i.uv[1].is_finite()
        || !i.uv[2].is_finite() || !i.uv[3].is_finite()
    {
        return [0.0; 4];
    }
    let half = [i.rect[2] * 0.5, i.rect[3] * 0.5];
    let d = sd_rounded([p[0] - i.rect[0] - half[0], p[1] - i.rect[1] - half[1]], half, i.radii);
    let local = [(p[0] - i.rect[0]) * i.uv[2] / i.rect[2] - 0.5, (p[1] - i.rect[1]) * i.uv[3] / i.rect[3] - 0.5];
    if !local[0].is_finite() || !local[1].is_finite() { return [0.0; 4]; }
    let lo = [i.uv[0] as i32, i.uv[1] as i32];
    // A sub-texel crop (0 < uv < 1) spans less than one texel and samples `lo` without bleeding
    // into neighbouring atlas slots.
    let hi = [
        lo[0].saturating_add((i.uv[2] as i32).saturating_sub(1).max(0)),
        lo[1].saturating_add((i.uv[3] as i32).saturating_sub(1).max(0)),
    ];
    let base = [local[0].floor(), local[1].floor()];
    let f = [local[0] - base[0], local[1] - base[1]];
    let bx = lo[0].saturating_add(base[0] as i32);
    let by = lo[1].saturating_add(base[1] as i32);
    let t0 = [bx.clamp(lo[0], hi[0]), by.clamp(lo[1], hi[1])];
    let t1 = [bx.saturating_add(1).clamp(lo[0], hi[0]), by.saturating_add(1).clamp(lo[1], hi[1])];
    let pm = i.params[2] > shader_consts::FLAG_ON_THRESHOLD;
    let top = mix4(image_texel(page, t0, pm), image_texel(page, [t1[0], t0[1]], pm), f[0]);
    let bottom = mix4(image_texel(page, [t0[0], t1[1]], pm), image_texel(page, t1, pm), f[0]);
    scale4(mix4(top, bottom, f[1]), i.fill[3] * clamp01(0.5 - d))
}

#[cfg(test)]
#[path = "shade_tests.rs"]
mod tests;
