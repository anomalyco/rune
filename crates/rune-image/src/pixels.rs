//! Pixel buffers an app replaces at will (a CPU-shaded HUD frame, a video frame), drawn by
//! `El::pixels`. Each runtime keeps a CPU-side RGBA atlas the renderer uploads by dirty rows,
//! like the glyph atlas, allocated only once an element shows pixels.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::place::{SurfaceFit, SurfacePlace};

pub const IMAGE_ATLAS_SIZE: u32 = 1024;

/// The largest texture side of any GPU that draws images in this process (`limit_side`).
static LIMIT: AtomicU32 = AtomicU32::new(crate::MAX_SIDE);

/// Says a GPU draws images here whose textures are at most `side` texels a side. The atlas then
/// places nothing larger: a decoded image draws from its largest mip that fits.
pub fn limit_side(side: u32) {
    LIMIT.fetch_min(side.max(1), Ordering::Relaxed);
}

/// The largest side the atlas places.
pub fn max_side() -> u32 {
    LIMIT.load(Ordering::Relaxed)
}
/// Rows a shelf page starts with (`ImagePage::new`).
const IMAGE_PAGE_START_ROWS: u32 = 128;

struct Frame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    generation: u64,
    mips: Option<Arc<crate::Decoded>>,
}

/// A replaceable RGBA image, shared between the producer and the view. Clone it freely.
#[derive(Clone)]
pub struct Pixels {
    id: u64,
    frame: Arc<Mutex<Frame>>,
    /// Wakes the windows and overlays whose scene shows this image.
    #[doc(hidden)]
    pub source: Arc<rune_core::live::Source>,
    /// The URL or path an `ImageCache` loaded it from.
    url: Option<Arc<str>>,
    /// The candidates `ImageCache::url_set` chose `url` from, as an HTML `srcset`.
    srcset: Option<Arc<str>>,
    /// The `ImageCache` entry these pixels came from, so the cache still finds it while
    /// anything draws them, and a view that asks for the image again on its next build gets it
    /// back instead of fetching it anew.
    keep: Option<Arc<dyn std::any::Any + Send + Sync>>,
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let frame = self.frame.lock().unwrap();
        write!(f, "Pixels({}, {}x{}, gen {})", self.id, frame.width, frame.height, frame.generation)
    }
}

impl Default for Pixels {
    fn default() -> Self {
        Self::new()
    }
}

impl Pixels {
    pub fn new() -> Pixels {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Pixels {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            frame: Arc::new(Mutex::new(Frame { width: 0, height: 0, rgba: Vec::new(), generation: 0, mips: None })),
            source: rune_core::live::Source::new(0),
            url: None,
            srcset: None,
            keep: None,
        }
    }

    pub(crate) fn from_url(url: Option<Arc<str>>, srcset: Option<Arc<str>>) -> Pixels {
        Pixels { url, srcset, ..Pixels::new() }
    }

    pub(crate) fn keeping(mut self, keep: Arc<dyn std::any::Any + Send + Sync>) -> Pixels {
        self.keep = Some(keep);
        self
    }

    /// The URL ([`ImageCache::url`](crate::ImageCache::url)) or path
    /// ([`ImageCache::file`](crate::ImageCache::file)) the image was loaded from, as given, so
    /// a page export can point an `<img>` at it.
    pub fn source_url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// For an image from [`ImageCache::url_set`](crate::ImageCache::url_set), every size it
    /// chose from, as an HTML `srcset` (`"a-640.avif 640w, a-1280.avif 1280w"`).
    pub fn source_set(&self) -> Option<&str> {
        self.srcset.as_deref()
    }

