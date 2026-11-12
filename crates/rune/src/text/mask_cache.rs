//! Windfoil's atlas masks kept across launches: a glyph whose mask an earlier launch of this
//! build made is placed from those bytes, with no outline built and no fill, so a first screen
//! of text seen before costs a copy. It's a cache of computed inputs, not a picture: every mask
//! is keyed by what decides its texels (the font by its identity, ont_identity, never this run's id;
//! the glyph, size, variation, x and y phase, dilation, look, slant and synthetic bold), and the
//! whole cache by this build's link stamp, so any other build, font or setting misses and fills
//! as before. It's read before the first frame wherever the app reads it from ([`offer`]), and
//! written a few seconds after launch and then as the screen's masks change, on a thread of its
//! own ([`save_with`]), bounded by [`BUDGET`] with the masks in use first. Windows only for now:
//! it's the platform whose first frame is filled on the CPU, and the one whose image carries a
//! link stamp to key by.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::atlas_fill::{QueuedFill, Residency};
use super::*;
use crate::machine::Effects;
use crate::time::Instant;

/// The encoding's version: bump it whenever what's stored changes shape.
const FORMAT: u32 = 2;
const MAGIC: [u8; 8] = *b"RUNEMSK1";
/// Mask bytes a cache keeps: the masks in use when it's written, then earlier ones, most recent
/// first. A session's first screen at 2880x1800 is about 112 KB.
pub const BUDGET: usize = 1 << 20;
/// Seconds after the first frame before the first write (never during launch), and between
/// writes after it.
const SETTLE: f64 = 3.0;
const BETWEEN: f64 = 30.0;

/// A mask's key with the font named by its identity (ont_identity).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct StableKey {
    font: u64,
    index: u32,
    glyph: u32,
    size: u32,
    coords: u64,
    dilation: u32,
    subpx: u8,
    subpy: u8,
    apple: u8,
    skew: u8,
    bold: bool,
    /// A colour glyph's RGBA image (`color_glyphs`), not an alpha or LCD mask.
    color: bool,
}

const KEY_BYTES: usize = 40;
const ENTRY_BYTES: usize = KEY_BYTES + 16;

impl StableKey {
    fn of(key: &GlyphKey, font: u64) -> StableKey {
        StableKey { font, index: key.index, glyph: key.glyph, size: key.size, coords: key.coords, dilation: key.dilation, subpx: key.subpx, subpy: key.subpy, apple: key.apple, skew: key.skew, bold: key.bold, color: false }
    }

    /// Colour glyph `key`'s (its `color_key`).
    fn color(key: &GlyphKey, font: u64) -> StableKey {
        StableKey { color: true, ..StableKey::of(key, font) }
    }

    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.font.to_le_bytes());
        out.extend_from_slice(&self.coords.to_le_bytes());
        for v in [self.index, self.glyph, self.size, self.dilation] { out.extend_from_slice(&v.to_le_bytes()); }
        out.extend_from_slice(&[self.subpx, self.subpy, self.apple, self.skew, u8::from(self.bold), u8::from(self.color), 0, 0]);
    }

    fn read(b: &[u8]) -> StableKey {
        let u64_at = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or_default());
        let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap_or_default());
        StableKey { font: u64_at(0), coords: u64_at(8), index: u32_at(16), glyph: u32_at(20), size: u32_at(24), dilation: u32_at(28), subpx: b[32], subpy: b[33], apple: b[34], skew: b[35], bold: b[36] != 0, color: b[37] != 0 }
    }
}

/// A stored mask: texels `w` x `h` (three a pixel when `lcd`, four RGBA bytes when `color`),
/// its offset from the pen, and where its bytes start in the cache's data. A glyph that draws
/// nothing is 0 x 0.
#[derive(Clone, Copy)]
struct Entry { w: u32, h: u32, left: i32, top: i32, lcd: bool, color: bool, at: usize }

impl Entry {
    fn len(&self) -> usize { (self.w * self.h) as usize * if self.color { 4 } else { 1 } }
}

/// Masks an earlier launch of this build saved, decoded.
#[derive(Default)]
pub struct MaskCache {
    entries: HashMap<StableKey, Entry, std::hash::BuildHasherDefault<rune_motion::Fx>>,
    data: Vec<u8>,
    /// It holds colour glyphs, so a colour font's lookups are worth naming it.
    color: bool,
}

