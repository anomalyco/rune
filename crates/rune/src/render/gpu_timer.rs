//! Frame-profile builds: the GPU time of each render pass a frame records, from timestamp
//! queries. With `RUNE_GPU_LOG=FILE` (and a device with `TIMESTAMP_QUERY`), each frame is written
//! to FILE as one JSON line once the GPU has done it: its passes in order with their label, their
//! time in microseconds and the bytes of render target each one covers, the frame's span from the
//! first pass's start to the last one's end, and the submit's index. Ordinary builds compile none
//! of this.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Passes timed in one frame; later ones go untimed.
const CAP: u32 = 128;
/// Frames whose results can be in flight at once.
const SLOTS: usize = 4;
const AFTER: &str = "after last pass";

pub(super) struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: Vec<Slot>,
    /// This frame's passes so far: (label, target bytes).
    passes: Mutex<Vec<(&'static str, u64)>>,
    /// This frame's instances by kind (`KIND_*`): count, device pixels rasterized, and those inside their clip.
    kinds: Mutex<Vec<(u32, f64, f64)>>,
    /// The slot resolved for this frame, mapped after its submit.
    pending: Mutex<Option<usize>>,
    frames: std::sync::atomic::AtomicU64,
    period: f32,
    /// The device can stamp between passes (`TIMESTAMP_QUERY_INSIDE_ENCODERS`).
    inside: bool,
    out: Mutex<std::io::BufWriter<std::fs::File>>,
}

