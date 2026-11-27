//! D3D shared textures from another process or API, read on Rune's D3D12 device.
//!
//! The producer renders BGRA8 frames into textures it shares by name (a D3D11 texture made with
//! `D3D11_RESOURCE_MISC_SHARED | D3D11_RESOURCE_MISC_SHARED_NTHANDLE`, then
//! `IDXGIResource1::CreateSharedHandle`, or a D3D12 one with `CreateSharedHandle`), on the adapter
//! from [`adapter_luid`]. Rune copies a pushed frame into a texture of its own the first time it
//! draws it, so the shared texture is read once, in one short submission. When the frame names a
//! release fence, Rune signals it to the frame's value on its GPU queue after that copy; the
//! producer may write the texture again once the fence reaches the value. A frame replaced before
//! it was drawn is never read and signals nothing: the next frame's higher value covers it.

#![allow(unsafe_code)]

use super::*;
use ::windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE};
use ::windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_DIMENSION_TEXTURE2D, ID3D12Device, ID3D12Fence, ID3D12Resource};
use ::windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use ::windows::core::{HSTRING, Interface};
use std::sync::atomic::{AtomicU64, Ordering};
use wgpu::hal::api::Dx12;

/// How many opened textures and fences an importer keeps across active surfaces.
const OPENED: usize = 16;
/// Imports on one surface without touching a cached shared texture before it is considered
/// abandoned (e.g. a producer recreated handles at the same dimensions). Matches `OPENED` so
/// producers cycling up to 16 ring textures stay cached without reopening every frame.
const STALE_IMPORTS: u64 = OPENED as u64;
/// Owned copy targets kept per active surface: enough to avoid stalling against in-flight
/// swapchain frames while capping VRAM well below `KEPT_BINDS`.
const COPY_SLOTS: usize = 3;

/// A frame in a named D3D shared texture, BGRA8 (`DXGI_FORMAT_B8G8R8A8_UNORM`), complete before
/// it is pushed. Build one with [`named`](Self::named).
#[derive(Clone, Debug)]
pub struct SharedTexture {
    name: String,
    size: [u32; 2],
    release: Option<(String, u64)>,
}

impl SharedTexture {
    /// The texture shared under `name`, `width` × `height` pixels.
    ///
    /// ```
    /// use rune_media::native_surface::windows::SharedTexture;
    /// let frame = SharedTexture::named("my-app-frame-0", 1280, 720).release_fence("my-app-read", 42);
    /// # let _ = frame;
    /// // surface.push_shared(frame, 42)?;
    /// ```
    pub fn named(name: impl Into<String>, width: u32, height: u32) -> Self {
        Self { name: name.into(), size: [width, height], release: None }
    }

    /// Signal the fence shared under `name` (`ID3D11Fence` or `ID3D12Fence`, made with a shared
    /// flag and shared by name) to `value` once Rune has read the texture. Values must grow from
    /// frame to frame.
    pub fn release_fence(mut self, name: impl Into<String>, value: u64) -> Self {
        self.release = Some((name.into(), value));
        self
    }

    pub(crate) fn size(&self) -> [u32; 2] {
        self.size
    }
}

/// One NV12 picture Rune's video decoder (`video::windows`) wrote into a shared texture it opened
/// on Rune's D3D12 device. Rune samples its two planes in place (`with_plane_slice`): no copy.
pub(crate) struct DecodedPlanes {
    pub(crate) resource: ID3D12Resource,
    pub(crate) size: [u32; 2],
    pub(crate) fences: Arc<VideoFences>,
    /// The raw `ID3D12Device` the resource and fences were opened on.
    pub(crate) device: usize,
}

/// A decoder's two fences on Rune's D3D12 device: it signals `ready` once a picture is written;
/// Rune waits on it before drawing the picture and signals `release` with the same value after
/// the submission that drew it, when the decoder may write that texture again.
pub(crate) struct VideoFences {
    pub(crate) ready: ID3D12Fence,
    pub(crate) release: ID3D12Fence,
}

/// The D3D12 device of the last window Rune made with D3D12, which video decoders open their
/// textures on.
static DEVICE: Mutex<Option<ID3D12Device>> = Mutex::new(None);

