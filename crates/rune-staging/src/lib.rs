//! Per-frame GPU uploads through reused staging memory.
//!
//! On native backends every `Queue::write_buffer` and `Queue::write_texture` allocates a fresh
//! staging buffer and frees it once the GPU has copied out of it. `Staging` writes into mapped
//! chunks instead and records the copies itself: into an encoder at `flush`, or in a submission
//! of its own at `submit`. A chunk comes back for reuse once the GPU is done with it, so a steady
//! animating frame allocates no buffers.
//!
//! Writes land in the order they were made, at the next `flush` or `submit`. A write must be
//! flushed into an encoder ahead of the commands that read it, or submitted before them.
//! On the web, whose `writeBuffer` already stages, writes go straight to the queue. A backend
//! whose buffers or textures the CPU can write in place registers a `Direct` / `DirectTexture`
//! writer, and writes it takes skip the staging.

mod buffer;
pub mod health;
mod oversized;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

pub use buffer::GrowBuffer;
pub use buffer::Readback;
pub use buffer::padded_row_bytes;
pub use buffer::unpad_rows;

/// Writes `data` to `buffer` at `offset` in place, for the commands recorded after it, or
/// returns false to have it staged. Commands recorded before it keep reading what they would
/// have, so the write lands no later than a flush would land it.
pub type Direct = fn(&wgpu::Buffer, u64, &[u8]) -> bool;

static DIRECT: OnceLock<Direct> = OnceLock::new();

/// Registers the process's `Direct` writer. The first one stays.
pub fn set_direct(direct: Direct) {
    DIRECT.set(direct).ok();
}

/// Writes tightly packed full-width rows of `texture` starting at row `y` in place, for the
/// commands recorded after it, or returns false to have them staged. Commands recorded before it
/// keep reading what they would have, so the write lands no later than a flush would land it.
pub type DirectTexture = fn(&wgpu::Texture, u32, &[u8]) -> bool;

static DIRECT_TEXTURE: OnceLock<DirectTexture> = OnceLock::new();

/// Registers the process's `DirectTexture` writer. The first one stays.
pub fn set_direct_texture(direct: DirectTexture) {
    DIRECT_TEXTURE.set(direct).ok();
}

/// The size of a new chunk; a larger write up to `KEEP_CHUNK_MAX` gets its next power of two.
const CHUNK: u64 = 128 * 1024;
/// Returned normal chunks kept for reuse, by count and in bytes.
const KEEP_CHUNKS: usize = 8;
const KEEP_BYTES: u64 = 4 << 20;
const KEEP_CHUNK_MAX: u64 = KEEP_BYTES;
/// Staged bytes past which a write first submits what is pending, so a caller that never
/// flushes can't grow the chunks without bound.
const MAX_PENDING: u64 = 16 << 20;
/// Staged texture rows start at this alignment, and `bytes_per_row` is a multiple of it.
const ROW_ALIGN: u64 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64;

/// A shared handle to one set of staging chunks and the writes pending in them: the chunks,
/// the count of lost ones, whether any are pending or mapped (so `flush` and `submit` with
/// nothing to do never lock), whether the oversized pool is active, and how many recyclable
/// oversized chunks are in flight on the GPU.
#[derive(Clone)]
pub struct Staging {
    inner: Arc<Mutex<Inner>>,
    lost: Arc<AtomicU64>,
    busy: Arc<AtomicBool>,
    oversized_active: Arc<AtomicBool>,
    oversized_in_flight: Arc<AtomicUsize>,
}

/// Counters, for tests and benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stats {
    /// Chunks created, each one buffer allocation.
    pub chunks: u64,
    /// Writes staged.
    pub writes: u64,
    /// Bytes staged, row padding included.
    pub bytes: u64,
    /// Pending writes recorded into a caller's encoder.
    pub flushes: u64,
    /// Pending writes submitted on their own.
    pub submits: u64,
    /// Writes made in place by the `Direct` writer, counted in `writes` and `bytes` too.
    pub direct: u64,
}

