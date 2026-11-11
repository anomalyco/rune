//! Scrolling by moving pixels: when a frame's content moved by a whole number of pixels inside a
//! clip that stayed put (a scroll view), the rows of the buffer that hold it are moved by that
//! much, and only the tiles that came out different repaint (the strip that scrolled in, and
//! anything that didn't move with the content).
//!
//! A tile counts as moved only if what draws there is, instance for instance and in order,
//! what drew `dy` rows away last frame: each the same bytes but for its rect's y moved by `dy`
//! (and its clip's, or a clip that stayed put and is whole over both places), or the same bytes
//! where it paints one colour over both places. Every position the shading reads is relative to
//! the instance's rect, so those draw the same pixels `dy` rows away.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::Scene;
use super::bin::{Bins, Show, Solid, TILE_H, TILE_W, covers, interior};
use super::shade::{Clip, has_sides};
use crate::render::{Instance, KIND_LAYER, KIND_LAYER_END};

/// What the last frame drew, kept for the next frame's comparison.
pub(super) struct Prev {
    pub instances: Vec<Instance>,
    pub bins: Bins,
}

/// Rows `rect` of the new frame come from `dy` rows up of the old buffer (down when negative);
/// `clean[t]` says tile `t` needs nothing more.
pub(super) struct Shift {
    pub rect: [u32; 4],
    pub dy: i32,
    pub clean: Vec<bool>,
}

/// Words of an instance (as 32 `u32`s) that move with it: its rect's y and its clip's y edges.
const RECT_Y: usize = 1;
const CLIP_Y0: usize = 25;
const CLIP_Y1: usize = 27;

fn words(i: &Instance) -> &[u32; 32] { bytemuck::cast_ref(i) }

