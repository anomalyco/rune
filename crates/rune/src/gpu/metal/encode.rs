//! The command encoder: one `MTLCommandBuffer` with unretained references, blit copies between
//! passes, and the list of what it used, which the queue keeps alive until the buffer completes.

use std::any::Any;
use std::sync::Arc;

use objc2_foundation_video::NSRange;
use objc2_metal::{MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLOrigin, MTLSize};
use objc2_video::rc::autoreleasepool;
use objc2_video::runtime::ProtocolObject;
use parking_lot::Mutex;
use wgpu::custom::*;

use super::pass::MetalPass;
use super::resource::{MetalBuffer, MetalTexture, TextureRaw};
use super::submit::Keep;
use super::{Obj, Shared, send_sync};

/// What an encoder has recorded so far. Its passes share it, so a pass's resources join it.
#[derive(Debug)]
pub(super) struct Rec {
    shared: Arc<Shared>,
    cb: Option<Obj<dyn MTLCommandBuffer>>,
    blit: Option<Obj<dyn MTLBlitCommandEncoder>>,
    pub(super) keep: Keep,
    /// Whether a pass drew into a drawable, so the buffer waits for its present.
    pub(super) drawable: bool,
}

#[derive(Debug)]
pub(crate) struct MetalEncoder {
    rec: Arc<Mutex<Rec>>,
}

/// A finished encoder's buffer, until a submit takes it.
#[derive(Debug)]
pub(crate) struct MetalCommandBuffer(Mutex<Option<Recorded>>);

#[derive(Debug)]
pub(super) struct Recorded {
    pub(super) cb: Obj<dyn MTLCommandBuffer>,
    pub(super) keep: Keep,
    pub(super) drawable: bool,
}

/// Pipeline caches: accepted and ignored.
#[derive(Debug)]
pub(crate) struct Inert;

send_sync!(Rec, Recorded);

impl Rec {
    pub(super) fn cb(&mut self) -> &ProtocolObject<dyn MTLCommandBuffer> {
        self.cb.get_or_insert_with(|| self.shared.command_buffer())
    }

    /// Ends an open blit encoder, as a pass must before it starts.
    pub(super) fn end_blit(&mut self) {
        if let Some(blit) = self.blit.take() { blit.endEncoding(); }
    }

    fn blit(&mut self) -> &ProtocolObject<dyn MTLBlitCommandEncoder> {
        if self.blit.is_none() {
            let cb = self.cb();
            let blit = autoreleasepool(|_| cb.blitCommandEncoder()).expect("a Metal blit encoder");
            self.blit = Some(blit);
        }
        self.blit.as_deref().unwrap()
    }

    fn keep(&mut self, items: impl IntoIterator<Item = Arc<dyn Any + Send + Sync>>) { self.keep.extend(items); }

    pub(super) fn shared(&self) -> &Shared { &self.shared }
}

impl Drop for Rec {
    /// An encoder dropped unfinished. Metal ends a buffer's open encoders when it frees the
    /// buffer, reading their attachments, so the buffer goes before what it used.
    fn drop(&mut self) {
        self.end_blit();
        self.cb = None;
        self.shared.recycle(std::mem::take(&mut self.keep));
    }
}

impl MetalEncoder {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        let keep = shared.keep_list();
        MetalEncoder { rec: Arc::new(Mutex::new(Rec { shared, cb: None, blit: None, keep, drawable: false })) }
    }
}

impl MetalCommandBuffer {
    pub(super) fn take(&self) -> Option<Recorded> { self.0.lock().take() }
}

fn origin(o: wgpu::Origin3d, z: u32) -> MTLOrigin { MTLOrigin { x: o.x as usize, y: o.y as usize, z: z as usize } }

fn extent(width: u32, height: u32, depth: u32) -> MTLSize { MTLSize { width: width as usize, height: height as usize, depth: depth as usize } }

/// A buffer layout's row and image strides in bytes, filling in tightly packed defaults.
fn strides(format: wgpu::TextureFormat, layout: wgpu::TexelCopyBufferLayout, size: wgpu::Extent3d) -> (usize, usize) {
    let (bw, bh) = format.block_dimensions();
    let block = format.block_copy_size(None).expect("a copyable color format");
    let row = layout.bytes_per_row.unwrap_or(size.width.div_ceil(bw) * block) as usize;
    let rows = layout.rows_per_image.unwrap_or(size.height).div_ceil(bh) as usize;
    (row, row * rows)
}

