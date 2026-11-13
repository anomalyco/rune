//! Ganesh's tessellator for convex paths (`GrAAConvexTessellator.cpp`), ported step for step:
//! Chromium's GPU raster draws a closed convex path stroked with mitred or bevelled joins at
//! least a device pixel wide this way (`AALinearizingConvexPathRenderer`), as triangles whose
//! corners carry a coverage the GPU interpolates. The path's curves are flattened into lines
//! (a fifth of a device pixel from them at most), the polygon's edges pushed out and pulled in by
//! half the stroke width, and rings a device pixel apart ramp the coverage from 0 to 1 across each
//! side of the stroke. `super::raster` covers the triangles as the GPU rasterizes them.

use super::geom::{self, V, nearly_equal, nearly_zero};
use super::{El, conic_quads, map};
use swash::zeno::{Join, Transform};

/// Device distance points are pushed in or out to make the soft antialiased edge.
const AA_RADIUS: f32 = 0.5;
/// Points this close (device pixels) fuse, and near-colinear points within it are dropped.
const CLOSE: f32 = 1.0 / 16.0;
const CLOSE_SQD: f32 = CLOSE * CLOSE;
const CURVE_TOLERANCE: f32 = 0.2;
/// Dot product below which a curve's corner gets an extra rounding point.
const ROUND_CAP_THRESHOLD: f32 = 0.8;
/// Dot product above which two adjacent curves count as one.
const CURVE_CONNECTION_THRESHOLD: f32 = 0.8;

#[derive(Clone, Copy, PartialEq)]
enum Curve {
    Sharp,
    Indeterminate,
    Smooth,
}

/// One point of a ring: an index into the pool, the edge of the original polygon it measures
/// depth from, and its outward normal and inward bisector.
#[derive(Clone, Copy, Default)]
struct RingPt {
    norm: V,
    bisector: V,
    index: usize,
    orig_edge: usize,
}

#[derive(Clone, Default)]
struct Ring(Vec<RingPt>);

#[derive(Clone, Copy)]
struct Candidate {
    pt: V,
    originating: Option<usize>,
    orig_edge: usize,
    needs_new: bool,
}

/// The triangles of a convex stroke, in device pixels: points, their coverage, and indices.
pub(in super::super) struct Mesh {
    pub(in super::super) pts: Vec<V>,
    pub(in super::super) coverage: Vec<f32>,
    pub(in super::super) indices: Vec<usize>,
}

struct Tess {
    pts: Vec<V>,
    coverage: Vec<f32>,
    movable: Vec<bool>,
    curve: Vec<Curve>,
    norms: Vec<V>,
    bisectors: Vec<V>,
    /// Winding of the original polygon: 1 right, -1 left.
    side: i32,
    indices: Vec<usize>,
    join: Join,
    miter_limit: f32,
    accum_error: f32,
}

/// Ganesh's mesh for `path` (user units, one closed convex contour) under `ctm` (a similarity)
/// stroked `width` user units with `join`, or None where Ganesh would decline it.
pub(in super::super) fn stroke(path: &[El], ctm: Transform, width: f32, join: Join, miter_limit: f32) -> Option<Mesh> {
    let scale = (ctm.xx * ctm.xx + ctm.xy * ctm.xy).sqrt().max((ctm.yx * ctm.yx + ctm.yy * ctm.yy).sqrt());
    let mut t = Tess {
        pts: Vec::new(),
        coverage: Vec::new(),
        movable: Vec::new(),
        curve: Vec::new(),
        norms: Vec::new(),
        bisectors: Vec::new(),
        side: 0,
        indices: Vec::new(),
        join,
        miter_limit,
        accum_error: 0.0,
    };
    let initial = t.extract(path, ctm)?;
    let w = scale * width;
    let mut outer = Ring::default();
    t.outer_ring(&initial, w / 2.0 - AA_RADIUS, 1.0, &mut outer);
    t.init_ring(&mut outer);
    let mut outer_aa = Ring::default();
    t.outer_ring(&outer, AA_RADIUS * 2.0, 0.0, &mut outer_aa);
    t.bisectors.clear();
    if initial.0.len() > 2 {
        let depth = w / 2.0 - AA_RADIUS;
        if let Some(inset) = t.inset_rings(initial, 0.0, 1.0, depth, 1.0) {
            t.inset_rings(inset, depth, 1.0, depth + AA_RADIUS * 2.0, 0.0);
        }
    }
    Some(Mesh { pts: t.pts, coverage: t.coverage, indices: t.indices })
}

