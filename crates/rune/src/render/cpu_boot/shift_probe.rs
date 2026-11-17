//! Measuring only (frame-profile, `RUNE_SHIFT_PROBE=FILE`): on each GPU frame of a window, runs
//! the CPU painter's scroll proof (`scroll::plan`, with its bins and tile hashes) against the
//! last frame, paints nothing, and writes one JSON line: whether the frame would shift, by how
//! much, how many tiles the proof clears against how many the hashes alone redraw, and the proof's
//! CPU time. Not shipped.

use std::io::Write;
use std::sync::Mutex;

use super::bin::{Bins, region_tiles};
use super::{Scene, damage, scroll};
use crate::time::Instant;

struct State {
    prev: Option<scroll::Prev>,
    hashes: Vec<u64>,
    next: Vec<u64>,
    each: Vec<u64>,
    size: (u32, u32),
    out: std::io::BufWriter<std::fs::File>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The painter's threshold: only a frame that changed this many tiles can be a scroll.
const MIN_SCROLL_TILES: usize = 16;

struct One;

impl super::raster::Exec for One {
    fn threads(&self) -> usize { 1 }
    fn run(&self, job: &(dyn Fn() + Sync)) { job() }
}

/// One frame's proof (see the module doc); nothing unless `RUNE_SHIFT_PROBE` names a file.
pub(crate) fn frame(instances: &[super::Instance], atlas: &crate::text::Atlas, images: &crate::pixels::ImageAtlas, size: (u32, u32), clear: [f32; 4]) {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let Some(path) = std::env::var_os("RUNE_SHIFT_PROBE") else { return };
        let Ok(file) = std::fs::File::create(path) else { return };
        *guard = Some(State { prev: None, hashes: Vec::new(), next: Vec::new(), each: Vec::new(), size: (0, 0), out: std::io::BufWriter::new(file) });
    }
    let st = guard.as_mut().unwrap();
    let scene = Scene::new(instances, atlas, images, size, clear);
    let start = Instant::now();
    let bins = Bins::of(&scene);
    let binned = Instant::now();
    damage::hashes(&scene, &bins, &mut st.each, &mut st.next);
    let hashed = Instant::now();
    let all = st.hashes.is_empty() || st.size != size;
    let hash_changed = |t: usize| all || st.hashes.get(t) != st.next.get(t);
    let mut redo = vec![false; st.next.len()];
    for (_, rec) in &bins.blurs {
        if all || region_tiles(rec.region, bins.cols, bins.rows).any(hash_changed) {
            for t in region_tiles(rec.region, bins.cols, bins.rows) { redo[t] = true; }
        }
    }
    let changed = |t: usize| redo[t] || hash_changed(t);
    let by_hash = (0..st.next.len()).filter(|&t| changed(t)).count();
    let shift = if all || by_hash < MIN_SCROLL_TILES { None } else { st.prev.as_ref().and_then(|p| scroll::plan(p, &scene, &bins, &One)) };
    let planned = Instant::now();
    let touched = |s: &scroll::Shift, t: usize| {
        let (tx, ty) = (t as u32 % bins.cols, t as u32 / bins.cols);
        tx * super::bin::TILE_W < s.rect[2] && (tx + 1) * super::bin::TILE_W > s.rect[0] && ty * super::bin::TILE_H < s.rect[3] && (ty + 1) * super::bin::TILE_H > s.rect[1]
    };
    let dirty = |s: Option<&scroll::Shift>, t: usize| match s {
        Some(s) if touched(s, t) => bins.quiet[t] || !s.clean[t],
        _ => changed(t),
    };
    let tiles = bins.tile_count();
    let (in_rect, clean) = shift.as_ref().map_or((0, 0), |s| ((0..tiles).filter(|&t| touched(s, t)).count(), s.clean.iter().filter(|&&c| c).count()));
    let after = (0..tiles).filter(|&t| dirty(shift.as_ref(), t)).count();
    // As the painter keeps a move: only when it leaves fewer tiles to paint than the hashes alone.
    let kept = shift.as_ref().is_some_and(|_| after < by_hash);
    let line = serde_json::json!({
        "t_ns": crate::frame::host_clock::now_ns(), "tiles": tiles, "changed_by_hash": by_hash, "first": all,
        "planned": shift.is_some(), "shift": kept, "dy": shift.as_ref().map(|s| s.dy), "rect": shift.as_ref().map(|s| s.rect),
        "rect_tiles": in_rect, "clean": clean, "dirty_after": if kept { after } else { by_hash },
        "bin_us": (binned - start).as_secs_f64() * 1e6, "hash_us": (hashed - binned).as_secs_f64() * 1e6, "plan_us": (planned - hashed).as_secs_f64() * 1e6,
        "instances": instances.len(),
    });
    let _ = writeln!(st.out, "{line}");
    let _ = st.out.flush();
    std::mem::swap(&mut st.hashes, &mut st.next);
    st.size = size;
    st.prev = Some(scroll::Prev { instances: instances.to_vec(), bins });
}
