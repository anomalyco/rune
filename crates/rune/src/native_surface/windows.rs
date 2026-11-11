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

use super::*;
use ::windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE};
use ::windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_DIMENSION_TEXTURE2D, ID3D12Device, ID3D12Fence, ID3D12Resource};
use ::windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use ::windows::core::{HSTRING, Interface};
use std::sync::atomic::{AtomicU64, Ordering};
use wgpu::hal::api::Dx12;

/// How many opened textures and fences an importer keeps: a producer usually cycles a few
/// textures per stream, and one fence.
const OPENED: usize = 16;

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
    /// use rune::native_surface::windows::SharedTexture;
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
}

pub(crate) struct Importer {
    device: wgpu::Device,
    textures: Vec<(String, ID3D12Resource)>,
    fences: Vec<(String, ID3D12Fence)>,
    /// Decoded pictures' plane wrappers, by resource: a decoder cycles a few textures. Each goes
    /// once its decoder and every picture in it are gone.
    planes: Vec<(usize, std::sync::Weak<DecodedPlanes>, [wgpu::Texture; 2])>,
}

impl Importer {
    pub fn new(device: &wgpu::Device) -> Result<Self, String> {
        // SAFETY: only checks the backend.
        if unsafe { device.as_hal::<Dx12>() }.is_none() {
            return Err("shared textures need wgpu's D3D12 backend".into());
        }
        Ok(Self { device: device.clone(), textures: Vec::new(), fences: Vec::new(), planes: Vec::new() })
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
                self.forget_gone();
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

    /// Copies `frame` into a texture of Rune's own, then signals its release fence.
    pub fn import(&mut self, queue: &wgpu::Queue, frame: &SharedTexture) -> Result<[wgpu::Texture; 2], String> {
        let fence = match &frame.release {
            Some((name, value)) => Some((self.fence(name)?, *value)),
            None => None,
        };
        let copied = self.copy(queue, frame);
        if let Some((fence, value)) = fence {
            // After the copy's submission (or at once if it failed), so the producer never waits
            // on a frame Rune gave up on.
            // SAFETY: the raw queue only gets a signal, ordered after work wgpu already submitted.
            let hal = unsafe { queue.as_hal::<Dx12>() }.ok_or("shared textures need wgpu's D3D12 backend")?;
            // SAFETY: as above; `fence` is a live shared fence opened on this device.
            unsafe { hal.as_raw().Signal(&fence, value) }.map_err(|e| format!("signal release fence: {e}"))?;
        }
        copied
    }

    /// Lets go of plane wrappers whose decoder and pictures are all gone, so their textures go too.
    pub fn forget_gone(&mut self) {
        self.planes.retain(|(_, live, _)| live.strong_count() > 0);
    }

    fn copy(&mut self, queue: &wgpu::Queue, frame: &SharedTexture) -> Result<[wgpu::Texture; 2], String> {
        let [width, height] = frame.size;
        let resource = self.texture(&frame.name, width, height)?;
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
        let mut desc = pipeline::descriptor(width, height, wgpu::TextureFormat::Bgra8Unorm);
        desc.usage |= wgpu::TextureUsages::COPY_DST;
        let owned = self.device.create_texture(&desc);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shared texture copy") });
        encoder.copy_texture_to_texture(source.as_image_copy(), owned.as_image_copy(), desc.size);
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition { texture: &source, selector: None, state: wgpu::TextureUses::PRESENT }),
        );
        queue.submit([encoder.finish()]);
        Ok([owned.clone(), owned])
    }

    fn texture(&mut self, name: &str, width: u32, height: u32) -> Result<ID3D12Resource, String> {
        if let Some((_, resource)) = self.textures.iter().find(|(n, _)| n == name) {
            return Ok(resource.clone());
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
        remember(&mut self.textures, name, resource.clone());
        Ok(resource)
    }

    fn fence(&mut self, name: &str) -> Result<ID3D12Fence, String> {
        if let Some((_, fence)) = self.fences.iter().find(|(n, _)| n == name) {
            return Ok(fence.clone());
        }
        let fence: ID3D12Fence = self.open(name)?;
        remember(&mut self.fences, name, fence.clone());
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
        let _ = unsafe { CloseHandle(handle) };
        opened.map_err(|e| format!("open shared {name:?}: {e}"))?;
        object.ok_or_else(|| format!("open shared {name:?}: no object"))
    }
}

/// After the submission that drew a decoded picture: the decoder may write its texture again.
pub(crate) fn release_decoded(queue: &wgpu::Queue, planes: &DecodedPlanes, value: u64) {
    // SAFETY: the raw queue only gets a signal, ordered after work wgpu already submitted.
    if let Some(raw) = unsafe { queue.as_hal::<Dx12>() } {
        // SAFETY: as above; the fence is live and belongs to this device.
        let _ = unsafe { raw.as_raw().Signal(&planes.fences.release, value) };
    }
}

/// Keeps `value` under `name`, dropping the oldest past [`OPENED`].
fn remember<T>(list: &mut Vec<(String, T)>, name: &str, value: T) {
    if list.len() >= OPENED {
        list.remove(0);
    }
    list.push((name.to_owned(), value));
}