fn duplicate(a: V, b: V) -> bool {
    a.distance_sqd(b) < CLOSE_SQD
}

fn intersect(p0: V, n0: V, p1: V, n1: V) -> Option<f32> {
    let v = p1 - p0;
    let perp_dot = n0.x * n1.y - n0.y * n1.x;
    if nearly_zero(perp_dot) {
        return None;
    }
    let t = (v.x * n1.y - v.y * n1.x) / perp_dot;
    t.is_finite().then_some(t)
}

fn perp_intersect(p0: V, n0: V, p1: V, perp: V) -> Option<f32> {
    let v = p1 - p0;
    let perp_dot = n0.dot(perp);
    if nearly_zero(perp_dot) {
        return None;
    }
    let t = v.dot(perp) / perp_dot;
    t.is_finite().then_some(t)
}

fn colinear_and_b_is_middle(a: V, b: V, c: V, accum: &mut f32) -> bool {
    let to_c = c - a;
    let mut n = V::new(to_c.y, -to_c.x);
    n.normalize();
    let dist = (n.dot(b) - n.dot(a)).abs();
    if *accum + dist >= CLOSE || to_c.dot(b - a) <= 0.0 || to_c.dot(c - b) <= 0.0 {
        return false;
    }
    *accum += dist;
    true
}

fn coverage_at(depth: f32, initial_depth: f32, initial_coverage: f32, target_depth: f32, target_coverage: f32) -> f32 {
    if nearly_equal(initial_depth, target_depth) {
        return target_coverage;
    }
    ((depth - initial_depth) / (target_depth - initial_depth) * (target_coverage - initial_coverage) + initial_coverage).clamp(0.0, 1.0)
}

impl Tess {
    fn add_pt(&mut self, pt: V, coverage: f32, movable: bool, curve: Curve) -> usize {
        self.pts.push(pt);
        self.coverage.push(coverage);
        self.movable.push(movable);
        self.curve.push(curve);
        self.pts.len() - 1
    }

    fn pop_last(&mut self) {
        self.pts.pop();
        self.coverage.pop();
        self.movable.pop();
        self.curve.pop();
    }

    /// `SkTDArray::removeShuffle(0)`: the last point takes the first's place.
    fn pop_first_shuffle(&mut self) {
        self.pts.swap_remove(0);
        self.coverage.swap_remove(0);
        self.movable.swap_remove(0);
        self.curve.swap_remove(0);
    }

    fn add_tri(&mut self, a: usize, b: usize, c: usize) {
        if a == b || b == c || c == a {
            return;
        }
        self.indices.extend([a, b, c]);
    }

    fn line_to(&mut self, p: V, curve: Curve) {
        if let Some(&last) = self.pts.last()
            && duplicate(p, last)
        {
            return;
        }
        let n = self.pts.len();
        if n >= 2 && colinear_and_b_is_middle(self.pts[n - 2], self.pts[n - 1], p, &mut self.accum_error) {
            self.pop_last();
            if self.pts.last().is_some_and(|&last| duplicate(p, last)) {
                return;
            }
        } else {
            self.accum_error = 0.0;
        }
        // A stroke's first ring is fully covered.
        self.add_pt(p, 1.0, false, curve);
    }

    fn quad_to(&mut self, p: [V; 3]) {
        let max = geom::max_vertices(geom::quadratic_log2(1.0 / CURVE_TOLERANCE, p));
        let mut out = Vec::new();
        geom::quadratic_points(p[0], p[1], p[2], CURVE_TOLERANCE * CURVE_TOLERANCE, &mut out, max);
        self.flattened(&out);
    }

