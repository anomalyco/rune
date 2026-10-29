//! Spike (research/34, phase 0): a native Metal backend behind `RUNE_GPU_BACKEND=metal`, through
//! wgpu's `custom` backend, so compositors and the frame loop keep calling wgpu types.
//!
//! A frame is one `MTLCommandBuffer`: the encoder makes it on its first pass, a submit that drew
//! into the drawable holds it, and `Queue::present` presents the drawable on it and commits it.
//! Buffers are `MTLStorageModeShared` and `Queue::write_buffer` copies into them directly. Every
//! commit signals an `MTLSharedEvent`, and acquiring the next drawable waits on it when two
//! frames are in flight. There are no completion handlers.
//!
//! Only what the overlay HUD workload needs works: render passes that clear or load (draws are
//! dropped), buffers and plain 2D textures. Shader modules, pipelines, bind groups and samplers
//! are inert handles, and copies and texture writes do nothing. A direct write can race a frame
//! still on the GPU that reads the same buffer.

pub(crate) mod bench;
mod encode;
mod surface;
#[cfg(test)]
mod shader_tests;
#[cfg(test)]
mod tests;

use std::future::ready;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use objc2_metal::{MTLCommandBuffer, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLEvent, MTLSharedEvent};
use objc2_video::rc::Retained;
use objc2_video::runtime::ProtocolObject;
use wgpu::custom::*;

use encode::{Inert, MetalBuffer, MetalEncoder, MetalTexture};

pub(crate) type Obj<P> = Retained<ProtocolObject<P>>;

/// Frames the CPU may run ahead of the GPU.
const IN_FLIGHT: u64 = 2;

/// Whether `RUNE_GPU_BACKEND=metal` asks for this backend.
pub(crate) fn enabled() -> bool {
    std::env::var("RUNE_GPU_BACKEND").is_ok_and(|v| v == "metal")
}

pub(crate) fn instance() -> wgpu::Instance {
    wgpu::Instance::from_custom(MetalInstance)
}

/// Whether `device` is this backend's: its submit and present cost too little to hand to a thread.
pub(crate) fn native(device: &wgpu::Device) -> bool {
    device.as_custom::<MetalDevice>().is_some()
}

macro_rules! send_sync {
    ($($t:ty),*) => { $(
        // SAFETY: Metal devices, queues, resources and events are thread-safe; command buffers,
        // encoders and drawables are used by one thread at a time, as wgpu's handles are.
        unsafe impl Send for $t {}
        unsafe impl Sync for $t {}
    )* };
}
pub(crate) use send_sync;

/// The device, its one queue, the pacing event, and the frame's command buffer while it waits
/// for its drawable's present.
#[derive(Debug)]
pub(crate) struct Shared {
    device: Obj<dyn MTLDevice>,
    queue: Obj<dyn MTLCommandQueue>,
    event: Obj<dyn MTLSharedEvent>,
    committed: AtomicU64,
    held: Mutex<Option<Obj<dyn MTLCommandBuffer>>>,
}
send_sync!(Shared, MetalAdapter);

impl Shared {
    fn commit(&self, cb: &ProtocolObject<dyn MTLCommandBuffer>) {
        let n = self.committed.fetch_add(1, Ordering::Relaxed) + 1;
        cb.encodeSignalEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&*self.event), n);
        cb.commit();
    }

    /// Commits a held buffer whose drawable was dropped or never presented.
    fn flush_held(&self) {
        if let Some(cb) = self.held.lock().unwrap().take() { self.commit(&cb); }
    }

    /// Waits until at most `IN_FLIGHT - 1` committed frames are still on the GPU.
    fn pace(&self) {
        let committed = self.committed.load(Ordering::Relaxed);
        if committed < IN_FLIGHT { return; }
        let target = committed - (IN_FLIGHT - 1);
        if self.event.signaledValue() < target { self.event.waitUntilSignaledValue_timeoutMS(target, 1000); }
    }
}

#[derive(Debug)]
struct MetalInstance;

impl InstanceInterface for MetalInstance {
    fn new(_: wgpu::InstanceDescriptor) -> Self { MetalInstance }
    unsafe fn create_surface(&self, target: wgpu::SurfaceTargetUnsafe) -> Result<DispatchSurface, wgpu::CreateSurfaceError> {
        Ok(unsafe { surface::create(target) })
    }
    fn request_adapter(&self, _: &wgpu::RequestAdapterOptions<'_, '_>) -> Pin<Box<dyn RequestAdapterFuture>> {
        Box::pin(ready(Ok(DispatchAdapter::custom(MetalAdapter::new()))))
    }
    fn poll_all_devices(&self, _: bool) -> bool { true }
    fn wgsl_language_features(&self) -> wgpu::WgslLanguageFeatures { wgpu::WgslLanguageFeatures::empty() }
    fn enumerate_adapters(&self, _: wgpu::Backends) -> Pin<Box<dyn EnumerateAdapterFuture>> {
        Box::pin(ready(vec![DispatchAdapter::custom(MetalAdapter::new())]))
    }
}

