//! Ganesh's convex fill (`AAConvexPathRenderer.cpp`), ported step for step: Chromium's GPU
//! raster fills a convex path whose winding is known this way. The path becomes line and quad
//! segments (cubics cut into quads that keep within their tangents), fanned from its centre of
//! mass, each edge given a pixel-wide outer strip and each corner a wedge; the GPU shades every
//! strip by the edge's implicit quadratic over its gradient, a ramp half a pixel either side.

use super::El;
use super::geom::{V, distance_to_segment_sqd, nearly_zero};
use swash::zeno::Transform;

/// One vertex: device position, and the shader's `(u, v, d0, d1)`.
#[derive(Clone, Copy)]
pub(in super::super) struct EdgeVert {
    pub(in super::super) pos: [f32; 2],
    pub(in super::super) edge: [f32; 4],
}

pub(in super::super) struct EdgeMesh {
    pub(in super::super) verts: Vec<EdgeVert>,
    pub(in super::super) indices: Vec<usize>,
}

const CLOSE: f32 = 1.0 / 16.0;
const CLOSE_SQD: f32 = CLOSE * CLOSE;

#[derive(Clone, Copy, Default)]
struct Segment {
    quad: bool,
    pts: [V; 2],
    norms: [V; 2],
    mid: V,
}

impl Segment {
    fn count(&self) -> usize {
        if self.quad { 2 } else { 1 }
    }

    fn end(&self) -> V {
        self.pts[self.count() - 1]
    }

    fn end_norm(&self) -> V {
        self.norms[self.count() - 1]
    }
}

/// Ganesh's mesh for filling `path` (user units, convex, winding `dir`: 1 clockwise, -1
/// counter) under `ctm`, or None where it draws nothing (a path with no area).
pub(in super::super) fn fill(path: &[El], ctm: Transform, dir: i32) -> Option<EdgeMesh> {
    let dir = if ctm.xx * ctm.yy - ctm.yx * ctm.xy < 0.0 { -dir } else { dir };
    let map = |p: V| super::map(ctm, p);
    let mut segs: Vec<Segment> = Vec::new();
    let mut degenerate = Degenerate::default();
    let (mut start, mut cur) = (None, None);
    let line = |segs: &mut Vec<Segment>, p: V| segs.push(Segment { pts: [p, V::default()], ..Segment::default() });
    for el in path {
        match *el {
            El::Move(p) => {
                degenerate.add(map(p));
                (start, cur) = (Some(p), Some(p));
            }
            El::Line(p) => {
                if cur != Some(p) {
                    let d = map(p);
                    degenerate.add(d);
                    line(&mut segs, d);
                }
                cur = Some(p);
            }
            El::Quad(c, p) => {
                let Some(from) = cur else { continue };
                if !(from == c && c == p) {
                    let q = [map(from), map(c), map(p)];
                    degenerate.add(q[1]);
                    degenerate.add(q[2]);
                    add_quad(q, &mut segs);
                }
                cur = Some(p);
            }
            El::Conic(c, p, w) => {
                let Some(from) = cur else { continue };
                if !(from == c && c == p) {
                    let (q0, q) = (map(from), super::conic_quads([map(from), map(c), map(p)], w));
                    let mut last = q0;
                    for pair in q.as_chunks::<2>().0 {
                        degenerate.add(pair[0]);
                        degenerate.add(pair[1]);
                        add_quad([last, pair[0], pair[1]], &mut segs);
                        last = pair[1];
                    }
                }
                cur = Some(p);
            }
            El::Cubic(c1, c2, p) => {
                let Some(from) = cur else { continue };
                if !(from == c1 && c1 == c2 && c2 == p) {
                    let q = [map(from), map(c1), map(c2), map(p)];
                    degenerate.add(q[1]);
                    degenerate.add(q[2]);
                    degenerate.add(q[3]);
                    let mut quads = Vec::new();
                    cubic_to_quads(q, dir, &mut quads);
                    for t in quads.as_chunks::<3>().0 {
                        add_quad([t[0], t[1], t[2]], &mut segs);
                    }
                }
                cur = Some(p);
            }
            // `SkPath::Iter` closes the contour with a line back to its start.
            El::Close => {
                if let (Some(s), Some(c)) = (start, cur)
                    && s != c
                {
                    let d = map(s);
                    degenerate.add(d);
                    line(&mut segs, d);
                }
                cur = start;
            }
        }
    }
    if degenerate.stage != Stage::NonDegenerate {
        return None;
    }
    let fan = compute_vectors(&mut segs, dir)?;
    Some(create_vertices(&segs, fan))
}

