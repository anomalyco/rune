//! Dmabufs from another process, read on Rune's Vulkan device.
//!
//! The producer renders BGRA8 frames (`DRM_FORMAT_ARGB8888`) into single-plane, linear
//! (`DRM_FORMAT_MOD_LINEAR`) dmabufs on the GPU at [`import_target`]'s render node, and finishes
//! writing a frame before it pushes it. Rune imports each buffer once and copies a pushed frame
//! into a texture of its own the first time it draws it, then draws that copy as often as it
//! needs, in every window on the same device. Once the GPU has finished the copy, a thread of the
//! importer calls the frame's [`on_read`](Dmabuf::on_read) with `Ok` (no later frame or redraw is
//! needed for that), and the producer may write the buffer again. When the frame can't be shown
//! the callback gets the error instead. A frame replaced before it was drawn is never read and its
//! callback is dropped without running: the next frame's covers it.

use super::*;
use pipeline::completion::{OnRead, ReadWorker};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Weak;
use std::time::Duration;
use wgpu::hal::api::Vulkan;

/// `DRM_FORMAT_ARGB8888`: B, G, R, A bytes in memory, the one format Rune imports.
pub const DRM_FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
/// `DRM_FORMAT_MOD_LINEAR`, the one layout Rune imports.
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;

/// How many imported buffers an importer keeps, and how many of their bytes: a producer cycles a
/// few buffers per stream.
const OPENED: usize = 16;
const OPENED_BYTES: u64 = 512 << 20;
/// Owned copy targets kept per active surface: enough to avoid stalling against in-flight
/// swapchain frames while capping VRAM well below `KEPT_BINDS`.
const COPY_SLOTS: usize = 3;
/// How long the importer waits for its copy to finish before it gives the frame up.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Where a dmabuf's one plane lies in its file, in bytes.
#[derive(Debug)]
pub struct DmabufPlane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
    pub modifier: u64,
}

/// One dmabuf a producer writes frames into, imported once and drawn from as often as it's pushed.
#[derive(Debug)]
pub struct DmabufBuffer {
    plane: DmabufPlane,
    size: [u32; 2],
}

impl DmabufBuffer {
    /// A `size` (width, height) buffer of DRM format `fourcc` in `plane`. Only
    /// [`DRM_FORMAT_ARGB8888`] with [`DRM_FORMAT_MOD_LINEAR`] imports; rows must hold the width and
    /// the file the whole plane.
    #[allow(unsafe_code)]
    pub fn new(plane: DmabufPlane, size: [u32; 2], fourcc: u32) -> Result<Self, String> {
        let [width, height] = size;
        if fourcc != DRM_FORMAT_ARGB8888 {
            return Err(format!("dmabuf format {fourcc:#x} isn't ARGB8888"));
        }
        if plane.modifier != DRM_FORMAT_MOD_LINEAR {
            return Err(format!("dmabuf modifier {:#x} isn't linear", plane.modifier));
        }
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err("dmabuf size must be 1..16384".into());
        }
        if u64::from(plane.stride) < u64::from(width) * 4 {
            return Err(format!("dmabuf rows of {} bytes can't hold {width} pixels", plane.stride));
        }
        // SAFETY: lseek on a descriptor this owns; a dmabuf's end is its size.
        let len = unsafe { libc::lseek(plane.fd.as_raw_fd(), 0, libc::SEEK_END) };
        // SAFETY: as above, back to the start.
        unsafe { libc::lseek(plane.fd.as_raw_fd(), 0, libc::SEEK_SET) };
        let needed = u64::from(plane.offset) + u64::from(plane.stride) * u64::from(height);
        if len < 0 || (len as u64) < needed {
            return Err(format!("dmabuf of {len} bytes can't hold {needed}"));
        }
        Ok(Self { plane, size })
    }

    fn bytes(&self) -> u64 {
        u64::from(self.plane.stride) * u64::from(self.size[1])
    }
}

/// Whether the producer has heard about its frame.
enum Release {
    /// It didn't ask to.
    Unasked,
    Waiting(OnRead),
    /// It has, and may be writing the buffer again.
    Told,
}

