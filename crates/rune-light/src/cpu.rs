//! CPU references for the soft-shadow pass.
//!
//! - [`estimate`]: the shader's penumbra estimator over the *exact* analytic field (no half-res texture), so a GPU
//!   texel can be checked against the same maths without quantisation.
//! - [`ground_truth`]: brute force. Many rays from the receiver to points spread over the spherical source's disc,
//!   each sphere traced against the exact 3D distance to every slab; visibility is the unblocked share.

use crate::{FLAG_OCCLUDER, Light, Prim};

const CONTINUOUS_K: f32 = 1.528_664_8;

fn sd_continuous(p: [f32; 2], half: [f32; 2], r: f32) -> f32 {
    let e = CONTINUOUS_K * r;
    let q = [half[0] - p[0].abs(), half[1] - p[1].abs()];
    let v = [e - q[0], e - q[1]];
    if v[0] <= 0.0 || v[1] <= 0.0 {
        return (-q[0]).max(-q[1]);
    }
    let u = v[1].atan2(v[0]) * (4.0 / std::f32::consts::PI) - 1.0;
    let u2 = u * u;
    (v[0] * v[0] + v[1] * v[1]).sqrt() - e * (1.146_022_95 + u2 * (-0.375_731_61 + u2 * (0.307_966_09 - 0.077_243_54 * u2)))
}

fn sd_rrect(p: [f32; 2], half: [f32; 2], radii: [f32; 4]) -> f32 {
    let r = if p[0] > 0.0 {
        if p[1] > 0.0 { radii[2] } else { radii[1] }
    } else if p[1] > 0.0 {
        radii[3]
    } else {
        radii[0]
    };
    let m = half[0].min(half[1]);
    if r < 0.0 {
        return sd_continuous(p, half, (-r).min(m));
    }
    let r = r.min(m);
    let q = [p[0].abs() - half[0] + r, p[1].abs() - half[1] + r];
    q[0].max(q[1]).min(0.0) + (q[0].max(0.0).hypot(q[1].max(0.0))) - r
}

fn prim_sd(pr: &Prim, p: [f32; 2]) -> f32 {
    let half = [pr.rect[2] * 0.5, pr.rect[3] * 0.5];
    sd_rrect([p[0] - pr.rect[0] - half[0], p[1] - pr.rect[1] - half[1]], half, pr.radii)
}

/// The field texel the shader builds: union 2D distance and the nearest occluder's z range.
pub fn field(prims: &[Prim], p: [f32; 2]) -> [f32; 3] {
    let mut best = [1e4, 0.0, 0.0];
    for pr in prims.iter().filter(|p| p.flags() & FLAG_OCCLUDER != 0) {
        let d = prim_sd(pr, p);
        if d < best[0] {
            best = [d, pr.z[0], pr.z[1]];
        }
    }
    best
}

fn slab(f: [f32; 3], z: f32) -> f32 {
    let dz = (z - f[2]).max(f[1] - z);
    let a = [f[0], dz];
    a[0].max(0.0).hypot(a[1].max(0.0)) + a[0].max(a[1]).min(0.0)
}

/// Exact 3D distance to the union of slabs.
pub fn scene_sd(prims: &[Prim], q: [f32; 3]) -> f32 {
    prims
        .iter()
        .filter(|p| p.flags() & FLAG_OCCLUDER != 0)
        .map(|pr| slab([prim_sd(pr, [q[0], q[1]]), pr.z[0], pr.z[1]], q[2]))
        .fold(1e4, f32::min)
}

fn smooth(res: f32) -> f32 {
    let k = (0.5 + 0.5 * res).clamp(0.0, 1.0);
    k * k * (3.0 - 2.0 * k)
}

/// The shader's estimator (shadow.wgsl `trace`) with the field evaluated exactly.
pub fn estimate(prims: &[Prim], light: &Light, p: [f32; 3]) -> f32 {
    let l = [light.pos[0], light.pos[1], light.pos[2]];
    let radius = light.pos[3] * light.extra[3];
    let d = [l[0] - p[0], l[1] - p[1], l[2] - p[2]];
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let rd = [d[0] / dist, d[1] / dist, d[2] / dist];
    let theta = (radius / dist).max(1e-3);
    let zmax = prims.iter().filter(|p| p.flags() & FLAG_OCCLUDER != 0).fold(0.0f32, |m, p| m.max(p.z[1])) + 0.5;
    let (mut t, mut res, mut ph) = (0.75f32, 1.0f32, 1e10f32);
    for _ in 0..64 {
        if t >= dist - radius {
            break;
        }
        let q = [p[0] + rd[0] * t, p[1] + rd[1] * t, p[2] + rd[2] * t];
        if rd[2] >= 0.0 && q[2] > zmax {
            break;
        }
        let h = slab(field(prims, [q[0], q[1]]), q[2]);
        if h <= 0.0 {
            res = res.min(h / (t * theta));
        } else {
            let y = h * h / (2.0 * ph);
            let dd = (h * h - y * y).max(0.0).sqrt();
            res = res.min(dd / ((t - y).max(1e-3) * theta));
        }
        ph = h.max(1e-3);
        if res <= -1.0 {
            break;
        }
        t += h.abs().clamp(0.6 + t * 0.012, 96.0);
    }
    smooth(res)
}

fn blocked(prims: &[Prim], p: [f32; 3], target: [f32; 3]) -> bool {
    let d = [target[0] - p[0], target[1] - p[1], target[2] - p[2]];
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let rd = [d[0] / dist, d[1] / dist, d[2] / dist];
    let mut t = 0.05;
    while t < dist {
        let q = [p[0] + rd[0] * t, p[1] + rd[1] * t, p[2] + rd[2] * t];
        let h = scene_sd(prims, q);
        if h < 0.01 {
            return true;
        }
        t += h.max(0.02);
    }
    false
}

/// Unblocked share of the source's disc (as seen from p), `n`×`n` stratified samples.
pub fn ground_truth(prims: &[Prim], light: &Light, p: [f32; 3], n: u32) -> f32 {
    let c = [light.pos[0], light.pos[1], light.pos[2]];
    let r = light.pos[3] * light.extra[3];
    let w = [c[0] - p[0], c[1] - p[1], c[2] - p[2]];
    let wl = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
    let w = [w[0] / wl, w[1] / wl, w[2] / wl];
    let up = if w[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let u = cross(w, up);
    let ul = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
    let u = [u[0] / ul, u[1] / ul, u[2] / ul];
    let v = cross(w, u);
    let (mut lit, mut total) = (0u32, 0u32);
    for i in 0..n {
        for j in 0..n {
            let a = [(i as f32 + 0.5) / n as f32 * 2.0 - 1.0, (j as f32 + 0.5) / n as f32 * 2.0 - 1.0];
            if a[0] * a[0] + a[1] * a[1] > 1.0 {
                continue;
            }
            total += 1;
            let tgt = [
                c[0] + (u[0] * a[0] + v[0] * a[1]) * r,
                c[1] + (u[1] * a[0] + v[1] * a[1]) * r,
                c[2] + (u[2] * a[0] + v[2] * a[1]) * r,
            ];
            if !blocked(prims, p, tgt) {
                lit += 1;
            }
        }
    }
    lit as f32 / total.max(1) as f32
}
