//! The glyph atlas: shelf-packed greyscale pages of glyph and icon masks, aged and evicted by
//! use, and the quads that sample them.

use super::*;

pub const ATLAS_SIZE: u32 = 2048;

/// Looked up for every glyph of every frame, so hashed with Fx rather than SipHash.
pub(super) type GlyphMap<V> = HashMap<GlyphKey, V, std::hash::BuildHasherDefault<rune_motion::Fx>>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct GlyphKey {
    pub(super) font: u64,
    pub(super) index: u32,
    pub(super) glyph: u32,
    pub(super) size: u32,
    pub(super) coords: u64,
    /// The x phase in sixtieths of a pixel (`coretext_match::PHASES`).
    pub(super) subpx: u8,
    /// The dilation radius in device px, as f32 bits.
    pub(super) dilation: u32,
    /// The CoreGraphics pipeline's stages (`macos_smoothing::Smoothing::key`).
    pub(super) apple: u8,
    /// Synthetic oblique, in degrees (0 for none).
    pub(super) skew: u8,
}

#[derive(Clone, Copy)]
pub(super) struct AtlasGlyph {
    pub(super) page: u16,
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) left: i32,
    pub(super) top: i32,
}

/// Steady-state shelf pages (page 0 + one secondary page for ping-pong eviction).
pub(super) const MAX_STEADY_PAGES: usize = 2;
/// Hard ceiling on total glyph atlas pages if a single frame or unframed loop overflows.
pub(super) const MAX_OVERFLOW_PAGES: usize = 8;
/// CPU byte budget for cached RGBA colour glyphs (`color_pixels`) before frame-boundary eviction.
pub(super) const COLOR_GLYPH_BUDGET: usize = 16 * 1024 * 1024;

pub struct AtlasPage {
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    pub(super) shelf_x: u32,
    pub(super) shelf_y: u32,
    pub(super) shelf_h: u32,
    pub(super) shown: bool,
    pub(super) touched: bool,
    pub(super) full: bool,
    pub(super) age: u64,
    pub(super) live_texels: u32,
}

impl AtlasPage {
    pub(super) fn new(age: u64) -> Self {
        AtlasPage {
            data: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            dirty: None,
            shelf_x: 1,
            shelf_y: 1,
            shelf_h: 0,
            shown: false,
            touched: false,
            full: false,
            age,
            live_texels: 0,
        }
    }

    pub(super) fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.shelf_x + w + 1 > ATLAS_SIZE {
            self.shelf_y += self.shelf_h + 1;
            self.shelf_x = 1;
            self.shelf_h = 0;
        }
        if self.shelf_y + h + 1 > ATLAS_SIZE {
            self.full = true;
            return None;
        }
        let pos = (self.shelf_x, self.shelf_y);
        self.shelf_x += w + 1;
        self.shelf_h = self.shelf_h.max(h);
        Some(pos)
    }
}

pub(super) enum Rasterized {
    Empty,
    Placed(AtlasGlyph),
    Unplaced,
}

/// One glyph to draw: device-pixel rect, atlas origin, brush and source byte (for reveals).
#[derive(Clone, Copy, Debug)]
pub struct GlyphQuad {
    pub rect: [f32; 4],
    /// Atlas origin with the page packed into y (`page * ATLAS_SIZE + y`).
    /// Use [`Self::atlas_page`] and [`Self::atlas_uv`] when sampling an atlas page directly.
    pub uv: [f32; 2],
    pub ink: u16,
    pub byte: u32,
    /// A colour glyph (emoji) drawn from [`TextSystem::color_glyph`] instead of the alpha
    /// atlas: 0 for none, else its index plus one.
    pub color: u32,
}

impl GlyphQuad {
    /// Alpha atlas page. Placements remain valid until the next frame begins.
    pub fn atlas_page(&self) -> usize { self.uv[1] as usize / ATLAS_SIZE as usize }

