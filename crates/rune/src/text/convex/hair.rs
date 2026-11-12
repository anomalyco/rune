//! Ganesh's hairlines (`AAHairLinePathRenderer.cpp`), ported step for step: Chromium's GPU raster
//! draws a stroke a device pixel wide or less this way, at the width's share of full coverage.
//! Each line segment is a hexagon whose coverage falls from the line to nothing a pixel either
//! side (and over a pixel across each end); each quad (a cubic is first cut into quads) is a
//! pentagon round its control triangle, shaded by the quad's implicit distance as a one pixel
//! tent. Lines draw first, then curves, each blended over.

use super::El;
use super::fill::{chop_at_inflections, quad_uv_matrix};
use super::geom::V;
use super::raster::{gradient, rasterize};
use swash::zeno::Transform;

/// Draws `path` (user units) under `ctm` stroked `width` device pixels wide, at most one, into
/// `out` (`px` × `px`, cleared first); `ctm` places it `origin` device pixels off the canvas.
pub(in super::super) fn render(path: &[El], ctm: Transform, width: f32, origin: [f32; 2], px: u32, out: &mut [u8]) {
    // `newCoverage`: the width's share of full, to a byte.
    let coverage = (width.clamp(0.0, 1.0) * 255.0).round() / 255.0;
    let (lines, quads) = gather(path, ctm);
    // Every vertex: its position, and either a line's coverage or a quad's (u, v).
    let mut pts: Vec<[f32; 2]> = Vec::new();
    let mut attr: Vec<(bool, [f32; 2])> = Vec::new();
    let mut indices: Vec<usize> = Vec::new();
    for l in lines.as_chunks::<2>().0 {
        let Some(hex) = line(l[0], l[1], coverage) else { continue };
        let base = pts.len();
        pts.extend(hex.iter().map(|(p, _)| [p.x, p.y]));
        attr.extend(hex.iter().map(|&(_, c)| (false, [c, 0.0])));
        indices.extend([0, 1, 3, 0, 3, 2, 0, 4, 5, 0, 5, 1, 0, 2, 4, 1, 5, 3].map(|i| base + i));
    }
    for q in quads.as_chunks::<3>().0 {
        let Some(pent) = bloat_quad([q[0], q[1], q[2]]) else { continue };
        let m = quad_uv_matrix([q[0], q[1], q[2]]);
        let base = pts.len();
        pts.extend(pent.iter().map(|p| [p.x, p.y]));
        attr.extend(pent.iter().map(|p| (true, [m[0] * p.x + m[1] * p.y + m[2], m[3] * p.x + m[4] * p.y + m[5]])));
        indices.extend([0, 1, 2, 2, 4, 3, 1, 4, 2].map(|i| base + i));
    }
    rasterize(&pts, &indices, origin, px, out, |tri, w| {
        let at = |k: usize| (0..3).map(|i| w[i] * attr[tri[i]].1[k] as f64).sum::<f64>() as f32;
        if !attr[tri[0]].0 {
            return at(0);
        }
        // `GrQuadEffect`, hairline: one less the implicit distance, times the coverage.
        let (u, v) = (at(0), at(1));
        let (du, dv) = (gradient(&pts, tri, |i| attr[i].1[0], origin), gradient(&pts, tri, |i| attr[i].1[1], origin));
        let g = [2.0 * u * du[0] - dv[0], 2.0 * u * du[1] - dv[1]];
        let f = u * u - v;
        let gg = g[0] * g[0] + g[1] * g[1];
        if gg == 0.0 {
            return 0.0;
        }
        (1.0 - (f * f / gg).sqrt()).max(0.0) * coverage
    });
}

/// `gather_lines_and_quads`: line segments (pairs) and quads (triples), in device pixels.
fn gather(path: &[El], ctm: Transform) -> (Vec<V>, Vec<V>) {
    let map = |p: V| super::map(ctm, p);
    let (mut lines, mut quads) = (Vec::new(), Vec::new());
    let add_quad = |q: [V; 3], lines: &mut Vec<V>, quads: &mut Vec<V>| {
        if degenerate(q) {
            lines.extend([q[0], q[1], q[1], q[2]]);
        } else {
            quads.extend(q);
        }
    };
    let (mut start, mut cur) = (V::default(), V::default());
    for el in path {
        match *el {
            El::Move(p) => (start, cur) = (p, p),
            El::Line(p) => {
                lines.extend([map(cur), map(p)]);
                cur = p;
            }
            El::Quad(c, p) => {
                for q in chop_at_max_curvature([cur, c, p]) {
                    add_quad(q.map(map), &mut lines, &mut quads);
                }
                cur = p;
            }
            El::Conic(c, p, w) => {
                // Drawn as quads (Ganesh's conic effect isn't ported).
                let mut last = map(cur);
                let qs = super::conic_quads([map(cur), map(c), map(p)], w);
                for pair in qs.as_chunks::<2>().0 {
                    add_quad([last, pair[0], pair[1]], &mut lines, &mut quads);
                    last = pair[1];
                }
                cur = p;
            }
            El::Cubic(c1, c2, p) => {
                let d = [map(cur), map(c1), map(c2), map(p)];
                for c in chop_at_inflections(d) {
                    let mut q = Vec::new();
                    cubic_to_quads(c, 1.0, &mut q, 0, true, true);
                    for t in q.as_chunks::<3>().0 {
                        add_quad([t[0], t[1], t[2]], &mut lines, &mut quads);
                    }
                }
                cur = p;
            }
            El::Close => {
                if cur != start {
                    lines.extend([map(cur), map(start)]);
                }
                cur = start;
            }
        }
    }
    (lines, quads)
}

