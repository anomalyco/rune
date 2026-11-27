//! Off-screen D3D12-to-D3D11 shared-texture ring and shared-fence plumbing for synchronized drag
//! resize, plus the narrow [`DragSeam`] trait isolating HWND-bound presentation and DComp calls.
//!
//! Non-compositor apps render into the top-left `(width, height)` sub-rectangle of a capacited,
//! double-buffered shared texture ring (`RING_SLOTS = 2`, grown in 256 px steps) so ordinary
//! resize steps allocate zero D3D12/D3D11 textures or NT handles and D3D12 does not stall on the
//! immediately preceding frame's D3D11 copy.

#![allow(unsafe_code)]

use rune_gpu::d3d11_interop::{
    D3d11Device, SharedFence, SharedRenderTarget, create_d3d11_device_for_luid,
    create_shared_fence, create_shared_render_target, d3d12_device_luid,
};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
};
use windows::core::Interface;

/// Allocation granularity (in pixels) for capacited drag shared textures so drag steps inside the
/// allocated bounds create zero textures or shared handles.
pub(crate) const CAPACITY_STEP: u32 = 256;

/// Number of shared render-target slots in [`DragTargetRing`]. Two slots let D3D12 render frame
/// `N + 1` without waiting for D3D11 to finish copying frame `N`.
pub(crate) const RING_SLOTS: usize = 2;

/// Computes the shared texture capacity for `needed` under `max_dim`.
///
/// When `exact` is true (custom compositors that may assume `target.texture().size() == size`),
/// returns `needed` unchanged. Otherwise reuses `current` whenever it already covers `needed`,
/// and grows only the overflowing axis in [`CAPACITY_STEP`] increments with headroom, clamped to
/// `max_dim`.
pub(crate) fn grow_capacity(
    current: Option<(u32, u32)>,
    needed: (u32, u32),
    max_dim: u32,
    exact: bool,
) -> (u32, u32) {
    let max_dim = max_dim.max(1);
    let need_w = needed.0.clamp(1, max_dim);
    let need_h = needed.1.clamp(1, max_dim);
    if exact {
        return (need_w, need_h);
    }
    if let Some((cw, ch)) = current
        && cw >= need_w
        && ch >= need_h
    {
        return (cw, ch);
    }
    let grow_axis = |cur: u32, need: u32| -> u32 {
        if cur >= need {
            cur.min(max_dim)
        } else {
            need.saturating_add(CAPACITY_STEP)
                .next_multiple_of(CAPACITY_STEP)
                .min(max_dim)
                .max(need)
        }
    };
    let (cw, ch) = current.unwrap_or((0, 0));
    (grow_axis(cw, need_w), grow_axis(ch, need_h))
}

/// Lazily created D3D11 device and cross-API fence pair bound to one Rune D3D12 device.
pub(crate) struct DragDevice {
    pub(super) d3d11: D3d11Device,
    d3d12_raw: usize,
    pub(super) ready: SharedFence,
    pub(super) release: SharedFence,
    /// Auto-reset Win32 event waited on once per drag (`BeginDrag`) to confirm the first D3D11
    /// `Present(0, 0)` blt finished on the GPU before detaching DirectComposition.
    fence_event: *mut std::ffi::c_void,
    next_ready: u64,
    last_release: u64,
}

impl Drop for DragDevice {
    fn drop(&mut self) {
        if !self.fence_event.is_null() {
            // SAFETY: `fence_event` is an owned Win32 event handle created by `CreateEventW`.
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.fence_event);
            }
            self.fence_event = std::ptr::null_mut();
        }
    }
}

impl DragDevice {
    /// Creates a D3D11 device on `device`'s exact DXGI adapter and a pair of shared fences
    /// (`ready`: D3D12 signals -> D3D11 waits; `release`: D3D11 signals -> D3D12 waits).
    pub(crate) fn for_wgpu(device: &wgpu::Device) -> Result<Self, String> {
        let (d3d12, luid) =
            d3d12_device_luid(device).ok_or_else(|| "drag resize sync requires D3D12".to_owned())?;
        let d3d11 = create_d3d11_device_for_luid(luid, false, false)?;
        let ready = create_shared_fence(&d3d12, &d3d11.device5)?;
        let release = create_shared_fence(&d3d12, &d3d11.device5)?;
        // SAFETY: creates an unnamed, unsignaled auto-reset Win32 event owned by `Self`.
        let fence_event = unsafe {
            windows_sys::Win32::System::Threading::CreateEventW(
                std::ptr::null(),
                0,
                0,
                std::ptr::null(),
            )
        };
        if fence_event.is_null() {
            return Err("CreateEventW for drag resize sync fence failed".into());
        }
        let d3d12_raw = d3d12.as_raw() as usize;
        Ok(Self { d3d11, d3d12_raw, ready, release, fence_event, next_ready: 0, last_release: 0 })
    }

