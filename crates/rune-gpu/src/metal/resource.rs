//! Buffers, textures, views and samplers.

#![allow(unsafe_code)]

use std::ops::Range;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

use objc2_foundation::NSRange;
#[cfg(debug_assertions)]
use objc2_foundation::NSString;
#[cfg(debug_assertions)]
use objc2_metal::MTLResource;
use objc2_metal::{
    MTLBuffer, MTLCompareFunction, MTLDevice, MTLGPUFamily, MTLPixelFormat, MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerBorderColor,
    MTLSamplerDescriptor, MTLSamplerMinMagFilter, MTLSamplerMipFilter, MTLSamplerState, MTLStorageMode, MTLTexture, MTLTextureDescriptor,
    MTLTextureType, MTLTextureUsage,
};
use wgpu::custom::*;

use crate::lazy_init::{LazyInit, SubresourceRange};
use super::store::Store;
use super::{Obj, Owner, Shared, send_sync};

pub(crate) fn pixel_format(format: wgpu::TextureFormat) -> MTLPixelFormat {
    use MTLPixelFormat as M;
    use wgpu::TextureFormat as F;
    match format {
        F::R8Unorm => M::R8Unorm,
        F::R8Snorm => M::R8Snorm,
        F::R8Uint => M::R8Uint,
        F::R8Sint => M::R8Sint,
        F::R16Unorm => M::R16Unorm,
        F::R16Uint => M::R16Uint,
        F::R16Sint => M::R16Sint,
        F::R16Float => M::R16Float,
        F::Rg8Unorm => M::RG8Unorm,
        F::Rg8Snorm => M::RG8Snorm,
        F::Rg8Uint => M::RG8Uint,
        F::R32Uint => M::R32Uint,
        F::R32Sint => M::R32Sint,
        F::R32Float => M::R32Float,
        F::Rg16Unorm => M::RG16Unorm,
        F::Rg16Uint => M::RG16Uint,
        F::Rg16Float => M::RG16Float,
        F::Rgba8Unorm => M::RGBA8Unorm,
        F::Rgba8UnormSrgb => M::RGBA8Unorm_sRGB,
        F::Rgba8Snorm => M::RGBA8Snorm,
        F::Rgba8Uint => M::RGBA8Uint,
        F::Bgra8Unorm => M::BGRA8Unorm,
        F::Bgra8UnormSrgb => M::BGRA8Unorm_sRGB,
        F::Rgb10a2Unorm => M::RGB10A2Unorm,
        F::Rg11b10Ufloat => M::RG11B10Float,
        F::Rg32Uint => M::RG32Uint,
        F::Rg32Float => M::RG32Float,
        F::Rgba16Unorm => M::RGBA16Unorm,
        F::Rgba16Uint => M::RGBA16Uint,
        F::Rgba16Float => M::RGBA16Float,
        F::Rgba32Uint => M::RGBA32Uint,
        F::Rgba32Float => M::RGBA32Float,
        F::Stencil8 => M::Stencil8,
        F::Depth16Unorm => M::Depth16Unorm,
        F::Depth24Plus => M::Depth32Float,
        F::Depth24PlusStencil8 => M::Depth32Float_Stencil8,
        F::Depth32Float => M::Depth32Float,
        F::Depth32FloatStencil8 => M::Depth32Float_Stencil8,
        other => super::unsupported!("texture format {other:?}"),
    }
}

/// Metal pixel format for a texture view over a texture of `texture_format`.
/// Combined depth-stencil formats use `X32_Stencil8` for stencil-only views and keep
/// `Depth32Float_Stencil8` for depth-only views (Metal forbids aliasing combined depth-stencil
/// textures to `Depth32Float`).
fn view_pixel_format(
    texture_format: wgpu::TextureFormat,
    view_format: Option<wgpu::TextureFormat>,
    aspect: wgpu::TextureAspect,
) -> MTLPixelFormat {
    if texture_format.is_combined_depth_stencil_format() {
        return match aspect {
            wgpu::TextureAspect::StencilOnly => MTLPixelFormat::X32_Stencil8,
            wgpu::TextureAspect::DepthOnly | wgpu::TextureAspect::All => pixel_format(texture_format),
            other => super::unsupported!("texture aspect {other:?}"),
        };
    }
    pixel_format(view_format.unwrap_or(texture_format))
}

