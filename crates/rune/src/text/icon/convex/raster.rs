//! Triangles covered as a D3D11 GPU rasterizes them: corners snapped to 1/256 of a pixel, a
//! pixel drawn where its centre is inside (on an edge, only a top or left one), its attributes
//! interpolated there, and each triangle blended over what's below.

use super::tess::Mesh;
use super::fill::EdgeMesh;

/// Covers `out` (`px` × `px`, cleared first) with `mesh`, its coverage interpolated, whose
/// points are `origin` device pixels off the canvas's.
pub(in super::super) fn fill_triangles(mesh: &Mesh, origin: [f32; 2], px: u32, out: &mut [u8]) {
    let pts: Vec<[f32; 2]> = mesh.pts.iter().map(|p| [p.x, p.y]).collect();
    rasterize(&pts, &mesh.indices, origin, px, out, |tri, w| (0..3).map(|k| w[k] * mesh.coverage[tri[k]] as f64).sum::<f64>() as f32);
}

/// Covers `out` with `mesh` as Ganesh's `QuadEdgeEffect` shades it: per pixel, the edge's
/// implicit quadratic `u² - v` over its gradient (the varyings' screen derivatives, which are
/// constant across a triangle), or, between a curve's tangent lines, their distance.
pub(in super::super) fn fill_quad_edges(mesh: &EdgeMesh, origin: [f32; 2], px: u32, out: &mut [u8]) {
    let pts: Vec<[f32; 2]> = mesh.verts.iter().map(|v| v.pos).collect();
    rasterize(&pts, &mesh.indices, origin, px, out, |tri, w| {
        let at = |k: usize| (0..3).map(|i| w[i] * mesh.verts[tri[i]].edge[k] as f64).sum::<f64>() as f32;
        let e = [at(0), at(1), at(2), at(3)];
        if e[2] > 0.0 && e[3] > 0.0 {
            return (e[2].min(e[3]) + 0.5).min(1.0);
        }
        let (du, dv) = (gradient(&pts, tri, |i| mesh.verts[i].edge[0], origin), gradient(&pts, tri, |i| mesh.verts[i].edge[1], origin));
        let g = [2.0 * e[0] * du[0] - dv[0], 2.0 * e[0] * du[1] - dv[1]];
        let f = e[0] * e[0] - e[1];
        (0.5 - f / g[0].hypot(g[1]).max(1e-6)).clamp(0.0, 1.0)
    });
}

/// How the attribute `a` of triangle `tri` changes per device pixel in x and in y.
pub(super) fn gradient(pts: &[[f32; 2]], tri: [usize; 3], a: impl Fn(usize) -> f32, origin: [f32; 2]) -> [f32; 2] {
    let p = tri.map(|i| [((pts[i][0] - origin[0]) * 256.0).round() as f64 / 256.0, ((pts[i][1] - origin[1]) * 256.0).round() as f64 / 256.0]);
    let v = tri.map(|i| a(i) as f64);
    let det = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
    if det == 0.0 {
        return [0.0, 0.0];
    }
    let dx = ((v[1] - v[0]) * (p[2][1] - p[0][1]) - (v[2] - v[0]) * (p[1][1] - p[0][1])) / det;
    let dy = ((v[2] - v[0]) * (p[1][0] - p[0][0]) - (v[1] - v[0]) * (p[2][0] - p[0][0])) / det;
    [dx as f32, dy as f32]
}

/// Each triangle of `indices` over `pts` (device pixels, `origin` off the canvas), shaded by
/// `shade(triangle, barycentric weights)` and blended over.
pub(super) fn rasterize(pts: &[[f32; 2]], indices: &[usize], origin: [f32; 2], px: u32, out: &mut [u8], shade: impl Fn([usize; 3], [f64; 3]) -> f32) {
    let n = px as usize;
    let mut acc = vec![0f32; n * n];
    // Fixed point, 8 bits of subpixel.
    let snap = |i: usize| [((pts[i][0] - origin[0]) * 256.0).round() as i64, ((pts[i][1] - origin[1]) * 256.0).round() as i64];
    for t in indices.as_chunks::<3>().0 {
        let mut tri = [t[0], t[1], t[2]];
        let (mut a, mut b, c) = (snap(tri[0]), snap(tri[1]), snap(tri[2]));
        let mut area = edge(a, b, c);
        if area == 0 {
            continue;
        }
        // Clockwise on screen (y down), so the inside is where every edge function is positive.
        if area < 0 {
            std::mem::swap(&mut a, &mut b);
            tri.swap(0, 1);
            area = -area;
        }
        let x0 = (a[0].min(b[0]).min(c[0]) >> 8).max(0);
        let x1 = ((a[0].max(b[0]).max(c[0]) >> 8) + 1).min(n as i64 - 1);
        let y0 = (a[1].min(b[1]).min(c[1]) >> 8).max(0);
        let y1 = ((a[1].max(b[1]).max(c[1]) >> 8) + 1).min(n as i64 - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = [x * 256 + 128, y * 256 + 128];
                let (wa, wb, wc) = (edge(b, c, p), edge(c, a, p), edge(a, b, p));
                if !(inside(wa, b, c) && inside(wb, c, a) && inside(wc, a, b)) {
                    continue;
                }
                let cov = shade(tri, [wa as f64 / area as f64, wb as f64 / area as f64, wc as f64 / area as f64]).clamp(0.0, 1.0);
                let d = &mut acc[y as usize * n + x as usize];
                *d = cov + *d * (1.0 - cov);
            }
        }
    }
    for (o, a) in out.iter_mut().zip(acc) {
        *o = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
}

/// Twice the signed area of `a b p`: positive with `p` right of `a → b` on a y-down screen.
fn edge(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> i64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// Inside the edge `a → b`, or on it where it's a top edge (flat, running right) or a left one
/// (running up).
fn inside(w: i64, a: [i64; 2], b: [i64; 2]) -> bool {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    w > 0 || (w == 0 && ((dy == 0 && dx > 0) || dy < 0))
}
