//! The device: what it shares with its queue, and the resources it makes.

use std::future::ready;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

use objc2_metal::{MTLCommandQueue, MTLDevice, MTLLanguageVersion, MTLSharedEvent};
use parking_lot::Mutex;
use wgpu::custom::*;

use super::encode::{Inert, MetalEncoder};
use super::layout::{BindGroup, BindGroupLayout, PipelineLayout};
use super::resource::{MetalBuffer, MetalSampler, MetalTexture};
use super::shader::ShaderModule;
use super::submit::{MetalQueue, State};
use super::{Obj, info, send_sync};

/// The device, its one queue, the event every command buffer signals on completion, and the
/// submission state behind it.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(super) device: Obj<dyn MTLDevice>,
    pub(super) queue: Obj<dyn MTLCommandQueue>,
    pub(super) event: Obj<dyn MTLSharedEvent>,
    pub(super) msl: ((u8, u8), MTLLanguageVersion),
    pub(super) state: Mutex<State>,
    /// Buffers alive, for `Device::get_internal_counters`.
    pub(super) buffers: Arc<AtomicIsize>,
    /// Encoders that made a command buffer not yet submitted or dropped. While none has, every
    /// recorded command that reads a buffer's storage was submitted.
    pub(super) recording: AtomicUsize,
}
send_sync!(Shared);

#[derive(Debug)]
pub(crate) struct MetalDevice {
    pub(crate) shared: Arc<Shared>,
    features: wgpu::Features,
    limits: wgpu::Limits,
}

pub(super) fn new(device: Obj<dyn MTLDevice>, desc: &wgpu::DeviceDescriptor<'_>) -> (MetalDevice, MetalQueue) {
    let shared = Arc::new(Shared {
        queue: device.newCommandQueue().expect("a Metal command queue"),
        event: device.newSharedEvent().expect("a Metal shared event"),
        msl: msl_version(),
        state: Mutex::new(State::default()),
        buffers: Arc::default(),
        recording: AtomicUsize::new(0),
        device,
    });
    let device = MetalDevice { shared: shared.clone(), features: desc.required_features, limits: desc.required_limits.clone() };
    (device, MetalQueue(shared))
}

/// wgpu-hal's choice of MSL version for this macOS.
fn msl_version() -> ((u8, u8), MTLLanguageVersion) {
    let mut buf = [0u8; 32];
    let mut len = buf.len();
    // SAFETY: sysctlbyname writes at most `len` bytes into `buf` and the length it wrote into `len`.
    let ok = unsafe { libc::sysctlbyname(c"kern.osproductversion".as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) } == 0;
    let version = if ok { String::from_utf8_lossy(&buf[..len.saturating_sub(1)]).into_owned() } else { String::new() };
    let major: u32 = version.split('.').next().and_then(|m| m.trim().parse().ok()).unwrap_or(0);
    match major {
        26.. => ((4, 0), MTLLanguageVersion::Version4_0),
        15 => ((3, 2), MTLLanguageVersion::Version3_2),
        14 => ((3, 1), MTLLanguageVersion::Version3_1),
        _ => ((3, 0), MTLLanguageVersion::Version3_0),
    }
}