/// A buffer's storage, which its CPU pointer stays valid with.
#[derive(Debug)]
pub(crate) struct BufferRaw {
    pub(super) raw: Obj<dyn MTLBuffer>,
    contents: NonNull<u8>,
    pub(super) size: u64,
    live: Arc<AtomicIsize>,
}

#[derive(Debug)]
pub(crate) struct MetalBuffer {
    pub(super) store: Arc<Store>,
    pub(super) shared: Arc<Shared>,
}

#[derive(Debug)]
pub(crate) struct MappedRange {
    at: NonNull<u8>,
    len: usize,
}

#[derive(Debug)]
pub(crate) struct TextureRaw {
    pub(super) raw: Obj<dyn MTLTexture>,
    pub(super) format: wgpu::TextureFormat,
    pub(super) dimension: wgpu::TextureDimension,
    pub(super) raw_type: MTLTextureType,
    pub(super) sample_count: u32,
    pub(super) layers: u32,
    pub(super) mips: u32,
    pub(super) drawable: bool,
    /// Allocated with `MTLStorageMode::Shared` (zeroed on CPU, never blit-zeroed).
    shared_storage: bool,
    /// Active encoders and in-flight command buffers referencing this texture; `Some` iff
    /// allocated as `MTLStorageMode::Shared` with `COPY_DST` for CPU `replaceRegion` uploads.
    pub(super) uses: Option<Arc<AtomicUsize>>,
    /// Per-(mip, layer) lazy zero-init state for Private `RENDER_ATTACHMENT` textures.
    pub(super) lazy_init: Option<LazyInit>,
    /// What keeps a texture this backend didn't make valid (`texture_from_raw`).
    owner: Option<Owner>,
    #[allow(dead_code, reason = "held, never read")]
    live: Option<Live>,
}

#[derive(Debug)]
pub(crate) struct MetalTexture {
    pub(super) raw: Arc<TextureRaw>,
    pub(super) shared: Option<Arc<Shared>>,
}

#[derive(Debug)]
pub(crate) struct ViewRaw {
    pub(super) raw: Obj<dyn MTLTexture>,
    pub(super) format: wgpu::TextureFormat,
    pub(super) aspect: wgpu::TextureAspect,
    pub(super) drawable: bool,
    /// Its texture's in-flight usage counter, when the texture uses `MTLStorageMode::Shared`.
    pub(super) uses: Option<Arc<AtomicUsize>>,
    /// Parent texture when it uses lazy render-pass zeroing; `None` for Shared/eager textures.
    pub(super) texture: Option<Arc<TextureRaw>>,
    /// Mip and array-layer subresources exposed by this view.
    pub(super) range: SubresourceRange,
    /// Its texture's owner, which bind groups and passes that use the view keep.
    pub(super) owner: Option<Owner>,
}

#[derive(Debug)]
pub(crate) struct MetalView {
    pub(super) raw: Arc<ViewRaw>,
}

#[derive(Debug)]
pub(crate) struct SamplerRaw(pub(super) Obj<dyn MTLSamplerState>);

#[derive(Debug)]
pub(crate) struct MetalSampler {
    pub(super) raw: Arc<SamplerRaw>,
}

send_sync!(BufferRaw, MappedRange, TextureRaw, ViewRaw, SamplerRaw);

/// Counts one live resource of a kind, for `Device::get_internal_counters`, until dropped.
#[derive(Debug)]
pub(crate) struct Live(Arc<AtomicIsize>);

impl Live {
    pub(super) fn new(count: &Arc<AtomicIsize>) -> Self {
        count.fetch_add(1, Ordering::Relaxed);
        Live(count.clone())
    }
}