impl MaskCache {
    /// The masks in `bytes` if this build wrote them (its link stamp and this encoding);
    /// anything else is an empty cache.
    pub fn decode(bytes: &[u8]) -> MaskCache {
        decode(bytes, build_stamp()).unwrap_or_default()
    }

    /// Masks it holds.
    pub fn len(&self) -> usize { self.entries.len() }

    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
}

fn decode(bytes: &[u8], stamp: Option<u64>) -> Option<MaskCache> {
    let stamp = stamp?;
    let rest = bytes.strip_prefix(&MAGIC)?;
    let u32_at = |b: &[u8], i: usize| b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    if u32_at(rest, 0)? != FORMAT || rest.get(4..12)? != stamp.to_le_bytes() { return None; }
    let count = u32_at(rest, 12)? as usize;
    let table = rest.get(16..16 + count * ENTRY_BYTES)?;
    let data = rest.get(16 + count * ENTRY_BYTES..)?.to_vec();
    let mut entries = HashMap::with_capacity_and_hasher(count, Default::default());
    let table = table.as_chunks::<ENTRY_BYTES>().0;
    for e in table {
        let at = |i: usize| u32_at(e, KEY_BYTES + i).unwrap_or_default();
        let (wl, h, left, top) = (at(0), at(4), at(8) as i32, at(12) as i32);
        let (w, lcd, color) = (wl & 0x3fff_ffff, wl >> 31 != 0, wl >> 30 & 1 != 0);
        let entry = Entry { w, h, left, top, lcd, color, at: 0 };
        entries.insert(StableKey::read(&e[..KEY_BYTES]), entry);
    }
    // Bytes follow in the table's order.
    let mut at = 0;
    for e in table {
        let Some(entry) = entries.get_mut(&StableKey::read(&e[..KEY_BYTES])) else { continue };
        entry.at = at;
        at += entry.len();
    }
    let color = entries.values().any(|e| e.color);
    (at == data.len()).then_some(MaskCache { entries, data, color })
}

/// The masks offered for the text systems made from now on, and those already running.
static OFFERED: Mutex<Option<Arc<MaskCache>>> = Mutex::new(None);
/// Where text systems hand the masks they save.
static SINK: OnceLock<fn(Vec<u8>)> = OnceLock::new();

/// Gives every text system the masks an earlier launch saved (`bytes` as [`save_with`]'s sink
/// got them): decoded here, on the caller's thread. Call it as early as the bytes are read, on
/// any thread; a text system takes it at its next frame, the first one included.
pub fn offer(bytes: &[u8]) {
    let cache = MaskCache::decode(bytes);
    *OFFERED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(cache));
}

/// Where text systems hand the masks to keep: `sink` gets the encoded bytes on a thread of the
/// cache's own, a few seconds after launch and then as the masks in use change. Set it once.
pub fn save_with(sink: fn(Vec<u8>)) {
    let _sink = SINK.get_or_init(|| sink);
}

/// This build's identity: the link time stamp the running image's PE header carries, which every
/// link of other code changes. None where there's no such stamp, and the cache stays off.
pub fn build_stamp() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        unsafe extern "C" {
            /// The image's base, which the MSVC linker defines.
            static __ImageBase: u8;
        }
        // SAFETY: `__ImageBase` is the start of this module's mapped image: a DOS header whose
        // `e_lfanew` (at 0x3c) leads to the PE signature and file header, all mapped and
        // readable for the life of the process. Reads are unaligned.
        unsafe {
            let base = &raw const __ImageBase;
            let pe = base.add(0x3c).cast::<u32>().read_unaligned() as usize;
            let stamp = base.add(pe + 8).cast::<u32>().read_unaligned();
            (stamp != 0).then_some(u64::from(stamp))
        }
    }
    #[cfg(not(target_os = "windows"))]
    None
}

/// When a text system writes its masks: as an explicit machine.
#[derive(Default)]
pub(super) enum Saving {
    /// No frame yet.
    #[default]
    Unstarted,
    /// Launch settling: the first write waits until `SETTLE` after `since`.
    Settling { since: Instant },
    /// A write is on its thread; `done` is set when it's through.
    Writing { done: Arc<AtomicBool>, placed: u64 },
    /// Written with the atlas at `placed` masks placed, at `at`.
    Written { at: Instant, placed: u64 },
}

