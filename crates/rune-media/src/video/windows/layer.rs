//! A playing clip's own layer: an NV12 swap chain on the decoders' D3D11 device, which the window
//! shows on a DirectComposition visual under its UI while the video's place is still. The decoder
//! thread scales each picture into it with the GPU's video processor and presents it at the
//! picture's time, so playing draws no UI frame per picture.
//!
//! Made as Chromium makes its video swap chains: on a DirectComposition surface handle through
//! `IDXGIFactoryMedia`, which is also the only way to an NV12 swap chain (one the display can scan
//! out as an overlay plane); `CreateSwapChainForComposition` refuses NV12. The window wraps the
//! handle in a surface of its own DirectComposition device (`handle`).

use super::gpu::Device;
use super::mf::{self, Picture};
use crate::native_surface::{Format, Matrix, Orient};
use std::mem::ManuallyDrop;
use std::sync::Arc;
use windows::Win32::Foundation::{CloseHandle, HANDLE, RECT};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::{HRESULT, Interface};

type CreateSurfaceHandle = unsafe extern "system" fn(u32, *const core::ffi::c_void, *mut HANDLE) -> HRESULT;

/// `COMPOSITIONOBJECT_READ | COMPOSITIONOBJECT_WRITE`.
const ALL_ACCESS: u32 = 0x3;

pub(super) struct Layer {
    swap: IDXGISwapChain3,
    /// The DirectComposition surface handle the swap chain presents to; closed on drop.
    handle: HANDLE,
    /// The video's place on screen, in device pixels, as the window asked.
    pub size: (u32, u32),
    /// The swap chain's buffers: `size` with even sides.
    buffer: (u32, u32),
    video: ID3D11VideoDevice,
    context: ID3D11VideoContext1,
    processor: Option<(ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor, ID3D11VideoProcessorOutputView)>,
    /// Input views of the decoder's textures, by texture and array slice.
    inputs: Vec<(usize, u32, ID3D11VideoProcessorInputView)>,
    picture: Picture,
}

// SAFETY: the decoder thread makes, uses and drops a layer; the device is multithread-protected.
unsafe impl Send for Layer {}

