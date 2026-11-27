use super::super::*;
#[cfg(not(target_arch = "wasm32"))]
use super::completion::{CompletionWorker, Parked};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Maximum tracked frames queued in `pending_submit` when a standalone `SurfaceImporter` user
/// never calls `SurfaceImporter::submit`, preventing unbounded retention.
#[cfg(any(target_os = "windows", target_os = "linux"))]
const MAX_PENDING_SUBMIT: usize = 64;

pub(crate) struct Imported {
    pub(super) frame: Arc<Frame>,
    pub(super) textures: [wgpu::Texture; 2],
    /// Whether `textures` come from a bounded pool or decoder ring that may be returned again,
    /// so caching their bind group in `Prepared::binds` avoids per-frame reallocation without
    /// retaining one-shot textures.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(super) reusable: bool,
    /// Keeps a pooled copy target reserved while this `Imported` is live (in `cache`,
    /// `Prepared`, `ImportedSurface`, or an in-flight submission callback).
    pub(super) _lease: Option<Arc<()>>,
}

/// A [`NativeSurface`]'s current frame imported onto a `wgpu::Device` as GPU texture plane(s).
///
/// Cloning is cheap (reference-counted). Holding an `ImportedSurface` keeps any underlying
/// platform mapping or pooled copy slot reserved; when obtained through Rune's compositor
/// (`Frame::surface_texture`, `Renderer::surface_texture`, or [`SurfaceRenderer::surface_texture`]),
/// Rune also retains pooled slots until the frame's GPU submission completes.
#[derive(Clone)]
pub struct ImportedSurface {
    imported: Arc<Imported>,
    mirrored: bool,
    revision: u64,
}

impl std::fmt::Debug for ImportedSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedSurface")
            .field("frame_id", &self.frame_id())
            .field("revision", &self.revision)
            .field("size", &self.size())
            .field("format", &self.format())
            .field("color", &self.color())
            .field("orient", &self.orient())
            .field("mirrored", &self.mirrored)
            .field("planes", &self.planes().len())
            .finish()
    }
}

impl ImportedSurface {
    /// The imported GPU texture plane(s):
    /// - [`Format::Bgra`]: 1 plane (`Bgra8Unorm`, `size()`).
    /// - [`Format::Nv12Video`] / [`Format::Nv12Full`]: 2 planes: plane 0 is luma (`R8Unorm`,
    ///   `size()`) and plane 1 is interleaved chroma (`Rg8Unorm`, half width and height).
    pub fn planes(&self) -> &[wgpu::Texture] {
        match self.imported.frame.format {
            Format::Bgra => &self.imported.textures[..1],
            Format::Nv12Video | Format::Nv12Full => &self.imported.textures[..2],
        }
    }

    /// Plane 0: the sole `Bgra8Unorm` texture for [`Format::Bgra`], or the `R8Unorm` luma (Y)
    /// plane for NV12 formats.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.imported.textures[0]
    }

    /// Plane 1: the `Rg8Unorm` interleaved chroma (CbCr) plane for NV12 formats, or `None` for
    /// [`Format::Bgra`].
    pub fn chroma(&self) -> Option<&wgpu::Texture> {
        match self.imported.frame.format {
            Format::Bgra => None,
            Format::Nv12Video | Format::Nv12Full => Some(&self.imported.textures[1]),
        }
    }

    /// Buffer dimensions `[width, height]` in pixels, before applying [`Self::orient`].
    pub fn size(&self) -> [u32; 2] {
        self.imported.frame.size
    }

    /// Upright dimensions `[width, height]` in pixels after applying [`Self::orient`].
    pub fn upright_size(&self) -> [u32; 2] {
        self.imported.frame.orient.upright(self.imported.frame.size)
    }

    /// Pixel format and range of the imported plane(s).
    pub fn format(&self) -> Format {
        self.imported.frame.format
    }

    /// YCbCr matrix and transfer function for converting the plane(s) to RGB.
    pub fn color(&self) -> ColorInfo {
        self.imported.frame.color
    }

    /// Orientation matrix mapping centred upright coordinates to centred buffer coordinates.
    pub fn orient(&self) -> Orient {
        self.imported.frame.orient
    }

    /// Whether [`NativeSurface::set_mirrored`] requests a horizontal flip when sampling.
    pub fn mirrored(&self) -> bool {
        self.mirrored
    }

    /// Producer-supplied `frame_id` from the push call that published this frame.
    pub fn frame_id(&self) -> u64 {
        self.imported.frame.id
    }

    /// Monotonic revision of the surface's visible state (frame identity and mirror flag).
    /// Changes on every push, clear, or mirror toggle even if a producer reuses `frame_id`.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

