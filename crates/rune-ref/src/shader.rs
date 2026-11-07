//! Deliberately scalar transcription of shader.wgsl. No shared shader generator:
//! keeping the implementations independent catches changes to either lane.
use crate::{Atlas, Primitive};
use libm::{atan2f, cosf, expf, floorf, powf, sinf, sqrtf};

fn clamp(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}
fn sd(p: [f32; 2], h: [f32; 2], radii: [f32; 4]) -> f32 {
    let index = if p[0] > 0.0 {
        if p[1] > 0.0 { 2 } else { 1 }
    } else if p[1] > 0.0 {
        3
    } else {
        0
    };
    if radii[index] < 0.0 {
        return sd_continuous(p, h, (-radii[index]).min(h[0].min(h[1])));
    }
    let r = radii[index].min(h[0].min(h[1]));
    let q = [p[0].abs() - h[0] + r, p[1].abs() - h[1] + r];
    let a = q[0].max(0.0);
    let b = q[1].max(0.0);
    q[0].max(q[1]).min(0.0) + sqrtf(a * a + b * b) - r
}
/// Apple's continuous corner: from (K·r, K·r) inside the corner, the curve lies K·r·g(u) away.
fn sd_continuous(p: [f32; 2], h: [f32; 2], r: f32) -> f32 {
    let e = 1.528_664_8 * r;
    let q = [h[0] - p[0].abs(), h[1] - p[1].abs()];
    let v = [e - q[0], e - q[1]];
    if v[0] <= 0.0 || v[1] <= 0.0 {
        return (-q[0]).max(-q[1]);
    }
    let u = atan2f(v[1], v[0]) * (4.0 / std::f32::consts::PI) - 1.0;
    let u2 = u * u;
    let g = 1.146_022_9 + u2 * (-0.375_731_6 + u2 * (0.307_966_1 - 0.077_243_54 * u2));
    sqrtf(v[0] * v[0] + v[1] * v[1]) - e * g
}
fn ramp(d: f32, fade: f32) -> f32 {
    if fade == 0.0 {
        return clamp(d + 0.5);
    }
    let x = clamp(d / fade.abs());
    if fade < 0.0 { x } else { x * x * (3.0 - 2.0 * x) }
}
fn erf(x: f32) -> f32 {
    let s = if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    };
    let a = x.abs();
    let r = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    let r = r * r;
    s - s / (r * r)
}
fn shadow(p: [f32; 2], h: [f32; 2], sigma: f32, corner: f32) -> f32 {
    let lo = p[1] - h[1];
    let hi = p[1] + h[1];
    let start = (-3.0 * sigma).clamp(lo, hi);
    let end = (3.0 * sigma).clamp(lo, hi);
    let step = (end - start) / 4.0;
    let mut y = start + step * 0.5;
    let mut v = 0.0;
    for _ in 0..4 {
        let delta = (h[1] - corner - (p[1] - y).abs()).min(0.0);
        let curved = h[0] - corner + sqrtf((corner * corner - delta * delta).max(0.0));
        let scale = sqrtf(0.5) / sigma;
        let left = 0.5 + 0.5 * erf((p[0] - curved) * scale);
        let right = 0.5 + 0.5 * erf((p[0] + curved) * scale);
        let gaussian =
            expf(-(y * y) / (2.0 * sigma * sigma)) / (sqrtf(2.0 * std::f32::consts::PI) * sigma);
        v += (right - left) * gaussian * step;
        y += step;
    }
    v
}

