//! Buffers, textures, views, the command encoder and its render passes, and the inert handles
//! that stand in for everything the spike doesn't draw with.

use std::future::ready;
use std::ops::Range;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use objc2_metal::{
    MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLDevice, MTLLoadAction, MTLPixelFormat,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLResourceOptions, MTLStorageMode, MTLStoreAction, MTLTexture,
    MTLTextureDescriptor, MTLTextureUsage,
};
use wgpu::custom::*;

use super::{Obj, Shared, send_sync};

pub(crate) fn pixel_format(format: wgpu::TextureFormat) -> MTLPixelFormat {
    use wgpu::TextureFormat as F;
    match format {
        F::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
        F::Bgra8UnormSrgb => MTLPixelFormat::BGRA8Unorm_sRGB,
        F::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
        F::Rgba8UnormSrgb => MTLPixelFormat::RGBA8Unorm_sRGB,
        F::R8Unorm => MTLPixelFormat::R8Unorm,
        F::Rgba16Float => MTLPixelFormat::RGBA16Float,
        _ => MTLPixelFormat::Invalid,
    }
}

#[derive(Debug)]
pub(crate) struct MetalBuffer {
    /// Owns the storage `contents` points into.
    _buffer: Obj<dyn MTLBuffer>,
    contents: NonNull<u8>,
    size: u64,
}

#[derive(Debug)]
pub(crate) struct MappedRange {
    at: NonNull<u8>,
    len: usize,
}

#[derive(Debug)]
pub(crate) struct MetalTexture {
    pub(crate) texture: Option<Obj<dyn MTLTexture>>,
    pub(crate) drawable: bool,
}

#[derive(Debug)]
pub(crate) struct MetalView {
    texture: Option<Obj<dyn MTLTexture>>,
    drawable: bool,
}

#[derive(Debug)]
pub(crate) struct MetalEncoder {
    shared: Arc<Shared>,
    cb: OnceLock<Obj<dyn MTLCommandBuffer>>,
    drawable: AtomicBool,
}

#[derive(Debug)]
pub(crate) struct MetalPass(Option<Obj<dyn MTLRenderCommandEncoder>>);

#[derive(Debug)]
pub(crate) struct MetalCommandBuffer {
    /// None when nothing was encoded: nothing to commit.
    pub(crate) cb: Option<Obj<dyn MTLCommandBuffer>>,
    /// Whether a pass drew into a drawable, so the buffer waits for its present.
    pub(crate) drawable: bool,
}

/// Shader modules, layouts, bind groups, pipelines and samplers: accepted and ignored.
#[derive(Debug)]
pub(crate) struct Inert;

send_sync!(MetalBuffer, MappedRange, MetalTexture, MetalView, MetalEncoder, MetalPass, MetalCommandBuffer);

impl MetalBuffer {
    pub(crate) fn new(device: &Obj<dyn MTLDevice>, size: u64) -> Self {
        let buffer = device.newBufferWithLength_options(size.max(4) as usize, MTLResourceOptions::StorageModeShared).expect("a Metal buffer");
        let contents = buffer.contents().cast();
        MetalBuffer { _buffer: buffer, contents, size }
    }

    pub(crate) fn write(&self, offset: u64, data: &[u8]) {
        assert!(offset + data.len() as u64 <= self.size, "write past the end of a buffer");
        // SAFETY: in bounds of the buffer's shared storage, which lives as long as `buffer`.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), self.contents.as_ptr().add(offset as usize), data.len()) };
    }
}

impl BufferInterface for MetalBuffer {
    fn map_async(&self, _: wgpu::MapMode, _: Range<wgpu::BufferAddress>, callback: BufferMapCallback) { callback(Ok(())) }
    fn get_mapped_range(&self, range: Range<wgpu::BufferAddress>) -> Result<DispatchBufferMappedRange, wgpu::MapRangeError> {
        assert!(range.end <= self.size);
        // SAFETY: in bounds, checked above.
        let at = unsafe { self.contents.add(range.start as usize) };
        Ok(DispatchBufferMappedRange::custom(MappedRange { at, len: (range.end - range.start) as usize }))
    }
    fn unmap(&self) {}
    fn destroy(&self) {}
}

impl BufferMappedRangeInterface for MappedRange {
    fn len(&self) -> usize { self.len }
    unsafe fn read_slice(&self) -> &[u8] { unsafe { std::slice::from_raw_parts(self.at.as_ptr(), self.len) } }
    unsafe fn write_slice(&mut self) -> wgpu::WriteOnly<'_, [u8]> {
        unsafe { std::slice::from_raw_parts_mut(self.at.as_ptr(), self.len) }.into()
    }
}