/// The copies a texture region splits into: one per array layer, or one for a 3D texture.
/// Yields (slice, z, depth, image index).
fn layers(t: &TextureRaw, at: wgpu::Origin3d, size: wgpu::Extent3d) -> impl Iterator<Item = (usize, u32, u32, usize)> {
    let three = t.dimension == wgpu::TextureDimension::D3;
    let count = if three { 1 } else { size.depth_or_array_layers };
    (0..count).map(move |i| if three { (0, at.z, size.depth_or_array_layers, 0) } else { ((at.z + i) as usize, 0, 1, i as usize) })
}

pub(super) fn buffer_to_texture(blit: &ProtocolObject<dyn MTLBlitCommandEncoder>, buffer: &ProtocolObject<dyn MTLBuffer>,
    layout: wgpu::TexelCopyBufferLayout, texture: &TextureRaw, mip: u32, at: wgpu::Origin3d, size: wgpu::Extent3d) {
    let (row, image) = strides(texture.format, layout, size);
    for (slice, z, depth, i) in layers(texture, at, size) {
        let per_image = if depth > 1 { image } else { 0 };
        // SAFETY: wgpu validated the regions against both resources.
        unsafe {
            blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                buffer, layout.offset as usize + image * i, row, per_image, extent(size.width, size.height, depth),
                &texture.raw, slice, mip as usize, origin(at, z))
        }
    }
}

fn texture_to_buffer(blit: &ProtocolObject<dyn MTLBlitCommandEncoder>, texture: &TextureRaw, mip: u32, at: wgpu::Origin3d,
    buffer: &ProtocolObject<dyn MTLBuffer>, layout: wgpu::TexelCopyBufferLayout, size: wgpu::Extent3d) {
    let (row, image) = strides(texture.format, layout, size);
    for (slice, z, depth, i) in layers(texture, at, size) {
        let per_image = if depth > 1 { image } else { 0 };
        // SAFETY: wgpu validated the regions against both resources.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                &texture.raw, slice, mip as usize, origin(at, z), extent(size.width, size.height, depth),
                buffer, layout.offset as usize + image * i, row, per_image)
        }
    }
}

fn texture(info: &wgpu::TexelCopyTextureInfo<'_>) -> Arc<TextureRaw> {
    info.texture.as_custom::<MetalTexture>().expect("a Metal texture").raw.clone()
}

fn buffer(buffer: &DispatchBuffer) -> &MetalBuffer { buffer.as_custom::<MetalBuffer>().expect("a Metal buffer") }

