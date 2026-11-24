//! Polygon triangulation for simple and compound [`Shape`](super::Shape) fills.
//!
//! Simple polygons are ear-clipped directly. Compound paths (multiple subpaths, holes, islands,
//! and self-intersecting or overlapping contours) are classified under [`FillRule`], resolved via
//! planar arrangement when edges cross or touch, stitched with mutually-visible Eberly bridges,
//! and ear-clipped with only true contour edges marked in the 3-bit antialiasing mask.

use rustc_hash::FxHashSet;

use super::FillRule;
use super::planar::{
    self, cross2, extract_boundary_loops, has_intersections_or_touches, is_filled, loop_area2,
    scale_eps, signed_area2, winding_number, winding_number_indexed,
};

/// Ear-clips a simple polygon. Each triangle is three vertex indices and a mask of which of its
/// edges (a→b, b→c, c→a as bits 0, 1, 2) lie on the polygon's boundary.
#[doc(hidden)]
pub fn triangulate(points: &[[f32; 2]]) -> Vec<([usize; 3], u32)> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    let ccw = area > 0.0;
    let cross = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
        let v = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if ccw { v } else { -v }
    };
    let boundary = |i: usize, j: usize| (i + 1) % n == j || (j + 1) % n == i;
    let mut ring: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let mut misses = 0;
    let mut i = 0;
    while ring.len() > 3 {
        let m = ring.len();
        let (ia, ib, ic) = (ring[(i + m - 1) % m], ring[i % m], ring[(i + 1) % m]);
        let (a, b, c) = (points[ia], points[ib], points[ic]);
        let convex = cross(a, b, c) > 0.0;
        let empty = convex
            && ring.iter().all(|&k| {
                k == ia || k == ib || k == ic || {
                    let p = points[k];
                    !(cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0)
                }
            });
        // Degenerate (collinear) vertices are dropped once no ear is left.
        if empty || (misses >= m && cross(a, b, c) == 0.0) {
            if empty {
                out.push(([ia, ib, ic], mask(boundary, ia, ib, ic)));
            }
            ring.remove(i % m);
            misses = 0;
            i %= ring.len();
        } else {
            misses += 1;
            if misses > 2 * m {
                // Not simple: fall back to a fan over what is left.
                for k in 1..ring.len() - 1 {
                    let (x, y, z) = (ring[0], ring[k], ring[k + 1]);
                    out.push(([x, y, z], mask(boundary, x, y, z)));
                }
                return out;
            }
            i = (i + 1) % m;
        }
    }
    let (a, b, c) = (ring[0], ring[1], ring[2]);
    out.push(([a, b, c], mask(boundary, a, b, c)));
    out
}

