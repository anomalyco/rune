//! The live CPU renderer: paints each frame into the same window buffer, repainting only the
//! tiles whose inputs changed since the frame before (`damage.rs`), on a pool of threads kept
//! between frames, and says which rectangles of the buffer changed so the window copies only
//! those to the screen.

use std::time::Instant;

use super::bin::Bins;
use super::pool::Pool;
use super::{NeedsGpu, Report, Scene, Target, damage, raster, scroll};

/// Rectangles a frame's damage is merged into at most.
const MAX_RECTS: usize = 8;
/// Dirty tiles per extra thread: a handful of tiles paints faster on the calling thread than it
/// takes to wake another.
const TILES_PER_THREAD: usize = 24;
/// Tiles a frame must change before it's checked for a scroll.
const MIN_SCROLL_TILES: usize = 16;

/// Paints frames into one retained buffer.
pub struct CpuPainter {
    pool: Pool,
    /// Each tile's inputs last frame, for a buffer of `size`; empty when the buffer's pixels
    /// aren't known (`forget`).
    hashes: Vec<u64>,
    next: Vec<u64>,
    each: Vec<u64>,
    size: (u32, u32),
    dirty: Vec<u32>,
    /// The last frame's instances and bins, for telling a scroll (`scroll.rs`).
    prev: Option<scroll::Prev>,
}

/// What a frame painted.
#[derive(Clone, Debug, Default)]
pub struct Painted {
    pub report: Report,
    /// Tiles repainted, of all of them.
    pub dirty_tiles: usize,
    pub tiles: usize,
    /// The buffer's changed pixels, `[x0, y0, x1, y1)`, at most eight rectangles.
    pub rects: Vec<[u32; 4]>,
    /// Rows of the buffer moved by `dy` (down when positive) before the dirty tiles painted:
    /// a window can scroll its own copy by as much and copy only the rest.
    pub scrolled: Option<([u32; 4], i32)>,
    /// Hashing tiles, on the calling thread, with telling a scroll and moving its rows.
    pub hash_ns: u64,
    /// Of that, telling whether the frame scrolled (scroll.rs), and moving the rows.
    pub plan_ns: u64,
    pub move_ns: u64,
}

impl CpuPainter {
    /// A painter running on `threads` threads, the caller among them (the others are made now,
    /// and park between frames).
    pub fn new(threads: usize) -> CpuPainter {
        CpuPainter { pool: Pool::new(threads), hashes: Vec::new(), next: Vec::new(), each: Vec::new(), size: (0, 0), dirty: Vec::new(), prev: None }
    }

    /// The buffer's pixels are no longer the last frame's (a new or resized buffer): the next
    /// frame paints every tile.
    pub fn forget(&mut self) {
        self.hashes.clear();
        self.prev = None;
    }

    /// Fills the masks the text system queued (`fill_masks`) on this painter's threads.
    pub fn fill_masks(&self, atlas: &mut crate::text::Atlas) -> super::FillReport {
        super::fill::fill_with(atlas, &|threads| Threads { pool: &self.pool, extra: (threads - 1).min(self.pool.threads() - 1) })
    }

