//! The command encoder: one `MTLCommandBuffer` with unretained references, blit copies between
//! passes, and the list of what it used, which the queue keeps alive until the buffer completes.

#![allow(unsafe_code)]

use std::any::Any;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use objc2_foundation::NSRange;
#[cfg(debug_assertions)]
use objc2_foundation::NSString;
#[cfg(debug_assertions)]
use objc2_metal::MTLResource;
use objc2_metal::{MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLDevice, MTLEvent, MTLLoadAction, MTLOrigin, MTLRegion, MTLRenderPassDescriptor, MTLResourceOptions, MTLSize, MTLStoreAction, MTLTexture};
use rune_core::machine::Effects;
use objc2::rc::autoreleasepool;
use objc2::runtime::ProtocolObject;
use parking_lot::Mutex;
use wgpu::custom::*;

use crate::lazy_init::{Effect, Event, SubresourceRange};
use super::compute::MetalComputePass;
use super::pass::MetalPass;
use super::query::QuerySet;
use super::resource::{MetalBuffer, MetalTexture, TextureRaw};
use super::submit::Keep;
use super::{Obj, Shared, send_sync};

/// What an encoder has recorded so far. Its passes share it, so a pass's resources join it.
#[derive(Debug)]
pub(super) struct Rec {
    shared: Arc<Shared>,
    open: Open,
    pub(super) keep: Keep,
    /// Whether a pass drew into a drawable, so the buffer waits for its present.
    pub(super) drawable: bool,
    /// Events set once the buffer's work finished (`super::signal_event_on_finish`).
    signals: Vec<(Obj<dyn MTLEvent>, u64)>,
    #[cfg(debug_assertions)]
    label: Option<Box<str>>,
}