/// Triangulates one or more closed contours together under `rule`, supporting transparent holes,
/// nested islands, and self-intersecting or overlapping subpaths.
///
/// Returns the vertex buffer and a list of `(triangle_indices, aa_edge_mask)` pairs. Only true
/// filled/unfilled contour edges are marked in the 3-bit `aa_edge_mask`; bridge edges and interior
/// diagonals are always unmarked (`0`) so adjacent triangles share hard edges without seams.
#[doc(hidden)]
pub fn triangulate_compound(
    contours: &[Vec<[f32; 2]>],
    rule: FillRule,
) -> (Vec<[f32; 2]>, Vec<([usize; 3], u32)>) {
    let clean = sanitize_contours(contours);
    if clean.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let eps = scale_eps(&clean);
    let loops = if !has_intersections_or_touches(&clean, eps) {
        // Fast path: single simple contour with no self-intersections.
        if clean.len() == 1 {
            let pts: Vec<[f32; 2]> = clean[0].iter().map(|&[x, y]| [x as f32, y as f32]).collect();
            let tris = triangulate(&pts);
            return (pts, tris);
        }
        classify_disjoint_contours(&clean, rule)
    } else {
        extract_boundary_loops(&clean, rule, eps)
    };

    if loops.outers.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Attach each hole to its immediate enclosing outer loop (smallest positive area).
    let outer_areas: Vec<f64> = loops
        .outers
        .iter()
        .map(|o| loop_area2(&loops.vertices, o))
        .collect();
    let outer_boxes: Vec<[f64; 4]> = loops
        .outers
        .iter()
        .map(|o| loop_bbox(&loops.vertices, o))
        .collect();
    let mut holes_of: Vec<Vec<Vec<usize>>> = vec![Vec::new(); loops.outers.len()];

    for hole in loops.holes {
        if hole.len() < 3 {
            continue;
        }
        let a = loops.vertices[hole[0]];
        let b = loops.vertices[hole[1]];
        let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        let mut parent: Option<(usize, f64)> = None;
        for (oi, outer) in loops.outers.iter().enumerate() {
            let bb = outer_boxes[oi];
            if mid[0] < bb[0] - eps || mid[0] > bb[2] + eps || mid[1] < bb[1] - eps || mid[1] > bb[3] + eps {
                continue;
            }
            if winding_number_indexed(&loops.vertices, outer, mid) != 0 {
                let area = outer_areas[oi];
                if parent.is_none_or(|(_, best)| area < best) {
                    parent = Some((oi, area));
                }
            }
        }
        if let Some((oi, _)) = parent {
            holes_of[oi].push(hole);
        }
    }

    let mut triangles = Vec::new();
    for (oi, outer) in loops.outers.into_iter().enumerate() {
        let mut boundary_edges = FxHashSet::default();
        insert_loop_edges(&mut boundary_edges, &outer);
        let holes = std::mem::take(&mut holes_of[oi]);
        for h in &holes {
            insert_loop_edges(&mut boundary_edges, h);
        }
        let has_holes = !holes.is_empty();
        let mut ring = outer;
        if has_holes {
            stitch_holes(&loops.vertices, &mut ring, holes, eps);
        }
        ear_clip_ring(&loops.vertices, ring, &mut boundary_edges, has_holes, eps, &mut triangles);
    }

    let verts: Vec<[f32; 2]> = loops
        .vertices
        .into_iter()
        .map(|[x, y]| [x as f32, y as f32])
        .collect();
    (verts, triangles)
}

/// Removes non-finite points, consecutive duplicates, closing duplicates, and zero-area contours.
fn sanitize_contours(contours: &[Vec<[f32; 2]>]) -> Vec<Vec<[f64; 2]>> {
    let mut raw: Vec<Vec<[f64; 2]>> = Vec::with_capacity(contours.len());
    for c in contours {
        if c.len() < 3 || c.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            continue;
        }
        let pts: Vec<[f64; 2]> = c.iter().map(|&[x, y]| [f64::from(x), f64::from(y)]).collect();
        raw.push(pts);
    }
    if raw.is_empty() {
        return Vec::new();
    }
    let eps = scale_eps(&raw);
    let eps2 = eps * eps;
    let mut out = Vec::with_capacity(raw.len());
    for pts in raw {
        let mut dedup: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
        for p in pts {
            if let Some(&prev) = dedup.last()
                && dist2(prev, p) <= eps2
            {
                continue;
            }
            dedup.push(p);
        }
        while dedup.len() >= 2 && dist2(dedup[0], *dedup.last().unwrap()) <= eps2 {
            dedup.pop();
        }
        if dedup.len() >= 3 && signed_area2(&dedup).abs() > eps2 {
            out.push(dedup);
        }
    }
    out
}

/// Classifies non-intersecting contours into positive-area outers and negative-area holes under
/// `rule`, dropping contours that have filled (or unfilled) regions on both sides.
fn classify_disjoint_contours(contours: &[Vec<[f64; 2]>], rule: FillRule) -> planar::Loops {
    let areas: Vec<f64> = contours.iter().map(|c| signed_area2(c)).collect();
    let mut vertices = Vec::new();
    let mut outers = Vec::new();
    let mut holes = Vec::new();

    for (i, c) in contours.iter().enumerate() {
        let probe = c[0];
        let w_out: i32 = contours
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != i)
            .map(|(_, other)| winding_number(other, probe))
            .sum();
        let sign = if areas[i] > 0.0 { 1 } else { -1 };
        let f_out = is_filled(w_out, rule);
        let f_in = is_filled(w_out + sign, rule);
        if f_out == f_in {
            continue;
        }
        let base = vertices.len();
        vertices.extend_from_slice(c);
        let mut ring: Vec<usize> = (base..base + c.len()).collect();
        if !f_out && f_in {
            // Outer boundary: ensure positive signed area (interior on the left).
            if areas[i] < 0.0 {
                ring.reverse();
            }
            outers.push(ring);
        } else {
            // Hole boundary: ensure negative signed area (filled exterior on the left).
            if areas[i] > 0.0 {
                ring.reverse();
            }
            holes.push(ring);
        }
    }

    planar::Loops { vertices, outers, holes }
}