impl MetalTexture {
    pub(crate) fn new(device: &Obj<dyn MTLDevice>, desc: &wgpu::TextureDescriptor<'_>) -> Self {
        let format = pixel_format(desc.format);
        let flat = desc.dimension == wgpu::TextureDimension::D2 && desc.size.depth_or_array_layers == 1 && desc.sample_count == 1;
        let texture = (format != MTLPixelFormat::Invalid && flat).then(|| {
            // SAFETY: a valid format and nonzero size.
            let d = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                format, desc.size.width.max(1) as usize, desc.size.height.max(1) as usize, false) };
            let mut usage = MTLTextureUsage::ShaderRead;
            if desc.usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) { usage |= MTLTextureUsage::RenderTarget; }
            d.setUsage(usage);
            d.setStorageMode(MTLStorageMode::Private);
            device.newTextureWithDescriptor(&d)
        }).flatten();
        MetalTexture { texture, drawable: false }
    }
}

impl TextureInterface for MetalTexture {
    fn create_view(&self, _: &wgpu::TextureViewDescriptor<'_>) -> DispatchTextureView {
        DispatchTextureView::custom(MetalView { texture: self.texture.clone(), drawable: self.drawable })
    }
    fn destroy(&self) {}
}

impl TextureViewInterface for MetalView {}

impl MetalEncoder {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        MetalEncoder { shared, cb: OnceLock::new(), drawable: AtomicBool::new(false) }
    }
}

impl CommandEncoderInterface for MetalEncoder {
    fn copy_buffer_to_buffer(&self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: &DispatchBuffer, _: wgpu::BufferAddress, _: Option<wgpu::BufferAddress>) {}
    fn copy_buffer_to_texture(&self, _: wgpu::TexelCopyBufferInfo<'_>, _: wgpu::TexelCopyTextureInfo<'_>, _: wgpu::Extent3d) {}
    fn copy_texture_to_buffer(&self, _: wgpu::TexelCopyTextureInfo<'_>, _: wgpu::TexelCopyBufferInfo<'_>, _: wgpu::Extent3d) {}
    fn copy_texture_to_texture(&self, _: wgpu::TexelCopyTextureInfo<'_>, _: wgpu::TexelCopyTextureInfo<'_>, _: wgpu::Extent3d) {}
    fn begin_compute_pass(&self, _: &wgpu::ComputePassDescriptor<'_>) -> DispatchComputePass { unimplemented!("metal spike: compute") }
    fn begin_render_pass(&self, desc: &wgpu::RenderPassDescriptor<'_>) -> DispatchRenderPass {
        let target = desc.color_attachments.first().and_then(Option::as_ref)
            .and_then(|a| Some((a, a.view.as_custom::<MetalView>()?)))
            .and_then(|(a, v)| Some((a, v.texture.as_ref()?, v.drawable)));
        let Some((attachment, texture, drawable)) = target else { return DispatchRenderPass::custom(MetalPass(None)) };
        if drawable { self.drawable.store(true, Ordering::Relaxed); }
        let cb = self.cb.get_or_init(|| self.shared.queue.commandBufferWithUnretainedReferences().expect("a Metal command buffer"));
        let pass = MTLRenderPassDescriptor::new();
        // SAFETY: index 0 always exists.
        let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(texture));
        match attachment.ops.load {
            wgpu::LoadOp::Clear(c) => {
                color.setLoadAction(MTLLoadAction::Clear);
                color.setClearColor(MTLClearColor { red: c.r, green: c.g, blue: c.b, alpha: c.a });
            }
            _ => color.setLoadAction(MTLLoadAction::Load),
        }
        color.setStoreAction(if attachment.ops.store == wgpu::StoreOp::Store { MTLStoreAction::Store } else { MTLStoreAction::DontCare });
        DispatchRenderPass::custom(MetalPass(cb.renderCommandEncoderWithDescriptor(&pass)))
    }
    fn finish(&mut self) -> DispatchCommandBuffer {
        DispatchCommandBuffer::custom(MetalCommandBuffer { cb: self.cb.take(), drawable: *self.drawable.get_mut() })
    }
    fn clear_texture(&self, _: &DispatchTexture, _: &wgpu::ImageSubresourceRange) {}
    fn clear_buffer(&self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: Option<wgpu::BufferAddress>) {}
    fn insert_debug_marker(&self, _: &str) {}
    fn push_debug_group(&self, _: &str) {}
    fn pop_debug_group(&self) {}
    fn write_timestamp(&self, _: &DispatchQuerySet, _: u32) {}
    fn resolve_query_set(&self, _: &DispatchQuerySet, _: u32, _: u32, _: &DispatchBuffer, _: wgpu::BufferAddress) {}
    fn mark_acceleration_structures_built<'a>(&self, _: &mut dyn Iterator<Item = &'a wgpu::Blas>, _: &mut dyn Iterator<Item = &'a wgpu::Tlas>) {
        unimplemented!("metal spike: ray tracing")
    }
    fn build_acceleration_structures<'a>(&self, _: &mut dyn Iterator<Item = &'a wgpu::BlasBuildEntry<'a>>, _: &mut dyn Iterator<Item = &'a wgpu::Tlas>) {
        unimplemented!("metal spike: ray tracing")
    }
    fn transition_resources<'a>(&mut self, _: &mut dyn Iterator<Item = wgpu::wgt::BufferTransition<&'a DispatchBuffer>>,
        _: &mut dyn Iterator<Item = wgpu::wgt::TextureTransition<&'a DispatchTexture>>) {}
}

