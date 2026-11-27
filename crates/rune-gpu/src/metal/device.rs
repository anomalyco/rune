//! The device: what it shares with its queue, and the resources it makes.

#![allow(unsafe_code)]

use std::future::ready;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

#[cfg(debug_assertions)]
use objc2_foundation::NSString;
#[cfg(debug_assertions)]
use objc2_metal::MTLEvent;
use objc2_metal::{MTLCommandQueue, MTLDevice, MTLLanguageVersion, MTLLibrary, MTLSharedEvent};
use parking_lot::Mutex;
use wgpu::custom::*;

use super::encode::{Inert, MetalEncoder, Zeros, zero_shared_texture, zero_texture};
use super::layout::{BindGroup, BindGroupLayout, PipelineLayout};
use super::resource::{Live, MetalBuffer, MetalSampler, MetalTexture};
use super::shader::ShaderModule;
use super::submit::{Callback, MetalQueue, State};
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
    /// Bind groups, textures, and render pipelines alive, likewise.
    bind_groups: Arc<AtomicIsize>,
    textures: Arc<AtomicIsize>,
    pub(super) render_pipelines: Arc<AtomicIsize>,
    /// Encoders that made a command buffer not yet submitted or dropped. While none has, every
    /// recorded command that reads a buffer's storage was submitted.
    pub(super) recording: AtomicUsize,
    /// What new textures are zeroed from, made with the first.
    pub(super) zeros: OnceLock<Zeros>,
    /// Rune's precompiled pipelines, loaded with the first pipeline that looks one up.
    pub(super) builtin: OnceLock<Option<Obj<dyn MTLLibrary>>>,
    /// Whether the GPU stopped running this queue's work, and who hears it.
    pub(super) lost: Mutex<Lost>,
    /// Held while one thread runs the callbacks retired work owes, so they run in order: a
    /// loss's before the work-done callbacks of what failed (`submit::Shared::retire`).
    pub(super) dispatch: Mutex<()>,
    /// Active `Device::push_error_scope` stack for capturing shader and pipeline errors.
    pub(super) scopes: Mutex<ErrorScopes>,
}
send_sync!(Shared);

/// Active wgpu error scopes pushed on this device, innermost at the back, scoped per thread
/// (matching `wgpu-core`'s thread-local error scope stacks so background pipeline compilation
/// cannot collide with UI-thread scopes).
#[derive(Default)]
pub(super) struct ErrorScopes {
    next: u32,
    stack: Vec<(u32, std::thread::ThreadId, wgpu::ErrorFilter, Option<wgpu::Error>)>,
}

impl std::fmt::Debug for ErrorScopes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ErrorScopes").field("depth", &self.stack.len()).finish()
    }
}

/// The device's loss (`Device::set_device_lost_callback`), reported either through a failed
/// command buffer (`submit::failed`) or an explicit `Device::destroy`.
pub(super) struct Lost {
    pub(super) reason: wgpu::DeviceLostReason,
    pub(super) message: Option<String>,
    pub(super) callback: Option<BoxDeviceLostCallback>,
}

impl Default for Lost {
    fn default() -> Self {
        Self { reason: wgpu::DeviceLostReason::Unknown, message: None, callback: None }
    }
}

impl std::fmt::Debug for Lost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lost").field("reason", &self.reason).field("message", &self.message).finish_non_exhaustive()
    }
}

impl Shared {
    /// Records an error of `filter` into the current thread's innermost matching error scope,
    /// returning `true` when a scope captured it and `false` when no matching scope is active.
    pub(super) fn capture_error(&self, filter: wgpu::ErrorFilter, message: String) -> bool {
        let tid = std::thread::current().id();
        let mut scopes = self.scopes.lock();
        let Some((_, _, _, slot)) = scopes.stack.iter_mut().rev().find(|(_, thread, kind, _)| *thread == tid && *kind == filter) else {
            return false;
        };
        if slot.is_none() {
            let source = Box::new(std::io::Error::other(message.clone()));
            *slot = Some(match filter {
                wgpu::ErrorFilter::Validation => wgpu::Error::Validation { source, description: message },
                wgpu::ErrorFilter::OutOfMemory => wgpu::Error::OutOfMemory { source },
                wgpu::ErrorFilter::Internal => wgpu::Error::Internal { source, description: message },
            });
        }
        true
    }

    /// Marks the device lost once with `(reason, message)`, enqueues its device-lost callback
    /// ahead of pending work-done and map callbacks, and dispatches them in order.
    pub(super) fn mark_lost(&self, reason: wgpu::DeviceLostReason, message: String) {
        {
            let mut st = self.state.lock();
            let mut lost = self.lost.lock();
            if lost.message.is_some() { return }
            lost.reason = reason;
            lost.message = Some(message.clone());
            if let Some(f) = lost.callback.take() {
                st.callbacks.push_front((0, Callback::Lost(f, reason, message)));
            }
        }
        self.dispatch();
    }

