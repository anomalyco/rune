//! The glyph atlas: shelf-packed greyscale pages of glyph and icon masks, aged and evicted by
//! use, and the quads that sample them.

pub(crate) mod fill;
#[doc(hidden)]
pub mod fill_bench;
pub(crate) mod glyph_cache;
pub(crate) mod glyph_emit;
pub mod mask_cache;

use super::*;
use fill::Residency;

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
    /// How far down the mask is drawn, in quarters of a pixel (`glyph_emit::GLIDE_PHASES`).
    pub(super) subpy: u8,
    /// The dilation radius in device px, as f32 bits.
    pub(super) dilation: u32,
    /// The CoreGraphics pipeline's stages (`macos::smoothing::Smoothing::key`).
    pub(super) apple: u8,
    /// Synthetic oblique, in degrees (0 for none).
    pub(super) skew: u8,
    /// Synthetic bold (`synthetic_bold`).
    pub(super) bold: bool,
}

#[derive(Clone, Copy)]
pub(super) struct AtlasGlyph {
    pub(super) page: u16,
    pub(super) x: u32,
    pub(super) y: u32,
    /// Texels wide: three a pixel for a subpixel mask (`lcd`).
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) left: i32,
    pub(super) top: i32,
    /// A subpixel mask: three coverages a pixel, red to blue.
    pub(super) lcd: bool,
}

/// Steady-state shelf pages (page 0 + one secondary page for ping-pong eviction).
pub(super) const MAX_STEADY_PAGES: usize = 2;
/// Hard ceiling on total glyph atlas pages if a single frame or unframed loop overflows.
pub(super) const MAX_OVERFLOW_PAGES: usize = 8;
/// CPU byte budget for cached RGBA colour glyphs (`color_pixels`) before frame-boundary eviction.
pub(super) const COLOR_GLYPH_BUDGET: usize = 16 * 1024 * 1024;

/// A shelf's use, relative to the frame now in progress: `LastFrame` means it drew a frame ago
/// and has not been asked for anything since.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Use {
    /// Untouched in both the last frame and this one.
    #[default]
    Idle,
    /// Touched in the last frame, not yet in this one.
    LastFrame,
    /// Touched in this frame.
    ThisFrame,
}

struct Shelf {
    x: u32,
    y: u32,
    h: u32,
    used: Use,
    full: bool,
    age: u64,
    live_texels: u32,
}

impl Shelf {
    fn new(age: u64) -> Self {
        Shelf { x: 1, y: 1, h: 0, used: Use::Idle, full: false, age, live_texels: 0 }
    }

    fn reset(&mut self, age: u64) {
        *self = Self::new(age);
    }

    fn non_empty(&self) -> bool {
        self.x > 1 || self.y > 1 || self.h > 0
    }

    /// Rows `[0, y1)` holding every glyph placed since the last reset, with the gutter below.
    fn used_rows(&self) -> Option<(u32, u32)> {
        self.non_empty().then(|| (0, (self.y + self.h + 1).min(ATLAS_SIZE)))
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.x + w + 1 > ATLAS_SIZE {
            self.y += self.h + 1;
            self.x = 1;
            self.h = 0;
        }
        if self.y + h + 1 > ATLAS_SIZE {
            self.full = true;
            return None;
        }
        let pos = (self.x, self.y);
        self.x += w + 1;
        self.h = self.h.max(h);
        Some(pos)
    }

    fn touch(&mut self, texels: u32) {
        self.used = Use::ThisFrame;
        self.live_texels = self.live_texels.saturating_add(texels);
    }

    fn end_frame(&mut self) {
        self.used = match self.used {
            Use::ThisFrame => Use::LastFrame,
            Use::Idle | Use::LastFrame => Use::Idle,
        };
        self.live_texels = 0;
    }
}

pub struct AtlasPage {
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    shelf: Shelf,
    /// Monotonic eviction generation for this page, so reusable animated slots detect resets.
    pub(super) epoch: u64,
}

