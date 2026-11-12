//! Damage by tile hash: a tile whose painting inputs hash the same as last frame's holds the
//! same pixels, since the window's buffer keeps them between frames. A tile's inputs are what
//! it draws (`Bins::tile`, culled to what can show): each instance's 128 bytes in order, the
//! layer plans its scope records stand for, whether it starts cleared or covered and the clear
//! colour, the atlas generation for glyphs and the image texels images sample.

use super::Scene;
use super::bin::{Bins, Solid, TILE_H, TILE_W, covers, interior};
use super::shade::Clip;
use crate::render::{KIND_GLYPH, KIND_IMAGE, KIND_LAYER, KIND_LAYER_END, KIND_RECT};

/// FxHash's step, word by word.
#[inline]
fn mix(h: u64, w: u64) -> u64 { (h.rotate_left(5) ^ w).wrapping_mul(0x517c_c1b7_2722_0a95) }

/// Each tile's hash into `out`, from each instance's hash, made once into `each`.
pub(super) fn hashes(scene: &Scene<'_>, bins: &Bins, each: &mut Vec<u64>, out: &mut Vec<u64>) {
    each.clear();
    each.extend((0..scene.instances.len()).map(|n| instance(scene, bins, n)));
    let flat: Vec<(u32, Flat)> = (0..scene.instances.len()).filter_map(|n| flat(&scene.instances[n]).map(|f| (n as u32, f))).collect();
    out.clear();
    out.extend((0..bins.tile_count()).map(|t| tile(scene, bins, each, t, |n| flat.binary_search_by_key(&n, |f| f.0).ok().map(|k| &flat[k].1))));
}

/// Tile `t`'s hash again, after instances in it changed (`each` already holds theirs).
pub(super) fn rehash(scene: &Scene<'_>, bins: &Bins, each: &[u64], t: usize) -> u64 {
    let flats: Vec<(u32, Flat)> = bins.tile(t).iter().filter_map(|&n| flat(&scene.instances[n as usize]).map(|f| (n, f))).collect();
    tile(scene, bins, each, t, |n| flats.iter().find(|f| f.0 == n).map(|f| &f.1))
}

/// One instance's inputs: its bytes, and what they stand for elsewhere (a layer's plan, the
/// atlas's evictions for a glyph, an image's texels).
pub(super) fn instance(scene: &Scene<'_>, bins: &Bins, n: usize) -> u64 {
    let i = &scene.instances[n];
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
}

/// A box whose one colour fills a whole tile draws only that colour there, wherever its edges
/// are: a tile it fills takes its colour, not its bytes, so a panel that grows or a window card
/// that resizes leaves the tiles inside it clean. Boxes big enough to fill a tile; each with the
/// rect its corners and its clip's edges can't reach (a tile inside that is filled without
/// asking).
pub(super) struct Flat {
    solid: Solid,
    clip: Clip,
    token: u64,
    sure: [f32; 4],
}

fn flat(i: &crate::render::Instance) -> Option<Flat> {
    if i.params[1] != KIND_RECT || i.rect[2] < TILE_W as f32 || i.rect[3] < TILE_H as f32 { return None; }
    let solid = interior(i)?;
    let clip = Clip::of(i);
    let fill: &[u32; 4] = bytemuck::cast_ref(&i.fill);
    let m = solid.radii.iter().fold(0.0f32, |a, r| a.max(r.abs())) + solid.inset + 1.0;
    let e = solid.edges;
    let inset = clip.inset();
    let cm = clip.radii.map_or(0.0, |r| r.iter().fold(0.0f32, |a, v| a.max(v.abs()))) + 1.0;
    let c = clip.rect;
    let sure = [(e[0] + m).max(c[0] + inset[0] + cm), (e[1] + m).max(c[1] + inset[1] + cm), (e[2] - m).min(c[2] - inset[2] - cm), (e[3] - m).min(c[3] - inset[3] - cm)];
    let token = mix(mix(0x5a5a, u64::from(fill[0]) | u64::from(fill[1]) << 32), u64::from(fill[2]) | u64::from(fill[3]) << 32);
    Some(Flat { solid, clip, token, sure })
}

/// Tile `t`'s hash: how it starts, then each instance it draws, by its colour where it fills the
/// tile (`flat_of`), else by its inputs.
fn tile<'f>(scene: &Scene<'_>, bins: &Bins, each: &[u64], t: usize, flat_of: impl Fn(u32) -> Option<&'f Flat>) -> u64 {
    let clear = scene.clear.map(f32::to_bits);
    let base = mix(mix(0, u64::from(clear[0]) | u64::from(clear[1]) << 32), u64::from(clear[2]) | u64::from(clear[3]) << 32);
    let (w, h) = scene.size;
    let mut hash = mix(mix(base, u64::from(bins.covered[t])), u64::from(bins.quiet[t]));
    let (tx, ty) = (t as u32 % bins.cols, t as u32 / bins.cols);
    let r = [(tx * TILE_W) as f32, (ty * TILE_H) as f32, ((tx + 1) * TILE_W).min(w) as f32, ((ty + 1) * TILE_H).min(h) as f32];
    for &n in bins.tile(t) {
        let token = match flat_of(n) {
            Some(f) if inside(r, f.sure) || covers(Some(&f.solid), &f.clip, r) => f.token,
            _ => each[n as usize],
        };
        hash = mix(hash, token);
    }
    hash
}

fn inside(r: [f32; 4], o: [f32; 4]) -> bool { r[0] >= o[0] && r[1] >= o[1] && r[2] <= o[2] && r[3] <= o[3] }
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

/// Adds `r` to `rects`, merging the closest pair while there are more than `max`.
pub(super) fn add(rects: &mut Vec<[u32; 4]>, r: [u32; 4], max: usize) {
    rects.push(r);
    merge(rects, max);
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
    merge(&mut runs, max);
    runs
}

/// Merges rectangles into their bounding boxes pairwise, the pair that wastes least first, until
/// at most `max` are left.
fn merge(runs: &mut Vec<[u32; 4]>, max: usize) {
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
}
