//! Layers turned by a multiple of 90° (`bin::quarter`). A tile can't draw one from its own
//! pixels, since what it shows comes from elsewhere, even from outside the window, so before
//! any tile paints, each turned layer's contents paint upright into a buffer of their own (its
//! target, as the GPU draws them into the layer's texture), and the layer's end composites that
//! buffer turned, sampled as shader.wgsl's turned `KIND_LAYER` branch samples its texture.

use super::super::bin::{Bins, Show, TILE_H, TILE_W};
use super::super::shade::Clip;
use super::super::{Scene, Target};
use super::{Exec, Painter, hole_alpha};
use crate::render::{Instance, Kind, NO_CLIP};

/// A turned layer's contents, painted upright: premultiplied B, G, R, A bytes, a row of
/// `target`'s width after another. They're 8-bit, not half floats like the GPU's `Rgba16Float`
/// layer target and this painter's other layer scratch tiles, so the composite can come out a
/// rounding step off the GPU's, most visibly below full opacity.
pub(in crate::render::cpu_boot) struct Turned {
    target: [i32; 4],
    px: Vec<u8>,
}

/// Paints the contents of each turned layer whose end draws in `tiles`, by plan; empty when
/// there are none. A turned layer inside another paints with that one's contents.
pub(super) fn prerender(scene: &Scene<'_>, bins: &Bins, tiles: &[u32], exec: &dyn Exec, wide: &dyn Exec) -> Vec<Option<Turned>> {
    let mut out: Vec<Option<Turned>> = Vec::new();
    for (p, l) in bins.layers.iter().enumerate() {
        let Some(turn) = l.turn else { continue };
        if l.show != Show::Render { continue; }
        if bins.layers.iter().any(|o| o.turn.is_some() && o.begin < l.begin && l.end < o.end) { continue; }
        let b = bins.bounds[l.end as usize];
        let shown = tiles.iter().any(|&t| {
            let (tx, ty) = (t % bins.cols, t / bins.cols);
            ((tx * TILE_W) as i32) < b[2] && (((tx + 1) * TILE_W) as i32) > b[0] && ((ty * TILE_H) as i32) < b[3] && (((ty + 1) * TILE_H) as i32) > b[1]
        });
        if !shown { continue; }
        let t = turn.target;
        let (w, h) = ((t[2] - t[0]) as u32, (t[3] - t[1]) as u32);
        let d = [-t[0] as f32, -t[1] as f32];
        let moved: Vec<Instance> = scene.instances[l.begin as usize + 1..l.end as usize].iter().map(|i| shifted(i, d)).collect();
        let part = scene.part(&moved, (w, h));
        let part_bins = Bins::of(&part);
        let mut px = vec![0u8; w as usize * h as usize * 4];
        let all: Vec<u32> = (0..part_bins.tile_count() as u32).collect();
        super::run(&part, &part_bins, &mut Target::new(&mut px, w, h), &all, exec, wide);
        if out.is_empty() { out.resize_with(bins.layers.len(), || None); }
        out[p] = Some(Turned { target: t, px });
    }
    out
}

/// `i` moved by `d` (whole pixels): every position it holds, its rect's and its clip's and
/// those some kinds keep elsewhere.
fn shifted(i: &Instance, d: [f32; 2]) -> Instance {
    let mut m = *i;
    m.rect[0] += d[0];
    m.rect[1] += d[1];
    if m.clip != NO_CLIP { m.clip = [m.clip[0] + d[0], m.clip[1] + d[1], m.clip[2] + d[0], m.clip[3] + d[1]]; }
    let pair = |v: &mut [f32; 4]| { v[0] += d[0]; v[1] += d[1]; };
    match i.kind() {
        // A glyph drawn from its outline keeps its origin in `radii.xy`.
        Some(Kind::Glyph) if i.radii[3] > 0.0 => pair(&mut m.radii),
        // A wrapped image keeps its placement rect in `border.xy` (`shade::image`); an
        // unwrapped one ignores `border`.
        Some(Kind::Image) => pair(&mut m.border),
        Some(Kind::Segment) => m.radii = [m.radii[0] + d[0], m.radii[1] + d[1], m.radii[2] + d[0], m.radii[3] + d[1]],
        Some(Kind::Triangle) => {
            m.radii = [m.radii[0] + d[0], m.radii[1] + d[1], m.radii[2] + d[0], m.radii[3] + d[1]];
            pair(&mut m.uv);
        }
        Some(Kind::Layer) => {
            // A cached scope's `radii` name its node; any other's are its hole.
            if i.params[3] != 1.0 { pair(&mut m.radii); }
            if i.border[1] != 0.0 { m.border[2] += d[0]; m.border[3] += d[1]; }
        }
        _ => {}
    }
    m
}

impl Painter<'_, '_> {
    /// A turned layer's end: its contents' buffer over pixels `r`, each pixel turned back into
    /// it and sampled bilinearly (nothing outside it), by the layer's opacity, clip and hole.
    pub(super) fn turned(&mut self, plan: usize, r: [u32; 4]) {
        let Some(Some(t)) = self.turned.get(plan) else { return };
        let l = self.bins.layers[plan];
        let Some(turn) = l.turn else { return };
        let begin = &self.scene.instances[l.begin as usize];
        let clip = Clip::of_stops(begin, self.scene.fade_stops);
        let opacity = begin.fill[3];
        let ([c, s], pivot) = (turn.cs, turn.pivot);
        let (w, h) = (t.target[2] - t.target[0], t.target[3] - t.target[1]);
        let texel = |x: i32, y: i32| -> [f32; 4] {
            if x < 0 || y < 0 || x >= w || y >= h { return [0.0; 4]; }
            let at = (y as usize * w as usize + x as usize) * 4;
            let p = &t.px[at..at + 4];
            [p[2] as f32 / 255.0, p[1] as f32 / 255.0, p[0] as f32 / 255.0, p[3] as f32 / 255.0]
        };
        let mix = |a: [f32; 4], b: [f32; 4], f: f32| -> [f32; 4] { std::array::from_fn(|k| a[k] * (1.0 - f) + b[k] * f) };
        for y in r[1]..r[3] {
            for x in r[0]..r[2] {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                let q = [p[0] - pivot[0], p[1] - pivot[1]];
                let local = [pivot[0] + c * q[0] + s * q[1] - t.target[0] as f32 - 0.5, pivot[1] + c * q[1] - s * q[0] - t.target[1] as f32 - 0.5];
                let base = [local[0].floor(), local[1].floor()];
                let f = [local[0] - base[0], local[1] - base[1]];
                let (bx, by) = (base[0] as i32, base[1] as i32);
                let color = mix(mix(texel(bx, by), texel(bx + 1, by), f[0]), mix(texel(bx, by + 1), texel(bx + 1, by + 1), f[0]), f[1]);
                if color == [0.0; 4] { continue; }
                let a = opacity * clip.alpha(p) * hole_alpha(p, l.hole);
                let out = color.map(|v| v * a);
                if out != [0.0; 4] { self.put(x, y, out); }
            }
        }
    }
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