/// Splices every hole into `ring` via mutually-visible Eberly bridge edges, sorted rightmost first.
fn stitch_holes(verts: &[[f64; 2]], ring: &mut Vec<usize>, holes: Vec<Vec<usize>>, eps: f64) {
    let mut keyed: Vec<(f64, f64, usize, Vec<usize>)> = holes
        .into_iter()
        .map(|h| {
            let (mi, mp) = rightmost_vertex(verts, &h);
            (mp[0], mp[1], mi, h)
        })
        .collect();
    // Rightmost hole first (largest x; tie-break smallest y).
    keyed.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.total_cmp(&b.1)));

    for (_, _, m_idx, hole) in keyed {
        let m_vert = hole[m_idx];
        let h_len = hole.len();
        let h_next = verts[hole[(m_idx + 1) % h_len]];
        let Some(r_idx) = find_bridge_vertex(verts, ring, m_vert, h_next, eps) else {
            continue;
        };
        let p_vert = ring[r_idx];
        let mut splice = Vec::with_capacity(h_len + 2);
        if p_vert == m_vert || dist2(verts[p_vert], verts[m_vert]) <= eps * eps {
            // Touching hole: splice directly at the shared vertex without a zero-length bridge.
            for step in 1..=h_len {
                splice.push(hole[(m_idx + step) % h_len]);
            }
        } else {
            // Bridge P -> M, walk around hole back to M, bridge M -> P.
            for step in 0..=h_len {
                splice.push(hole[(m_idx + step) % h_len]);
            }
            splice.push(p_vert);
        }
        ring.splice(r_idx + 1..r_idx + 1, splice);
    }
}

/// Returns `(index_in_hole, coords)` of the vertex with maximum x (tie-break minimum y).
fn rightmost_vertex(verts: &[[f64; 2]], hole: &[usize]) -> (usize, [f64; 2]) {
    let mut best_i = 0;
    let mut best_p = verts[hole[0]];
    for (i, &vi) in hole.iter().enumerate().skip(1) {
        let p = verts[vi];
        if p[0] > best_p[0] || (p[0] == best_p[0] && p[1] < best_p[1]) {
            best_i = i;
            best_p = p;
        }
    }
    (best_i, best_p)
}