impl Drop for Layer {
    fn drop(&mut self) {
        // SAFETY: the handle this layer made and nobody else closes.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

impl Layer {
    /// A layer `size` device pixels for `picture`'s clip. Rotated clips stay in the UI.
    pub(super) fn new(device: Arc<Device>, picture: Picture, size: (u32, u32)) -> Result<Layer, String> {
        if picture.orient != Orient::default() {
            return Err("a rotated clip draws in the UI".into());
        }
        let buffer = even(size);
        let dcomp = mf::system_library("dcomp.dll").ok_or("dcomp.dll is missing")?;
        // SAFETY: the documented entry point at its documented signature; the handle it writes is
        // owned by this layer. The swap chain is made on the decoders' live device.
        unsafe {
            let create: CreateSurfaceHandle = mf::symbol(dcomp, c"DCompositionCreateSurfaceHandle").ok_or("no DCompositionCreateSurfaceHandle")?;
            let mut handle = HANDLE::default();
            create(ALL_ACCESS, std::ptr::null(), &mut handle).ok().map_err(|e| format!("composition surface: {}", e.message()))?;
            let swap = match swap_chain(&device, handle, buffer) {
                Ok(swap) => swap,
                Err(e) => {
                    let _ = CloseHandle(handle);
                    return Err(e);
                }
            };
            let video: ID3D11VideoDevice = device.device.cast().map_err(|_| "no video processor")?;
            let context: ID3D11VideoContext1 = device.context.cast().map_err(|_| "no video processor")?;
            Ok(Layer { swap, handle, size, buffer, video, context, processor: None, inputs: Vec::new(), picture })
        }
    }

    /// The DirectComposition surface handle, for the window's `CreateSurfaceFromHandle`.
    pub(super) fn handle(&self) -> usize {
        self.handle.0 as usize
    }

    /// The video's place changed size: the swap chain follows; the next picture fills it.
    pub(super) fn resize(&mut self, size: (u32, u32)) -> Result<(), String> {
        if size == self.size {
            return Ok(());
        }
        let buffer = even(size);
        // The output view holds the old buffer: it goes before the buffers are made again.
        self.processor = None;
        // SAFETY: no view of the swap chain's buffers is alive.
        unsafe { self.swap.ResizeBuffers(0, buffer.0, buffer.1, DXGI_FORMAT_UNKNOWN, SWAP_FLAGS) }.map_err(|e| format!("resize video layer: {}", e.message()))?;
        self.size = size;
        self.buffer = buffer;
        Ok(())
    }

    /// Scales array slice `index` of the decoder's `texture` into the swap chain's next buffer.
    pub(super) fn blit(&mut self, texture: &ID3D11Texture2D, index: u32) -> Result<(), String> {
        if self.processor.is_none() {
            self.processor = Some(self.processor()?);
        }
        let Some((enumerator, processor, output)) = &self.processor else { return Ok(()) };
        let key = texture.as_raw() as usize;
        let input = match self.inputs.iter().find(|(t, i, _)| *t == key && *i == index) {
            Some((_, _, view)) => view.clone(),
            None => {
                let mut desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC { FourCC: 0, ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D, ..Default::default() };
                desc.Anonymous.Texture2D = D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: index };
                let mut view = None;
                // SAFETY: a view of a live decoder texture on this device.
                unsafe { self.video.CreateVideoProcessorInputView(texture, enumerator, &desc, Some(&mut view)) }.map_err(|e| format!("video input: {}", e.message()))?;
                let view = view.ok_or("video input")?;
                // A decoder cycles a handful of textures; a new decoder's are new keys.
                if self.inputs.len() >= 32 {
                    self.inputs.remove(0);
                }
                self.inputs.push((key, index, view.clone()));
                view
            }
        };
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM { Enable: true.into(), pInputSurface: ManuallyDrop::new(Some(input)), ..Default::default() };
        // SAFETY: one enabled stream whose input view and output view are live on this device.
        let done = unsafe { self.context.VideoProcessorBlt(processor, output, 0, std::slice::from_ref(&stream)) };
        // SAFETY: drops the reference the stream took above, once.
        unsafe { ManuallyDrop::drop(&mut stream.pInputSurface) };
        done.map_err(|e| format!("scale picture: {}", e.message()))
    }

    /// Shows the last blitted picture at the display's next refresh.
    pub(super) fn present(&self) -> Result<(), String> {
        // SAFETY: a present on this layer's swap chain.
        let presented = unsafe { self.swap.Present(1, DXGI_PRESENT(0)) };
        presented.ok().map_err(|e| format!("present video layer: {}", e.message()))
    }

    /// The video processor for this clip and this size, writing into buffer 0 (which D3D11 moves
    /// to each present's next buffer).
    fn processor(&self) -> Result<(ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor, ID3D11VideoProcessorOutputView), String> {
        let (x, y, w, h) = self.picture.crop;
        let rate = DXGI_RATIONAL { Numerator: (1.0 / self.picture.frame.max(1e-3)).round() as u32, Denominator: 1 };
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: self.picture.coded.0,
            InputHeight: self.picture.coded.1,
            OutputFrameRate: rate,
            OutputWidth: self.buffer.0,
            OutputHeight: self.buffer.1,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        // SAFETY: video processor setup on this device; the buffer is the swap chain's own.
        unsafe {
            let enumerator = self.video.CreateVideoProcessorEnumerator(&desc).map_err(|e| format!("video processor: {}", e.message()))?;
            let processor = self.video.CreateVideoProcessor(&enumerator, 0).map_err(|e| format!("video processor: {}", e.message()))?;
            let buffer: ID3D11Texture2D = self.swap.GetBuffer(0).map_err(|e| format!("video layer buffer: {}", e.message()))?;
            let out = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC { ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D, ..Default::default() };
            let mut output = None;
            self.video.CreateVideoProcessorOutputView(&buffer, &enumerator, &out, Some(&mut output)).map_err(|e| format!("video output: {}", e.message()))?;
            let c = &self.context;
            c.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            // No denoising, edge enhancement or other "improvements": the picture as decoded.
            c.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            c.VideoProcessorSetStreamColorSpace1(&processor, 0, input_space(&self.picture));
            c.VideoProcessorSetOutputColorSpace1(&processor, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
            let source = RECT { left: x as i32, top: y as i32, right: (x + w) as i32, bottom: (y + h) as i32 };
            let target = RECT { left: 0, top: 0, right: self.buffer.0 as i32, bottom: self.buffer.1 as i32 };
            c.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&source));
            c.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&target));
            c.VideoProcessorSetOutputTargetRect(&processor, true, Some(&target));
            Ok((enumerator, processor, output.ok_or("video output")?))
        }
    }
}

/// `DXGI_SWAP_CHAIN_FLAG_YUV_VIDEO | DXGI_SWAP_CHAIN_FLAG_FULLSCREEN_VIDEO`, as Chromium's.
const SWAP_FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG(DXGI_SWAP_CHAIN_FLAG_YUV_VIDEO.0 | DXGI_SWAP_CHAIN_FLAG_FULLSCREEN_VIDEO.0);

/// An NV12 swap chain presenting to the composition surface `handle`.
unsafe fn swap_chain(device: &Device, handle: HANDLE, size: (u32, u32)) -> Result<IDXGISwapChain3, String> {
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: size.0,
        Height: size.1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        Flags: SWAP_FLAGS.0 as u32,
        ..Default::default()
    };
    // SAFETY: the decoders' live device and a composition surface handle this layer owns.
    unsafe {
        let factory: IDXGIFactoryMedia = device.device.cast::<IDXGIDevice>().and_then(|d| d.GetAdapter()).and_then(|a| a.GetParent()).map_err(|e| format!("DXGI media factory: {}", e.message()))?;
        let swap: IDXGISwapChain3 = factory
            .CreateSwapChainForCompositionSurfaceHandle(&device.device, Some(handle), &desc, None)
            .and_then(|s| s.cast())
            .map_err(|e| format!("video layer: {}", e.message()))?;
        swap.SetColorSpace1(DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709).map_err(|e| format!("video layer colours: {}", e.message()))?;
        Ok(swap)
    }
}

/// The decoder's YCbCr encoding as DXGI names it.
fn input_space(picture: &Picture) -> DXGI_COLOR_SPACE_TYPE {
    match (picture.color.matrix, picture.format) {
        (Matrix::Bt709, Format::Nv12Full) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709,
        (Matrix::Bt709, _) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
        (Matrix::Bt601, Format::Nv12Full) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601,
        (Matrix::Bt601, _) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
    }
}

/// NV12 needs even sides.
fn even((w, h): (u32, u32)) -> (u32, u32) {
    ((w.max(2) + 1) & !1, (h.max(2) + 1) & !1)
}