    /// Pixel origin within [`Self::atlas_page`], without the packed page index.
    pub fn atlas_uv(&self) -> [f32; 2] {
        [self.uv[0], self.uv[1] - (self.atlas_page() * ATLAS_SIZE as usize) as f32]
    }
}

/// A colour glyph's RGBA image (`GlyphQuad::color`) and its placement around the pen (device px).
#[derive(Clone, Copy)]
pub(super) struct ColorGlyph {
    pub(super) index: u32,
    pub(super) used: bool,
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) w: u32,
    pub(super) h: u32,
}

impl ColorGlyph {
    pub(super) fn quad(&self, x: f32, y: f32, ink: u16, byte: u32) -> GlyphQuad {
        let rect = [x.round() + self.left as f32, y.round() - self.top as f32, self.w as f32, self.h as f32];
        GlyphQuad { rect, uv: [0.0, 0.0], ink, byte, color: self.index }
    }
}

pub struct Atlas {
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    /// `(node_id, signature)` of `El::cache` targets currently resident in the GPU renderer.
    pub(crate) resident_caches: HashMap<u64, u64>,
    /// Secondary and overflow shelf pages (`page >= 1` lives at `pages[page - 1]`).
    pub(crate) pages: Vec<AtlasPage>,
    pub(crate) resets: u64,
    pub(crate) rasterized: u64,
    pub(super) shelf_x: u32,
    pub(super) shelf_y: u32,
    pub(super) shelf_h: u32,
    pub(super) shown: bool,
    pub(super) touched: bool,
    pub(super) full: bool,
    pub(super) age: u64,
    pub(super) live_texels: u32,
    pub(super) clock: u64,
    pub(super) map: GlyphMap<Option<AtlasGlyph>>,
    pub(super) icons: HashMap<(usize, u32), Option<AtlasGlyph>>,
}

impl Atlas {
    /// Marks every row for upload (a new texture, e.g. after the window's format changed).
    pub fn invalidate(&mut self) {
        self.dirty = Some((0, ATLAS_SIZE));
        for page in &mut self.pages {
            page.dirty = Some((0, ATLAS_SIZE));
        }
        self.resident_caches.clear();
    }

    /// Number of allocated 2048×2048 shelf pages (at least 1).
    pub fn page_count(&self) -> usize {
        1 + self.pages.len()
    }

    /// Alpha texels for a page returned by [`GlyphQuad::atlas_page`].
    pub fn page_data(&self, page: usize) -> Option<&[u8]> {
        if page == 0 { Some(&self.data) } else { self.pages.get(page - 1).map(|p| p.data.as_slice()) }
    }

    /// Number of shelf page resets since the atlas was created.
    pub fn resets(&self) -> u64 {
        self.resets
    }

    /// Number of glyphs and icons placed into the atlas since creation.
    pub fn rasterized(&self) -> u64 {
        self.rasterized
    }

    /// Rasterized glyph entries, for tests that bound the atlas.
    #[cfg(test)]
    pub(crate) fn glyph_entries(&self) -> usize { self.map.len() }

    pub(super) fn new() -> Self {
        Atlas {
            data: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            dirty: None,
            resident_caches: HashMap::new(),
            pages: Vec::new(),
            resets: 0,
            rasterized: 0,
            shelf_x: 1,
            shelf_y: 1,
            shelf_h: 0,
            shown: false,
            touched: false,
            full: false,
            age: 1,
            live_texels: 0,
            clock: 1,
            map: GlyphMap::default(),
            icons: HashMap::new(),
        }
    }

    pub(super) fn alloc_page0(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.shelf_x + w + 1 > ATLAS_SIZE {
            self.shelf_y += self.shelf_h + 1;
            self.shelf_x = 1;
            self.shelf_h = 0;
        }
        if self.shelf_y + h + 1 > ATLAS_SIZE {
            self.full = true;
            return None;
        }
        let pos = (self.shelf_x, self.shelf_y);
        self.shelf_x += w + 1;
        self.shelf_h = self.shelf_h.max(h);
        Some(pos)
    }