    /// The device is lost, and its callback hears it once (a command buffer's failure does it
    /// under the state lock: `submit::Shared::pop_completed`).
    #[cfg(test)]
    pub(super) fn lose(&self, message: String) {
        self.mark_lost(wgpu::DeviceLostReason::Unknown, message);
    }
}

#[derive(Debug)]
pub(crate) struct MetalDevice {
    pub(crate) shared: Arc<Shared>,
    features: wgpu::Features,
    limits: wgpu::Limits,
}

pub(super) fn new(device: Obj<dyn MTLDevice>, desc: &wgpu::DeviceDescriptor<'_>) -> (MetalDevice, MetalQueue) {
    let queue = device.newCommandQueue().expect("a Metal command queue");
    let event = device.newSharedEvent().expect("a Metal shared event");
    #[cfg(debug_assertions)]
    {
        queue.setLabel(Some(&NSString::from_str("rune metal queue")));
        event.setLabel(Some(&NSString::from_str("rune metal event")));
    }
    let shared = Arc::new(Shared {
        queue,
        event,
        msl: msl_version(),
        state: Mutex::new(State::default()),
        buffers: Arc::default(),
        bind_groups: Arc::default(),
        textures: Arc::default(),
        render_pipelines: Arc::default(),
        recording: AtomicUsize::new(0),
        zeros: OnceLock::new(),
        builtin: OnceLock::new(),
        lost: Mutex::new(Lost::default()),
        dispatch: Mutex::new(()),
        scopes: Mutex::new(ErrorScopes::default()),
        device,
    });
    let device = MetalDevice { shared: shared.clone(), features: desc.required_features, limits: desc.required_limits.clone() };
    (device, MetalQueue(shared))
}

