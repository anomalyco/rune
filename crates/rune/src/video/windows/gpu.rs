//! Where decoded pictures live on the GPU. Media Foundation decodes on a D3D11 device (its DXVA
//! decoders take a DXGI device manager), made once per process on the adapter Rune draws with.
//! Each player keeps a small ring of shared NV12 textures: the decoder's output is copied into a
//! free one on the GPU, the copy signals a fence, and Rune's D3D12 device samples the texture in
//! place (`native_surface::windows::DecodedPlanes`). Pictures never pass through the CPU.

use super::mf::{self, Picture};
use crate::native_surface::windows::{DecodedPlanes, VideoFences, presenting_device};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HMODULE, LUID};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D12::{D3D12_FENCE_FLAG_NONE, D3D12_FENCE_FLAG_SHARED, ID3D12Device, ID3D12Fence, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIFactory4, IDXGIResource1};
use windows::Win32::Media::MediaFoundation::IMFDXGIDeviceManager;
use windows::core::{GUID, HRESULT, Interface};

/// Pictures a player's ring holds: the few the decoder runs ahead, the one on screen, one the GPU
/// may still be reading and a spare.
pub(super) const RING: usize = 6;

type CreateDevice = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    D3D_DRIVER_TYPE,
    HMODULE,
    u32,
    *const D3D_FEATURE_LEVEL,
    u32,
    u32,
    *mut *mut core::ffi::c_void,
    *mut D3D_FEATURE_LEVEL,
    *mut *mut core::ffi::c_void,
) -> HRESULT;
type CreateFactory = unsafe extern "system" fn(*const GUID, *mut *mut core::ffi::c_void) -> HRESULT;

/// The D3D11 device decoders share, on the adapter of Rune's D3D12 device.
pub(super) struct Device {
    pub device: ID3D11Device,
    context: ID3D11DeviceContext4,
    device5: ID3D11Device5,
    /// Rune's device, which opens the ring's textures and owns the fences.
    pub d3d12: ID3D12Device,
    luid: LUID,
}

static DEVICE: Mutex<Option<Arc<Device>>> = Mutex::new(None);

/// The decoders' D3D11 device, made on first use and kept; None when Rune draws without D3D12
/// or the adapter can't make one (decoding then runs on the CPU).
pub(super) fn device() -> Result<Arc<Device>, String> {
    let d3d12 = presenting_device().ok_or("no D3D12 device draws a window")?;
    // SAFETY: a plain query on a live device.
    let luid = unsafe { d3d12.GetAdapterLuid() };
    let mut slot = DEVICE.lock().unwrap();
    if let Some(device) = slot.as_ref().filter(|d| d.luid.LowPart == luid.LowPart && d.luid.HighPart == luid.HighPart) {
        return Ok(device.clone());
    }
    let device = Arc::new(make(d3d12, luid)?);
    *slot = Some(device.clone());
    Ok(device)
}