    fn cubic_to(&mut self, p: [V; 4]) {
        let max = geom::max_vertices(geom::cubic_log2(1.0 / CURVE_TOLERANCE, p));
        let mut out = Vec::new();
        geom::cubic_points(p[0], p[1], p[2], p[3], CURVE_TOLERANCE * CURVE_TOLERANCE, &mut out, max);
        self.flattened(&out);
    }

    fn flattened(&mut self, out: &[V]) {
        for &q in &out[..out.len() - 1] {
            self.line_to(q, Curve::Smooth);
        }
        self.line_to(out[out.len() - 1], if out.len() == 1 { Curve::Sharp } else { Curve::Indeterminate });
    }

    /// `extractFromPath`: the polygon, its normals and bisectors, and the initial ring.
    fn extract(&mut self, path: &[El], ctm: Transform) -> Option<Ring> {
        let map = |p: V| map(ctm, p);
        let (mut start, mut cur) = (V::new(0.0, 0.0), V::new(0.0, 0.0));
        let mut open = false;
        for el in path {
            match *el {
                El::Move(p) => {
                    if open && cur != start {
                        self.line_to(start, Curve::Sharp);
                    }
                    start = map(p);
                    cur = start;
                    open = false;
                }
                El::Line(p) => {
                    let q = map(p);
                    if q != cur {
                        self.line_to(q, Curve::Sharp);
                    }
                    (cur, open) = (q, true);
                }
                El::Quad(c, p) => {
                    let (c, q) = (map(c), map(p));
                    if !(c == cur && q == cur) {
                        self.quad_to([cur, c, q]);
                    }
                    (cur, open) = (q, true);
                }
                El::Cubic(c1, c2, p) => {
                    let (c1, c2, q) = (map(c1), map(c2), map(p));
                    if !(c1 == cur && c2 == cur && q == cur) {
                        self.cubic_to([cur, c1, c2, q]);
                    }
                    (cur, open) = (q, true);
                }
                El::Conic(c, p, w) => {
                    let (c, q) = (map(c), map(p));
                    if !(c == cur && q == cur) {
                        // `conicTo`: quads a quarter pixel from the conic, each flattened.
                        let quads = conic_quads([cur, c, q], w);
                        let mut last = cur;
                        for (i, pair) in quads.as_chunks::<2>().0.iter().enumerate() {
                            let end = if i == quads.len() / 2 - 1 { q } else { pair[1] };
                            self.quad_to([last, pair[0], end]);
                            last = end;
                        }
                    }
                    (cur, open) = (q, true);
                }
                El::Close => {
                    if open && cur != start {
                        self.line_to(start, Curve::Sharp);
                    }
                    cur = start;
                    open = false;
                }
            }
        }
        if open && cur != start {
            self.line_to(start, Curve::Sharp);
        }
        if self.pts.len() < 2 {
            return None;
        }
        if duplicate(self.pts[self.pts.len() - 1], self.pts[0]) {
            self.pop_last();
        }
        self.accum_error = 0.0;
        loop {
            let n = self.pts.len();
            if n < 3 {
                break;
            }
            if colinear_and_b_is_middle(self.pts[n - 2], self.pts[n - 1], self.pts[0], &mut self.accum_error) {
                self.pop_last();
            } else if colinear_and_b_is_middle(self.pts[n - 1], self.pts[0], self.pts[1], &mut self.accum_error) {
                self.pop_first_shuffle();
            } else {
                break;
            }
        }
        match self.pts.len() {
            n if n >= 3 => {
                self.compute_normals();
                self.compute_bisectors();
            }
            2 => {
                self.side = -1;
                let mut n0 = (self.pts[1] - self.pts[0]).orthog(self.side);
                n0.normalize();
                self.norms = vec![n0, -n0];
                self.bisectors = vec![V::new(0.0, 0.0); 2];
            }
            _ => return None,
        }
        let ring = (0..self.pts.len()).map(|i| RingPt { norm: self.norms[i], bisector: self.bisectors[i], index: i, orig_edge: i }).collect();
        Some(Ring(ring))
    }