/// A finished frame in a [`DmabufBuffer`].
///
/// ```no_run
/// use std::sync::Arc;
/// use rune_media::native_surface::linux::{DRM_FORMAT_ARGB8888, DRM_FORMAT_MOD_LINEAR, Dmabuf, DmabufBuffer, DmabufPlane};
/// # let fd: std::os::fd::OwnedFd = unimplemented!();
/// let plane = DmabufPlane { fd, offset: 0, stride: 5120, modifier: DRM_FORMAT_MOD_LINEAR };
/// let buffer = Arc::new(DmabufBuffer::new(plane, [1280, 720], DRM_FORMAT_ARGB8888).unwrap());
/// let frame = Dmabuf::new(buffer).on_read(|read| eprintln!("frame 42 read: {read:?}"));
/// rune_media::NativeSurface::new().push_dmabuf(frame, 42).unwrap();
/// ```
type CopyEntry = (wgpu::Device, wgpu::Texture, Option<Arc<()>>);

pub struct Dmabuf {
    buffer: Arc<DmabufBuffer>,
    release: Mutex<Release>,
    /// Rune's copy on each device that has drawn the frame, plus an optional pool lease that
    /// keeps a pooled copy target from being overwritten while this frame is alive.
    copies: Mutex<Vec<CopyEntry>>,
}

impl Dmabuf {
    pub fn new(buffer: Arc<DmabufBuffer>) -> Self {
        Self { buffer, release: Mutex::new(Release::Unasked), copies: Mutex::new(Vec::new()) }
    }

    /// Runs `f` once Rune's GPU has finished reading the frame (`Ok`), or with why it couldn't
    /// (`Err`). It runs on a thread of Rune's.
    pub fn on_read(self, f: impl FnOnce(Result<(), String>) + Send + 'static) -> Self {
        *self.release.lock().unwrap() = Release::Waiting(Box::new(f));
        self
    }

    pub(crate) fn size(&self) -> [u32; 2] {
        self.buffer.size
    }

    /// Moves to `Told`, handing back the callback if one waited.
    fn tell(&self) -> Option<OnRead> {
        match std::mem::replace(&mut *self.release.lock().unwrap(), Release::Told) {
            Release::Waiting(f) => Some(f),
            Release::Unasked | Release::Told => None,
        }
    }

    fn told(&self) -> bool {
        matches!(*self.release.lock().unwrap(), Release::Told)
    }
}

impl std::fmt::Debug for Dmabuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Dmabuf").field(&self.buffer).finish()
    }
}

/// Where a producer renders frames Rune can import.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImportTarget {
    /// The render node of the GPU Rune draws windows on (`/dev/dri/renderD128`), when its driver
    /// says (`VK_EXT_physical_device_drm`).
    pub render_node: Option<String>,
}

static TARGET: Mutex<Option<ImportTarget>> = Mutex::new(None);

/// The GPU the last window Rune made draws on, once one has been made with a Vulkan device that
/// imports dmabufs (`VK_EXT_external_memory_dma_buf` and `VK_EXT_image_drm_format_modifier`);
/// None before, or when it can't.
pub fn import_target() -> Option<ImportTarget> {
    TARGET.lock().unwrap().clone()
}

/// Remembers `device`'s GPU for [`import_target`].
pub(crate) fn note_device(device: &wgpu::Device) {
    let target = device.features().contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF).then(|| ImportTarget { render_node: render_node(device) });
    *TARGET.lock().unwrap() = target;
}

#[allow(unsafe_code)]
fn render_node(device: &wgpu::Device) -> Option<String> {
    use ash::vk;
    // SAFETY: the raw handles are only queried.
    let hal = unsafe { device.as_hal::<Vulkan>() }?;
    let instance = hal.shared_instance().raw_instance();
    let physical = hal.raw_physical_device();
    // SAFETY: plain queries on live handles; the DRM properties are only chained in when the
    // device has the extension.
    unsafe {
        let extensions = instance.enumerate_device_extension_properties(physical).ok()?;
        if !extensions.iter().any(|e| e.extension_name_as_c_str() == Ok(ash::ext::physical_device_drm::NAME)) {
            return None;
        }
        let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
        let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
        instance.get_physical_device_properties2(physical, &mut props);
        (drm.has_render == vk::TRUE).then(|| format!("/dev/dri/renderD{}", drm.render_minor))
    }
}

/// `frame`'s picture on `device`, through the importer in `slot`. Tells the producer about every
/// failure.
pub(crate) fn import(
    slot: &mut Option<Importer>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: usize,
    frame: &Dmabuf,
) -> Result<([wgpu::Texture; 2], bool), String> {
    let result = if frame.size().iter().any(|v| *v > device.limits().max_texture_dimension_2d) {
        Err("video dimensions exceed this GPU device's texture limit".into())
    } else {
        match slot {
            Some(importer) => Ok(importer),
            empty => Importer::new(device).map(|importer| empty.insert(importer)),
        }
        .and_then(|importer| importer.import(queue, surface, frame))
    };
    if let Err(e) = &result
        && let Some(f) = frame.tell()
    {
        f(Err(e.clone()));
    }
    result
}