#[derive(Debug)]
struct MetalAdapter {
    device: Obj<dyn MTLDevice>,
}

impl MetalAdapter {
    fn new() -> Self {
        MetalAdapter { device: MTLCreateSystemDefaultDevice().expect("RUNE_GPU_BACKEND=metal needs a Metal device") }
    }
}

fn info(device: &ProtocolObject<dyn MTLDevice>) -> wgpu::AdapterInfo {
    let mut info = wgpu::AdapterInfo::new(wgpu::DeviceType::IntegratedGpu, wgpu::Backend::Metal);
    info.name = device.name().to_string();
    info.driver = "rune metal spike".into();
    info
}

impl AdapterInterface for MetalAdapter {
    fn request_device(&self, _: &wgpu::DeviceDescriptor<'_>) -> Pin<Box<dyn RequestDeviceFuture>> {
        let shared = Arc::new(Shared {
            device: self.device.clone(),
            queue: self.device.newCommandQueue().expect("a Metal command queue"),
            event: self.device.newSharedEvent().expect("a Metal shared event"),
            committed: AtomicU64::new(0),
            held: Mutex::new(None),
        });
        Box::pin(ready(Ok((DispatchDevice::custom(MetalDevice(shared.clone())), DispatchQueue::custom(MetalQueue(shared))))))
    }
    fn is_surface_supported(&self, _: &DispatchSurface) -> bool { true }
    fn features(&self) -> wgpu::Features { wgpu::Features::empty() }
    fn limits(&self) -> wgpu::Limits { wgpu::Limits::default() }
    fn downlevel_capabilities(&self) -> wgpu::DownlevelCapabilities { Default::default() }
    fn get_info(&self) -> wgpu::AdapterInfo { info(&self.device) }
    fn get_texture_format_features(&self, _: wgpu::TextureFormat) -> wgpu::TextureFormatFeatures {
        use wgpu::TextureFormatFeatureFlags as F;
        wgpu::TextureFormatFeatures { allowed_usages: wgpu::TextureUsages::all(), flags: F::FILTERABLE | F::BLENDABLE }
    }
    fn get_presentation_timestamp(&self) -> wgpu::PresentationTimestamp { wgpu::PresentationTimestamp(0) }
    fn cooperative_matrix_properties(&self) -> Vec<wgpu::wgt::CooperativeMatrixProperties> { Vec::new() }
}

#[derive(Debug)]
pub(crate) struct MetalDevice(pub(crate) Arc<Shared>);