fn make(d3d12: ID3D12Device, luid: LUID) -> Result<Device, String> {
    let d3d11 = mf::system_library("d3d11.dll").ok_or("d3d11.dll is missing")?;
    let dxgi = mf::system_library("dxgi.dll").ok_or("dxgi.dll is missing")?;
    // SAFETY: documented entry points cast to their documented signatures; the out pointers are
    // owned references on success.
    unsafe {
        let create: CreateDevice = mf::symbol(d3d11, c"D3D11CreateDevice").ok_or("no D3D11CreateDevice")?;
        let factory: CreateFactory = mf::symbol(dxgi, c"CreateDXGIFactory1").ok_or("no CreateDXGIFactory1")?;
        let mut raw = std::ptr::null_mut();
        factory(&IDXGIFactory4::IID, &mut raw).ok().map_err(|e| format!("DXGI: {}", e.message()))?;
        let factory = IDXGIFactory4::from_raw(raw);
        let adapter: IDXGIAdapter = factory.EnumAdapterByLuid(luid).map_err(|e| format!("Rune's GPU: {}", e.message()))?;
        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let (mut device, mut context) = (std::ptr::null_mut(), std::ptr::null_mut());
        let flags = (D3D11_CREATE_DEVICE_VIDEO_SUPPORT.0 | D3D11_CREATE_DEVICE_BGRA_SUPPORT.0) as u32;
        create(adapter.as_raw(), D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), flags, levels.as_ptr(), levels.len() as u32, D3D11_SDK_VERSION, &mut device, std::ptr::null_mut(), &mut context)
            .ok()
            .map_err(|e| format!("no D3D11 video device: {}", e.message()))?;
        let device = ID3D11Device::from_raw(device);
        let context = ID3D11DeviceContext::from_raw(context);
        // Decoders and players' threads share the device.
        if let Ok(mt) = device.cast::<ID3D11Multithread>() {
            let _ = mt.SetMultithreadProtected(true);
        }
        let device5 = device.cast::<ID3D11Device5>().map_err(|_| "D3D11 fences need Windows 10 1703")?;
        let context = context.cast::<ID3D11DeviceContext4>().map_err(|_| "D3D11 fences need Windows 10 1703")?;
        Ok(Device { device, context, device5, d3d12, luid })
    }
}

impl Device {
    /// A DXGI device manager over this device, for one reader.
    pub(super) fn manager(&self, lib: &mf::Library) -> Result<IMFDXGIDeviceManager, String> {
        let (manager, token) = lib.device_manager()?;
        // SAFETY: hands the manager a reference to the live device.
        unsafe { manager.ResetDevice(&self.device, token) }.map_err(|e| e.message())?;
        Ok(manager)
    }
}

/// One texture of the ring.
struct Slot {
    texture: ID3D11Texture2D,
    planes: Arc<DecodedPlanes>,
    /// The fence value of the picture last written into it.
    value: u64,
}

/// A player's shared NV12 textures, the size of its pictures.
pub(super) struct Ring {
    device: Arc<Device>,
    slots: Vec<Slot>,
    fences: Arc<VideoFences>,
    ready: ID3D11Fence,
    value: u64,
    crop: (u32, u32, u32, u32),
}

impl Ring {
    pub(super) fn new(device: Arc<Device>, picture: &Picture) -> Result<Self, String> {
        // SAFETY: fences made on Rune's device and opened on the D3D11 one through a shared
        // handle, closed once both hold the object.
        let (fences, ready) = unsafe {
            let ready12: ID3D12Fence = device.d3d12.CreateFence(0, D3D12_FENCE_FLAG_SHARED).map_err(|e| format!("fence: {}", e.message()))?;
            let release: ID3D12Fence = device.d3d12.CreateFence(0, D3D12_FENCE_FLAG_NONE).map_err(|e| format!("fence: {}", e.message()))?;
            let handle = device.d3d12.CreateSharedHandle(&ready12, None, GENERIC_ALL.0, None).map_err(|e| format!("share fence: {}", e.message()))?;
            let mut ready: Option<ID3D11Fence> = None;
            let opened = device.device5.OpenSharedFence(handle, &mut ready);
            let _ = CloseHandle(handle);
            opened.map_err(|e| format!("open fence: {}", e.message()))?;
            (Arc::new(VideoFences { ready: ready12, release }), ready.ok_or("open fence")?)
        };
        Ok(Self { device, slots: Vec::with_capacity(RING), fences, ready, value: 0, crop: picture.crop })
    }

    /// Whether a picture could be written now: a slot is free, or there's room for another.
    pub(super) fn free(&self) -> bool {
        self.slots.len() < RING || self.reusable().is_some()
    }