impl CommandEncoderInterface for MetalEncoder {
    fn copy_buffer_to_buffer(&self, source: &DispatchBuffer, from: wgpu::BufferAddress, destination: &DispatchBuffer,
        to: wgpu::BufferAddress, size: Option<wgpu::BufferAddress>) {
        let (src, dst) = (buffer(source).raw(), buffer(destination).store.gpu_write());
        let size = size.unwrap_or(src.size - from);
        if size == 0 { return }
        let mut rec = self.rec.lock();
        // SAFETY: wgpu validated both ranges.
        unsafe { rec.blit().copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(&src.raw, from as usize, &dst.raw, to as usize, size as usize) };
        rec.keep([src as _, dst as _]);
    }
    fn copy_buffer_to_texture(&self, source: wgpu::TexelCopyBufferInfo<'_>, destination: wgpu::TexelCopyTextureInfo<'_>, size: wgpu::Extent3d) {
        if size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 { return }
        let (src, dst) = (source.buffer.as_custom::<MetalBuffer>().expect("a Metal buffer").raw(), texture(&destination));
        let mut rec = self.rec.lock();
        buffer_to_texture(rec.blit(), &src.raw, source.layout, &dst, destination.mip_level, destination.origin, size);
        rec.keep([src as _, dst as _]);
    }
    fn copy_texture_to_buffer(&self, source: wgpu::TexelCopyTextureInfo<'_>, destination: wgpu::TexelCopyBufferInfo<'_>, size: wgpu::Extent3d) {
        if size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 { return }
        let (src, dst) = (texture(&source), destination.buffer.as_custom::<MetalBuffer>().expect("a Metal buffer").store.gpu_write());
        let mut rec = self.rec.lock();
        texture_to_buffer(rec.blit(), &src, source.mip_level, source.origin, &dst.raw, destination.layout, size);
        rec.keep([src as _, dst as _]);
    }
    fn copy_texture_to_texture(&self, source: wgpu::TexelCopyTextureInfo<'_>, destination: wgpu::TexelCopyTextureInfo<'_>, size: wgpu::Extent3d) {
        if size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 { return }
        let (src, dst) = (texture(&source), texture(&destination));
        let mut rec = self.rec.lock();
        let blit = rec.blit();
        for ((from_slice, from_z, depth, _), (to_slice, to_z, _, _)) in layers(&src, source.origin, size).zip(layers(&dst, destination.origin, size)) {
            // SAFETY: wgpu validated both regions.
            unsafe {
                blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    &src.raw, from_slice, source.mip_level as usize, origin(source.origin, from_z), extent(size.width, size.height, depth),
                    &dst.raw, to_slice, destination.mip_level as usize, origin(destination.origin, to_z))
            }
        }
        rec.keep([src as _, dst as _]);
    }
    fn begin_compute_pass(&self, _: &wgpu::ComputePassDescriptor<'_>) -> DispatchComputePass {
        unimplemented!("RUNE_GPU_BACKEND=metal: compute")
    }
    fn begin_render_pass(&self, desc: &wgpu::RenderPassDescriptor<'_>) -> DispatchRenderPass {
        DispatchRenderPass::custom(MetalPass::begin(self.rec.clone(), desc))
    }
    fn finish(&mut self) -> DispatchCommandBuffer {
        let mut rec = self.rec.lock();
        rec.end_blit();
        let recorded = rec.cb.take().map(|cb| Recorded { cb, keep: std::mem::take(&mut rec.keep), drawable: rec.drawable });
        DispatchCommandBuffer::custom(MetalCommandBuffer(Mutex::new(recorded)))
    }
    fn clear_texture(&self, _: &DispatchTexture, _: &wgpu::ImageSubresourceRange) {
        unimplemented!("RUNE_GPU_BACKEND=metal: CommandEncoder::clear_texture")
    }
    fn clear_buffer(&self, target: &DispatchBuffer, offset: wgpu::BufferAddress, size: Option<wgpu::BufferAddress>) {
        let raw = buffer(target).store.gpu_write();
        let size = size.unwrap_or(raw.size - offset);
        if size == 0 { return }
        let mut rec = self.rec.lock();
        rec.blit().fillBuffer_range_value(&raw.raw, NSRange::new(offset as usize, size as usize), 0);
        rec.keep([raw as _]);
    }
    fn insert_debug_marker(&self, _: &str) {}
    fn push_debug_group(&self, _: &str) {}
    fn pop_debug_group(&self) {}
    fn write_timestamp(&self, _: &DispatchQuerySet, _: u32) { unimplemented!("RUNE_GPU_BACKEND=metal: timestamps") }
    fn resolve_query_set(&self, _: &DispatchQuerySet, _: u32, _: u32, _: &DispatchBuffer, _: wgpu::BufferAddress) {
        unimplemented!("RUNE_GPU_BACKEND=metal: query sets")
    }
    fn mark_acceleration_structures_built<'a>(&self, _: &mut dyn Iterator<Item = &'a wgpu::Blas>, _: &mut dyn Iterator<Item = &'a wgpu::Tlas>) {
        unimplemented!("RUNE_GPU_BACKEND=metal: ray tracing")
    }
    fn build_acceleration_structures<'a>(&self, _: &mut dyn Iterator<Item = &'a wgpu::BlasBuildEntry<'a>>, _: &mut dyn Iterator<Item = &'a wgpu::Tlas>) {
        unimplemented!("RUNE_GPU_BACKEND=metal: ray tracing")
    }
    fn transition_resources<'a>(&mut self, _: &mut dyn Iterator<Item = wgpu::wgt::BufferTransition<&'a DispatchBuffer>>,
        _: &mut dyn Iterator<Item = wgpu::wgt::TextureTransition<&'a DispatchTexture>>) {}
}

impl CommandBufferInterface for MetalCommandBuffer {}

impl PipelineCacheInterface for Inert {
    fn get_data(&self) -> Option<Vec<u8>> { None }
}