    /// Paints `scene` into `target`, which holds the frame this painter painted last unless
    /// `forget` was called since.
    pub fn paint(&mut self, scene: &Scene<'_>, target: &mut Target<'_>) -> Result<Painted, NeedsGpu> {
        if let Some(why) = scene.needs_gpu() { return Err(why); }
        super::check_target(scene, target)?;
        let start = Instant::now();
        let bins = Bins::of(scene);
        let binned = Instant::now();
        damage::hashes(scene, &bins, &mut self.each, &mut self.next);
        if self.size != scene.size { self.hashes.clear(); }
        self.dirty.clear();
        let all = self.hashes.len() != self.next.len();
        // A backdrop blur reads every pixel of its region: when any tile there changed, the
        // whole region paints again (what's under the blur first, then the blur).
        let mut redo = vec![false; self.next.len()];
        for (_, rec) in &bins.blurs {
            if all || super::bin::region_tiles(rec.region, bins.cols, bins.rows).any(|t| self.hashes[t] != self.next[t]) {
                for t in super::bin::region_tiles(rec.region, bins.cols, bins.rows) { redo[t] = true; }
            }
        }
        let changed = |t: usize| all || redo[t] || self.hashes[t] != self.next[t];
        let by_hash = (0..self.next.len()).filter(|&t| changed(t)).count();
        // Only a frame that changed many tiles can be a scroll; and a move is kept only when it
        // leaves fewer tiles to paint than the hashes alone do.
        let planning = Instant::now();
        let shift = if all || by_hash < MIN_SCROLL_TILES { None } else { self.prev.as_ref().and_then(|p| scroll::plan(p, scene, &bins, &Threads { pool: &self.pool, extra: self.pool.threads() - 1 })) };
        let touched = |s: &scroll::Shift, t: usize| {
            let (tx, ty) = (t as u32 % bins.cols, t as u32 / bins.cols);
            tx * super::bin::TILE_W < s.rect[2] && (tx + 1) * super::bin::TILE_W > s.rect[0] && ty * super::bin::TILE_H < s.rect[3] && (ty + 1) * super::bin::TILE_H > s.rect[1]
        };
        // A tile the move touched holds the right pixels only if it moved whole.
        let dirty = |s: Option<&scroll::Shift>, t: usize| match s {
            Some(s) if touched(s, t) => bins.quiet[t] || !s.clean[t],
            _ => changed(t),
        };
        let shift = shift.filter(|s| (0..self.next.len()).filter(|&t| dirty(Some(s), t)).count() < by_hash);
        let moving = Instant::now();
        if let Some(s) = &shift {
            let movers = (s.clean.iter().filter(|&&c| c).count() / TILES_PER_THREAD).min(self.pool.threads() - 1);
            move_tiles(&Threads { pool: &self.pool, extra: movers }, target, scene.size, bins.cols, &s.clean, s.dy);
        }
        let moved = Instant::now();
        self.dirty.extend((0..self.next.len() as u32).filter(|&t| dirty(shift.as_ref(), t as usize)));
        std::mem::swap(&mut self.hashes, &mut self.next);
        self.size = scene.size;
        let hashed = Instant::now();
        let extra = (self.dirty.len() / TILES_PER_THREAD).min(self.pool.threads() - 1);
        let mut report = raster::run(scene, &bins, target, &self.dirty, &Threads { pool: &self.pool, extra });
        let done = Instant::now();
        report.bin_ns = super::ns(binned - start);
        report.raster_ns = super::ns(done - hashed);
        report.total_ns = super::ns(done - start);
        report.tiles = bins.tile_count();
        report.binned = bins.len();
        report.culled = bins.culled;
        let mut rects = damage::rects(&self.dirty, bins.cols, scene.size, MAX_RECTS);
        if let Some(s) = &shift { damage::add(&mut rects, s.rect, MAX_RECTS); }
        let scrolled = shift.as_ref().map(|s| (s.rect, s.dy));
        self.prev = Some(scroll::Prev { instances: scene.instances.to_vec(), bins });
        Ok(Painted { dirty_tiles: self.dirty.len(), tiles: report.tiles, rects, scrolled, hash_ns: super::ns(hashed - binned), plan_ns: super::ns(moving - planning), move_ns: super::ns(moved - moving), report })
    }
}

/// Moves the tiles the scroll left clean `dy` rows (down when positive): each takes what was
/// `dy` rows back. Tiles of one column read only that column, so columns go to threads; within a
/// column the tiles and their rows go in the order that reads each row before it's written
/// (bottom up for a move down). The rest repaint, so they aren't moved.
fn move_tiles(threads: &Threads<'_>, target: &mut Target<'_>, size: (u32, u32), cols: u32, clean: &[bool], dy: i32) {
    use super::bin::{TILE_H, TILE_W};
    use std::sync::atomic::{AtomicU32, Ordering};
    let (base, len, stride) = (target.pixels.as_mut_ptr() as usize, target.pixels.len(), target.stride);
    let rows = size.1.div_ceil(TILE_H);
    let next = AtomicU32::new(0);
    let job = || loop {
        let tx = next.fetch_add(1, Ordering::Relaxed);
        if tx >= cols { break; }
        let (x0, x1) = (tx * TILE_W, ((tx + 1) * TILE_W).min(size.0));
        let bytes = (x1 - x0) as usize * 4;
        let copy = |y: u32| {
            let (src, dst) = ((y as i64 - dy as i64) as usize * stride + x0 as usize * 4, y as usize * stride + x0 as usize * 4);
            assert!(src + bytes <= len && dst + bytes <= len);
            // SAFETY: both in bounds (checked); `dy != 0`, so the two rows don't overlap; only
            // this thread touches column `tx`, and a row is read before any move writes it.
            unsafe { std::ptr::copy_nonoverlapping((base + src) as *const u8, (base + dst) as *mut u8, bytes) };
        };
        let tile_rows = |ty: u32| (ty * TILE_H, ((ty + 1) * TILE_H).min(size.1));
        let moved = |ty: u32| clean[(ty * cols + tx) as usize];
        if dy > 0 {
            for ty in (0..rows).rev().filter(|&ty| moved(ty)) { let (a, b) = tile_rows(ty); for y in (a..b).rev() { copy(y); } }
        } else {
            for ty in (0..rows).filter(|&ty| moved(ty)) { let (a, b) = tile_rows(ty); for y in a..b { copy(y); } }
        }
    };
    raster::Exec::run(threads, &job);
}
/// The pool, with as many of its workers as the frame's damage pays for.
struct Threads<'p> {
    pool: &'p Pool,
    extra: usize,
}

impl raster::Exec for Threads<'_> {
    fn threads(&self) -> usize { self.extra + 1 }

    fn run(&self, job: &(dyn Fn() + Sync)) { self.pool.run(self.extra, job); }
}
