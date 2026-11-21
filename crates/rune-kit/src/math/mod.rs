//! Sandboxed, off-UI-thread LaTeX display math rendering for closed ```` ```latex ```` and
//! ```` ```math ```` Markdown fences (`anomalyco/opencode#64380`).
//!
//! - **Zero startup and zero hot-path cost:** the `wasmi` interpreter (native) / Web Worker
//!   (`wasm32`), the deflated `ratex-math.wasm` module, and the deflated KaTeX font pack are never
//!   touched until the first closed math fence settles.
//! - **Host-enforced isolation:** every limit (16 KiB input, 40M fuel, 16 MiB linear memory,
//!   250 ms wall-clock deadline, 4096×4096 max texture dim, 4 MiB max single bitmap, 2048 dash cap,
//!   and full `RM01` wire validation) is enforced outside the guest. Any failure falls back to
//!   syntax-highlighted code and is negatively cached so it is never retried on scroll or hover.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rune::{Color, Direction, El, Pixels, Waker, col, el};

mod raster;
#[cfg(not(target_arch = "wasm32"))]
mod sandbox;
#[allow(dead_code)]
mod wasm_hash;
#[cfg(target_arch = "wasm32")]
mod web_worker;
mod wire;

#[cfg(not(target_arch = "wasm32"))]
use sandbox::MAX_INPUT_BYTES;
#[cfg(target_arch = "wasm32")]
use web_worker::MAX_INPUT_BYTES;

/// Maximum cached entries (ready + failed + pending) before LRU eviction.
const MAX_CACHE_ENTRIES: usize = 128;
/// Maximum total RGBA bytes kept across all cached math bitmaps (8 MiB; single bitmap cap is 4 MiB).
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;
/// Tight vertical padding around display math (on top of Markdown's block margins).
const DISPLAY_PAD_Y: f32 = 2.0;

/// Exact cache key (no keyless `u64` hash collisions).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct MathKey {
    src: Arc<str>,
    em_bits: u32,
    ink: [u8; 4],
    scale_bits: u32,
}

impl MathKey {
    fn new(src: &str, body_pt: f32, ink: Color, scale: f32) -> Self {
        let ink_rgba = [
            (ink.r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (ink.g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (ink.b.clamp(0.0, 1.0) * 255.0).round() as u8,
            (ink.a.clamp(0.0, 1.0) * 255.0).round() as u8,
        ];
        Self {
            src: Arc::from(src),
            em_bits: body_pt.clamp(8.0, 72.0).to_bits(),
            ink: ink_rgba,
            scale_bits: scale.clamp(0.5, 3.0).to_bits(),
        }
    }
}

/// A rendered math bitmap ready to paint, with monotonic identity for memo keys.
#[derive(Clone)]
pub(crate) struct RenderedMath {
    pub(crate) id: u64,
    pub(crate) pixels: Pixels,
    pub(crate) width_pt: f32,
    pub(crate) height_pt: f32,
    byte_len: usize,
}

#[derive(Clone)]
pub(crate) enum MathStatus {
    Ready(RenderedMath),
    Pending,
    Failed,
}

enum CacheEntry {
    Pending,
    Failed,
    Ready(RenderedMath),
}

#[derive(Default)]
struct MathCache {
    entries: HashMap<MathKey, CacheEntry>,
    order: VecDeque<MathKey>,
    rendered_srcs: HashMap<Arc<str>, usize>,
    total_bytes: usize,
}

impl MathCache {
    fn touch(&mut self, key: &MathKey) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.clone());
    }

    fn remove_entry(&mut self, key: &MathKey) {
        if let Some(CacheEntry::Ready(r)) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(r.byte_len);
            if let Some(count) = self.rendered_srcs.get_mut(&key.src) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.rendered_srcs.remove(&key.src);
                }
            }
        }
    }

    fn insert(&mut self, key: MathKey, entry: CacheEntry) {
        self.remove_entry(&key);
        if let CacheEntry::Ready(ref r) = entry {
            self.total_bytes = self.total_bytes.saturating_add(r.byte_len);
            *self.rendered_srcs.entry(key.src.clone()).or_insert(0) += 1;
        }
        self.entries.insert(key.clone(), entry);
        self.touch(&key);
        // Never evict the entry we just inserted (`r.byte_len <= 4 MiB <= MAX_CACHE_BYTES`).
        while (self.entries.len() > MAX_CACHE_ENTRIES || self.total_bytes > MAX_CACHE_BYTES)
            && self.order.len() > 1
        {
            if let Some(oldest) = self.order.pop_front() {
                if oldest == key {
                    self.order.push_back(oldest);
                    break;
                }
                self.remove_entry(&oldest);
            }
        }
    }

    fn clear_bitmaps(&mut self) {
        let ready_keys: Vec<MathKey> = self
            .entries
            .iter()
            .filter_map(|(k, v)| matches!(v, CacheEntry::Ready(_)).then(|| k.clone()))
            .collect();
        for k in &ready_keys {
            self.remove_entry(k);
        }
        self.order.retain(|k| self.entries.contains_key(k));
    }
}

static CACHE: OnceLock<Mutex<MathCache>> = OnceLock::new();
static WAKER: OnceLock<Mutex<Option<Waker>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn cache() -> &'static Mutex<MathCache> {
    CACHE.get_or_init(|| Mutex::new(MathCache::default()))
}

/// Registers the app's [`rune::Waker`] so completed background math renders wake the UI immediately.
pub fn set_waker(waker: Waker) {
    let slot = WAKER.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(waker);
    }
}