struct Chunk {
    buffer: wgpu::Buffer,
    used: u64,
    /// The whole chunk's mapping, made at its first write and dropped before it unmaps, so a
    /// write costs wgpu no mapped-range bookkeeping of its own.
    view: Option<wgpu::BufferViewMut>,
    /// False for one-shot oversized chunks, which unmap and drop on encode without remapping.
    recyclable: bool,
}

enum Dst {
    Buffer(wgpu::Buffer, u64),
    /// Full-width texture rows from `y`, `rows` of them, `bytes_per_row` apart in the chunk.
    Rows { texture: wgpu::Texture, y: u32, rows: u32, bytes_per_row: u32 },
}

struct Pending {
    src: wgpu::Buffer,
    offset: u64,
    size: u64,
    dst: Dst,
}

struct Inner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Mapped chunks holding pending writes.
    active: Vec<Chunk>,
    free: Vec<Chunk>,
    oversized_free: VecDeque<Chunk>,
    oversized: oversized::State,
    recycle: mpsc::Sender<Chunk>,
    returned: mpsc::Receiver<Chunk>,
    pending: Vec<Pending>,
    pending_bytes: u64,
    stats: Stats,
}

/// Hands a chunk back when the GPU has finished copying out of it. Dropped without that (its
/// encoder was dropped unsubmitted, or the device was lost), it counts a lost chunk.
struct Return {
    chunk: Option<Chunk>,
    recycle: mpsc::Sender<Chunk>,
    lost: Arc<AtomicU64>,
    in_flight: Option<Arc<AtomicUsize>>,
}

impl Drop for Return {
    fn drop(&mut self) {
        if let Some(in_flight) = &self.in_flight {
            in_flight.fetch_sub(1, Ordering::AcqRel);
        }
        if self.chunk.is_some() { self.lost.fetch_add(1, Ordering::Relaxed); }
    }
}

/// Zero-buffer lost-chunk guard attached to `encoder.on_submitted_work_done` for one-shot
/// oversized chunks, preserving `Staging::lost()` without retaining or remapping the buffer.
struct LostGuard {
    ran: bool,
    lost: Arc<AtomicU64>,
}

impl LostGuard {
    fn complete(&mut self) {
        self.ran = true;
    }
}

impl Drop for LostGuard {
    fn drop(&mut self) {
        if !self.ran { self.lost.fetch_add(1, Ordering::Relaxed); }
    }
}

impl Staging {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Staging {
        let (recycle, returned) = mpsc::channel();
        Staging {
            inner: Arc::new(Mutex::new(Inner {
                device: device.clone(), queue: queue.clone(), active: Vec::new(), free: Vec::new(),
                oversized_free: VecDeque::new(), oversized: oversized::State::Cold, recycle, returned,
                pending: Vec::new(), pending_bytes: 0, stats: Stats::default(),
            })),
            lost: Arc::default(),
            busy: Arc::default(),
            oversized_active: Arc::default(),
            oversized_in_flight: Arc::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stages a write of `data` to `buffer` at `offset`. Both are multiples of 4.
    pub fn write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        if data.is_empty() { return; }
        if let Some(direct) = DIRECT.get() && direct(buffer, offset, data) {
            let mut s = self.lock();
            s.stats.direct += 1;
            s.stats.writes += 1;
            s.stats.bytes += data.len() as u64;
            return;
        }
        let mut s = self.lock();
        if cfg!(target_arch = "wasm32") { return s.queue.write_buffer(buffer, offset, data); }
        let size = data.len() as u64;
        let (src, at) = s.stage(size, wgpu::MAP_ALIGNMENT, self, |view| view.slice(..data.len()).copy_from_slice(data));
        s.pending.push(Pending { src, offset: at, size, dst: Dst::Buffer(buffer.clone(), offset) });
    }

