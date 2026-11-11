//! Damage by tile hash: a tile whose painting inputs hash the same as last frame's holds the
//! same pixels, since the window's buffer keeps them between frames. A tile's inputs are what
//! it draws (`Bins::tile`, culled to what can show): each instance's 128 bytes in order, the
//! layer plans its scope records stand for, whether it starts cleared or covered and the clear
//! colour, the atlas generation for glyphs and the image texels images sample.

use super::Scene;
use super::bin::{Bins, TILE_H, TILE_W};
use crate::render::{KIND_GLYPH, KIND_IMAGE, KIND_LAYER, KIND_LAYER_END};

/// FxHash's step, word by word.
#[inline]
fn mix(h: u64, w: u64) -> u64 { (h.rotate_left(5) ^ w).wrapping_mul(0x517c_c1b7_2722_0a95) }

/// Each tile's hash into `out`, from each instance's hash, made once into `each`.
pub(super) fn hashes(scene: &Scene<'_>, bins: &Bins, each: &mut Vec<u64>, out: &mut Vec<u64>) {
    each.clear();
    each.extend(scene.instances.iter().enumerate().map(|(n, i)| {
        let words: &[u32; 32] = bytemuck::cast_ref(i);
        let mut h = 0;
        for w in words.chunks_exact(2) { h = mix(h, u64::from(w[0]) | u64::from(w[1]) << 32); }
        let kind = i.params[1];
        if kind == KIND_LAYER || kind == KIND_LAYER_END {
            if let Some(&p) = bins.scope.get(&(n as u32)) {
                let l = &bins.layers[p as usize];
                let b = l.bounds;
                h = mix(mix(h, (b[0] as u32 as u64) | (b[1] as u32 as u64) << 32), (b[2] as u32 as u64) | (b[3] as u32 as u64) << 32);
                h = mix(h, l.show as u64);
            }
        } else if kind == KIND_GLYPH {
            h = mix(h, scene.atlas_resets);
        } else if kind == KIND_IMAGE {
            h = image(scene, i, h);
        }
        h
    }));
    out.clear();
    let clear = scene.clear.map(f32::to_bits);
    let base = mix(mix(0, u64::from(clear[0]) | u64::from(clear[1]) << 32), u64::from(clear[2]) | u64::from(clear[3]) << 32);
    for t in 0..bins.tile_count() {
        let mut h = mix(mix(base, u64::from(bins.covered[t])), u64::from(bins.quiet[t]));
        for &n in bins.tile(t) { h = mix(h, each[n as usize]); }
        out.push(h);
    }
}
/// The texels an image instance samples, hashed: an animated image writes its next frame into
/// the same place.
fn image(scene: &Scene<'_>, i: &crate::render::Instance, mut h: u64) -> u64 {
    let Some(page) = scene.images.get(i.params[0] as usize) else { return h };
    let w = page.width as usize;
    let (x0, y0) = (i.uv[0].max(0.0) as usize, i.uv[1].max(0.0) as usize);
    let (x1, y1) = (x0 + i.uv[2].max(0.0) as usize, y0 + i.uv[3].max(0.0) as usize);
    for y in y0..y1 {
        let Some(row) = page.data.get((y * w + x0) * 4..(y * w + x1) * 4) else { break };
        for c in row.chunks(8) {
            let mut b = [0u8; 8];
            b[..c.len()].copy_from_slice(c);
            h = mix(h, u64::from_le_bytes(b));
        }
    }
    h
}

/// Dirty tiles as at most `max` rectangles of pixels `[x0, y0, x1, y1)`: runs along each tile row,
/// stacked where consecutive rows have the same run, then merged into their bounding boxes
/// pairwise (the closest first) until few enough.
pub(super) fn rects(dirty: &[u32], cols: u32, size: (u32, u32), max: usize) -> Vec<[u32; 4]> {
    let mut runs: Vec<[u32; 4]> = Vec::new();
    let mut k = 0;
    while k < dirty.len() {
        let (t0, row) = (dirty[k], dirty[k] / cols);
        let mut t1 = t0;
        while k + 1 < dirty.len() && dirty[k + 1] == t1 + 1 && dirty[k + 1] / cols == row { k += 1; t1 += 1; }
        k += 1;
        let r = [(t0 % cols) * TILE_W, row * TILE_H, ((t1 % cols + 1) * TILE_W).min(size.0), ((row + 1) * TILE_H).min(size.1)];
        match runs.iter_mut().find(|p| p[0] == r[0] && p[2] == r[2] && p[3] == r[1]) {
            Some(p) => p[3] = r[3],
            None => runs.push(r),
        }
    }
    while runs.len() > max {
        let area = |r: &[u32; 4]| u64::from(r[2] - r[0]) * u64::from(r[3] - r[1]);
        let union = |a: &[u32; 4], b: &[u32; 4]| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
        let mut best = (0, 1, u64::MAX);
        for a in 0..runs.len() {
            for b in a + 1..runs.len() {
                let waste = area(&union(&runs[a], &runs[b])).saturating_sub(area(&runs[a]) + area(&runs[b]));
                if waste < best.2 { best = (a, b, waste); }
            }
        }
        let b = runs.swap_remove(best.1);
        runs[best.0] = union(&runs[best.0], &b);
    }
    runs
}