impl Drop for MetalPass {
    fn drop(&mut self) {
        if let Some(encoder) = &self.0 { encoder.endEncoding(); }
    }
}

impl RenderPassInterface for MetalPass {
    fn set_pipeline(&mut self, _: &DispatchRenderPipeline) {}
    fn set_bind_group(&mut self, _: u32, _: Option<&DispatchBindGroup>, _: &[wgpu::DynamicOffset]) {}
    fn set_index_buffer(&mut self, _: &DispatchBuffer, _: wgpu::IndexFormat, _: wgpu::BufferAddress, _: Option<wgpu::BufferSize>) {}
    fn set_vertex_buffer(&mut self, _: u32, _: Option<&DispatchBuffer>, _: wgpu::BufferAddress, _: Option<wgpu::BufferSize>) {}
    fn set_immediates(&mut self, _: u32, _: &[u8]) {}
    fn set_blend_constant(&mut self, _: wgpu::Color) {}
    fn set_scissor_rect(&mut self, _: u32, _: u32, _: u32, _: u32) {}
    fn set_viewport(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
    fn set_stencil_reference(&mut self, _: u32) {}
    fn draw(&mut self, _: Range<u32>, _: Range<u32>) {}
    fn draw_indexed(&mut self, _: Range<u32>, _: i32, _: Range<u32>) {}
    fn draw_mesh_tasks(&mut self, _: u32, _: u32, _: u32) {}
    fn draw_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress) {}
    fn draw_indexed_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress) {}
    fn draw_mesh_tasks_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress) {}
    fn multi_draw_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) {}
    fn multi_draw_indexed_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) {}
    fn multi_draw_indirect_count(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) {}
    fn multi_draw_mesh_tasks_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) {}
    fn multi_draw_indexed_indirect_count(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) {}
    fn multi_draw_mesh_tasks_indirect_count(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) {}
    fn insert_debug_marker(&mut self, _: &str) {}
    fn push_debug_group(&mut self, _: &str) {}
    fn pop_debug_group(&mut self) {}
    fn write_timestamp(&mut self, _: &DispatchQuerySet, _: u32) {}
    fn begin_occlusion_query(&mut self, _: u32) {}
    fn end_occlusion_query(&mut self) {}
    fn begin_pipeline_statistics_query(&mut self, _: &DispatchQuerySet, _: u32) {}
    fn end_pipeline_statistics_query(&mut self) {}
    fn execute_bundles(&mut self, _: &mut dyn Iterator<Item = &DispatchRenderBundle>) {}
}

impl CommandBufferInterface for MetalCommandBuffer {}

impl ShaderModuleInterface for Inert {
    fn get_compilation_info(&self) -> Pin<Box<dyn ShaderCompilationInfoFuture>> {
        Box::pin(ready(wgpu::CompilationInfo { messages: Vec::new() }))
    }
}
impl BindGroupLayoutInterface for Inert {}
impl BindGroupInterface for Inert {}
impl SamplerInterface for Inert {}
impl PipelineLayoutInterface for Inert {}
impl RenderBundleInterface for Inert {}
impl RenderPipelineInterface for Inert {
    fn get_bind_group_layout(&self, _: u32) -> DispatchBindGroupLayout { DispatchBindGroupLayout::custom(Inert) }
}
impl ComputePipelineInterface for Inert {
    fn get_bind_group_layout(&self, _: u32) -> DispatchBindGroupLayout { DispatchBindGroupLayout::custom(Inert) }
}
impl PipelineCacheInterface for Inert {
    fn get_data(&self) -> Option<Vec<u8>> { None }
}
impl QuerySetInterface for Inert {
    fn destroy(&self) {}
}