struct CopySlot {
    texture: wgpu::Texture,
    /// Weak handle to the `Dmabuf` frame currently using this slot; `strong_count() == 0` means
    /// no live frame (in any window or in-flight submission) still references this texture.
    in_use: Weak<()>,
}

struct CopyPool {
    surface: usize,
    size: [u32; 2],
    slots: Vec<CopySlot>,
}

struct CopiedDmabuf {
    texture: wgpu::Texture,
    lease: Option<Arc<()>>,
    submission: wgpu::SubmissionIndex,
}

pub(crate) struct Importer {
    /// The thread that waits for each copy and tells its producer; dropped before `device`.
    reads: ReadWorker<(wgpu::SubmissionIndex, OnRead)>,
    device: wgpu::Device,
    opened: Vec<(Weak<DmabufBuffer>, wgpu::Texture)>,
    /// Per-surface pool of owned copy targets, sized lazily up to `COPY_SLOTS`.
    copies: Vec<CopyPool>,
}

impl Drop for Importer {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Importer {
    fn new(device: &wgpu::Device) -> Result<Self, String> {
        if !device.features().contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF) {
            return Err("this GPU device doesn't import dmabufs".into());
        }
        let reads = ReadWorker::spawn_device("rune-dmabuf-read", device.clone(), READ_TIMEOUT)
            .map_err(|e| format!("start the dmabuf read thread: {e}"))?;
        Ok(Self { reads, device: device.clone(), opened: Vec::new(), copies: Vec::new() })
    }

    /// Stops and joins the dmabuf read thread before its device is torn down.
    pub(crate) fn stop(&mut self) {
        self.reads.stop();
    }

    /// Rune's copy of `frame` on this device, made the first time; the producer hears once the
    /// GPU has finished it. Returns the textures and whether they belong to this importer's pool.
    fn import(&mut self, queue: &wgpu::Queue, surface: usize, frame: &Dmabuf) -> Result<([wgpu::Texture; 2], bool), String> {
        if let Some((_, copy, lease)) = frame.copies.lock().unwrap().iter().find(|(d, _, _)| *d == self.device) {
            // Another window on the same device may have copied this frame into its own pool;
            // only mark it reusable when it belongs to this importer's pool so we never pin
            // another window's pooled texture inside this renderer's `Prepared::binds`.
            let reusable = lease.is_some()
                && self.copies.iter().any(|p| p.surface == surface && p.slots.iter().any(|s| s.texture == *copy));
            return Ok(([copy.clone(), copy.clone()], reusable));
        }
        if frame.told() {
            return Err("the producer may be writing this dmabuf again; another GPU device read it first".into());
        }
        let copied = self.copy(queue, surface, &frame.buffer)?;
        let reusable = copied.lease.is_some();
        let copy = copied.texture;
        let submission = copied.submission;
        frame.copies.lock().unwrap().push((self.device.clone(), copy.clone(), copied.lease));
        if let Some(f) = frame.tell()
            && let Err((_, f)) = self.reads.send((submission, f))
        {
            f(Err("wait for the dmabuf copy: the dmabuf read thread stopped".into()));
        }
        Ok(([copy.clone(), copy], reusable))
    }

    /// Lets go of buffers whose producer and frames are all gone, and copy pools for surfaces no
    /// longer live. Returns true when any buffer or copy pool was dropped.
    pub(crate) fn forget_gone(&mut self, mut is_live_dmabuf: impl FnMut(usize) -> bool) -> bool {
        let before = self.opened.len() + self.copies.len();
        self.opened.retain(|(live, _)| live.strong_count() > 0);
        self.copies.retain(|p| is_live_dmabuf(p.surface));
        self.opened.len() + self.copies.len() < before
    }

