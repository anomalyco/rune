//! Ganesh's `FillRRectOp` (`src/gpu/ganesh/ops/FillRRectOp.cpp`), its vertex shader step for
//! step: an inset octagon at full coverage, linear ramps along the straight edges and, past each
//! corner's chord, the arc's `x² + y² - 1` over its `fwidth`. Corners under 0.75 px become sharp,
//! and the rest are drawn at least 1.5 px round. `fwidth` is a difference across the pixel's 2×2
//! quad, which is where the window's pixel grid comes in: `parity` is the canvas's origin, odd
//! or even, on it.

use super::raster::{gradient, rasterize};

/// One of the op's 40 vertices (`kVertexData`): its corner (top-left, top-right, bottom-right,
/// bottom-left), where on that corner it sits, how it bloats for antialiasing, its coverage and
/// whether that coverage is linear.
struct Vertex {
    corner: [f32; 2],
    radius_outset: [f32; 2],
    bloat: [f32; 2],
    coverage: f32,
    linear: bool,
}

const fn v(corner: [f32; 2], radius_outset: [f32; 2], bloat: [f32; 2], coverage: f32, linear: bool) -> Vertex {
    Vertex { corner, radius_outset, bloat, coverage, linear }
}

const OCTO: f32 = 1.0 / (1.0 + std::f32::consts::FRAC_1_SQRT_2);
const TL: [f32; 2] = [-1.0, -1.0];
const TR: [f32; 2] = [1.0, -1.0];
const BR: [f32; 2] = [1.0, 1.0];
const BL: [f32; 2] = [-1.0, 1.0];

const VERTICES: [Vertex; 40] = [
    // Inset edges.
    v(BL, [0.0, -1.0], [1.0, 0.0], 1.0, true),
    v(TL, [0.0, 1.0], [1.0, 0.0], 1.0, true),
    v(TL, [1.0, 0.0], [0.0, 1.0], 1.0, true),
    v(TR, [-1.0, 0.0], [0.0, 1.0], 1.0, true),
    v(TR, [0.0, 1.0], [-1.0, 0.0], 1.0, true),
    v(BR, [0.0, -1.0], [-1.0, 0.0], 1.0, true),
    v(BR, [-1.0, 0.0], [0.0, -1.0], 1.0, true),
    v(BL, [1.0, 0.0], [0.0, -1.0], 1.0, true),
    // Outset edges.
    v(BL, [0.0, -1.0], [-1.0, 0.0], 0.0, true),
    v(TL, [0.0, 1.0], [-1.0, 0.0], 0.0, true),
    v(TL, [1.0, 0.0], [0.0, -1.0], 0.0, true),
    v(TR, [-1.0, 0.0], [0.0, -1.0], 0.0, true),
    v(TR, [0.0, 1.0], [1.0, 0.0], 0.0, true),
    v(BR, [0.0, -1.0], [1.0, 0.0], 0.0, true),
    v(BR, [-1.0, 0.0], [0.0, 1.0], 0.0, true),
    v(BL, [1.0, 0.0], [0.0, 1.0], 0.0, true),
    // Top-left arc.
    v(TL, [0.0, 1.0], [-1.0, 0.0], 0.0, false),
    v(TL, [0.0, 1.0], [1.0, 0.0], 1.0, false),
    v(TL, [1.0, 0.0], [0.0, 1.0], 1.0, false),
    v(TL, [1.0, 0.0], [0.0, -1.0], 0.0, false),
    v(TL, [OCTO, 0.0], [-1.0, -1.0], 0.0, false),
    v(TL, [0.0, OCTO], [-1.0, -1.0], 0.0, false),
    // Top-right arc.
    v(TR, [-1.0, 0.0], [0.0, -1.0], 0.0, false),
    v(TR, [-1.0, 0.0], [0.0, 1.0], 1.0, false),
    v(TR, [0.0, 1.0], [-1.0, 0.0], 1.0, false),
    v(TR, [0.0, 1.0], [1.0, 0.0], 0.0, false),
    v(TR, [0.0, OCTO], [1.0, -1.0], 0.0, false),
    v(TR, [-OCTO, 0.0], [1.0, -1.0], 0.0, false),
    // Bottom-right arc.
    v(BR, [0.0, -1.0], [1.0, 0.0], 0.0, false),
    v(BR, [0.0, -1.0], [-1.0, 0.0], 1.0, false),
    v(BR, [-1.0, 0.0], [0.0, -1.0], 1.0, false),
    v(BR, [-1.0, 0.0], [0.0, 1.0], 0.0, false),
    v(BR, [-OCTO, 0.0], [1.0, 1.0], 0.0, false),
    v(BR, [0.0, -OCTO], [1.0, 1.0], 0.0, false),
    // Bottom-left arc.
    v(BL, [1.0, 0.0], [0.0, 1.0], 0.0, false),
    v(BL, [1.0, 0.0], [0.0, -1.0], 1.0, false),
    v(BL, [0.0, -1.0], [1.0, 0.0], 1.0, false),
    v(BL, [0.0, -1.0], [-1.0, 0.0], 0.0, false),
    v(BL, [0.0, -OCTO], [-1.0, 1.0], 0.0, false),
    v(BL, [OCTO, 0.0], [-1.0, 1.0], 0.0, false),
];

