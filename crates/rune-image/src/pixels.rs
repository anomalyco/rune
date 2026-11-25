//! Pixel buffers an app replaces at will (a CPU-shaded HUD frame, a video frame), drawn by
//! `El::pixels`. Each runtime keeps a CPU-side RGBA atlas the renderer uploads by dirty rows,
//! like the glyph atlas, allocated only once an element shows pixels.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::place::{ImageFilter, SurfaceFit, SurfacePlace};

pub const IMAGE_ATLAS_SIZE: u32 = 1024;
/// Maximum retained atlas pages when evicting unshown slots across frames.
pub const IMAGE_ATLAS_MAX_PAGES: usize = 16;
/// Maximum retained decoded CPU pixel bytes across atlas pages (64 MiB: 16 full 1024×1024 pages).
pub const IMAGE_ATLAS_MAX_BYTES: usize = 64 * 1024 * 1024;
/// Maximum retained slot entries in the atlas.
pub const IMAGE_ATLAS_MAX_SLOTS: usize = 4096;

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
pub(crate) const IMAGE_PAGE_START_ROWS: u32 = 128;

/// Backing bytes for one frame. Shared through `Arc` so `ImageAtlas::place_at` copies outside
/// the frame lock without blocking `Pixels::set`. Peak memory for a streaming `Pixels` is at
/// most two RGBA buffers: the current frame in `Frame` plus one in-flight snapshot held on
/// `place_at`'s stack when `set` runs concurrently (any further `set` drops the uncopied
/// intermediate frame immediately, and the snapshot drops before `place_at` returns).
#[derive(Clone)]
enum FrameData {
    Empty,
    Rgba(Arc<Vec<u8>>),
    Mips(Arc<crate::Decoded>),
}

struct Frame {
    width: u32,
    height: u32,
    generation: u64,
    data: FrameData,
}

struct FrameInner {
    frame: Mutex<Frame>,
    /// Monotonic latest-wins ticket advanced by `Pixels::replace`/`set`, `store_mips`, and
    /// `ImageCache::rgba_into`, so an older off-thread mip build never overwrites a newer frame.
    ticket: AtomicU64,
}

/// Weak reference to a `Pixels`' frame storage and live source, used by off-thread
/// [`crate::ImageCache::rgba_into`] jobs so abandoned handles skip mip work.
pub(crate) struct WeakPixels {
    inner: std::sync::Weak<FrameInner>,
    source: std::sync::Weak<rune_core::live::Source>,
}

impl WeakPixels {
    pub(crate) fn is_current(&self, ticket: u64) -> bool {
        self.inner.upgrade().is_some_and(|inner| inner.ticket.load(Ordering::Acquire) == ticket)
    }

    pub(crate) fn store_mips_if_current(&self, ticket: u64, mips: Arc<crate::Decoded>) -> bool {
        let (Some(inner), Some(source)) = (self.inner.upgrade(), self.source.upgrade()) else {
            return false;
        };
        store_mips_if_current_inner(&inner, &source, ticket, mips)
    }
}

fn reclaim_rgba(replaced: FrameData) -> Vec<u8> {
    match replaced {
        FrameData::Rgba(arc) => Arc::into_inner(arc).unwrap_or_default(),
        FrameData::Empty | FrameData::Mips(_) => Vec::new(),
    }
}

fn store_mips_if_current_inner(
    inner: &FrameInner,
    source: &rune_core::live::Source,
    ticket: u64,
    mips: Arc<crate::Decoded>,
) -> bool {
    let (width, height) = (mips.mips[0].width, mips.mips[0].height);
    let replaced = {
        let mut frame = inner.frame.lock().unwrap();
        if inner.ticket.load(Ordering::Acquire) != ticket {
            drop(frame);
            return false;
        }
        frame.width = width;
        frame.height = height;
        frame.generation += 1;
        std::mem::replace(&mut frame.data, FrameData::Mips(mips))
    };
    drop(replaced);
    source.notify();
    true
}