    /// Whether this D3D11 device matches `device`'s live D3D12 device.
    pub(crate) fn matches(&self, device: &wgpu::Device) -> bool {
        let Some((d3d12, _)) = d3d12_device_luid(device) else { return false };
        if d3d12.as_raw() as usize != self.d3d12_raw {
            return false;
        }
        // SAFETY: plain query on a live D3D12 device.
        unsafe { d3d12.GetDeviceRemovedReason() }.is_ok()
    }

    /// Orders subsequent D3D12 work on `queue` after D3D11 finished reading the target slot
    /// (`release_value == 0` when the slot has never been read yet).
    pub(crate) fn wait_before_d3d12(&self, queue: &wgpu::Queue, release_value: u64) -> Result<(), String> {
        if release_value == 0 {
            return Ok(());
        }
        // SAFETY: accesses only the raw D3D12 command queue to enqueue a GPU wait on our live
        // shared fence.
        let hal = unsafe { queue.as_hal::<wgpu::hal::api::Dx12>() }
            .ok_or_else(|| "drag resize sync requires D3D12 queue".to_owned())?;
        // SAFETY: `self.release.d3d12` was created on this queue's device.
        unsafe { hal.as_raw().Wait(&self.release.d3d12, release_value) }
            .map_err(|e| format!("D3D12 queue wait on release fence: {e}"))
    }

    /// Signals the shared `ready` fence on `queue` after D3D12 submitted a drag frame, returning
    /// the monotonic fence value D3D11 must wait on before reading the shared texture.
    pub(crate) fn signal_after_d3d12(&mut self, queue: &wgpu::Queue) -> Result<u64, String> {
        self.next_ready += 1;
        let value = self.next_ready;
        // SAFETY: accesses only the raw D3D12 command queue to enqueue a GPU signal on our live
        // shared fence.
        let hal = unsafe { queue.as_hal::<wgpu::hal::api::Dx12>() }
            .ok_or_else(|| "drag resize sync requires D3D12 queue".to_owned())?;
        // SAFETY: `self.ready.d3d12` was created on this queue's device.
        unsafe { hal.as_raw().Signal(&self.ready.d3d12, value) }
            .map_err(|e| format!("D3D12 queue signal on ready fence: {e}"))?;
        Ok(value)
    }

    /// Advances and returns the next release-fence value for D3D11 to signal after copying the
    /// shared texture.
    pub(super) fn next_release_value(&mut self) -> u64 {
        self.last_release += 1;
        self.last_release
    }

    /// Enqueues a post-present fence signal on the D3D11 immediate context and flushes it so the
    /// caller can wait for a copy-model `Present(0, 0)` blt into the HWND redirection surface to
    /// complete on the GPU before detaching DirectComposition (`BeginDrag` only).
    pub(super) fn signal_post_present(&mut self) -> Result<u64, String> {
        let value = self.next_release_value();
        // SAFETY: `self.release.d3d11` was opened on this live D3D11 device.
        unsafe {
            self.d3d11
                .context
                .Signal(&self.release.d3d11, value)
                .map_err(|e| format!("D3D11 post-present signal: {e}"))?;
            self.d3d11.context.Flush();
        }
        Ok(value)
    }