    /// Replaces the image: `rgba` is `width * height` straight-alpha sRGB pixels, rows top
    /// first. Call it from any thread: it wakes only the windows and overlays showing the
    /// image, and their next frame uploads it. Sets between two frames coalesce into one wake
    /// and one upload of the latest image; a hidden or covered window catches up when it shows
    /// again. This is the streaming path: no decoding or mip generation, and a same-size
    /// frame reuses its atlas slot.
    pub fn set(&self, width: u32, height: u32, rgba: Vec<u8>) {
        assert_eq!(Some(rgba.len()), (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4)), "rgba must hold width * height pixels");
        {
            let mut frame = self.frame.lock().unwrap();
            frame.width = width;
            frame.height = height;
            frame.rgba = rgba;
            frame.generation += 1;
            frame.mips = None;
        }
        self.source.notify();
    }

    pub(crate) fn set_mips(&self, mips: Arc<crate::Decoded>) {
        {
            let mut frame = self.frame.lock().unwrap();
            frame.width = mips.mips[0].width;
            frame.height = mips.mips[0].height;
            frame.rgba.clear();
            frame.mips = Some(mips);
            frame.generation += 1;
        }
        self.source.notify();
    }

    pub fn size(&self) -> (u32, u32) {
        let frame = self.frame.lock().unwrap();
        (frame.width, frame.height)
    }

    #[doc(hidden)]
    pub fn signature(&self) -> (u64, u64) {
        let frame = self.frame.lock().unwrap();
        (self.id, frame.generation)
    }

    /// The image's RGBA bytes as [`Pixels::set`] gave them (empty for a decoded image's mips).
    #[doc(hidden)]
    pub fn rgba(&self) -> Vec<u8> {
        self.frame.lock().unwrap().rgba.clone()
    }
}

/// Where an image of `size` texels draws in `rect` ([x, y, w, h], device pixels) and which window
/// of it shows, as fractions [x0, y0, x1, y1]. `None` stretches; `Fit` letterboxes (on whole
/// pixels at unscaled rest, continuous while scaled); `Fill` keeps the rect and crops the
/// overflow. Spare space and overflow split by `place`'s anchor (centred by default); an unscaled
/// place draws one texel per pixel and crops or leaves the rest empty.
#[doc(hidden)]
pub fn fit(
    fit: Option<SurfaceFit>,
    place: SurfacePlace,
    rect: [f32; 4],
    size: (u32, u32),
) -> ([f32; 4], [f32; 4]) {
    fit_in(fit, place, rect, size, 1.0)
}

#[doc(hidden)]
pub fn fit_in(
    fit: Option<SurfaceFit>,
    place: SurfacePlace,
    rect: [f32; 4],
    size: (u32, u32),
    snap_weight: f32,
) -> ([f32; 4], [f32; 4]) {
    let whole = [0.0, 0.0, 1.0, 1.0];
    let [x, y, w, h] = rect;
    let (iw, ih) = (size.0 as f32, size.1 as f32);
    if fit.is_none() || iw <= 0.0 || ih <= 0.0 || !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return (rect, whole);
    }
    let [kx, ky] = place.share();
    let u = if (w - w.round()).abs() < 1.0 / 512.0 && (h - h.round()).abs() < 1.0 / 512.0 && !snap_weight.is_nan() {
        snap_weight.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let blend = |exact: [f32; 4], snapped: [f32; 4]| {
        if u <= 0.0 {
            exact
        } else if u >= 1.0 {
            snapped
        } else {
            std::array::from_fn(|i| exact[i] + (snapped[i] - exact[i]) * u)
        }
    };
    if place.unscaled {
        let (dw, dh) = (iw.min(w), ih.min(h));
        let (sx, sy) = (dw / iw, dh / ih);
        let ((x0, x1), (y0, y1)) = (window(sx, kx), window(sy, ky));
        let exact = [x + (w - dw) * kx, y + (h - dh) * ky, dw, dh];
        let snapped = [x + ((w - dw) * kx).round(), y + ((h - dh) * ky).round(), dw, dh];
        return (blend(exact, snapped), [x0, y0, x1, y1]);
    }
    if fit == Some(SurfaceFit::Fit) {
        let k = (w / iw).min(h / ih);
        let (min_w, min_h) = (w.min(1.0), h.min(1.0));
        let (dw, dh) = ((iw * k).clamp(min_w, w), (ih * k).clamp(min_h, h));
        let exact = [x + (w - dw) * kx, y + (h - dh) * ky, dw, dh];
        if u <= 0.0 {
            return (exact, whole);
        }
        let (sw, sh) = ((iw * k).round().clamp(min_w, w), (ih * k).round().clamp(min_h, h));
        let snapped = [x + ((w - sw) * kx).round(), y + ((h - sh) * ky).round(), sw, sh];
        return (blend(exact, snapped), whole);
    }
    let k = (w / iw).max(h / ih);
    if !(k > 0.0 && k.is_finite()) { return (rect, whole); }
    let (sx, sy) = ((w / (iw * k)).clamp(0.0, 1.0), (h / (ih * k)).clamp(0.0, 1.0));
    let ((x0, x1), (y0, y1)) = (window(sx, kx), window(sy, ky));
    (rect, [x0, y0, x1, y1])
}

