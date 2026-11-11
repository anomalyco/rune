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
    /// Hashing tiles, on the calling thread.
    pub hash_ns: u64,
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
        let shift = if all { None } else { self.prev.as_ref().and_then(|p| scroll::plan(p, scene, &bins)) };
        if let Some(s) = &shift { move_rows(target, s.rect, s.dy); }
        let touched = |t: usize| shift.as_ref().is_some_and(|s| {
            let (tx, ty) = (t as u32 % bins.cols, t as u32 / bins.cols);
            tx * super::bin::TILE_W < s.rect[2] && (tx + 1) * super::bin::TILE_W > s.rect[0] && ty * super::bin::TILE_H < s.rect[3] && (ty + 1) * super::bin::TILE_H > s.rect[1]
        });
        // A blur reads its neighbours' pixels: the tiles under one paint every frame. A tile the
        // move touched holds the right pixels only if it moved whole.
        self.dirty.extend((0..self.next.len() as u32).filter(|&t| {
            let t = t as usize;
            all || bins.quiet[t] || if touched(t) { !shift.as_ref().is_some_and(|s| s.clean[t]) } else { self.hashes[t] != self.next[t] }
        }));
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
        Ok(Painted { dirty_tiles: self.dirty.len(), tiles: report.tiles, rects, scrolled, hash_ns: super::ns(hashed - binned), report })
    }
}

/// Moves rows `rect` of the buffer by `dy`: row `y` takes what row `y - dy` held, in the order
/// that reads each row before it's written.
fn move_rows(target: &mut Target<'_>, rect: [u32; 4], dy: i32) {
    let (x0, x1, stride) = (rect[0] as usize * 4, rect[2] as usize * 4, target.stride);
    let mut copy = |y: u32| {
        let src = (y as i64 - dy as i64) as usize * stride;
        let dst = y as usize * stride;
        target.pixels.copy_within(src + x0..src + x1, dst + x0);
    };
    if dy > 0 { for y in (rect[1]..rect[3]).rev() { copy(y); } } else { for y in rect[1]..rect[3] { copy(y); } }
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