impl AtlasPage {
    pub(super) fn new(age: u64, epoch: u64) -> Self {
        AtlasPage {
            data: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            dirty: None,
            shelf: Shelf::new(age),
            epoch,
        }
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
    pub ink: u32,
    pub byte: u32,
    /// A colour glyph (emoji) drawn from [`TextSystem::color_glyph`] instead of the alpha
    /// atlas: 0 for none, else its index plus one.
    pub color: u32,
    /// A glyph drawn straight from its outline instead of the atlas (`uv` unused).
    pub outline: Option<super::Outline>,
    /// A subpixel mask, three texels a pixel (`FontSmoothing::WindfoilRgb`, `ClearType`).
    pub lcd: bool,
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
    /// The quad for a resting pen at `[x, y]`, rounded at rest, then moved by `glide`
    /// (the image draws filtered).
    pub(super) fn quad(&self, x: f32, y: f32, glide: [f32; 2], ink: u32, byte: u32) -> GlyphQuad {
        let rect = [
            x.round() + self.left as f32 + glide[0],
            y.round() - self.top as f32 + glide[1],
            self.w as f32,
            self.h as f32,
        ];
        GlyphQuad { rect, uv: [0.0, 0.0], ink, byte, color: self.index, outline: None, lcd: false }
    }
}

pub struct Atlas {
    /// The first page's CPU copy: empty until its first mask (`alloc`), then 2048x2048.
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    /// `(node_id, signature)` of `El::cache` targets currently resident in the GPU renderer.
    #[doc(hidden)]
    pub resident_caches: HashMap<u64, u64>,
    /// Secondary and overflow shelf pages (`page >= 1` lives at `pages[page - 1]`).
    #[doc(hidden)]
    pub pages: Vec<AtlasPage>,
    pub(crate) resets: u64,
    pub(crate) rasterized: u64,
    shelf: Shelf,
    page0_gen: u64,
    /// Monotonic atlas-wide page generation counter so popped and recreated overflow pages never
    /// collide with stale reusable slot handles (`SlotEntry`).
    next_page_gen: u64,
    /// Monotonic counter incremented each time the renderer consumes dirty atlas rows
    /// (`rows_uploaded`), so double-buffered animated slots detect missed drawables per slot
    /// without horizontal shelf aliasing.
    pub(super) upload_gen: u64,
    pub(super) clock: u64,
    pub(super) map: GlyphMap<Residency>,
    /// Text draws subpixel masks this frame: the renderer blends by channel
    /// (`FontSmoothing::lcd`).
    #[doc(hidden)]
    pub lcd: bool,
    /// Text asks for subpixel masks (`FontSmoothing::lcd` of the mode asked for), though it may
    /// draw grayscale until the renderer's subpixel pipeline is built: the renderer builds it then.
    #[doc(hidden)]
    pub lcd_wanted: bool,
    /// Per page, the entries whose GPU fill is queued (`atlas_fill.rs`).
    pub(super) fills: Vec<Vec<GlyphKey>>,
    pub(super) icons: HashMap<(usize, u32), Option<AtlasGlyph>>,
    /// Glyph outlines drawn straight on the GPU, uploaded with the pages.
    #[doc(hidden)]
    pub outlines: super::outline::OutlineStore,
}

impl Atlas {
    /// Marks every used row for upload (a new texture, e.g. after the window's format changed).
    pub fn invalidate(&mut self) {
        self.resend_rows();
        self.resident_caches.clear();
        self.outlines.invalidate();
    }

    /// Marks the rows the shelves have placed glyphs in for upload, to a texture that's new or
    /// lost its writes. Rows below them hold nothing a glyph samples and a new texture starts
    /// zeroed, so the never-touched rest of each 4 MiB page is neither read (a page fault per
    /// 4 KiB) nor copied.
    #[doc(hidden)]
    pub fn resend_rows(&mut self) {
        self.dirty = self.shelf.used_rows();
        for page in &mut self.pages {
            page.dirty = page.shelf.used_rows();
        }
    }

