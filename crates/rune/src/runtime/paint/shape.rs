//! Painter-stream instances for vector shapes (`El::shape`, `El::shape_with`).
use super::*;

/// How `Op::Rect` primitives inside a [`Shape`] map onto device pixels.
#[derive(Clone, Copy)]
pub(in crate::runtime) enum RectSnap {
    /// Snap all four edges independently in window space with `pixel_snap::snap` against `rigid`
    /// (device px), matching a child box element's fill.
    ///
    /// When `origin_pt` is `Some([ox, oy])` (no `view_box`, unscaled), each edge is quantized on
    /// the 1/64 px grid via `rune_motion::snap::grid(ox + local, s) * s` before snapping so it
    /// matches a child box at `absolute(x, y).size(w, h)` bit-for-bit. Otherwise (`view_box` or
    /// grid-row overlay), edges scale from `origin` by `k` before snapping.
    Window {
        origin_pt: Option<[f32; 2]>,
        s: f32,
        rigid: [f32; 2],
    },
    /// Exact (unsnapped) placement from `origin` (device px) scaled by `k`: used when pixel
    /// snapping is disabled, rotated, scaled, gliding, or in a matched flight.
    Exact {
        origin: [f32; 2],
    },
}

/// Instances for a vector shape whose origin sits at `origin` (device px), with `k` device px per
/// coordinate unit on each axis and `k_px` device px per point for stroke widths and corner radii.
#[allow(clippy::too_many_arguments)]
pub(in crate::runtime) fn push_shape(
    out: &mut Vec<Instance>,
    shape: &Shape,
    origin: [f32; 2],
    k: [f32; 2],
    k_px: f32,
    rect_snap: RectSnap,
    opacity: f32,
    srgb: bool,
    clip: [f32; 4],
    fade: [f32; 4],
) {
    let at = |p: [f32; 2]| [origin[0] + p[0] * k[0], origin[1] + p[1] * k[1]];
    for op in &shape.ops {
        match op {
            Op::Stroke { points, closed, width, cap, color } => {
                let fill = premul(*color, opacity, srgb);
                if fill[3] <= 0.0005 {
                    continue;
                }
                let hw = width * k_px * 0.5;
                let pad = hw.max(0.5) + 1.0;
                let n = points.len();
                let count = if *closed { n } else { n - 1 };
                for i in 0..count {
                    let (a, b) = (at(points[i]), at(points[(i + 1) % n]));
                    let (x0, y0) = (a[0].min(b[0]) - pad, a[1].min(b[1]) - pad);
                    out.push(Instance {
                        rect: [x0, y0, a[0].max(b[0]) + pad - x0, a[1].max(b[1]) + pad - y0],
                        radii: [a[0], a[1], b[0], b[1]],
                        fill,
                        border: [0.0; 4],
                        params: [hw, KIND_SEGMENT, if *cap == LineCap::Round { 1.0 } else { 0.0 }, 1.0],
                        uv: [0.0; 4],
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
            }
            Op::Fill { points, color } => {
                let fill = premul(*color, opacity, srgb);
                if fill[3] <= 0.0005 {
                    continue;
                }
                let tris = crate::shape::triangulate(points);
                push_triangles(out, points, &tris, at, fill, clip, fade);
            }
            Op::FillCompound { contours, rule, color } => {
                let fill = premul(*color, opacity, srgb);
                if fill[3] <= 0.0005 {
                    continue;
                }
                let (verts, tris) = crate::shape::triangulate_compound(contours, *rule);
                push_triangles(out, &verts, &tris, at, fill, clip, fade);
            }
            Op::FillArea { area, color } => {
                let fill = premul(*color, opacity, srgb);
                if fill[3] <= 0.0005 {
                    continue;
                }
                let (verts, tris) = area.triangles();
                push_triangles(out, verts, tris, at, fill, clip, fade);
            }
            Op::Rect { rect, radii, color } => {
                let fill = premul(*color, opacity, srgb);
                if fill[3] <= 0.0005 {
                    continue;
                }
                let [rx, ry, rw, rh] = *rect;
                if rw <= 0.0 || rh <= 0.0 {
                    continue;
                }
                let [x0, y0, x1, y1] = match rect_snap {
                    RectSnap::Window { origin_pt, s, rigid } => {
                        let edge_px = match origin_pt {
                            Some([ox, oy]) => {
                                let px = |v: f32| rune_motion::snap::grid(v, s) * s;
                                [px(ox + rx), px(oy + ry), px(ox + rx + rw), px(oy + ry + rh)]
                            }
                            None => [
                                origin[0] + rx * k[0],
                                origin[1] + ry * k[1],
                                origin[0] + (rx + rw) * k[0],
                                origin[1] + (ry + rh) * k[1],
                            ],
                        };
                        pixel_snap::snap(edge_px, [0.0; 4], rigid, None, |_, c| c).0
                    }
                    RectSnap::Exact { origin: ro } => [
                        ro[0] + rx * k[0],
                        ro[1] + ry * k[1],
                        ro[0] + (rx + rw) * k[0],
                        ro[1] + (ry + rh) * k[1],
                    ],
                };
                let (bw, bh) = (x1 - x0, y1 - y0);
                if bw <= 0.0 || bh <= 0.0 {
                    continue;
                }
                out.push(Instance {
                    rect: [x0, y0, bw, bh],
                    radii: radii.map(|r| r * k_px),
                    fill,
                    border: [0.0; 4],
                    params: [0.0, KIND_RECT, 0.0, 1.0],
                    uv: [0.0; 4],
                    clip,
                    fade: crate::render::pack_fade(fade, clip, clip),
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_triangles(
    out: &mut Vec<Instance>,
    verts: &[[f32; 2]],
    tris: &[([usize; 3], u32)],
    at: impl Fn([f32; 2]) -> [f32; 2],
    fill: [f32; 4],
    clip: [f32; 4],
    fade: [f32; 4],
) {
    for &([ia, ib, ic], aa) in tris {
        let (a, b, c) = (at(verts[ia]), at(verts[ib]), at(verts[ic]));
        let (x0, y0) = (a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1]));
        let (x1, y1) = (a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1]));
        out.push(Instance {
            rect: [x0, y0, x1 - x0, y1 - y0],
            radii: [a[0], a[1], b[0], b[1]],
            fill,
            border: [0.0; 4],
            params: [aa as f32, KIND_TRIANGLE, 0.0, 1.0],
            uv: [c[0], c[1], 0.0, 0.0],
            clip,
            fade: crate::render::pack_fade(fade, clip, clip),
        });
    }
}

#[cfg(test)]
#[path = "shape_tests.rs"]
mod tests;
