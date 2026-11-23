//! The live CPU renderer: paints each frame into the same window buffer, repainting only the
//! tiles whose inputs changed since the frame before (`damage.rs`), on a pool of threads kept
//! between frames, and says which rectangles of the buffer changed so the window copies only
//! those to the screen.

use crate::time::Instant;

use super::bin::{Bins, TILE_H, TILE_W};
use crate::render::{INSTANCE_WORDS, Instance, Kind, word};
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
    /// The buffer's row pitch last frame, in bytes.
    stride: usize,
    dirty: Vec<u32>,
    /// Pre-scroll-merge dirty tile rectangles from the last frame painted.
    dirty_rects: Vec<[u32; 4]>,
    /// The last frame's instances and bins, for telling a scroll (`scroll.rs`) and for a frame
    /// that only recolours (`recoloured`).
    prev: Option<scroll::Prev>,
    /// The atlas's evictions last frame (`Atlas::resets`).
    resets: u64,
    /// The last frame's scene-wide switches (`Scene::flags`), from a frame the CPU could draw.
    flags: Option<super::Flags>,
    /// Scratch for `recoloured`: the instances that changed, and which tiles they mark.
    changed: Vec<u32>,
    marked: Vec<bool>,
}

/// Instances a frame may recolour and still take `recoloured`.
const RECOLOURED_MAX: usize = 256;

/// One self-copy on a retained surface: copy `width × height` pixels from `(src_x, src_y)` to
/// `(dst_x, dst_y)`.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScrollBlit {
    pub src_x: u32,
    pub src_y: u32,
    pub dst_x: u32,
    pub dst_y: u32,
    pub width: u32,
    pub height: u32,
}

/// How a host with a retained surface updates it from a painted frame: an optional self-blit of
/// the scrolled region followed by uploading `uploads` from the CPU buffer.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UploadPlan<'a> {
    pub blit: Option<ScrollBlit>,
    pub uploads: &'a [[u32; 4]],
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
    /// Of that, telling whether the frame scrolled (`scroll.rs`), and moving the rows.
    pub plan_ns: u64,
    pub move_ns: u64,
}

impl CpuPainter {
    /// A painter running on `threads` threads, the caller among them (the others are made now,
    /// and park between frames).
    pub fn new(threads: usize) -> CpuPainter {
        CpuPainter { pool: Pool::new(threads), hashes: Vec::new(), next: Vec::new(), each: Vec::new(), size: (0, 0), stride: 0, dirty: Vec::new(), dirty_rects: Vec::new(), prev: None, resets: 0, flags: None, changed: Vec::new(), marked: Vec::new() }
    }

    /// The buffer's pixels are no longer the last frame's (a new buffer): the next frame paints
    /// every tile. A buffer that keeps its row pitch when the window resizes needn't: tiles
    /// whole in both sizes keep what they held, and only those that changed paint.
    pub fn forget(&mut self) {
        self.hashes.clear();
        self.dirty.clear();
        self.dirty_rects.clear();
        self.prev = None;
        self.flags = None;
    }

    /// Fills the masks the text system queued (`fill_masks`) on this painter's threads.
    pub fn fill_masks(&self, atlas: &mut crate::text::Atlas) -> super::FillReport {
        super::fill::fill_with(atlas, &|threads| Threads { pool: &self.pool, extra: (threads - 1).min(self.pool.threads() - 1) })
    }