    /// Rows of `page` its shelves have placed masks in, with the gutter below: what a texture of
    /// that page must hold.
    #[doc(hidden)]
    pub fn page_rows(&self, page: usize) -> u32 {
        self.shelf(page).used_rows().map_or(0, |(_, y1)| y1)
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
    #[doc(hidden)]
    pub fn glyph_entries(&self) -> usize { self.map.len() }

    #[doc(hidden)]
    pub fn new() -> Self {
        Atlas { outlines: Default::default(), ..Self::empty() }
    }

    /// An unallocated placeholder atlas, for [`TextSystem::empty`].
    pub(crate) fn empty() -> Self {
        Atlas {
            // Made with the first mask (`alloc`).
            data: Vec::new(),
            dirty: None,
            resident_caches: HashMap::new(),
            pages: Vec::new(),
            resets: 0,
            rasterized: 0,
            shelf: Shelf::new(1),
            page0_gen: 1,
            next_page_gen: 2,
            upload_gen: 0,
            clock: 1,
            map: GlyphMap::default(),
            lcd: false,
            lcd_wanted: false,
            fills: Vec::new(),
            icons: HashMap::new(),
            outlines: super::outline::OutlineStore::empty(),
        }
    }

    fn shelf(&self, page: usize) -> &Shelf {
        if page == 0 { &self.shelf } else { &self.pages[page - 1].shelf }
    }

    fn shelf_mut(&mut self, page: usize) -> &mut Shelf {
        if page == 0 { &mut self.shelf } else { &mut self.pages[page - 1].shelf }
    }

    pub(super) fn touch(&mut self, page: u16, texels: u32) {
        if page == 0 {
            self.shelf.touch(texels);
        } else if let Some(p) = self.pages.get_mut(page as usize - 1) {
            p.shelf.touch(texels);
        }
    }

    fn alloc_page_gen(&mut self) -> u64 {
        let id = self.next_page_gen;
        self.next_page_gen = self.next_page_gen.wrapping_add(1).max(1);
        id
    }

    pub(super) fn evict_page(&mut self, page: usize) {
        let p16 = page as u16;
        self.map.retain(|_, v| v.mask().is_none_or(|g| g.page != p16));
        if let Some(queue) = self.fills.get_mut(page) { queue.clear(); }
        self.icons.retain(|_, v| v.is_none_or(|g| g.page != p16));
        self.clock += 1;
        self.resets += 1;
        let age = self.clock;
        let next_gen = self.alloc_page_gen();
        if page == 0 {
            self.dirty = None;
            self.shelf.reset(age);
            self.page0_gen = next_gen;
        } else if let Some(p) = self.pages.get_mut(page - 1) {
            p.dirty = None;
            p.shelf.reset(age);
            p.epoch = next_gen;
        }
    }

    pub(super) fn page_gen(&self, page: u16) -> u64 {
        if page == 0 {
            self.page0_gen
        } else {
            self.pages.get(page as usize - 1).map_or(0, |p| p.epoch)
        }
    }

    pub(crate) fn begin_frame(&mut self, pinned_mask: u32) {
        // A text flight's glyphs keep their outline records until it lands.
        if self.outlines.begin_frame(pinned_mask != 0) {
            self.forget_outline_masks();
        }
        let total = self.page_count();
        let total_live: u64 = (0..total).map(|p| self.shelf(p).live_texels as u64).sum();
        let half_page = (ATLAS_SIZE * ATLAS_SIZE / 2) as u64;
        let needed = ((total_live / half_page) as usize + MAX_STEADY_PAGES)
            .clamp(MAX_STEADY_PAGES, MAX_OVERFLOW_PAGES);
        let full_count = (0..total).filter(|&p| self.shelf(p).full).count();
        if full_count >= needed || total > needed {
            let keep = needed.min(total);
            if let Some(oldest) = (0..keep)
                .filter(|&p| self.shelf(p).full && (pinned_mask & (1u32 << p.min(31))) == 0)
                .min_by_key(|&p| self.shelf(p).age)
            {
                self.evict_page(oldest);
            }
            for p in keep..total {
                if (pinned_mask & (1u32 << p.min(31))) == 0 && self.shelf(p).non_empty() {
                    self.evict_page(p);
                }
            }
            while self.page_count() > needed {
                let last = self.page_count() - 1;
                if (pinned_mask & (1u32 << last.min(31))) != 0 || self.shelf(last).non_empty() {
                    break;
                }
                self.pages.pop();
            }
        }
        for p in 0..self.page_count() {
            self.shelf_mut(p).end_frame();
            if (pinned_mask & (1u32 << p.min(31))) != 0 {
                self.touch(p as u16, 0);
            }
        }
    }

    /// Room for a `w`x`h` mask: its page and place. The first page's CPU copy is made here, at its
    /// first mask, so a text system that only measures (`measure`'s, on each thread that calls it)
    /// never holds its 4 MiB.
    pub(super) fn alloc(&mut self, w: u32, h: u32) -> Option<(usize, u32, u32)> {
        let placed = self.place(w, h);
        if placed.is_some_and(|(page, ..)| page == 0) && self.data.is_empty() {
            self.data = vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize];
        }
        placed
    }

    fn place(&mut self, w: u32, h: u32) -> Option<(usize, u32, u32)> {
        if w == 0 || h == 0 || w + 2 > ATLAS_SIZE || h + 2 > ATLAS_SIZE {
            return None;
        }
        let total = self.page_count();
        for p in 0..total {
            if !self.shelf(p).full && let Some((x, y)) = self.shelf_mut(p).alloc(w, h) {
                return Some((p, x, y));
            }
        }
        // Try shelf tails on full pages for small glyphs before opening or resetting a page.
        for p in 0..total {
            if let Some((x, y)) = self.shelf_mut(p).alloc(w, h) {
                return Some((p, x, y));
            }
        }
        // Recycle an idle page (untouched in both the last frame and this frame so far).
        if let Some(p) = (0..total)
            .filter(|&p| {
                let s = self.shelf(p);
                s.used == Use::Idle && s.non_empty()
            })
            .min_by_key(|&p| self.shelf(p).age)
        {
            self.evict_page(p);
            let (x, y) = self.shelf_mut(p).alloc(w, h).unwrap();
            return Some((p, x, y));
        }
        // Once we have the steady-state page pair, recycle the oldest page not touched this frame.
        if total >= MAX_STEADY_PAGES
            && let Some(p) = (0..total)
                .filter(|&p| {
                    let s = self.shelf(p);
                    s.used != Use::ThisFrame && s.non_empty()
                })
                .min_by_key(|&p| self.shelf(p).age)
        {
            self.evict_page(p);
            let (x, y) = self.shelf_mut(p).alloc(w, h).unwrap();
            return Some((p, x, y));
        }
        if total < MAX_OVERFLOW_PAGES {
            self.clock += 1;
            let epoch = self.alloc_page_gen();
            let mut page = AtlasPage::new(self.clock, epoch);
            let (x, y) = page.shelf.alloc(w, h).unwrap();
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
        Some(AtlasGlyph { page: page as u16, x: ax, y: ay, w, h, left, top, lcd: false })
    }

    /// Borrow of this atlas restricted to in-place icon slot updates (`loop_step`).
    /// By construction, exposes no allocation or eviction methods and never touches `live_texels`.
    pub(super) fn in_place(&mut self) -> InPlaceAtlas<'_> {
        InPlaceAtlas(self)
    }