impl DeviceInterface for MetalDevice {
    fn features(&self) -> wgpu::Features { wgpu::Features::empty() }
    fn limits(&self) -> wgpu::Limits { wgpu::Limits::default() }
    fn adapter_info(&self) -> wgpu::AdapterInfo { info(&self.0.device) }
    fn create_shader_module(&self, _: wgpu::ShaderModuleDescriptor<'_>, _: wgpu::ShaderRuntimeChecks) -> DispatchShaderModule {
        DispatchShaderModule::custom(Inert)
    }
    unsafe fn create_shader_module_passthrough(&self, _: &wgpu::ShaderModuleDescriptorPassthrough<'_>) -> DispatchShaderModule {
        DispatchShaderModule::custom(Inert)
    }
    fn create_bind_group_layout(&self, _: &wgpu::BindGroupLayoutDescriptor<'_>) -> DispatchBindGroupLayout { DispatchBindGroupLayout::custom(Inert) }
    fn create_bind_group(&self, _: &wgpu::BindGroupDescriptor<'_>) -> DispatchBindGroup { DispatchBindGroup::custom(Inert) }
    fn create_pipeline_layout(&self, _: &wgpu::PipelineLayoutDescriptor<'_>) -> DispatchPipelineLayout { DispatchPipelineLayout::custom(Inert) }
    fn create_render_pipeline(&self, _: &wgpu::RenderPipelineDescriptor<'_>) -> DispatchRenderPipeline { DispatchRenderPipeline::custom(Inert) }
    fn create_mesh_pipeline(&self, _: &wgpu::MeshPipelineDescriptor<'_>) -> DispatchRenderPipeline { DispatchRenderPipeline::custom(Inert) }
    fn create_compute_pipeline(&self, _: &wgpu::ComputePipelineDescriptor<'_>) -> DispatchComputePipeline { DispatchComputePipeline::custom(Inert) }
    unsafe fn create_pipeline_cache(&self, _: &wgpu::PipelineCacheDescriptor<'_>) -> DispatchPipelineCache { DispatchPipelineCache::custom(Inert) }
    fn create_buffer(&self, desc: &wgpu::BufferDescriptor<'_>) -> DispatchBuffer { DispatchBuffer::custom(MetalBuffer::new(&self.0.device, desc.size)) }
    fn create_texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> DispatchTexture { DispatchTexture::custom(MetalTexture::new(&self.0.device, desc)) }
    fn create_external_texture(&self, _: &wgpu::ExternalTextureDescriptor<'_>, _: &[&wgpu::TextureView]) -> DispatchExternalTexture {
        unimplemented!("metal spike: external textures")
    }
    fn create_blas(&self, _: &wgpu::CreateBlasDescriptor<'_>, _: wgpu::BlasGeometrySizeDescriptors) -> (Option<u64>, DispatchBlas) {
        unimplemented!("metal spike: ray tracing")
    }
    fn create_tlas(&self, _: &wgpu::CreateTlasDescriptor<'_>) -> DispatchTlas { unimplemented!("metal spike: ray tracing") }
    fn create_sampler(&self, _: &wgpu::SamplerDescriptor<'_>) -> DispatchSampler { DispatchSampler::custom(Inert) }
    fn create_query_set(&self, _: &wgpu::QuerySetDescriptor<'_>) -> DispatchQuerySet { DispatchQuerySet::custom(Inert) }
    fn create_command_encoder(&self, _: &wgpu::CommandEncoderDescriptor<'_>) -> DispatchCommandEncoder {
        DispatchCommandEncoder::custom(MetalEncoder::new(self.0.clone()))
    }
    fn create_render_bundle_encoder(&self, _: &wgpu::RenderBundleEncoderDescriptor<'_>) -> DispatchRenderBundleEncoder {
        unimplemented!("metal spike: render bundles")
    }
    fn set_device_lost_callback(&self, _: BoxDeviceLostCallback) {}
    fn on_uncaptured_error(&self, _: Arc<dyn wgpu::UncapturedErrorHandler>) {}
    fn push_error_scope(&self, _: wgpu::ErrorFilter) -> u32 { 0 }
    fn pop_error_scope(&self, _: u32) -> Pin<Box<dyn PopErrorScopeFuture>> { Box::pin(ready(None)) }
    unsafe fn start_graphics_debugger_capture(&self) {}
    unsafe fn stop_graphics_debugger_capture(&self) {}
    fn poll(&self, _: wgpu::wgt::PollType<u64>) -> Result<wgpu::PollStatus, wgpu::PollError> { Ok(wgpu::PollStatus::QueueEmpty) }
    fn get_internal_counters(&self) -> wgpu::InternalCounters { Default::default() }
    fn generate_allocator_report(&self) -> Option<wgpu::AllocatorReport> { None }
    fn destroy(&self) {}
}

#[derive(Debug)]
struct MetalQueue(Arc<Shared>);

impl QueueInterface for MetalQueue {
    fn write_buffer(&self, buffer: &DispatchBuffer, offset: wgpu::BufferAddress, data: &[u8]) {
        buffer.as_custom::<MetalBuffer>().expect("a metal spike buffer").write(offset, data);
    }
    fn create_staging_buffer(&self, _: wgpu::BufferSize) -> Option<DispatchQueueWriteBuffer> { None }
    fn validate_write_buffer(&self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: wgpu::BufferSize) -> Option<()> { Some(()) }
    fn write_staging_buffer(&self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: &DispatchQueueWriteBuffer) {}
    fn write_texture(&self, _: wgpu::TexelCopyTextureInfo<'_>, _: &[u8], _: wgpu::TexelCopyBufferLayout, _: wgpu::Extent3d) {}
    fn submit(&self, command_buffers: &mut dyn Iterator<Item = DispatchCommandBuffer>) -> u64 {
        for buffer in command_buffers {
            let Some((cb, drawable)) = buffer.as_custom::<encode::MetalCommandBuffer>().and_then(|b| Some((b.cb.clone()?, b.drawable))) else { continue };
            self.0.flush_held();
            if drawable { *self.0.held.lock().unwrap() = Some(cb); } else { self.0.commit(&cb); }
        }
        self.0.committed.load(Ordering::Relaxed)
    }
    fn get_timestamp_period(&self) -> f32 { 1.0 }
    fn on_submitted_work_done(&self, callback: BoxSubmittedWorkDoneCallback) { callback() }
    fn compact_blas(&self, _: &DispatchBlas) -> (Option<u64>, DispatchBlas) { unimplemented!("metal spike: ray tracing") }
    fn present(&self, detail: &DispatchSurfaceOutputDetail) {
        let Some(drawable) = detail.as_custom::<surface::MetalOutput>().and_then(|o| o.drawable.as_ref()) else { return };
        let held = self.0.held.lock().unwrap().take();
        let Some(cb) = held.or_else(|| self.0.queue.commandBufferWithUnretainedReferences()) else { return };
        cb.presentDrawable(ProtocolObject::from_ref(&**drawable));
        self.0.commit(&cb);
    }
}
