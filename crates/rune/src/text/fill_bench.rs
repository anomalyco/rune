//! The CPU mask fill measured on real frames. With `RUNE_FILL_RECORD=<dir>` a debug build writes
//! each frame's queued fills, and the outline records they read, to `<dir>/fills-<n>.bin`;
//! [`FillSet`] reads one back and fills, times or counts it. A measurement, not a fill.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::atlas_fill::{Fill, QueuedFill};
use super::outline::{self, F4, OutlineStore, Portable};
pub use super::outline::Census;

const MAGIC: [u8; 8] = *b"RUNEFIL1";

/// Masks to fill and the outline store they read.
pub struct FillSet {
    store: OutlineStore,
    fills: Vec<QueuedFill>,
}

/// The lanes a fill integrates on: the target's vector form ([`F4`]), four plain `f32`s (the form
/// on targets without one), or one at a time. All three give the same bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillLanes {
    Native,
    Portable,
    One,
}

impl FillSet {
    pub(super) fn new(store: OutlineStore, fills: Vec<QueuedFill>) -> FillSet { FillSet { store, fills } }

    pub fn decode(bytes: &[u8]) -> Option<FillSet> {
        let mut words = bytes.strip_prefix(&MAGIC)?.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c));
        let mut next = || words.next();
        let texels = (0..next()?).map(|_| Some([next()?, next()?, next()?, next()?])).collect::<Option<Vec<_>>>()?;
        let fills = (0..next()?).map(|_| {
            let rect = [next()?, next()?, next()?, next()?];
            let (record, hint, scale, radius) = (next()?, next()?, f32::from_bits(next()?), f32::from_bits(next()?));
            let pen = [f32::from_bits(next()?), f32::from_bits(next()?)];
            Some(QueuedFill { rect, fill: Fill { record, hint, scale, radius, pen, coverage: next()? } })
        }).collect::<Option<Vec<_>>>()?;
        Some(FillSet { store: OutlineStore::from_texels(texels), fills })
    }

    pub fn masks(&self) -> usize { self.fills.len() }

    pub fn texels(&self) -> u64 { self.fills.iter().map(|q| u64::from(q.rect[2] * q.rect[3])).sum() }

    /// Every mask's bytes, one after another.
    pub fn bytes(&self, lanes: FillLanes) -> Vec<u8> {
        let mut outs = vec![Vec::new(); self.fills.len()];
        for group in outline::share_rows(&self.fills) {
            fill(&self.store, &self.fills, &group, lanes, &mut outs, None);
        }
        outs.concat()
    }

    /// What filling it does.
    pub fn census(&self) -> Census {
        let mut census = Census::default();
        let mut outs = vec![Vec::new(); self.fills.len()];
        for group in outline::share_rows(&self.fills) {
            fill(&self.store, &self.fills, &group, FillLanes::Native, &mut outs, Some(&mut census));
        }
        census
    }

    /// Fills every mask, the biggest groups of masks that share rows first, on `threads` threads
    /// that are already running and spin until told to start (so no wake or spawn is timed): the
    /// wall time, ns. The grouping is timed too.
    pub fn time(&self, threads: usize, lanes: FillLanes) -> u64 {
        let (next, done, go) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicBool::new(false));
        let order = std::sync::OnceLock::new();
        let work = |order: &Vec<Vec<u32>>| {
            let mut outs: Vec<Vec<u8>> = Vec::new();
            loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some(group) = order.get(k) else { break };
                outs.resize_with(group.len(), || Vec::with_capacity(4096));
                fill(&self.store, &self.fills, group, lanes, &mut outs, None);
                std::hint::black_box(&outs);
            }
            done.fetch_add(1, Ordering::AcqRel);
        };
        std::thread::scope(|s| {
            for _ in 1..threads.max(1) {
                s.spawn(|| {
                    while !go.load(Ordering::Acquire) { std::hint::spin_loop(); }
                    if let Some(order) = order.get() { work(order); }
                });
            }
            // Let the workers reach their spin before the clock starts.
            std::thread::sleep(std::time::Duration::from_millis(2));
            let start = std::time::Instant::now();
            let mut groups = outline::share_rows(&self.fills);
            let size = |g: &Vec<u32>| g.iter().map(|&i| self.fills[i as usize].rect[2] * self.fills[i as usize].rect[3]).sum::<u32>();
            groups.sort_by_key(|g| std::cmp::Reverse(size(g)));
            let order = order.get_or_init(|| groups);
            go.store(true, Ordering::Release);
            work(order);
            while done.load(Ordering::Acquire) < threads.max(1) { std::hint::spin_loop(); }
            start.elapsed().as_nanos() as u64
        })
    }
}

/// Fills `group` (indices into `fills`); `outs` holds a buffer for every fill (indexed like
/// `fills`) or, timed, one per member of the group.
fn fill(store: &OutlineStore, fills: &[QueuedFill], group: &[u32], lanes: FillLanes, outs: &mut [Vec<u8>], census: Option<&mut Census>) {
    let mut own: Vec<Vec<u8>> = if outs.len() == group.len() { Vec::new() } else { group.iter().map(|_| Vec::new()).collect() };
    let slots: &mut [Vec<u8>] = if own.is_empty() { outs } else { &mut own };
    match lanes {
        FillLanes::Native => outline::fill_shared::<F4>(store, fills, group, slots, census),
        FillLanes::Portable => outline::fill_shared::<Portable>(store, fills, group, slots, census),
        FillLanes::One => outline::fill_shared::<f32>(store, fills, group, slots, census),
    }
    if !own.is_empty() {
        for (&i, bytes) in group.iter().zip(own) { outs[i as usize] = bytes; }
    }
}
/// The recording of `fills` over `store`.
#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
fn encode(store: &OutlineStore, fills: &[QueuedFill]) -> Vec<u8> {
    let texels = store.used_texels();
    let mut out = MAGIC.to_vec();
    let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());
    put(texels.len() as u32);
    texels.iter().flatten().for_each(|&v| put(v));
    put(fills.len() as u32);
    for q in fills {
        q.rect.iter().for_each(|&v| put(v));
        let f = &q.fill;
        [f.record, f.hint, f.scale.to_bits(), f.radius.to_bits(), f.pen[0].to_bits(), f.pen[1].to_bits(), f.coverage].iter().for_each(|&v| put(v));
    }
    out
}

/// Writes a frame's fills to `RUNE_FILL_RECORD`'s folder, numbered in turn, when it's set.
#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
pub(super) fn record(store: &OutlineStore, fills: &[(usize, Vec<QueuedFill>)]) {
    static DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let Some(dir) = DIR.get_or_init(|| std::env::var_os("RUNE_FILL_RECORD").map(Into::into)) else { return };
    let fills: Vec<QueuedFill> = fills.iter().flat_map(|(_, f)| f.iter().copied()).collect();
    if fills.is_empty() { return; }
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(dir.join(format!("fills-{n}.bin")), encode(store, &fills))) {
        eprintln!("rune: fill record: {e}");
    }
}