pub(crate) fn presenting_device() -> Option<ID3D12Device> {
    DEVICE.lock().unwrap().clone()
}

/// Forgets the last window's device once it is removed. Decoders opening a clip then wait for
/// the next one (`presenting_device` is None until a window notes it).
pub(crate) fn release_removed_device() {
    let mut device = DEVICE.lock().unwrap();
    // SAFETY: a plain query on a device this slot holds.
    if device.as_ref().is_some_and(|d| unsafe { d.GetDeviceRemovedReason() }.is_err()) {
        *device = None;
    }
}

/// Counts the windows' D3D12 devices Rune made: it moves when one replaces a lost one, so what
/// was opened on the old device (a decoder's textures and fences) is opened again.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn device_generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

static ADAPTER_LUID: AtomicU64 = AtomicU64::new(0);

/// The LUID (`HighPart`, `LowPart`) of the adapter Rune last made a window's device on, once it
/// has made one with D3D12: a producer of shared textures renders on it so Rune can open them.
/// Chromium takes it as `--use-adapter-luid=HIGH,LOW`.
pub fn adapter_luid() -> Option<(i32, u32)> {
    let luid = ADAPTER_LUID.load(Ordering::Relaxed);
    (luid != 0).then_some(((luid >> 32) as i32, luid as u32))
}