    fn acquire_copy(&mut self, surface: usize, size: [u32; 2]) -> (wgpu::Texture, Option<Arc<()>>) {
        let i = match self.copies.iter().position(|p| p.surface == surface) {
            Some(i) => i,
            None => {
                self.copies.push(CopyPool { surface, size, slots: Vec::new() });
                self.copies.len() - 1
            }
        };
        let pool = &mut self.copies[i];
        if pool.size != size {
            pool.size = size;
            pool.slots.clear();
        }
        // Prefer an idle slot and rotate it to the back so slots cycle evenly.
        if let Some(i) = pool.slots.iter().position(|s| s.in_use.strong_count() == 0) {
            let mut slot = pool.slots.remove(i);
            let lease = Arc::new(());
            slot.in_use = Arc::downgrade(&lease);
            let texture = slot.texture.clone();
            pool.slots.push(slot);
            return (texture, Some(lease));
        }
        let mut desc = pipeline::descriptor(size[0], size[1], wgpu::TextureFormat::Bgra8Unorm);
        desc.usage |= wgpu::TextureUsages::COPY_DST;
        let texture = self.device.create_texture(&desc);
        if pool.slots.len() < COPY_SLOTS {
            let lease = Arc::new(());
            pool.slots.push(CopySlot { texture: texture.clone(), in_use: Arc::downgrade(&lease) });
            (texture, Some(lease))
        } else {
            // All pooled slots are held by live frames (e.g. another window or a burst of
            // in-flight submissions): allocate a one-shot texture so we never overwrite one in use.
            (texture, None)
        }
    }

    fn copy(
        &mut self,
        queue: &wgpu::Queue,
        surface: usize,
        buffer: &Arc<DmabufBuffer>,
    ) -> Result<CopiedDmabuf, String> {
        let source = self.source(buffer)?;
        let (texture, lease) = self.acquire_copy(surface, buffer.size);
        let size = wgpu::Extent3d { width: buffer.size[0], height: buffer.size[1], depth_or_array_layers: 1 };
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("dmabuf copy") });
        encoder.copy_texture_to_texture(source.as_image_copy(), texture.as_image_copy(), size);
        Ok(CopiedDmabuf { texture, lease, submission: queue.submit([encoder.finish()]) })
    }

    /// `buffer` as a texture on this device, imported the first time it's drawn.
    #[allow(unsafe_code)]
    fn source(&mut self, buffer: &Arc<DmabufBuffer>) -> Result<wgpu::Texture, String> {
        self.opened.retain(|(live, _)| live.strong_count() > 0);
        if let Some((_, texture)) = self.opened.iter().find(|(live, _)| std::ptr::eq(live.as_ptr(), Arc::as_ptr(buffer))) {
            return Ok(texture.clone());
        }
        let [width, height] = buffer.size;
        let mut desc = pipeline::descriptor(width, height, wgpu::TextureFormat::Bgra8Unorm);
        desc.usage = wgpu::TextureUsages::COPY_SRC;
        let hal_desc = wgpu::hal::TextureDescriptor {
            label: Some("dmabuf"),
            size: desc.size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: desc.dimension,
            format: desc.format,
            usage: wgpu::TextureUses::COPY_SRC,
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };
        let plane = &buffer.plane;
        let fd = plane.fd.try_clone().map_err(|e| format!("dup dmabuf: {e}"))?;
        // SAFETY: only the raw device is borrowed, for the import.
        let hal = unsafe { self.device.as_hal::<Vulkan>() }.ok_or("dmabufs need wgpu's Vulkan backend")?;
        // SAFETY: `fd` is a linear single-plane ARGB8888 dmabuf of `width` × `height` whose rows,
        // offset and length `DmabufBuffer::new` checked; the device has
        // `VULKAN_EXTERNAL_MEMORY_DMA_BUF` (`Importer::new`).
        let raw = unsafe { hal.texture_from_dmabuf_fd(fd, &hal_desc, plane.modifier, u64::from(plane.stride), u64::from(plane.offset)) }
            .map_err(|e| format!("import dmabuf: {e}"))?;
        drop(hal);
        // The image is new, so its layout starts undefined. A linear buffer has no compression
        // metadata, so the first transition keeps the producer's pixels, and as the producer only
        // writes between reads the texture stays in the copy-source state wgpu leaves it in.
        // SAFETY: `raw` was made on this device from `hal_desc`, which matches `desc`.
        let texture = unsafe { self.device.create_texture_from_hal::<Vulkan>(raw, &desc, wgpu::TextureUses::UNINITIALIZED) };
        self.opened.push((Arc::downgrade(buffer), texture.clone()));
        let bytes = |opened: &[(Weak<DmabufBuffer>, wgpu::Texture)]| opened.iter().filter_map(|(b, _)| b.upgrade()).map(|b| b.bytes()).sum::<u64>();
        while self.opened.len() > OPENED || (self.opened.len() > 1 && bytes(&self.opened) > OPENED_BYTES) {
            self.opened.remove(0);
        }
        Ok(texture)
    }
}