/// An instance's bytes with what moves with it left out.
fn key(i: &Instance) -> u64 {
    let mut h = 0u64;
    for (k, &w) in words(i).iter().enumerate() {
        if k == RECT_Y || k == CLIP_Y0 || k == CLIP_Y1 { continue; }
        h = (h.rotate_left(5) ^ u64::from(w)).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    h
}

/// Kinds whose every position is relative to their rect (segments and triangles hold absolute
/// points, so they never count as moved).
fn movable(kind: f32) -> bool {
    matches!(kind as u32, 0 | 1 | 2 | 3 | 4 | 7 | 8 | 9 | 10 | 11)
}

/// The shift that explains this frame, if one does: the whole-pixel `dy` most of the moved
/// instances agree on, inside the clip they share.
pub(super) fn plan(prev: &Prev, scene: &Scene<'_>, bins: &Bins, exec: &dyn super::raster::Exec) -> Option<Shift> {
    let old = &prev.instances;
    let voter = |i: &Instance| i.params[1] == crate::render::KIND_GLYPH || i.params[1] == crate::render::KIND_RECT;
    // The old voters by key, sorted: those with one key are a run.
    let mut by_key: Vec<(u64, u32)> = old.iter().enumerate().filter(|(_, i)| voter(i)).map(|(m, i)| (key(i), m as u32)).collect();
    by_key.sort_unstable();
    // Votes for (dy, the clip the content stayed inside); a frame has a handful of each.
    let mut votes: Vec<((i32, [u32; 4]), u32)> = Vec::new();
    for i in scene.instances.iter().filter(|i| voter(i)) {
        let k = key(i);
        let at = by_key.partition_point(|e| e.0 < k);
        // One vote each, for the smallest move that explains it: repeated glyphs (a column of
        // the same letter in code) match other lines too, and staying put (0) beats them.
        let dy = by_key[at..].iter().take_while(|e| e.0 == k).map(|e| &old[e.1 as usize]).filter(|o| i.clip == o.clip).map(|o| i.rect[1] - o.rect[1])
            .filter(|dy| dy.fract() == 0.0 && dy.abs() < scene.size.1 as f32).min_by(|a, b| a.abs().total_cmp(&b.abs()));
        let Some(dy) = dy else { continue };
        let vote = (dy as i32, i.clip.map(f32::to_bits));
        match votes.iter_mut().find(|v| v.0 == vote) {
            Some(v) => v.1 += 1,
            None => votes.push((vote, 1)),
        }
    }
    // The move most agree on, if more in its clip moved by it than stayed put.
    let &((dy, clip_bits), count) = votes.iter().filter(|v| v.0.0 != 0).max_by_key(|v| v.1)?;
    if count < 8 || votes.iter().any(|v| v.0 == (0, clip_bits) && v.1 >= count) { return None; }
    let clip = clip_bits.map(f32::from_bits);
    // Rows whose centres are where the clip is whole, now and `dy` rows back.
    let c = Clip::of(&Instance { clip, ..bytemuck::Zeroable::zeroed() });
    let inset = c.inset();
    let (w, h) = (scene.size.0 as f32, scene.size.1 as f32);
    let whole = [(clip[0] + inset[0]).max(0.0), (clip[1] + inset[1]).max(0.0), (clip[2] - inset[2]).min(w), (clip[3] - inset[3]).min(h)];
    let y0 = whole[1].max(whole[1] + dy as f32);
    let y1 = whole[3].min(whole[3] + dy as f32);
    let rect = [(whole[0] - 0.5).ceil().max(0.0) as u32, (y0 - 0.5).ceil().max(0.0) as u32, ((whole[2] - 0.5).floor() + 1.0).max(0.0) as u32, ((y1 - 0.5).floor() + 1.0).max(0.0) as u32];
    if rect[2] <= rect[0] || rect[3] <= rect[1] { return None; }
    // A backdrop blur's pixels never move: its record is in place in the old tiles that held it,
    // so no tile drawn from them counts as moved.
    if prev.bins.cols != bins.cols { return None; }
    // Any tile may have moved whole: instance for instance is the test, not the rect. Tile rows
    // go to threads.
    let clean: Vec<AtomicBool> = (0..bins.tile_count()).map(|_| AtomicBool::new(false)).collect();
    let next = AtomicU32::new(0);
    exec.run(&|| loop {
        let ty = next.fetch_add(1, Ordering::Relaxed);
        if ty >= bins.rows { break; }
        for tx in 0..bins.cols {
            let t = (ty * bins.cols + tx) as usize;
            let r = [tx * TILE_W, ty * TILE_H, ((tx + 1) * TILE_W).min(scene.size.0), ((ty + 1) * TILE_H).min(scene.size.1)];
            if !bins.quiet[t] && tile_moved(prev, scene, bins, t, r, dy) { clean[t].store(true, Ordering::Relaxed); }
        }
    });
    Some(Shift { rect, dy, clean: clean.into_iter().map(AtomicBool::into_inner).collect() })
}

/// Whether tile `t` (pixels `r`) draws what the old frame drew `dy` rows back.
fn tile_moved(prev: &Prev, scene: &Scene<'_>, bins: &Bins, t: usize, r: [u32; 4], dy: i32) -> bool {
    let back = [r[0] as i32, r[1] as i32 - dy, r[2] as i32, r[3] as i32 - dy];
    if back[1] < 0 || back[3] > scene.size.1 as i32 { return false; }
    let ob = &prev.bins;
    // What drew over the old rows: the old tiles that held them (one, or two when `dy` isn't a
    // whole number of tiles), each in painter order, merged, those reaching the rows kept.
    let (tx, oty) = (r[0] / TILE_W, back[1] as u32 / TILE_H);
    let upper = ob.tile((oty * ob.cols + tx) as usize);
    let lower = if (back[3] as u32).div_ceil(TILE_H) > oty + 1 && oty + 1 < ob.rows { ob.tile(((oty + 1) * ob.cols + tx) as usize) } else { &[] };
    let reaches = |m: u32| {
        let b = ob.bounds[m as usize];
        b[0] < back[2] && b[2] > back[0] && b[1] < back[3] && b[3] > back[1]
    };
    let (mut a, mut b) = (0, 0);
    let mut olds = std::iter::from_fn(|| loop {
        let m = match (upper.get(a), lower.get(b)) {
            (Some(&x), Some(&y)) if x == y => { a += 1; b += 1; x }
            (Some(&x), Some(&y)) if x < y => { a += 1; x }
            (_, Some(&y)) => { b += 1; y }
            (Some(&x), None) => { a += 1; x }
            (None, None) => return None,
        };
        if reaches(m) { return Some(m); }
    });
    let both = [r[0] as f32, r[1].min(back[1] as u32) as f32, r[2] as f32, r[3].max(back[3] as u32) as f32];
    bins.tile(t).iter().all(|&n| olds.next().is_some_and(|m| same(scene, bins, prev, n, m, dy, both))) && olds.next().is_none()
}

/// Whether new instance `n` draws over the tile what old instance `m` drew `dy` rows back:
/// moved with the content, or one colour over `both` (the tile and its old place together).
fn same(scene: &Scene<'_>, bins: &Bins, prev: &Prev, n: u32, m: u32, dy: i32, both: [f32; 4]) -> bool {
    let (a, b) = (&scene.instances[n as usize], &prev.instances[m as usize]);
    let (wa, wb) = (words(a), words(b));
    if !movable(a.params[1]) { return false; }
    let marker = a.params[1] == KIND_LAYER || a.params[1] == KIND_LAYER_END;
    if wa == wb {
        // In place: it must look the same everywhere it reaches over both.
        return if marker { layer_same(bins, prev, n, m, 0) && layer_covers(scene, bins, n, both) } else { flat_over(a, both) || rows_alike(a, both) };
    }
    let rest = (0..32).all(|k| k == RECT_Y || k == CLIP_Y0 || k == CLIP_Y1 || wa[k] == wb[k]);
    if !rest || a.rect[1] != b.rect[1] + dy as f32 { return false; }
    // Its clip moved with it, or stayed put and cuts every row over both places alike.
    let clip_moved = a.clip[1] == b.clip[1] + dy as f32 && a.clip[3] == b.clip[3] + dy as f32;
    let clip_kept = a.clip[1] == b.clip[1] && a.clip[3] == b.clip[3] && clip_rows_alike(&Clip::of(a), both);
    if !clip_moved && !clip_kept { return false; }
    !marker || layer_same(bins, prev, n, m, dy)
}

/// Whether the layer plans of markers `n` (new) and `m` (old) are the same `dy` rows apart.
fn layer_same(bins: &Bins, prev: &Prev, n: u32, m: u32, dy: i32) -> bool {
    let (Some(&p), Some(&q)) = (bins.scope.get(&n), prev.bins.scope.get(&m)) else { return false };
    let (l, o) = (&bins.layers[p as usize], &prev.bins.layers[q as usize]);
    l.show == o.show && l.additive == o.additive && l.hole == o.hole
        && l.bounds == [o.bounds[0], o.bounds[1] + dy, o.bounds[2], o.bounds[3] + dy]
}

/// Whether a layer in place composites the same over `both`: its target covers them, with no
/// hole and its clip whole there.
fn layer_covers(scene: &Scene<'_>, bins: &Bins, n: u32, both: [f32; 4]) -> bool {
    let Some(&p) = bins.scope.get(&n) else { return false };
    let l = &bins.layers[p as usize];
    if l.show != Show::Render { return true; }
    let b = l.bounds.map(|v| v as f32);
    let begin = &scene.instances[l.begin as usize];
    l.hole[3] <= 0.0 && b[1] <= both[1] && b[3] >= both[3] && clip_rows_alike(&Clip::of(begin), both)
}

/// Whether an instance in place draws every row of `r` alike: a box or a shadow whose rows
/// there are all in its straight middle, far enough from its top and bottom edges and corners
/// that what it draws depends on x alone, inside a clip that cuts them alike.
fn rows_alike(i: &Instance, r: [f32; 4]) -> bool {
    let (top, bottom) = (r[1] + 0.5, r[3] - 0.5);
    let middle = |margin: f32| top >= i.rect[1] + margin && bottom <= i.rect[1] + i.rect[3] - margin;
    let reach = i.radii.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let alike = match i.params[1] as u32 {
        // `sd_rounded` there is `|x| - half width`, or deep inside past the border's reach.
        0 => !has_sides(i) && middle(reach + 4.0 * (i.params[0].max(0.0) + i.params[2].max(0.5)) + 4.0),
        // `shadow_row`'s samples and its interior test are the same on every row there.
        1 => i.uv[2] <= 0.0 && {
            let half = [i.rect[2] * 0.5, i.rect[3] * 0.5];
            let corner = i.radii[0].abs().min(half[0].min(half[1]));
            middle(corner + 6.0 * i.params[2].max(0.01) + 1.0)
        },
        _ => false,
    };
    alike && clip_rows_alike(&Clip::of(i), r)
}

/// Whether `clip` cuts every row of `r` alike: they're all where its top and bottom edges, their
/// fades and its corners don't reach.
fn clip_rows_alike(c: &Clip, r: [f32; 4]) -> bool {
    let inset = c.inset();
    let (mut top, mut bottom) = (inset[1], inset[3]);
    if let Some(radii) = c.radii {
        let (t, b) = Solid { edges: c.rect, radii, inset: 0.5 }.bands();
        (top, bottom) = (top.max(t + 1.0), bottom.max(b + 1.0));
    }
    r[1] + 0.5 >= c.rect[1] + top && r[3] - 0.5 <= c.rect[3] - bottom
}

/// Whether a box in place paints one colour over all of `r`.
fn flat_over(i: &Instance, r: [f32; 4]) -> bool {
    i.params[1] == crate::render::KIND_RECT && interior(i).is_some_and(|s| covers(Some(&s), &Clip::of(i), r))
}