pub(super) enum SavingEvent {
    Frame { now: Instant, placed: u64 },
    /// A timer's poll on a window that draws nothing new: it writes when due, but never starts
    /// the settling (the first frame does).
    Poll { now: Instant, placed: u64 },
}

#[derive(PartialEq, Eq)]
pub(super) enum SavingEffect {
    Write,
    /// Ask the window to wake when the first write is due, so one that draws nothing more
    /// after launch still writes.
    WakeIn(std::time::Duration),
}

impl Saving {
    pub(super) fn step(&mut self, event: SavingEvent) -> Effects<SavingEffect> {
        let (now, placed, frame) = match event {
            SavingEvent::Frame { now, placed } => (now, placed, true),
            SavingEvent::Poll { now, placed } => (now, placed, false),
        };
        match self {
            Saving::Unstarted if frame => {
                *self = Saving::Settling { since: now };
                return Effects::one(SavingEffect::WakeIn(std::time::Duration::from_secs_f64(SETTLE)));
            }
            Saving::Settling { since } if now.duration_since(*since).as_secs_f64() >= SETTLE => return Effects::one(SavingEffect::Write),
            Saving::Writing { done, placed: p } if done.load(Ordering::Acquire) => *self = Saving::Written { at: now, placed: *p },
            Saving::Written { at, placed: p } if *p != placed && now.duration_since(*at).as_secs_f64() >= BETWEEN => return Effects::one(SavingEffect::Write),
            _ => {}
        }
        Effects::none()
    }

    /// A write started for the atlas at `placed`.
    pub(super) fn writing(&mut self, done: Arc<AtomicBool>, placed: u64) {
        *self = Saving::Writing { done, placed };
    }
}

/// A text system's side of the cache: the masks it was offered, its fonts' hashes, and its
/// writes.
#[derive(Default)]
pub(crate) struct Masks {
    offered: Option<Arc<MaskCache>>,
    /// Each font's (blob id's) bytes and their identity, once worked out.
    fonts: FxHashMap<u64, (parley::fontique::Blob<u8>, Option<u64>)>,
    saving: Saving,
}

impl TextSystem {
    /// The cached mask for `key` (a Windfoil mask's), placed in the atlas as one the CPU drew;
    /// None when there's no cache or it lacks the mask.
    pub(super) fn cached_mask(&mut self, key: &GlyphKey, face: &Face) -> Option<Rasterized> {
        let cache = self.masks.offered.clone()?;
        if cache.is_empty() { return None; }
        let font = self.font_hash(face);
        let entry = *cache.entries.get(&StableKey::of(key, font))?;
        if crate::marks::counting() { crate::marks::add("text.mask_cache_hits", 1); }
        if entry.w == 0 || entry.h == 0 {
            self.atlas.map.insert(*key, Residency::Blank);
            return Some(Rasterized::Empty);
        }
        let bytes = &cache.data[entry.at..entry.at + entry.len()];
        let mut mask = self.atlas.put(entry.w, entry.h, bytes, entry.left, entry.top)?;
        mask.lcd = entry.lcd;
        self.atlas.map.insert(*key, Residency::Uploaded(mask));
        Some(Rasterized::Placed(mask))
    }

    /// The colour glyph an earlier launch saved for `key` (its `color_key`), as a fresh paint
    /// gives it: the image and its placement. None when there's no cache or it lacks the glyph.
    pub(super) fn cached_color(&mut self, key: &GlyphKey, face: &Face) -> Option<(crate::pixels::Pixels, swash::zeno::Placement)> {
        let cache = self.masks.offered.clone()?;
        if !cache.color { return None; }
        let font = self.font_hash(face);
        let entry = *cache.entries.get(&StableKey::color(key, font))?;
        if crate::marks::counting() { crate::marks::add("text.mask_cache_hits", 1); }
        let pixels = crate::pixels::Pixels::new();
        pixels.set(entry.w, entry.h, cache.data[entry.at..entry.at + entry.len()].to_vec());
        Some((pixels, swash::zeno::Placement { left: entry.left, top: entry.top, width: entry.w, height: entry.h }))
    }

    /// Notes the font of a Windfoil mask, so a write can name it by its bytes.
    pub(super) fn note_mask_font(&mut self, face: &Face) {
        self.masks.fonts.entry(face.font.data.id()).or_insert_with(|| (face.font.data.clone(), None));
    }