    /// How a host with a retained surface updates it for `painted` on a `size` surface: when the
    /// frame scrolled cleanly, returns the in-surface blit and the dirty-only rectangles to upload;
    /// otherwise returns no blit and `painted.rects`.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn upload_plan<'a>(&'a self, painted: &'a Painted, size: (u32, u32)) -> UploadPlan<'a> {
        match scroll_blit(painted.scrolled, size) {
            Some(blit) => UploadPlan { blit: Some(blit), uploads: &self.dirty_rects },
            None => UploadPlan { blit: None, uploads: &painted.rects },
        }
    }

    /// Paints `scene` into `target`, which holds the frame this painter painted last unless
    /// `forget` was called since.
    pub fn paint(&mut self, scene: &Scene<'_>, target: &mut Target<'_>) -> Result<Painted, NeedsGpu> {
        super::check_target(scene, target)?;
        let start = crate::time::now();
        // A recoloured frame draws what the last one did, so whatever it needed it still does.
        if self.flags == Some(scene.flags()) && let Some(p) = self.recoloured(scene, target, start) { return Ok(p); }
        if let Some(why) = scene.needs_gpu() { return Err(why); }
        self.flags = Some(scene.flags());
        let bins = Bins::of(scene);
        let binned = crate::time::now();
        damage::hashes(scene, &bins, &mut self.each, &mut self.next);
        // Pixels stay where they were only in rows as far apart as before.
        if self.stride != target.stride { self.hashes.clear(); }
        self.dirty.clear();
        let all = self.hashes.is_empty();
        let resized = self.size != scene.size;
        // A tile's hash last frame. After a resize, a tile keeps its place (the buffer's rows
        // don't move), so one whole in both sizes compares with itself; one cut by either
        // edge holds pixels that were never painted.
        let (old, ocols) = (self.size, self.size.0.div_ceil(super::bin::TILE_W));
        let whole = |tx: u32, ty: u32, (w, h): (u32, u32)| (tx + 1) * super::bin::TILE_W <= w && (ty + 1) * super::bin::TILE_H <= h;
        let before = |t: usize| -> Option<u64> {
            if all { return None; }
            if !resized { return Some(self.hashes[t]); }
            let (tx, ty) = (t as u32 % bins.cols, t as u32 / bins.cols);
            (whole(tx, ty, old) && whole(tx, ty, scene.size)).then(|| self.hashes[(ty * ocols + tx) as usize])
        };
        let hash_changed = |t: usize| before(t) != Some(self.next[t]);
        // A backdrop blur reads every pixel of its region: when any tile there changed, the
        // whole region paints again (what's under the blur first, then the blur).
        let mut redo = vec![false; self.next.len()];
        for (_, rec) in &bins.blurs {
            if all || super::bin::region_tiles(rec.region, bins.cols, bins.rows).any(hash_changed) {
                for t in super::bin::region_tiles(rec.region, bins.cols, bins.rows) { redo[t] = true; }
            }
        }
        let changed = |t: usize| redo[t] || hash_changed(t);
        let by_hash = (0..self.next.len()).filter(|&t| changed(t)).count();
        // Only a frame that changed many tiles can be a scroll; and a move is kept only when it
        // leaves fewer tiles to paint than the hashes alone do.
        let planning = crate::time::now();
        let shift = if all || resized || by_hash < MIN_SCROLL_TILES { None } else { self.prev.as_ref().and_then(|p| scroll::plan(p, scene, &bins, &Threads { pool: &self.pool, extra: self.pool.threads() - 1 })) };
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
        let moving = crate::time::now();
        if let Some(s) = &shift {
            let movers = (s.clean.iter().filter(|&&c| c).count() / TILES_PER_THREAD).min(self.pool.threads() - 1);
            move_tiles(&Threads { pool: &self.pool, extra: movers }, target, scene.size, bins.cols, &s.clean, s.dy);
        }
        let moved = crate::time::now();
        self.dirty.extend((0..self.next.len() as u32).filter(|&t| dirty(shift.as_ref(), t as usize)));
        std::mem::swap(&mut self.hashes, &mut self.next);
        self.size = scene.size;
        self.stride = target.stride;
        self.resets = scene.atlas_resets;
        let hashed = crate::time::now();
        let extra = (self.dirty.len() / TILES_PER_THREAD).min(self.pool.threads() - 1);
        let mut report = raster::run(scene, &bins, target, &self.dirty, &Threads { pool: &self.pool, extra }, &Threads { pool: &self.pool, extra: self.pool.threads() - 1 });
        let done = crate::time::now();
        report.bin_ns = super::ns(binned - start);
        report.raster_ns = super::ns(done - hashed);
        report.total_ns = super::ns(done - start);
        report.tiles = bins.tile_count();
        report.binned = bins.len();
        report.culled = bins.culled;
        let dirty_rects = damage::rects(&self.dirty, bins.cols, scene.size, MAX_RECTS);
        // Tile-aligned bounding box of the clean tiles moved in `touched(s, _)`: every tile is
        // either completely inside it (moved cleanly or overwritten by `dirty_rects`) or disjoint
        // from it (unchanged or overwritten by `dirty_rects`), and both source and destination
        // stay within `scene.size` because `tile_moved` verified `back` for every clean tile.
        let scrolled = shift.as_ref().and_then(|s| {
            let mut r = [u32::MAX, u32::MAX, 0u32, 0u32];
            for t in 0..self.next.len() {
                if touched(s, t) && !bins.quiet[t] && s.clean[t] {
                    let (tx, ty) = (t as u32 % bins.cols, t as u32 / bins.cols);
                    r[0] = r[0].min(tx * TILE_W);
                    r[1] = r[1].min(ty * TILE_H);
                    r[2] = r[2].max(((tx + 1) * TILE_W).min(scene.size.0));
                    r[3] = r[3].max(((ty + 1) * TILE_H).min(scene.size.1));
                }
            }
            (r[0] < r[2] && r[1] < r[3]).then_some((r, s.dy))
        });
        let mut rects = dirty_rects.clone();
        if let Some((r, _)) = scrolled { damage::add(&mut rects, r, MAX_RECTS); }
        self.dirty_rects = dirty_rects;
        self.prev = Some(scroll::Prev { instances: scene.instances.to_vec(), bins });
        Ok(Painted { dirty_tiles: self.dirty.len(), tiles: report.tiles, rects, scrolled, hash_ns: super::ns(hashed - binned), plan_ns: super::ns(moving - planning), move_ns: super::ns(moved - moving), report })
    }
}