/// The visible fraction `[a, b]` of an axis when `span` of it shows and `share` of the hidden part lies before.
fn window(span: f32, share: f32) -> (f32, f32) {
    if share == 0.5 { return ((1.0 - span) * 0.5, (1.0 + span) * 0.5); }
    let a = (1.0 - span) * share;
    (a, a + span)
}

struct Slot {
    page: usize,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    generation: u64,
    /// The animation frame copied in.
    frame: usize,
}

/// One stable shelf page, or an exact-size standalone page for a large image.
pub struct ImagePage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    /// Drawn from in the last completed frame.
    shown: bool,
    /// Drawn from in this frame so far; only ever set until the frame ends.
    touched: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub rect: [u32; 4],
    pub page: usize,
    pub premultiplied: bool,
}

/// Streaming buffers keep one slot. Static images keep a slot per sampled mip level.
#[derive(Default)]
pub struct ImageAtlas {
    pub pages: Vec<ImagePage>,
    slots: HashMap<(u64, usize), Slot>,
    /// When each animated image drawn here started playing, on the animation clock: its first
    /// draw, kept while the atlas holds it.
    starts: HashMap<u64, f64>,
    /// Frames copied in so far, so the idle meter sees new pixels in a slot it already drew.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub copies: u64,
}

impl ImageAtlas {
    /// The atlas origin and size of `pixels`, copying a new frame in when it changed. None while
    /// it's empty or doesn't fit.
    pub fn place(&mut self, pixels: &Pixels, drawn: [f32; 2]) -> Option<Placed> {
        self.place_at(pixels, drawn, None).map(|(placed, _)| placed)
    }

