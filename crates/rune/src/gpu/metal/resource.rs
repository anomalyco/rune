//! Buffers, textures, views and samplers.

use std::ops::Range;
use std::ptr::NonNull;
use std::sync::Arc;

use objc2_foundation_video::NSRange;
use objc2_metal::{
    MTLBuffer, MTLCompareFunction, MTLDevice, MTLPixelFormat, MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerBorderColor,
    MTLSamplerDescriptor, MTLSamplerMinMagFilter, MTLSamplerMipFilter, MTLSamplerState, MTLStorageMode, MTLTexture, MTLTextureDescriptor,
    MTLTextureType, MTLTextureUsage,
};
use wgpu::custom::*;

use super::{Obj, Shared, send_sync};

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
        F::Depth32Float => M::Depth32Float,
        F::Depth16Unorm => M::Depth16Unorm,
        other => unimplemented!("RUNE_GPU_BACKEND=metal: texture format {other:?}"),
    }
}

/// A buffer's storage, which its CPU pointer stays valid with.
#[derive(Debug)]
pub(crate) struct BufferRaw {
    pub(super) raw: Obj<dyn MTLBuffer>,
    contents: NonNull<u8>,
    pub(super) size: u64,
}

#[derive(Debug)]
pub(crate) struct MetalBuffer {
    pub(super) raw: Arc<BufferRaw>,
    shared: Arc<Shared>,
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
    layers: u32,
    mips: u32,
    pub(super) drawable: bool,
}

#[derive(Debug)]
pub(crate) struct MetalTexture {
    pub(super) raw: Arc<TextureRaw>,
}

#[derive(Debug)]
pub(crate) struct ViewRaw {
    pub(super) raw: Obj<dyn MTLTexture>,
    pub(super) drawable: bool,
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

impl BufferRaw {
    fn from(raw: Obj<dyn MTLBuffer>, size: u64) -> Self {
        let contents = raw.contents().cast();
        BufferRaw { raw, contents, size }
    }

    /// A shared buffer of `size` zeroed bytes.
    pub(super) fn new(device: &Obj<dyn MTLDevice>, size: u64) -> Self {
        let raw = device.newBufferWithLength_options(size.max(4) as usize, MTLResourceOptions::StorageModeShared).expect("a Metal buffer");
        Self::from(raw, size)
    }

    /// A shared buffer holding a copy of `data`, which is not empty.
    pub(super) fn with_bytes(device: &Obj<dyn MTLDevice>, data: &[u8]) -> Self {
        // SAFETY: `data` is valid for its length, which is nonzero.
        let raw = unsafe {
            device.newBufferWithBytes_length_options(NonNull::from(data).cast(), data.len(), MTLResourceOptions::StorageModeShared)
        }.expect("a Metal buffer");
        Self::from(raw, data.len() as u64)
    }

    /// Writes `data` at `offset` from the CPU, which the GPU must not be reading then.
    pub(super) fn write(&self, offset: u64, data: &[u8]) {
        assert!(offset + data.len() as u64 <= self.size, "a write past the end of a buffer");
        // SAFETY: in bounds, checked above, of storage that lives as long as `raw`.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), self.contents.as_ptr().add(offset as usize), data.len()) };
    }