/// Finds the index in `ring` of a vertex mutually visible from `verts[m_vert]` using David
/// Eberly's horizontal rightward ray algorithm.
fn find_bridge_vertex(
    verts: &[[f64; 2]],
    ring: &[usize],
    m_vert: usize,
    hole_next: [f64; 2],
    eps: f64,
) -> Option<usize> {
    let m = verts[m_vert];
    let n = ring.len();
    let eps2 = eps * eps;

    // Case 1: M already coincides with a vertex of ring.
    let mut touch_fallback = None;
    for i in 0..n {
        if ring[i] == m_vert || dist2(verts[ring[i]], m) <= eps2 {
            let prev = verts[ring[(i + n - 1) % n]];
            let curr = verts[ring[i]];
            let next = verts[ring[(i + 1) % n]];
            if in_cone(prev, curr, next, hole_next) {
                return Some(i);
            }
            touch_fallback = Some(i);
        }
    }
    if touch_fallback.is_some() {
        return touch_fallback;
    }

    // Case 2: Cast a +x ray from M and find the closest intersection with a right-facing edge.
    let mut best_x = f64::INFINITY;
    let mut best_edge: Option<(usize, usize)> = None;

    for i in 0..n {
        let j = (i + 1) % n;
        let a = verts[ring[i]];
        let b = verts[ring[j]];
        if a[1] == m[1] && b[1] == m[1] {
            // Horizontal edge lying on the ray line.
            let hi_x = a[0].max(b[0]);
            if hi_x >= m[0] - eps {
                let x_hit = a[0].min(b[0]).max(m[0]);
                if x_hit < best_x - eps {
                    best_x = x_hit;
                    best_edge = Some((i, j));
                }
            }
            continue;
        }
        // For a positive-area ring, right-facing edges cross y = m.y upward (a.y <= m.y <= b.y).
        if a[1] <= m[1] && b[1] >= m[1] && a[1] < b[1] {
            let t = (m[1] - a[1]) / (b[1] - a[1]);
            let x_hit = a[0] + t * (b[0] - a[0]);
            if x_hit >= m[0] - eps && x_hit < best_x - eps {
                best_x = x_hit;
                best_edge = Some((i, j));
            }
        }
    }

    let (ei, ej) = best_edge?;
    let a = verts[ring[ei]];
    let b = verts[ring[ej]];
    let hit = [best_x, m[1]];

    // Direct hit on an endpoint of the closest edge.
    if dist2(hit, a) <= eps2 {
        return Some(pick_cone_occurrence(verts, ring, ei, m, eps2));
    }
    if dist2(hit, b) <= eps2 {
        return Some(pick_cone_occurrence(verts, ring, ej, m, eps2));
    }

    // Otherwise, candidate P is the endpoint of the hit edge with larger x.
    let p_idx = if a[0] >= b[0] { ei } else { ej };
    let p = verts[ring[p_idx]];

    // Check if any reflex (or flat) vertex of ring lies inside or on triangle (M, hit, P).
    let mut best_reflex: Option<(usize, f64, f64)> = None;
    for k in 0..n {
        let q = verts[ring[k]];
        if ring[k] == ring[p_idx] || dist2(q, p) <= eps2 || dist2(q, m) <= eps2 {
            continue;
        }
        let prev = verts[ring[(k + n - 1) % n]];
        let next = verts[ring[(k + 1) % n]];
        if cross2(prev, q, next) > eps2 {
            // Strictly convex vertex cannot block visibility inside (M, hit, P).
            continue;
        }
        if !point_in_triangle_any_winding(q, m, hit, p, eps) {
            continue;
        }
        if !in_cone(prev, q, next, m) {
            continue;
        }
        let dx = (q[0] - m[0]).max(0.0);
        let dy = (q[1] - m[1]).abs();
        let tan = if dx > 1e-12 { dy / dx } else { f64::INFINITY };
        let d2 = dist2(m, q);
        let better = match best_reflex {
            None => true,
            Some((_, b_tan, b_d2)) => tan < b_tan - 1e-12 || ((tan - b_tan).abs() <= 1e-12 && d2 < b_d2),
        };
        if better {
            best_reflex = Some((k, tan, d2));
        }
    }

    let chosen = best_reflex.map_or(p_idx, |(k, _, _)| k);
    Some(pick_cone_occurrence(verts, ring, chosen, m, eps2))
}

/// When a vertex appears more than once in `ring` (from an earlier bridge), selects the occurrence
/// whose local interior cone contains `target`.
fn pick_cone_occurrence(
    verts: &[[f64; 2]],
    ring: &[usize],
    fallback: usize,
    target: [f64; 2],
    eps2: f64,
) -> usize {
    let n = ring.len();
    let v = ring[fallback];
    let pv = verts[v];
    for i in 0..n {
        if ring[i] == v || dist2(verts[ring[i]], pv) <= eps2 {
            let prev = verts[ring[(i + n - 1) % n]];
            let next = verts[ring[(i + 1) % n]];
            if in_cone(prev, verts[ring[i]], next, target) {
                return i;
            }
        }
    }
    fallback
}