    pub(super) fn touch(&mut self, page: u16, texels: u32) {
        if page == 0 {
            self.touched = true;
            self.live_texels = self.live_texels.saturating_add(texels);
        } else if let Some(p) = self.pages.get_mut(page as usize - 1) {
            p.touched = true;
            p.live_texels = p.live_texels.saturating_add(texels);
        }
    }

    pub(super) fn page_age(&self, page: usize) -> u64 {
        if page == 0 { self.age } else { self.pages[page - 1].age }
    }

    pub(super) fn page_full(&self, page: usize) -> bool {
        if page == 0 { self.full } else { self.pages[page - 1].full }
    }

    pub(super) fn page_touched(&self, page: usize) -> bool {
        if page == 0 { self.touched } else { self.pages[page - 1].touched }
    }

    pub(super) fn page_shown(&self, page: usize) -> bool {
        if page == 0 { self.shown } else { self.pages[page - 1].shown }
    }

    pub(super) fn page_non_empty(&self, page: usize) -> bool {
        if page == 0 {
            self.shelf_x > 1 || self.shelf_y > 1 || self.shelf_h > 0
        } else {
            let p = &self.pages[page - 1];
            p.shelf_x > 1 || p.shelf_y > 1 || p.shelf_h > 0
        }
    }

    pub(super) fn evict_page(&mut self, page: usize) {
        let p16 = page as u16;
        self.map.retain(|_, v| v.is_none_or(|g| g.page != p16));
        self.icons.retain(|_, v| v.is_none_or(|g| g.page != p16));
        self.clock += 1;
        self.resets += 1;
        if page == 0 {
            self.shelf_x = 1;
            self.shelf_y = 1;
            self.shelf_h = 0;
            self.dirty = None;
            self.shown = false;
            self.touched = false;
            self.full = false;
            self.age = self.clock;
            self.live_texels = 0;
        } else if let Some(p) = self.pages.get_mut(page - 1) {
            p.shelf_x = 1;
            p.shelf_y = 1;
            p.shelf_h = 0;
            p.dirty = None;
            p.shown = false;
            p.touched = false;
            p.full = false;
            p.age = self.clock;
            p.live_texels = 0;
        }
    }

    pub(crate) fn begin_frame(&mut self, pinned_mask: u32) {
        let total_live = self.live_texels as u64
            + self.pages.iter().map(|p| p.live_texels as u64).sum::<u64>();
        let half_page = (ATLAS_SIZE * ATLAS_SIZE / 2) as u64;
        let needed = ((total_live / half_page) as usize + MAX_STEADY_PAGES)
            .clamp(MAX_STEADY_PAGES, MAX_OVERFLOW_PAGES);
        let total = self.page_count();
        let full_count = (0..total).filter(|&p| self.page_full(p)).count();
        if full_count >= needed || total > needed {
            let keep = needed.min(total);
            if let Some(oldest) = (0..keep)
                .filter(|&p| self.page_full(p) && (pinned_mask & (1u32 << p.min(31))) == 0)
                .min_by_key(|&p| self.page_age(p))
            {
                self.evict_page(oldest);
            }
            for p in keep..total {
                if (pinned_mask & (1u32 << p.min(31))) == 0 && self.page_non_empty(p) {
                    self.evict_page(p);
                }
            }
            while self.page_count() > needed {
                let last = self.page_count() - 1;
                if (pinned_mask & (1u32 << last.min(31))) != 0 || self.page_non_empty(last) {
                    break;
                }
                self.pages.pop();
            }
        }
        self.shown = std::mem::take(&mut self.touched);
        self.live_texels = 0;
        for p in &mut self.pages {
            p.shown = std::mem::take(&mut p.touched);
            p.live_texels = 0;
        }
        for p in 0..self.page_count() {
            if (pinned_mask & (1u32 << p.min(31))) != 0 {
                self.touch(p as u16, 0);
            }
        }
    }