    /// Blocks on the CPU (up to a bounded safety timeout) until the D3D11 release fence reaches
    /// `value`. Used once per drag (`BeginDrag`) right before `detach_dcomp` so DWM never reveals
    /// a stale redirection surface from a previous drag or startup frame before the first drag
    /// frame's `Present(0, 0)` blt has completed on the GPU.
    pub(super) fn wait_for_release(&self, value: u64) -> Result<(), String> {
        if value == 0 {
            return Ok(());
        }
        const WAIT_TIMEOUT_MS: u32 = 1_000;
        // SAFETY: `self.release.d3d11` and `self.fence_event` are owned by `self` and live for
        // the duration of this synchronous wait.
        unsafe {
            if self.release.d3d11.GetCompletedValue() >= value {
                return Ok(());
            }
            self.release
                .d3d11
                .SetEventOnCompletion(value, windows::Win32::Foundation::HANDLE(self.fence_event))
                .map_err(|e| format!("D3D11 release fence SetEventOnCompletion: {e}"))?;
            let waited = windows_sys::Win32::System::Threading::WaitForSingleObject(
                self.fence_event,
                WAIT_TIMEOUT_MS,
            );
            if waited == windows_sys::Win32::Foundation::WAIT_OBJECT_0 {
                Ok(())
            } else {
                Err(format!("D3D11 present fence wait timed out or failed ({waited:#x})"))
            }
        }
    }

    /// Waits for `ready_value` on D3D11, copies the top-left `copy_size` sub-rectangle of `target`
    /// into a CPU-readable D3D11 staging texture, signals `release`, and returns `(pixels,
    /// release_value)` with tightly-packed 4-byte-per-pixel rows.
    /// Used by off-screen tests to prove bit-exact sub-rect D3D12 -> D3D11 transfer without an HWND.
    #[cfg(test)]
    pub(super) fn readback_via_d3d11(
        &mut self,
        target: &SharedDragTarget,
        copy_size: (u32, u32),
        ready_value: u64,
    ) -> Result<(Vec<u8>, u64), String> {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
            D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
        };
        use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

        let (width, height) = copy_size;
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: target.dxgi_format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let src_box = D3D11_BOX { left: 0, top: 0, front: 0, right: width, bottom: height, back: 1 };
        let release_value = self.next_release_value();
        // SAFETY: COM calls on our live D3D11 device and immediate context; `src_box` lies inside
        // `target.d3d11_texture` (`target.size >= copy_size`).
        unsafe {
            let mut staging = None;
            self.d3d11
                .device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .map_err(|e| format!("staging texture: {e}"))?;
            let staging = staging.ok_or("staging texture")?;
            self.d3d11
                .context
                .Wait(&self.ready.d3d11, ready_value)
                .map_err(|e| format!("D3D11 wait: {e}"))?;
            self.d3d11.context.CopySubresourceRegion(
                &staging,
                0,
                0,
                0,
                0,
                &target.d3d11_texture,
                0,
                Some(&src_box),
            );
            self.d3d11
                .context
                .Signal(&self.release.d3d11, release_value)
                .map_err(|e| format!("D3D11 signal: {e}"))?;
            self.d3d11.context.Flush();
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.d3d11
                .context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| format!("D3D11 map: {e}"))?;
            let row_bytes = (width as usize) * 4;
            let pitch = mapped.RowPitch as usize;
            let mut out = vec![0u8; row_bytes * (height as usize)];
            for y in 0..height as usize {
                let src = std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * pitch),
                    row_bytes,
                );
                out[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(src);
            }
            self.d3d11.context.Unmap(&staging, 0);
            Ok((out, release_value))
        }
    }

    /// Creates a plain default-usage BGRA8 D3D11 texture for off-screen GPU-copy tests.
    #[cfg(test)]
    pub(super) fn create_test_dst_texture(&self, size: (u32, u32)) -> Result<ID3D11Texture2D, String> {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_BIND_RENDER_TARGET, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        };
        use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.0,
            Height: size.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        // SAFETY: plain default-usage D3D11 texture on our live test D3D11 device.
        unsafe {
            let mut raw = None;
            self.d3d11
                .device
                .CreateTexture2D(&desc, None, Some(&mut raw))
                .map_err(|e| format!("test dst texture: {e}"))?;
            raw.ok_or_else(|| "test dst texture".to_owned())
        }
    }

    /// GPU-only D3D11 copy without CPU map/readback, for measuring steady-state off-screen
    /// D3D12 -> D3D11 shared-texture transfer latency.
    #[cfg(test)]
    pub(super) fn gpu_copy_only_via_d3d11(
        &mut self,
        target: &SharedDragTarget,
        dst: &ID3D11Texture2D,
        copy_size: (u32, u32),
        ready_value: u64,
    ) -> Result<u64, String> {
        use windows::Win32::Graphics::Direct3D11::D3D11_BOX;
        let src_box = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: copy_size.0,
            bottom: copy_size.1,
            back: 1,
        };
        let release_value = self.next_release_value();
        // SAFETY: live D3D11 context and textures; `src_box` lies within both.
        unsafe {
            self.d3d11
                .context
                .Wait(&self.ready.d3d11, ready_value)
                .map_err(|e| format!("D3D11 wait: {e}"))?;
            self.d3d11.context.CopySubresourceRegion(
                dst,
                0,
                0,
                0,
                0,
                &target.d3d11_texture,
                0,
                Some(&src_box),
            );
            self.d3d11
                .context
                .Signal(&self.release.d3d11, release_value)
                .map_err(|e| format!("D3D11 signal: {e}"))?;
            self.d3d11.context.Flush();
        }
        Ok(release_value)
    }
}