    /// A slot no picture on screen, in a queue or in flight on the GPU uses.
    fn reusable(&self) -> Option<usize> {
        // SAFETY: a plain read of the fence's value.
        let released = unsafe { self.fences.release.GetCompletedValue() };
        self.slots.iter().position(|s| Arc::strong_count(&s.planes) == 1 && (s.value == 0 || released >= s.value))
    }

    /// A free slot, made when every made one is busy and the ring isn't full: a clip that shows
    /// one picture (a paused clip, a poster) makes one texture, not the whole ring.
    pub(super) fn take(&mut self) -> Result<Option<usize>, String> {
        if let Some(i) = self.reusable() {
            return Ok(Some(i));
        }
        if self.slots.len() == RING {
            return Ok(None);
        }
        let (_, _, w, h) = self.crop;
        self.slots.push(slot(&self.device, &self.fences, w, h)?);
        Ok(Some(self.slots.len() - 1))
    }

    /// Copies a decoder texture (array slice `index`) into slot `i` on the GPU.
    pub(super) fn copy(&mut self, i: usize, source: &ID3D11Texture2D, index: u32) -> (Arc<DecodedPlanes>, u64) {
        let (x, y, w, h) = self.crop;
        let area = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
        // SAFETY: both textures are NV12 on this device; the box lies inside the source.
        unsafe { self.device.context.CopySubresourceRegion(&self.slots[i].texture, 0, 0, 0, 0, source, index, Some(&area)) };
        self.signal(i)
    }

    /// Uploads packed NV12 planes (a software decoder's picture) into slot `i`.
    pub(super) fn upload(&mut self, i: usize, frame: &crate::native_surface::ByteFrame) -> (Arc<DecodedPlanes>, u64) {
        let w = frame.width as usize;
        // D3D11 takes an NV12 subresource as one block: the chroma rows follow the luma rows.
        let mut packed = Vec::with_capacity(frame.planes[0].len() + frame.planes[1].len());
        packed.extend_from_slice(&frame.planes[0]);
        packed.extend_from_slice(&frame.planes[1]);
        // SAFETY: `packed` holds `height * 1.5` rows of `w` bytes, the texture's layout.
        unsafe { self.device.context.UpdateSubresource(&self.slots[i].texture, 0, None, packed.as_ptr().cast(), w as u32, 0) };
        self.signal(i)
    }

    fn signal(&mut self, i: usize) -> (Arc<DecodedPlanes>, u64) {
        self.value += 1;
        self.slots[i].value = self.value;
        // SAFETY: a signal after the copy on the immediate context; the flush sends both to the GPU.
        unsafe {
            let _ = self.device.context.Signal(&self.ready, self.value);
            self.device.context.Flush();
        }
        (self.slots[i].planes.clone(), self.value)
    }
}

/// A shared NV12 texture made on D3D11 and opened on Rune's D3D12 device.
fn slot(device: &Device, fences: &Arc<VideoFences>, w: u32, h: u32) -> Result<Slot, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
    };
    // SAFETY: a texture of `desc` shared through an NT handle that's closed once D3D12 opened it.
    unsafe {
        let mut texture = None;
        device.device.CreateTexture2D(&desc, None, Some(&mut texture)).map_err(|e| format!("video texture: {}", e.message()))?;
        let texture = texture.ok_or("video texture")?;
        let handle = texture.cast::<IDXGIResource1>().and_then(|r| r.CreateSharedHandle(None, GENERIC_ALL.0, None)).map_err(|e| format!("share video texture: {}", e.message()))?;
        let mut resource: Option<ID3D12Resource> = None;
        let opened = device.d3d12.OpenSharedHandle(handle, &mut resource);
        let _ = CloseHandle(handle);
        opened.map_err(|e| format!("open video texture: {}", e.message()))?;
        let planes = Arc::new(DecodedPlanes {
            resource: resource.ok_or("open video texture")?,
            size: [w, h],
            fences: fences.clone(),
            device: device.d3d12.as_raw() as usize,
        });
        Ok(Slot { texture, planes, value: 0 })
    }
}