impl Drop for Live {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::Relaxed); }
}

impl Drop for BufferRaw {
    fn drop(&mut self) { self.live.fetch_sub(1, Ordering::Relaxed); }
}

impl BufferRaw {
    fn from(shared: &Shared, raw: Obj<dyn MTLBuffer>, size: u64) -> Self {
        shared.buffers.fetch_add(1, Ordering::Relaxed);
        let contents = raw.contents().cast();
        BufferRaw { raw, contents, size, live: shared.buffers.clone() }
    }

    /// A shared buffer of `size` zeroed bytes.
    pub(super) fn new(shared: &Shared, size: u64, label: Option<&str>) -> Self {
        let raw = shared.device.newBufferWithLength_options(size.max(4) as usize, MTLResourceOptions::StorageModeShared).expect("a Metal buffer");
        #[cfg(debug_assertions)]
        if let Some(label) = label { raw.setLabel(Some(&NSString::from_str(label))); }
        #[cfg(not(debug_assertions))]
        let _ = label;
        Self::from(shared, raw, size)
    }

    /// A shared buffer holding a copy of `data`, which is not empty.
    pub(super) fn with_bytes(shared: &Shared, data: &[u8], label: Option<&str>) -> Self {
        // SAFETY: `data` is valid for its length, which is nonzero.
        let raw = unsafe {
            shared.device.newBufferWithBytes_length_options(NonNull::from(data).cast(), data.len(), MTLResourceOptions::StorageModeShared)
        }.expect("a Metal buffer");
        #[cfg(debug_assertions)]
        if let Some(label) = label { raw.setLabel(Some(&NSString::from_str(label))); }
        #[cfg(not(debug_assertions))]
        let _ = label;
        Self::from(shared, raw, data.len() as u64)
    }

    /// Writes `data` at `offset` from the CPU, which the GPU must not be reading then.
    pub(super) fn write(&self, offset: u64, data: &[u8]) {
        assert!(offset + data.len() as u64 <= self.size, "a write past the end of a buffer");
        // SAFETY: in bounds, checked above, of storage that lives as long as `raw`.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), self.contents.as_ptr().add(offset as usize), data.len()) };
    }

    /// Copies `range` of `other`, a buffer of the same size, into the same range here.
    pub(super) fn copy_from(&self, other: &BufferRaw, range: Range<u64>) {
        if range.is_empty() { return }
        assert!(range.end <= self.size && range.end <= other.size, "a copy past the end of a buffer");
        // SAFETY: in bounds of both, checked above; distinct allocations.
        unsafe {
            std::ptr::copy_nonoverlapping(other.contents.as_ptr().add(range.start as usize), self.contents.as_ptr().add(range.start as usize),
                (range.end - range.start) as usize)
        };
    }

    fn range(&self, range: Range<u64>) -> MappedRange {
        assert!(range.start <= range.end && range.end <= self.size, "a mapped range past the end of a buffer");
        // SAFETY: in bounds, checked above.
        MappedRange { at: unsafe { self.contents.add(range.start as usize) }, len: (range.end - range.start) as usize }
    }
}

impl MetalBuffer {
    pub(crate) fn new(shared: &Arc<Shared>, size: u64, label: Option<&str>) -> Self {
        MetalBuffer { store: Arc::new(Store::new(BufferRaw::new(shared, size, label))), shared: shared.clone() }
    }

    /// The storage commands recorded now read.
    pub(super) fn raw(&self) -> Arc<BufferRaw> { self.store.current() }
}

impl BufferInterface for MetalBuffer {
    fn map_async(&self, _: wgpu::MapMode, _: Range<wgpu::BufferAddress>, callback: BufferMapCallback) {
        self.shared.map_after_work(callback);
    }
    fn get_mapped_range(&self, range: Range<wgpu::BufferAddress>) -> Result<DispatchBufferMappedRange, wgpu::MapRangeError> {
        Ok(DispatchBufferMappedRange::custom(self.store.current().range(range)))
    }
    fn unmap(&self) {}
    fn destroy(&self) {}
}