/// One D3D12-rendered, D3D11-readable shared 2D texture wrapped as a `wgpu::Texture`.
pub(crate) struct SharedDragTarget {
    pub(crate) wgpu_texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) d3d11_texture: ID3D11Texture2D,
    /// Allocated capacity `(width, height)` in pixels (`>= frame_size`).
    #[cfg(test)]
    pub(crate) size: (u32, u32),
    pub(crate) format: wgpu::TextureFormat,
    #[cfg(test)]
    pub(super) dxgi_format: DXGI_FORMAT,
}

impl SharedDragTarget {
    /// Creates a shared render-target texture of `size` and `format` on `device`, opens it on
    /// `drag`'s D3D11 device, and wraps the D3D12 resource as a `wgpu::Texture`.
    pub(crate) fn new(
        device: &wgpu::Device,
        drag: &DragDevice,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> Result<Self, String> {
        let dxgi_format = dxgi_format_for(format)?;
        let (d3d12, _) =
            d3d12_device_luid(device).ok_or_else(|| "drag resize sync requires D3D12".to_owned())?;
        let SharedRenderTarget { d3d12: resource12, d3d11: d3d11_texture, .. } =
            create_shared_render_target(&d3d12, &drag.d3d11.device1, size, dxgi_format)?;
        let extent = wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 };
        let desc = wgpu::TextureDescriptor {
            label: Some("rune drag resize shared"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        // SAFETY: `resource12` is a single-mip, single-sample 2D texture created on `device`'s
        // raw D3D12 device with matching `size` and `format`, starting in `COMMON` (which wgpu's
        // DX12 backend models as `TextureUses::PRESENT`).
        let wgpu_texture = unsafe {
            let hal = wgpu::hal::dx12::Device::texture_from_raw(
                resource12,
                desc.format,
                desc.dimension,
                desc.size,
                desc.mip_level_count,
                desc.sample_count,
            );
            device.create_texture_from_hal::<wgpu::hal::api::Dx12>(
                hal,
                &desc,
                wgpu::TextureUses::PRESENT,
            )
        };
        let view = wgpu_texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            wgpu_texture,
            view,
            d3d11_texture,
            #[cfg(test)]
            size,
            format,
            #[cfg(test)]
            dxgi_format,
        })
    }

    /// Transitions the shared texture back to `COMMON` (`TextureUses::PRESENT`) at the end of a
    /// D3D12 command encoder so render-target caches flush before D3D11 reads it.
    pub(crate) fn transition_for_d3d11(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: &self.wgpu_texture,
                selector: None,
                state: wgpu::TextureUses::PRESENT,
            }),
        );
    }
}

/// One slot of [`DragTargetRing`], pairing a shared texture with its last D3D11 release fence.
pub(crate) struct DragSlot {
    pub(crate) target: SharedDragTarget,
    pub(crate) last_release: u64,
}

/// Bounded 2-slot capacited ring of shared drag render targets.
#[derive(Default)]
pub(crate) struct DragTargetRing {
    slots: [Option<DragSlot>; RING_SLOTS],
    next_slot: usize,
    capacity: Option<(u32, u32)>,
    format: Option<wgpu::TextureFormat>,
    exact: bool,
    #[cfg(test)]
    pub(super) allocations: u64,
}