    /// `face`'s font's identity, worked out once.
    fn font_hash(&mut self, face: &Face) -> u64 {
        let entry = self.masks.fonts.entry(face.font.data.id()).or_insert_with(|| (face.font.data.clone(), None));
        *entry.1.get_or_insert_with(|| font_identity(face.font.data.as_ref()))
    }

    /// Each frame (and each timer poll, `frame` false): takes the masks offered, and starts a
    /// write when it's due. Returns when the window should wake for the first write.
    pub(crate) fn mask_cache_frame(&mut self, frame: bool) -> Option<std::time::Duration> {
        if self.masks.offered.is_none() {
            self.masks.offered = OFFERED.lock().unwrap_or_else(|e| e.into_inner()).clone();
        }
        let &sink = SINK.get()?;
        let placed = self.atlas.rasterized + self.color_placed;
        let now = Instant::now();
        let event = if frame { SavingEvent::Frame { now, placed } } else { SavingEvent::Poll { now, placed } };
        let mut write = false;
        let mut wake = None;
        for effect in self.masks.saving.step(event) {
            match effect {
                SavingEffect::Write => write = true,
                SavingEffect::WakeIn(after) => wake = Some(after),
            }
        }
        if !write { return wake; }
        let Some(stamp) = build_stamp() else { return wake };
        let snapshot = self.mask_snapshot(stamp);
        let done = Arc::new(AtomicBool::new(false));
        self.masks.saving.writing(done.clone(), placed);
        let spawned = std::thread::Builder::new().name("rune-mask-cache".into()).spawn(move || {
            sink(snapshot.encode());
            done.store(true, Ordering::Release);
        });
        if spawned.is_err() { self.masks.saving = Saving::Written { at: Instant::now(), placed }; }
        wake
    }

    /// What a write needs, copied: the Windfoil masks the atlas holds (their bytes, or their
    /// fills for those the GPU filled), the outline records those read, the fonts, and the
    /// masks offered, kept after these.
    fn mask_snapshot(&self, stamp: u64) -> Snapshot {
        let mut items = Vec::new();
        let mut fills = Vec::new();
        for (key, residency) in self.atlas.map.iter() {
            if key.apple & (glyph_emit::WINDFOIL_KEY | glyph_emit::DIRECT_WRITE_KEY) != glyph_emit::WINDFOIL_KEY || !self.masks.fonts.contains_key(&key.font) { continue; }
            match *residency {
                Residency::Blank => items.push((*key, Item::Blank)),
                Residency::Uploaded(g) => {
                    let Some(page) = self.atlas.page_data(g.page as usize) else { continue };
                    let bytes = (0..g.h).flat_map(|row| {
                        let at = ((g.y + row) * ATLAS_SIZE + g.x) as usize;
                        page[at..at + g.w as usize].iter().copied()
                    }).collect();
                    items.push((*key, Item::Bytes(Mask { w: g.w, h: g.h, left: g.left, top: g.top, lcd: g.lcd, color: false }, bytes)));
                }
                Residency::Pending(g, fill) | Residency::Filled(g, fill) => {
                    items.push((*key, Item::Fill(Mask { w: g.w, h: g.h, left: g.left, top: g.top, lcd: g.lcd, color: false }, fills.len())));
                    fills.push(QueuedFill { rect: [g.x, g.y, g.w, g.h], fill });
                }
            }
        }
        // Colour glyphs after the masks, each its RGBA image under its `color_key`.
        for (key, glyph) in self.color_glyphs.iter() {
            if !self.masks.fonts.contains_key(&key.font) { continue; }
            let Some(pixels) = self.color_pixels.get(glyph.index as usize - 1) else { continue };
            let bytes = pixels.rgba();
            if bytes.len() != (glyph.w * glyph.h * 4) as usize { continue; }
            items.push((*key, Item::Bytes(Mask { w: glyph.w, h: glyph.h, left: glyph.left, top: glyph.top, lcd: false, color: true }, bytes)));
        }
        let store = (!fills.is_empty()).then(|| outline::OutlineStore::from_texels(self.atlas.outlines.used_texels().to_vec()));
        let fonts = self.masks.fonts.iter().map(|(&id, (blob, hash))| (id, (blob.clone(), *hash))).collect();
        Snapshot { stamp, items, fills, store, fonts, earlier: self.masks.offered.clone() }
    }
}

/// A mask's place relative to its pen.
#[derive(Clone, Copy)]
struct Mask { w: u32, h: u32, left: i32, top: i32, lcd: bool, color: bool }