fn ring_arc(p: [f32; 2], size: [f32; 2], radii: [f32; 4], bw: f32) -> (f32, f32) {
    let h = bw * 0.5;
    let m = (size[0] * 0.5).min(size[1] * 0.5);
    let rc = radii.map(|r| (r.abs().min(m) - h).max(0.0));
    let e = rc.map(|r| r + h);
    let arc = rc.map(|r| r * 1.5707964);
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
    let s = if p[0] > size[0] * 0.5 {
        if p[1] > size[1] * 0.5 {
            let v = [p[0] - (size[0] - e[2]), p[1] - (size[1] - e[2])];
            if v[0] > 0.0 && v[1] > 0.0 { s_br + atan2f(v[1], v[0]) * rc[2] }
            else if size[0] - p[0] < size[1] - p[1] { right } else { bottom }
        } else {
            let v = [p[0] - (size[0] - e[1]), p[1] - e[1]];
            if v[0] > 0.0 && v[1] < 0.0 { s_tr + atan2f(v[0], -v[1]) * rc[1] }
            else if size[0] - p[0] < p[1] { right } else { top }
        }
    } else if p[1] > size[1] * 0.5 {
        let v = [p[0] - e[3], p[1] - (size[1] - e[3])];
        if v[0] < 0.0 && v[1] > 0.0 { s_bl + atan2f(-v[0], v[1]) * rc[3] }
        else if p[0] < size[1] - p[1] { left } else { bottom }
    } else {
        let v = [p[0] - e[0], p[1] - e[0]];
        if v[0] < 0.0 && v[1] < 0.0 { s_tl + atan2f(-v[1], -v[0]) * rc[0] }
        else if p[0] < p[1] { left } else { top }
    };
    (s, s_tl + arc[0])
}
fn dash_cov(s: f32, total: f32, d: [f32; 3]) -> f32 {
    let nominal = d[0] + d[1];
    let k = total.max(0.001) / ((floorf(total / nominal + 0.5)).max(1.0) * nominal);
    let period = nominal * k;
    let dash = d[0] * k;
    let a = s + d[2] * k - dash * 0.5;
    let m = a - period * floorf(a / period + 0.5);
    clamp(dash * 0.5 - m.abs() + 0.5)
}

/// A border over its own background, as CSS paints one (shader.wgsl `border_over`).
#[allow(clippy::too_many_arguments)]
fn border_over(fill: [f32; 4], edge: [f32; 4], d: f32, inner_d: f32, soft: f32, bw: f32, opacity: f32, shrink: bool) -> [f32; 4] {
    let cov = clamp(0.5 - d / (soft * 2.0));
    let ring = (cov - clamp(0.5 - inner_d / (soft * 2.0))).max(0.0);
    let hide = clamp(edge[3] / opacity.clamp(1e-6, 1.0));
    let pull = if shrink && hide > 0.999 { bw * 0.5 } else { 0.0 };
    let under = clamp(0.5 - (d + pull) / (soft * 2.0));
    std::array::from_fn(|c| edge[c] * ring + fill[c] * (under * (1.0 - hide * ring)))
}

/// How much of the pixel at `p` the box covers: the exact area at a square corner, the distance
/// estimate at a rounded one (shader.wgsl `box_cov`).
fn box_cov(p: [f32; 2], h: [f32; 2], radii: [f32; 4]) -> f32 {
    let r = if p[0] > 0.0 {
        if p[1] > 0.0 { radii[2] } else { radii[1] }
    } else if p[1] > 0.0 {
        radii[3]
    } else {
        radii[0]
    };
    if r == 0.0 {
        let e = |a: usize| clamp(h[a].min(p[a] + 0.5) - (-h[a]).max(p[a] - 0.5));
        return e(0) * e(1);
    }
    clamp(0.5 - sd(p, h, radii))
}

