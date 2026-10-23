//! Deliberately scalar transcription of shader.wgsl. No shared shader generator:
//! keeping the implementations independent catches changes to either lane.
use crate::{Atlas, Primitive};
use libm::{expf, floorf, powf, sqrtf};

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
    let r = radii[index].min(h[0].min(h[1]));
    let q = [p[0].abs() - h[0] + r, p[1].abs() - h[1] + r];
    let a = q[0].max(0.0);
    let b = q[1].max(0.0);
    q[0].max(q[1]).min(0.0) + sqrtf(a * a + b * b) - r
}
fn ramp(d: f32, fade: f32) -> f32 {
    if fade <= 0.0 {
        return clamp(d + 0.5);
    }
    let x = clamp(d / fade);
    x * x * (3.0 - 2.0 * x)
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

pub(crate) fn fragment(i: &Primitive, atlas: &Atlas, p: [f32; 2]) -> [f32; 4] {
    let kind = i.params[1] as u32;
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
            let cov = clamp(0.5 - d / (soft * 2.0));
            let inner = clamp(0.5 - (d + i.params[0]) / (soft * 2.0));
            for (c, out) in col.iter_mut().enumerate() {
                *out = if i.params[0] > 0.0 {
                    i.fill[c] * inner + i.border[c] * (cov - inner)
                } else {
                    i.fill[c] * cov
                };
            }
        }
        1 => {
            let cov = shadow(q, h, i.params[2].max(0.01), i.radii[0].min(h[0].min(h[1])));
            col = i.fill.map(|c| c * cov);
        }
        2 => {
            let cov = if i.uv[2] > 0.0 {
                let px = (p[0] - i.rect[0]) * i.uv[2] / i.rect[2] - 0.5;
                let py = (p[1] - i.rect[1]) * i.uv[3] / i.rect[3] - 0.5;
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
            let cov = clamp(0.5 - sd(q, h, i.radii));
            let spread = i.params[0];
            let ih = h.map(|v| (v - spread).max(0.0));
            let q = [q[0] - i.uv[0], q[1] - i.uv[1]];
            let lit = if i.params[2] < 0.3 {
                clamp(0.5 - sd(q, ih, i.radii.map(|r| (r - spread).max(0.0))))
            } else {
                shadow(
                    q,
                    ih,
                    i.params[2],
                    (i.radii[0] - spread).min(ih[0].min(ih[1])).max(0.0),
                )
            };
            col = i.fill.map(|c| c * cov * (1.0 - clamp(lit)));
        }
        4 => {
            let cov = clamp(0.5 - sd(q, h, i.radii));
            let t = clamp((p[1] - i.rect[1]) / i.rect[3].max(1.0));
            col = std::array::from_fn(|c| mix(i.fill[c], i.uv[c], t) * cov);
        }
        _ => panic!("unsupported primitive kind"),
    }
    let origin = i.fade_origin.unwrap_or(i.clip);
    let hard = [p[0] - i.clip[0], p[1] - i.clip[1], i.clip[2] - p[0], i.clip[3] - p[1]];
    let clip: f32 = (0..4).map(|edge| {
        let width = half::f16::from_f32(i.fade[edge].clamp(0.0, 65504.0)).to_f32();
        let distance = if edge < 2 { i.clip[edge] - origin[edge] } else { origin[edge] - i.clip[edge] };
        let offset = if i.fade[edge] > 0.0 { (distance / i.fade[edge]).clamp(0.0, 1.0) } else { 0.0 };
        let offset = libm::floorf(offset * 255.0 + 0.5) / 255.0;
        if offset >= 1.0 { ramp(hard[edge], 0.0) }
        else if offset > 0.0 { ramp(hard[edge], 0.0).min(ramp(hard[edge] + offset * width, width)) }
        else { ramp(hard[edge], width) }
    }).product();
    col.map(|c| c * clip)
}
