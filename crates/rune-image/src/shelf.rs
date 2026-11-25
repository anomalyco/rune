//! Vertical-band shelf allocator and `ImagePage` backing storage for `ImageAtlas`.
//!
//! Each shelf page (`IMAGE_ATLAS_SIZE × IMAGE_ATLAS_SIZE`) is partitioned top-to-bottom into
//! contiguous bands from `y = 0` to `used_rows`:
//! - Active bands (`live > 0`) hold slots of height `<= band.h` and maintain sorted, coalesced
//!   free horizontal spans `[x0, x1)` within `0..IMAGE_ATLAS_SIZE`.
//! - Empty bands (`live == 0`) represent freed vertical space; adjacent empty bands coalesce and
//!   trailing empty bands pop, shrinking `used_rows` so bottom rows can be reclaimed at rest or
//!   under memory pressure without moving any live slot.

use crate::pixels::{IMAGE_ATLAS_SIZE, IMAGE_PAGE_START_ROWS};

struct Band {
    y: u32,
    h: u32,
    /// Sorted, non-overlapping, coalesced free horizontal spans `[x0, x1)`.
    free: Vec<(u32, u32)>,
    /// Live slots (plus any mid-frame deferred-free rects) currently occupying this band.
    live: u32,
}

#[derive(Default)]
pub(crate) struct Shelves {
    bands: Vec<Band>,
    /// Rows occupied from `0..used_rows` on a shelf page, or `0`/`h` on a standalone page.
    used_rows: u32,
}

impl Shelves {
    pub(crate) fn used_rows(&self) -> u32 {
        self.used_rows
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.used_rows == 0
    }

    /// Whether this shelf page has any unallocated vertical rows or free horizontal spans.
    pub(crate) fn has_free_space(&self) -> bool {
        self.used_rows < IMAGE_ATLAS_SIZE || self.bands.iter().any(|b| b.live == 0 || !b.free.is_empty())
    }

    pub(crate) fn reset(&mut self) {
        self.bands.clear();
        self.used_rows = 0;
    }

    fn alloc_standalone(&mut self, page_w: u32, page_h: u32, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.used_rows == 0 && w == page_w && h == page_h {
            self.used_rows = h;
            Some((0, 0))
        } else {
            None
        }
    }

    fn alloc_shelf(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w == 0 || h == 0 || w > IMAGE_ATLAS_SIZE || h > IMAGE_ATLAS_SIZE {
            return None;
        }
        // 1. Best-height active band within a tight height envelope so small icons don't fragment tall shelves.
        let max_tight = h.saturating_mul(2).max(h.saturating_add(32));
        if let Some((bi, fi)) = self.find_active_band(w, h, max_tight) {
            return Some(self.take_from_band(bi, fi, w));
        }
        // 2. Smallest coalesced empty band tall enough: split off top `h` rows.
        if let Some(bi) = self.bands.iter().enumerate()
            .filter(|(_, b)| b.live == 0 && b.h >= h)
            .min_by_key(|(_, b)| b.h)
            .map(|(i, _)| i)
        {
            let y = self.bands[bi].y;
            let rem = self.bands[bi].h - h;
            self.bands[bi].h = h;
            self.bands[bi].live = 1;
            self.bands[bi].free.clear();
            if w < IMAGE_ATLAS_SIZE {
                self.bands[bi].free.push((w, IMAGE_ATLAS_SIZE));
            }
            if rem > 0 {
                self.bands.insert(bi + 1, Band {
                    y: y + h,
                    h: rem,
                    free: vec![(0, IMAGE_ATLAS_SIZE)],
                    live: 0,
                });
            }
            return Some((0, y));
        }
        // 3. Append a new band below `used_rows` if it fits inside `IMAGE_ATLAS_SIZE`.
        if self.used_rows.saturating_add(h) <= IMAGE_ATLAS_SIZE {
            let y = self.used_rows;
            let mut free = Vec::new();
            if w < IMAGE_ATLAS_SIZE {
                free.push((w, IMAGE_ATLAS_SIZE));
            }
            self.bands.push(Band { y, h, free, live: 1 });
            self.used_rows = y + h;
            return Some((0, y));
        }
        // 4. Fallback: any active band tall enough when the page has no vertical room left.
        if let Some((bi, fi)) = self.find_active_band(w, h, IMAGE_ATLAS_SIZE) {
            return Some(self.take_from_band(bi, fi, w));
        }
        None
    }

