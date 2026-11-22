//! Sandboxed, off-UI-thread LaTeX math rendering for closed ```` ```latex ```` and
//! ```` ```math ```` Markdown fences (`anomalyco/opencode#64380`), `$$` paragraphs, and inline
//! `$…$`, `\(…\)` and `$$…$$` in text, which stand on the line's baseline (in text style but for
//! `$$`) and show their source until rendered or when they fail.
//!
//! - **Zero startup and zero hot-path cost:** the `wasmi` interpreter (native) / Web Worker
//!   (`wasm32`), the deflated `ratex-math.wasm` module, and the deflated KaTeX font pack are never
//!   touched until the first math in a document is drawn.
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

#[cfg(test)]
mod inline_tests;

#[cfg(not(target_arch = "wasm32"))]
use sandbox::MAX_INPUT_BYTES;
#[cfg(target_arch = "wasm32")]
use web_worker::MAX_INPUT_BYTES;

/// Cached entries (ready + failed) kept for rows that aren't showing them; what the last two commits
/// drew and what is still rendering are never evicted, so a screen of math may go over it.
const MAX_CACHE_ENTRIES: usize = 256;
/// RGBA bytes kept the same way: about two screens of a 2560×1440 window full of math (one bitmap is
/// at most 4 MiB).
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
/// Sources a full worker queue refused, sent as it drains (each at most 16 KiB of TeX).
const MAX_WAITING: usize = 1024;
/// Commits a bitmap stays after a view drew it or it finished, whatever the caps.
const KEEP_COMMITS: u64 = 16;
/// Rows whose waiting sources are remembered (`collect`); past it the list starts again.
const MAX_ROWS: usize = 4096;
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
    /// The baseline below the bitmap's top edge, and the clear margin around the ink, in points.
    pub(crate) baseline_pt: f32,
    pub(crate) margin_pt: f32,
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

struct Cached {
    entry: CacheEntry,
    /// The UI thread's [`rune::commit_generation`] when a view last drew it.
    used: u64,
}

/// A render the worker queue had no room for, sent when it has.
struct Waiting {
    key: MathKey,
    em_pt: f32,
    ink: Color,
    scale: f32,
}

#[derive(Default)]
struct MathCache {
    entries: HashMap<MathKey, Cached>,
    order: VecDeque<MathKey>,
    rendered_srcs: HashMap<Arc<str>, usize>,
    total_bytes: usize,
    /// The latest commit a view drew math in.
    commit: u64,
    /// Renders finished (ready or failed) so far, and the count each source last finished at: a row
    /// that drew a source still rendering builds again once it moves, and only then.
    finishes: u64,
    finished: HashMap<Arc<str>, u64>,
    waiting: VecDeque<Waiting>,
}

impl MathCache {
    fn touch(&mut self, key: &MathKey) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.clone());
    }

    fn remove_entry(&mut self, key: &MathKey) {
        if let Some(Cached { entry: CacheEntry::Ready(r), .. }) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(r.byte_len);
            if let Some(count) = self.rendered_srcs.get_mut(&key.src) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.rendered_srcs.remove(&key.src);
                }
            }
        }
    }

    /// Whether `key` may go to make room: not still rendering, and neither drawn nor finished within
    /// the last `KEEP_COMMITS` commits (on screen, or about to be: every window commits once per
    /// finish). Eviction never wakes a window or moves a row's version: a row showing the bitmap
    /// keeps its pixels, and only a row that builds again asks for it anew.
    fn evictable(&self, key: &MathKey) -> bool {
        self.entries.get(key).is_some_and(|c| !matches!(c.entry, CacheEntry::Pending) && c.used.saturating_add(KEEP_COMMITS) < self.commit)
    }

    fn insert(&mut self, key: MathKey, entry: CacheEntry) {
        let used = self.commit;
        self.remove_entry(&key);
        if let CacheEntry::Ready(ref r) = entry {
            self.total_bytes = self.total_bytes.saturating_add(r.byte_len);
            *self.rendered_srcs.entry(key.src.clone()).or_insert(0) += 1;
        }
        self.entries.insert(key.clone(), Cached { entry, used });
        self.touch(&key);
        while self.entries.len() > MAX_CACHE_ENTRIES || self.total_bytes > MAX_CACHE_BYTES {
            let Some(pos) = self.order.iter().position(|k| *k != key && self.evictable(k)) else { break };
            if let Some(oldest) = self.order.remove(pos) {
                self.remove_entry(&oldest);
            }
        }
    }

    fn clear_bitmaps(&mut self) {
        let ready_keys: Vec<MathKey> = self
            .entries
            .iter()
            .filter_map(|(k, v)| matches!(v.entry, CacheEntry::Ready(_)).then(|| k.clone()))
            .collect();
        for k in &ready_keys {
            self.remove_entry(k);
        }
        self.order.retain(|k| self.entries.contains_key(k));
    }
}