    fn compute_normals(&mut self) {
        let n = self.pts.len();
        let first = self.pts[1] - self.pts[0];
        let last = self.pts[0] - self.pts[n - 1];
        self.side = if first.cross(last) > 0.0 { 1 } else { -1 };
        let side = self.side;
        let norm = |v: V| {
            let mut o = v.orthog(side);
            o.normalize();
            o
        };
        self.norms = (0..n)
            .map(|i| match i {
                0 => norm(first),
                _ if i == n - 1 => norm(last),
                _ => norm(self.pts[i + 1] - self.pts[i]),
            })
            .collect();
    }

    fn compute_bisectors(&mut self) {
        let n = self.norms.len();
        self.bisectors = vec![V::new(0.0, 0.0); n];
        let mut prev = n - 1;
        for cur in 0..n {
            let mut b = self.norms[cur] + self.norms[prev];
            if !b.normalize() {
                b = self.norms[cur].orthog(-self.side) + self.norms[prev].orthog(self.side);
                b.normalize();
            } else {
                b = -b;
            }
            self.bisectors[cur] = b;
            if self.curve[prev] == Curve::Indeterminate {
                if self.curve[cur] == Curve::Sharp {
                    self.curve[prev] = Curve::Sharp;
                } else if self.norms[cur].dot(self.norms[prev]).abs() > CURVE_CONNECTION_THRESHOLD {
                    self.curve[prev] = Curve::Smooth;
                    self.curve[cur] = Curve::Smooth;
                } else {
                    self.curve[prev] = Curve::Sharp;
                    self.curve[cur] = Curve::Sharp;
                }
            }
            prev = cur;
        }
    }

    /// `Ring::init(tess)`: normals and bisectors from the ring's own points.
    fn init_ring(&self, ring: &mut Ring) {
        let n = ring.0.len();
        for cur in 0..n {
            let next = (cur + 1) % n;
            let mut d = self.pts[ring.0[next].index] - self.pts[ring.0[cur].index];
            d.normalize();
            ring.0[cur].norm = d.orthog(self.side);
        }
        let mut prev = n.wrapping_sub(1);
        for cur in 0..n {
            let mut b = ring.0[cur].norm + ring.0[prev].norm;
            if !b.normalize() {
                b = ring.0[cur].norm.orthog(-self.side) + ring.0[prev].norm.orthog(self.side);
                b.normalize();
            } else {
                b = -b;
            }
            ring.0[cur].bisector = b;
            prev = cur;
        }
    }

    fn depth_from_edge(&self, edge: usize, p: V) -> f32 {
        -self.norms[edge].dot(p - self.pts[edge])
    }

    fn pt_along_bisector(&self, start: usize, bisector: V, edge: usize, depth: f32) -> Option<V> {
        let norm = self.norms[edge];
        let t = perp_intersect(self.pts[start], bisector, self.pts[edge], norm)?;
        let p = if nearly_equal(t, 0.0) {
            self.pts[start]
        } else if t < 0.0 {
            bisector * t + self.pts[start]
        } else {
            return None;
        };
        let t = -depth / bisector.dot(norm);
        Some(bisector * t + p)
    }