/// An event a command buffer waits for or signals, kept until the buffer completes (it doesn't
/// retain what it uses).
struct KeptEvent(#[allow(dead_code)] Obj<dyn MTLEvent>);
send_sync!(KeptEvent);

/// The command buffer as recorded so far.
#[derive(Debug, Default)]
enum Open {
    /// Nothing recorded yet.
    #[default]
    Empty,
    Buffer(Obj<dyn MTLCommandBuffer>),
    /// A blit encoder is open for copies, until a pass or `finish` ends it.
    Blit(Obj<dyn MTLCommandBuffer>, Obj<dyn MTLBlitCommandEncoder>),
}

impl Open {
    /// The buffer, ending an open blit encoder.
    fn close(self) -> Option<Obj<dyn MTLCommandBuffer>> {
        match self {
            Open::Empty => None,
            Open::Buffer(cb) => Some(cb),
            Open::Blit(cb, blit) => {
                blit.endEncoding();
                Some(cb)
            }
        }
    }
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
    pub(super) shared: Arc<Shared>,
}

/// Pipeline caches: accepted and ignored.
#[derive(Debug)]
pub(crate) struct Inert;

send_sync!(Rec, Recorded);

impl Rec {
    /// The buffer, made on the first command: from then until it is submitted or dropped, the
    /// encoder counts in `Shared::recording`, so read buffer storage only after this.
    fn cb(&mut self) -> &ProtocolObject<dyn MTLCommandBuffer> {
        if let Open::Empty = self.open {
            self.shared.recording.fetch_add(1, Ordering::AcqRel);
            let cb = self.shared.command_buffer();
            #[cfg(debug_assertions)]
            {
                let label = self.label.as_deref().unwrap_or("rune metal encoder");
                cb.setLabel(Some(&NSString::from_str(label)));
            }
            self.open = Open::Buffer(cb);
        }
        match &self.open {
            Open::Buffer(cb) | Open::Blit(cb, _) => cb,
            Open::Empty => unreachable!("opened above"),
        }
    }

    /// The buffer for a render or compute pass: an open blit encoder ends first, as Metal
    /// allows one encoder at a time.
    pub(super) fn pass_buffer(&mut self) -> &ProtocolObject<dyn MTLCommandBuffer> {
        if let Open::Blit(..) = self.open
            && let Some(cb) = std::mem::take(&mut self.open).close()
        {
            self.open = Open::Buffer(cb);
        }
        self.cb()
    }

    fn blit(&mut self) -> &ProtocolObject<dyn MTLBlitCommandEncoder> {
        if !matches!(self.open, Open::Blit(..)) {
            self.cb();
            let Open::Buffer(cb) = std::mem::take(&mut self.open) else { unreachable!("opened above") };
            let blit = autoreleasepool(|_| cb.blitCommandEncoder()).expect("a Metal blit encoder");
            #[cfg(debug_assertions)]
            blit.setLabel(Some(&NSString::from_str("rune metal blit")));
            self.open = Open::Blit(cb, blit);
        }
        match &self.open {
            Open::Blit(_, blit) => blit,
            _ => unreachable!("opened above"),
        }
    }

    fn keep(&mut self, items: impl IntoIterator<Item = Arc<dyn Any + Send + Sync>>) { self.keep.extend(items); }

    pub(super) fn shared(&self) -> &Shared { &self.shared }

    pub(super) fn shared_arc(&self) -> Arc<Shared> { self.shared.clone() }
}

impl Drop for Rec {
    /// An encoder dropped unfinished. Metal ends a buffer's open encoders when it frees the
    /// buffer, reading their attachments, so the buffer goes before what it used.
    fn drop(&mut self) {
        if std::mem::take(&mut self.open).close().is_some() { self.shared.recording.fetch_sub(1, Ordering::AcqRel); }
        self.shared.recycle(std::mem::take(&mut self.keep));
    }
}

impl MetalEncoder {
    pub(crate) fn new(shared: Arc<Shared>, label: Option<&str>) -> Self {
        let keep = shared.keep_list();
        #[cfg(not(debug_assertions))]
        let _ = label;
        MetalEncoder {
            rec: Arc::new(Mutex::new(Rec {
                shared,
                open: Open::Empty,
                keep,
                drawable: false,
                signals: Vec::new(),
                #[cfg(debug_assertions)]
                label: label.map(Box::from),
            })),
        }
    }

    /// Before what this encoder records next, the GPU waits until `event` reaches `value`.
    pub(super) fn wait_for_event(&self, event: &ProtocolObject<dyn MTLEvent>, value: u64) {
        let event = retain(event);
        let mut rec = self.rec.lock();
        rec.pass_buffer().encodeWaitForEvent_value(&event, value);
        rec.keep([Arc::new(KeptEvent(event)) as _]);
    }

    /// Once all this encoder's work finished, the GPU sets `event` to `value`.
    pub(super) fn signal_on_finish(&self, event: &ProtocolObject<dyn MTLEvent>, value: u64) {
        self.rec.lock().signals.push((retain(event), value));
    }
}

fn retain(event: &ProtocolObject<dyn MTLEvent>) -> Obj<dyn MTLEvent> {
    objc2::Message::retain(event)
}

impl MetalCommandBuffer {
    pub(super) fn take(&self) -> Option<Recorded> { self.0.lock().take() }
}

impl Recorded {
    /// No longer recording: submitted, or dropped unsubmitted.
    pub(super) fn close(self) -> (Obj<dyn MTLCommandBuffer>, Keep) {
        self.shared.recording.fetch_sub(1, Ordering::AcqRel);
        (self.cb, self.keep)
    }
}

impl Drop for MetalCommandBuffer {
    fn drop(&mut self) {
        if let Some(recorded) = self.0.get_mut().take() {
            let shared = recorded.shared.clone();
            let (cb, keep) = recorded.close();
            drop(cb);
            shared.recycle(keep);
        }
    }
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

/// Bytes of zeros a texture's first clear copies from: the widest row, 16384 texels of 16 bytes.
const ZEROS: usize = 256 << 10;

/// A buffer of `ZEROS` zeroed bytes, outside the live buffer count.
#[derive(Debug)]
pub(super) struct Zeros(Obj<dyn MTLBuffer>);
send_sync!(Zeros);

impl Zeros {
    pub(super) fn new(shared: &Shared) -> Self {
        let raw = shared.device.newBufferWithLength_options(ZEROS, MTLResourceOptions::StorageModeShared).expect("a Metal buffer");
        #[cfg(debug_assertions)]
        raw.setLabel(Some(&NSString::from_str("rune metal zeros")));
        Zeros(raw)
    }
}

/// Zeroes every mip and layer of a new texture, which wgpu promises reads as zero until written.
pub(super) fn zero_texture(blit: &ProtocolObject<dyn MTLBlitCommandEncoder>, zeros: &Zeros, texture: &TextureRaw) {
    let Some(block) = texture.format.block_copy_size(None) else { return };
    let (bw, bh) = texture.format.block_dimensions();
    let three = texture.dimension == wgpu::TextureDimension::D3;
    let raw = &texture.raw;
    for mip in 0..texture.mips as usize {
        let (w, h) = ((raw.width() >> mip).max(1), (raw.height() >> mip).max(1));
        let row = w.div_ceil(bw as usize) * block as usize;
        let strip = (ZEROS / row).max(1) * bh as usize;
        let slices = if three { (raw.depth() >> mip).max(1) } else { texture.layers as usize };
        for s in 0..slices {
            let (slice, z) = if three { (0, s) } else { (s, 0) };
            for y in (0..h).step_by(strip) {
                // SAFETY: each strip is within the mip and reads at most `ZEROS` bytes.
                unsafe {
                    blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                        &zeros.0, 0, row, 0, MTLSize { width: w, height: strip.min(h - y), depth: 1 },
                        raw, slice, mip, MTLOrigin { x: 0, y, z })
                }
            }
        }
    }
}

/// Zeroes every mip and layer of a new `MTLStorageMode::Shared` texture on the CPU, avoiding a
/// `zero_texture` blit encoder.
pub(super) fn zero_shared_texture(texture: &TextureRaw) {
    static ZERO_BYTES: [u8; ZEROS] = [0; ZEROS];
    let Some(block) = texture.format.block_copy_size(None) else { return };
    let (bw, bh) = texture.format.block_dimensions();
    let three = texture.dimension == wgpu::TextureDimension::D3;
    let one_d = texture.dimension == wgpu::TextureDimension::D1;
    let raw = &texture.raw;
    for mip in 0..texture.mips as usize {
        let (w, h) = ((raw.width() >> mip).max(1), (raw.height() >> mip).max(1));
        let row = w.div_ceil(bw as usize) * block as usize;
        let strip = (ZEROS / row).max(1) * bh as usize;
        let slices = if three { (raw.depth() >> mip).max(1) } else { texture.layers as usize };
        for s in 0..slices {
            let (slice, z) = if three { (0, s) } else { (s, 0) };
            for y in (0..h).step_by(strip) {
                let rows = strip.min(h - y);
                let region = MTLRegion {
                    origin: MTLOrigin { x: 0, y, z },
                    size: MTLSize { width: w, height: rows, depth: 1 },
                };
                let bpr = if one_d { 0 } else { row };
                let bpi = if three { row * rows.div_ceil(bh as usize) } else { 0 };
                // SAFETY: each strip is within the mip and reads at most `ZEROS` bytes.
                unsafe {
                    raw.replaceRegion_mipmapLevel_slice_withBytes_bytesPerRow_bytesPerImage(
                        region, mip, slice, NonNull::from(&ZERO_BYTES[0]).cast(), bpr, bpi,
                    );
                }
            }
        }
    }
}

/// Writes tightly packed full-width rows of an idle shared texture starting at row `y`.
pub(super) fn replace_texture_rows(texture: &TextureRaw, y: u32, rows: &[u8]) {
    let Some(block) = texture.format.block_copy_size(None) else { return };
    let w = texture.raw.width();
    let row = w * block as usize;
    let count = rows.len() / row;
    if count == 0 { return }
    assert!(y as usize + count <= texture.raw.height(), "row write past the end of a texture");
    let region = MTLRegion {
        origin: MTLOrigin { x: 0, y: y as usize, z: 0 },
        size: MTLSize { width: w, height: count, depth: 1 },
    };
    let bpr = if texture.dimension == wgpu::TextureDimension::D1 { 0 } else { row };
    // SAFETY: `rows` holds `count` tightly packed full-width rows within mip 0, checked above.
    unsafe {
        texture.raw.replaceRegion_mipmapLevel_slice_withBytes_bytesPerRow_bytesPerImage(
            region, 0, 0, NonNull::from(&rows[0]).cast(), bpr, 0,
        );
    }
}

/// Writes an arbitrary region of an idle shared texture on the CPU (`Queue::write_texture`).
pub(super) fn replace_texture(
    texture: &TextureRaw,
    mip: u32,
    at: wgpu::Origin3d,
    data: &[u8],
    layout: wgpu::TexelCopyBufferLayout,
    size: wgpu::Extent3d,
) {
    let (row, image) = strides(texture.format, layout, size);
    let one_d = texture.dimension == wgpu::TextureDimension::D1;
    for (slice, z, depth, i) in layers(texture, at, size) {
        let offset = layout.offset as usize + image * i;
        let region = MTLRegion { origin: origin(at, z), size: extent(size.width, size.height, depth) };
        let bpr = if one_d { 0 } else { row };
        let bpi = if depth > 1 { image } else { 0 };
        // SAFETY: wgpu validated `data`, `layout`, `at`, `size`, and `mip` against `texture`.
        unsafe {
            texture.raw.replaceRegion_mipmapLevel_slice_withBytes_bytesPerRow_bytesPerImage(
                region, mip as usize, slice, NonNull::from(&data[offset]).cast(), bpr, bpi,
            );
        }
    }
}

/// Whether a 2D copy at `(at, size)` covers every texel of mip `mip` of `texture`.
pub(super) fn is_full_mip(texture: &TextureRaw, mip: u32, at: wgpu::Origin3d, size: wgpu::Extent3d) -> bool {
    let w = (texture.raw.width() >> mip).max(1) as u32;
    let h = (texture.raw.height() >> mip).max(1) as u32;
    at.x == 0 && at.y == 0 && size.width >= w && size.height >= h
}

fn copy_range(mip: u32, at: wgpu::Origin3d, size: wgpu::Extent3d) -> SubresourceRange {
    SubresourceRange { base_mip: mip, mips: 1, base_layer: at.z, layers: size.depth_or_array_layers }
}

/// Clears one `(mip, layer)` of a Private color-renderable texture to zero via a clear-only
/// render pass, opening no blit encoder.
pub(super) fn clear_subresource(cb: &ProtocolObject<dyn MTLCommandBuffer>, texture: &TextureRaw, mip: u32, layer: u32) {
    let d = MTLRenderPassDescriptor::new();
    // SAFETY: color attachment 0 always exists on a newly created render pass descriptor.
    let color = unsafe { d.colorAttachments().objectAtIndexedSubscript(0) };
    color.setTexture(Some(&texture.raw));
    // SAFETY: `mip` and `layer` are within `texture`'s subresource bounds.
    unsafe {
        color.setLevel(mip as usize);
        color.setSlice(layer as usize);
    }
    color.setLoadAction(MTLLoadAction::Clear);
    color.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 });
    color.setStoreAction(MTLStoreAction::Store);
    let pass = autoreleasepool(|_| cb.renderCommandEncoderWithDescriptor(&d)).expect("a Metal clear render encoder");
    #[cfg(debug_assertions)]
    pass.setLabel(Some(&NSString::from_str("rune metal lazy clear")));
    pass.endEncoding();
}