    fn range(&self, range: Range<u64>) -> MappedRange {
        assert!(range.start <= range.end && range.end <= self.size, "a mapped range past the end of a buffer");
        // SAFETY: in bounds, checked above.
        MappedRange { at: unsafe { self.contents.add(range.start as usize) }, len: (range.end - range.start) as usize }
    }
}

impl MetalBuffer {
    pub(crate) fn new(shared: &Arc<Shared>, size: u64) -> Self {
        MetalBuffer { raw: Arc::new(BufferRaw::new(&shared.device, size)), shared: shared.clone() }
    }
}

impl BufferInterface for MetalBuffer {
    fn map_async(&self, _: wgpu::MapMode, _: Range<wgpu::BufferAddress>, callback: BufferMapCallback) {
        self.shared.map_after_work(callback);
    }
    fn get_mapped_range(&self, range: Range<wgpu::BufferAddress>) -> Result<DispatchBufferMappedRange, wgpu::MapRangeError> {
        Ok(DispatchBufferMappedRange::custom(self.raw.range(range)))
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

fn texture_type(dimension: wgpu::TextureViewDimension) -> MTLTextureType {
    use wgpu::TextureViewDimension as D;
    match dimension {
        D::D1 => MTLTextureType::Type1D,
        D::D2 => MTLTextureType::Type2D,
        D::D2Array => MTLTextureType::Type2DArray,
        D::Cube => MTLTextureType::TypeCube,
        D::CubeArray => MTLTextureType::TypeCubeArray,
        D::D3 => MTLTextureType::Type3D,
    }
}

impl TextureRaw {
    /// A drawable's texture, which a surface hands out for one frame.
    pub(super) fn drawable(raw: Obj<dyn MTLTexture>, format: wgpu::TextureFormat) -> Self {
        TextureRaw { raw, format, dimension: wgpu::TextureDimension::D2, layers: 1, mips: 1, drawable: true }
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
    pub(crate) fn new(device: &Obj<dyn MTLDevice>, desc: &wgpu::TextureDescriptor<'_>) -> Self {
        assert_eq!(desc.sample_count, 1, "RUNE_GPU_BACKEND=metal: multisampled textures");
        let layers = if desc.dimension == wgpu::TextureDimension::D3 { 1 } else { desc.size.depth_or_array_layers };
        let raw = TextureRaw { raw: device_texture(device, desc, layers), format: desc.format, dimension: desc.dimension, layers,
            mips: desc.mip_level_count, drawable: false };
        MetalTexture { raw: Arc::new(raw) }
    }
}

fn device_texture(device: &Obj<dyn MTLDevice>, desc: &wgpu::TextureDescriptor<'_>, layers: u32) -> Obj<dyn MTLTexture> {
    use wgpu::TextureUsages as U;
    let d = MTLTextureDescriptor::new();
    let kind = match desc.dimension {
        wgpu::TextureDimension::D1 => MTLTextureType::Type1D,
        wgpu::TextureDimension::D2 if layers > 1 => MTLTextureType::Type2DArray,
        wgpu::TextureDimension::D2 => MTLTextureType::Type2D,
        wgpu::TextureDimension::D3 => MTLTextureType::Type3D,
    };
    d.setTextureType(kind);
    d.setPixelFormat(pixel_format(desc.format));
    // SAFETY: nonzero sizes, within the device's limits as wgpu's validation would require.
    unsafe {
        d.setWidth(desc.size.width.max(1) as usize);
        d.setHeight(desc.size.height.max(1) as usize);
        d.setDepth(if kind == MTLTextureType::Type3D { desc.size.depth_or_array_layers.max(1) as usize } else { 1 });
        d.setArrayLength(layers.max(1) as usize);
        d.setMipmapLevelCount(desc.mip_level_count.max(1) as usize);
    }
    let mut usage = MTLTextureUsage::Unknown;
    if desc.usage.intersects(U::TEXTURE_BINDING | U::STORAGE_BINDING) { usage |= MTLTextureUsage::ShaderRead; }
    if desc.usage.contains(U::STORAGE_BINDING) { usage |= MTLTextureUsage::ShaderWrite; }
    if desc.usage.contains(U::RENDER_ATTACHMENT) { usage |= MTLTextureUsage::RenderTarget; }
    if desc.view_formats.iter().any(|f| *f != desc.format) { usage |= MTLTextureUsage::PixelFormatView; }
    d.setUsage(usage);
    d.setStorageMode(MTLStorageMode::Private);
    device.newTextureWithDescriptor(&d).expect("a Metal texture")
}

impl TextureInterface for MetalTexture {
    fn create_view(&self, desc: &wgpu::TextureViewDescriptor<'_>) -> DispatchTextureView {
        let t = &self.raw;
        let format = desc.format.unwrap_or(t.format);
        let dimension = desc.dimension.unwrap_or_else(|| t.view_dimension());
        let mips = desc.mip_level_count.unwrap_or(t.mips - desc.base_mip_level);
        let layers = desc.array_layer_count.unwrap_or(t.layers - desc.base_array_layer);
        let whole = format == t.format && dimension == t.view_dimension() && desc.base_mip_level == 0 && mips == t.mips
            && desc.base_array_layer == 0 && layers == t.layers;
        let raw = if whole {
            t.raw.clone()
        } else {
            let levels = NSRange::new(desc.base_mip_level as usize, mips as usize);
            let slices = NSRange::new(desc.base_array_layer as usize, layers as usize);
            // SAFETY: the ranges are within the texture, as wgpu's validation would require.
            unsafe { t.raw.newTextureViewWithPixelFormat_textureType_levels_slices(pixel_format(format), texture_type(dimension), levels, slices) }
                .expect("a Metal texture view")
        };
        DispatchTextureView::custom(MetalView { raw: Arc::new(ViewRaw { raw, drawable: t.drawable }) })
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
        MetalSampler { raw: Arc::new(SamplerRaw(device.newSamplerStateWithDescriptor(&d).expect("a Metal sampler"))) }
    }
}

impl SamplerInterface for MetalSampler {}