    /// `createOuterRing`.
    fn outer_ring(&mut self, prev_ring: &Ring, outset: f32, coverage: f32, next: &mut Ring) {
        let n = prev_ring.0.len();
        if n == 0 {
            return;
        }
        let mut prev = n - 1;
        let (mut last_perp, mut first_perp) = (usize::MAX, usize::MAX);
        let outset_sq = outset * outset;
        let miter_sq = (outset * self.miter_limit) * (outset * self.miter_limit);
        for cur in 0..n {
            let original = prev_ring.0[cur].index;
            let normal1 = prev_ring.0[prev].norm;
            let perp1 = normal1 * outset + self.pts[original];
            let normal2 = prev_ring.0[cur].norm;
            let perp2 = normal2 * outset + self.pts[original];
            let curve = self.curve[original];
            let perp1_idx = self.add_pt(perp1, coverage, false, curve);
            next.0.push(RingPt { index: perp1_idx, orig_edge: original, ..RingPt::default() });
            let perp2_idx = if duplicate(perp2, self.pts[perp1_idx]) { perp1_idx } else { self.add_pt(perp2, coverage, false, curve) };
            if perp2_idx != perp1_idx {
                if curve == Curve::Smooth {
                    if normal1.dot(normal2) < ROUND_CAP_THRESHOLD {
                        let mut miter = prev_ring.0[cur].bisector;
                        miter.set_length(-outset);
                        let miter = miter + self.pts[original];
                        if !duplicate(miter, self.pts[perp1_idx]) {
                            let m = self.add_pt(miter, coverage, false, Curve::Sharp);
                            next.0.push(RingPt { index: m, orig_edge: original, ..RingPt::default() });
                            self.add_tri(original, perp1_idx, m);
                            self.add_tri(original, m, perp2_idx);
                        }
                    } else {
                        self.add_tri(original, perp1_idx, perp2_idx);
                    }
                } else if self.join == Join::Miter {
                    let mut miter = prev_ring.0[cur].bisector;
                    let sin_half_sq = ((1.0 + normal1.dot(normal2)) / 2.0).max(0.0);
                    let length_sq = outset_sq / sin_half_sq;
                    if length_sq > miter_sq {
                        self.add_tri(original, perp1_idx, perp2_idx);
                    } else {
                        miter.set_length(-length_sq.sqrt());
                        let miter = miter + self.pts[original];
                        if !duplicate(miter, self.pts[perp1_idx]) {
                            let m = self.add_pt(miter, coverage, false, Curve::Sharp);
                            next.0.push(RingPt { index: m, orig_edge: original, ..RingPt::default() });
                            self.add_tri(original, perp1_idx, m);
                            self.add_tri(original, m, perp2_idx);
                        } else {
                            self.add_tri(original, perp1_idx, perp2_idx);
                        }
                    }
                } else {
                    self.add_tri(original, perp1_idx, perp2_idx);
                }
                next.0.push(RingPt { index: perp2_idx, orig_edge: original, ..RingPt::default() });
            }
            if cur == 0 {
                first_perp = perp1_idx;
            } else {
                let prev_idx = prev_ring.0[prev].index;
                self.add_tri(prev_idx, perp1_idx, original);
                self.add_tri(prev_idx, last_perp, perp1_idx);
            }
            last_perp = perp2_idx;
            prev = cur;
        }
        let last = prev_ring.0[n - 1].index;
        self.add_tri(last, first_perp, prev_ring.0[0].index);
        self.add_tri(last, last_perp, first_perp);
    }

    /// `createInsetRings`: rings inward until `target_depth`, at most eight. The final ring.
    fn inset_rings(&mut self, first: Ring, initial_depth: f32, initial_coverage: f32, target_depth: f32, target_coverage: f32) -> Option<Ring> {
        if first.0.len() < 3 {
            return None;
        }
        let mut current = first;
        let mut i = 0;
        while i < 8 {
            let mut next = Ring::default();
            let done = self.inset_ring(&current, &mut next, initial_depth, initial_coverage, target_depth, target_coverage, i == 0);
            current = next;
            if done {
                break;
            }
            self.init_ring(&mut current);
            i += 1;
        }
        // Bailing after eight (a fill would fan the last ring; a stroke leaves it).
        if i == 8 {
            return None;
        }
        if current.0.len() < 3 {
            return None;
        }
        self.init_ring(&mut current);
        Some(current)
    }

