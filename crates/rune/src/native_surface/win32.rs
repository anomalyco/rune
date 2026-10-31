//! Direct3D shared-texture ownership boundary, the Windows sibling of `macos.rs`. A producer (a
//! decoder, or mpv rendering through OpenGL/D3D11 interop) writes into a shareable D3D11/12
//! texture and hands its NT handle over; Rune opens it once on its own D3D12 device and draws it
//! with no copy. Two shared fences let them take turns: Rune's queue waits for `ready` before
//! sampling, and signals `done` once it no longer reads the frame.
use super::*;
use std::collections::HashMap;
use std::sync::Weak;
use std::sync::atomic::{AtomicU64, Ordering};
use windows::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM};
use windows::Win32::System::Threading::GetCurrentProcess;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// An NT handle this process owns (a duplicate of the producer's), closed on drop.
struct Owned(HANDLE);
// A handle is a process-wide token; duplicating and closing it is thread safe.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}
impl Owned {
    /// # Safety
    /// `raw` must be a live handle of this process.
    unsafe fn duplicate(raw: *mut std::ffi::c_void) -> Result<Self, String> {
        if raw.is_null() {
            return Err("null shared handle".into());
        }
        let mut out = HANDLE::default();
        unsafe {
            let me = GetCurrentProcess();
            DuplicateHandle(me, HANDLE(raw), me, &mut out, 0, false, DUPLICATE_SAME_ACCESS)
                .map_err(|e| format!("DuplicateHandle: {e}"))?;
        }
        Ok(Self(out))
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct Texture {
    handle: Owned,
    /// Identity for the importer's cache: handles are reused once closed.
    id: u64,
    size: [u32; 2],
    format: Format,
    color: ColorInfo,
}

/// A shareable D3D fence (`D3D11_FENCE_FLAG_SHARED` or `D3D12_FENCE_FLAG_SHARED`), opened once.
#[derive(Clone)]
pub struct SharedFence(Arc<(Owned, u64)>);
impl SharedFence {
    /// Keep a duplicate of the fence's shared NT handle; the caller keeps its own.
    ///
    /// # Safety
    /// `handle` must be a live shared NT handle of an `ID3D11Fence` or `ID3D12Fence` on the GPU
    /// Rune draws with.
    pub unsafe fn from_nt_handle(handle: *mut std::ffi::c_void) -> Result<Self, String> {
        Ok(Self(Arc::new((unsafe { Owned::duplicate(handle)? }, NEXT_ID.fetch_add(1, Ordering::Relaxed)))))
    }
}

/// How a frame and its producer take turns: Rune's queue waits until `ready` reaches
/// `ready_value` before sampling the texture, and raises `done` to a frame's id once the GPU no
/// longer reads it or any earlier frame of the same surface (frames skipped by a newer push
/// included), so the producer may write into that texture again. Frame ids must increase.
#[derive(Clone)]
pub struct FrameSync {
    pub ready: SharedFence,
    pub ready_value: u64,
    pub done: SharedFence,
}

/// A frame in a shareable D3D11/12 texture: 8-bit BGRA or RGBA (`Format::Bgra`, either channel
/// order: the texture's own `DXGI_FORMAT` decides), 2D, one mip, one sample. Clones share it.
#[derive(Clone)]
pub struct SharedTexture {
    texture: Arc<Texture>,
    sync: Option<FrameSync>,
}
impl SharedTexture {
    /// Keep a duplicate of the texture's shared NT handle (from `IDXGIResource1::CreateSharedHandle`
    /// or `ID3D12Device::CreateSharedHandle`); the caller keeps its own. Without `with_sync`, the
    /// pixels must be complete before the push and stay unchanged until the frame is replaced and
    /// every draw using it has completed (see `NativeSurface::counts`).
    ///
    /// # Safety
    /// `handle` must be a live shared NT handle of a texture of `size` and `format` on the GPU
    /// Rune draws with.
    pub unsafe fn from_nt_handle(handle: *mut std::ffi::c_void, size: [u32; 2], format: Format, color: ColorInfo) -> Result<Self, String> {
        check_size(size, format)?;
        let texture = Texture { handle: unsafe { Owned::duplicate(handle)? }, id: NEXT_ID.fetch_add(1, Ordering::Relaxed), size, format, color };
        Ok(Self { texture: Arc::new(texture), sync: None })
    }
    /// The same texture, synchronized with its producer by fences (see [`FrameSync`]).
    pub fn with_sync(mut self, sync: FrameSync) -> Self {
        self.sync = Some(sync);
        self
    }
    pub(crate) fn info(&self) -> Result<([u32; 2], Format, ColorInfo), String> {
        let t = &self.texture;
        check_size(t.size, t.format)?;
        Ok((t.size, t.format, t.color))
    }
    /// Test source only: a D3D11 texture on the default adapter, filled with `frame` and shared.
    /// The production path never touches pixel memory on the CPU.
    pub fn from_bytes(frame: &ByteFrame) -> Result<Self, String> {
        let size = [frame.width, frame.height];
        check_size(size, frame.format)?;
        let (w, h) = (frame.width as usize, frame.height as usize);
        let nv12 = frame.format != Format::Bgra;
        if frame.planes[0].len() != w * h * if nv12 { 1 } else { 4 } || frame.planes[1].len() != if nv12 { w * h / 2 } else { 0 } {
            return Err("invalid plane length".into());
        }
        let handle = d3d11::shared_texture(frame)?;
        let made = unsafe { Self::from_nt_handle(handle.0.0, size, frame.format, frame.color) };
        drop(handle);
        made
    }
}

fn check_size(size: [u32; 2], format: Format) -> Result<(), String> {
    if size.contains(&0) || size.iter().any(|v| *v > 16384) {
        return Err("invalid video dimensions".into());
    }
    if format != Format::Bgra && (!size[0].is_multiple_of(2) || !size[1].is_multiple_of(2)) {
        return Err("NV12 requires even dimensions".into());
    }
    Ok(())
}

/// D3D12 objects Rune shares between its render thread and drop sites. D3D12 is free-threaded.
struct Shared<T>(T);
unsafe impl<T> Send for Shared<T> {}
unsafe impl<T> Sync for Shared<T> {}

/// Raises a frame's `done` fence when the last draw holding the frame lets go, after every
/// command already submitted on Rune's queue: so once it completes, the GPU is done with it.
pub(crate) struct Release {
    queue: Shared<ID3D12CommandQueue>,
    fence: Shared<ID3D12Fence>,
    value: AtomicU64,
}
impl Release {
    /// A newer frame `id` replaced this one: the frames between were never drawn, release them too.
    pub(crate) fn replaced_by(&self, id: u64) {
        self.value.fetch_max(id.saturating_sub(1), Ordering::Relaxed);
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        let _ = unsafe { self.queue.0.Signal(&self.fence.0, self.value.load(Ordering::Relaxed)) };
    }
}

/// Opens shared textures and fences on Rune's D3D12 device and caches them by identity, as
/// `CVMetalTextureCache` does for IOSurfaces. `flush` drops what no producer holds any more.
pub(crate) struct Importer {
    device: wgpu::Device,
    raw: Shared<ID3D12Device>,
    queue: Shared<ID3D12CommandQueue>,
    textures: HashMap<u64, (Weak<Texture>, wgpu::Texture)>,
    fences: HashMap<u64, (Weak<(Owned, u64)>, Shared<ID3D12Fence>)>,
}
impl Importer {
    pub fn new(device: &wgpu::Device) -> Result<Self, String> {
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }
            .ok_or("NativeSurface needs wgpu's DX12 backend on Windows (WGPU_BACKEND=dx12)")?;
        Ok(Self {
            device: device.clone(),
            raw: Shared(hal.raw_device().clone()),
            queue: Shared(hal.raw_queue().clone()),
            textures: HashMap::new(),
            fences: HashMap::new(),
        })
    }
    pub fn flush(&mut self) {
        self.textures.retain(|_, (weak, _)| weak.strong_count() > 0);
        self.fences.retain(|_, (weak, _)| weak.strong_count() > 0);
    }
    fn fence(&mut self, fence: &SharedFence) -> Result<ID3D12Fence, String> {
        if let Some((_, f)) = self.fences.get(&fence.0.1) {
            return Ok(f.0.clone());
        }
        let mut opened: Option<ID3D12Fence> = None;
        unsafe { self.raw.0.OpenSharedHandle(fence.0.0.0, &mut opened) }.map_err(|e| format!("OpenSharedHandle (fence): {e}"))?;
        let opened = opened.ok_or("null shared fence")?;
        self.fences.insert(fence.0.1, (Arc::downgrade(&fence.0), Shared(opened.clone())));
        Ok(opened)
    }
    /// The frame's planes as Rune textures, after making Rune's queue wait for its `ready` fence;
    /// and what raises its `done` fence once no draw holds it.
    pub fn import(&mut self, frame: &SharedTexture, frame_id: u64) -> Result<([wgpu::Texture; 2], Option<Release>), String> {
        let t = &frame.texture;
        let texture = match self.textures.get(&t.id) {
            Some((_, texture)) => texture.clone(),
            None => {
                let mut opened: Option<ID3D12Resource> = None;
                unsafe { self.raw.0.OpenSharedHandle(t.handle.0, &mut opened) }
                    .map_err(|e| format!("OpenSharedHandle (texture; is it on Rune's GPU?): {e}"))?;
                let resource = opened.ok_or("null shared texture")?;
                if t.format != Format::Bgra {
                    return Err("NV12 shared textures aren't imported on Windows yet; share BGRA or RGBA".into());
                }
                // Four 8-bit channels either way round: the texture says which (OpenGL interop
                // writes RGBA; decoders and D3D swap chains BGRA).
                let format = match unsafe { resource.GetDesc() }.Format {
                    DXGI_FORMAT_B8G8R8A8_UNORM => wgpu::TextureFormat::Bgra8Unorm,
                    DXGI_FORMAT_R8G8B8A8_UNORM => wgpu::TextureFormat::Rgba8Unorm,
                    other => return Err(format!("unsupported shared texture format {other:?}; use B8G8R8A8 or R8G8B8A8 UNORM")),
                };
                let desc = pipeline::descriptor(t.size[0], t.size[1], format);
                let hal = unsafe {
                    wgpu::hal::dx12::Device::texture_from_raw(
                        resource,
                        format,
                        wgpu::TextureDimension::D2,
                        wgpu::Extent3d { width: t.size[0], height: t.size[1], depth_or_array_layers: 1 },
                        1,
                        1,
                    )
                };
                // A shared texture rests in D3D12_RESOURCE_STATE_COMMON and is promoted to a shader
                // resource implicitly on use, so wgpu's first transition is a no-op.
                let texture = unsafe { self.device.create_texture_from_hal::<wgpu::hal::api::Dx12>(hal, &desc, wgpu::TextureUses::RESOURCE) };
                self.textures.insert(t.id, (Arc::downgrade(t), texture.clone()));
                texture
            }
        };
        let release = match &frame.sync {
            None => None,
            Some(sync) => {
                let ready = self.fence(&sync.ready)?;
                unsafe { self.queue.0.Wait(&ready, sync.ready_value) }.map_err(|e| format!("ID3D12CommandQueue::Wait: {e}"))?;
                let done = self.fence(&sync.done)?;
                // Drawing this frame from now on: frames before it (on screen until now, or skipped
                // by this push before they were ever drawn) go back once the work already on the
                // queue is through. Without this a skipped frame would never be returned.
                unsafe { self.queue.0.Signal(&done, frame_id.saturating_sub(1)) }.map_err(|e| format!("ID3D12CommandQueue::Signal: {e}"))?;
                Some(Release { queue: Shared(self.queue.0.clone()), fence: Shared(done), value: AtomicU64::new(frame_id) })
            }
        };
        Ok(([texture.clone(), texture], release))
    }
}

/// D3D11 for the test source: make a shareable texture with pixels in it.
mod d3d11 {
    use super::*;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::{DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIResource1};
    use windows::core::Interface;