    /// Allocates a `(px + 2) × (px + 2)` icon slot with a 1-texel transparent apron around `cov`
    /// (`px × px`), returning its placement and page eviction generation for later in-place reuse.
    pub(super) fn put_bordered(&mut self, px: u32, cov: &[u8]) -> Option<(AtlasGlyph, u64)> {
        let side = px + 2;
        let (page, ax, ay) = self.alloc(side, side)?;
        let epoch = self.page_gen(page as u16);
        let slot = AtlasGlyph { page: page as u16, x: ax, y: ay, w: side, h: side, left: 0, top: 0, lcd: false };
        self.write_bordered_pixels(slot, px, cov);
        self.touch(slot.page, side * side);
        self.rasterized += 1;
        Some((slot, epoch))
    }

    /// Rewrites an existing `(px + 2) × (px + 2)` bordered icon slot in place if its page has not
    /// been evicted since allocation. Marks only the slot's row band dirty on the glyph atlas.
    pub(super) fn write_bordered(&mut self, slot: AtlasGlyph, page_gen: u64, px: u32, cov: &[u8]) -> bool {
        if slot.w != px + 2 || slot.h != px + 2 || self.page_gen(slot.page) != page_gen {
            return false;
        }
        self.write_bordered_pixels(slot, px, cov);
        self.touch(slot.page, slot.w * slot.h);
        true
    }

    fn write_bordered_pixels(&mut self, slot: AtlasGlyph, px: u32, cov: &[u8]) {
        let (page, ax, ay, side) = (slot.page as usize, slot.x, slot.y, px + 2);
        let (data, dirty) = if page == 0 {
            (&mut self.data, &mut self.dirty)
        } else {
            let p = &mut self.pages[page - 1];
            (&mut p.data, &mut p.dirty)
        };
        let x0 = (ax - 1) as usize;
        let x1 = (ax + side + 1) as usize;
        // Zero outer gutter rows and top/bottom 1px transparent apron rows.
        for gy in [ay - 1, ay, ay + px + 1, ay + side] {
            let row_start = gy as usize * ATLAS_SIZE as usize;
            data[row_start + x0..row_start + x1].fill(0);
        }
        for row in 0..px {
            let src = &cov[(row * px) as usize..((row + 1) * px) as usize];
            let line = (ay + 1 + row) as usize * ATLAS_SIZE as usize;
            data[line + x0] = 0;
            data[line + ax as usize] = 0;
            data[line + (ax + 1) as usize..line + (ax + 1 + px) as usize].copy_from_slice(src);
            data[line + (ax + 1 + px) as usize] = 0;
            data[line + (ax + side) as usize] = 0;
        }
        let (y0, y1) = (ay - 1, ay + side + 1);
        let d = dirty.get_or_insert((y0, y1));
        *d = (d.0.min(y0), d.1.max(y1));
    }
}

/// Restricted borrow of [`Atlas`] for `loop_step` in-place animated icon updates.
/// By construction, it has no `alloc`, `place`, `put_bordered`, `evict_page`, or `touch` methods,
/// so `loop_step` can never allocate in or evict from the glyph atlas or inflate `live_texels`.
pub(super) struct InPlaceAtlas<'a>(&'a mut Atlas);

impl InPlaceAtlas<'_> {
    pub(super) fn page_gen(&self, page: u16) -> u64 {
        self.0.page_gen(page)
    }

    pub(super) fn upload_gen(&self) -> u64 {
        self.0.upload_gen
    }

    pub(super) fn write_bordered(&mut self, slot: AtlasGlyph, page_gen: u64, px: u32, cov: &[u8]) -> bool {
        if slot.w != px + 2 || slot.h != px + 2 || self.0.page_gen(slot.page) != page_gen {
            return false;
        }
        self.0.write_bordered_pixels(slot, px, cov);
        true
    }
}