/// Monotonic generation bumped whenever a background math render finishes (ready or failed).
/// Hosts with outer row memos can include this only on rows containing math fences.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Releases cached math bitmaps (for example when a window rests), keeping negative failure records.
pub fn rest() {
    if let Some(c) = CACHE.get()
        && let Ok(mut guard) = c.lock()
    {
        guard.clear_bitmaps();
    }
}

fn finish_job(key: MathKey, rendered: Option<raster::Rasterized>) {
    let entry = match rendered {
        Some(r) => CacheEntry::Ready(RenderedMath {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            pixels: r.pixels,
            width_pt: r.width_pt,
            height_pt: r.height_pt,
            byte_len: r.byte_len,
        }),
        None => CacheEntry::Failed,
    };
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, entry);
    }
    GENERATION.fetch_add(1, Ordering::Release);
    if let Some(slot) = WAKER.get()
        && let Ok(guard) = slot.lock()
        && let Some(w) = guard.as_ref()
    {
        w();
    }
}

/// Whether a fenced code block's info-string language names display math (`latex` or `math`),
/// matching `anomalyco/opencode`'s `isMathFence` (`packages/ui/src/context/marked-parser.tsx`).
pub fn is_math_fence(lang: Option<&str>) -> bool {
    lang.is_some_and(|l| l.eq_ignore_ascii_case("latex") || l.eq_ignore_ascii_case("math"))
}

/// Fast ASCII check for whether `src` might contain a `latex` or `math` code fence (used to
/// avoid invalidating whole-doc/row memos on documents that have no math fences).
pub fn may_have_math_fence(src: &str) -> bool {
    if !src.contains("```") && !src.contains("~~~") {
        return false;
    }
    let b = src.as_bytes();
    b.windows(5).any(|w| w.eq_ignore_ascii_case(b"latex"))
        || b.windows(4).any(|w| w.eq_ignore_ascii_case(b"math"))
}

/// Whether `source` currently has a ready rendered math bitmap in the cache (used by
/// `Markdown::find` so conversation search skips rendered math without skipping fallback code).
pub(crate) fn is_rendered(source: &str) -> bool {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Some(c) = CACHE.get() else {
        return false;
    };
    c.lock()
        .ok()
        .is_some_and(|guard| guard.rendered_srcs.contains_key(trimmed))
}

/// Looks up `(source, body_pt, ink, scale)` without blocking the UI thread, enqueueing a
/// background render on the first miss.
pub(crate) fn poll_or_enqueue(source: &str, body_pt: f32, ink: Color, scale: f32) -> MathStatus {
    let trimmed = source.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_INPUT_BYTES {
        return MathStatus::Failed;
    }
    let key = MathKey::new(trimmed, body_pt, ink, scale);
    {
        let Ok(mut guard) = cache().lock() else {
            return MathStatus::Failed;
        };
        match guard.entries.get(&key) {
            Some(CacheEntry::Ready(r)) => {
                let r = r.clone();
                guard.touch(&key);
                return MathStatus::Ready(r);
            }
            Some(CacheEntry::Pending) => return MathStatus::Pending,
            Some(CacheEntry::Failed) => return MathStatus::Failed,
            None => {
                guard.insert(key.clone(), CacheEntry::Pending);
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let queued = sandbox::enqueue(key.clone(), body_pt, ink, scale);
    #[cfg(target_arch = "wasm32")]
    let queued = web_worker::enqueue(key.clone(), body_pt, ink, scale);
    if !queued {
        if let Ok(mut guard) = cache().lock() {
            guard.remove_entry(&key);
        }
        return MathStatus::Failed;
    }
    MathStatus::Pending
}

/// Builds the display-math element for a ready `RenderedMath` inside `style`, honouring
/// `avail_width`, `scroll_wide`, and `wide_fade`.
pub(crate) fn view(
    source: &str,
    rendered: &RenderedMath,
    wide_fade: f32,
    avail_width: Option<f32>,
    scroll_wide: bool,
) -> El {
    let img = el()
        .key("math")
        .role("img")
        .label(source.trim().to_owned())
        .size(rendered.width_pt, rendered.height_pt)
        .pixels(&rendered.pixels);
    let needs_scroll = match avail_width {
        Some(avail) => rendered.width_pt > avail,
        None => scroll_wide,
    };
    if !needs_scroll {
        return col()
            .dir(Direction::Ltr)
            .w_full()
            .py(DISPLAY_PAD_Y)
            .center()
            .child(img);
    }
    let mut scroller = col().key("scroll").w_full().scroll_x();
    if wide_fade > 0.0 {
        scroller = scroller.scroll_edge_fade(wide_fade);
    }
    col()
        .dir(Direction::Ltr)
        .w_full()
        .py(DISPLAY_PAD_Y)
        .child(scroller.child(img))
}

/// Synchronous host-sandbox helper for external proof/fuzz harnesses (never called from the UI).
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub fn eval_wire_for_proof(source: &str) -> Option<Vec<u8>> {
    thread_local! {
        static PROOF_SB: std::cell::RefCell<Option<sandbox::Sandbox>> = const { std::cell::RefCell::new(None) };
    }
    PROOF_SB.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = sandbox::Sandbox::new();
        }
        slot.as_mut()?.eval_wire(source.trim())
    })
}

/// Host `RM01` wire decoder + rasteriser entry point for external fuzz harnesses treating guest
/// output bytes as adversarial.
#[doc(hidden)]
pub fn decode_and_raster_for_proof(wire_bytes: &[u8], body_pt: f32, ink: Color, scale: f32) -> Option<(u32, u32)> {
    let dl = wire::decode(wire_bytes)?;
    let r = raster::rasterize(&dl, body_pt, ink, scale)?;
    Some((
        (r.width_pt * scale.clamp(0.5, 3.0)).round() as u32,
        (r.height_pt * scale.clamp(0.5, 3.0)).round() as u32,
    ))
}