/// Executes any lazy-clear effects for `texture` directly on `rec.pass_buffer()` before a copy.
fn clear_on_encoder(rec: &mut Rec, texture: &Arc<TextureRaw>, effects: Effects<Effect>) {
    if effects.is_empty() { return }
    let cb = rec.pass_buffer();
    for effect in effects {
        if let Effect::ClearSubresource(sub) = effect {
            clear_subresource(cb, texture, sub.mip, sub.layer);
        }
    }
    rec.keep.push(texture.clone());
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
        let size = size.unwrap_or_else(|| buffer(source).raw().size - from);
        if size == 0 { return }
        let mut rec = self.rec.lock();
        rec.cb();
        let (src, dst) = (buffer(source).raw(), buffer(destination).store.gpu_write());
        // SAFETY: wgpu validated both ranges.
        unsafe { rec.blit().copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(&src.raw, from as usize, &dst.raw, to as usize, size as usize) };
        rec.keep([src as _, dst as _]);
    }
    fn copy_buffer_to_texture(&self, source: wgpu::TexelCopyBufferInfo<'_>, destination: wgpu::TexelCopyTextureInfo<'_>, size: wgpu::Extent3d) {
        if size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 { return }
        let mut rec = self.rec.lock();
        rec.cb();
        let (src, dst) = (source.buffer.as_custom::<MetalBuffer>().expect("a Metal buffer").raw(), texture(&destination));
        if let Some(lazy) = &dst.lazy_init && lazy.has_uninit() {
            let range = copy_range(destination.mip_level, destination.origin, size);
            let event = if is_full_mip(&dst, destination.mip_level, destination.origin, size) {
                Event::WriteFull(range)
            } else {
                Event::WritePartial(range)
            };
            clear_on_encoder(&mut rec, &dst, lazy.step(event));
        }
        buffer_to_texture(rec.blit(), &src.raw, source.layout, &dst, destination.mip_level, destination.origin, size);
        rec.drawable |= dst.drawable;
        if let Some(uses) = &dst.uses { rec.keep.use_texture(uses); }
        rec.keep([src as _, dst as _]);
    }
    fn copy_texture_to_buffer(&self, source: wgpu::TexelCopyTextureInfo<'_>, destination: wgpu::TexelCopyBufferInfo<'_>, size: wgpu::Extent3d) {
        if size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 { return }
        let (src, dst) = (texture(&source), destination.buffer.as_custom::<MetalBuffer>().expect("a Metal buffer").store.gpu_write());
        let mut rec = self.rec.lock();
        if let Some(lazy) = &src.lazy_init && lazy.has_uninit() {
            let effects = lazy.step(Event::Read(copy_range(source.mip_level, source.origin, size)));
            clear_on_encoder(&mut rec, &src, effects);
        }
        texture_to_buffer(rec.blit(), &src, source.mip_level, source.origin, &dst.raw, destination.layout, size);
        if let Some(uses) = &src.uses { rec.keep.use_texture(uses); }
        rec.keep([src as _, dst as _]);
    }
    fn copy_texture_to_texture(&self, source: wgpu::TexelCopyTextureInfo<'_>, destination: wgpu::TexelCopyTextureInfo<'_>, size: wgpu::Extent3d) {
        if size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 { return }
        let (src, dst) = (texture(&source), texture(&destination));
        let mut rec = self.rec.lock();
        if let Some(lazy) = &src.lazy_init && lazy.has_uninit() {
            let effects = lazy.step(Event::Read(copy_range(source.mip_level, source.origin, size)));
            clear_on_encoder(&mut rec, &src, effects);
        }
        if let Some(lazy) = &dst.lazy_init && lazy.has_uninit() {
            let range = copy_range(destination.mip_level, destination.origin, size);
            let event = if is_full_mip(&dst, destination.mip_level, destination.origin, size) {
                Event::WriteFull(range)
            } else {
                Event::WritePartial(range)
            };
            clear_on_encoder(&mut rec, &dst, lazy.step(event));
        }
        let blit = rec.blit();
        for ((from_slice, from_z, depth, _), (to_slice, to_z, _, _)) in layers(&src, source.origin, size).zip(layers(&dst, destination.origin, size)) {
            // SAFETY: wgpu validated both regions.
            unsafe {
                blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    &src.raw, from_slice, source.mip_level as usize, origin(source.origin, from_z), extent(size.width, size.height, depth),
                    &dst.raw, to_slice, destination.mip_level as usize, origin(destination.origin, to_z))
            }
        }
        rec.drawable |= dst.drawable;
        if let Some(uses) = &src.uses { rec.keep.use_texture(uses); }
        if let Some(uses) = &dst.uses { rec.keep.use_texture(uses); }
        rec.keep([src as _, dst as _]);
    }
    fn begin_compute_pass(&self, desc: &wgpu::ComputePassDescriptor<'_>) -> DispatchComputePass {
        DispatchComputePass::custom(MetalComputePass::begin(self.rec.clone(), desc))
    }
    fn begin_render_pass(&self, desc: &wgpu::RenderPassDescriptor<'_>) -> DispatchRenderPass {
        DispatchRenderPass::custom(MetalPass::begin(self.rec.clone(), desc))
    }
    fn finish(&mut self) -> DispatchCommandBuffer {
        let mut rec = self.rec.lock();
        let signals = std::mem::take(&mut rec.signals);
        if !signals.is_empty() {
            rec.cb();
        }
        let recorded = std::mem::take(&mut rec.open).close().map(|cb| {
            for (event, value) in signals {
                cb.encodeSignalEvent_value(&event, value);
                rec.keep([Arc::new(KeptEvent(event)) as _]);
            }
            Recorded { cb, keep: std::mem::take(&mut rec.keep), drawable: rec.drawable, shared: rec.shared.clone() }
        });
        DispatchCommandBuffer::custom(MetalCommandBuffer(Mutex::new(recorded)))
    }
    fn clear_texture(&self, _: &DispatchTexture, _: &wgpu::ImageSubresourceRange) {
        super::unsupported!("CommandEncoder::clear_texture")
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
    fn write_timestamp(&self, _: &DispatchQuerySet, _: u32) { super::unsupported!("timestamps between passes") }
    fn resolve_query_set(&self, set: &DispatchQuerySet, first: u32, count: u32, destination: &DispatchBuffer, offset: wgpu::BufferAddress) {
        if count == 0 { return }
        let set = set.as_custom::<QuerySet>().expect("a Metal query set").raw.clone();
        let dst = buffer(destination).store.gpu_write();
        let mut rec = self.rec.lock();
        // SAFETY: wgpu validated the range against the set and the destination's size.
        unsafe { rec.blit().resolveCounters_inRange_destinationBuffer_destinationOffset(&set.0, NSRange::new(first as usize, count as usize), &dst.raw, offset as usize) };
        rec.keep([set as _, dst as _]);
    }
    fn mark_acceleration_structures_built<'a>(&self, _: &mut dyn Iterator<Item = &'a wgpu::Blas>, _: &mut dyn Iterator<Item = &'a wgpu::Tlas>) {
        super::unsupported!("ray tracing")
    }
    fn build_acceleration_structures<'a>(&self, _: &mut dyn Iterator<Item = &'a wgpu::BlasBuildEntry<'a>>, _: &mut dyn Iterator<Item = &'a wgpu::Tlas>) {
        super::unsupported!("ray tracing")
    }
    fn transition_resources<'a>(&mut self, _: &mut dyn Iterator<Item = wgpu::wgt::BufferTransition<&'a DispatchBuffer>>,
        _: &mut dyn Iterator<Item = wgpu::wgt::TextureTransition<&'a DispatchTexture>>) {}
}

impl CommandBufferInterface for MetalCommandBuffer {}

impl PipelineCacheInterface for Inert {
    fn get_data(&self) -> Option<Vec<u8>> { None }
}
