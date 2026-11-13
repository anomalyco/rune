//! Dmabufs from another process, read on Rune's Vulkan device.
//!
//! The producer renders BGRA8 frames (`DRM_FORMAT_ARGB8888`) into single-plane dmabufs with an
//! explicit DRM format modifier (`DRM_FORMAT_MOD_LINEAR` imports on every driver that imports
//! dmabufs), on the GPU at [`import_target`]'s render node, and finishes writing a frame before
//! it pushes it. Rune imports each buffer once and copies a pushed frame into a texture of its
//! own the first time it draws it. Once the GPU has finished that copy, Rune calls the frame's
//! [`on_read`](Dmabuf::on_read) with `Ok`, and the producer may write the buffer again; if the
//! frame can't be imported the callback gets the error. A frame replaced before it was drawn is
//! never read and its callback is dropped without running: the next frame's covers it.

use super::*;
use std::os::fd::OwnedFd;
use std::sync::Weak;
use wgpu::hal::api::Vulkan;

/// How many imported buffers an importer keeps: a producer usually cycles a few per stream.
const OPENED: usize = 16;

/// One dmabuf a producer writes frames into, imported once and drawn from as often as it's pushed.
#[derive(Debug)]
pub struct DmabufBuffer {
    fd: OwnedFd,
    size: [u32; 2],
    stride: u32,
    offset: u32,
    modifier: u64,
}

impl DmabufBuffer {
    /// A `width` × `height` BGRA8 (`DRM_FORMAT_ARGB8888`) buffer: its one plane starts at
    /// `offset` in `fd`, rows `stride` bytes apart, laid out as DRM format `modifier` says.
    pub fn new(fd: OwnedFd, width: u32, height: u32, stride: u32, offset: u32, modifier: u64) -> Self {
        Self { fd, size: [width, height], stride, offset, modifier }
    }
}

type OnRead = Box<dyn FnOnce(Result<(), String>) + Send>;

/// A finished frame in a [`DmabufBuffer`].
///
/// ```no_run
/// use std::sync::Arc;
/// use rune::native_surface::linux::{Dmabuf, DmabufBuffer};
/// # let fd: std::os::fd::OwnedFd = unimplemented!();
/// let buffer = Arc::new(DmabufBuffer::new(fd, 1280, 720, 5120, 0, 0));
/// let frame = Dmabuf::new(buffer).on_read(|read| eprintln!("frame 42 read: {read:?}"));
/// rune::NativeSurface::new().push_dmabuf(frame, 42).unwrap();
/// ```
pub struct Dmabuf {
    buffer: Arc<DmabufBuffer>,
    on_read: Mutex<Option<OnRead>>,
}

impl Dmabuf {
    pub fn new(buffer: Arc<DmabufBuffer>) -> Self {
        Self { buffer, on_read: Mutex::new(None) }
    }

    /// Runs `f` once Rune's GPU has finished reading the frame (`Ok`), or with why it couldn't
    /// (`Err`). It runs on whichever thread Rune finds out on.
    pub fn on_read(self, f: impl FnOnce(Result<(), String>) + Send + 'static) -> Self {
        *self.on_read.lock().unwrap() = Some(Box::new(f));
        self
    }

    pub(crate) fn size(&self) -> [u32; 2] {
        self.buffer.size
    }

    fn done(&self, result: Result<(), String>) -> Option<impl FnOnce() + use<>> {
        let f = self.on_read.lock().unwrap().take()?;
        Some(move || f(result))
    }
}

impl std::fmt::Debug for Dmabuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Dmabuf").field(&self.buffer).finish()
    }
}

/// Where a producer renders frames Rune can import.
#[derive(Clone, Debug, PartialEq, Eq)]
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

pub(crate) struct Importer {
    device: wgpu::Device,
    opened: Vec<(Weak<DmabufBuffer>, wgpu::Texture)>,
}

impl Importer {
    pub fn new(device: &wgpu::Device) -> Result<Self, String> {
        if !device.features().contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF) {
            return Err("this GPU device doesn't import dmabufs".into());
        }
        Ok(Self { device: device.clone(), opened: Vec::new() })
    }

    /// The importer in `slot`, made on `device` the first time.
    pub fn on<'a>(slot: &'a mut Option<Self>, device: &wgpu::Device) -> Result<&'a mut Self, String> {
        match slot {
            Some(importer) => Ok(importer),
            empty => Ok(empty.insert(Self::new(device)?)),
        }
    }

    /// Copies `frame` into a texture of Rune's own, and tells the producer once the GPU has read it.
    pub fn import(&mut self, queue: &wgpu::Queue, frame: &Dmabuf) -> Result<[wgpu::Texture; 2], String> {
        let copied = self.copy(queue, &frame.buffer);
        match &copied {
            Ok(_) => {
                if let Some(read) = frame.done(Ok(())) {
                    queue.on_submitted_work_done(read);
                }
            }
            Err(e) => {
                if let Some(failed) = frame.done(Err(e.clone())) {
                    failed();
                }
            }
        }
        copied
    }

    fn copy(&mut self, queue: &wgpu::Queue, buffer: &Arc<DmabufBuffer>) -> Result<[wgpu::Texture; 2], String> {
        let source = self.source(buffer)?;
        let [width, height] = buffer.size;
        let mut desc = pipeline::descriptor(width, height, wgpu::TextureFormat::Bgra8Unorm);
        desc.usage |= wgpu::TextureUsages::COPY_DST;
        let owned = self.device.create_texture(&desc);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("dmabuf copy") });
        encoder.copy_texture_to_texture(source.as_image_copy(), owned.as_image_copy(), desc.size);
        queue.submit([encoder.finish()]);
        Ok([owned.clone(), owned])
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
        let fd = buffer.fd.try_clone().map_err(|e| format!("dup dmabuf: {e}"))?;
        // SAFETY: only the raw device is borrowed, for the import.
        let hal = unsafe { self.device.as_hal::<Vulkan>() }.ok_or("dmabufs need wgpu's Vulkan backend")?;
        // SAFETY: `fd` is a dmabuf holding a `width` × `height` single-plane BGRA8 image laid out
        // as `modifier`, `stride` and `offset` say (the producer's contract, `DmabufBuffer::new`);
        // the device has `VULKAN_EXTERNAL_MEMORY_DMA_BUF` (`Importer::new`).
        let raw = unsafe { hal.texture_from_dmabuf_fd(fd, &hal_desc, buffer.modifier, u64::from(buffer.stride), u64::from(buffer.offset)) }
            .map_err(|e| format!("import dmabuf: {e}"))?;
        drop(hal);
        // The image is new, so its layout starts undefined. A linear buffer has no compression
        // metadata, so the first transition keeps the producer's pixels, and as the producer only
        // writes between reads the texture stays in the copy-source state wgpu leaves it in.
        // SAFETY: `raw` was made on this device from `hal_desc`, which matches `desc`.
        let texture = unsafe { self.device.create_texture_from_hal::<Vulkan>(raw, &desc, wgpu::TextureUses::UNINITIALIZED) };
        if self.opened.len() >= OPENED {
            self.opened.remove(0);
        }
        self.opened.push((Arc::downgrade(buffer), texture.clone()));
        Ok(texture)
    }
}