    /// [`Self::place`] at `now` on the animation clock: an animated image shows the frame for
    /// the time since it was first drawn here, and says when (on the same clock) the next one
    /// shows, or `None` for a still or an animation that has stopped. `None` for `now` shows
    /// an animation's first frame.
    #[doc(hidden)]
    pub fn place_at(&mut self, pixels: &Pixels, drawn: [f32; 2], now: Option<f64>) -> Option<(Placed, Option<f64>)> {
        let frame = pixels.frame.lock().unwrap();
        let (mut w, mut h) = (frame.width, frame.height);
        if w == 0 || h == 0 || w > crate::MAX_SIDE || h > crate::MAX_SIDE {
            return None;
        }
        let mut level = 0;
        let mut shown = 0;
        let mut next = None;
        let bytes = if let Some(decoded) = &frame.mips {
            let mips = match (&decoded.motion, now) {
                (Some(motion), Some(now)) => {
                    let start = *self.starts.entry(pixels.id).or_insert(now);
                    let (i, change) = motion.at(now - start);
                    (shown, next) = (i, change.map(|t| start + t));
                    motion.frame(&decoded.mips, i)
                }
                _ => &decoded.mips[..],
            };
            // Choose the smallest prefiltered level that still covers both device-pixel axes.
            // No mip construction or resampling happens on the render thread.
            while let Some(next) = mips.get(level + 1) {
                if (next.width as f32) < drawn[0].clamp(1.0, frame.width as f32)
                    || (next.height as f32) < drawn[1].clamp(1.0, frame.height as f32) { break; }
                level += 1;
            }
            // A level past the GPU's largest texture can't go up: draw from the largest that fits.
            let limit = max_side();
            while mips[level].width.max(mips[level].height) > limit && level + 1 < mips.len() { level += 1; }
            let mip = &mips[level];
            (w, h) = (mip.width, mip.height);
            &mip.pixels
        } else { &frame.rgba };
        if w > max_side() || h > max_side() { return None; }
        let key = (pixels.id, level);
        if self.slots.get(&key).is_some_and(|s| s.w != w || s.h != h) {
            self.slots.remove(&key);
        }
        if !self.slots.contains_key(&key) {
            let (page, x, y) = self.allocate(w, h);
            self.slots.insert(key, Slot { page, x, y, w, h, generation: 0, frame: 0 });
        }
        let slot = self.slots.get_mut(&key).unwrap();
        self.pages[slot.page].touched = true;
        if slot.generation != frame.generation || slot.frame != shown {
            slot.generation = frame.generation;
            slot.frame = shown;
            #[cfg(debug_assertions)]
            { self.copies += 1; }
            let page = &mut self.pages[slot.page];
            let row = (page.width * 4) as usize;
            for line in 0..h as usize {
                let dst = (slot.y as usize + line) * row + slot.x as usize * 4;
                let src = line * w as usize * 4;
                page.data[dst..dst + w as usize * 4].copy_from_slice(&bytes[src..src + w as usize * 4]);
            }
            let (y0, y1) = (slot.y, slot.y + h);
            page.dirty = Some(page.dirty.map_or((y0, y1), |(a, b)| (a.min(y0), b.max(y1))));
        }
        Some((Placed { rect: [slot.x, slot.y, w, h], page: slot.page, premultiplied: frame.mips.is_some() }, next))
    }

    /// Whether the atlas currently holds a slot for `pixels` at mip `level`.
    #[doc(hidden)]
    pub fn has_slot(&self, pixels: &Pixels, level: usize) -> bool {
        self.slots.contains_key(&(pixels.id, level))
    }

    /// Forgets images no element showed since the last call, except on pages that still show
    /// one and on the first page: shelves never reuse a slot's space until their page starts
    /// over, so an image shown again there (a view come back to) needs no copy or upload. A
    /// page left with nothing starts over so replaced sizes don't fill the atlas; a full atlas
    /// starts over a page that showed nothing last frame or yet this one before it adds one.
    pub fn end_frame(&mut self) {
        let pages = &self.pages;
        let first_kept = pages.first().is_some_and(|p| p.width <= IMAGE_ATLAS_SIZE && p.height <= IMAGE_ATLAS_SIZE);
        self.slots.retain(|_, s| pages[s.page].touched || (s.page == 0 && first_kept));
        if !self.starts.is_empty() {
            let slots = &self.slots;
            self.starts.retain(|id, _| slots.keys().any(|(slot, _)| slot == id));
        }
        for (i, page) in self.pages.iter_mut().enumerate() {
            page.shown = std::mem::take(&mut page.touched);
            if !self.slots.values().any(|slot| slot.page == i) {
                (page.shelf_x, page.shelf_y, page.shelf_h) = (0, 0, 0);
                page.dirty = None;
                if page.width > IMAGE_ATLAS_SIZE || page.height > IMAGE_ATLAS_SIZE {
                    page.data = Vec::new();
                }
            }
        }
        let keep = self.slots.values().map(|s| s.page + 1).max().unwrap_or(0);
        self.pages.truncate(keep);
    }

    /// A window at rest (`window::rest`): forgets the images no element showed in the last
    /// frame, the first page's included, and frees the CPU copy of every page left empty. An
    /// image shown again after it is copied in again.
    pub fn rest(&mut self) {
        let pages = &self.pages;
        self.slots.retain(|_, s| pages[s.page].shown);
        let slots = &self.slots;
        self.starts.retain(|id, _| slots.keys().any(|(slot, _)| slot == id));
        for (i, page) in self.pages.iter_mut().enumerate() {
            if !self.slots.values().any(|slot| slot.page == i) {
                (page.shelf_x, page.shelf_y, page.shelf_h) = (0, 0, 0);
                page.dirty = None;
                page.data = Vec::new();
            }
        }
        let keep = self.slots.values().map(|s| s.page + 1).max().unwrap_or(0);
        self.pages.truncate(keep);
    }