pub(crate) fn fragment(i: &Primitive, atlas: &Atlas, screen: [f32; 2]) -> [f32; 4] {
    let kind = i.params[1] as u32;
    // An effect slot (kind 12) holds a compositor's place in the stream and covers no pixels.
    if kind == 12 {
        return [0.0; 4];
    }
    // A glyph's params[0] turns its quad clockwise about the rect centre. Undo the turn so
    // bounds and sampling run in the glyph's own frame; the clip stays on screen.
    let rotated = kind == 2 && i.params[0] != 0.0;
    let p = if rotated {
        let (s, c) = (sinf(i.params[0]), cosf(i.params[0]));
        let center = [i.rect[0] + i.rect[2] * 0.5, i.rect[1] + i.rect[3] * 0.5];
        let d = [screen[0] - center[0], screen[1] - center[1]];
        [center[0] + (c * d[0] + s * d[1]), center[1] + (c * d[1] - s * d[0])]
    } else {
        screen
    };
    let grow = match kind {
        1 => i.params[2] * 3.0 + 1.0,
        2 => 0.0,
        3 => 1.0,
        _ => 1.0 + i.params[2] * 2.0,
    };
    // Rasterization bounds matter for glyphs and very soft shadows. Pixel centers,
    // half-open right/bottom edges, matching the two axis-aligned GPU triangles.
    if p[0] < i.rect[0] - grow
        || p[1] < i.rect[1] - grow
        || p[0] >= i.rect[0] + i.rect[2] + grow
        || p[1] >= i.rect[1] + i.rect[3] + grow
    {
        return [0.0; 4];
    }
    let h = [i.rect[2] * 0.5, i.rect[3] * 0.5];
    let q = [p[0] - (i.rect[0] + h[0]), p[1] - (i.rect[1] + h[1])];
    let mut col = [0.0; 4];
    match kind {
        0 => {
            let d = sd(q, h, i.radii);
            let soft = i.params[2].max(0.5);
            let bw = i.params[0];
            if bw > 0.0 {
                // Per-side widths (top, right, bottom, left) in `uv`; a band window has uv[2] < 0.
                let w = i.uv;
                let sides = w[2] >= 0.0 && w != [0.0; 4];
                let inner_d = if sides {
                    let ih = [(h[0] - (w[1] + w[3]) * 0.5).max(0.0), (h[1] - (w[0] + w[2]) * 0.5).max(0.0)];
                    let at = [q[0] - (w[3] - w[1]) * 0.5, q[1] - (w[0] - w[2]) * 0.5];
                    let cut = [w[0].max(w[3]), w[0].max(w[1]), w[2].max(w[1]), w[2].max(w[3])];
                    sd(at, ih, std::array::from_fn(|n| (i.radii[n].abs() - cut[n]).max(0.0).copysign(i.radii[n])))
                } else {
                    d + bw
                };
                col = border_over(i.fill, i.border, d, inner_d, soft, bw, i.params[3], !sides && i.radii != [0.0; 4]);
            } else {
                col = i.fill.map(|c| c * clamp(0.5 - d / (soft * 2.0)));
            }
        }
        1 => {
            let mut cov = shadow(q, h, i.params[2].max(0.01), i.radii[0].abs().min(h[0].min(h[1])));
            if i.uv[2] > 0.0 {
                let hole = [i.uv[2] * 0.5, i.uv[3] * 0.5];
                let at = [p[0] - (i.rect[0] + i.uv[0] + hole[0]), p[1] - (i.rect[1] + i.uv[1] + hole[1])];
                cov *= clamp(0.5 + sd(at, hole, [i.radii[3]; 4]));
            }
            col = i.fill.map(|c| c * cov);
        }
        2 => {
            let cov = if i.uv[2] > 0.0 || rotated {
                let span = if i.uv[2] > 0.0 { [i.uv[2], i.uv[3]] } else { [i.rect[2], i.rect[3]] };
                let px = (p[0] - i.rect[0]) * span[0] / i.rect[2] - 0.5;
                let py = (p[1] - i.rect[1]) * span[1] / i.rect[3] - 0.5;
                let x = (i.uv[0] + floorf(px)) as i32;
                let y = (i.uv[1] + floorf(py)) as i32;
                let fx = px - floorf(px);
                let fy = py - floorf(py);
                mix(
                    mix(atlas.load(x, y), atlas.load(x + 1, y), fx),
                    mix(atlas.load(x, y + 1), atlas.load(x + 1, y + 1), fx),
                    fy,
                )
            } else {
                atlas.load(
                    (i.uv[0] + floorf(p[0] - i.rect[0])) as i32,
                    (i.uv[1] + floorf(p[1] - i.rect[1])) as i32,
                )
            };
            col = i.fill.map(|c| c * powf(cov, i.params[3]));
        }
        3 => {
            let cov = box_cov(q, h, i.radii);
            let spread = i.params[0];
            let ih = h.map(|v| (v - spread).max(0.0));
            let q = [q[0] - i.uv[0], q[1] - i.uv[1]];
            let lit = if i.params[2] < 0.3 {
                box_cov(q, ih, i.radii.map(|r| (r.abs() - spread).max(0.0).copysign(r)))
            } else {
                shadow(
                    q,
                    ih,
                    i.params[2],
                    (i.radii[0].abs() - spread).min(ih[0].min(ih[1])).max(0.0),
                )
            };
            col = i.fill.map(|c| c * cov * (1.0 - clamp(lit)));
        }
        4 => {
            let d = sd(q, h, i.radii);
            let soft = i.params[2].max(0.5);
            let bw = i.params[0];
            let t = clamp((p[1] - i.rect[1]) / i.rect[3].max(1.0));
            col = if bw > 0.0 {
                let edge = std::array::from_fn(|c| mix(i.border[c], i.uv[c], t));
                border_over(i.fill, edge, d, d + bw, soft, bw, i.params[3], i.radii != [0.0; 4])
            } else {
                let cov = clamp(0.5 - d / (soft * 2.0));
                std::array::from_fn(|c| mix(i.fill[c], i.uv[c], t) * cov)
            };
        }
        10 => {
            let d = sd(q, h, i.radii);
            let soft = i.params[2].max(0.5);
            let ring = clamp(0.5 - d / (soft * 2.0)) - clamp(0.5 - (d + i.params[0]) / (soft * 2.0));
            let t = clamp((p[1] - i.rect[1]) / i.rect[3].max(1.0));
            let local = [p[0] - i.rect[0], p[1] - i.rect[1]];
            let (s, total) = ring_arc(local, [i.rect[2], i.rect[3]], i.radii, i.params[0]);
            let cov = ring * dash_cov(s, total, [i.uv[0], i.uv[1], i.uv[2]]);
            col = std::array::from_fn(|c| mix(i.fill[c], i.border[c], t) * cov);
        }
        11 => {
            let d = sd(q, h, i.radii);
            let soft = i.params[2].max(0.5);
            let cov = clamp(0.5 - d / (soft * 2.0));
            let t0_raw = i.params[0];
            let t1_raw = i.params[3];
            let lower_open = t0_raw >= 1.5;
            let upper_open = t1_raw >= 1.5;
            let t0 = if lower_open { t0_raw - 2.0 } else { t0_raw };
            let t1 = if upper_open { t1_raw - 2.0 } else { t1_raw };
            let t = if i.uv[2] > 0.0 {
                let dx = (p[0] - i.rect[0] - i.uv[0]) / i.uv[2];
                let dy = (p[1] - i.rect[1] - i.uv[1]) / i.uv[3];
                sqrtf(dx * dx + dy * dy)
            } else {
                0.5 + (q[0] * i.uv[0] + q[1] * i.uv[1])
            };
            if !((lower_open && t < t0) || (upper_open && t >= t1)) {
                let u = if t1 <= t0 + 1e-6 {
                    if t >= t0 { 1.0 } else { 0.0 }
                } else {
                    clamp((t - t0) / (t1 - t0))
                };
                col = std::array::from_fn(|c| mix(i.fill[c], i.border[c], u) * cov);
            }
        }
        _ => panic!("unsupported primitive kind"),
    }
    let clip = clip_coverage(i, screen);
    col.map(|c| c * clip)
}