impl DeviceInterface for MetalDevice {
    fn features(&self) -> wgpu::Features { self.features }
    fn limits(&self) -> wgpu::Limits { self.limits.clone() }
    fn adapter_info(&self) -> wgpu::AdapterInfo { info(&self.shared.device) }
    fn create_shader_module(&self, desc: wgpu::ShaderModuleDescriptor<'_>, checks: wgpu::ShaderRuntimeChecks) -> DispatchShaderModule {
        DispatchShaderModule::custom(ShaderModule::new(desc, checks))
    }
    unsafe fn create_shader_module_passthrough(&self, _: &wgpu::ShaderModuleDescriptorPassthrough<'_>) -> DispatchShaderModule {
        unimplemented!("RUNE_GPU_BACKEND=metal doesn't offer PASSTHROUGH_SHADERS")
    }
    fn create_bind_group_layout(&self, desc: &wgpu::BindGroupLayoutDescriptor<'_>) -> DispatchBindGroupLayout {
        DispatchBindGroupLayout::custom(BindGroupLayout::new(desc))
    }
    fn create_bind_group(&self, desc: &wgpu::BindGroupDescriptor<'_>) -> DispatchBindGroup { DispatchBindGroup::custom(BindGroup::new(desc)) }
    fn create_pipeline_layout(&self, desc: &wgpu::PipelineLayoutDescriptor<'_>) -> DispatchPipelineLayout {
        DispatchPipelineLayout::custom(PipelineLayout::new(desc))
    }
    fn create_render_pipeline(&self, desc: &wgpu::RenderPipelineDescriptor<'_>) -> DispatchRenderPipeline {
        DispatchRenderPipeline::custom(super::pipeline::create(&self.shared, desc))
    }
    fn create_mesh_pipeline(&self, _: &wgpu::MeshPipelineDescriptor<'_>) -> DispatchRenderPipeline {
        unimplemented!("RUNE_GPU_BACKEND=metal: mesh shaders")
    }
    fn create_compute_pipeline(&self, _: &wgpu::ComputePipelineDescriptor<'_>) -> DispatchComputePipeline {
        unimplemented!("RUNE_GPU_BACKEND=metal: compute")
    }
    unsafe fn create_pipeline_cache(&self, _: &wgpu::PipelineCacheDescriptor<'_>) -> DispatchPipelineCache { DispatchPipelineCache::custom(Inert) }
    fn create_buffer(&self, desc: &wgpu::BufferDescriptor<'_>) -> DispatchBuffer {
        DispatchBuffer::custom(MetalBuffer::new(&self.shared, desc.size))
    }
    fn create_texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> DispatchTexture {
        DispatchTexture::custom(MetalTexture::new(&self.shared.device, desc))
    }
    fn create_external_texture(&self, _: &wgpu::ExternalTextureDescriptor<'_>, _: &[&wgpu::TextureView]) -> DispatchExternalTexture {
        unimplemented!("RUNE_GPU_BACKEND=metal: external textures")
    }
    fn create_blas(&self, _: &wgpu::CreateBlasDescriptor<'_>, _: wgpu::BlasGeometrySizeDescriptors) -> (Option<u64>, DispatchBlas) {
        unimplemented!("RUNE_GPU_BACKEND=metal: ray tracing")
    }
    fn create_tlas(&self, _: &wgpu::CreateTlasDescriptor<'_>) -> DispatchTlas { unimplemented!("RUNE_GPU_BACKEND=metal: ray tracing") }
    fn create_sampler(&self, desc: &wgpu::SamplerDescriptor<'_>) -> DispatchSampler {
        DispatchSampler::custom(MetalSampler::new(&self.shared.device, desc))
    }
    fn create_query_set(&self, _: &wgpu::QuerySetDescriptor<'_>) -> DispatchQuerySet {
        unimplemented!("RUNE_GPU_BACKEND=metal: query sets")
    }
    fn create_command_encoder(&self, _: &wgpu::CommandEncoderDescriptor<'_>) -> DispatchCommandEncoder {
        DispatchCommandEncoder::custom(MetalEncoder::new(self.shared.clone()))
    }
    fn create_render_bundle_encoder(&self, _: &wgpu::RenderBundleEncoderDescriptor<'_>) -> DispatchRenderBundleEncoder {
        unimplemented!("RUNE_GPU_BACKEND=metal: render bundles")
    }
    fn set_device_lost_callback(&self, _: BoxDeviceLostCallback) {}
    fn on_uncaptured_error(&self, _: Arc<dyn wgpu::UncapturedErrorHandler>) {}
    // The backend validates nothing, so a scope never catches an error.
    fn push_error_scope(&self, _: wgpu::ErrorFilter) -> u32 { 0 }
    fn pop_error_scope(&self, _: u32) -> Pin<Box<dyn PopErrorScopeFuture>> { Box::pin(ready(None)) }
    unsafe fn start_graphics_debugger_capture(&self) {}
    unsafe fn stop_graphics_debugger_capture(&self) {}
    fn poll(&self, poll: wgpu::wgt::PollType<u64>) -> Result<wgpu::PollStatus, wgpu::PollError> { self.shared.poll(poll) }
    fn get_internal_counters(&self) -> wgpu::InternalCounters {
        let counters = wgpu::InternalCounters::default();
        counters.hal.buffers.set(self.shared.buffers.load(Ordering::Relaxed));
        counters
    }
    fn generate_allocator_report(&self) -> Option<wgpu::AllocatorReport> { None }
    fn destroy(&self) {}
}