struct CachedImport {
    imported: Arc<Imported>,
    /// True while direct [`SurfaceImporter::import`] calls are keeping this entry alive.
    direct: bool,
    /// `frame_gen` when direct [`SurfaceImporter::import`] last touched this entry.
    last_direct_gen: u64,
}

/// Per-device importer and cache turning [`NativeSurface`] frames into `wgpu::Texture` planes.
///
/// One `SurfaceImporter` belongs to one `wgpu::Device` (`Frame::device_key()` in a compositor).
/// Drop and recreate it on device loss (`Compositor::release_gpu`). Inside a Rune compositor,
/// prefer `Frame::surface_texture` or `Renderer::surface_texture`, which reuse Rune's per-device
/// importer and automatically tie pooled copy leases and decoder fences to `Renderer::submit`.
pub struct SurfaceImporter {
    device: wgpu::Device,
    cache: HashMap<usize, CachedImport>,
    frame_gen: AtomicU64,
    last_prune_gen: u64,
    pending_submit: Mutex<Vec<Arc<Imported>>>,
    #[cfg(not(target_arch = "wasm32"))]
    completion: Mutex<Option<CompletionWorker>>,
    #[cfg(target_os = "macos")]
    macos: Option<macos::Importer>,
    #[cfg(target_os = "windows")]
    windows: Option<windows::Importer>,
    #[cfg(target_os = "linux")]
    linux: Option<linux::Importer>,
}

impl Drop for SurfaceImporter {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(worker) = self.completion.get_mut().unwrap_or_else(|e| e.into_inner()).as_mut() {
            worker.stop();
        }
        #[cfg(target_os = "linux")]
        if let Some(dmabuf) = &mut self.linux {
            dmabuf.stop();
        }
    }
}