#[derive(Default, PartialEq, Clone, Copy)]
enum Stage {
    #[default]
    Initial,
    Point,
    Line,
    NonDegenerate,
}

#[derive(Default)]
struct Degenerate {
    stage: Stage,
    first: V,
    normal: V,
    c: f32,
}

impl Degenerate {
    fn add(&mut self, p: V) {
        match self.stage {
            Stage::Initial => {
                self.first = p;
                self.stage = Stage::Point;
            }
            Stage::Point => {
                if p.distance_sqd(self.first) > CLOSE_SQD {
                    let mut n = p - self.first;
                    n.normalize();
                    self.normal = n.orthog(-1);
                    self.c = -self.normal.dot(self.first);
                    self.stage = Stage::Line;
                }
            }
            Stage::Line => {
                if (self.normal.dot(p) + self.c).abs() > CLOSE {
                    self.stage = Stage::NonDegenerate;
                }
            }
            Stage::NonDegenerate => {}
        }
    }
}

fn add_quad(p: [V; 3], segs: &mut Vec<Segment>) {
    if distance_to_segment_sqd(p[1], p[0], p[2]) < CLOSE_SQD {
        if p[0] != p[2] {
            segs.push(Segment { pts: [p[2], V::default()], ..Segment::default() });
        }
    } else {
        segs.push(Segment { quad: true, pts: [p[1], p[2]], ..Segment::default() });
    }
}

fn center_of_mass(segs: &[Segment]) -> Option<V> {
    let n = segs.len();
    if n == 0 {
        return None;
    }
    let (mut area, mut center, mut p0) = (0.0f32, V::new(0.0, 0.0), V::new(0.0, 0.0));
    if n > 2 {
        p0 = segs[0].end();
        let mut pj = segs[1].end() - p0;
        for i in 1..n - 1 {
            let pi = pj;
            pj = segs[i + 1].end() - p0;
            let t = pi.cross(pj);
            area += t;
            center.x += (pi.x + pj.x) * t;
            center.y += (pi.y + pj.y) * t;
        }
    }
    let c = if nearly_zero(area) {
        let mut avg = segs.iter().fold(V::new(0.0, 0.0), |a, s| V::new(a.x + s.end().x, a.y + s.end().y));
        let denom = 1.0 / n as f32;
        avg = avg * denom;
        avg
    } else {
        let inv = 1.0 / (area * 3.0);
        center * inv + p0
    };
    (c.x.is_finite() && c.y.is_finite()).then_some(c)
}

fn compute_vectors(segs: &mut [Segment], dir: i32) -> Option<V> {
    let fan = center_of_mass(segs)?;
    let n = segs.len();
    // Normals face out: to the right of a counter-clockwise path's edges, left of a clockwise one's.
    let side = if dir == -1 { 1 } else { -1 };
    for a in 0..n {
        let b = (a + 1) % n;
        let mut prev = segs[a].end();
        for p in 0..segs[b].count() {
            let mut d = segs[b].pts[p] - prev;
            d.normalize();
            segs[b].norms[p] = d.orthog(side);
            prev = segs[b].pts[p];
        }
    }
    for a in 0..n {
        let b = (a + 1) % n;
        let mut mid = segs[b].norms[0] + segs[a].end_norm();
        mid.normalize();
        segs[b].mid = mid;
    }
    Some(fan)
}