    pub(super) fn alloc(&mut self, w: u32, h: u32) -> Option<(usize, u32, u32)> {
        if w == 0 || h == 0 || w + 2 > ATLAS_SIZE || h + 2 > ATLAS_SIZE {
            return None;
        }
        if !self.full && let Some((x, y)) = self.alloc_page0(w, h) {
            return Some((0, x, y));
        }
        for (i, page) in self.pages.iter_mut().enumerate() {
            if !page.full && let Some((x, y)) = page.alloc(w, h) {
                return Some((i + 1, x, y));
            }
        }
        // Try shelf tails on full pages for small glyphs before opening or resetting a page.
        if let Some((x, y)) = self.alloc_page0(w, h) {
            return Some((0, x, y));
        }
        for (i, page) in self.pages.iter_mut().enumerate() {
            if let Some((x, y)) = page.alloc(w, h) {
                return Some((i + 1, x, y));
            }
        }
        let total = self.page_count();
        // Recycle an idle page (untouched in both the last frame and this frame so far).
        if let Some(p) = (0..total)
            .filter(|&p| !self.page_touched(p) && !self.page_shown(p) && self.page_non_empty(p))
            .min_by_key(|&p| self.page_age(p))
        {
            self.evict_page(p);
            let (x, y) = if p == 0 {
                self.alloc_page0(w, h).unwrap()
            } else {
                self.pages[p - 1].alloc(w, h).unwrap()
            };
            return Some((p, x, y));
        }
        // Once we have the steady-state page pair, recycle the oldest page not touched this frame.
        if total >= MAX_STEADY_PAGES
            && let Some(p) = (0..total)
                .filter(|&p| !self.page_touched(p) && self.page_non_empty(p))
                .min_by_key(|&p| self.page_age(p))
        {
            self.evict_page(p);
            let (x, y) = if p == 0 {
                self.alloc_page0(w, h).unwrap()
            } else {
                self.pages[p - 1].alloc(w, h).unwrap()
            };
            return Some((p, x, y));
        }
        if total < MAX_OVERFLOW_PAGES {
            self.clock += 1;
            let mut page = AtlasPage::new(self.clock);
            let (x, y) = page.alloc(w, h).unwrap();
            self.pages.push(page);
            return Some((self.pages.len(), x, y));
        }
        // Every page is referenced by this frame (or pinned by a text flight). Never
        // overwrite emitted quads to satisfy a later request. Failure is not cached,
        // so the request can succeed after a subsequent frame frees capacity.
        None
    }

    pub(super) fn put(&mut self, w: u32, h: u32, src: &[u8], left: i32, top: i32) -> Option<AtlasGlyph> {
        let (page, ax, ay) = self.alloc(w, h)?;
        let (data, dirty) = if page == 0 {
            (&mut self.data, &mut self.dirty)
        } else {
            let p = &mut self.pages[page - 1];
            (&mut p.data, &mut p.dirty)
        };
        // Zero the 1px border around `[ax..ax+w, ay..ay+h]` so bilinear filtering never samples
        // stale texels from a previous generation without zeroing the whole 4 MB page on reset.
        let x0 = (ax - 1) as usize;
        let x1 = (ax + w + 1) as usize;
        let top_row = (ay - 1) as usize * ATLAS_SIZE as usize;
        data[top_row + x0..top_row + x1].fill(0);
        for row in 0..h {
            let s = &src[(row * w) as usize..((row + 1) * w) as usize];
            let line = (ay + row) as usize * ATLAS_SIZE as usize;
            data[line + x0] = 0;
            data[line + ax as usize..line + (ax + w) as usize].copy_from_slice(s);
            data[line + (ax + w) as usize] = 0;
        }
        let bot_row = (ay + h) as usize * ATLAS_SIZE as usize;
        data[bot_row + x0..bot_row + x1].fill(0);
        let (y0, y1) = (ay - 1, ay + h + 1);
        let d = dirty.get_or_insert((y0, y1));
        *d = (d.0.min(y0), d.1.max(y1));
        self.rasterized += 1;
        self.touch(page as u16, w * h);
        Some(AtlasGlyph { page: page as u16, x: ax, y: ay, w, h, left, top })
    }
}