impl CpuPainter {
    /// A frame that draws the last frame's instances, some of them in other colours (a spinner's
    /// dots, a caret's blink, a hover's fill) or with new texels (an animated image): the bins
    /// still hold, so it skips binning and hashing every tile, repaints only the tiles the
    /// changed instances reach, and rehashes only those. With nothing changed it touches
    /// nothing: no tile, no rect, nothing to present. None when the frame needs the full path.
    fn recoloured(&mut self, scene: &Scene<'_>, target: &mut Target<'_>, start: Instant) -> Option<Painted> {
        let prev = self.prev.as_mut()?;
        if self.hashes.is_empty() || self.size != scene.size || self.stride != target.stride || prev.instances.len() != scene.instances.len() || self.resets != scene.atlas_resets {
            return None;
        }
        // The whole list at memcmp's speed first: most frames that get here changed nothing.
        let images = scene.images.iter().any(|p| p.dirty.is_some());
        if !images && bytemuck::cast_slice::<Instance, u8>(scene.instances) == bytemuck::cast_slice::<Instance, u8>(&prev.instances) {
            self.dirty.clear();
            self.dirty_rects.clear();
            let mut report = Report { total_ns: super::ns(start.elapsed()), tiles: prev.bins.tile_count(), ..Report::default() };
            report.raster_ns = 0;
            return Some(Painted { tiles: report.tiles, hash_ns: report.total_ns, report, ..Painted::default() });
        }
        self.changed.clear();
        for (n, (new, old)) in scene.instances.iter().zip(&prev.instances).enumerate() {
            let same = bytemuck::bytes_of(new) == bytemuck::bytes_of(old)
                && !(new.kind() == Some(Kind::Image) && image_written(scene, new) && damage::instance(scene, &prev.bins, n) != self.each[n]);
            if same { continue; }
            if self.changed.len() == RECOLOURED_MAX || !recolour(new, old) { return None; }
            self.changed.push(n as u32);
        }
        let bins = &prev.bins;
        let tiles = bins.tile_count();
        self.marked.clear();
        self.marked.resize(tiles, false);
        for &n in &self.changed {
            let b = bins.bounds[n as usize];
            if b[2] <= b[0] || b[3] <= b[1] { continue; }
            let (tx0, tx1) = (b[0] as u32 / TILE_W, (b[2] as u32).div_ceil(TILE_W).min(bins.cols));
            for ty in b[1] as u32 / TILE_H..(b[3] as u32).div_ceil(TILE_H).min(bins.rows) {
                for tx in tx0..tx1 { self.marked[(ty * bins.cols + tx) as usize] = true; }
            }
        }
        // A blur under a changed tile is taken again: its whole region paints.
        for (_, rec) in &bins.blurs {
            if super::bin::region_tiles(rec.region, bins.cols, bins.rows).any(|t| self.marked[t]) {
                for t in super::bin::region_tiles(rec.region, bins.cols, bins.rows) { self.marked[t] = true; }
            }
        }
        self.dirty.clear();
        self.dirty.extend((0..tiles as u32).filter(|&t| self.marked[t as usize]));
        for &n in &self.changed {
            self.each[n as usize] = damage::instance(scene, bins, n as usize);
            prev.instances[n as usize] = scene.instances[n as usize];
        }
        for &t in &self.dirty {
            self.hashes[t as usize] = damage::rehash(scene, bins, &self.each, t as usize);
        }
        let hashed = crate::time::now();
        let extra = (self.dirty.len() / TILES_PER_THREAD).min(self.pool.threads() - 1);
        let mut report = if self.dirty.is_empty() { Report::default() } else { raster::run(scene, bins, target, &self.dirty, &Threads { pool: &self.pool, extra }, &Threads { pool: &self.pool, extra: self.pool.threads() - 1 }) };
        let done = crate::time::now();
        report.raster_ns = super::ns(done - hashed);
        report.total_ns = super::ns(done - start);
        report.tiles = tiles;
        let rects = damage::rects(&self.dirty, bins.cols, scene.size, MAX_RECTS);
        self.dirty_rects.clone_from(&rects);
        Some(Painted { dirty_tiles: self.dirty.len(), tiles, rects, scrolled: None, hash_ns: super::ns(hashed - start), plan_ns: 0, move_ns: 0, report })
    }
}