    /// Stages a write of full-width rows of `texture` from row `y`. `rows` holds them tightly
    /// packed; its length is a whole number of rows.
    pub fn write_texture_rows(&self, texture: &wgpu::Texture, y: u32, rows: &[u8]) {
        let row = texture.width() * texture.format().block_copy_size(None).expect("a color format");
        let count = (rows.len() / row as usize) as u32;
        if count == 0 { return; }
        let byte_len = row as usize * count as usize;
        if let Some(direct) = DIRECT_TEXTURE.get() {
            let has_pending = self.busy() && self.lock().has_pending_texture(texture);
            if !has_pending && direct(texture, y, &rows[..byte_len]) {
                let mut s = self.lock();
                s.stats.direct += 1;
                s.stats.writes += 1;
                s.stats.bytes += byte_len as u64;
                return;
            }
        }
        let mut s = self.lock();
        if cfg!(target_arch = "wasm32") {
            return s.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x: 0, y, z: 0 }, aspect: wgpu::TextureAspect::All },
                rows,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
                wgpu::Extent3d { width: texture.width(), height: count, depth_or_array_layers: 1 },
            );
        }
        let padded = u64::from(row).next_multiple_of(ROW_ALIGN);
        let size = padded * u64::from(count);
        let (src, at) = s.stage(size, ROW_ALIGN, self, |view| {
            if padded == u64::from(row) { return view.copy_from_slice(&rows[..size as usize]); }
            for (r, line) in rows.chunks_exact(row as usize).enumerate() {
                let start = r * padded as usize;
                view.slice(start..start + line.len()).copy_from_slice(line);
            }
        });
        s.pending.push(Pending { src, offset: at, size,
            dst: Dst::Rows { texture: texture.clone(), y, rows: count, bytes_per_row: padded as u32 } });
    }

    /// Records the pending writes into `encoder` here, ahead of what is recorded after them.
    /// `encoder` must be submitted: dropped, the writes never land (`lost` counts it).
    pub fn flush(&self, encoder: &mut wgpu::CommandEncoder) {
        let busy = self.busy();
        if !busy && !self.oversized_active.load(Ordering::Acquire) { return; }
        let mut s = self.lock();
        if s.pending.is_empty() && s.active.is_empty() {
            s.step_oversized(self, oversized::Event::Flush { had_oversized: false });
            return;
        }
        s.encode(encoder, self);
        s.stats.flushes += 1;
    }

    /// Records the pending copies into `encoder` but leaves their chunks mapped, so later writes
    /// share them: one chunk unmapped and remapped per frame instead of one per write. The next
    /// `flush` unmaps them and must be into the same encoder (or one submitted after it); a
    /// `submit` before then would remap them before `encoder` copies out of them.
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.busy() { return; }
        let mut s = self.lock();
        if s.pending.is_empty() { return; }
        s.copy(encoder);
        s.stats.flushes += 1;
    }

    /// Submits the pending writes now, ahead of anything submitted later.
    pub fn submit(&self) {
        if !self.busy() { return; }
        self.lock().submit(self);
    }

    /// Whether writes are pending or chunks still mapped since the last `flush` or `submit`.
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Whether writes are waiting for a `flush` or `submit`.
    pub fn is_pending(&self) -> bool {
        !self.lock().pending.is_empty()
    }

    /// Whether any recyclable oversized (> 4 MiB) staging chunks are currently in flight on the
    /// GPU awaiting return via `map_buffer_on_submit`.
    pub fn has_oversized_in_flight(&self) -> bool {
        self.oversized_in_flight.load(Ordering::Acquire) > 0
    }

    /// Chunks whose writes may never have landed because their encoder was dropped unsubmitted.
    /// A caller that skips writes matching what it last wrote resends everything when it grows.
    pub fn lost(&self) -> u64 {
        self.lost.load(Ordering::Relaxed)
    }

    pub fn stats(&self) -> Stats {
        self.lock().stats
    }

    /// Frees the chunks kept for reuse, those the GPU has handed back included; the next write
    /// makes a new one. Chunks holding pending writes stay.
    pub fn trim(&self) {
        let mut s = self.lock();
        s.step_oversized(self, oversized::Event::Trim);
        while s.returned.try_recv().is_ok() {}
        s.free.clear();
        s.oversized_free.clear();
    }

    /// Frees only retained oversized (> 4 MiB) chunks and resets the oversized pool, keeping
    /// normal <= 4 MiB chunks intact for the next frame.
    pub fn trim_oversized(&self) {
        let mut s = self.lock();
        s.step_oversized(self, oversized::Event::Trim);
        s.oversized_free.clear();
        // Drain returned chunks: normal <= 4 MiB chunks move into `free`; oversized chunks are
        // dropped because the pool is now `Cold`.
        s.receive();
    }
}