const INDICES: [usize; 90] = [
    // Inset octagon.
    0, 1, 7, 1, 2, 7, 7, 2, 6, 2, 3, 6, 6, 3, 5, 3, 4, 5,
    // Edge ramps.
    0, 1, 8, 1, 9, 8, 2, 3, 10, 3, 11, 10, 4, 5, 12, 5, 13, 12, 6, 7, 14, 7, 15, 14,
    // Arcs.
    16, 17, 21, 17, 21, 18, 21, 18, 20, 18, 20, 19,
    22, 23, 27, 23, 27, 24, 27, 24, 26, 24, 26, 25,
    28, 29, 33, 29, 33, 30, 33, 30, 32, 30, 32, 31,
    34, 35, 39, 35, 39, 36, 39, 36, 38, 36, 38, 37,
];

/// Covers `out` (`px` × `px`, cleared first) with the round rect `[lo, hi]` (canvas device
/// pixels) whose corners are `r` device pixels round, filled antialiased by Ganesh's
/// `CircularRRectOp`: a nine-patch half a pixel wider, whose corners interpolate each pixel's
/// offset from their circle's centre, normalized to the radius half a pixel wider, covered by
/// how far inside that circle it is.
pub(in super::super) fn fill_circular(lo: [f32; 2], hi: [f32; 2], r: f32, px: u32, out: &mut [u8]) {
    let outer = r + 0.5;
    let (l, t, rr, b) = (lo[0] - 0.5, lo[1] - 0.5, hi[0] + 0.5, hi[1] + 0.5);
    let xs = [l, l + outer, rr - outer, rr];
    let ys = [t, t + outer, b - outer, b];
    const OFFSETS: [f32; 4] = [-1.0, 0.0, 0.0, 1.0];
    let mut pts = Vec::with_capacity(16);
    let mut offsets = Vec::with_capacity(16);
    for (j, &y) in ys.iter().enumerate() {
        for (i, &x) in xs.iter().enumerate() {
            pts.push([x, y]);
            offsets.push([OFFSETS[i], OFFSETS[j]]);
        }
    }
    let mut indices = Vec::with_capacity(54);
    for j in 0..3 {
        for i in 0..3 {
            let k = j * 4 + i;
            indices.extend([k, k + 1, k + 5, k, k + 5, k + 4]);
        }
    }
    rasterize(&pts, &indices, [0.0, 0.0], px, out, |tri, w, _| {
        let at = |c: usize| (0..3).map(|i| w[i] as f32 * offsets[tri[i]][c]).sum::<f32>();
        (outer * (1.0 - at(0).hypot(at(1)))).clamp(0.0, 1.0)
    });
}

fn sign(v: f32) -> f32 {
    if v > 0.0 { 1.0 } else if v < 0.0 { -1.0 } else { 0.0 }
}