    pub(super) fn shared_texture(frame: &ByteFrame) -> Result<Owned, String> {
        let e = |what: &'static str| move |err: windows::core::Error| format!("{what}: {err}");
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, Default::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None,
                D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)).map_err(e("D3D11CreateDevice"))?;
        }
        let (device, context): (ID3D11Device, ID3D11DeviceContext) = (device.ok_or("no D3D11 device")?, context.ok_or("no D3D11 context")?);
        let nv12 = frame.format != Format::Bgra;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: frame.width,
            Height: frame.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: if nv12 { DXGI_FORMAT_NV12 } else { DXGI_FORMAT_B8G8R8A8_UNORM },
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
        };
        // NV12's planes are one allocation: luma rows, then interleaved chroma rows, same pitch.
        let pixels: Vec<u8> = if nv12 { [frame.planes[0].as_slice(), frame.planes[1].as_slice()].concat() } else { frame.planes[0].clone() };
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: pixels.as_ptr().cast(), SysMemPitch: frame.width * if nv12 { 1 } else { 4 }, SysMemSlicePitch: 0 };
        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, Some(&init), Some(&mut texture)) }.map_err(e("CreateTexture2D"))?;
        let texture = texture.ok_or("no texture")?;
        // The upload must land before another device reads the texture.
        let query_desc = D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 };
        let mut query = None;
        unsafe { device.CreateQuery(&query_desc, Some(&mut query)) }.map_err(e("CreateQuery"))?;
        let query = query.ok_or("no query")?;
        unsafe {
            context.End(&query);
            context.Flush();
            let mut done = 0i32;
            while context.GetData(&query, Some((&raw mut done).cast()), 4, 0).is_err() || done == 0 {
                std::thread::yield_now();
            }
        }
        let resource: IDXGIResource1 = texture.cast().map_err(e("IDXGIResource1"))?;
        let handle = unsafe { resource.CreateSharedHandle(None, (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0, None) }
            .map_err(e("CreateSharedHandle"))?;
        Ok(Owned(handle))
    }
}