fn create_vertices(segs: &[Segment], fan: V) -> EdgeMesh {
    let mut verts: Vec<EdgeVert> = Vec::new();
    let mut idx: Vec<usize> = Vec::new();
    let vert = |p: V, uv: [f32; 2], d: [f32; 2]| EdgeVert { pos: [p.x, p.y], edge: [uv[0], uv[1], d[0], d[1]] };
    let neg = [-1.0, -1.0];
    let n = segs.len();
    for a in 0..n {
        let (sa, sb) = (segs[a], segs[(a + 1) % n]);
        let v = verts.len();
        let p0 = sa.end();
        verts.push(vert(p0, [0.0, 0.0], neg));
        verts.push(vert(p0 + sa.end_norm(), [0.0, -1.0], neg));
        verts.push(vert(p0 + sb.mid, [0.0, -1.0], neg));
        verts.push(vert(p0 + sb.norms[0], [0.0, -1.0], neg));
        idx.extend([v, v + 2, v + 1, v, v + 3, v + 2]);
        let v = verts.len();
        if !sb.quad {
            let (v1, v2) = (sa.end(), sb.pts[0]);
            let dist = distance_to_line(fan, v1, v2);
            verts.push(vert(fan, [0.0, dist], neg));
            verts.push(vert(v1, [0.0, 0.0], neg));
            verts.push(vert(v2, [0.0, 0.0], neg));
            verts.push(vert(v1 + sb.norms[0], [0.0, -1.0], neg));
            verts.push(vert(v2 + sb.norms[0], [0.0, -1.0], neg));
            idx.extend([v + 3, v + 1, v + 2, v + 4, v + 3, v + 2]);
            if n >= 3 {
                idx.extend([v, v + 2, v + 1]);
            }
        } else {
            let q = [sa.end(), sb.pts[0], sb.pts[1]];
            let c0 = sb.norms[0].dot(q[0]);
            let c1 = sb.norms[1].dot(q[2]);
            let mut mid = sb.norms[0] + sb.norms[1];
            mid.normalize();
            let pos = [fan, q[0], q[2], q[0] + sb.norms[0], q[2] + sb.norms[1], q[1] + mid];
            let m = quad_uv_matrix(q);
            let uv = |p: V| [m[0] * p.x + m[1] * p.y + m[2], m[3] * p.x + m[4] * p.y + m[5]];
            // A large negative that interpolates without upsetting a GPU's precision.
            let big = -f32::MAX / 1_000_000.0;
            verts.push(vert(pos[0], uv(pos[0]), [-sb.norms[0].dot(fan) + c0, -sb.norms[1].dot(fan) + c1]));
            verts.push(vert(pos[1], uv(pos[1]), [0.0, -sb.norms[1].dot(q[0]) + c1]));
            verts.push(vert(pos[2], uv(pos[2]), [-sb.norms[0].dot(q[2]) + c0, 0.0]));
            for &p in &pos[3..] {
                verts.push(vert(p, uv(p), [big, big]));
            }
            idx.extend([v + 3, v + 1, v + 2, v + 4, v + 3, v + 2, v + 5, v + 3, v + 4]);
            if n >= 3 {
                idx.extend([v, v + 2, v + 1]);
            }
        }
    }
    EdgeMesh { verts, indices: idx }
}

/// `SkPointPriv::DistanceToLineBetween`.
fn distance_to_line(pt: V, a: V, b: V) -> f32 {
    let (u, v) = (b - a, pt - a);
    let det = u.cross(v);
    (det / u.dot(u) * det).sqrt()
}

/// `GrPathUtils::QuadUVMatrix::set`: the map from device points to the quad's `(u, v)`.
pub(super) fn quad_uv_matrix(q: [V; 3]) -> [f32; 6] {
    let (x0, y0, x1, y1, x2, y2) = (q[0].x as f64, q[0].y as f64, q[1].x as f64, q[1].y as f64, q[2].x as f64, q[2].y as f64);
    let (a2, a5, a8) = (x1 * y2 - x2 * y1, x2 * y0 - x0 * y2, x0 * y1 - x1 * y0);
    let det = a2 + a5 + a8;
    if !det.is_finite() || (det as f32).abs() <= (1.0 / 4096.0) * (1.0 / 4096.0) {
        let d = [q[0].distance_sqd(q[1]), q[1].distance_sqd(q[2]), q[2].distance_sqd(q[0])];
        let mut max_edge = 0;
        let mut max_d = d[0];
        for (i, &di) in d.iter().enumerate().skip(1) {
            if di > max_d {
                (max_d, max_edge) = (di, i);
            }
        }
        if max_d > 0.0 {
            let l = (q[(max_edge + 1) % 3] - q[max_edge]).orthog(-1);
            return [0.0, 0.0, 0.0, l.x, l.y, -l.dot(q[max_edge])];
        }
        return [0.0, 0.0, 100.0, 0.0, 0.0, 100.0];
    }
    let scale = 1.0 / det;
    let (a3, a4, a6, a7) = (y2 - y0, x0 - x2, y0 - y1, x1 - x0);
    [
        ((0.5 * a3 + a6) * scale) as f32,
        ((0.5 * a4 + a7) * scale) as f32,
        ((0.5 * a5 + a8) * scale) as f32,
        (a6 * scale) as f32,
        (a7 * scale) as f32,
        (a8 * scale) as f32,
    ]
}

