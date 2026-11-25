//! Where decoded pictures live on the GPU. Media Foundation decodes on a D3D11 device (its DXVA
//! decoders take a DXGI device manager), made once per process on the adapter Rune draws with.
//! Each player keeps a small ring of shared NV12 textures: the decoder's output is copied into a
//! free one on the GPU, the copy signals a fence, and Rune's D3D12 device samples the texture in
//! place (`native_surface::windows::DecodedPlanes`). Pictures never pass through the CPU.

#![allow(unsafe_code)]

use super::mf::{self, Picture};
use crate::native_surface::windows::{DecodedPlanes, VideoFences, device_generation, presenting_device};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, LUID};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D12::{D3D12_FENCE_FLAG_NONE, ID3D12Device, ID3D12Fence, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIResource1;
use windows::Win32::Media::MediaFoundation::IMFDXGIDeviceManager;
use windows::core::Interface;

/// Pictures a player's ring holds: the few the decoder runs ahead, the one on screen, one the GPU
/// may still be reading and a spare.
pub(super) const RING: usize = 6;

/// The D3D11 device decoders share, on the adapter of Rune's D3D12 device.
pub(super) struct Device {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext4,
    device5: ID3D11Device5,
    /// Rune's device, which opens the ring's textures and owns the fences.
    pub d3d12: ID3D12Device,
    luid: LUID,
    /// `device_generation` when `d3d12` was Rune's device: a ring made on an older one is
    /// checked against the current device (`Ring::stale`).
    generation: u64,
}

static DEVICE: Mutex<Option<Arc<Device>>> = Mutex::new(None);

/// The decoders' D3D11 device, made on first use and kept; None when Rune draws without D3D12
/// or the adapter can't make one (decoding then runs on the CPU).
pub(super) fn device() -> Result<Arc<Device>, String> {
    // The generation first: a device made after this read makes the result stale at once.
    let generation = device_generation();
    let d3d12 = presenting_device().ok_or("no D3D12 device draws a window")?;
    // SAFETY: a plain query on a live device.
    let luid = unsafe { d3d12.GetAdapterLuid() };
    let mut slot = DEVICE.lock().unwrap();
    if let Some(device) = slot.as_ref().filter(|d| d.luid.LowPart == luid.LowPart && d.luid.HighPart == luid.HighPart) {
        if device.d3d12.as_raw() == d3d12.as_raw() && device.generation == generation {
            return Ok(device.clone());
        }
        if device.d3d12.as_raw() == d3d12.as_raw() {
            // Only the count moved (another window noted the same device).
            let current = Arc::new(Device { device: device.device.clone(), context: device.context.clone(), device5: device.device5.clone(), d3d12, luid, generation });
            *slot = Some(current.clone());
            return Ok(current);
        }
        // Rune replaced a lost device. Whatever lost it (a removed device, a driver update, a
        // TDR) leaves the decoders' D3D11 device unusable too, so everything is made again.
    }
    let device = Arc::new(make(d3d12, luid, generation)?);
    *slot = Some(device.clone());
    Ok(device)
}

/// Drops the kept decoder device once Rune's device under it is removed: it holds that device.
/// Decoders still holding their own reference let go as they see the loss (`lost_since`).
pub(crate) fn release_removed_device() {
    let mut slot = DEVICE.lock().unwrap();
    // SAFETY: a plain query on a device this slot holds.
    if slot.as_ref().is_some_and(|d| unsafe { d.d3d12.GetDeviceRemovedReason() }.is_err()) {
        *slot = None;
    }
}

/// Rune's device was replaced since `generation`, or the one it presents with is removed.
pub(super) fn lost_since(generation: u64) -> bool {
    // SAFETY: a plain query on a live device.
    device_generation() != generation || presenting_device().is_some_and(|d| unsafe { d.GetDeviceRemovedReason() }.is_err())
}

fn make(d3d12: ID3D12Device, luid: LUID, generation: u64) -> Result<Device, String> {
    let d3d11 = rune_gpu::d3d11_interop::create_d3d11_device_for_luid(luid, true, true)?;
    Ok(Device { device: d3d11.device, context: d3d11.context, device5: d3d11.device5, d3d12, luid, generation })
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

/// A slot's `value` once its picture is known never to reach Rune (`Ring::unread`), or before
/// it has held one.
pub(super) const UNREAD: u64 = 0;

/// Whether a slot can take a new picture: only the ring holds its last one (`holders`), and
/// Rune's draw of that one or of a later one is done (`released`), or none will ever come.
pub(super) fn writable(holders: usize, value: u64, released: u64) -> bool {
    holders == 1 && (value == UNREAD || released >= value)
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
    /// The decoders' device this ring's textures are on.
    pub(super) fn device(&self) -> Arc<Device> {
        self.device.clone()
    }

    /// Rune made a new device since this ring opened its textures and fences on the old one.
    pub(super) fn stale(&self) -> bool {
        self.device.generation != device_generation()
    }

    /// Takes `device` when it's the device this ring opened its textures on (only the count
    /// moved); false when the ring must be made again.
    pub(super) fn adopt(&mut self, device: &Arc<Device>) -> bool {
        let same = device.d3d12.as_raw() == self.device.d3d12.as_raw();
        if same {
            self.device = device.clone();
        }
        same
    }

    pub(super) fn new(device: Arc<Device>, picture: &Picture) -> Result<Self, String> {
        let shared = rune_gpu::d3d11_interop::create_shared_fence(&device.d3d12, &device.device5)?;
        // SAFETY: plain non-shared D3D12 fence on Rune's live device.
        let release: ID3D12Fence = unsafe { device.d3d12.CreateFence(0, D3D12_FENCE_FLAG_NONE) }
            .map_err(|e| format!("fence: {}", e.message()))?;
        let fences = Arc::new(VideoFences { ready: shared.d3d12, release });
        Ok(Self { device, slots: Vec::with_capacity(RING), fences, ready: shared.d3d11, value: 0, crop: picture.crop })
    }

    /// Whether a picture could be written now: a slot is free, or there's room for another.
    pub(super) fn free(&self) -> bool {
        self.slots.len() < RING || self.reusable().is_some()
    }

    /// A slot no picture on screen, in a queue or in flight on the GPU uses.
    fn reusable(&self) -> Option<usize> {
        // SAFETY: a plain read of the fence's value.
        let released = unsafe { self.fences.release.GetCompletedValue() };
        self.slots.iter().position(|s| writable(Arc::strong_count(&s.planes), s.value, released))
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

    /// The picture written as `value` never reached Rune (dropped as an earlier seek's or as
    /// late, or its layer showed it instead): its slot is free once the last reference goes, with
    /// no read of Rune's to wait for.
    pub(super) fn unread(&mut self, planes: &Arc<DecodedPlanes>, value: u64) {
        if let Some(slot) = self.slots.iter_mut().find(|s| Arc::ptr_eq(&s.planes, planes) && s.value == value) {
            slot.value = UNREAD;
        }
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
            self.device.context.Signal(&self.ready, self.value).ok();
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
        CloseHandle(handle).ok();
        opened.map_err(|e| format!("open video texture: {}", e.message()))?;
        let planes = Arc::new(DecodedPlanes {
            resource: resource.ok_or("open video texture")?,
            size: [w, h],
            fences: fences.clone(),
            device: device.d3d12.as_raw() as usize,
        });
        Ok(Slot { texture, planes, value: UNREAD })
    }
}

#[cfg(test)]
mod tests;