pub(crate) fn layer_clip_coverage(i: &Primitive, screen: [f32; 2]) -> f32 {
    let clip = clip_coverage(i, screen) * if i.params[3] == 0.0 { hole_alpha(screen, i.radii) } else { 1.0 };
    if i.border == [0.0; 4] { return clip; }
    let half = [(i.clip[2] - i.clip[0]) * 0.5, (i.clip[3] - i.clip[1]) * 0.5];
    let p = [screen[0] - i.clip[0] - half[0], screen[1] - i.clip[1] - half[1]];
    clip * clamp(0.5 - sd(p, half, i.border))
}

/// A layer's round hole (`El::cutout_circle`): centre `h[0..2]`, clear within `h[2]`, whole from
/// `h[3]`, linear between and at least a pixel wide; none when `h[3]` is zero.
fn hole_alpha(p: [f32; 2], h: [f32; 4]) -> f32 {
    if h[3] <= 0.0 { return 1.0; }
    let (dx, dy) = (p[0] - h[0], p[1] - h[1]);
    clamp((sqrtf(dx * dx + dy * dy) - (h[2] + h[3]) * 0.5) / (h[3] - h[2]).max(1.0) + 0.5)
}

fn clip_coverage(i: &Primitive, screen: [f32; 2]) -> f32 {
    let origin = i.fade_origin.unwrap_or(i.clip);
    let p = screen;
    let hard = [p[0] - i.clip[0], p[1] - i.clip[1], i.clip[2] - p[0], i.clip[3] - p[1]];
    (0..4).map(|edge| {
        let width = half::f16::from_f32(i.fade[edge].clamp(-65504.0, 65504.0)).to_f32();
        let distance = if edge < 2 { i.clip[edge] - origin[edge] } else { origin[edge] - i.clip[edge] };
        let w = i.fade[edge].abs();
        let offset = if w > 0.0 { (distance / w).clamp(0.0, 1.0) } else { 0.0 };
        let offset = libm::floorf(offset * 255.0 + 0.5) / 255.0;
        if offset >= 1.0 { ramp(hard[edge], 0.0) }
        else if offset > 0.0 { ramp(hard[edge], 0.0).min(ramp(hard[edge] + offset * width.abs(), width)) }
        else { ramp(hard[edge], width) }
    }).product()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_negative_fade_ramps_linearly_and_a_positive_one_eases() {
        for (d, linear) in [(0.0, 0.0), (4.0, 0.25), (8.0, 0.5), (12.0, 0.75), (16.0, 1.0), (20.0, 1.0)] {
            assert!((ramp(d, -16.0) - linear).abs() < 1e-6, "linear at {d}: {}", ramp(d, -16.0));
        }
        assert!((ramp(4.0, 16.0) - 0.15625).abs() < 1e-6, "smoothstep a quarter in");
        assert_eq!(ramp(8.0, 16.0), ramp(8.0, -16.0), "both are half way at the middle");
    }

    // Walking the ring's centre line clockwise, arc length grows by the distance walked and
    // comes back round to zero after exactly the line's length.
    #[test]
    fn ring_arc_walks_the_centre_line() {
        for (size, radii, bw) in [
            ([40.0f32, 20.0], [10.0f32; 4], 2.0f32),
            ([30.0, 30.0], [0.0, 6.0, 15.0, 3.0], 1.0),
            ([50.0, 12.0], [6.0; 4], 0.5),
            ([24.0, 18.0], [0.0; 4], 1.5),
        ] {
            let h = bw * 0.5;
            let rc = radii.map(|r: f32| (r.min(size[0].min(size[1]) * 0.5) - h).max(0.0));
            let (_, total) = ring_arc([size[0] * 0.5, h], size, radii, bw);
            let expected = 2.0 * (size[0] + size[1] - 2.0 * bw)
                - (2.0 - std::f32::consts::FRAC_PI_2) * rc.iter().sum::<f32>();
            assert!((total - expected).abs() < 1e-3, "{total} vs {expected}");
            // Sample the centre line by marching a point round the inset rounded rect.
            let mut points = Vec::new();
            let steps = 400;
            for n in 0..steps {
                let angle = n as f32 / steps as f32 * std::f32::consts::TAU;
                let dir = [libm::sinf(angle), -libm::cosf(angle)];
                // Ray from the centre to the centre line: bisect on the ring's centre distance.
                let (mut lo, mut hi) = (0.0, size[0] + size[1]);
                for _ in 0..60 {
                    let mid = (lo + hi) * 0.5;
                    let q = [dir[0] * mid, dir[1] * mid];
                    if sd(q, [size[0] * 0.5, size[1] * 0.5], radii) + h < 0.0 { lo = mid } else { hi = mid }
                }
                points.push([size[0] * 0.5 + dir[0] * lo, size[1] * 0.5 + dir[1] * lo]);
            }
            // Each step advances s by the chord, or up to a corner's detour where the line turns
            // sharply, and the steps add up to one trip round.
            let mut trip = 0.0;
            for n in 1..=steps {
                let (a, b) = (points[n - 1], points[n % steps]);
                let chord = sqrtf((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2));
                let ds = (ring_arc(b, size, radii, bw).0 - ring_arc(a, size, radii, bw).0).rem_euclid(total);
                assert!(ds > chord - 1e-3 && ds < chord * 1.5 + 1e-3, "{size:?} {radii:?} step {n}: {ds} vs {chord}");
                trip += ds;
            }
            assert!((trip - total).abs() < 1e-2, "{trip} vs {total}");
        }
    }

    #[test]
    fn dash_pattern_closes_at_the_seam() {
        for total in [37.3, 100.0, 251.9, 3.0] {
            for d in [[4.0, 3.0, 0.0], [6.0, 2.5, 1.7], [10.0, 10.0, -4.0]] {
                for n in 0..50 {
                    let s = n as f32 * 0.37;
                    assert!((dash_cov(s, total, d) - dash_cov(s + total, total, d)).abs() < 1e-3);
                    assert!((dash_cov(s, total, d) - dash_cov(s - total, total, d)).abs() < 1e-3);
                }
            }
        }
        // With no phase the pattern starts on a dash: half covered at s = 0, full just after.
        assert!((dash_cov(0.0, 100.0, [4.0, 3.0, 0.0]) - 0.5).abs() < 1e-4);
        assert_eq!(dash_cov(1.0, 100.0, [4.0, 3.0, 0.0]), 1.0);
        // One nominal period of phase is one whole stretched period.
        for n in 0..50 {
            let s = n as f32 * 1.3;
            assert!((dash_cov(s, 100.0, [4.0, 3.0, 7.0]) - dash_cov(s, 100.0, [4.0, 3.0, 0.0])).abs() < 1e-3);
        }
    }
}