static CACHE: OnceLock<Mutex<MathCache>> = OnceLock::new();
static CLIENTS: Mutex<Vec<std::sync::Weak<ClientState>>> = Mutex::new(Vec::new());
static GENERATION: AtomicU64 = AtomicU64::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn cache() -> &'static Mutex<MathCache> {
    CACHE.get_or_init(|| Mutex::new(MathCache::default()))
}

#[derive(Default)]
struct ClientState {
    waker: Mutex<Option<Waker>>,
    seen: AtomicU64,
    resting: std::sync::atomic::AtomicBool,
}

/// One window's (or view's) part in display math: its own wake when a render finishes, its own
/// record of which finishes it has seen, and its say in when cached bitmaps may go. Keep one per
/// window; dropping it leaves.
pub struct Client(Arc<ClientState>);

impl Default for Client {
    fn default() -> Self {
        let state = Arc::new(ClientState::default());
        state.seen.store(GENERATION.load(Ordering::Acquire), Ordering::Relaxed);
        if let Ok(mut list) = CLIENTS.lock() {
            list.retain(|c| c.strong_count() > 0);
            list.push(Arc::downgrade(&state));
        }
        Self(state)
    }
}

impl Client {
    /// This window's waker, called when a background render finishes.
    pub fn set_waker(&self, waker: Waker) {
        if let Ok(mut w) = self.0.waker.lock() {
            *w = Some(waker);
        }
    }

    /// True once after renders finished since this window last asked: its view should commit, so
    /// rows that drew those sources (and only those, [`version`]) build again. Also sends renders a
    /// full worker queue held back.
    pub fn changed(&self) -> bool {
        pump_waiting();
        let now = GENERATION.load(Ordering::Acquire);
        self.0.seen.swap(now, Ordering::AcqRel) != now
    }

    /// This window shows math again (after [`Client::rest`]).
    pub fn hold(&self) {
        self.0.resting.store(false, Ordering::Relaxed);
    }

    /// This window shows no math now. Cached bitmaps go only once every window rests: another
    /// window's rows still draw them.
    pub fn rest(&self) {
        self.0.resting.store(true, Ordering::Relaxed);
        let all_rest = CLIENTS.lock().is_ok_and(|list| list.iter().filter_map(std::sync::Weak::upgrade).all(|c| c.resting.load(Ordering::Relaxed)));
        if all_rest
            && let Some(c) = CACHE.get()
            && let Ok(mut guard) = c.lock()
        {
            guard.clear_bitmaps();
            let kept: std::collections::HashSet<Arc<str>> = guard.entries.keys().map(|k| k.src.clone()).collect();
            guard.finished.retain(|src, _| kept.contains(src));
        }
    }
}

thread_local! {
    /// Sources still rendering that the builds now running drew, each with its finish count as it was
    /// drawn, innermost build last (`collect`).
    static COLLECTING: std::cell::RefCell<Vec<Vec<(Arc<str>, u64)>>> = const { std::cell::RefCell::new(Vec::new()) };
    static ROWS: std::cell::RefCell<HashMap<String, Row>> = std::cell::RefCell::new(HashMap::new());
}

