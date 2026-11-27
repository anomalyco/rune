//! Thin HWND-bound implementation of [`super::gpu::DragSeam`]: a D3D11
//! `DXGI_SWAP_EFFECT_DISCARD` copy-model HWND swap chain presented inside `WM_PAINT`
//! (`BeginPaint`/`EndPaint`) paired with `Handover::detach_root` / `Handover::attach_root`.
//!
//! Kept deliberately minimal because it requires a live Win32 `HWND` inside a live modal
//! size-move loop. Per Microsoft Learn (`DXGI_SWAP_CHAIN_DESC1`, `DXGI_SCALING`), copy-model
//! `DXGI_SWAP_EFFECT_DISCARD` requires `DXGI_SCALING_STRETCH`, so the HWND swap chain resizes to
//! `copy_size` on each size step while copying only the `(0, 0, copy_size.0, copy_size.1)`
//! sub-rectangle from Rune's capacited shared texture.

#![allow(unsafe_code)]

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{D3D11_BOX, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    IDXGISwapChain1,
};
use windows::core::BOOL;

use super::gpu::{DragDevice, DragSeam, SharedDragTarget};
use crate::platform::win32::dcomp_handover::Handover;

/// Copy-model (`DXGI_SWAP_EFFECT_DISCARD`) D3D11 HWND swap chain owned only for the duration of
/// an active resize drag.
pub(super) struct DxgiDragSwapChain {
    swap_chain: IDXGISwapChain1,
    size: (u32, u32),
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
}

impl DxgiDragSwapChain {
    fn new(
        drag: &DragDevice,
        hwnd: HWND,
        size: (u32, u32),
        wgpu_format: wgpu::TextureFormat,
    ) -> Result<Self, String> {
        let format = swap_chain_format(wgpu_format);
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.0,
            Height: size.1,
            Format: format,
            Stereo: BOOL(0),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 1,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };
        // SAFETY: creates a documented copy-model swap chain for a live HWND on our D3D11 device.
        let swap_chain = unsafe {
            let sc = drag
                .d3d11
                .factory2
                .CreateSwapChainForHwnd(&drag.d3d11.device, hwnd, &desc, None, None)
                .map_err(|e| format!("CreateSwapChainForHwnd(DISCARD): {e}"))?;
            drag.d3d11.factory2.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER).ok();
            sc
        };
        Ok(Self { swap_chain, size, format })
    }

    fn ensure_size(
        &mut self,
        size: (u32, u32),
        wgpu_format: wgpu::TextureFormat,
    ) -> Result<(), String> {
        let format = swap_chain_format(wgpu_format);
        if self.size == size && self.format == format {
            return Ok(());
        }
        // SAFETY: no back-buffer reference is retained between presents.
        unsafe {
            self.swap_chain
                .ResizeBuffers(1, size.0, size.1, format, DXGI_SWAP_CHAIN_FLAG(0))
                .map_err(|e| format!("drag swap chain ResizeBuffers: {e}"))?;
        }
        self.size = size;
        self.format = format;
        Ok(())
    }

    fn copy_and_present(
        &mut self,
        drag: &mut DragDevice,
        target: &SharedDragTarget,
        copy_size: (u32, u32),
        ready_value: u64,
        wait_for_present: bool,
    ) -> Result<(u64, Option<u64>), String> {
        let src_box = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: copy_size.0,
            bottom: copy_size.1,
            back: 1,
        };
        let release_value = drag.next_release_value();
        // SAFETY: waits on the shared ready fence, copies the `(0, 0)..copy_size` sub-rectangle
        // from the capacited shared D3D12 texture into buffer 0 of the DISCARD swap chain, drops
        // buffer 0 immediately, signals release, and presents with sync interval 0.
        unsafe {
            drag.d3d11
                .context
                .Wait(&drag.ready.d3d11, ready_value)
                .map_err(|e| format!("D3D11 context Wait: {e}"))?;
            let back_buffer: ID3D11Texture2D = self
                .swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("drag swap chain GetBuffer(0): {e}"))?;
            drag.d3d11.context.CopySubresourceRegion(
                &back_buffer,
                0,
                0,
                0,
                0,
                &target.d3d11_texture,
                0,
                Some(&src_box),
            );
            drop(back_buffer);
            drag.d3d11
                .context
                .Signal(&drag.release.d3d11, release_value)
                .map_err(|e| format!("D3D11 context Signal: {e}"))?;
            self.swap_chain
                .Present(0, DXGI_PRESENT(0))
                .ok()
                .map_err(|e| format!("drag swap chain Present: {e}"))?;
        }
        let present_done = if wait_for_present { Some(drag.signal_post_present()?) } else { None };
        Ok((release_value, present_done))
    }
}