/// Whether `target` lies inside the local interior cone of vertex `p` in a positive-area ring.
fn in_cone(prev: [f64; 2], p: [f64; 2], next: [f64; 2], target: [f64; 2]) -> bool {
    if cross2(prev, p, next) >= 0.0 {
        cross2(prev, p, target) >= 0.0 && cross2(p, next, target) >= 0.0
    } else {
        !(cross2(next, p, target) > 0.0 && cross2(p, prev, target) > 0.0)
    }
}

/// Inclusive point-in-triangle test independent of `(a, b, c)` orientation.
fn point_in_triangle_any_winding(
    q: [f64; 2],
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    eps: f64,
) -> bool {
    let area2 = cross2(a, b, c);
    let tol = eps * eps;
    if area2.abs() <= tol {
        // Degenerate triangle (e.g. horizontal M -> hit -> P): check bounding box + collinearity.
        let min_x = a[0].min(b[0]).min(c[0]) - eps;
        let max_x = a[0].max(b[0]).max(c[0]) + eps;
        let min_y = a[1].min(b[1]).min(c[1]) - eps;
        let max_y = a[1].max(b[1]).max(c[1]) + eps;
        return q[0] >= min_x && q[0] <= max_x && q[1] >= min_y && q[1] <= max_y;
    }
    let sign = if area2 > 0.0 { 1.0 } else { -1.0 };
    cross2(a, b, q) * sign >= -tol
        && cross2(b, c, q) * sign >= -tol
        && cross2(c, a, q) * sign >= -tol
}

/// Ear-clips a positive-area ring (possibly containing duplicate bridge vertices) while marking
/// only edges present in `boundary_edges` in the 3-bit antialiasing mask.
fn ear_clip_ring(
    verts: &[[f64; 2]],
    mut ring: Vec<usize>,
    boundary_edges: &mut FxHashSet<(usize, usize)>,
    has_bridges: bool,
    eps: f64,
    out: &mut Vec<([usize; 3], u32)>,
) {
    let eps2 = eps * eps;
    let mut misses = 0usize;
    let mut i = 0usize;

    while ring.len() > 3 {
        let m = ring.len();
        let idx_prev = (i + m - 1) % m;
        let idx_curr = i % m;
        let idx_next = (i + 1) % m;
        let (ia, ib, ic) = (ring[idx_prev], ring[idx_curr], ring[idx_next]);

        // Drop consecutive duplicate vertices or zero-area collapsed bridge flaps (A -> B -> A).
        if ia == ib || ib == ic {
            ring.remove(idx_curr);
            misses = 0;
            i %= ring.len();
            continue;
        }
        if ia == ic {
            ring.remove(idx_curr);
            misses = 0;
            i %= ring.len();
            continue;
        }

        let (a, b, c) = (verts[ia], verts[ib], verts[ic]);
        let area2 = cross2(a, b, c);
        let strict_inside = misses < 2 * m;

        let is_ear = area2 > 0.0 && {
            let no_point_inside = ring.iter().all(|&k| {
                if k == ia || k == ib || k == ic {
                    return true;
                }
                let p = verts[k];
                if dist2(p, a) <= eps2 || dist2(p, b) <= eps2 || dist2(p, c) <= eps2 {
                    return true;
                }
                if strict_inside {
                    !(cross2(a, b, p) >= 0.0 && cross2(b, c, p) >= 0.0 && cross2(c, a, p) >= 0.0)
                } else {
                    !(cross2(a, b, p) > eps2 && cross2(b, c, p) > eps2 && cross2(c, a, p) > eps2)
                }
            });
            no_point_inside && (!has_bridges || !diagonal_crosses_ring(verts, &ring, idx_prev, idx_next, eps2))
        };

        if is_ear {
            out.push(([ia, ib, ic], edge_mask(boundary_edges, ia, ib, ic)));
            ring.remove(idx_curr);
            misses = 0;
            i %= ring.len();
        } else if misses >= m && area2.abs() <= eps2 {
            // Drop collinear vertex; preserve boundary AA if both adjacent edges were boundaries.
            if boundary_edges.contains(&canon(ia, ib)) && boundary_edges.contains(&canon(ib, ic)) {
                boundary_edges.insert(canon(ia, ic));
            }
            ring.remove(idx_curr);
            misses = 0;
            i %= ring.len();
        } else {
            misses += 1;
            if misses > 3 * m {
                for k in 1..ring.len() - 1 {
                    let (x, y, z) = (ring[0], ring[k], ring[k + 1]);
                    if x != y && y != z && x != z && cross2(verts[x], verts[y], verts[z]).abs() > eps2 {
                        out.push(([x, y, z], edge_mask(boundary_edges, x, y, z)));
                    }
                }
                return;
            }
            i = (i + 1) % m;
        }
    }

    if ring.len() == 3 {
        let (a, b, c) = (ring[0], ring[1], ring[2]);
        if a != b && b != c && a != c && cross2(verts[a], verts[b], verts[c]).abs() > eps2 {
            out.push(([a, b, c], edge_mask(boundary_edges, a, b, c)));
        }
    }
}