impl BufferMappedRangeInterface for MappedRange {
    fn len(&self) -> usize { self.len }
    unsafe fn read_slice(&self) -> &[u8] {
        // SAFETY: `self.at..self.at + self.len` was bounds-checked against the shared `MTLBuffer`
        // in `BufferRaw::range`, and the caller guarantees the buffer stays mapped with no concurrent writes.
        unsafe { std::slice::from_raw_parts(self.at.as_ptr(), self.len) }
    }
    unsafe fn write_slice(&mut self) -> wgpu::WriteOnly<'_, [u8]> {
        // SAFETY: `self.at..self.at + self.len` was bounds-checked against the shared `MTLBuffer`
        // in `BufferRaw::range`, and the caller guarantees exclusive mutable access while mapped.
        unsafe { std::slice::from_raw_parts_mut(self.at.as_ptr(), self.len) }.into()
    }
}

fn texture_type(dimension: wgpu::TextureViewDimension, sample_count: u32) -> MTLTextureType {
    use wgpu::TextureViewDimension as D;
    if sample_count > 1 {
        return match dimension {
            D::D2 => MTLTextureType::Type2DMultisample,
            D::D2Array => MTLTextureType::Type2DMultisampleArray,
            other => super::unsupported!("multisampled {other:?} texture view"),
        };
    }
    match dimension {
        D::D1 => MTLTextureType::Type1D,
        D::D2 => MTLTextureType::Type2D,
        D::D2Array => MTLTextureType::Type2DArray,
        D::Cube => MTLTextureType::TypeCube,
        D::CubeArray => MTLTextureType::TypeCubeArray,
        D::D3 => MTLTextureType::Type3D,
    }
}

fn descriptor_texture_type(dimension: wgpu::TextureDimension, layers: u32, sample_count: u32) -> MTLTextureType {
    match dimension {
        wgpu::TextureDimension::D1 => MTLTextureType::Type1D,
        wgpu::TextureDimension::D2 if sample_count > 1 && layers > 1 => MTLTextureType::Type2DMultisampleArray,
        wgpu::TextureDimension::D2 if sample_count > 1 => MTLTextureType::Type2DMultisample,
        wgpu::TextureDimension::D2 if layers > 1 => MTLTextureType::Type2DArray,
        wgpu::TextureDimension::D2 => MTLTextureType::Type2D,
        wgpu::TextureDimension::D3 => MTLTextureType::Type3D,
    }
}

impl TextureRaw {
    /// A drawable's texture, which a surface hands out for one frame.
    pub(super) fn drawable(raw: Obj<dyn MTLTexture>, format: wgpu::TextureFormat) -> Self {
        TextureRaw { raw, format, dimension: wgpu::TextureDimension::D2, raw_type: MTLTextureType::Type2D, sample_count: 1,
            layers: 1, mips: 1, drawable: true, shared_storage: false, uses: None, lazy_init: None, owner: None, live: None }
    }

    pub(super) fn is_shared(&self) -> bool { self.shared_storage }

    pub(super) fn is_lazy(&self) -> bool { self.lazy_init.is_some() }

    /// Whether this texture uses shared storage with CPU writes and has no active encoder or
    /// in-flight GPU reference; retires completed command buffers once if needed.
    pub(super) fn is_idle(&self, shared: &Shared) -> bool {
        let Some(uses) = &self.uses else { return false };
        if uses.load(Ordering::Acquire) == 0 { return true }
        shared.retire();
        uses.load(Ordering::Acquire) == 0
    }

