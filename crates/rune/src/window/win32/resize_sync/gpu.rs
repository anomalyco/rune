//! Off-screen D3D12-to-D3D11 shared-texture and shared-fence plumbing for synchronized drag
//! resize, plus the narrow [`DragSeam`] trait isolating HWND-bound presentation and DComp calls.

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

/// Lazily created D3D11 device and cross-API fence pair bound to one Rune D3D12 device.
pub(crate) struct DragDevice {
    pub(super) d3d11: D3d11Device,
    d3d12_raw: usize,
    pub(super) ready: SharedFence,
    pub(super) release: SharedFence,
    next_ready: u64,
    last_release: u64,
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
        let d3d12_raw = d3d12.as_raw() as usize;
        Ok(Self { d3d11, d3d12_raw, ready, release, next_ready: 0, last_release: 0 })
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

    /// Orders subsequent D3D12 work on `queue` after the last D3D11 copy finished reading the
    /// shared texture.
    pub(crate) fn wait_before_d3d12(&self, queue: &wgpu::Queue) -> Result<(), String> {
        if self.last_release == 0 {
            return Ok(());
        }
        // SAFETY: accesses only the raw D3D12 command queue to enqueue a GPU wait on our live
        // shared fence.
        let hal = unsafe { queue.as_hal::<wgpu::hal::api::Dx12>() }
            .ok_or_else(|| "drag resize sync requires D3D12 queue".to_owned())?;
        // SAFETY: `self.release.d3d12` was created on this queue's device.
        unsafe { hal.as_raw().Wait(&self.release.d3d12, self.last_release) }
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

    /// Waits for `ready_value` on D3D11, copies `target` into a CPU-readable D3D11 staging
    /// texture, signals `release`, and returns tightly-packed pixel rows (4 bytes per pixel).
    /// Used by off-screen tests to prove bit-exact D3D12 -> D3D11 transfer without an HWND.
    #[cfg(test)]
    pub(super) fn readback_via_d3d11(
        &mut self,
        target: &SharedDragTarget,
        ready_value: u64,
    ) -> Result<Vec<u8>, String> {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_STAGING,
        };
        use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

        let (width, height) = target.size;
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
        let release_value = self.next_release_value();
        // SAFETY: COM calls on our live D3D11 device and immediate context with matching texture
        // size and format.
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
            self.d3d11.context.CopyResource(&staging, &target.d3d11_texture);
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
            Ok(out)
        }
    }
}

/// One D3D12-rendered, D3D11-readable shared 2D texture wrapped as a `wgpu::Texture`.
pub(crate) struct SharedDragTarget {
    pub(crate) wgpu_texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) d3d11_texture: ID3D11Texture2D,
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
/// machine, D3D12/D3D11 shared target, and lifecycle cleanup can be tested off screen with a fake.
pub(super) trait DragSeam {
    fn present_shared(
        &mut self,
        drag: &mut DragDevice,
        target: &SharedDragTarget,
        ready_value: u64,
    ) -> Result<(), String>;
    fn detach_dcomp(&mut self) -> Result<(), String>;
    fn attach_dcomp(&mut self) -> Result<(), String>;
    fn set_valid_rects_active(&mut self, active: bool);
    fn release_swap_chain(&mut self);
}