/// A row's sources that were still rendering when it last built, each with the finish count it had
/// then, and how many times one of them has finished since rows began.
#[derive(Default)]
struct Row {
    waiting: Vec<(Arc<str>, u64)>,
    version: u64,
}

/// Builds `f` (a row of a host's own memo, under its `key`), noting which display math it drew
/// that is still rendering, for [`version`].
pub fn collect<R>(key: &str, f: impl FnOnce() -> R) -> R {
    COLLECTING.with_borrow_mut(|stack| stack.push(Vec::new()));
    let out = f();
    let mut waiting = COLLECTING.with_borrow_mut(|stack| stack.pop()).unwrap_or_default();
    waiting.sort();
    waiting.dedup_by(|a, b| a.0 == b.0);
    COLLECTING.with_borrow_mut(|stack| {
        if let Some(outer) = stack.last_mut() {
            outer.extend(waiting.iter().cloned());
        }
    });
    ROWS.with_borrow_mut(|rows| {
        if rows.len() >= MAX_ROWS && !rows.contains_key(key) {
            rows.clear();
        }
        rows.entry(key.to_owned()).or_default().waiting = waiting;
    });
    out
}

/// The display-math version of the row [`collect`] built under `key`, for the host memo's key: it
/// moves only when a source that row drew while it was still rendering finishes. A bitmap leaving
/// the cache never moves it.
pub fn version(key: &str) -> u64 {
    ROWS.with_borrow_mut(|rows| {
        let Some(row) = rows.get_mut(key) else { return 0 };
        if row.waiting.is_empty() {
            return row.version;
        }
        let Some(c) = CACHE.get() else { return row.version };
        let Ok(guard) = c.lock() else { return row.version };
        let mut moved = false;
        for (src, at) in &mut row.waiting {
            let now = guard.finished.get(src).copied().unwrap_or(0);
            if now != *at {
                *at = now;
                moved = true;
            }
        }
        if moved {
            row.version = row.version.wrapping_add(1);
        }
        row.version
    })
}

fn finish_job(key: MathKey, rendered: Option<raster::Rasterized>) {
    let entry = match rendered {
        Some(r) => CacheEntry::Ready(RenderedMath {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            pixels: r.pixels,
            width_pt: r.width_pt,
            height_pt: r.height_pt,
            baseline_pt: r.baseline_pt,
            margin_pt: r.margin_pt,
            byte_len: r.byte_len,
        }),
        None => CacheEntry::Failed,
    };
    if let Ok(mut guard) = cache().lock() {
        let src = key.src.clone();
        guard.insert(key, entry);
        guard.finishes += 1;
        let n = guard.finishes;
        guard.finished.insert(src, n);
    }
    GENERATION.fetch_add(1, Ordering::Release);
    let wakers: Vec<Waker> = CLIENTS
        .lock()
        .map(|list| list.iter().filter_map(std::sync::Weak::upgrade).filter_map(|c| c.waker.lock().ok().and_then(|w| w.clone())).collect())
        .unwrap_or_default();
    for w in wakers {
        w();
    }
}

/// Sends what a full queue held back while it has room (on the UI thread: the web worker's queue
/// lives there).
fn pump_waiting() {
    loop {
        let Some(job) = CACHE.get().and_then(|c| c.lock().ok()?.waiting.pop_front()) else { return };
        if !send(job.key.clone(), job.em_pt, job.ink, job.scale) {
            if let Ok(mut guard) = cache().lock() {
                guard.waiting.push_front(job);
            }
            return;
        }
    }
}

fn send(key: MathKey, em_pt: f32, ink: Color, scale: f32) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return sandbox::enqueue(key, em_pt, ink, scale);
    #[cfg(target_arch = "wasm32")]
    return web_worker::enqueue(key, em_pt, ink, scale);
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