    /// Drops every page, slot and animation start, freeing all CPU image buffers. Returns whether
    /// the atlas held any pages or slots.
    pub fn clear(&mut self) -> bool {
        let had = !self.pages.is_empty() || !self.slots.is_empty() || !self.starts.is_empty();
        self.pages.clear();
        self.slots.clear();
        self.starts.clear();
        had
    }

    /// Marks every row for upload (a new texture).
    pub fn invalidate(&mut self) {
        for page in &mut self.pages { page.dirty = Some((0, page.height)); }
    }

    fn allocate(&mut self, w: u32, h: u32) -> (usize, u32, u32) {
        for (i, page) in self.pages.iter_mut().enumerate() {
            if page.data.is_empty() { *page = ImagePage::new(w, h); }
            if let Some((x, y)) = page.allocate(w, h) { return (i, x, y); }
        }
        // Full: start over a shelf page that showed nothing last frame or yet this one. A page
        // not drawn from yet this frame may still be, later in the tree.
        let idle = |p: &ImagePage| !p.shown && !p.touched && p.width <= IMAGE_ATLAS_SIZE && p.height <= IMAGE_ATLAS_SIZE
            && p.shelf_y + p.shelf_h > 0;
        if w <= IMAGE_ATLAS_SIZE && h <= IMAGE_ATLAS_SIZE && let Some(i) = self.pages.iter().position(idle) {
            self.slots.retain(|_, s| s.page != i);
            let page = &mut self.pages[i];
            (page.shelf_x, page.shelf_y, page.shelf_h) = (0, 0, 0);
            let (x, y) = page.allocate(w, h).unwrap();
            return (i, x, y);
        }
        let mut page = ImagePage::new(w, h);
        let (x, y) = page.allocate(w, h).unwrap();
        self.pages.push(page);
        (self.pages.len() - 1, x, y)
    }
}

impl ImagePage {
    /// A shelf page starts with the rows its first image needs (at least
    /// `IMAGE_PAGE_START_ROWS`) and grows as its shelves fill (`allocate`), up to
    /// `IMAGE_ATLAS_SIZE`: a window that shows a few icons holds 512 KiB of it, not 4 MiB.
    fn new(w: u32, h: u32) -> Self {
        let (width, height) = if w > IMAGE_ATLAS_SIZE || h > IMAGE_ATLAS_SIZE { (w, h) }
            else { (IMAGE_ATLAS_SIZE, h.next_power_of_two().max(IMAGE_PAGE_START_ROWS)) };
        Self { width, height, data: vec![0; width as usize * height as usize * 4],
            dirty: None, shelf_x: 0, shelf_y: 0, shelf_h: 0, shown: false, touched: false }
    }

    fn allocate(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let shelf = self.width == IMAGE_ATLAS_SIZE && self.height <= IMAGE_ATLAS_SIZE;
        if w > self.width || h > if shelf { IMAGE_ATLAS_SIZE } else { self.height } { return None; }
        if self.shelf_x + w > self.width {
            self.shelf_y += self.shelf_h;
            self.shelf_x = 0;
            self.shelf_h = 0;
        }
        if self.shelf_y + h > self.height {
            if !shelf || self.shelf_y + h > IMAGE_ATLAS_SIZE { return None; }
            // Rows already placed keep their texels; the renderer makes the page's texture again
            // at its new size and uploads all of it.
            self.height = (self.shelf_y + h).next_power_of_two().min(IMAGE_ATLAS_SIZE);
            self.data.resize(self.width as usize * self.height as usize * 4, 0);
            self.dirty = Some((0, self.height));
        }
        let at = (self.shelf_x, self.shelf_y);
        self.shelf_x += w;
        self.shelf_h = self.shelf_h.max(h);
        Some(at)
    }
}

#[cfg(test)]
#[path = "pixels_tests.rs"]
mod tests;