    fn free(&mut self, is_shelf: bool, x: u32, y: u32, w: u32) {
        if !is_shelf {
            self.used_rows = 0;
            return;
        }
        let Ok(idx) = self.bands.binary_search_by_key(&y, |b| b.y) else { return; };
        let band = &mut self.bands[idx];
        band.live = band.live.saturating_sub(1);
        if band.live == 0 {
            band.free.clear();
            band.free.push((0, IMAGE_ATLAS_SIZE));
            if idx + 1 < self.bands.len() && self.bands[idx + 1].live == 0 {
                self.bands[idx].h += self.bands[idx + 1].h;
                self.bands.remove(idx + 1);
            }
            if idx > 0 && self.bands[idx - 1].live == 0 {
                self.bands[idx - 1].h += self.bands[idx].h;
                self.bands.remove(idx);
            }
            while self.bands.last().is_some_and(|b| b.live == 0) {
                self.bands.pop();
            }
            self.used_rows = self.bands.last().map_or(0, |b| b.y + b.h);
            return;
        }
        let (lo, hi) = (x, x + w);
        let pos = band.free.partition_point(|&(a, _)| a < lo);
        let merge_prev = pos > 0 && band.free[pos - 1].1 == lo;
        let merge_next = pos < band.free.len() && band.free[pos].0 == hi;
        match (merge_prev, merge_next) {
            (true, true) => {
                band.free[pos - 1].1 = band.free[pos].1;
                band.free.remove(pos);
            }
            (true, false) => band.free[pos - 1].1 = hi,
            (false, true) => band.free[pos].0 = lo,
            (false, false) => band.free.insert(pos, (lo, hi)),
        }
    }

    fn find_active_band(&self, w: u32, h: u32, max_h: u32) -> Option<(usize, usize)> {
        self.bands.iter().enumerate()
            .filter(|(_, b)| b.live > 0 && b.h >= h && b.h <= max_h)
            .filter_map(|(bi, b)| {
                let fi = b.free.iter().position(|&(a, c)| c - a >= w)?;
                Some((bi, fi, b.h - h))
            })
            .min_by_key(|&(_, _, waste)| waste)
            .map(|(bi, fi, _)| (bi, fi))
    }

    fn take_from_band(&mut self, bi: usize, fi: usize, w: u32) -> (u32, u32) {
        let band = &mut self.bands[bi];
        let (x0, x1) = band.free[fi];
        if x0 + w == x1 {
            band.free.remove(fi);
        } else {
            band.free[fi].0 = x0 + w;
        }
        band.live += 1;
        (x0, band.y)
    }
}

/// One stable shelf page, or an exact-size standalone page for a large image.
pub struct ImagePage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    pub(crate) shelves: Shelves,
    /// Drawn from in the last completed frame.
    pub(crate) shown: bool,
    /// Drawn from in this frame so far; only ever set until the frame ends.
    pub(crate) touched: bool,
}

impl ImagePage {
    fn initial_size(w: u32, h: u32) -> (u32, u32) {
        if w > IMAGE_ATLAS_SIZE || h > IMAGE_ATLAS_SIZE { (w, h) }
        else { (IMAGE_ATLAS_SIZE, h.next_power_of_two().max(IMAGE_PAGE_START_ROWS)) }
    }

    pub(crate) fn initial_bytes(w: u32, h: u32) -> usize {
        let (pw, ph) = Self::initial_size(w, h);
        (pw as usize).saturating_mul(ph as usize).saturating_mul(4)
    }

    pub(crate) fn is_shelf(&self) -> bool {
        self.width == IMAGE_ATLAS_SIZE && self.height <= IMAGE_ATLAS_SIZE
    }

    /// Whether this page might accept a relocated slot without evicting a currently shown page.
    pub(crate) fn has_free_space(&self) -> bool {
        self.data.is_empty() || !self.shown || (self.is_shelf() && self.shelves.has_free_space())
    }

    /// A shelf page starts with the rows its first image needs (at least
    /// `IMAGE_PAGE_START_ROWS`) and grows as its shelves fill (`allocate`), up to
    /// `IMAGE_ATLAS_SIZE`: a window that shows a few icons holds 512 KiB of it, not 4 MiB.
    pub(crate) fn new(w: u32, h: u32) -> Self {
        let (width, height) = Self::initial_size(w, h);
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize * 4],
            dirty: None,
            shelves: Shelves::default(),
            shown: false,
            touched: false,
        }
    }

    pub(crate) fn free_rect(&mut self, x: u32, y: u32, w: u32) {
        if !self.data.is_empty() {
            self.shelves.free(self.is_shelf(), x, y, w);
        }
    }

    pub(crate) fn empty(&mut self) {
        self.shelves.reset();
        self.dirty = None;
        self.data = Vec::new();
    }

    pub(crate) fn shrink_to_fit_rows(&mut self) {
        if self.data.is_empty() || !self.is_shelf() { return; }
        let target_h = self.shelves.used_rows().next_power_of_two().clamp(IMAGE_PAGE_START_ROWS, IMAGE_ATLAS_SIZE);
        if target_h < self.height {
            self.height = target_h;
            self.data.truncate(self.width as usize * self.height as usize * 4);
            self.data.shrink_to_fit();
            self.dirty = Some((0, self.height));
        }
    }

    pub(crate) fn allocate(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if !self.is_shelf() {
            return self.shelves.alloc_standalone(self.width, self.height, w, h);
        }
        let at = self.shelves.alloc_shelf(w, h)?;
        if self.shelves.used_rows() > self.height {
            self.height = self.shelves.used_rows().next_power_of_two().min(IMAGE_ATLAS_SIZE);
            self.data.resize(self.width as usize * self.height as usize * 4, 0);
            self.dirty = Some((0, self.height));
        }
        Some(at)
    }
}