struct Slot {
    buffer: wgpu::Buffer,
    passes: Mutex<Vec<(&'static str, u64)>>,
    kinds: Mutex<Vec<(u32, f64, f64)>>,
    frame: std::sync::atomic::AtomicU64,
    /// When the frame was submitted (`host_clock::now_ns`, the live frame log's clock).
    submitted: std::sync::atomic::AtomicU64,
    busy: AtomicBool,
    ready: Arc<AtomicBool>,
}

impl GpuTimer {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<GpuTimer> {
        let path = std::env::var_os("RUNE_GPU_LOG")?;
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) { eprintln!("rune: RUNE_GPU_LOG: the device has no timestamp queries"); return None; }
        let file = std::fs::File::create(&path).ok()?;
        let bytes = u64::from(CAP) * 2 * 8;
        let buffer = |usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some("gpu timer"), size: bytes, usage, mapped_at_creation: false });
        Some(GpuTimer {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("gpu timer"), ty: wgpu::QueryType::Timestamp, count: CAP * 2 }),
            resolve: buffer(wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
            slots: (0..SLOTS).map(|_| Slot { buffer: buffer(wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST), passes: Mutex::new(Vec::new()), kinds: Mutex::new(Vec::new()),
                frame: Default::default(), submitted: Default::default(), busy: AtomicBool::new(false), ready: Arc::new(AtomicBool::new(false)) }).collect(),
            passes: Mutex::new(Vec::new()),
            kinds: Mutex::new(Vec::new()),
            pending: Mutex::new(None),
            frames: Default::default(),
            period: queue.get_timestamp_period(),
            inside: device.features().contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS),
            out: Mutex::new(std::io::BufWriter::new(file)),
        })
    }

    /// This frame's instances: count and covered device pixels by kind, clipped to their clip and the viewport.
    pub(super) fn instances(&self, instances: &[super::Instance], viewport: (u32, u32)) {
        // 0..16 by kind; rects (kind 0) also by class: 16 plain opaque (square, borderless, alpha 1, no fade or
        // clip radius), 17 plain translucent, 18 rounded, 19 bordered (square), 20 faded or clipped round only.
        let mut kinds = vec![(0u32, 0f64, 0f64); 21];
        for i in instances {
            let k = (i.params[1] as usize).min(15);
            // The quad `base_vs` rasterizes: grown past the rect, a band (`runtime/paint/bands.rs`) only its window.
            let grow = match k { 1 => i.params[2] * 3.0 + 1.0, 2 => 0.0, 3 | 5..=9 => 1.0, _ => 1.0 + i.params[2] * 2.0 };
            let (mut lo, mut hi) = ([i.rect[0] - grow, i.rect[1] - grow], [i.rect[0] + i.rect[2] + grow, i.rect[1] + i.rect[3] + grow]);
            if k <= 1 && i.uv[2] < 0.0 {
                lo = [lo[0].max(i.rect[0] + i.uv[0]), lo[1].max(i.rect[1] + i.uv[1])];
                hi = [hi[0].min(i.rect[0] - i.uv[2]).max(lo[0]), hi[1].min(i.rect[1] + i.uv[3]).max(lo[1])];
            }
            let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
            let area = |x0: f32, y0: f32, x1: f32, y1: f32| f64::from((x1.min(vw) - x0.max(0.0)).max(0.0) * (y1.min(vh) - y0.max(0.0)).max(0.0));
            kinds[k].0 += 1;
            // Rasterized (the quad in the viewport: outside the clip a fragment still runs, then discards), and inside the clip.
            kinds[k].1 += area(lo[0], lo[1], hi[0], hi[1]);
            kinds[k].2 += area(lo[0].max(i.clip[0]), lo[1].max(i.clip[1]), hi[0].min(i.clip[2]), hi[1].min(i.clip[3]));
            if k == 0 {
                let plain = i.radii == [0.0; 4] && i.params[0] == 0.0 && i.fade == [0; 4];
                let class = if plain { if i.fill[3] >= 1.0 { 16 } else { 17 } } else if i.radii != [0.0; 4] { 18 } else if i.params[0] != 0.0 { 19 } else { 20 };
                kinds[class].0 += 1;
                kinds[class].1 += area(lo[0], lo[1], hi[0], hi[1]);
                kinds[class].2 += area(lo[0].max(i.clip[0]), lo[1].max(i.clip[1]), hi[0].min(i.clip[2]), hi[1].min(i.clip[3]));
            }
        }
        // RUNE_CENSUS_COVER=1 (rehearsals only: it costs CPU per frame): what covered-rect culling could skip. In
        // reverse paint order on 4 px cells, each instance's rasterized cells already under later plain opaque
        // rects (square, borderless, alpha 1, no fade, no band, inside their clip): 21 all its cells covered (could
        // be dropped), 22 the covered cells of every instance (what a depth pre-pass would reject). A layer, a
        // backdrop or an effect marker (kinds 8, 9, 12, 13) clears the cells: nothing is assumed across a target change.
        if std::env::var_os("RUNE_CENSUS_COVER").is_some() {
            kinds.resize(27, (0, 0.0, 0.0));
            static DUMPED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let dump = std::env::var_os("RUNE_CENSUS_COVER_DUMP").is_some() && DUMPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 300;
            const CELL: f32 = 4.0;
            let (cw, ch) = ((viewport.0 as f32 / CELL).ceil() as usize, (viewport.1 as f32 / CELL).ceil() as usize);
            let mut covered = vec![false; cw * ch];
            let across = std::env::var_os("RUNE_CENSUS_COVER").is_some_and(|v| v == "across");
            let cells = |x0: f32, y0: f32, x1: f32, y1: f32, inside: bool| {
                // Cells whose centre is in the box (inside = false) or that lie wholly in it (inside = true).
                let (a, b) = if inside { ((x0 / CELL).ceil(), (x1 / CELL).floor()) } else { ((x0 / CELL - 0.5).ceil(), (x1 / CELL - 0.5).floor() + 1.0) };
                let (c, d) = if inside { ((y0 / CELL).ceil(), (y1 / CELL).floor()) } else { ((y0 / CELL - 0.5).ceil(), (y1 / CELL - 0.5).floor() + 1.0) };
                (a.max(0.0) as usize, (b.max(0.0) as usize).min(cw), c.max(0.0) as usize, (d.max(0.0) as usize).min(ch))
            };
            for i in instances.iter().rev() {
                let k = (i.params[1] as usize).min(15);
                // RUNE_CENSUS_COVER=across: an upper bound that also counts what a layer's opaque content covers below it.
                if matches!(k, 8 | 9 | 12 | 13) { kinds[23].0 += 1; if !across { covered.iter_mut().for_each(|c| *c = false); } continue; }
                let grow = match k { 1 => i.params[2] * 3.0 + 1.0, 2 => 0.0, 3 | 5..=7 | 10 => 1.0, _ => 1.0 + i.params[2] * 2.0 };
                // The quad it rasterizes, as above: a band (`uv.z < 0`) only its window.
                let (mut lo, mut hi) = ([i.rect[0] - grow, i.rect[1] - grow], [i.rect[0] + i.rect[2] + grow, i.rect[1] + i.rect[3] + grow]);
                if k <= 1 && i.uv[2] < 0.0 {
                    lo = [lo[0].max(i.rect[0] + i.uv[0]), lo[1].max(i.rect[1] + i.uv[1])];
                    hi = [hi[0].min(i.rect[0] - i.uv[2]).max(lo[0]), hi[1].min(i.rect[1] + i.uv[3]).max(lo[1])];
                }
                let (x0, x1, y0, y1) = cells(lo[0], lo[1], hi[0], hi[1], false);
                let (mut all, mut some) = (x0 < x1 && y0 < y1, 0u32);
                for y in y0..y1 { for x in x0..x1 { if covered[y * cw + x] { some += 1 } else { all = false } } }
                let px = f64::from(CELL * CELL);
                if all { kinds[21].0 += 1; kinds[21].1 += f64::from(some) * px; }
                kinds[22].0 += u32::from(some > 0);
                kinds[22].1 += f64::from(some) * px;
                // 26: the covered cells of rects alone; RUNE_CENSUS_COVER_DUMP: the instances most covered, once.
                if k == 0 { kinds[26].0 += u32::from(some > 0); kinds[26].1 += f64::from(some) * px; }
                if some > 2000 && dump { eprintln!("rune-cover: kind {k} rect {:?} radii {:?} border {} fill {:?} band {} cells {} of {}", i.rect, i.radii, i.params[0], i.fill, i.uv[2] < 0.0, some, (x1.saturating_sub(x0)) * (y1.saturating_sub(y0))); }
                // Opaque rects cover: plain ones whole, rounded or bordered ones their box inset by the largest radius
                // and the border (24: how many covered as rounded).
                let opaque = k == 0 && i.fade == [0; 4] && i.fill[3] >= 1.0 && i.uv[2] >= 0.0;
                if opaque {
                    let inset = i.radii.iter().fold(0f32, |a, r| a.max(r.abs())) + i.params[0].abs();
                    if inset > 0.0 { kinds[24].0 += 1; }
                    let (x0, x1, y0, y1) = { let c = cells((i.rect[0] + inset).max(i.clip[0]), (i.rect[1] + inset).max(i.clip[1]), (i.rect[0] + i.rect[2] - inset).min(i.clip[2]), (i.rect[1] + i.rect[3] - inset).min(i.clip[3]), true); (c.0, c.1, c.2, c.3) };
                    for y in y0..y1 { for x in x0..x1 { covered[y * cw + x] = true } }
                }
            }
        }
        // 25: runs of one kind in paint order (what #1140 splits a frame pass into, before batch boundaries).
        kinds.resize(kinds.len().max(27), (0, 0.0, 0.0));
        kinds[25].0 = instances.chunk_by(|a, b| a.params[1] == b.params[1]).count() as u32;
        *self.kinds.lock().unwrap_or_else(|e| e.into_inner()) = kinds;
    }

    /// Timestamps for one more pass of this frame, labelled `label`, covering `bytes` of target.
    pub(super) fn writes(&self, label: &'static str, bytes: u64) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let mut passes = self.passes.lock().unwrap_or_else(|e| e.into_inner());
        let n = passes.len() as u32;
        if n >= CAP { return None; }
        passes.push((label, bytes));
        Some(wgpu::RenderPassTimestampWrites { query_set: &self.queries, beginning_of_pass_write_index: Some(2 * n), end_of_pass_write_index: Some(2 * n + 1) })
    }

    /// Before the frame's submit: writes out what earlier frames measured, then resolves this
    /// frame's timestamps into a free slot (a frame with no free slot goes unmeasured).
    pub(super) fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        self.collect();
        let mut passes = std::mem::take(&mut *self.passes.lock().unwrap_or_else(|e| e.into_inner()));
        let frame = self.frames.fetch_add(1, Ordering::Relaxed);
        if passes.is_empty() { return; }
        let Some(at) = self.slots.iter().position(|s| !s.busy.load(Ordering::Acquire)) else { return };
        let slot = &self.slots[at];
        // The encoder's end: whatever it records after the last pass (the kept frame's copy into
        // the swap chain) runs before this stamp. Its time is measured from the last pass's end.
        if self.inside && (passes.len() as u32) < CAP {
            let n = passes.len() as u32;
            encoder.write_timestamp(&self.queries, 2 * n);
            encoder.write_timestamp(&self.queries, 2 * n + 1);
            passes.push((AFTER, 0));
        }
        let n = passes.len() as u32;
        encoder.resolve_query_set(&self.queries, 0..2 * n, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &slot.buffer, 0, u64::from(n) * 16);
        *slot.passes.lock().unwrap_or_else(|e| e.into_inner()) = passes;
        *slot.kinds.lock().unwrap_or_else(|e| e.into_inner()) = std::mem::take(&mut *self.kinds.lock().unwrap_or_else(|e| e.into_inner()));
        slot.frame.store(frame, Ordering::Relaxed);
        slot.submitted.store(crate::frame::host_clock::now_ns(), Ordering::Relaxed);
        slot.ready.store(false, Ordering::Release);
        slot.busy.store(true, Ordering::Release);
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(at);
    }

    /// After the frame's submit: maps its slot, which a later frame reads.
    pub(super) fn submitted(&self) {
        let Some(at) = self.pending.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let ready = self.slots[at].ready.clone();
        self.slots[at].buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| if r.is_ok() { ready.store(true, Ordering::Release) });
    }

    fn collect(&self) {
        for slot in &self.slots {
            if !slot.busy.load(Ordering::Acquire) || !slot.ready.load(Ordering::Acquire) { continue; }
            let passes = std::mem::take(&mut *slot.passes.lock().unwrap_or_else(|e| e.into_inner()));
            let kinds: serde_json::Map<String, serde_json::Value> = std::mem::take(&mut *slot.kinds.lock().unwrap_or_else(|e| e.into_inner())).iter().enumerate()
                .filter(|(_, (n, _, _))| *n > 0).map(|(k, (n, raster, shown))| (k.to_string(), serde_json::json!([n, raster.round(), shown.round()]))).collect();
            let ticks: Vec<u64> = {
                let Ok(view) = slot.buffer.slice(..).get_mapped_range() else { continue };
                bytemuck::cast_slice::<u8, u64>(&view[..passes.len() * 16]).to_vec()
            };
            slot.buffer.unmap();
            slot.busy.store(false, Ordering::Release);
            let us = |t: u64| t as f64 * f64::from(self.period) / 1e3;
            let ends = |upto: usize| (0..upto).map(|i| ticks[2 * i + 1]).max().unwrap_or(0);
            let list: Vec<_> = passes.iter().enumerate().map(|(i, (label, bytes))| {
                let (a, b) = if *label == AFTER { (ends(i), ticks[2 * i + 1]) } else { (ticks[2 * i], ticks[2 * i + 1]) };
                serde_json::json!([label, (us(b.saturating_sub(a)) * 10.0).round() / 10.0, bytes])
            }).collect();
            let first = (0..passes.len()).map(|i| ticks[2 * i]).filter(|&t| t != 0).min().unwrap_or(0);
            let last = (0..passes.len()).map(|i| ticks[2 * i + 1]).max().unwrap_or(0);
            let line = serde_json::json!({ "frame": slot.frame.load(Ordering::Relaxed), "t": slot.submitted.load(Ordering::Relaxed), "span_us": (us(last.saturating_sub(first)) * 10.0).round() / 10.0,
                // The GPU clock's own times of the frame's first pass start and last end: the gap before a frame is the GPU's idle time.
                "gpu_start_us": (us(first) * 10.0).round() / 10.0, "gpu_end_us": (us(last) * 10.0).round() / 10.0,
                "kinds": kinds, "passes": passes.len(), "bytes": passes.iter().map(|p| p.1).sum::<u64>(), "by": list });
            let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    }
}

/// The bytes of render target a pass into `view`'s texture covers (all of it: loads and stores
/// are whole-attachment).
pub(super) fn target_bytes(view: &wgpu::TextureView) -> u64 {
    let t = view.texture();
    u64::from(t.width()) * u64::from(t.height()) * u64::from(t.format().block_copy_size(None).unwrap_or(4))
}
