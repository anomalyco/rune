//! Windfoil's atlas masks filled on the CPU, across threads, before any renderer exists: the
//! fills the text system queued for the GPU (`text::atlas_fill`), each mask the bytes the fill's
//! shader writes for it (`outline.wgsl`'s `outline_glyph`, through `text::outline::fill_shared`,
//! its CPU form).
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

/// Texels a thread fills before another is worth waking: about 50 µs of work (a parked pool
/// thread wakes in a few µs; one made for the job costs tens).
pub(super) const TEXELS_PER_THREAD: u64 = 1_500;

/// `fill_masks` on the threads `exec` gives for this many.
pub(super) fn fill_with<E: super::raster::Exec>(atlas: &mut Atlas, exec: &dyn Fn(usize) -> E) -> FillReport {
    let started = Instant::now();
    let fills: Vec<(usize, QueuedFill)> = atlas.take_cpu_fills().into_iter().flat_map(|(page, f)| f.into_iter().map(move |q| (page, q))).collect();
    let queued: Vec<QueuedFill> = fills.iter().map(|f| f.1).collect();
    // Masks of one glyph at several x phases share their rows (`outline::share_rows`): a group is
    // one piece of work. The biggest first, so the last ones a thread picks up are small.
    let mut groups = outline::share_rows(&queued);
    let size = |g: &Vec<u32>| g.iter().map(|&i| u64::from(queued[i as usize].rect[2] * queued[i as usize].rect[3])).sum::<u64>();
    groups.sort_by_key(|g| std::cmp::Reverse(size(g)));
    let texels: u64 = groups.iter().map(size).sum();
    let next = AtomicUsize::new(0);
    let done: Vec<Mutex<Vec<Vec<u8>>>> = groups.iter().map(|_| Mutex::new(Vec::new())).collect();
    let store = &atlas.outlines;
    let worker = || loop {
        let k = next.fetch_add(1, Ordering::Relaxed);
        let Some(group) = groups.get(k) else { break };
        let mut masks: Vec<Vec<u8>> = group.iter().map(|&i| Vec::with_capacity((queued[i as usize].rect[2] * queued[i as usize].rect[3]) as usize)).collect();
        outline::fill_shared::<outline::F4>(store, &queued, group, &mut masks, None);
        *done[k].lock().unwrap_or_else(|e| e.into_inner()) = masks;
    };
    let exec = exec(texels.div_ceil(TEXELS_PER_THREAD).max(1) as usize);
    let threads = exec.threads();
    if !fills.is_empty() { exec.run(&worker); }
    let covered = Instant::now();
    for (group, masks) in groups.iter().zip(done) {
        for (&i, mask) in group.iter().zip(masks.into_inner().unwrap_or_else(|e| e.into_inner())) {
            let (page, q) = &fills[i as usize];
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
        }
    }
    let end = Instant::now();
    FillReport { threads, masks: fills.len(), texels, cover_ns: super::ns(covered - started), copy_ns: super::ns(end - covered) }
}