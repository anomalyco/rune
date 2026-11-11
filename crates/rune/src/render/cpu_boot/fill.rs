//! Windfoil's atlas masks filled on the CPU, across threads, before any renderer exists: the
//! fills the text system queued for the GPU (`text::atlas_fill`), each texel the coverage the
//! fill's shader computes for it (`outline.wgsl`'s `outline_glyph`, through
//! `text::outline::coverage`, its CPU form).
//!
//! A filled mask becomes one the CPU drew (`Residency::Uploaded`) and its rows are marked for
//! upload: a renderer that arrives later uploads these texels and samples them, never filling
//! them again, so text at the hand-over is the same texels on both sides.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::text::{ATLAS_SIZE, Atlas, QueuedFill, outline};

/// Tells the text system its masks will be filled from outlines, as a Windfoil renderer's
/// `attach_text` does, so the first frame's glyphs queue fills for [`fill_masks`] instead of
/// filling them one by one on the thread that lays the frame out.
pub fn attach_text(atlas: &mut Atlas) {
    atlas.outlines.set_support(outline::Support::Drawn);
}

/// How a `fill_masks` went.
#[derive(Clone, Debug, Default)]
pub struct FillReport {
    pub threads: usize,
    pub masks: usize,
    pub texels: u64,
    /// Computing every texel's coverage, all threads, wall clock.
    pub cover_ns: u64,
    /// Copying the masks into their pages.
    pub copy_ns: u64,
}

/// Fills every mask the text system queued for the GPU (`attach_text`), on `threads` threads.
pub fn fill_masks(atlas: &mut Atlas, threads: usize) -> FillReport {
    fill_with(atlas, &|n| super::raster::Spawn(n.min(threads.max(1))))
}

/// Masks a thread fills before another is worth waking.
pub(super) const MASKS_PER_THREAD: usize = 8;

/// `fill_masks` on the threads `exec` gives for this many masks.
pub(super) fn fill_with<E: super::raster::Exec>(atlas: &mut Atlas, exec: &dyn Fn(usize) -> E) -> FillReport {
    let started = Instant::now();
    let fills: Vec<(usize, QueuedFill)> = atlas.take_cpu_fills().into_iter().flat_map(|(page, f)| f.into_iter().map(move |q| (page, q))).collect();
    // The biggest first, so the last ones a thread picks up are small.
    let mut order: Vec<usize> = (0..fills.len()).collect();
    order.sort_by_key(|&n| std::cmp::Reverse(fills[n].1.rect[2] * fills[n].1.rect[3]));
    let next = AtomicUsize::new(0);
    let done: Vec<Mutex<Vec<u8>>> = (0..fills.len()).map(|_| Mutex::new(Vec::new())).collect();
    let store = &atlas.outlines;
    let worker = || loop {
        let k = next.fetch_add(1, Ordering::Relaxed);
        let Some(&n) = order.get(k) else { break };
        let texels = cover(store, &fills[n].1);
        *done[n].lock().unwrap_or_else(|e| e.into_inner()) = texels;
    };
    let exec = exec(fills.len().div_ceil(MASKS_PER_THREAD).max(1));
    let threads = exec.threads();
    if !fills.is_empty() { exec.run(&worker); }
    let covered = Instant::now();
    let mut texels = 0;
    for ((page, q), mask) in fills.iter().zip(done) {
        let mask = mask.into_inner().unwrap_or_else(|e| e.into_inner());
        let (data, dirty) = if *page == 0 { (&mut atlas.data, &mut atlas.dirty) } else {
            let Some(p) = atlas.pages.get_mut(page - 1) else { continue };
            (&mut p.data, &mut p.dirty)
        };
        let [x, y, w, h] = q.rect;
        for row in 0..h {
            let at = ((y + row) * ATLAS_SIZE + x) as usize;
            data[at..at + w as usize].copy_from_slice(&mask[(row * w) as usize..((row + 1) * w) as usize]);
        }
        let rows = (y - 1, y + h + 1);
        let d = dirty.get_or_insert(rows);
        *d = (d.0.min(rows.0), d.1.max(rows.1));
        texels += u64::from(w * h);
    }
    let end = Instant::now();
    FillReport { threads, masks: fills.len(), texels, cover_ns: super::ns(covered - started), copy_ns: super::ns(end - covered) }
}

/// One mask's texels, row by row: the GPU fill's own bytes (`outline::fill_mask`, the text
/// system's one definition of a mask, subpixel masks included).
fn cover(store: &outline::OutlineStore, q: &QueuedFill) -> Vec<u8> {
    let [x, y, w, h] = q.rect;
    let mut out = Vec::with_capacity((w * h) as usize);
    outline::fill_mask(store, &q.fill, [x, y], w, h, &mut out);
    out
}