enum Item {
    Blank,
    Bytes(Mask, Vec<u8>),
    /// Filled on the GPU: its fill, by index.
    Fill(Mask, usize),
}

/// A write's inputs, owned, for its thread.
struct Snapshot {
    stamp: u64,
    items: Vec<(GlyphKey, Item)>,
    fills: Vec<QueuedFill>,
    store: Option<outline::OutlineStore>,
    fonts: FxHashMap<u64, (parley::fontique::Blob<u8>, Option<u64>)>,
    earlier: Option<Arc<MaskCache>>,
}

impl Snapshot {
    /// The cache's bytes: the masks in use (the GPU's filled again here), then earlier ones not
    /// among them, until [`BUDGET`].
    fn encode(self) -> Vec<u8> {
        let mut filled: Vec<Vec<u8>> = vec![Vec::new(); self.fills.len()];
        if let Some(store) = &self.store {
            for group in outline::share_rows(&self.fills) {
                let mut outs: Vec<Vec<u8>> = group.iter().map(|_| Vec::new()).collect();
                outline::fill_shared::<outline::F4>(store, &self.fills, &group, &mut outs, None);
                for (&i, bytes) in group.iter().zip(outs) { filled[i as usize] = bytes; }
            }
        }
        let font = |id: u64| self.fonts.get(&id).map(|(blob, hash)| hash.unwrap_or_else(|| font_identity(blob.as_ref())));
        let mut sums: FxHashMap<u64, u64> = FxHashMap::default();
        let mut table = Vec::new();
        let mut data = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut add = |key: StableKey, m: Option<Mask>, bytes: &[u8], table: &mut Vec<u8>, data: &mut Vec<u8>| {
            if data.len() + bytes.len() > BUDGET || !seen.insert(key) { return; }
            key.put(table);
            let m = m.unwrap_or(Mask { w: 0, h: 0, left: 0, top: 0, lcd: false, color: false });
            for v in [m.w | (u32::from(m.lcd) << 31) | (u32::from(m.color) << 30), m.h, m.left as u32, m.top as u32] { table.extend_from_slice(&v.to_le_bytes()); }
            data.extend_from_slice(bytes);
        };
        for (key, item) in &self.items {
            let Some(hash) = sums.get(&key.font).copied().or_else(|| font(key.font)) else { continue };
            sums.insert(key.font, hash);
            let stable = match item {
                Item::Bytes(m, _) if m.color => StableKey::color(key, hash),
                _ => StableKey::of(key, hash),
            };
            match item {
                Item::Blank => add(stable, None, &[], &mut table, &mut data),
                Item::Bytes(m, bytes) => add(stable, Some(*m), bytes, &mut table, &mut data),
                Item::Fill(m, i) => add(stable, Some(*m), &filled[*i], &mut table, &mut data),
            }
        }
        if let Some(earlier) = &self.earlier {
            for (key, e) in &earlier.entries {
                let bytes = &earlier.data[e.at..e.at + e.len()];
                add(*key, Some(Mask { w: e.w, h: e.h, left: e.left, top: e.top, lcd: e.lcd, color: e.color }), bytes, &mut table, &mut data);
            }
        }
        let count = (table.len() / ENTRY_BYTES) as u32;
        let mut out = Vec::with_capacity(8 + 16 + table.len() + data.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT.to_le_bytes());
        out.extend_from_slice(&self.stamp.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&table);
        out.extend_from_slice(&data);
        out
    }
}

/// A font's identity across launches, cheap enough for a 100 MB emoji file at its first lookup:
/// its length, its first and last 64 KiB (the table directory, and the tables at the end), and
/// its first face's `head` table, whose `checkSumAdjustment` sums the whole file and whose
/// `modified` dates it.
fn font_identity(bytes: &[u8]) -> u64 {
    use std::hash::Hasher;
    const EDGE: usize = 64 * 1024;
    let mut h = rustc_hash::FxHasher::default();
    h.write_usize(bytes.len());
    h.write(&bytes[..bytes.len().min(EDGE)]);
    h.write(&bytes[bytes.len().saturating_sub(EDGE)..]);
    if let Some(head) = swash::FontRef::from_index(bytes, 0).and_then(|f| f.table(swash::tag_from_bytes(b"head"))) { h.write(head); }
    h.finish()
}