/// `GrPathUtils::convertCubicToQuadsConstrainToTangents` with a tolerance of one device pixel.
fn cubic_to_quads(p: [V; 4], dir: i32, quads: &mut Vec<V>) {
    if p.iter().any(|q| !q.x.is_finite() || !q.y.is_finite()) {
        return;
    }
    for c in chop_at_inflections(p) {
        noninflect_to_quads(c, 1.0, dir, quads, 0);
    }
}

fn is_within_tangents(a: V, ab: V, dc: V, d: V, dir: i32, p: V) -> bool {
    let ap_ab = (p - a).cross(ab);
    if (dir == 1 && ap_ab > 0.0) || (dir != 1 && ap_ab < 0.0) {
        return false;
    }
    let dp_dc = (p - d).cross(dc);
    !((dir == 1 && dp_dc < 0.0) || (dir != 1 && dp_dc > 0.0))
}

fn noninflect_to_quads(p: [V; 4], tol_sqd: f32, dir: i32, quads: &mut Vec<V>, sublevel: u32) {
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
    let da = p[0] - p[3];
    let mut do_quads = dc.dot(dc) < near || ab.dot(ab) < near;
    if !do_quads {
        let mut inv = da.dot(da);
        if inv > near {
            inv = 1.0 / inv;
            let det_ab = ab.cross(da);
            let det_ab = det_ab * det_ab;
            let det_dc = dc.cross(da);
            let det_dc = det_dc * det_dc;
            if det_ab * inv < tol_sqd && det_dc * inv < tol_sqd {
                do_quads = true;
            }
        }
    }
    if do_quads {
        let (b, c) = (p[0] + ab, p[3] + dc);
        let mid = (b + c) * 0.5;
        if da.dot(dc) < 0.0 || ab.dot(da) > 0.0 {
            quads.extend([p[0], b, mid, mid, c, p[3]]);
        } else {
            quads.extend([p[0], mid, p[3]]);
        }
        return;
    }
    let (ab, dc) = (ab * 1.5, dc * 1.5);
    let (c0, c1) = (p[0] + ab, p[3] + dc);
    let d_sqd = if sublevel > 10 { 0.0 } else { c0.distance_sqd(c1) };
    if d_sqd < tol_sqd {
        let mut avg = (c0 + c1) * 0.5;
        let mut subdivide = false;
        if !is_within_tangents(p[0], ab, dc, p[3], dir, avg) {
            let ab = ab.orthog(-1);
            let z0 = -ab.dot(p[0]);
            let dc = dc.orthog(-1);
            let z1 = -dc.dot(p[3]);
            avg = V::new(ab.y * z1 - z0 * dc.y, z0 * dc.x - ab.x * z1);
            let z = 1.0 / (ab.x * dc.y - ab.y * dc.x);
            avg = V::new(avg.x * z, avg.y * z);
            if sublevel <= 10 {
                let (d0, d1) = (c0.distance_sqd(avg), c1.distance_sqd(avg));
                subdivide = 2.0 * (d0 * d1).sqrt() + d0 + d1 > tol_sqd;
            }
        }
        if !subdivide {
            quads.extend([p[0], avg, p[3]]);
            return;
        }
    }
    let h = chop_at(p, 0.5);
    noninflect_to_quads([h[0], h[1], h[2], h[3]], tol_sqd, dir, quads, sublevel + 1);
    noninflect_to_quads([h[3], h[4], h[5], h[6]], tol_sqd, dir, quads, sublevel + 1);
}