/// Validates `scrolled` against a `size` surface and turns it into source/destination copy bounds.
#[cfg(any(target_arch = "wasm32", test))]
fn scroll_blit(scrolled: Option<([u32; 4], i32)>, size: (u32, u32)) -> Option<ScrollBlit> {
    let ([x0, y0, x1, y1], dy) = scrolled?;
    if dy == 0 || x1 <= x0 || y1 <= y0 || x1 > size.0 || y1 > size.1 {
        return None;
    }
    let src_y0 = i64::from(y0) - i64::from(dy);
    let src_y1 = i64::from(y1) - i64::from(dy);
    if src_y0 < 0 || src_y1 > i64::from(size.1) {
        return None;
    }
    Some(ScrollBlit {
        src_x: x0,
        src_y: src_y0 as u32,
        dst_x: x0,
        dst_y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

/// Whether `new` is `old` in other colours: every word the same but its fill, a box's border and
/// `params.w` (an opacity, a text gamma), which no bin, bound or layer plan reads outside layers,
/// and opaque exactly where it was (the occluders and the tiles it covers whole stay). Layer scopes and blurs (whose opacity plans a layer) don't count.
fn recolour(new: &Instance, old: &Instance) -> bool {
    let (a, b): (&[u32; INSTANCE_WORDS], &[u32; INSTANCE_WORDS]) = (bytemuck::cast_ref(new), bytemuck::cast_ref(old));
    // The border is a colour only for boxes and gradients (a glyph's names its outline).
    let (border, shading_w) = match new.kind() {
        Some(Kind::Layer | Kind::LayerEnd | Kind::BackdropBlur | Kind::Effect) | None => return false,
        Some(Kind::Rect) => (true, true),
        Some(Kind::Glyph) => (false, true),
        Some(Kind::Gradient | Kind::Dashed | Kind::Radial) => (true, false),
        Some(Kind::Shadow | Kind::Inset | Kind::Segment | Kind::Triangle | Kind::Image) => (false, false),
    };
    (0..INSTANCE_WORDS).all(|k| {
        word::FILL.contains(&k)
            || (word::BORDER.contains(&k) && border)
            || (k == word::PARAMS_W && shading_w)
            || a[k] == b[k]
    }) && super::bin::opaque_box(new) == super::bin::opaque_box(old)
}

/// Whether rows `new` samples were written into its image page since the renderer last took it.
fn image_written(scene: &Scene<'_>, i: &Instance) -> bool {
    let Some(page) = scene.images.get(i.params[0] as usize) else { return false };
    let (y0, y1) = (i.uv[1].max(0.0) as u32, (i.uv[1] + i.uv[3]).max(0.0).ceil() as u32);
    page.dirty.is_some_and(|(a, b)| a < y1 && b > y0)
}

/// Minimum vertical shift (in rows) before `move_tiles` splits work into parallel row waves.
/// Smaller shifts copy sequentially with merged row spans so rows stay hot in cache without
/// paying per-wave thread-pool barriers.
const MIN_WAVE_ROWS: u32 = TILE_H * 2;

struct MoveGrid<'a> {
    size: (u32, u32),
    stride: usize,
    cols: u32,
    clean: &'a [bool],
}

impl MoveGrid<'_> {
    fn row_clean(&self, ty: u32) -> &[bool] {
        let a = (ty * self.cols) as usize;
        &self.clean[a..a + self.cols as usize]
    }

    fn any_clean_in_rows(&self, y0: u32, y1: u32) -> bool {
        let a = ((y0 / TILE_H) * self.cols) as usize;
        let b = (y1.div_ceil(TILE_H) * self.cols) as usize;
        self.clean[a..b].iter().any(|&c| c)
    }

    /// Copies every contiguous run of clean tiles in tile-row `ty` from `src_row` to `dst_row`.
    fn copy_spans(&self, dst_row: &mut [u8], src_row: &[u8], ty: u32) {
        let row_clean = self.row_clean(ty);
        let mut tx = 0u32;
        while tx < self.cols {
            if !row_clean[tx as usize] {
                tx += 1;
                continue;
            }
            let start = tx;
            while tx < self.cols && row_clean[tx as usize] {
                tx += 1;
            }
            let x0 = (start * TILE_W) as usize * 4;
            let x1 = (tx * TILE_W).min(self.size.0) as usize * 4;
            dst_row[x0..x1].copy_from_slice(&src_row[x0..x1]);
        }
    }

    /// Copies one disjoint wave of destination rows starting at `dst_start` from `src_buf` (whose
    /// first byte is row `src_origin`) across `exec`'s threads.
    fn copy_wave(
        &self,
        exec: &dyn raster::Exec,
        dst_buf: &mut [u8],
        src_buf: &[u8],
        dst_start: u32,
        src_origin: u32,
        dy: i32,
    ) {
        use std::sync::Mutex;
        let stride = self.stride;
        let row_bytes = self.size.0 as usize * 4;
        let shift = dy.unsigned_abs();
        let total = dst_buf.len().div_ceil(stride);
        let workers = exec.threads().max(1);
        let band = (total / (workers * 2)).max(8).min(total.max(1));
        let work = Mutex::new((dst_start, dst_buf.chunks_mut(band * stride)));
        exec.run(&|| loop {
            let next = {
                let mut guard = work.lock().unwrap_or_else(|e| e.into_inner());
                guard.1.next().map(|chunk| {
                    let y = guard.0;
                    guard.0 += chunk.len().div_ceil(stride) as u32;
                    (y, chunk)
                })
            };
            let Some((y_start, chunk)) = next else { break };
            for (i, dst_row) in chunk.chunks_mut(stride).enumerate() {
                let y = y_start + i as u32;
                let ty = y / TILE_H;
                if !self.row_clean(ty).iter().any(|&c| c) { continue; }
                let src_y = if dy > 0 { y - shift } else { y + shift };
                let src_at = (src_y - src_origin) as usize * stride;
                let src_row = &src_buf[src_at..src_at + row_bytes];
                self.copy_spans(dst_row, src_row, ty);
            }
        });
    }

    /// Copies clean tile runs row by row in-place via `copy_within`, merging adjacent clean tiles
    /// into contiguous horizontal spans.
    fn move_sequential(&self, pixels: &mut [u8], dy: i32) {
        let tile_rows = self.size.1.div_ceil(TILE_H);
        let shift = dy.unsigned_abs();
        let copy_row = |pixels: &mut [u8], ty: u32, y: u32, src_y: u32| {
            let row_clean = self.row_clean(ty);
            let dst_row = y as usize * self.stride;
            let src_row = src_y as usize * self.stride;
            let mut tx = 0u32;
            while tx < self.cols {
                if !row_clean[tx as usize] {
                    tx += 1;
                    continue;
                }
                let start = tx;
                while tx < self.cols && row_clean[tx as usize] {
                    tx += 1;
                }
                let x0 = (start * TILE_W) as usize * 4;
                let x1 = (tx * TILE_W).min(self.size.0) as usize * 4;
                pixels.copy_within(src_row + x0..src_row + x1, dst_row + x0);
            }
        };
        if dy > 0 {
            for ty in (0..tile_rows).rev() {
                if !self.row_clean(ty).iter().any(|&c| c) { continue; }
                let (a, b) = (ty * TILE_H, ((ty + 1) * TILE_H).min(self.size.1));
                for y in (a..b).rev() {
                    copy_row(pixels, ty, y, y - shift);
                }
            }
        } else {
            for ty in 0..tile_rows {
                if !self.row_clean(ty).iter().any(|&c| c) { continue; }
                let (a, b) = (ty * TILE_H, ((ty + 1) * TILE_H).min(self.size.1));
                for y in a..b {
                    copy_row(pixels, ty, y, y + shift);
                }
            }
        }
    }
}

/// Moves the tiles the scroll left clean `dy` rows (down when positive): each takes what was
/// `dy` rows back. Adjacent clean tiles on a row copy together as one contiguous span. Larger
/// shifts (`|dy| >= MIN_WAVE_ROWS`) split into disjoint `|dy|`-row waves whose source and
/// destination slices don't overlap, running each wave across `threads`'s workers in row order
/// that reads every row before it is written (bottom up for a move down); smaller shifts copy
/// sequentially in place. The rest repaint, so they aren't moved.
pub(super) fn move_tiles(threads: &dyn raster::Exec, target: &mut Target<'_>, size: (u32, u32), cols: u32, clean: &[bool], dy: i32) {
    let shift = dy.unsigned_abs();
    if shift == 0 || shift >= size.1 || size.0 == 0 { return; }
    let grid = MoveGrid { size, stride: target.stride, cols, clean };
    if threads.threads() <= 1 || shift < MIN_WAVE_ROWS {
        grid.move_sequential(target.pixels, dy);
        return;
    }
    if dy > 0 {
        let mut dst_end = size.1;
        while dst_end > shift {
            let dst_start = dst_end.saturating_sub(shift).max(shift);
            if grid.any_clean_in_rows(dst_start, dst_end) {
                let split = dst_start as usize * grid.stride;
                let (src_buf, dst_rest) = target.pixels.split_at_mut(split);
                let wave_bytes = ((dst_end - dst_start) as usize * grid.stride).min(dst_rest.len());
                grid.copy_wave(threads, &mut dst_rest[..wave_bytes], src_buf, dst_start, 0, dy);
            }
            dst_end = dst_start;
        }
    } else {
        let limit = size.1 - shift;
        let mut dst_start = 0u32;
        while dst_start < limit {
            let dst_end = (dst_start + shift).min(limit);
            if grid.any_clean_in_rows(dst_start, dst_end) {
                let split = dst_end as usize * grid.stride;
                let (dst_all, src_buf) = target.pixels.split_at_mut(split);
                let dst_buf = &mut dst_all[dst_start as usize * grid.stride..];
                grid.copy_wave(threads, dst_buf, src_buf, dst_start, dst_end, dy);
            }
            dst_start = dst_end;
        }
    }
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