/// Remembers `device`'s adapter for [`adapter_luid`]; nothing off D3D12.
pub(crate) fn note_adapter(device: &wgpu::Device) {
    // SAFETY: the raw device is only asked for its LUID.
    let Some(hal) = (unsafe { device.as_hal::<Dx12>() }) else { return };
    // SAFETY: a plain query on the live device.
    let luid = unsafe { hal.raw_device().GetAdapterLuid() };
    ADAPTER_LUID.store((u64::from(luid.HighPart as u32) << 32) | u64::from(luid.LowPart), Ordering::Relaxed);
    *DEVICE.lock().unwrap() = Some(hal.raw_device().clone());
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

struct CachedTexture {
    surface: usize,
    name: String,
    size: [u32; 2],
    last_import: u64,
    resource: ID3D12Resource,
}

struct CachedFence {
    surface: usize,
    name: String,
    fence: ID3D12Fence,
    /// Whether `ID3D12CommandQueue::Signal` has been called on this fence.
    signaled: bool,
}

/// `ID3D12Fence` kept alive in a `wgpu` completion callback until the GPU has executed every
/// queued `ID3D12CommandQueue::Signal` referencing it.
struct RetainedFence {
    _fence: ID3D12Fence,
    #[cfg(test)]
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
}

// SAFETY: D3D12 COM objects (`ID3D12Fence`) are free-threaded and `IUnknown::Release` is
// thread-safe per the D3D12 specification.
unsafe impl Send for RetainedFence {}

#[cfg(test)]
impl Drop for RetainedFence {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

struct CopySlot {
    texture: wgpu::Texture,
    /// Weak handle to the `Imported` frame currently using this slot; `strong_count() == 0`
    /// means no live cache entry, prepared draw, or in-flight submission still uses it.
    in_use: std::sync::Weak<()>,
}

struct CopyRing {
    surface: usize,
    size: [u32; 2],
    slots: Vec<CopySlot>,
}

pub(crate) struct Importer {
    device: wgpu::Device,
    /// Queue used for shared copies and raw `ID3D12CommandQueue::Signal` calls, kept so
    /// evicted or dropped signaled fences can be retired behind an empty wgpu submission.
    queue: Option<wgpu::Queue>,
    textures: Vec<CachedTexture>,
    fences: Vec<CachedFence>,
    /// Per-surface monotonic import counter (keyed by `NativeSurface::key()`).
    import_counts: Vec<(usize, u64)>,
    /// Per-surface ring of owned copy targets, sized lazily up to `COPY_SLOTS`.
    copies: Vec<CopyRing>,
    /// Decoded pictures' plane wrappers, by resource: a decoder cycles a few textures. Each goes
    /// once its decoder and every picture in it are gone.
    planes: Vec<(usize, std::sync::Weak<DecodedPlanes>, [wgpu::Texture; 2])>,
    /// Set when a retirement submission or resize drop happened during `import` so the caller
    /// wakes `CompletionWorker` at the end of `prepare`.
    needs_poll: bool,
    #[cfg(test)]
    in_flight_fences: Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for Importer {
    fn drop(&mut self) {
        let fences = std::mem::take(&mut self.fences);
        let retired: Vec<RetainedFence> = fences
            .into_iter()
            .filter(|e| e.signaled)
            .map(|e| self.wrap_retained(e.fence))
            .collect();
        retire_fences_on(&self.device, self.queue.as_ref(), retired);
    }
}

impl Importer {
    pub fn new(device: &wgpu::Device) -> Result<Self, String> {
        // SAFETY: only checks the backend.
        if unsafe { device.as_hal::<Dx12>() }.is_none() {
            return Err("shared textures need wgpu's D3D12 backend".into());
        }
        Ok(Self {
            device: device.clone(),
            queue: None,
            textures: Vec::new(),
            fences: Vec::new(),
            import_counts: Vec::new(),
            copies: Vec::new(),
            planes: Vec::new(),
            needs_poll: false,
            #[cfg(test)]
            in_flight_fences: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })
    }

    /// The two planes of a decoded picture as R8 and RG8 textures over its own resource, once the
    /// decoder's write of `value` is done on the GPU. The wrappers start and stay in the shader
    /// resource state as far as wgpu knows: a shared resource leaves every submission in COMMON
    /// and is promoted to a read state implicitly, so wgpu never transitions it.
    pub fn import_decoded(&mut self, queue: &wgpu::Queue, planes: &Arc<DecodedPlanes>, value: u64) -> Result<[wgpu::Texture; 2], String> {
        // SAFETY: only the raw device is compared.
        let hal = unsafe { self.device.as_hal::<Dx12>() }.ok_or("decoded video needs wgpu's D3D12 backend")?;
        if hal.raw_device().as_raw() as usize != planes.device {
            return Err("the video was decoded for another GPU device".into());
        }
        drop(hal);
        let key = planes.resource.as_raw() as usize;
        let textures = match self.planes.iter().find(|(k, live, _)| *k == key && live.strong_count() > 0) {
            Some((_, _, textures)) => textures.clone(),
            None => {
                let [w, h] = planes.size;
                let wrap = |format, width, height, plane| {
                    let desc = pipeline::descriptor(width, height, format);
                    // SAFETY: the resource is a 2D, single-mip NV12 texture of `planes.size` on this
                    // device; plane 0 is `w`x`h` R8, plane 1 is half that in RG8.
                    let hal = unsafe {
                        wgpu::hal::dx12::Device::texture_from_raw(planes.resource.clone(), format, desc.dimension, desc.size, 1, 1)
                    }
                    .with_plane_slice(plane);
                    // SAFETY: as above; it is only ever sampled.
                    unsafe { self.device.create_texture_from_hal::<Dx12>(hal, &desc, wgpu::TextureUses::RESOURCE) }
                };
                let textures = [wrap(wgpu::TextureFormat::R8Unorm, w, h, 0), wrap(wgpu::TextureFormat::Rg8Unorm, w / 2, h / 2, 1)];
                self.forget_gone_planes();
                self.planes.push((key, Arc::downgrade(planes), textures.clone()));
                textures
            }
        };
        // SAFETY: the raw queue only gets a wait, ordered before the work that samples the planes.
        let raw = unsafe { queue.as_hal::<Dx12>() }.ok_or("decoded video needs wgpu's D3D12 backend")?;
        // SAFETY: as above; the fence is live and opened on this device.
        unsafe { raw.as_raw().Wait(&planes.fences.ready, value) }.map_err(|e| format!("wait for the decoder: {e}"))?;
        Ok(textures)
    }

    /// Copies `frame` into a pooled texture of Rune's own, then signals its release fence.
    /// Returns the copied textures and an optional pool lease (None on overflow one-shot copy).
    pub fn import(&mut self, queue: &wgpu::Queue, surface: usize, frame: &SharedTexture) -> Result<([wgpu::Texture; 2], Option<Arc<()>>), String> {
        self.queue = Some(queue.clone());
        let import_gen = self.next_import_gen(surface);
        let fence = match &frame.release {
            Some((name, value)) => Some((self.fence(surface, name)?, name.as_str(), *value)),
            None => {
                let retired = self.take_surface_fences(surface, None);
                self.retire_fences(retired);
                None
            }
        };
        let copied = self.copy(queue, surface, import_gen, frame);
        if let Some((fence, name, value)) = fence {
            // After the copy's submission (or at once if it failed), so the producer never waits
            // on a frame Rune gave up on.
            // SAFETY: the raw queue only gets a signal, ordered after work wgpu already submitted.
            let hal = unsafe { queue.as_hal::<Dx12>() }.ok_or("shared textures need wgpu's D3D12 backend")?;
            // SAFETY: as above; `fence` is a live shared fence opened on this device.
            unsafe { hal.as_raw().Signal(&fence, value) }.map_err(|e| format!("signal release fence: {e}"))?;
            if let Some(entry) = self.fences.iter_mut().rev().find(|e| e.surface == surface && e.name == name) {
                entry.signaled = true;
            }
        }
        copied
    }

    /// Lets go of decoded plane wrappers whose decoder and pictures are gone, and prunes cached
    /// shared handles, fences, and copy rings for surfaces no longer live. Returns true when any
    /// texture, copy ring, or fence retirement needs `CompletionWorker` polling.
    pub fn forget_gone(&mut self, mut is_live_shared: impl FnMut(usize) -> bool) -> bool {
        let before = self.planes.len() + self.textures.len() + self.copies.len();
        self.forget_gone_planes();
        // Dropping cached `ID3D12Resource` clones here is GPU-safe: every submitted copy already
        // moved its own COM reference into a `wgpu::Texture` retained by wgpu until completion.
        self.textures.retain(|e| is_live_shared(e.surface));
        self.copies.retain(|c| is_live_shared(c.surface));
        self.import_counts.retain(|(surface, _)| is_live_shared(*surface));
        let mut retired = Vec::new();
        let mut i = 0;
        while i < self.fences.len() {
            if is_live_shared(self.fences[i].surface) {
                i += 1;
            } else {
                let evicted = self.fences.remove(i);
                if evicted.signaled {
                    retired.push(self.wrap_retained(evicted.fence));
                }
            }
        }
        self.retire_fences(retired);
        let dropped = self.planes.len() + self.textures.len() + self.copies.len() < before;
        std::mem::take(&mut self.needs_poll) || dropped
    }

    fn forget_gone_planes(&mut self) {
        self.planes.retain(|(_, live, _)| live.strong_count() > 0);
    }

    fn wrap_retained(&self, fence: ID3D12Fence) -> RetainedFence {
        #[cfg(test)]
        {
            self.in_flight_fences.fetch_add(1, Ordering::SeqCst);
            RetainedFence { _fence: fence, in_flight: self.in_flight_fences.clone() }
        }
        #[cfg(not(test))]
        RetainedFence { _fence: fence }
    }

    fn retire_fences(&mut self, retired: Vec<RetainedFence>) {
        if retire_fences_on(&self.device, self.queue.as_ref(), retired) {
            self.needs_poll = true;
        }
    }

    fn take_surface_fences(&mut self, surface: usize, keep_name: Option<&str>) -> Vec<RetainedFence> {
        let mut retired = Vec::new();
        let mut i = 0;
        while i < self.fences.len() {
            if self.fences[i].surface == surface && keep_name != Some(self.fences[i].name.as_str()) {
                let evicted = self.fences.remove(i);
                if evicted.signaled {
                    retired.push(self.wrap_retained(evicted.fence));
                }
            } else {
                i += 1;
            }
        }
        retired
    }

    fn next_import_gen(&mut self, surface: usize) -> u64 {
        if let Some((_, count)) = self.import_counts.iter_mut().find(|(s, _)| *s == surface) {
            *count += 1;
            *count
        } else {
            self.import_counts.push((surface, 1));
            1
        }
    }

    fn acquire_copy(&mut self, surface: usize, size: [u32; 2]) -> (wgpu::Texture, Option<Arc<()>>) {
        let i = match self.copies.iter().position(|r| r.surface == surface) {
            Some(i) => i,
            None => {
                self.copies.push(CopyRing { surface, size, slots: Vec::new() });
                self.copies.len() - 1
            }
        };
        let ring = &mut self.copies[i];
        if ring.size != size {
            ring.size = size;
            ring.slots.clear();
            self.needs_poll = true;
        }
        // Prefer an idle slot and rotate it to the back so slots cycle evenly.
        if let Some(i) = ring.slots.iter().position(|s| s.in_use.strong_count() == 0) {
            let mut slot = ring.slots.remove(i);
            let lease = Arc::new(());
            slot.in_use = Arc::downgrade(&lease);
            let texture = slot.texture.clone();
            ring.slots.push(slot);
            return (texture, Some(lease));
        }
        let mut desc = pipeline::descriptor(size[0], size[1], wgpu::TextureFormat::Bgra8Unorm);
        desc.usage |= wgpu::TextureUsages::COPY_DST;
        let texture = self.device.create_texture(&desc);
        if ring.slots.len() < COPY_SLOTS {
            let lease = Arc::new(());
            ring.slots.push(CopySlot { texture: texture.clone(), in_use: Arc::downgrade(&lease) });
            (texture, Some(lease))
        } else {
            // All pooled slots are held by live/in-flight frames: allocate a one-shot texture
            // so we never overwrite a target an unsubmitted or in-flight draw still samples.
            (texture, None)
        }
    }

    fn copy(&mut self, queue: &wgpu::Queue, surface: usize, import_gen: u64, frame: &SharedTexture) -> Result<([wgpu::Texture; 2], Option<Arc<()>>), String> {
        let [width, height] = frame.size;
        let resource = self.texture(surface, import_gen, &frame.name, width, height)?;
        let mut desc = pipeline::descriptor(width, height, wgpu::TextureFormat::Bgra8Unorm);
        desc.usage = wgpu::TextureUsages::COPY_SRC;
        // SAFETY: `resource` was opened on this device and checked to be a 2D, single-mip,
        // single-sample BGRA8 texture of `desc`'s size.
        let hal = unsafe {
            wgpu::hal::dx12::Device::texture_from_raw(
                resource,
                desc.format,
                desc.dimension,
                desc.size,
                desc.mip_level_count,
                desc.sample_count,
            )
        };
        // A shared resource enters and leaves each submission in COMMON, which wgpu's D3D12
        // backend calls PRESENT.
        // SAFETY: the resource was checked against `desc` above and belongs to this device.
        let source = unsafe { self.device.create_texture_from_hal::<Dx12>(hal, &desc, wgpu::TextureUses::PRESENT) };
        let (owned, lease) = self.acquire_copy(surface, frame.size);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shared texture copy") });
        encoder.copy_texture_to_texture(source.as_image_copy(), owned.as_image_copy(), desc.size);
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition { texture: &source, selector: None, state: wgpu::TextureUses::PRESENT }),
        );
        queue.submit([encoder.finish()]);
        Ok(([owned.clone(), owned], lease))
    }

    fn texture(&mut self, surface: usize, import_gen: u64, name: &str, width: u32, height: u32) -> Result<ID3D12Resource, String> {
        let size = [width, height];
        // Drop handles for this surface whose size no longer matches (resize) or that have not
        // been referenced for `STALE_IMPORTS` frames (producer switched to new handle names).
        let before = self.textures.len();
        self.textures.retain(|e| {
            e.surface != surface || (e.size == size && import_gen.saturating_sub(e.last_import) <= STALE_IMPORTS)
        });
        if self.textures.len() < before {
            self.needs_poll = true;
        }
        if let Some(i) = self.textures.iter().position(|e| e.surface == surface && e.name == name) {
            let mut entry = self.textures.remove(i);
            entry.last_import = import_gen;
            let resource = entry.resource.clone();
            self.textures.push(entry);
            return Ok(resource);
        }
        let resource: ID3D12Resource = self.open(name)?;
        // SAFETY: a plain query on a live resource.
        let desc = unsafe { resource.GetDesc() };
        if desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
            || desc.Width != u64::from(width)
            || desc.Height != height
            || desc.DepthOrArraySize != 1
            || desc.MipLevels != 1
            || desc.SampleDesc.Count != 1
            || desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM
        {
            return Err(format!("shared texture {name:?} isn't a {width}x{height} BGRA8 texture"));
        }
        if self.textures.len() >= OPENED {
            self.textures.remove(0);
            self.needs_poll = true;
        }
        self.textures.push(CachedTexture {
            surface,
            name: name.to_owned(),
            size,
            last_import: import_gen,
            resource: resource.clone(),
        });
        Ok(resource)
    }

    fn fence(&mut self, surface: usize, name: &str) -> Result<ID3D12Fence, String> {
        if let Some(i) = self.fences.iter().position(|e| e.surface == surface && e.name == name) {
            let entry = self.fences.remove(i);
            let fence = entry.fence.clone();
            self.fences.push(entry);
            return Ok(fence);
        }
        let fence: ID3D12Fence = self.open(name)?;
        let mut retired = self.take_surface_fences(surface, Some(name));
        if self.fences.len() >= OPENED {
            let evicted = self.fences.remove(0);
            if evicted.signaled {
                retired.push(self.wrap_retained(evicted.fence));
            }
        }
        self.retire_fences(retired);
        self.fences.push(CachedFence { surface, name: name.to_owned(), fence: fence.clone(), signaled: false });
        Ok(fence)
    }

    fn open<T: Interface>(&self, name: &str) -> Result<T, String> {
        // SAFETY: only the raw device is borrowed, for the calls below.
        let hal = unsafe { self.device.as_hal::<Dx12>() }.ok_or("shared textures need wgpu's D3D12 backend")?;
        let device: &ID3D12Device = hal.raw_device();
        // SAFETY: opens a named shared object; the handle is closed once the object holds its
        // own reference.
        let handle: HANDLE = unsafe { device.OpenSharedHandleByName(&HSTRING::from(name), GENERIC_ALL.0) }
            .map_err(|e| format!("open shared {name:?}: {e}"))?;
        let mut object: Option<T> = None;
        // SAFETY: `handle` is the live handle opened above; `object` receives a new reference.
        let opened = unsafe { device.OpenSharedHandle(handle, &mut object) };
        // SAFETY: `handle` is ours and closed once; the opened object keeps the resource alive.
        unsafe { CloseHandle(handle) }.ok();
        opened.map_err(|e| format!("open shared {name:?}: {e}"))?;
        object.ok_or_else(|| format!("open shared {name:?}: no object"))
    }
}

/// After the submission that drew a decoded picture: the decoder may write its texture again.
pub(crate) fn release_decoded(queue: &wgpu::Queue, planes: &DecodedPlanes, value: u64) {
    // SAFETY: the raw queue only gets a signal, ordered after work wgpu already submitted.
    if let Some(raw) = unsafe { queue.as_hal::<Dx12>() } {
        // SAFETY: as above; the fence is live and belongs to this device.
        unsafe { raw.as_raw().Signal(&planes.fences.release, value) }.ok();
    }
}

/// Keeps `retired` fences alive until an empty wgpu submission placed strictly after every
/// earlier raw `ID3D12CommandQueue::Signal` completes on the GPU.
fn retire_fences_on(device: &wgpu::Device, queue: Option<&wgpu::Queue>, retired: Vec<RetainedFence>) -> bool {
    if retired.is_empty() {
        return false;
    }
    let Some(queue) = queue else {
        return false;
    };
    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("retire shared fences") });
    encoder.on_submitted_work_done(move || drop(retired));
    queue.submit([encoder.finish()]);
    true
}

#[cfg(test)]
impl Importer {
    pub(super) fn cached_texture_names(&self) -> Vec<&str> {
        self.textures.iter().map(|e| e.name.as_str()).collect()
    }

    pub(super) fn cached_fence_count(&self) -> usize {
        self.fences.len()
    }

    pub(super) fn in_flight_fences_counter(&self) -> Arc<std::sync::atomic::AtomicUsize> {
        self.in_flight_fences.clone()
    }

    pub(super) fn copy_pool_counts(&self) -> Vec<(usize, [u32; 2], usize)> {
        self.copies.iter().map(|c| (c.surface, c.size, c.slots.len())).collect()
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