    fn view_dimension(&self) -> wgpu::TextureViewDimension {
        match self.dimension {
            wgpu::TextureDimension::D1 => wgpu::TextureViewDimension::D1,
            wgpu::TextureDimension::D2 if self.layers > 1 => wgpu::TextureViewDimension::D2Array,
            wgpu::TextureDimension::D2 => wgpu::TextureViewDimension::D2,
            wgpu::TextureDimension::D3 => wgpu::TextureViewDimension::D3,
        }
    }
}

impl MetalTexture {
    pub(crate) fn new(shared: &Arc<Shared>, desc: &wgpu::TextureDescriptor<'_>, live: Live) -> Self {
        let sample_count = desc.sample_count.max(1);
        if sample_count > 1 {
            if desc.dimension != wgpu::TextureDimension::D2 {
                super::unsupported!("multisampled {:?} textures", desc.dimension);
            }
            if !shared.device.supportsTextureSampleCount(sample_count as usize) {
                super::unsupported!("sample count {sample_count}");
            }
        }
        let layers = if desc.dimension == wgpu::TextureDimension::D3 { 1 } else { desc.size.depth_or_array_layers };
        let raw_type = descriptor_texture_type(desc.dimension, layers, sample_count);
        let (mtl, is_shared) = device_texture(&shared.device, desc, layers, raw_type, sample_count);
        let uses = (is_shared && desc.usage.contains(wgpu::TextureUsages::COPY_DST)).then(|| Arc::new(AtomicUsize::new(0)));
        let lazy_init = lazy_clear_texture(desc, is_shared).then(|| LazyInit::new(desc.mip_level_count, layers));
        let raw = TextureRaw { raw: mtl, format: desc.format, dimension: desc.dimension, raw_type, sample_count, layers,
            mips: desc.mip_level_count, drawable: false, shared_storage: is_shared, uses, lazy_init, owner: None, live: Some(live) };
        MetalTexture { raw: Arc::new(raw), shared: Some(shared.clone()) }
    }

    /// Over `raw`, a texture made elsewhere that matches `desc`, kept valid by `owner`.
    pub(crate) fn from_raw(raw: Obj<dyn MTLTexture>, desc: &wgpu::TextureDescriptor<'_>, owner: Owner) -> Self {
        let sample_count = desc.sample_count.max(1);
        let layers = if desc.dimension == wgpu::TextureDimension::D3 { 1 } else { desc.size.depth_or_array_layers };
        let raw_type = descriptor_texture_type(desc.dimension, layers, sample_count);
        let raw = TextureRaw { raw, format: desc.format, dimension: desc.dimension, raw_type, sample_count,
            layers: desc.size.depth_or_array_layers, mips: desc.mip_level_count, drawable: false, shared_storage: false,
            uses: None, lazy_init: None, owner: Some(owner), live: None };
        MetalTexture { raw: Arc::new(raw), shared: None }
    }
}

/// CPU-written or read-only sampled textures on Apple Silicon use `MTLStorageMode::Shared` so
/// creation zeroes on the CPU and idle updates write via `replaceRegion` without a blit encoder.
/// `supportsFamily(Apple1)` excludes Intel iGPUs (which report `hasUnifiedMemory()` but reject
/// shared textures).
fn shared_texture(device: &Obj<dyn MTLDevice>, desc: &wgpu::TextureDescriptor<'_>) -> bool {
    use wgpu::TextureUsages as U;
    device.supportsFamily(MTLGPUFamily::Apple1)
        && desc.sample_count == 1
        && !desc.format.is_depth_stencil_format()
        && desc.format.block_copy_size(None).is_some()
        && desc.format.block_dimensions() == (1, 1)
        && desc.usage.intersects(U::COPY_DST | U::TEXTURE_BINDING)
        && !desc.usage.intersects(U::RENDER_ATTACHMENT | U::STORAGE_BINDING | U::COPY_SRC)
}