/// wgpu-hal's choice of MSL version for this macOS or iOS. Their numbers meet at 26; before, iOS
/// 18 and 17 match macOS 15 and 14.
pub(super) fn msl_version() -> ((u8, u8), MTLLanguageVersion) {
    let mut buf = [0u8; 32];
    let mut len = buf.len();
    // SAFETY: sysctlbyname writes at most `len` bytes into `buf` and the length it wrote into `len`.
    let ok = unsafe { libc::sysctlbyname(c"kern.osproductversion".as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) } == 0;
    let version = if ok { String::from_utf8_lossy(&buf[..len.saturating_sub(1)]).into_owned() } else { String::new() };
    let major: u32 = version.split('.').next().and_then(|m| m.trim().parse().ok()).unwrap_or(0);
    #[cfg(not(target_os = "macos"))]
    let major = match major { 18 => 15, 17 => 14, 26.. => major, _ => 0 };
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
        DispatchShaderModule::custom(ShaderModule::new(&self.shared, desc, checks))
    }
    unsafe fn create_shader_module_passthrough(&self, desc: &wgpu::ShaderModuleDescriptorPassthrough<'_>) -> DispatchShaderModule {
        DispatchShaderModule::custom(ShaderModule::passthrough(&self.shared, desc))
    }
    fn create_bind_group_layout(&self, desc: &wgpu::BindGroupLayoutDescriptor<'_>) -> DispatchBindGroupLayout {
        DispatchBindGroupLayout::custom(BindGroupLayout::new(desc))
    }
    fn create_bind_group(&self, desc: &wgpu::BindGroupDescriptor<'_>) -> DispatchBindGroup { DispatchBindGroup::custom(BindGroup::new(desc, Live::new(&self.shared.bind_groups))) }
    fn create_pipeline_layout(&self, desc: &wgpu::PipelineLayoutDescriptor<'_>) -> DispatchPipelineLayout {
        DispatchPipelineLayout::custom(PipelineLayout::new(desc))
    }
    fn create_render_pipeline(&self, desc: &wgpu::RenderPipelineDescriptor<'_>) -> DispatchRenderPipeline {
        DispatchRenderPipeline::custom(super::pipeline::create(&self.shared, desc))
    }
    fn create_mesh_pipeline(&self, _: &wgpu::MeshPipelineDescriptor<'_>) -> DispatchRenderPipeline {
        super::unsupported!("mesh shaders")
    }
    fn create_compute_pipeline(&self, desc: &wgpu::ComputePipelineDescriptor<'_>) -> DispatchComputePipeline {
        DispatchComputePipeline::custom(super::compute::create(&self.shared, desc))
    }
    unsafe fn create_pipeline_cache(&self, _: &wgpu::PipelineCacheDescriptor<'_>) -> DispatchPipelineCache { DispatchPipelineCache::custom(Inert) }
    fn create_buffer(&self, desc: &wgpu::BufferDescriptor<'_>) -> DispatchBuffer {
        DispatchBuffer::custom(MetalBuffer::new(&self.shared, desc.size, desc.label))
    }
    fn create_texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> DispatchTexture {
        let texture = MetalTexture::new(&self.shared, desc, Live::new(&self.shared.textures));
        let raw = texture.raw.clone();
        if raw.is_shared() {
            zero_shared_texture(&raw);
        } else if raw.is_lazy() {
            // Single-aspect 2D Private RENDER_ATTACHMENT textures clear lazily via render passes.
        } else if raw.format.is_combined_depth_stencil_format()
            && desc.usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
        {
            let subs = (0..raw.layers).flat_map(|layer| {
                (0..raw.mips).map(move |mip| crate::lazy_init::Subresource { mip, layer })
            });
            self.shared.clear_subresources(&raw, subs);
        } else if raw.sample_count == 1 {
            let zeros = self.shared.zeros.get_or_init(|| Zeros::new(&self.shared));
            self.shared.upload(|blit| zero_texture(blit, zeros, &raw), [raw.clone() as _], None);
        }
        DispatchTexture::custom(texture)
    }
    fn create_external_texture(&self, _: &wgpu::ExternalTextureDescriptor<'_>, _: &[&wgpu::TextureView]) -> DispatchExternalTexture {
        super::unsupported!("external textures")
    }
    fn create_blas(&self, _: &wgpu::CreateBlasDescriptor<'_>, _: wgpu::BlasGeometrySizeDescriptors) -> (Option<u64>, DispatchBlas) {
        super::unsupported!("ray tracing")
    }
    fn create_tlas(&self, _: &wgpu::CreateTlasDescriptor<'_>) -> DispatchTlas { super::unsupported!("ray tracing") }
    fn create_sampler(&self, desc: &wgpu::SamplerDescriptor<'_>) -> DispatchSampler {
        DispatchSampler::custom(MetalSampler::new(&self.shared.device, desc))
    }
    fn create_query_set(&self, desc: &wgpu::QuerySetDescriptor<'_>) -> DispatchQuerySet {
        DispatchQuerySet::custom(super::query::QuerySet::new(&self.shared.device, desc))
    }
    fn create_command_encoder(&self, desc: &wgpu::CommandEncoderDescriptor<'_>) -> DispatchCommandEncoder {
        DispatchCommandEncoder::custom(MetalEncoder::new(self.shared.clone(), desc.label))
    }
    fn create_render_bundle_encoder(&self, _: &wgpu::RenderBundleEncoderDescriptor<'_>) -> DispatchRenderBundleEncoder {
        super::unsupported!("render bundles")
    }
    fn set_device_lost_callback(&self, callback: BoxDeviceLostCallback) {
        let mut lost = self.shared.lost.lock();
        match lost.message.clone() {
            Some(message) => {
                let reason = lost.reason;
                drop(lost);
                callback(reason, message);
            }
            None => lost.callback = Some(callback),
        }
    }
    fn on_uncaptured_error(&self, _: Arc<dyn wgpu::UncapturedErrorHandler>) {}
    fn push_error_scope(&self, filter: wgpu::ErrorFilter) -> u32 {
        let mut scopes = self.shared.scopes.lock();
        scopes.next = scopes.next.wrapping_add(1);
        let id = scopes.next;
        scopes.stack.push((id, std::thread::current().id(), filter, None));
        id
    }
    fn pop_error_scope(&self, index: u32) -> Pin<Box<dyn PopErrorScopeFuture>> {
        let error = {
            let mut scopes = self.shared.scopes.lock();
            scopes.stack.iter().rposition(|(id, _, _, _)| *id == index).and_then(|pos| scopes.stack.remove(pos).3)
        };
        Box::pin(ready(error))
    }
    unsafe fn start_graphics_debugger_capture(&self) {}
    unsafe fn stop_graphics_debugger_capture(&self) {}
    fn poll(&self, poll: wgpu::wgt::PollType<u64>) -> Result<wgpu::PollStatus, wgpu::PollError> { self.shared.poll(poll) }
    fn get_internal_counters(&self) -> wgpu::InternalCounters {
        let counters = wgpu::InternalCounters::default();
        counters.hal.buffers.set(self.shared.buffers.load(Ordering::Relaxed));
        counters.hal.bind_groups.set(self.shared.bind_groups.load(Ordering::Relaxed));
        counters.hal.textures.set(self.shared.textures.load(Ordering::Relaxed));
        counters.hal.render_pipelines.set(self.shared.render_pipelines.load(Ordering::Relaxed));
        counters
    }
    fn generate_allocator_report(&self) -> Option<wgpu::AllocatorReport> { None }
    fn destroy(&self) {
        self.shared.mark_lost(wgpu::DeviceLostReason::Destroyed, "Device was destroyed.".to_owned());
    }
}