/// A replaceable RGBA image, shared between the producer and the view. Clone it freely.
#[derive(Clone)]
pub struct Pixels {
    id: u64,
    inner: Arc<FrameInner>,
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
        let (width, height, generation) = {
            let frame = self.inner.frame.lock().unwrap();
            (frame.width, frame.height, frame.generation)
        };
        write!(f, "Pixels({}, {}x{}, gen {})", self.id, width, height, generation)
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
            inner: Arc::new(FrameInner {
                frame: Mutex::new(Frame { width: 0, height: 0, generation: 0, data: FrameData::Empty }),
                ticket: AtomicU64::new(0),
            }),
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
    ///
    /// To recycle the previous frame's buffer instead of dropping it, use [`Pixels::replace`].
    /// To build downscaling mipmaps off the main thread into this handle, use
    /// [`crate::ImageCache::rgba_into`].
    pub fn set(&self, width: u32, height: u32, rgba: Vec<u8>) {
        let _ = self.replace(width, height, rgba);
    }

    /// Replaces the image like [`Pixels::set`] and returns the buffer the previous frame held,
    /// so a streaming producer can refill and reuse it without allocating a new buffer every
    /// frame.
    ///
    /// Returns an empty `Vec` on the first frame, when replacing decoded mipmaps, or while an
    /// in-flight render copy or [`Pixels::with_rgba`] borrow still holds the previous buffer
    /// (that buffer is freed once the reader finishes). No spare buffer is retained inside
    /// `Pixels` when idle.
    ///
    /// # Example
    /// ```
    /// use rune_image::Pixels;
    ///
    /// let pixels = Pixels::new();
    /// let mut spare = pixels.replace(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    /// assert!(spare.is_empty());
    /// spare.resize(8, 0);
    /// let recycled = pixels.replace(2, 1, spare);
    /// assert_eq!(recycled.len(), 8);
    /// ```
    pub fn replace(&self, width: u32, height: u32, rgba: Vec<u8>) -> Vec<u8> {
        assert_eq!(Some(rgba.len()), (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4)), "rgba must hold width * height pixels");
        if rgba.is_empty() {
            let replaced = {
                let mut frame = self.inner.frame.lock().unwrap();
                self.advance_ticket();
                frame.width = width;
                frame.height = height;
                frame.generation += 1;
                std::mem::replace(&mut frame.data, FrameData::Empty)
            };
            let old = reclaim_rgba(replaced);
            self.source.notify();
            return old;
        }
        // Fast path: when no reader holds a clone of the current Arc, swap the Vec in place so
        // steady-state streaming allocates nothing per update.
        let (rgba, old_vec) = {
            let mut frame = self.inner.frame.lock().unwrap();
            if let FrameData::Rgba(arc) = &mut frame.data
                && let Some(slot) = Arc::get_mut(arc)
            {
                self.advance_ticket();
                let old = std::mem::replace(slot, rgba);
                frame.width = width;
                frame.height = height;
                frame.generation += 1;
                (None, Some(old))
            } else {
                (Some(rgba), None)
            }
        };
        if let Some(old) = old_vec {
            self.source.notify();
            return old;
        }
        // Slow path: first frame, replacing mips, or an in-flight copy/borrow holds the old Arc.
        // Allocate the new Arc outside the lock, then swap it in and reclaim or drop the replaced
        // holder outside the lock.
        let data = FrameData::Rgba(Arc::new(rgba.unwrap()));
        let replaced = {
            let mut frame = self.inner.frame.lock().unwrap();
            self.advance_ticket();
            frame.width = width;
            frame.height = height;
            frame.generation += 1;
            std::mem::replace(&mut frame.data, data)
        };
        let old = reclaim_rgba(replaced);
        self.source.notify();
        old
    }

    /// Runs `f` with the current straight-alpha sRGB RGBA frame: `(width, height, bytes)`, rows
    /// top first, without copying the pixel buffer. Returns `None` when empty or when the handle
    /// holds decoded mipmaps (from [`crate::ImageCache`]).
    ///
    /// The frame lock is released before `f` runs, so calling [`Pixels::size`], [`Pixels::set`],
    /// or [`Pixels::replace`] inside `f` does not deadlock and does not block rendering. While
    /// `f` runs, the borrowed slice keeps showing the snapshotted frame even if another call
    /// replaces it.
    ///
    /// # Example
    /// ```
    /// use rune_image::Pixels;
    ///
    /// let pixels = Pixels::new();
    /// pixels.set(2, 1, vec![10, 20, 30, 255, 40, 50, 60, 255]);
    /// let first_red = pixels.with_rgba(|w, h, bytes| {
    ///     assert_eq!((w, h), (2, 1));
    ///     bytes[0]
    /// });
    /// assert_eq!(first_red, Some(10));
    /// ```
    pub fn with_rgba<R>(&self, f: impl FnOnce(u32, u32, &[u8]) -> R) -> Option<R> {
        let (width, height, rgba) = {
            let frame = self.inner.frame.lock().unwrap();
            match &frame.data {
                FrameData::Rgba(rgba) => (frame.width, frame.height, Arc::clone(rgba)),
                FrameData::Empty | FrameData::Mips(_) => return None,
            }
        };
        Some(f(width, height, &rgba))
    }

    /// Reads the current pixels back as straight-alpha sRGB RGBA8 (`width * height * 4` bytes,
    /// rows top first), whether this handle holds raw RGBA from [`Pixels::set`] /
    /// [`Pixels::replace`] or decoded mipmaps from [`crate::ImageCache`]. Returns `None` when
    /// empty or not yet decoded.
    ///
    /// Releases the frame lock before copying or converting pixels, so it does not block
    /// rendering. For mip-backed images, follows [`crate::Image::to_rgba`]'s rounding and
    /// SVG intrinsic-size rules.
    ///
    /// ```
    /// use rune_image::Pixels;
    ///
    /// let pixels = Pixels::new();
    /// assert_eq!(pixels.to_rgba(), None);
    /// pixels.set(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 128]);
    /// assert_eq!(pixels.to_rgba(), Some((2, 1, vec![255, 0, 0, 255, 0, 0, 255, 128])));
    /// ```
    pub fn to_rgba(&self) -> Option<(u32, u32, Vec<u8>)> {
        let (width, height, data) = {
            let frame = self.inner.frame.lock().unwrap();
            (frame.width, frame.height, frame.data.clone())
        };
        match data {
            FrameData::Empty => None,
            FrameData::Rgba(rgba) => (width > 0 && height > 0).then(|| (width, height, (*rgba).clone())),
            FrameData::Mips(decoded) => crate::readback::decoded_to_rgba(&decoded),
        }
    }

    pub(crate) fn advance_ticket(&self) -> u64 {
        self.inner.ticket.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub(crate) fn downgrade(&self) -> WeakPixels {
        WeakPixels {
            inner: Arc::downgrade(&self.inner),
            source: Arc::downgrade(&self.source),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn store_mips_if_current(&self, ticket: u64, mips: Arc<crate::Decoded>) -> bool {
        store_mips_if_current_inner(&self.inner, &self.source, ticket, mips)
    }

    /// Stores `mips` without notifying subscribers, so [`crate::Image::finish`] can publish
    /// `decoded` before waking windows (`Image::wait` and `is_ready` see the pixels, and woken
    /// windows see `is_ready` and `size`).
    pub(crate) fn store_mips(&self, mips: Arc<crate::Decoded>) {
        let (width, height) = (mips.mips[0].width, mips.mips[0].height);
        let replaced = {
            let mut frame = self.inner.frame.lock().unwrap();
            self.advance_ticket();
            frame.width = width;
            frame.height = height;
            frame.generation += 1;
            std::mem::replace(&mut frame.data, FrameData::Mips(mips))
        };
        drop(replaced);
    }

    pub fn size(&self) -> (u32, u32) {
        let frame = self.inner.frame.lock().unwrap();
        (frame.width, frame.height)
    }

    #[doc(hidden)]
    pub fn signature(&self) -> (u64, u64) {
        let frame = self.inner.frame.lock().unwrap();
        (self.id, frame.generation)
    }

    /// The image's RGBA bytes as [`Pixels::set`] gave them (empty for a decoded image's mips).
    #[doc(hidden)]
    pub fn rgba(&self) -> Vec<u8> {
        let data = self.inner.frame.lock().unwrap().data.clone();
        match data {
            FrameData::Rgba(rgba) => (*rgba).clone(),
            FrameData::Empty | FrameData::Mips(_) => Vec::new(),
        }
    }

    /// Holds the frame lock until dropped, so tests can pause a concurrent pixel update.
    #[cfg(test)]
    pub(crate) fn hold_frame(&self) -> impl Drop + '_ {
        self.inner.frame.lock().unwrap()
    }

    /// Weak handle to the current streaming RGBA buffer, so tests can verify reference counts.
    #[cfg(test)]
    fn rgba_weak(&self) -> Option<std::sync::Weak<Vec<u8>>> {
        match &self.inner.frame.lock().unwrap().data {
            FrameData::Rgba(rgba) => Some(Arc::downgrade(rgba)),
            FrameData::Empty | FrameData::Mips(_) => None,
        }
    }

    /// Raw pointer to the current streaming `Arc<Vec<u8>>` without taking a `Weak` count, so
    /// tests can verify in-place `Arc` reuse.
    #[cfg(test)]
    fn rgba_arc_ptr(&self) -> Option<*const Vec<u8>> {
        match &self.inner.frame.lock().unwrap().data {
            FrameData::Rgba(rgba) => Some(Arc::as_ptr(rgba)),
            FrameData::Empty | FrameData::Mips(_) => None,
        }
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
    /// Weak reference to the `Pixels` backing storage; once dropped and not drawn this frame,
    /// the slot can never be drawn again and is reclaimed immediately.
    live: std::sync::Weak<FrameInner>,
    /// Drawn in the last completed frame.
    shown: bool,
    /// Drawn in this frame so far.
    touched: bool,
    /// `ImageAtlas::frame` when this slot was last drawn, for LRU eviction under bound pressure.
    last_frame: u64,
    /// Set on slots left on overflow pages above `IMAGE_ATLAS_MAX_PAGES` so their next draw
    /// relocates them onto an earlier page.
    relocate: bool,
}

pub use crate::shelf::ImagePage;

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
    /// Monotonic frame counter advanced by `end_frame`, used for LRU eviction.
    frame: u64,
    /// Rects of touched slots replaced by a mid-frame resize, held until the frame finishes drawing.
    deferred_this_frame: Vec<(usize, u32, u32, u32)>,
    /// Deferred rects from the previous frame, released on the next `end_frame` or `rest`.
    deferred_prev_frame: Vec<(usize, u32, u32, u32)>,
    /// Frames copied in so far, so the idle meter sees new pixels in a slot it already drew.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub copies: u64,
    /// How long the last `place_at` call held `pixels.frame`.
    #[cfg(test)]
    last_lock_hold: std::time::Duration,
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
        self.place_at_filtered(pixels, drawn, now, ImageFilter::Linear)
    }

    /// [`Self::place_at`] with an explicit sampling filter: [`ImageFilter::Nearest`] selects mip
    /// level 0 (the highest level within `max_side()`) instead of linear-prefiltered downsampled
    /// mips so crisp pixel edges stay sharp even when scaled down.
    #[doc(hidden)]
    pub fn place_at_filtered(
        &mut self,
        pixels: &Pixels,
        drawn: [f32; 2],
        now: Option<f64>,
        filter: ImageFilter,
    ) -> Option<(Placed, Option<f64>)> {
        self.place_at_inner(pixels, drawn, now, filter, |_| {})
    }

    #[cfg(test)]
    fn place_at_with_copy_hook(
        &mut self,
        pixels: &Pixels,
        drawn: [f32; 2],
        now: Option<f64>,
        on_copy_row: impl FnMut(usize),
    ) -> Option<(Placed, Option<f64>)> {
        self.place_at_inner(pixels, drawn, now, ImageFilter::Linear, on_copy_row)
    }

    fn place_at_inner(
        &mut self,
        pixels: &Pixels,
        drawn: [f32; 2],
        now: Option<f64>,
        filter: ImageFilter,
        mut on_copy_row: impl FnMut(usize),
    ) -> Option<(Placed, Option<f64>)> {
        #[cfg(test)]
        let lock_start = rune_core::time::now();
        let (frame_w, frame_h, generation, data) = {
            let frame = pixels.inner.frame.lock().unwrap();
            if frame.width == 0 || frame.height == 0 || frame.width > crate::MAX_SIDE || frame.height > crate::MAX_SIDE {
                #[cfg(test)]
                { self.last_lock_hold = lock_start.elapsed(); }
                return None;
            }
            (frame.width, frame.height, frame.generation, frame.data.clone())
        };
        #[cfg(test)]
        { self.last_lock_hold = lock_start.elapsed(); }
        let (mut w, mut h) = (frame_w, frame_h);
        let mut level = 0;
        let mut shown = 0;
        let mut next = None;
        let premultiplied = matches!(data, FrameData::Mips(_));
        let bytes: &[u8] = match &data {
            FrameData::Mips(decoded) => {
                let mips = match (&decoded.motion, now) {
                    (Some(motion), Some(now)) => {
                        let start = *self.starts.entry(pixels.id).or_insert(now);
                        let (i, change) = motion.at(now - start);
                        (shown, next) = (i, change.map(|t| start + t));
                        motion.frame(&decoded.mips, i)
                    }
                    _ => &decoded.mips[..],
                };
                // Choose the smallest prefiltered level that still covers both device-pixel axes
                // when linearly filtered. Nearest-neighbour requests mip 0 so downsampled linear
                // mips never blur hard pixel edges. No mip construction happens on the render thread.
                if filter == ImageFilter::Linear {
                    while let Some(next) = mips.get(level + 1) {
                        if (next.width as f32) < drawn[0].clamp(1.0, frame_w as f32)
                            || (next.height as f32) < drawn[1].clamp(1.0, frame_h as f32) { break; }
                        level += 1;
                    }
                }
                // A level past the GPU's largest texture can't go up: draw from the largest that fits.
                let limit = max_side();
                while mips[level].width.max(mips[level].height) > limit && level + 1 < mips.len() { level += 1; }
                let mip = &mips[level];
                (w, h) = (mip.width, mip.height);
                &mip.pixels
            }
            FrameData::Rgba(rgba) => rgba,
            FrameData::Empty => return None,
        };
        if w > max_side() || h > max_side() { return None; }
        let key = (pixels.id, level);
        if self.slots.get(&key).is_some_and(|s| s.w != w || s.h != h)
            && let Some(old) = self.slots.remove(&key)
        {
            if old.touched {
                self.deferred_this_frame.push((old.page, old.x, old.y, old.w));
            } else {
                self.pages[old.page].free_rect(old.x, old.y, old.w);
            }
        } else if self.slots.get(&key).is_some_and(|s| s.relocate && !s.touched) {
            if let Some((page, x, y)) = self.try_allocate_within(w, h, IMAGE_ATLAS_MAX_PAGES, true) {
                if let Some(old) = self.slots.remove(&key) {
                    self.pages[old.page].free_rect(old.x, old.y, old.w);
                }
                self.slots.insert(key, Slot {
                    page, x, y, w, h, generation: 0, frame: 0,
                    live: Arc::downgrade(&pixels.inner),
                    shown: false, touched: false, last_frame: self.frame, relocate: false,
                });
            } else if let Some(slot) = self.slots.get_mut(&key) {
                slot.relocate = false;
            }
        }
        if !self.slots.contains_key(&key) {
            let (page, x, y) = self.allocate(w, h);
            self.slots.insert(key, Slot {
                page, x, y, w, h, generation: 0, frame: 0,
                live: Arc::downgrade(&pixels.inner),
                shown: false, touched: false, last_frame: self.frame, relocate: false,
            });
        }
        let slot = self.slots.get_mut(&key).unwrap();
        slot.touched = true;
        slot.last_frame = self.frame;
        self.pages[slot.page].touched = true;
        if slot.generation != generation || slot.frame != shown {
            slot.generation = generation;
            slot.frame = shown;
            #[cfg(debug_assertions)]
            { self.copies += 1; }
            let page = &mut self.pages[slot.page];
            let row = (page.width * 4) as usize;
            for line in 0..h as usize {
                let dst = (slot.y as usize + line) * row + slot.x as usize * 4;
                let src = line * w as usize * 4;
                page.data[dst..dst + w as usize * 4].copy_from_slice(&bytes[src..src + w as usize * 4]);
                on_copy_row(line);
            }
            let (y0, y1) = (slot.y, slot.y + h);
            page.dirty = Some(page.dirty.map_or((y0, y1), |(a, b)| (a.min(y0), b.max(y1))));
        }
        Some((Placed { rect: [slot.x, slot.y, w, h], page: slot.page, premultiplied }, next))
    }

    /// Whether the atlas currently holds a slot for `pixels` at mip `level`.
    #[doc(hidden)]
    pub fn has_slot(&self, pixels: &Pixels, level: usize) -> bool {
        self.slots.contains_key(&(pixels.id, level))
    }

    /// Forgets dropped images everywhere, images on untouched pages above page 0, and oldest
    /// unshown slots past `IMAGE_ATLAS_MAX_SLOTS` / `IMAGE_ATLAS_MAX_BYTES` / `IMAGE_ATLAS_MAX_PAGES`,
    /// freeing their shelf spans and emptying pages left with zero slots.
    pub fn end_frame(&mut self) {
        self.frame = self.frame.saturating_add(1);
        if !self.deferred_prev_frame.is_empty() {
            for (page, x, y, w) in self.deferred_prev_frame.drain(..) {
                if let Some(p) = self.pages.get_mut(page) { p.free_rect(x, y, w); }
            }
        }
        let first_kept = self.pages.first().is_some_and(ImagePage::is_shelf);
        {
            let Self { pages, slots, .. } = self;
            slots.retain(|_, s| {
                let keep = s.touched || (s.live.strong_count() > 0 && (pages[s.page].touched || (s.page == 0 && first_kept)));
                if !keep { pages[s.page].free_rect(s.x, s.y, s.w); }
                keep
            });
        }
        self.enforce_bounds();
        if !self.starts.is_empty() {
            let slots = &self.slots;
            self.starts.retain(|id, _| slots.keys().any(|(slot, _)| slot == id));
        }
        for slot in self.slots.values_mut() {
            slot.shown = std::mem::take(&mut slot.touched);
        }
        let Self { pages, slots, deferred_this_frame, .. } = self;
        for (i, page) in pages.iter_mut().enumerate() {
            page.shown = std::mem::take(&mut page.touched);
            if !slots.values().any(|slot| slot.page == i) && !deferred_this_frame.iter().any(|&(p, ..)| p == i) {
                page.shelves.reset();
                page.dirty = None;
                page.data = Vec::new();
            }
        }
        let keep = self.slots.values().map(|s| s.page + 1)
            .chain(self.deferred_this_frame.iter().map(|&(p, ..)| p + 1))
            .max().unwrap_or(0);
        self.pages.truncate(keep);
        if self.pages.len() > IMAGE_ATLAS_MAX_PAGES
            && self.pages[..IMAGE_ATLAS_MAX_PAGES].iter().any(ImagePage::has_free_space)
        {
            for slot in self.slots.values_mut() {
                if slot.page >= IMAGE_ATLAS_MAX_PAGES { slot.relocate = true; }
            }
        }
        std::mem::swap(&mut self.deferred_prev_frame, &mut self.deferred_this_frame);
    }

    /// A window at rest (`window::rest`): forgets the images no element showed in the last
    /// frame (or whose handles were dropped), the first page's included, shrinks shelf pages to
    /// their remaining rows, and frees the CPU copy of every page left empty.
    pub fn rest(&mut self) {
        for (page, x, y, w) in self.deferred_prev_frame.drain(..).chain(self.deferred_this_frame.drain(..)) {
            if let Some(p) = self.pages.get_mut(page) { p.free_rect(x, y, w); }
        }
        {
            let Self { pages, slots, .. } = self;
            slots.retain(|_, s| {
                let keep = s.shown && s.live.strong_count() > 0;
                if !keep { pages[s.page].free_rect(s.x, s.y, s.w); }
                keep
            });
        }
        let slots = &self.slots;
        self.starts.retain(|id, _| slots.keys().any(|(slot, _)| slot == id));
        for (i, page) in self.pages.iter_mut().enumerate() {
            if !self.slots.values().any(|slot| slot.page == i) {
                page.empty();
            } else {
                page.shrink_to_fit_rows();
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
        self.deferred_this_frame.clear();
        self.deferred_prev_frame.clear();
        had
    }

    /// Marks every row for upload (a new texture).
    pub fn invalidate(&mut self) {
        for page in &mut self.pages {
            if !page.data.is_empty() {
                page.dirty = Some((0, page.height));
            }
        }
    }

    fn try_allocate_within(&mut self, w: u32, h: u32, max_pages: usize, reclaim_stale: bool) -> Option<(usize, u32, u32)> {
        let limit = max_pages.min(self.pages.len());
        for (i, page) in self.pages[..limit].iter_mut().enumerate() {
            if page.data.is_empty() { *page = ImagePage::new(w, h); }
            if let Some((x, y)) = page.allocate(w, h) { return Some((i, x, y)); }
        }
        let is_shelf_req = w <= IMAGE_ATLAS_SIZE && h <= IMAGE_ATLAS_SIZE;
        // Full: start over an idle shelf page before growing.
        if is_shelf_req && let Some(i) = self.pages[..limit].iter().enumerate().position(|(i, p)| {
            !p.shown && !p.touched && p.is_shelf() && !p.shelves.is_empty() && !self.page_pinned(i)
        }) {
            self.slots.retain(|_, s| s.page != i);
            let page = &mut self.pages[i];
            page.shelves.reset();
            let (x, y) = page.allocate(w, h).unwrap();
            return Some((i, x, y));
        }
        if !reclaim_stale { return None; }
        // At or past the atlas bound: reclaim idle pages or stale unshown slots on shelf pages.
        if let Some(i) = self.pages[..limit].iter().enumerate().position(|(i, p)| !p.shown && !p.touched && !self.page_pinned(i)) {
            self.slots.retain(|_, s| s.page != i);
            self.pages[i] = ImagePage::new(w, h);
            let (x, y) = self.pages[i].allocate(w, h).unwrap();
            return Some((i, x, y));
        }
        if is_shelf_req && self.slots.values().any(|s| s.page < limit && !s.touched && !s.shown && self.pages[s.page].is_shelf()) {
            let need_area = u64::from(w) * u64::from(h);
            let max_area = u64::from(IMAGE_ATLAS_SIZE) * u64::from(IMAGE_ATLAS_SIZE);
            let mut evictable: Vec<((u64, usize), bool, u64)> = self.slots.iter()
                .filter(|(_, s)| s.page < limit && !s.touched && !s.shown && self.pages[s.page].is_shelf())
                .filter(|(_, s)| {
                    let pinned: u64 = self.slots.values()
                        .filter(|o| o.page == s.page && (o.touched || o.shown))
                        .map(|o| u64::from(o.w) * u64::from(o.h))
                        .sum();
                    pinned.saturating_add(need_area) <= max_area
                })
                .map(|(&k, s)| (k, s.live.strong_count() > 0, s.last_frame))
                .collect();
            evictable.sort_by_key(|&(_, alive, last)| (alive, last));
            for (key, _, _) in evictable {
                if let Some(s) = self.slots.remove(&key) {
                    self.pages[s.page].free_rect(s.x, s.y, s.w);
                    if let Some((x, y)) = self.pages[s.page].allocate(w, h) {
                        return Some((s.page, x, y));
                    }
                }
            }
        }
        None
    }

    fn allocate(&mut self, w: u32, h: u32) -> (usize, u32, u32) {
        let need_bytes = ImagePage::initial_bytes(w, h);
        let reclaim = self.pages.len() >= IMAGE_ATLAS_MAX_PAGES
            || self.total_bytes().saturating_add(need_bytes) > IMAGE_ATLAS_MAX_BYTES;
        if let Some(placed) = self.try_allocate_within(w, h, self.pages.len(), reclaim) {
            return placed;
        }
        let mut page = ImagePage::new(w, h);
        let (x, y) = page.allocate(w, h).unwrap();
        self.pages.push(page);
        (self.pages.len() - 1, x, y)
    }

    fn page_pinned(&self, page: usize) -> bool {
        self.deferred_this_frame.iter().any(|&(p, ..)| p == page)
    }

    fn total_bytes(&self) -> usize {
        self.pages.iter().map(|p| p.data.len()).sum()
    }

    fn active_pages(&self) -> usize {
        self.pages.iter().filter(|p| !p.data.is_empty()).count()
    }

    fn enforce_bounds(&mut self) {
        let over = |a: &Self| {
            a.slots.len() > IMAGE_ATLAS_MAX_SLOTS
                || a.total_bytes() > IMAGE_ATLAS_MAX_BYTES
                || a.active_pages() > IMAGE_ATLAS_MAX_PAGES
        };
        if !over(self) { return; }
        let mut evictable: Vec<((u64, usize), u64)> = self.slots.iter()
            .filter(|(_, s)| !s.touched)
            .map(|(&k, s)| (k, s.last_frame))
            .collect();
        evictable.sort_by_key(|&(_, last)| last);
        for (key, _) in evictable {
            if !over(self) { break; }
            if let Some(s) = self.slots.remove(&key) {
                self.pages[s.page].free_rect(s.x, s.y, s.w);
                if !self.page_pinned(s.page) && !self.slots.values().any(|o| o.page == s.page) {
                    self.pages[s.page].empty();
                }
            }
        }
        if self.total_bytes() > IMAGE_ATLAS_MAX_BYTES {
            for page in &mut self.pages { page.shrink_to_fit_rows(); }
        }
    }
}



#[cfg(test)]
#[path = "pixels_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "atlas_tests.rs"]
mod atlas_tests;
