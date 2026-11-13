//! Ganesh's hairlines (`AAHairLinePathRenderer.cpp`), ported step for step: Chromium's GPU raster
//! draws a stroke a device pixel wide or less this way, at the width's share of full coverage.
//! Each line segment is a hexagon whose coverage falls from the line to nothing a pixel either
//! side (and over a pixel across each end); each quad (a cubic is first cut into quads) is a
//! pentagon round its control triangle, shaded by the quad's implicit distance as a one pixel
//! tent; each conic likewise, by its own implicit form (`GrConicEffect`). Lines draw first,
//! then quads, then conics, each blended over.

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
    let (lines, quads, conics) = gather(path, ctm);
    // Every vertex: its position, its kind, and a line's coverage, a quad's (u, v) or a conic's
    // (k, l, m).
    let mut pts: Vec<[f32; 2]> = Vec::new();
    let mut attr: Vec<(Kind, [f32; 3])> = Vec::new();
    let mut indices: Vec<usize> = Vec::new();
    for l in lines.as_chunks::<2>().0 {
        let Some(hex) = line(l[0], l[1], coverage) else { continue };
        let base = pts.len();
        pts.extend(hex.iter().map(|(p, _)| [p.x, p.y]));
        attr.extend(hex.iter().map(|&(_, c)| (Kind::Line, [c, 0.0, 0.0])));
        indices.extend([0, 1, 3, 0, 3, 2, 0, 4, 5, 0, 5, 1, 0, 2, 4, 1, 5, 3].map(|i| base + i));
    }
    for q in quads.as_chunks::<3>().0 {
        let Some(pent) = bloat_quad([q[0], q[1], q[2]]) else { continue };
        let m = quad_uv_matrix([q[0], q[1], q[2]]);
        let base = pts.len();
        pts.extend(pent.iter().map(|p| [p.x, p.y]));
        attr.extend(pent.iter().map(|p| (Kind::Quad, [m[0] * p.x + m[1] * p.y + m[2], m[3] * p.x + m[4] * p.y + m[5], 0.0])));
        indices.extend([0, 1, 2, 2, 4, 3, 1, 4, 2].map(|i| base + i));
    }
    for (c, w) in conics {
        let Some(pent) = bloat_quad(c) else { continue };
        let klm = conic_klm(c, w);
        let base = pts.len();
        pts.extend(pent.iter().map(|p| [p.x, p.y]));
        attr.extend(pent.iter().map(|p| (Kind::Conic, klm.map(|r| r[0] * p.x + r[1] * p.y + r[2]))));
        indices.extend([0, 1, 2, 2, 4, 3, 1, 4, 2].map(|i| base + i));
    }
    rasterize(&pts, &indices, origin, px, out, |tri, w| {
        let at = |k: usize| (0..3).map(|i| w[i] * attr[tri[i]].1[k] as f64).sum::<f64>() as f32;
        let d = |k: usize| gradient(&pts, tri, |i| attr[i].1[k], origin);
        let (f, g) = match attr[tri[0]].0 {
            Kind::Line => return at(0),
            // `GrQuadEffect`, hairline: one less the implicit distance, times the coverage.
            Kind::Quad => {
                let (u, v, du, dv) = (at(0), at(1), d(0), d(1));
                (u * u - v, [2.0 * u * du[0] - dv[0], 2.0 * u * du[1] - dv[1]])
            }
            // `GrConicEffect`, likewise for k² - lm.
            Kind::Conic => {
                let (k, l, m, dk, dl, dm) = (at(0), at(1), at(2), d(0), d(1), d(2));
                let g = |a: usize| 2.0 * k * dk[a] - l * dm[a] - m * dl[a];
                (k * k - l * m, [g(0), g(1)])
            }
        };
        let gm = (g[0] * g[0] + g[1] * g[1]).sqrt();
        if gm == 0.0 {
            return 0.0;
        }
        (1.0 - f.abs() / gm).max(0.0) * coverage
    });
}

