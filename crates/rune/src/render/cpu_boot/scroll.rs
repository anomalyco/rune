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

use std::collections::HashMap;

use super::Scene;
use super::bin::{Bins, Show, Solid, TILE_H, TILE_W, clip_span, interior, intersect};
use super::shade::Clip;
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
pub(super) fn plan(prev: &Prev, scene: &Scene<'_>, bins: &Bins) -> Option<Shift> {
    let old = &prev.instances;
    let mut by_key: HashMap<u64, Vec<u32>> = HashMap::new();
    for (m, i) in old.iter().enumerate() {
        if movable(i.params[1]) { by_key.entry(key(i)).or_default().push(m as u32); }
    }
    // Votes for (dy, the clip the content stayed inside).
    let mut votes: HashMap<(i32, [u32; 4]), u32> = HashMap::new();
    for i in scene.instances.iter().filter(|i| i.params[1] == crate::render::KIND_GLYPH || i.params[1] == crate::render::KIND_RECT) {
        let Some(ms) = by_key.get(&key(i)) else { continue };
        for &m in ms {
            let o = &old[m as usize];
            let dy = i.rect[1] - o.rect[1];
            if dy != 0.0 && dy.fract() == 0.0 && i.clip == o.clip && dy.abs() < scene.size.1 as f32 {
                *votes.entry((dy as i32, i.clip.map(f32::to_bits))).or_default() += 1;
            }
        }
    }
    let ((dy, clip_bits), count) = votes.into_iter().max_by_key(|(_, c)| *c)?;
    if count < 8 { return None; }
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
    if prev.bins.cols != bins.cols || prev.bins.quiet.iter().any(|&q| q) { return None; }
    let mut clean = vec![false; bins.tile_count()];
    for t in 0..bins.tile_count() {
        let (tx, ty) = (t as u32 % bins.cols, t as u32 / bins.cols);
        let r = [tx * TILE_W, ty * TILE_H, ((tx + 1) * TILE_W).min(scene.size.0), ((ty + 1) * TILE_H).min(scene.size.1)];
        if r[0] < rect[0] || r[2] > rect[2] || r[1] < rect[1] || r[3] > rect[3] || bins.quiet[t] { continue; }
        clean[t] = tile_moved(prev, scene, bins, t, r, dy);
    }
    Some(Shift { rect, dy, clean })
}

/// Whether tile `t` (pixels `r`) draws what the old frame drew `dy` rows back.
fn tile_moved(prev: &Prev, scene: &Scene<'_>, bins: &Bins, t: usize, r: [u32; 4], dy: i32) -> bool {
    let back = [r[0] as i32, r[1] as i32 - dy, r[2] as i32, r[3] as i32 - dy];
    if back[1] < 0 || back[3] > scene.size.1 as i32 { return false; }
    let ob = &prev.bins;
    // What drew over the old rows, from the old tiles that held them.
    let tx = r[0] / TILE_W;
    let mut olds: Vec<u32> = Vec::new();
    for oty in back[1] as u32 / TILE_H..(back[3] as u32).div_ceil(TILE_H).min(ob.rows) {
        olds.extend_from_slice(ob.tile((oty * ob.cols + tx) as usize));
    }
    olds.sort_unstable();
    olds.dedup();
    olds.retain(|&m| {
        let b = ob.bounds[m as usize];
        b[0] < back[2] && b[2] > back[0] && b[1] < back[3] && b[3] > back[1]
    });
    let news = bins.tile(t);
    if news.len() != olds.len() { return false; }
    let both = [r[0] as f32, r[1].min(back[1] as u32) as f32, r[2] as f32, r[3].max(back[3] as u32) as f32];
    news.iter().zip(&olds).all(|(&n, &m)| same(scene, bins, prev, n, m, dy, both))
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
        return if marker { layer_same(bins, prev, n, m, 0) && layer_covers(scene, bins, n, both) } else { flat_over(a, both) };
    }
    let rest = (0..32).all(|k| k == RECT_Y || k == CLIP_Y0 || k == CLIP_Y1 || wa[k] == wb[k]);
    if !rest || a.rect[1] != b.rect[1] + dy as f32 { return false; }
    // Its clip moved with it, or stayed put and is whole over both places.
    let clip_moved = a.clip[1] == b.clip[1] + dy as f32 && a.clip[3] == b.clip[3] + dy as f32;
    let clip_kept = a.clip[1] == b.clip[1] && a.clip[3] == b.clip[3] && covers(None, &Clip::of(a), both);
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
    l.hole[3] <= 0.0 && b[0] <= both[0] && b[1] <= both[1] && b[2] >= both[2] && b[3] >= both[3] && covers(None, &Clip::of(begin), both)
}

/// Whether a box in place paints one colour over all of `r`.
fn flat_over(i: &Instance, r: [f32; 4]) -> bool {
    i.params[1] == crate::render::KIND_RECT && interior(i).is_some_and(|s| covers(Some(&s), &Clip::of(i), r))
}

/// Whether every pixel centre of `r` is inside `solid` (if given) where `clip` is whole: true at
/// its first and last rows means true between (both regions are convex).
fn covers(solid: Option<&Solid>, clip: &Clip, r: [f32; 4]) -> bool {
    let (x0, x1) = (r[0] + 0.5, r[2] - 0.5);
    [r[1] + 0.5, r[3] - 0.5].iter().all(|&y| {
        let span = solid.map_or(clip_span(clip, y), |s| intersect(s.span(y), clip_span(clip, y)));
        span.0 <= x0 && span.1 >= x1
    })
}