/// Returns true if the chord `(ring[ia_pos], ring[ic_pos])` properly crosses any ring edge or
/// leaves the local interior cone at either endpoint.
fn diagonal_crosses_ring(
    verts: &[[f64; 2]],
    ring: &[usize],
    ia_pos: usize,
    ic_pos: usize,
    eps2: f64,
) -> bool {
    let m = ring.len();
    let ia = ring[ia_pos];
    let ic = ring[ic_pos];
    let a = verts[ia];
    let c = verts[ic];
    let a_prev = verts[ring[(ia_pos + m - 1) % m]];
    let a_next = verts[ring[(ia_pos + 1) % m]];
    let c_prev = verts[ring[(ic_pos + m - 1) % m]];
    let c_next = verts[ring[(ic_pos + 1) % m]];
    if !in_cone(a_prev, a, a_next, c) || !in_cone(c_prev, c, c_next, a) {
        return true;
    }
    for k in 0..m {
        let k2 = (k + 1) % m;
        let u = ring[k];
        let v = ring[k2];
        if u == ia || u == ic || v == ia || v == ic {
            continue;
        }
        let pu = verts[u];
        let pv = verts[v];
        if dist2(pu, a) <= eps2 || dist2(pu, c) <= eps2 || dist2(pv, a) <= eps2 || dist2(pv, c) <= eps2 {
            continue;
        }
        let d1 = cross2(a, c, pu);
        let d2 = cross2(a, c, pv);
        let d3 = cross2(pu, pv, a);
        let d4 = cross2(pu, pv, c);
        if ((d1 > eps2 && d2 < -eps2) || (d1 < -eps2 && d2 > eps2))
            && ((d3 > eps2 && d4 < -eps2) || (d3 < -eps2 && d4 > eps2))
        {
            return true;
        }
    }
    false
}

#[inline]
fn canon(a: usize, b: usize) -> (usize, usize) {
    if a < b { (a, b) } else { (b, a) }
}

fn insert_loop_edges(set: &mut FxHashSet<(usize, usize)>, ring: &[usize]) {
    let n = ring.len();
    for i in 0..n {
        set.insert(canon(ring[i], ring[(i + 1) % n]));
    }
}

fn edge_mask(boundary_edges: &FxHashSet<(usize, usize)>, a: usize, b: usize, c: usize) -> u32 {
    (boundary_edges.contains(&canon(a, b)) as u32)
        | ((boundary_edges.contains(&canon(b, c)) as u32) << 1)
        | ((boundary_edges.contains(&canon(c, a)) as u32) << 2)
}

fn mask(boundary: impl Fn(usize, usize) -> bool, a: usize, b: usize, c: usize) -> u32 {
    (boundary(a, b) as u32) | ((boundary(b, c) as u32) << 1) | ((boundary(c, a) as u32) << 2)
}

fn loop_bbox(verts: &[[f64; 2]], ring: &[usize]) -> [f64; 4] {
    let mut bb = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for &vi in ring {
        let p = verts[vi];
        bb[0] = bb[0].min(p[0]);
        bb[1] = bb[1].min(p[1]);
        bb[2] = bb[2].max(p[0]);
        bb[3] = bb[3].max(p[1]);
    }
    bb
}

#[inline]
fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}