/// `is_degen_quad_or_conic` at Ganesh's default tolerance, a quarter pixel.
fn degenerate(p: [V; 3]) -> bool {
    let tol = 0.25f32 * 0.25;
    p[0].distance_sqd(p[1]) < tol || p[1].distance_sqd(p[2]) < tol || line_sqd(p[1], p[0], p[2]) < tol || line_sqd(p[2], p[1], p[0]) < tol
}

/// `SkPointPriv::DistanceToLineBetweenSqd`.
fn line_sqd(pt: V, a: V, b: V) -> f32 {
    let (u, v) = (b - a, pt - a);
    let det = u.cross(v);
    let t = det / u.dot(u) * det;
    if t.is_finite() { t } else { v.dot(v) }
}

/// `SkChopQuadAtMaxCurvature`.
fn chop_at_max_curvature(p: [V; 3]) -> Vec<[V; 3]> {
    let (a, b) = (p[1] - p[0], V::new(p[0].x - p[1].x - p[1].x + p[2].x, p[0].y - p[1].y - p[1].y + p[2].y));
    let (mut numer, mut denom) = (-(a.x * b.x + a.y * b.y), b.x * b.x + b.y * b.y);
    if denom < 0.0 {
        (numer, denom) = (-numer, -denom);
    }
    let t = if numer <= 0.0 { 0.0 } else if numer >= denom { 1.0 } else { numer / denom };
    if t > 0.0 && t < 1.0 {
        let mix = |a: V, b: V| (b - a) * t + a;
        let (ab, bc) = (mix(p[0], p[1]), mix(p[1], p[2]));
        let abc = mix(ab, bc);
        return vec![[p[0], ab, abc], [abc, bc, p[2]]];
    }
    vec![p]
}

/// `convert_noninflect_cubic_to_quads`, its tangents kept at the ends asked for.
fn cubic_to_quads(p: [V; 4], tol_sqd: f32, quads: &mut Vec<V>, sublevel: u32, first: bool, last: bool) {
    let near = 1.0 / 4096.0;
    let (mut ab, mut dc) = (p[1] - p[0], p[2] - p[3]);
    if ab.dot(ab) < near {
        if dc.dot(dc) < near {
            quads.extend([p[0], p[0], p[3]]);
            return;
        }
        ab = p[2] - p[0];
    }
    if dc.dot(dc) < near {
        dc = p[1] - p[3];
    }
    let (ab, dc) = (ab * 1.5, dc * 1.5);
    let (c0, c1) = (p[0] + ab, p[3] + dc);
    let d_sqd = if sublevel > 10 { 0.0 } else { c0.distance_sqd(c1) };
    if d_sqd < tol_sqd {
        let c = if first == last { (c0 + c1) * 0.5 } else if first { c0 } else { c1 };
        quads.extend([p[0], c, p[3]]);
        return;
    }
    let mix = |a: V, b: V| (b - a) * 0.5 + a;
    let (ab, bc, cd) = (mix(p[0], p[1]), mix(p[1], p[2]), mix(p[2], p[3]));
    let (abc, bcd) = (mix(ab, bc), mix(bc, cd));
    let abcd = mix(abc, bcd);
    cubic_to_quads([p[0], ab, abc, abcd], tol_sqd, quads, sublevel + 1, first, false);
    cubic_to_quads([abcd, bcd, cd, p[3]], tol_sqd, quads, sublevel + 1, false, last);
}

/// `add_line`: the six corners, each with its coverage.
fn line(a: V, b: V, coverage: f32) -> Option<[(V, f32); 6]> {
    let mut vec = b - a;
    let length_sqd = vec.dot(vec);
    if !vec.set_length(0.5) {
        return None;
    }
    let ortho = V::new(2.0 * vec.y, -2.0 * vec.x);
    let (i0, i1) = if length_sqd >= 1.0 {
        ((a + vec, coverage), (b - vec, coverage))
    } else {
        let len = length_sqd.sqrt();
        ((b - vec, coverage * len), (a + vec, coverage * len))
    };
    Some([i0, i1, (a - vec + ortho, 0.0), (b + vec + ortho, 0.0), (a - vec - ortho, 0.0), (b + vec - ortho, 0.0)])
}

/// `bloat_quad`: a0, a1, b0, c0, c1.
fn bloat_quad(q: [V; 3]) -> Option<[V; 5]> {
    let (a, b, c) = (q[0], q[1], q[2]);
    let (mut ab, ac, mut cb) = (b - a, c - a, b - c);
    let (ab_ok, cb_ok) = (ab.normalize(), cb.normalize());
    if !ab_ok {
        if !cb_ok {
            return None;
        }
        ab = cb;
    }
    if !cb_ok {
        cb = ab;
    }
    let mut ab_n = ab.orthog(-1);
    if ab_n.dot(ac) > 0.0 {
        ab_n = -ab_n;
    }
    let mut cb_n = cb.orthog(-1);
    if cb_n.dot(ac) < 0.0 {
        cb_n = -cb_n;
    }
    let (a0, a1, c0, c1) = (a + ab_n, a - ab_n, c + cb_n, c - cb_n);
    Some([a0, a1, intersect_lines(a0, ab_n, c0, cb_n), c0, c1])
}

fn intersect_lines(pa: V, na: V, pb: V, nb: V) -> V {
    let (aw, bw) = (-na.dot(pa), -nb.dot(pb));
    let w_inv = 1.0 / (na.x * nb.y - na.y * nb.x);
    if !w_inv.is_finite() {
        return (pa + pb) * 0.5 + na;
    }
    V::new((na.y * bw - aw * nb.y) * w_inv, (aw * nb.x - na.x * bw) * w_inv)
}