/// A conic's control triangle and weight, in device pixels.
type Conic = ([V; 3], f32);

#[derive(Clone, Copy)]
enum Kind {
    Line,
    Quad,
    Conic,
}

/// `GrPathUtils::getConicKLM`: the rows k, l and m of the conic's implicit form, scaled so the
/// largest coefficient is 10.
fn conic_klm(p: [V; 3], w: f32) -> [[f32; 3]; 3] {
    let w2 = 2.0 * w;
    let klm = [
        [p[2].y - p[0].y, p[0].x - p[2].x, p[2].x * p[0].y - p[0].x * p[2].y],
        [w2 * (p[1].y - p[0].y), w2 * (p[0].x - p[1].x), w2 * (p[1].x * p[0].y - p[0].x * p[1].y)],
        [w2 * (p[2].y - p[1].y), w2 * (p[1].x - p[2].x), w2 * (p[2].x * p[1].y - p[1].x * p[2].y)],
    ];
    let scale = 10.0 / klm.iter().flatten().fold(0.0f32, |a, &b| a.max(b.abs()));
    klm.map(|r| r.map(|c| c * scale))
}

/// `gather_lines_and_quads`: line segments (pairs), quads (triples) and conics with their
/// weights, in device pixels.
fn gather(path: &[El], ctm: Transform) -> (Vec<V>, Vec<V>, Vec<Conic>) {
    let map = |p: V| super::map(ctm, p);
    let (mut lines, mut quads, mut conics) = (Vec::new(), Vec::new(), Vec::new());
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
                // Cut where it curves most, and each half again, for a tighter fit.
                for (q, w) in split_conic([cur, c, p], w).into_iter().flat_map(|(q, w)| split_conic(q, w)) {
                    let d = q.map(map);
                    if degenerate(d) {
                        lines.extend([d[0], d[1], d[1], d[2]]);
                    } else {
                        conics.push((d, w));
                    }
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
    (lines, quads, conics)
}

/// `split_conic`: the conic cut at its quad's point of most curvature, if that's inside it.
fn split_conic(p: [V; 3], w: f32) -> Vec<([V; 3], f32)> {
    let t = max_curvature(p);
    if !(t > 0.0 && t < 1.0) {
        return vec![(p, w)];
    }
    // `SkConic::chopAt`: de Casteljau on the conic's homogeneous points, back in standard form.
    let h = [[p[0].x, p[0].y, 1.0], [p[1].x * w, p[1].y * w, w], [p[2].x, p[2].y, 1.0]];
    let mix = |a: [f32; 3], b: [f32; 3]| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    let (ab, bc) = (mix(h[0], h[1]), mix(h[1], h[2]));
    let abc = mix(ab, bc);
    let down = |a: [f32; 3]| V::new(a[0] / a[2], a[1] / a[2]);
    let root = abc[2].sqrt();
    let halves = [([p[0], down(ab), down(abc)], ab[2] / root), ([down(abc), down(bc), p[2]], bc[2] / root)];
    let finite = halves.iter().all(|(q, w)| w.is_finite() && q.iter().all(|v| v.x.is_finite() && v.y.is_finite()));
    if !finite {
        return vec![(p, w)];
    }
    halves.to_vec()
}

/// `SkFindQuadMaxCurvature`.
fn max_curvature(p: [V; 3]) -> f32 {
    let (a, b) = (p[1] - p[0], V::new(p[0].x - p[1].x - p[1].x + p[2].x, p[0].y - p[1].y - p[1].y + p[2].y));
    let (mut numer, mut denom) = (-(a.x * b.x + a.y * b.y), b.x * b.x + b.y * b.y);
    if denom < 0.0 {
        (numer, denom) = (-numer, -denom);
    }
    if numer <= 0.0 {
        return 0.0;
    }
    if numer >= denom {
        return 1.0;
    }
    numer / denom
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
    let t = max_curvature(p);
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