/// DXGI swap chains always take a non-sRGB UNORM back-buffer format; `CopySubresourceRegion`
/// copies raw bytes from either the UNORM or SRGB shared texture in the same typeless family
/// without re-encoding.
fn swap_chain_format(
    format: wgpu::TextureFormat,
) -> windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT {
    match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => {
            DXGI_FORMAT_R8G8B8A8_UNORM
        }
        _ => DXGI_FORMAT_B8G8R8A8_UNORM,
    }
}

/// Live Win32 seam borrowing the window's HWND, DirectComposition `Handover`, and drag swap
/// chain slot for one transition.
pub(super) struct Win32DragSeam<'a> {
    pub(super) hwnd: windows_sys::Win32::Foundation::HWND,
    pub(super) handover: Option<&'a Handover>,
    pub(super) swap_chain: &'a mut Option<DxgiDragSwapChain>,
    pub(super) dcomp_detached: &'a mut bool,
}

impl DragSeam for Win32DragSeam<'_> {
    fn present_shared(
        &mut self,
        drag: &mut DragDevice,
        target: &SharedDragTarget,
        copy_size: (u32, u32),
        ready_value: u64,
        wait_for_present: bool,
    ) -> Result<u64, String> {
        let win_hwnd = HWND(self.hwnd);
        if self.swap_chain.is_none() {
            *self.swap_chain = Some(DxgiDragSwapChain::new(drag, win_hwnd, copy_size, target.format)?);
        }
        let sc = self.swap_chain.as_mut().ok_or("drag swap chain missing")?;
        // Resize the copy-model swap chain before `BeginPaint` so the paint critical section only
        // executes the GPU wait, sub-rect copy, release signal, and `Present(0, 0)`.
        sc.ensure_size(copy_size, target.format)?;
        let mut present_result: Option<Result<(u64, Option<u64>), String>> = None;
        let ran = crate::platform::win32::run_in_paint(self.hwnd, || {
            present_result = Some(sc.copy_and_present(
                drag,
                target,
                copy_size,
                ready_value,
                wait_for_present,
            ));
        });
        let (release_value, present_done) = match present_result {
            Some(result) => result?,
            // If `UpdateWindow` found nothing to paint (for example a transiently empty clip),
            // present directly so the copy-model back buffer still receives the frame.
            None if !ran => sc.copy_and_present(drag, target, copy_size, ready_value, wait_for_present)?,
            None => (0, None),
        };
        // On `BeginDrag`, wait outside `BeginPaint..EndPaint` until the GPU has finished the
        // D3D12 render, D3D11 copy, and DXGI `Present(0, 0)` blt into the HWND redirection surface
        // so the subsequent `detach_dcomp` never exposes a stale pre-drag redirection bitmap.
        if let Some(done) = present_done {
            drag.wait_for_release(done)?;
            return Ok(done);
        }
        Ok(release_value)
    }

    fn detach_dcomp(&mut self) -> Result<(), String> {
        if *self.dcomp_detached {
            return Ok(());
        }
        let Some(handover) = self.handover else {
            return Err("no DirectComposition handover to detach".into());
        };
        handover.detach_root()?;
        *self.dcomp_detached = true;
        Ok(())
    }

    fn attach_dcomp(&mut self) -> Result<(), String> {
        if !*self.dcomp_detached {
            return Ok(());
        }
        *self.dcomp_detached = false;
        if let Some(handover) = self.handover {
            handover.attach_root()?;
        }
        Ok(())
    }

    fn set_valid_rects_active(&mut self, active: bool) {
        crate::platform::win32::set_resize_sync_active(self.hwnd, active);
    }

    fn release_swap_chain(&mut self) {
        *self.swap_chain = None;
    }
}