impl DragTargetRing {
    /// Acquires the next slot covering `size` and `format`. Reuses existing capacited textures
    /// without allocation whenever `capacity >= size` and `format` matches.
    pub(crate) fn acquire_slot(
        &mut self,
        device: &wgpu::Device,
        drag: &DragDevice,
        size: (u32, u32),
        format: wgpu::TextureFormat,
        exact: bool,
    ) -> Result<(usize, &DragSlot), String> {
        let max_dim = device.limits().max_texture_dimension_2d.max(1);
        let compatible = self.format == Some(format)
            && self.exact == exact
            && match self.capacity {
                Some(cap) if exact => cap == size,
                Some(cap) => cap.0 >= size.0 && cap.1 >= size.1,
                None => false,
            };
        if !compatible {
            let prev_cap = if self.format == Some(format) && !exact && !self.exact {
                self.capacity
            } else {
                None
            };
            let next_cap = grow_capacity(prev_cap, size, max_dim, exact);
            self.slots = [None, None];
            self.capacity = Some(next_cap);
            self.format = Some(format);
            self.exact = exact;
            self.next_slot = 0;
        }
        let cap = self.capacity.ok_or("drag target ring capacity missing")?;
        let idx = self.next_slot;
        self.next_slot = (self.next_slot + 1) % RING_SLOTS;
        if self.slots[idx].is_none() {
            let target = SharedDragTarget::new(device, drag, cap, format)?;
            #[cfg(test)]
            { self.allocations += 1; }
            self.slots[idx] = Some(DragSlot { target, last_release: 0 });
        }
        let slot = self.slots[idx].as_ref().ok_or("drag target slot missing")?;
        Ok((idx, slot))
    }

    /// Borrows slot `idx` after it was acquired.
    pub(crate) fn slot(&self, idx: usize) -> Option<&DragSlot> {
        self.slots.get(idx)?.as_ref()
    }

    /// Records the D3D11 release-fence value for the copy that just read slot `idx`.
    pub(crate) fn note_release(&mut self, idx: usize, release_value: u64) {
        if let Some(slot) = self.slots.get_mut(idx).and_then(Option::as_mut) {
            slot.last_release = release_value;
        }
    }

    /// Releases every shared texture in the ring.
    pub(crate) fn release(&mut self) {
        self.slots = [None, None];
        self.capacity = None;
        self.format = None;
        self.exact = false;
        self.next_slot = 0;
    }

    /// Whether zero shared textures are currently allocated.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// Current allocated texture capacity, if any.
    #[cfg(test)]
    pub(super) fn capacity(&self) -> Option<(u32, u32)> {
        self.capacity
    }
}

/// Maps Rune's swap-chain target formats to their DXGI equivalents.
pub(crate) fn dxgi_format_for(format: wgpu::TextureFormat) -> Result<DXGI_FORMAT, String> {
    match format {
        wgpu::TextureFormat::Bgra8Unorm => Ok(DXGI_FORMAT_B8G8R8A8_UNORM),
        wgpu::TextureFormat::Bgra8UnormSrgb => Ok(DXGI_FORMAT_B8G8R8A8_UNORM_SRGB),
        wgpu::TextureFormat::Rgba8Unorm => Ok(DXGI_FORMAT_R8G8B8A8_UNORM),
        wgpu::TextureFormat::Rgba8UnormSrgb => Ok(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB),
        other => Err(format!("drag resize sync does not support surface format {other:?}")),
    }
}

/// Narrow seam isolating the HWND-bound presentation and DirectComposition calls so the state
/// machine, D3D12/D3D11 shared target ring, frame telemetry, and lifecycle cleanup can be tested
/// off screen with a fake.
pub(super) trait DragSeam {
    fn present_shared(
        &mut self,
        drag: &mut DragDevice,
        target: &SharedDragTarget,
        copy_size: (u32, u32),
        ready_value: u64,
        wait_for_present: bool,
    ) -> Result<u64, String>;
    fn detach_dcomp(&mut self) -> Result<(), String>;
    fn attach_dcomp(&mut self) -> Result<(), String>;
    fn set_valid_rects_active(&mut self, active: bool);
    fn release_swap_chain(&mut self);
}