/// Covers `out` (`px` × `px`, cleared first) with the round rect `[lo, hi]` (canvas device
/// pixels) whose corners are `r` device pixels round, filled antialiased.
pub(in super::super) fn fill_rrect(lo: [f32; 2], hi: [f32; 2], r: f32, parity: [u32; 2], px: u32, out: &mut [u8]) {
    let half = [(hi[0] - lo[0]) * 0.5, (hi[1] - lo[1]) * 0.5];
    let center = [lo[0] + half[0], lo[1] + half[1]];
    let pixel = [1.0 / half[0], 1.0 / half[1]];
    let bloat_radius = [pixel[0] * 0.5, pixel[1] * 0.5];
    let given = [r / half[0], r / half[1]];
    let mut pts = Vec::with_capacity(VERTICES.len());
    // Per vertex: whether its coverage is linear, and its two varyings.
    let mut vary = Vec::with_capacity(VERTICES.len());
    for vx in &VERTICES {
        let mut corner = vx.corner;
        let mut radii = given;
        let mut neighbor = given;
        let mut multiplier = 1.0;
        if bloat_radius[0] > 1.0 || bloat_radius[1] > 1.0 {
            // Narrower than a ramp: drawn a ramp wide, at less coverage.
            corner = [corner[0].abs().max(bloat_radius[0]) * sign(corner[0]), corner[1].abs().max(bloat_radius[1]) * sign(corner[1])];
            multiplier = 1.0 / (bloat_radius[0].max(1.0) * bloat_radius[1].max(1.0));
            radii = [0.0, 0.0];
        }
        let (mut bloat, mut linear) = (vx.bloat, vx.linear);
        if radii[0] < bloat_radius[0] * 1.5 || radii[1] < bloat_radius[1] * 1.5 {
            // A corner too small to round: a sharp one, framed like a rect's.
            radii = [0.0, 0.0];
            bloat = [sign(corner[0]), sign(corner[1])];
            if vx.coverage > 0.5 {
                bloat = [-bloat[0], -bloat[1]];
            }
            linear = true;
        } else {
            for k in 0..2 {
                radii[k] = radii[k].clamp(pixel[k] * 1.5, 2.0 - pixel[k] * 1.5);
                neighbor[k] = neighbor[k].clamp(pixel[k] * 1.5, 2.0 - pixel[k] * 1.5);
                let spacing = 2.0 - radii[k] - neighbor[k];
                radii[k] -= (pixel[k] * 0.0625 - spacing).max(0.0) * 0.5;
            }
        }
        let outset = [bloat[0] * bloat_radius[0], bloat[1] * bloat_radius[1]];
        let mut pos = [corner[0] + vx.radius_outset[0] * radii[0] + outset[0], corner[1] + vx.radius_outset[1] * radii[1] + outset[1]];
        let mut coverage = vx.coverage;
        if coverage > 0.5 {
            // Inset ramps don't cross the centre.
            if bloat[0] != 0.0 && pos[0] * corner[0] < 0.0 {
                let back = pos[0].abs();
                pos[0] = 0.0;
                pos[1] += back * sign(corner[1]) * pixel[1] / pixel[0];
                coverage = (coverage - 0.5) * corner[0].abs() / (corner[0].abs() + back) + 0.5;
            }
            if bloat[1] != 0.0 && pos[1] * corner[1] < 0.0 {
                let back = pos[1].abs();
                pos[1] = 0.0;
                pos[0] += back * sign(corner[0]) * pixel[0] / pixel[1];
                coverage = (coverage - 0.5) * corner[1].abs() / (corner[1].abs() + back) + 0.5;
            }
        }
        pts.push([center[0] + pos[0] * half[0], center[1] + pos[1] * half[1]]);
        vary.push(if linear {
            (true, [0.0, coverage * multiplier])
        } else {
            let arc = [
                1.0 - vx.radius_outset[0].abs() + outset[0] / radii[0] * corner[0],
                1.0 - vx.radius_outset[1].abs() + outset[1] / radii[1] * corner[1],
            ];
            (false, [arc[0] + 1.0, arc[1]])
        });
    }
    rasterize(&pts, &INDICES, [0.0, 0.0], px, out, |tri, w, at| {
        let lerp = |k: usize| (0..3).map(|i| w[i] * vary[tri[i]].1[k] as f64).sum::<f64>();
        if vary[tri[0]].0 {
            return lerp(1) as f32;
        }
        let (x1, y) = (lerp(0), lerp(1));
        let gx = gradient(&pts, tri, |i| vary[i].1[0], [0.0, 0.0]).map(f64::from);
        let gy = gradient(&pts, tri, |i| vary[i].1[1], [0.0, 0.0]).map(f64::from);
        let f = |dx: f64, dy: f64| {
            let (a, b) = (x1 + gx[0] * dx + gx[1] * dy, y + gy[0] * dx + gy[1] * dy);
            a * (a - 2.0) + b * b
        };
        let f0 = f(0.0, 0.0);
        // The quad's other pixel in each direction: right of an even one, left of an odd one.
        let step = |c: i64, odd: u32| if (c + i64::from(odd)).rem_euclid(2) == 0 { 1.0 } else { -1.0 };
        let width = (f(step(at[0], parity[0]), 0.0) - f0).abs() + (f(0.0, step(at[1], parity[1])) - f0).abs();
        if width == 0.0 {
            return if f0 < 0.0 { 1.0 } else { 0.0 };
        }
        (0.5 - f0 / width) as f32
    });
}
