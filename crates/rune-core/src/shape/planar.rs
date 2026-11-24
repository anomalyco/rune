//! Planar arrangement for self-intersecting and overlapping compound shape contours.
//!
//! Splits intersecting segments at every crossing or collinear overlap, merges coincident vertices
//! and cancelling opposite edges, evaluates exact left/right winding numbers without epsilon
//! perturbation, and extracts non-crossing outer (positive area) and hole (negative area) boundary
//! loops whose left side is filled under the requested [`FillRule`].

use rustc_hash::{FxHashMap, FxHashSet};

use super::FillRule;

/// Result of extracting filled boundary loops from one or more contours.
pub(super) struct Loops {
    pub(super) vertices: Vec<[f64; 2]>,
    /// Outer boundary loops (positive signed area; interior on the left).
    pub(super) outers: Vec<Vec<usize>>,
    /// Hole boundary loops (negative signed area; filled exterior on the left).
    pub(super) holes: Vec<Vec<usize>>,
}

/// Signed 2D cross product `(b - a) × (c - a)`.
#[inline]
pub(super) fn cross2(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Twice the signed area of a closed polygon (positive when interior lies on the left of edges).
pub(super) fn signed_area2(pts: &[[f64; 2]]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// Twice the signed area of an indexed closed loop.
pub(super) fn loop_area2(verts: &[[f64; 2]], ring: &[usize]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    (0..n)
        .map(|i| {
            let (a, b) = (verts[ring[i]], verts[ring[(i + 1) % n]]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// Whether a winding number is considered filled under `rule`.
#[inline]
pub(super) fn is_filled(wn: i32, rule: FillRule) -> bool {
    match rule {
        FillRule::NonZero => wn != 0,
        FillRule::EvenOdd => (wn & 1) != 0,
    }
}

/// Winding number of `q` with respect to a single closed polygon using Dan Sunday's half-open
/// horizontal ray rule (`a.y <= q.y < b.y`).
pub(super) fn winding_number(poly: &[[f64; 2]], q: [f64; 2]) -> i32 {
    let n = poly.len();
    let mut wn = 0i32;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if a[1] <= q[1] {
            if b[1] > q[1] && cross2(a, b, q) > 0.0 {
                wn += 1;
            }
        } else if b[1] <= q[1] && cross2(a, b, q) < 0.0 {
            wn -= 1;
        }
    }
    wn
}

/// Winding number of `q` with respect to an indexed closed loop.
pub(super) fn winding_number_indexed(verts: &[[f64; 2]], ring: &[usize], q: [f64; 2]) -> i32 {
    let n = ring.len();
    let mut wn = 0i32;
    for i in 0..n {
        let a = verts[ring[i]];
        let b = verts[ring[(i + 1) % n]];
        if a[1] <= q[1] {
            if b[1] > q[1] && cross2(a, b, q) > 0.0 {
                wn += 1;
            }
        } else if b[1] <= q[1] && cross2(a, b, q) < 0.0 {
            wn -= 1;
        }
    }
    wn
}

struct Seg {
    a: [f64; 2],
    b: [f64; 2],
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
    contour: usize,
    edge: usize,
    contour_len: usize,
}

fn build_segs(contours: &[Vec<[f64; 2]>]) -> Vec<Seg> {
    let total: usize = contours.iter().map(Vec::len).sum();
    let mut segs = Vec::with_capacity(total);
    for (ci, c) in contours.iter().enumerate() {
        let n = c.len();
        for ei in 0..n {
            let a = c[ei];
            let b = c[(ei + 1) % n];
            segs.push(Seg {
                a,
                b,
                min_x: a[0].min(b[0]),
                max_x: a[0].max(b[0]),
                min_y: a[1].min(b[1]),
                max_y: a[1].max(b[1]),
                contour: ci,
                edge: ei,
                contour_len: n,
            });
        }
    }
    segs
}

/// Scale-relative positional tolerance for `contours`.
pub(super) fn scale_eps(contours: &[Vec<[f64; 2]>]) -> f64 {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for c in contours {
        for &[x, y] in c {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    let span = (x1 - x0).max(y1 - y0).max(1.0);
    span * 1e-6
}

/// Returns true if any non-adjacent edges intersect, touch within `eps`, or any adjacent edges fold
/// back collinearly over each other.
pub(super) fn has_intersections_or_touches(contours: &[Vec<[f64; 2]>], eps: f64) -> bool {
    let segs = build_segs(contours);
    let mut order: Vec<usize> = (0..segs.len()).collect();
    order.sort_by(|&i, &j| segs[i].min_x.total_cmp(&segs[j].min_x));

    for idx_a in 0..order.len() {
        let i = order[idx_a];
        let si = &segs[i];
        for &j in &order[idx_a + 1..] {
            let sj = &segs[j];
            if sj.min_x > si.max_x + eps {
                break;
            }
            if sj.min_y > si.max_y + eps || si.min_y > sj.max_y + eps {
                continue;
            }
            if si.contour == sj.contour {
                let n = si.contour_len;
                let adj = (si.edge + 1) % n == sj.edge || (sj.edge + 1) % n == si.edge;
                if adj {
                    if adjacent_fold_back(si, sj, eps) {
                        return true;
                    }
                    continue;
                }
            }
            if segments_touch_or_cross(si.a, si.b, sj.a, sj.b, eps) {
                return true;
            }
        }
    }
    false
}

/// Whether two adjacent edges in the same contour fold back collinearly over each other.
fn adjacent_fold_back(s1: &Seg, s2: &Seg, eps: f64) -> bool {
    let (shared, other1, other2) = if (s1.edge + 1) % s1.contour_len == s2.edge {
        (s1.b, s1.a, s2.b)
    } else {
        (s2.b, s2.a, s1.b)
    };
    let v1 = [other1[0] - shared[0], other1[1] - shared[1]];
    let v2 = [other2[0] - shared[0], other2[1] - shared[1]];
    let l1 = v1[0].hypot(v1[1]);
    let l2 = v2[0].hypot(v2[1]);
    if l1 <= eps || l2 <= eps {
        return true;
    }
    let dot = v1[0] * v2[0] + v1[1] * v2[1];
    let cross = (v1[0] * v2[1] - v1[1] * v2[0]).abs();
    dot > 0.0 && cross <= eps * l1.max(l2)
}

/// Whether segments `(a1, b1)` and `(a2, b2)` intersect or come within `eps` of each other.
fn segments_touch_or_cross(
    a1: [f64; 2],
    b1: [f64; 2],
    a2: [f64; 2],
    b2: [f64; 2],
    eps: f64,
) -> bool {
    let d1 = [b1[0] - a1[0], b1[1] - a1[1]];
    let d2 = [b2[0] - a2[0], b2[1] - a2[1]];
    let l1 = d1[0].hypot(d1[1]);
    let l2 = d2[0].hypot(d2[1]);
    if l1 <= eps || l2 <= eps {
        return true;
    }
    let denom = d1[0] * d2[1] - d1[1] * d2[0];
    if denom.abs() > 1e-12 * l1 * l2 {
        let diff = [a2[0] - a1[0], a2[1] - a1[1]];
        let t1 = (diff[0] * d2[1] - diff[1] * d2[0]) / denom;
        let t2 = (diff[0] * d1[1] - diff[1] * d1[0]) / denom;
        let dt1 = eps / l1;
        let dt2 = eps / l2;
        if t1 >= -dt1 && t1 <= 1.0 + dt1 && t2 >= -dt2 && t2 <= 1.0 + dt2 {
            return true;
        }
    }
    point_seg_dist2(a2, a1, b1) <= eps * eps
        || point_seg_dist2(b2, a1, b1) <= eps * eps
        || point_seg_dist2(a1, a2, b2) <= eps * eps
        || point_seg_dist2(b1, a2, b2) <= eps * eps
}

fn point_seg_dist2(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    if len2 == 0.0 {
        return (p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2);
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0);
    let proj = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    (p[0] - proj[0]).powi(2) + (p[1] - proj[1]).powi(2)
}

/// Spatial hash table that merges vertices within `eps`.
struct VertexPool {
    verts: Vec<[f64; 2]>,
    grid: FxHashMap<(i64, i64), Vec<usize>>,
    cell: f64,
    eps2: f64,
}

impl VertexPool {
    fn new(eps: f64) -> Self {
        Self {
            verts: Vec::new(),
            grid: FxHashMap::default(),
            cell: (eps * 2.0).max(1e-12),
            eps2: eps * eps,
        }
    }

    fn intern(&mut self, p: [f64; 2]) -> usize {
        let gx = (p[0] / self.cell).floor() as i64;
        let gy = (p[1] / self.cell).floor() as i64;
        let mut best: Option<(usize, f64)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(bucket) = self.grid.get(&(gx + dx, gy + dy)) {
                    for &idx in bucket {
                        let q = self.verts[idx];
                        let d2 = (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2);
                        if d2 <= self.eps2 && best.is_none_or(|(_, bd)| d2 < bd) {
                            best = Some((idx, d2));
                        }
                    }
                }
            }
        }
        if let Some((idx, _)) = best {
            return idx;
        }
        let idx = self.verts.len();
        self.verts.push(p);
        self.grid.entry((gx, gy)).or_default().push(idx);
        idx
    }
}

/// Extracts non-crossing filled outer and hole boundary loops via planar arrangement.
pub(super) fn extract_boundary_loops(
    contours: &[Vec<[f64; 2]>],
    rule: FillRule,
    eps: f64,
) -> Loops {
    let segs = build_segs(contours);
    let mut splits: Vec<Vec<f64>> = vec![Vec::new(); segs.len()];

    // Step 1: X-sweep AABB intersection and collinear-overlap splitting.
    let mut order: Vec<usize> = (0..segs.len()).collect();
    order.sort_by(|&i, &j| segs[i].min_x.total_cmp(&segs[j].min_x));

    for idx_a in 0..order.len() {
        let i = order[idx_a];
        let si = &segs[i];
        let d1 = [si.b[0] - si.a[0], si.b[1] - si.a[1]];
        let l1 = d1[0].hypot(d1[1]);
        if l1 <= eps {
            continue;
        }
        let dt1 = (eps / l1).min(0.49);

        for &j in &order[idx_a + 1..] {
            let sj = &segs[j];
            if sj.min_x > si.max_x + eps {
                break;
            }
            if sj.min_y > si.max_y + eps || si.min_y > sj.max_y + eps {
                continue;
            }
            let d2 = [sj.b[0] - sj.a[0], sj.b[1] - sj.a[1]];
            let l2 = d2[0].hypot(d2[1]);
            if l2 <= eps {
                continue;
            }
            let dt2 = (eps / l2).min(0.49);

            let denom = d1[0] * d2[1] - d1[1] * d2[0];
            if denom.abs() > 1e-12 * l1 * l2 {
                let diff = [sj.a[0] - si.a[0], sj.a[1] - si.a[1]];
                let t1 = (diff[0] * d2[1] - diff[1] * d2[0]) / denom;
                let t2 = (diff[0] * d1[1] - diff[1] * d1[0]) / denom;
                if t1 >= -dt1 && t1 <= 1.0 + dt1 && t2 >= -dt2 && t2 <= 1.0 + dt2 {
                    if t1 > dt1 && t1 < 1.0 - dt1 {
                        splits[i].push(t1);
                    }
                    if t2 > dt2 && t2 < 1.0 - dt2 {
                        splits[j].push(t2);
                    }
                }
            } else {
                for p in [sj.a, sj.b] {
                    if point_seg_dist2(p, si.a, si.b) <= eps * eps {
                        let t = ((p[0] - si.a[0]) * d1[0] + (p[1] - si.a[1]) * d1[1]) / (l1 * l1);
                        if t > dt1 && t < 1.0 - dt1 {
                            splits[i].push(t);
                        }
                    }
                }
                for p in [si.a, si.b] {
                    if point_seg_dist2(p, sj.a, sj.b) <= eps * eps {
                        let t = ((p[0] - sj.a[0]) * d2[0] + (p[1] - sj.a[1]) * d2[1]) / (l2 * l2);
                        if t > dt2 && t < 1.0 - dt2 {
                            splits[j].push(t);
                        }
                    }
                }
            }
        }
    }

    // Step 2: Subdivide segments, snap vertices, and aggregate net winding multiplicity per edge.
    let mut pool = VertexPool::new(eps);
    let mut edge_mult: FxHashMap<(usize, usize), i32> = FxHashMap::default();

    for (i, seg) in segs.iter().enumerate() {
        let len = (seg.b[0] - seg.a[0]).hypot(seg.b[1] - seg.a[1]);
        if len <= eps {
            continue;
        }
        let ts = &mut splits[i];
        ts.sort_by(f64::total_cmp);
        let dt = (eps / len).min(0.49);

        let mut prev_t = 0.0;
        let mut prev_v = pool.intern(seg.a);
        for &t in ts.iter() {
            if t <= prev_t + dt || t >= 1.0 - dt {
                continue;
            }
            let p = [
                seg.a[0] + (seg.b[0] - seg.a[0]) * t,
                seg.a[1] + (seg.b[1] - seg.a[1]) * t,
            ];
            let v = pool.intern(p);
            if v != prev_v {
                add_directed_edge(&mut edge_mult, prev_v, v);
                prev_v = v;
                prev_t = t;
            }
        }
        let end_v = pool.intern(seg.b);
        if end_v != prev_v {
            add_directed_edge(&mut edge_mult, prev_v, end_v);
        }
    }

    let verts = pool.verts;
    let mut edges: Vec<(usize, usize, i32)> = edge_mult
        .into_iter()
        .filter(|&(_, m)| m != 0)
        .map(|((u, v), m)| (u, v, m))
        .collect();
    edges.sort_unstable_by_key(|&(u, v, _)| (u, v));

    if edges.is_empty() {
        return Loops { vertices: verts, outers: Vec::new(), holes: Vec::new() };
    }

    // Step 3: Exact midpoint winding numbers on the left and right of each elementary edge.
    let mut dir_edges: Vec<(usize, usize)> = Vec::new();
    for (idx, &(u, v, k)) in edges.iter().enumerate() {
        let a = verts[u];
        let b = verts[v];
        let m = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];

        let mut w_other = 0i32;
        for (j, &(eu, ev, ek)) in edges.iter().enumerate() {
            if j == idx {
                continue;
            }
            let ea = verts[eu];
            let eb = verts[ev];
            if (ea[1] > m[1] && eb[1] > m[1]) || (ea[1] <= m[1] && eb[1] <= m[1]) {
                continue;
            }
            if ea[0].max(eb[0]) < m[0] {
                continue;
            }
            if ea[1] <= m[1] && eb[1] > m[1] {
                if cross2(ea, eb, m) > 0.0 {
                    w_other += ek;
                }
            } else if eb[1] <= m[1] && ea[1] > m[1] && cross2(ea, eb, m) < 0.0 {
                w_other -= ek;
            }
        }

        let (w_left, w_right) = if a[1] == b[1] {
            if a[0] < b[0] { (w_other, w_other - k) } else { (w_other + k, w_other) }
        } else if a[1] < b[1] {
            (w_other + k, w_other)
        } else {
            (w_other, w_other - k)
        };

        let fl = is_filled(w_left, rule);
        let fr = is_filled(w_right, rule);
        if fl && !fr {
            dir_edges.push((u, v));
        } else if !fl && fr {
            dir_edges.push((v, u));
        }
    }

    if dir_edges.is_empty() {
        return Loops { vertices: verts, outers: Vec::new(), holes: Vec::new() };
    }

    // Step 4: Trace minimal non-crossing cycles with filled region on the left.
    let loops = trace_cycles(&verts, &dir_edges);
    let mut outers = Vec::new();
    let mut holes = Vec::new();
    let min_area2 = eps * eps;
    for ring in loops {
        if ring.len() < 3 {
            continue;
        }
        let area2 = loop_area2(&verts, &ring);
        if area2 > min_area2 {
            outers.push(ring);
        } else if area2 < -min_area2 {
            holes.push(ring);
        }
    }

    Loops { vertices: verts, outers, holes }
}

fn add_directed_edge(map: &mut FxHashMap<(usize, usize), i32>, u: usize, v: usize) {
    if u < v {
        *map.entry((u, v)).or_default() += 1;
    } else {
        *map.entry((v, u)).or_default() -= 1;
    }
}

/// Pairs incoming and outgoing directed boundary edges at each vertex by turning as sharp-left as
/// possible (smallest sweep across the filled interior wedge), then extracts every cycle.
fn trace_cycles(verts: &[[f64; 2]], dir_edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let e_count = dir_edges.len();
    let mut incident: FxHashMap<usize, Vec<(bool, usize, f64)>> = FxHashMap::default();
    for (ei, &(u, v)) in dir_edges.iter().enumerate() {
        let pu = verts[u];
        let pv = verts[v];
        // Incoming at v: ray points back toward u.
        let ang_in = (pu[1] - pv[1]).atan2(pu[0] - pv[0]);
        incident.entry(v).or_default().push((false, ei, ang_in));
        // Outgoing at u: ray points forward toward v.
        let ang_out = (pv[1] - pu[1]).atan2(pv[0] - pu[0]);
        incident.entry(u).or_default().push((true, ei, ang_out));
    }

    let mut next_edge: Vec<Option<usize>> = vec![None; e_count];
    for (_, mut list) in incident {
        if list.len() == 2 {
            let (in_e, out_e) = if !list[0].0 && list[1].0 {
                (Some(list[0].1), Some(list[1].1))
            } else if list[0].0 && !list[1].0 {
                (Some(list[1].1), Some(list[0].1))
            } else {
                (None, None)
            };
            if let (Some(i), Some(o)) = (in_e, out_e) {
                next_edge[i] = Some(o);
            }
            continue;
        }
        // Sort in descending angle order so the filled wedge on the left of an incoming edge is
        // swept by advancing forward in the sorted ring to the next outgoing edge.
        list.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        let m = list.len();
        let mut taken_out: FxHashSet<usize> = FxHashSet::default();
        for idx in 0..m {
            if list[idx].0 {
                continue;
            }
            let in_e = list[idx].1;
            for step in 1..m {
                let cand = list[(idx + step) % m];
                if cand.0 && taken_out.insert(cand.1) {
                    next_edge[in_e] = Some(cand.1);
                    break;
                }
            }
        }
    }

    let mut visited = vec![false; e_count];
    let mut out = Vec::new();
    for start in 0..e_count {
        if visited[start] {
            continue;
        }
        let mut cycle = Vec::new();
        let mut cur = start;
        let mut closed = false;
        while !visited[cur] {
            visited[cur] = true;
            cycle.push(dir_edges[cur].0);
            let Some(nxt) = next_edge[cur] else { break };
            if nxt == start {
                closed = true;
                break;
            }
            cur = nxt;
        }
        if closed && cycle.len() >= 3 {
            out.push(cycle);
        }
    }
    out
}