impl Inner {
    fn has_pending_texture(&self, texture: &wgpu::Texture) -> bool {
        self.pending.iter().any(|p| matches!(&p.dst, Dst::Rows { texture: t, .. } if t == texture))
    }

    fn oversized_bytes(&self) -> u64 {
        self.oversized_free.iter().map(|c| c.buffer.size()).sum()
    }

    fn step_oversized(&mut self, staging: &Staging, event: oversized::Event) -> Option<oversized::Effect> {
        let effect = oversized::step(&mut self.oversized, event);
        staging.oversized_active.store(self.oversized.is_active(), Ordering::Release);
        if matches!(effect, Some(oversized::Effect::Evict)) {
            self.oversized_free.clear();
        }
        effect
    }

    /// Reserves `size` bytes (rounded up to 4) at `align` in a mapped chunk and fills them.
    fn stage(&mut self, size: u64, align: u64, staging: &Staging, fill: impl FnOnce(&mut wgpu::WriteOnly<'_, [u8]>)) -> (wgpu::Buffer, u64) {
        let size = size.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        if self.pending_bytes + size > MAX_PENDING { self.submit(staging); }
        let is_oversized = size > KEEP_CHUNK_MAX;
        let recyclable = if is_oversized {
            let poolable = oversized::alloc_size(size) <= oversized::OVERSIZED_KEEP_BYTES;
            matches!(
                self.step_oversized(staging, oversized::Event::OversizedWrite { poolable }),
                Some(oversized::Effect::Chunk(oversized::Disposition::Recycle))
            )
        } else {
            true
        };
        let fits = |c: &Chunk| {
            (c.buffer.size() > KEEP_CHUNK_MAX) == is_oversized
                && c.used.next_multiple_of(align) + size <= c.buffer.size()
        };
        let index = match self.active.iter().position(fits) {
            Some(i) => i,
            None => {
                self.receive();
                let chunk = if is_oversized {
                    let reused = recyclable.then(|| {
                        self.oversized_free.iter().enumerate()
                            .filter(|(_, c)| oversized::fits_reusable(c.buffer.size(), size))
                            .min_by_key(|(_, c)| c.buffer.size())
                            .map(|(i, _)| i)
                    }).flatten();
                    match reused {
                        Some(i) => {
                            let mut c = self.oversized_free.remove(i).expect("matched oversized index");
                            c.recyclable = true;
                            c
                        }
                        None => {
                            self.stats.chunks += 1;
                            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                                label: Some("rune staging oversized"),
                                size: oversized::alloc_size(size),
                                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                                mapped_at_creation: true,
                            });
                            Chunk { buffer, used: 0, view: None, recyclable }
                        }
                    }
                } else {
                    match self.free.iter().enumerate()
                        .filter(|(_, c)| c.buffer.size() >= size)
                        .min_by_key(|(_, c)| c.buffer.size())
                        .map(|(i, _)| i)
                    {
                        Some(i) => self.free.swap_remove(i),
                        None => {
                            self.stats.chunks += 1;
                            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                                label: Some("rune staging"),
                                size: CHUNK.max(size.next_power_of_two()),
                                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                                mapped_at_creation: true,
                            });
                            Chunk { buffer, used: 0, view: None, recyclable: true }
                        }
                    }
                };
                self.active.push(chunk);
                self.active.len() - 1
            }
        };
        let chunk = &mut self.active[index];
        let at = chunk.used.next_multiple_of(align);
        chunk.used = at + size;
        let view = match chunk.view.take() {
            Some(view) => chunk.view.insert(view),
            None => match chunk.buffer.get_mapped_range_mut(..) {
                Ok(view) => chunk.view.insert(view),
                // A lost device's new chunk can't map. Its frame draws nothing and the renderer
                // is made again on a new device, so the write has nowhere to go.
                Err(_) => return (chunk.buffer.clone(), at),
            },
        };
        fill(&mut view.slice(at as usize..(at + size) as usize));
        self.pending_bytes += size;
        self.stats.writes += 1;
        self.stats.bytes += size;
        staging.busy.store(true, Ordering::Release);
        (chunk.buffer.clone(), at)
    }

    /// Moves chunks the GPU is done with to the free lists, within their keep bounds.
    fn receive(&mut self) {
        while let Ok(mut chunk) = self.returned.try_recv() {
            chunk.used = 0;
            let size = chunk.buffer.size();
            if size <= KEEP_CHUNK_MAX {
                let kept: u64 = self.free.iter().map(|c| c.buffer.size()).sum();
                if self.free.len() < KEEP_CHUNKS && kept + size <= KEEP_BYTES {
                    self.free.push(chunk);
                }
            } else if size <= oversized::OVERSIZED_KEEP_BYTES && self.oversized.accepts_returns() {
                while self.oversized_free.len() >= oversized::OVERSIZED_KEEP_CHUNKS
                    || self.oversized_bytes() + size > oversized::OVERSIZED_KEEP_BYTES
                {
                    if self.oversized_free.pop_front().is_none() { break; }
                }
                self.oversized_free.push_back(chunk);
            }
        }
    }

    fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, staging: &Staging) {
        self.copy(encoder);
        let mut had_oversized = false;
        for mut chunk in self.active.drain(..) {
            let is_oversized = chunk.buffer.size() > KEEP_CHUNK_MAX;
            had_oversized |= is_oversized;
            chunk.view = None;
            chunk.buffer.unmap();
            if !chunk.recyclable {
                drop(chunk);
                let mut guard = LostGuard { ran: false, lost: staging.lost.clone() };
                encoder.on_submitted_work_done(move || guard.complete());
                continue;
            }
            let in_flight = is_oversized.then(|| {
                staging.oversized_in_flight.fetch_add(1, Ordering::AcqRel);
                staging.oversized_in_flight.clone()
            });
            let buffer = chunk.buffer.clone();
            let mut back = Return { chunk: Some(chunk), recycle: self.recycle.clone(), lost: staging.lost.clone(), in_flight };
            encoder.map_buffer_on_submit(&buffer, wgpu::MapMode::Write, .., move |mapped| {
                if mapped.is_ok() && let Some(chunk) = back.chunk.take() { back.recycle.send(chunk).ok(); }
            });
        }
        staging.busy.store(false, Ordering::Release);
        self.step_oversized(staging, oversized::Event::Flush { had_oversized });
    }

    /// Records the pending copies; their chunks stay mapped and active.
    fn copy(&mut self, encoder: &mut wgpu::CommandEncoder) {
        for p in self.pending.drain(..) {
            match p.dst {
                Dst::Buffer(buffer, offset) => encoder.copy_buffer_to_buffer(&p.src, p.offset, &buffer, offset, p.size),
                Dst::Rows { texture, y, rows, bytes_per_row } => encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo { buffer: &p.src, layout: wgpu::TexelCopyBufferLayout {
                        offset: p.offset, bytes_per_row: Some(bytes_per_row), rows_per_image: None } },
                    wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d { x: 0, y, z: 0 }, aspect: wgpu::TextureAspect::All },
                    wgpu::Extent3d { width: texture.width(), height: rows, depth_or_array_layers: 1 },
                ),
            }
        }
        self.pending_bytes = 0;
    }

    fn submit(&mut self, staging: &Staging) {
        if self.pending.is_empty() { return; }
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("rune uploads") });
        self.encode(&mut encoder, staging);
        self.queue.submit([encoder.finish()]);
        self.stats.submits += 1;
    }
}