fn mix(a: V, b: V, t: f32) -> V {
    (b - a) * t + a
}

/// `SkChopCubicAt(src, dst, t)`.
fn chop_at(p: [V; 4], t: f32) -> [V; 7] {
    if t == 1.0 {
        return [p[0], p[1], p[2], p[3], p[3], p[3], p[3]];
    }
    let (ab, bc, cd) = (mix(p[0], p[1], t), mix(p[1], p[2], t), mix(p[2], p[3], t));
    let (abc, bcd) = (mix(ab, bc, t), mix(bc, cd, t));
    [p[0], ab, abc, mix(abc, bcd, t), bcd, cd, p[3]]
}

/// `SkChopCubicAtInflections`: the cubic cut where it inflects, as Skia cuts it.
pub(super) fn chop_at_inflections(p: [V; 4]) -> Vec<[V; 4]> {
    let a = p[1] - p[0];
    let b = V::new(p[2].x - 2.0 * p[1].x + p[0].x, p[2].y - 2.0 * p[1].y + p[0].y);
    let c = V::new(p[3].x + 3.0 * (p[1].x - p[2].x) - p[0].x, p[3].y + 3.0 * (p[1].y - p[2].y) - p[0].y);
    let ts = unit_quad_roots(b.x * c.y - b.y * c.x, a.x * c.y - a.y * c.x, a.x * b.y - a.y * b.x);
    match ts.as_slice() {
        [] => vec![p],
        [t] => {
            let d = chop_at(p, *t);
            vec![[d[0], d[1], d[2], d[3]], [d[3], d[4], d[5], d[6]]]
        }
        [t0, t1, ..] => {
            let (t0, t1) = (*t0, *t1);
            if t1 == 1.0 {
                let d = chop_at(p, t0);
                return vec![[d[0], d[1], d[2], d[3]], [d[3], d[4], d[5], d[6]], [p[3], p[3], p[3], p[3]]];
            }
            // Both chops on the whole cubic, as Skia's two-lane chop does them.
            let (ab0, bc0, cd0) = (mix(p[0], p[1], t0), mix(p[1], p[2], t0), mix(p[2], p[3], t0));
            let (abc0, bcd0) = (mix(ab0, bc0, t0), mix(bc0, cd0, t0));
            let (ab1, bc1, cd1) = (mix(p[0], p[1], t1), mix(p[1], p[2], t1), mix(p[2], p[3], t1));
            let (abc1, bcd1) = (mix(ab1, bc1, t1), mix(bc1, cd1, t1));
            let (abcd0, abcd1) = (mix(abc0, bcd0, t0), mix(abc1, bcd1, t1));
            let (m0, m1) = (mix(abc0, bcd0, t1), mix(abc1, bcd1, t0));
            vec![[p[0], ab0, abc0, abcd0], [abcd0, m0, m1, abcd1], [abcd1, bcd1, cd1, p[3]]]
        }
    }
}

/// `SkFindUnitQuadRoots`.
fn unit_quad_roots(a: f32, b: f32, c: f32) -> Vec<f32> {
    let unit_divide = |mut numer: f32, mut denom: f32| {
        if numer < 0.0 {
            numer = -numer;
            denom = -denom;
        }
        if denom == 0.0 || numer == 0.0 || numer >= denom {
            return None;
        }
        let r = numer / denom;
        (!r.is_nan() && r != 0.0).then_some(r)
    };
    if a == 0.0 {
        return unit_divide(-c, b).into_iter().collect();
    }
    let dr = b as f64 * b as f64 - 4.0 * a as f64 * c as f64;
    if dr < 0.0 {
        return vec![];
    }
    let r = dr.sqrt() as f32;
    if !r.is_finite() {
        return vec![];
    }
    let q = if b < 0.0 { -(b - r) / 2.0 } else { -(b + r) / 2.0 };
    let mut roots: Vec<f32> = [unit_divide(q, a), unit_divide(c, q)].into_iter().flatten().collect();
    if roots.len() == 2 {
        if roots[0] > roots[1] {
            roots.swap(0, 1);
        } else if roots[0] == roots[1] {
            roots.pop();
        }
    }
    roots
}