    /// `createInsetRing`: true when done.
    #[allow(clippy::too_many_arguments)]
    fn inset_ring(&mut self, last: &Ring, next: &mut Ring, initial_depth: f32, initial_coverage: f32, target_depth: f32, target_coverage: f32, force_new: bool) -> bool {
        let n = last.0.len();
        let (mut min_dist, mut min_t, mut min_edge) = (f32::MAX, 0.0f32, usize::MAX);
        for cur in 0..n {
            let nx = (cur + 1) % n;
            let Some(t) = intersect(self.pts[last.0[cur].index], last.0[cur].bisector, self.pts[last.0[nx].index], last.0[nx].bisector) else { continue };
            if t <= 0.0 {
                continue;
            }
            let dist = -t * last.0[cur].norm.dot(last.0[cur].bisector);
            if min_dist > dist {
                (min_dist, min_t, min_edge) = (dist, t, cur);
            }
        }
        if min_edge == usize::MAX {
            return false;
        }
        let new_pt = last.0[min_edge].bisector * min_t + self.pts[last.0[min_edge].index];
        let mut depth = self.depth_from_edge(last.0[min_edge].orig_edge, new_pt);
        let mut done = false;
        if depth >= target_depth {
            depth = target_depth;
            done = true;
        }
        let mut cands: Vec<Candidate> = Vec::with_capacity(n);
        let mut dst = vec![0usize; n];
        let along = |t: &Tess, i: usize| t.pt_along_bisector(last.0[i].index, last.0[i].bisector, last.0[i].orig_edge, depth);
        let fresh = |t: &Tess, i: usize, pt: V| Candidate { pt, originating: Some(last.0[i].index), orig_edge: last.0[i].orig_edge, needs_new: !t.movable[last.0[i].index] };
        // Where a point can't be placed Ganesh gives up on the ring (and fans a fill's).
        let Some(p0) = along(self, 0) else { return true };
        cands.push(fresh(self, 0, p0));
        dst[0] = 0;
        for cur in 1..n - 1 {
            let Some(p) = along(self, cur) else { return true };
            if !duplicate(p, cands[cands.len() - 1].pt) {
                cands.push(fresh(self, cur, p));
            } else {
                fuse_prior(&mut cands, last.0[cur].orig_edge);
            }
            dst[cur] = cands.len() - 1;
        }
        let cur = n - 1;
        let Some(p) = along(self, cur) else { return true };
        let dup_prev = duplicate(p, cands[cands.len() - 1].pt);
        let dup_next = duplicate(p, cands[0].pt);
        dst[cur] = match (dup_prev, dup_next) {
            (false, false) => {
                cands.push(fresh(self, cur, p));
                cands.len() - 1
            }
            (true, false) => {
                fuse_prior(&mut cands, last.0[cur].orig_edge);
                cands.len() - 1
            }
            (false, true) => {
                cands[0].originating = None;
                cands[0].needs_new = true;
                0
            }
            (true, true) => {
                if !duplicate(cands[0].pt, cands[cands.len() - 1].pt) {
                    fuse_prior(&mut cands, last.0[cur].orig_edge);
                    cands.len() - 1
                } else {
                    if cands.len() > 1 {
                        cands.pop();
                    }
                    cands[0].originating = None;
                    cands[0].needs_new = true;
                    let target = dst[cur - 1];
                    let mut i = cur - 1;
                    loop {
                        if dst[i] != target {
                            break;
                        }
                        dst[i] = 0;
                        if i == 0 {
                            break;
                        }
                        i -= 1;
                    }
                    0
                }
            }
        };
        for c in &cands {
            let idx = match c.originating {
                Some(o) if !(c.needs_new || force_new) => {
                    self.pts[o] = c.pt;
                    self.coverage[o] = target_coverage;
                    o
                }
                _ => {
                    let cov = coverage_at(depth, initial_depth, initial_coverage, target_depth, target_coverage);
                    self.add_pt(c.pt, cov, c.originating.is_some(), Curve::Sharp)
                }
            };
            next.0.push(RingPt { index: idx, orig_edge: c.orig_edge, ..RingPt::default() });
        }
        for d in dst.iter_mut() {
            *d = next.0[*d].index;
        }
        for i in 0..n {
            let nx = (i + 1) % n;
            self.add_tri(last.0[i].index, last.0[nx].index, dst[nx]);
            self.add_tri(last.0[i].index, dst[nx], dst[i]);
        }
        if next.0.len() < 3 {
            done = true;
        }
        done
    }
}

fn fuse_prior(cands: &mut [Candidate], orig_edge: usize) {
    let last = cands.len() - 1;
    cands[last].orig_edge = orig_edge;
    cands[last].originating = None;
    cands[last].needs_new = true;
}