/// Fast check for whether `src` might hold any math: a `latex` or `math` fence, or the `$`,
/// `\(` or `\[` that inline math and display paragraphs open with. Hosts that memoise rows of
/// Markdown use it to tell which rows ask the math cache (see [`collect`] and [`version`]).
pub fn may_have_math(src: &str) -> bool {
    may_have_inline_math(src) || may_have_math_fence(src)
}

pub(crate) fn may_have_inline_math(src: &str) -> bool {
    src.contains('$') || src.contains("\\(") || src.contains("\\[")
}

/// The TeX of inline math as written (`$…$`, `\(…\)`, `$$…$$`): `$$` keeps display style, the
/// others set text style, as KaTeX's inline mode does.
fn inline_tex(raw: &str) -> Option<String> {
    let (tex, display) = if let Some(t) = raw.strip_prefix("$$").and_then(|t| t.strip_suffix("$$")) {
        (t, true)
    } else if let Some(t) = raw.strip_prefix("\\(").and_then(|t| t.strip_suffix("\\)")) {
        (t, false)
    } else {
        (raw.strip_prefix('$').and_then(|t| t.strip_suffix('$'))?, false)
    };
    let tex = tex.trim();
    (!tex.is_empty()).then(|| if display { tex.to_owned() } else { format!("\\textstyle {tex}") })
}

/// [`poll_or_enqueue`] for inline math as written (`raw`, delimiters and all) at `body_pt`.
pub(crate) fn poll_inline(raw: &str, body_pt: f32, ink: Color, scale: f32) -> MathStatus {
    match inline_tex(raw) {
        Some(tex) => poll_or_enqueue(&tex, body_pt, ink, scale),
        None => MathStatus::Failed,
    }
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
/// background render on the first miss. A source still rendering is noted for the row being
/// built ([`collect`]).
pub(crate) fn poll_or_enqueue(source: &str, body_pt: f32, ink: Color, scale: f32) -> MathStatus {
    let trimmed = source.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_INPUT_BYTES {
        return MathStatus::Failed;
    }
    pump_waiting();
    let key = MathKey::new(trimmed, body_pt, ink, scale);
    let commit = rune::commit_generation();
    let (status, seen) = {
        let Ok(mut guard) = cache().lock() else {
            return MathStatus::Failed;
        };
        guard.commit = guard.commit.max(commit);
        // Read with the status, so a render finishing before the row's build ends still moves it.
        let seen = guard.finished.get(&key.src).copied().unwrap_or(0);
        match guard.entries.get_mut(&key) {
            Some(c) => {
                c.used = commit;
                let status = match &c.entry {
                    CacheEntry::Ready(r) => MathStatus::Ready(r.clone()),
                    CacheEntry::Pending => MathStatus::Pending,
                    CacheEntry::Failed => MathStatus::Failed,
                };
                if matches!(status, MathStatus::Ready(_)) {
                    guard.touch(&key);
                }
                (Some(status), seen)
            }
            None => {
                guard.insert(key.clone(), CacheEntry::Pending);
                (None, seen)
            }
        }
    };
    let status = status.unwrap_or_else(|| {
        if send(key.clone(), body_pt, ink, scale) {
            return MathStatus::Pending;
        }
        // The queue is full: the render waits its turn (still Pending), or, past that, this build
        // shows the code and the next build of the row asks again.
        let Ok(mut guard) = cache().lock() else { return MathStatus::Failed };
        if guard.waiting.len() < MAX_WAITING {
            guard.waiting.push_back(Waiting { key: key.clone(), em_pt: body_pt, ink, scale });
            return MathStatus::Pending;
        }
        guard.remove_entry(&key);
        guard.order.retain(|k| *k != key);
        MathStatus::Failed
    });
    if matches!(status, MathStatus::Pending) {
        COLLECTING.with_borrow_mut(|stack| {
            if let Some(top) = stack.last_mut() {
                top.push((key.src.clone(), seen));
            }
        });
    }
    status
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