impl SurfaceImporter {
    /// Creates a lazy per-device importer bound to `device`. No platform caches or GPU resources
    /// are allocated until the first frame is imported.
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            device: device.clone(),
            cache: HashMap::new(),
            frame_gen: AtomicU64::new(0),
            last_prune_gen: 0,
            pending_submit: Mutex::new(Vec::new()),
            #[cfg(not(target_arch = "wasm32"))]
            completion: Mutex::new(None),
            #[cfg(target_os = "macos")]
            macos: None,
            #[cfg(target_os = "windows")]
            windows: None,
            #[cfg(target_os = "linux")]
            linux: None,
        }
    }

    /// Imports `surface`'s current frame onto this importer's device, reusing cached textures
    /// when the frame has not changed.
    ///
    /// Returns `Ok(None)` when `surface` has no frame, `Ok(Some(_))` with the imported plane
    /// texture(s) and metadata, or `Err` if importing failed (also stored on
    /// [`NativeSurface::last_error`]).
    ///
    /// Samples the surface's already-pushed frame mailbox; it does not pull a
    /// [`Player`](crate::Player)-backed surface.
    pub fn import(&mut self, queue: &wgpu::Queue, surface: &NativeSurface) -> Result<Option<ImportedSurface>, String> {
        #[cfg(not(target_arch = "wasm32"))]
        let res = self.import_with(queue, surface, None);
        #[cfg(target_arch = "wasm32")]
        let res = self.import_with(queue, surface);
        res
    }

    /// Drops all cached frame textures and releases cached platform handles and copy pools.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.pending_submit.lock().unwrap_or_else(|e| e.into_inner()).clear();
        #[cfg(not(target_arch = "wasm32"))]
        self.finish_prepare(None);
    }

    /// Submits `encoder` on `queue`, retaining any pooled copy leases until GPU completion,
    /// signaling Windows hardware-decoder release fences, and advancing the frame generation so
    /// surfaces that are no longer sampled are evicted on the next import.
    ///
    /// For macOS `PixelBuffer` and portable `ByteFrame` surfaces, textures are self-retaining and
    /// plain `Queue::submit` is also safe.
    #[cfg(target_arch = "wasm32")]
    pub fn submit(&self, queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        self.advance_gen();
        queue.submit([encoder.finish()])
    }

    /// Submits `encoder` on `queue`, retaining any pooled copy leases until GPU completion,
    /// signaling Windows hardware-decoder release fences, and advancing the frame generation so
    /// surfaces that are no longer sampled are evicted on the next import.
    ///
    /// For macOS `PixelBuffer` and portable `ByteFrame` surfaces, textures are self-retaining and
    /// plain `Queue::submit` is also safe.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn submit(&self, queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        let in_flight = self.take_pending_submit();
        self.submit_in_flight(queue, encoder, in_flight, None)
    }

    /// Evicts cached entries for surfaces that were not sampled during the last submitted frame.
    #[doc(hidden)]
    pub fn prune_stale(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.prune_stale_with(None);
        #[cfg(target_arch = "wasm32")]
        self.prune_stale_inner();
    }

    /// Advances the frame generation after a plain submission that bypassed [`Self::submit`].
    #[doc(hidden)]
    pub fn note_frame_submitted(&self) {
        self.advance_gen();
    }

    /// True when at least one direct-imported frame requires [`Self::submit`] to retain a pooled
    /// copy lease or release a hardware-decoder fence.
    #[doc(hidden)]
    pub fn has_pending_submit(&self) -> bool {
        !self.pending_submit.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    pub(super) fn import_with(
        &mut self,
        queue: &wgpu::Queue,
        surface: &NativeSurface,
        #[cfg(not(target_arch = "wasm32"))] completion: Option<&CompletionWorker>,
    ) -> Result<Option<ImportedSurface>, String> {
        #[cfg(not(target_arch = "wasm32"))]
        self.prune_stale_with(completion);
        #[cfg(target_arch = "wasm32")]
        self.prune_stale_inner();

        let key = surface.key();
        let Some((frame, mirrored, revision)) = surface.latest_snapshot() else {
            if self.cache.remove(&key).is_some() {
                #[cfg(not(target_arch = "wasm32"))]
                self.finish_prepare(completion);
            }
            return Ok(None);
        };

        let current_gen = *self.frame_gen.get_mut();
        if let Some(entry) = self.cache.get_mut(&key)
            && Arc::ptr_eq(&entry.imported.frame, &frame)
        {
            entry.direct = true;
            entry.last_direct_gen = current_gen;
            let imported = entry.imported.clone();
            self.record_pending(&imported);
            return Ok(Some(ImportedSurface { imported, mirrored, revision }));
        }

        match self.import_frame(queue, key, frame) {
            Ok(imported) => {
                let imported = Arc::new(imported);
                self.cache.insert(
                    key,
                    CachedImport { imported: imported.clone(), direct: true, last_direct_gen: current_gen },
                );
                surface.set_error(None);
                self.flush_macos();
                self.record_pending(&imported);
                Ok(Some(ImportedSurface { imported, mirrored, revision }))
            }
            Err(e) => {
                surface.set_error(Some(e.clone()));
                if self.cache.remove(&key).is_some() {
                    #[cfg(not(target_arch = "wasm32"))]
                    self.finish_prepare(completion);
                }
                Err(e)
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn prepare_retain(&mut self, draws: &[SurfaceDraw]) {
        let current_gen = *self.frame_gen.get_mut();
        self.last_prune_gen = current_gen;
        self.cache.retain(|key, entry| {
            if entry.direct && entry.last_direct_gen.saturating_add(1) < current_gen {
                entry.direct = false;
            }
            entry.direct || draws.iter().any(|d| d.surface.key() == *key)
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn import_for_draw(
        &mut self,
        queue: &wgpu::Queue,
        surface: &NativeSurface,
        frame: Arc<Frame>,
    ) -> Result<Arc<Imported>, String> {
        let key = surface.key();
        if let Some(entry) = self.cache.get(&key)
            && Arc::ptr_eq(&entry.imported.frame, &frame)
        {
            return Ok(entry.imported.clone());
        }
        let prev = self.cache.get(&key).map(|e| (e.direct, e.last_direct_gen));
        match self.import_frame(queue, key, frame) {
            Ok(imported) => {
                let imported = Arc::new(imported);
                let (direct, last_direct_gen) = prev.unwrap_or((false, 0));
                self.cache.insert(key, CachedImport { imported: imported.clone(), direct, last_direct_gen });
                surface.set_error(None);
                Ok(imported)
            }
            Err(e) => {
                surface.set_error(Some(e.clone()));
                self.cache.remove(&key);
                Err(e)
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn remove_for_draw(&mut self, key: usize) {
        self.cache.remove(&key);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn finish_prepare(&mut self, completion: Option<&CompletionWorker>) {
        let mut wake = false;
        #[cfg(target_os = "windows")]
        if let Some(shared) = &mut self.windows
            && shared.forget_gone(|key| {
                self.cache.get(&key).is_some_and(|v| matches!(v.imported.frame.source, Source::Shared(_)))
            })
        {
            wake = true;
        }
        #[cfg(target_os = "linux")]
        if let Some(dmabuf) = &mut self.linux
            && dmabuf.forget_gone(|key| {
                self.cache.get(&key).is_some_and(|v| matches!(v.imported.frame.source, Source::Dmabuf(_)))
            })
        {
            wake = true;
        }
        self.flush_macos();
        if wake {
            self.wake_completion(completion);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn take_pending_submit(&self) -> Vec<Arc<Imported>> {
        std::mem::take(&mut *self.pending_submit.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub(super) fn advance_gen(&self) {
        self.frame_gen.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn submit_in_flight(
        &self,
        queue: &wgpu::Queue,
        encoder: wgpu::CommandEncoder,
        in_flight: Vec<Arc<Imported>>,
        completion: Option<&CompletionWorker>,
    ) -> wgpu::SubmissionIndex {
        self.advance_gen();
        if in_flight.is_empty() {
            return queue.submit([encoder.finish()]);
        }
        #[cfg(target_os = "windows")]
        let decoded: Vec<(Arc<windows::DecodedPlanes>, u64)> = in_flight
            .iter()
            .filter_map(|imp| match &imp.frame.source {
                Source::Decoded(planes, value) => Some((planes.clone(), *value)),
                _ => None,
            })
            .collect();
        encoder.on_submitted_work_done(move || drop(in_flight));
        let index = queue.submit([encoder.finish()]);
        #[cfg(target_os = "windows")]
        for (planes, value) in decoded {
            windows::release_decoded(queue, &planes, value);
        }
        self.wake_completion(completion);
        index
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn stop_own_completion(&mut self) {
        if let Some(mut worker) = self.completion.get_mut().unwrap_or_else(|e| e.into_inner()).take() {
            worker.stop();
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn prune_stale_with(&mut self, completion: Option<&CompletionWorker>) {
        let current_gen = *self.frame_gen.get_mut();
        if current_gen == self.last_prune_gen {
            return;
        }
        self.last_prune_gen = current_gen;
        let before = self.cache.len();
        self.cache.retain(|_, entry| {
            !entry.direct || entry.last_direct_gen.saturating_add(1) >= current_gen
        });
        if self.cache.len() < before {
            self.finish_prepare(completion);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn prune_stale_inner(&mut self) {
        let current_gen = *self.frame_gen.get_mut();
        if current_gen == self.last_prune_gen {
            return;
        }
        self.last_prune_gen = current_gen;
        self.cache.retain(|_, entry| {
            !entry.direct || entry.last_direct_gen.saturating_add(1) >= current_gen
        });
    }

    fn flush_macos(&self) {
        #[cfg(target_os = "macos")]
        if let Some(i) = &self.macos {
            i.flush();
        }
    }

    fn record_pending(&self, imported: &Arc<Imported>) {
        if !needs_submit_tracking(imported) {
            return;
        }
        let mut pending = self.pending_submit.lock().unwrap_or_else(|e| e.into_inner());
        if pending.iter().any(|existing| Arc::ptr_eq(&existing.frame, &imported.frame)) {
            return;
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if pending.len() >= MAX_PENDING_SUBMIT {
            pending.remove(0);
        }
        pending.push(imported.clone());
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn wake_completion(&self, external: Option<&CompletionWorker>) {
        if let Some(worker) = external {
            worker.wake();
            return;
        }
        let mut slot = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        let worker = slot.get_or_insert_with(|| {
            CompletionWorker::spawn(self.device.clone(), Arc::new(Parked::default()))
        });
        worker.wake();
    }

    fn import_frame(&mut self, queue: &wgpu::Queue, surface: usize, frame: Arc<Frame>) -> Result<Imported, String> {
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        let _ = surface;
        let device = &self.device;
        let (textures, reusable, lease) = match &frame.source {
            // Checks the size itself, so every failure reaches the producer.
            #[cfg(target_os = "linux")]
            Source::Dmabuf(d) => {
                let (textures, reusable) = linux::import(&mut self.linux, device, queue, surface, d)?;
                (textures, reusable, None)
            }
            _ if frame.size.iter().any(|v| *v > device.limits().max_texture_dimension_2d) => {
                return Err("video dimensions exceed this GPU device's texture limit".into());
            }
            Source::Bytes(b) => {
                let a = upload(
                    device,
                    queue,
                    b.width,
                    b.height,
                    if b.format == Format::Bgra {
                        wgpu::TextureFormat::Bgra8Unorm
                    } else {
                        wgpu::TextureFormat::R8Unorm
                    },
                    &b.planes[0],
                );
                let b = if b.format == Format::Bgra {
                    a.clone()
                } else {
                    upload(device, queue, b.width / 2, b.height / 2, wgpu::TextureFormat::Rg8Unorm, &b.planes[1])
                };
                ([a, b], false, None)
            }
            #[cfg(target_os = "macos")]
            Source::PixelBuffer(b) => {
                if self.macos.is_none() {
                    self.macos = Some(macos::Importer::new(device)?);
                }
                let textures = objc2::rc::autoreleasepool(|_| {
                    self.macos.as_ref().unwrap().import(b, frame.format, frame.size)
                })?;
                (textures, false, None)
            }
            #[cfg(target_os = "windows")]
            Source::Shared(t) => {
                if self.windows.is_none() {
                    self.windows = Some(windows::Importer::new(device)?);
                }
                let (textures, lease) = self.windows.as_mut().unwrap().import(queue, surface, t)?;
                let reusable = lease.is_some();
                (textures, reusable, lease)
            }
            #[cfg(target_os = "windows")]
            Source::Decoded(planes, value) => {
                if self.windows.is_none() {
                    self.windows = Some(windows::Importer::new(device)?);
                }
                (self.windows.as_mut().unwrap().import_decoded(queue, planes, *value)?, true, None)
            }
        };
        Ok(Imported { frame, textures, reusable, _lease: lease })
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn live_planes(&self) -> usize {
        self.macos.as_ref().map_or(0, |i| i.live_planes.load(Ordering::SeqCst))
    }

    #[cfg(all(test, target_os = "windows"))]
    pub(crate) fn windows_importer(&self) -> Option<&windows::Importer> {
        self.windows.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn cached_len(&self) -> usize {
        self.cache.len()
    }
}

fn needs_submit_tracking(imported: &Imported) -> bool {
    if imported._lease.is_some() {
        return true;
    }
    #[cfg(target_os = "windows")]
    if matches!(imported.frame.source, Source::Decoded(..)) {
        return true;
    }
    #[cfg(target_os = "linux")]
    if matches!(imported.frame.source, Source::Dmabuf(..)) {
        return true;
    }
    false
}

fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    bytes: &[u8],
) -> wgpu::Texture {
    let mut desc = super::descriptor(width, height, format);
    desc.usage |= wgpu::TextureUsages::COPY_DST;
    let texture = device.create_texture(&desc);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * format.block_copy_size(None).unwrap()),
            rows_per_image: None,
        },
        desc.size,
    );
    texture
}