/// Single-aspect 2D `Private` textures with `RENDER_ATTACHMENT` (single-sample or multisampled,
/// color, depth-only, or stencil-only) skip eager `zero_texture` blits and clear lazily via
/// render-pass load actions or clear-only passes. Combined depth-stencil formats clear eagerly
/// via a clear-only render pass so depth and stencil can have independent store ops.
fn lazy_clear_texture(desc: &wgpu::TextureDescriptor<'_>, is_shared: bool) -> bool {
    !is_shared
        && desc.dimension == wgpu::TextureDimension::D2
        && !desc.format.is_combined_depth_stencil_format()
        && desc.usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
}

fn device_texture(
    device: &Obj<dyn MTLDevice>,
    desc: &wgpu::TextureDescriptor<'_>,
    layers: u32,
    kind: MTLTextureType,
    sample_count: u32,
) -> (Obj<dyn MTLTexture>, bool) {
    use wgpu::TextureUsages as U;
    let d = MTLTextureDescriptor::new();
    d.setTextureType(kind);
    d.setPixelFormat(pixel_format(desc.format));
    // SAFETY: nonzero sizes and validated sample count, within the device's limits.
    unsafe {
        d.setWidth(desc.size.width.max(1) as usize);
        d.setHeight(desc.size.height.max(1) as usize);
        d.setDepth(if kind == MTLTextureType::Type3D { desc.size.depth_or_array_layers.max(1) as usize } else { 1 });
        d.setArrayLength(layers.max(1) as usize);
        d.setMipmapLevelCount(desc.mip_level_count.max(1) as usize);
        if sample_count > 1 {
            d.setSampleCount(sample_count as usize);
        }
    }
    let mut usage = MTLTextureUsage::Unknown;
    if desc.usage.intersects(U::TEXTURE_BINDING | U::STORAGE_BINDING) { usage |= MTLTextureUsage::ShaderRead; }
    if desc.usage.contains(U::STORAGE_BINDING) { usage |= MTLTextureUsage::ShaderWrite; }
    if desc.usage.contains(U::RENDER_ATTACHMENT) { usage |= MTLTextureUsage::RenderTarget; }
    if desc.view_formats.iter().any(|f| *f != desc.format) || desc.format.is_combined_depth_stencil_format() {
        usage |= MTLTextureUsage::PixelFormatView;
    }
    d.setUsage(usage);
    let shared = shared_texture(device, desc);
    d.setStorageMode(if shared { MTLStorageMode::Shared } else { MTLStorageMode::Private });
    let raw = device.newTextureWithDescriptor(&d).expect("a Metal texture");
    #[cfg(debug_assertions)]
    if let Some(label) = desc.label { raw.setLabel(Some(&NSString::from_str(label))); }
    (raw, shared)
}

impl TextureInterface for MetalTexture {
    fn create_view(&self, desc: &wgpu::TextureViewDescriptor<'_>) -> DispatchTextureView {
        let t = &self.raw;
        let format = desc.format.unwrap_or(t.format);
        let dimension = desc.dimension.unwrap_or_else(|| t.view_dimension());
        let mips = desc.mip_level_count.unwrap_or(t.mips - desc.base_mip_level);
        let layers = desc.array_layer_count.unwrap_or(t.layers - desc.base_array_layer);
        let raw_format = view_pixel_format(t.format, desc.format, desc.aspect);
        let raw_type = texture_type(dimension, t.sample_count);
        let whole = raw_format == pixel_format(t.format) && raw_type == t.raw_type && desc.base_mip_level == 0 && mips == t.mips
            && desc.base_array_layer == 0 && layers == t.layers;
        let raw = if whole {
            t.raw.clone()
        } else {
            let levels = NSRange::new(desc.base_mip_level as usize, mips as usize);
            let slices = NSRange::new(desc.base_array_layer as usize, layers as usize);
            // SAFETY: the ranges are within the texture, as wgpu's validation would require.
            let view = unsafe { t.raw.newTextureViewWithPixelFormat_textureType_levels_slices(raw_format, raw_type, levels, slices) }
                .expect("a Metal texture view");
            #[cfg(debug_assertions)]
            if let Some(label) = desc.label {
                view.setLabel(Some(&NSString::from_str(label)));
            } else if let Some(label) = t.raw.label() {
                view.setLabel(Some(&label));
            }
            view
        };
        let texture = t.is_lazy().then(|| self.raw.clone());
        let range = SubresourceRange { base_mip: desc.base_mip_level, mips, base_layer: desc.base_array_layer, layers };
        DispatchTextureView::custom(MetalView { raw: Arc::new(ViewRaw {
            raw, format, aspect: desc.aspect, drawable: t.drawable, uses: t.uses.clone(), texture, range, owner: t.owner.clone(),
        }) })
    }
    fn destroy(&self) {}
}

impl TextureViewInterface for MetalView {}

fn address(mode: wgpu::AddressMode) -> MTLSamplerAddressMode {
    match mode {
        wgpu::AddressMode::ClampToEdge => MTLSamplerAddressMode::ClampToEdge,
        wgpu::AddressMode::Repeat => MTLSamplerAddressMode::Repeat,
        wgpu::AddressMode::MirrorRepeat => MTLSamplerAddressMode::MirrorRepeat,
        wgpu::AddressMode::ClampToBorder => MTLSamplerAddressMode::ClampToBorderColor,
    }
}

fn min_mag(mode: wgpu::FilterMode) -> MTLSamplerMinMagFilter {
    match mode {
        wgpu::FilterMode::Nearest => MTLSamplerMinMagFilter::Nearest,
        wgpu::FilterMode::Linear => MTLSamplerMinMagFilter::Linear,
    }
}

pub(super) fn compare(function: wgpu::CompareFunction) -> MTLCompareFunction {
    use wgpu::CompareFunction as C;
    match function {
        C::Never => MTLCompareFunction::Never,
        C::Less => MTLCompareFunction::Less,
        C::Equal => MTLCompareFunction::Equal,
        C::LessEqual => MTLCompareFunction::LessEqual,
        C::Greater => MTLCompareFunction::Greater,
        C::NotEqual => MTLCompareFunction::NotEqual,
        C::GreaterEqual => MTLCompareFunction::GreaterEqual,
        C::Always => MTLCompareFunction::Always,
    }
}

impl MetalSampler {
    pub(crate) fn new(device: &Obj<dyn MTLDevice>, desc: &wgpu::SamplerDescriptor<'_>) -> Self {
        let d = MTLSamplerDescriptor::new();
        d.setSAddressMode(address(desc.address_mode_u));
        d.setTAddressMode(address(desc.address_mode_v));
        d.setRAddressMode(address(desc.address_mode_w));
        d.setMinFilter(min_mag(desc.min_filter));
        d.setMagFilter(min_mag(desc.mag_filter));
        d.setMipFilter(match desc.mipmap_filter {
            wgpu::MipmapFilterMode::Nearest => MTLSamplerMipFilter::Nearest,
            wgpu::MipmapFilterMode::Linear => MTLSamplerMipFilter::Linear,
        });
        d.setLodMinClamp(desc.lod_min_clamp);
        d.setLodMaxClamp(desc.lod_max_clamp);
        d.setMaxAnisotropy(usize::from(desc.anisotropy_clamp.max(1)));
        if let Some(function) = desc.compare { d.setCompareFunction(compare(function)); }
        d.setBorderColor(match desc.border_color {
            Some(wgpu::SamplerBorderColor::OpaqueBlack) => MTLSamplerBorderColor::OpaqueBlack,
            Some(wgpu::SamplerBorderColor::OpaqueWhite) => MTLSamplerBorderColor::OpaqueWhite,
            _ => MTLSamplerBorderColor::TransparentBlack,
        });
        #[cfg(debug_assertions)]
        if let Some(label) = desc.label { d.setLabel(Some(&NSString::from_str(label))); }
        MetalSampler { raw: Arc::new(SamplerRaw(device.newSamplerStateWithDescriptor(&d).expect("a Metal sampler"))) }
    }
}

impl SamplerInterface for MetalSampler {}
