//! GPU instance, adapter and device creation, prewarming, and allocation reporting.

use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(all(test, target_os = "macos"))]
use super::{Renderer, gpu_events};
use super::prewarm;

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Shared device-lost and uncaptured-error callback state (`gpu_loss`).
    pub(super) callbacks: super::gpu_loss::GpuCallbacks,
    /// D3D12 removes a device before wgpu hears of it (`Gpu::is_lost`).
    pub(super) removal: rune_gpu::health::Removal,
}

/// Latched process-wide once a Linux direct-driver Vulkan instance fails to serve a device or
/// surface and Rune widens to an unlimited instance with all installed drivers.
static DIRECT_VULKAN_BYPASSED: AtomicBool = AtomicBool::new(false);

/// The most views and buffers a device binds at once (`Limits::max_non_sampler_bindings`).
/// D3D12 sizes its shader-visible heap from it and can't grow it: each layer target holds about
/// five of these, so 4,096 ran out at 1,024 layers in one frame.
#[cfg(not(target_arch = "wasm32"))]
const MAX_VIEWS: u32 = 65_536;

/// Vulkan loaders crash when two threads create instances or devices at once (seen with
/// parallel GPU tests on lavapipe), so Rune creates them one at a time.
static GPU_INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl Gpu {
    /// Panics without a usable adapter or device; windows use `try_new`.
    pub fn new(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Gpu {
        Self::try_new(instance, surface).unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn try_new(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Gpu, String> {
        Self::try_with_features(instance, surface, wgpu::Features::empty())
    }

    /// Like `try_new`, also enabling whichever of `wanted` the adapter supports.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn try_with_features(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, wanted: wgpu::Features) -> Result<Gpu, String> {
        Self::try_for(instance, surface, wanted, false)
    }

    /// Like `try_with_features`, preferring for a `translucent` surface `translucency::prefer`'s adapter.
    #[cfg(not(target_arch = "wasm32"))]
    fn try_for(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, wanted: wgpu::Features, translucent: bool) -> Result<Gpu, String> {
        Self::try_picked(instance, surface, wanted, translucent, false)
    }

    /// A device for a surface not made yet (`prewarm_gpu`, `warm_gpu`, `embed::LayerHost`):
    /// on the adapter a surface would get, with the surface uncaptured-error policy installed.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn try_for_display(instance: wgpu::Instance, wanted: wgpu::Features) -> Result<Gpu, String> {
        Self::try_picked(instance, None, wanted, false, true)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn try_picked(mut instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, wanted: wgpu::Features, translucent: bool, presented: bool) -> Result<Gpu, String> {
        use crate::gpu::fallback::{self, Candidate, Effect, Event, Failure, Policy, State};
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        // D3D12 keeps one device per adapter in a process: while a removed one is still held, a
        // new device on that adapter is the removed one again.
        crate::native_surface::release_removed_device();
        let pick = crate::gpu::pick::pick(&instance, surface, presented)?;
        let policy = pick.policy();
        let compositor = pick.compositor().map(str::to_owned);
        crate::marks::mark("gpu.adapter");
        if !crate::marks::noted("adapter") { crate::marks::note("adapter", Box::leak(pick.adapter.get_info().name.into_boxed_str())); }
        let adapter = match surface {
            Some(s) if translucent && !pick.from_env() => crate::render::translucency::prefer(&instance, s, &pick),
            _ => pick.adapter,
        };
        let draws_surfaces = surface.is_some() || presented;
        let first_info = adapter.get_info();
        let mut tried = vec![Self::candidate_info(0, &adapter, surface)];
        let mut state = State::first(policy);
        let mut event = match Self::device_on(&adapter, wanted) {
            Ok((device, queue)) => {
                crate::marks::mark("gpu.device");
                crate::native_surface::note_device(&device, draws_surfaces);
                return Ok(Gpu::made(instance, adapter, device, queue, draws_surfaces));
            }
            Err(error) => Event::Failed { kind: Failure::DeviceCreate, error },
        };
        let mut adapters: Vec<wgpu::Adapter> = Vec::new();
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut widened = false;
        loop {
            let mut next_event = None;
            for effect in fallback::step(&mut state, event) {
                match effect {
                    Effect::EnumerateFallbacks => {
                        adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
                        candidates = adapters
                            .iter()
                            .enumerate()
                            .map(|(i, other)| Self::candidate_info(i, other, surface))
                            .collect();
                        let ordered = fallback::order_fallbacks(
                            &tried,
                            &candidates,
                            &crate::gpu::pick::display_devices(),
                            compositor.as_deref(),
                            translucent,
                        );
                        next_event = Some(Event::Candidates(ordered));
                    }
                    Effect::TryCandidate(index) => {
                        let other = &adapters[index];
                        match Self::device_on(other, wanted) {
                            Ok((device, queue)) => {
                                let info = other.get_info();
                                let first_error = state.first_error().unwrap_or("");
                                eprintln!(
                                    "rune: {} ({:?}) made no device ({first_error}); falling back to {} ({:?}, {:?})",
                                    first_info.name, first_info.backend, info.name, info.backend, info.device_type
                                );
                                crate::native_surface::note_device(&device, draws_surfaces);
                                return Ok(Gpu::made(instance, other.clone(), device, queue, draws_surfaces));
                            }
                            Err(error) => {
                                tried.push(candidates[index].clone());
                                next_event = Some(Event::Failed { kind: Failure::DeviceCreate, error });
                            }
                        }
                    }
                    Effect::FinishOk { .. } => unreachable!("unexpected fallback effect in try_picked"),
                    Effect::FinishErr(error) => {
                        if surface.is_none()
                            && !widened
                            && policy == Policy::Automatic
                            && Self::is_direct_driver_instance(&instance)
                        {
                            widened = true;
                            Self::mark_direct_vulkan_bypassed();
                            eprintln!(
                                "rune: direct Vulkan driver made no device ({error}); retrying with all installed Vulkan drivers"
                            );
                            // Already holding `GPU_INIT`; build/fetch the unlimited instance directly.
                            instance = Self::wgpu_instance_on(wgpu::Backends::all(), true);
                            adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
                            candidates = adapters
                                .iter()
                                .enumerate()
                                .map(|(i, other)| Self::candidate_info(i, other, surface))
                                .collect();
                            let ordered = fallback::order_fallbacks(
                                &tried,
                                &candidates,
                                &crate::gpu::pick::display_devices(),
                                compositor.as_deref(),
                                translucent,
                            );
                            state = State::need_fallbacks(error);
                            next_event = Some(Event::Candidates(ordered));
                            continue;
                        }
                        return Err(error);
                    }
                }
            }
            event = next_event.expect("fallback state machine must emit an effect");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn candidate_info(index: usize, adapter: &wgpu::Adapter, surface: Option<&wgpu::Surface<'_>>) -> crate::gpu::fallback::Candidate {
        let info = adapter.get_info();
        let supported = surface.is_none_or(|s| adapter.is_surface_supported(s));
        let translucent = surface.is_none_or(|s| crate::render::translucency::mode(s, adapter).is_some());
        crate::gpu::fallback::Candidate::from_info(index, &info, supported, translucent)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn env_fallback_policy() -> crate::gpu::fallback::Policy {
        crate::gpu::pick::env_policy()
    }

    /// Active DX12 swap-chain presentation system on Windows: DirectComposition (`DxgiFromVisual`)
    /// by default, overridden explicitly by `WGPU_DX12_PRESENTATION_SYSTEM` (`visual` or `hwnd`).
    pub(crate) fn dx12_swapchain_kind() -> wgpu::Dx12SwapchainKind {
        wgpu::Dx12SwapchainKind::DxgiFromVisual.with_env()
    }

    /// Whether this process has fallen back from direct Linux Vulkan driver loading to an
    /// unlimited instance with all installed drivers.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn direct_vulkan_bypassed() -> bool {
        DIRECT_VULKAN_BYPASSED.load(Ordering::Acquire)
    }

    /// Latches that subsequent Linux Vulkan instances should load all installed drivers instead
    /// of restricting to the compositor's driver via `VK_LUNARG_direct_driver_loading`.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn mark_direct_vulkan_bypassed() {
        DIRECT_VULKAN_BYPASSED.store(true, Ordering::Release);
    }

    /// Whether `instance` was created with `VK_LUNARG_direct_driver_loading` (single-driver
    /// exclusive mode on Linux).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn is_direct_driver_instance(instance: &wgpu::Instance) -> bool {
        #[cfg(target_os = "linux")]
        {
            super::vulkan_icd::is_direct_driver_instance(instance)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = instance;
            false
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn device_on(adapter: &wgpu::Adapter, wanted: wgpu::Features) -> Result<(wgpu::Device, wgpu::Queue), String> {
        // Dual-source blending is for subpixel text (`FontSmoothing::WindfoilRgb`, `ClearType`).
        let optional_features = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::DUAL_SOURCE_BLENDING | wanted | crate::gpu::backend::free_features(adapter);
        // Precompiled uber shaders: a metallib on macOS, DXBC on Windows.
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let optional_features = optional_features | wgpu::Features::PASSTHROUGH_SHADERS;
        let required_features = adapter.features() & optional_features;
        // A compositor that asks for immediates (`Compositor::features`) gets all the adapter has.
        let max_immediate_size = if required_features.contains(wgpu::Features::IMMEDIATES) { adapter.limits().max_immediate_size } else { 0 };
        let adapter_limits = adapter.limits();
        // Views and buffers bound at once (D3D12's shader-visible descriptor heap, which wgpu sizes from
        // this limit when it makes the device). wgpu's default, a million, is a 32-64 MB heap on
        // Windows; 65,536 is far more than Rune binds and keeps it to a few MB (a bare wgpu window:
        // 82.0 MB private bytes with a million, 24.8 MB with 65,536; a bare D3D12 window, 14.1 MB).
        let views = MAX_VIEWS.min(adapter_limits.max_non_sampler_bindings);
        let required_limits = if adapter_limits.max_inter_stage_shader_variables < wgpu::Limits::default().max_inter_stage_shader_variables
            || adapter_limits.max_color_attachments < wgpu::Limits::default().max_color_attachments
            || adapter_limits.max_compute_workgroup_storage_size < wgpu::Limits::default().max_compute_workgroup_storage_size
        {
            wgpu::Limits {
                max_immediate_size,
                max_non_sampler_bindings: views,
                ..wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter_limits.clone())
                    .using_alignment(adapter_limits)
            }
        } else {
            wgpu::Limits {
                max_immediate_size,
                max_non_sampler_bindings: views,
                ..wgpu::Limits::default()
                    .using_resolution(adapter_limits.clone())
                    .using_alignment(adapter_limits)
            }
        };
        let made = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rune"),
            required_features,
            required_limits,
            experimental_features: Default::default(),
            // Defaults to `MemoryUsage` (`Performance` suballocates from 128 MB device and
            // 64 MB host blocks, committing ~190 MB more than a few atlases use). Callers can
            // override this at startup via `App::gpu_memory_policy` or `set_gpu_memory_policy`.
            memory_hints: crate::gpu::memory::wgpu_hints(),
            trace: Default::default(),
        }))
        .map_err(|error| format!("no GPU device: {error}"));
        #[cfg(feature = "alloc-sites")]
        if let Ok((device, _)) = &made {
            report_gpu_allocations(device.clone());
        }
        made
    }

    #[cfg(target_arch = "wasm32")]
    pub fn try_with_features(_: wgpu::Instance, _: Option<&wgpu::Surface<'_>>, _: wgpu::Features) -> Result<Gpu, String> {
        Err("synchronous GPU initialization is unsupported on web; use App::run".into())
    }


    pub fn headless() -> Gpu {
        Self::new(Self::instance(), None)
    }

    /// A headless GPU on wgpu's backends, for tests of what needs wgpu-hal.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn headless_wgpu() -> Gpu {
        Self::new(Self::wgpu_instance(), None)
    }

    /// A headless GPU for tests. Returns `Some(gpu)` whenever a real adapter is available; when
    /// no adapter exists and `RUNE_ALLOW_NO_GPU=1` is set, prints one skip line and returns
    /// `None`. Without `RUNE_ALLOW_NO_GPU=1`, or on any other GPU error, panics like `headless`.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn test_headless() -> Option<Gpu> {
        Self::test_on(Self::instance())
    }

    /// Like `test_headless`, using `instance`.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn test_on(instance: wgpu::Instance) -> Option<Gpu> {
        match Self::try_new(instance, None) {
            Ok(gpu) => Some(gpu),
            Err(error) => {
                crate::gpu::skip_test_without_gpu(&error);
                None
            }
        }
    }

    /// Checks whether a real GPU adapter is available for tests that don't construct a `Gpu`
    /// directly (headless runs, `try_setup_on` fallback tests). Returns `true` when an adapter
    /// exists; prints one skip line and returns `false` when none exists and `RUNE_ALLOW_NO_GPU=1`
    /// is set; panics otherwise.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn test_adapter_available() -> bool {
        static PROBE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
        let res = PROBE.get_or_init(|| {
            let instance = Self::instance();
            let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
            crate::gpu::pick::pick(&instance, None, false).map(|_| ())
        });
        match res {
            Ok(()) => true,
            Err(error) => {
                crate::gpu::skip_test_without_gpu(error);
                false
            }
        }
    }

    /// Like `test_headless`, on wgpu's own backends (`Gpu::wgpu_instance`).
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn test_headless_wgpu() -> Option<Gpu> {
        Self::test_on(Self::wgpu_instance())
    }

    /// Like `test_headless`, requesting optional `wanted` features.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn test_with_features(wanted: wgpu::Features) -> Option<Gpu> {
        match Self::try_with_features(Self::instance(), None, wanted) {
            Ok(gpu) => Some(gpu),
            Err(error) => {
                crate::gpu::skip_test_without_gpu(&error);
                None
            }
        }
    }

    /// A GPU on wgpu's noop backend, for non-pixel tests when `test-gpu-noop` is enabled.
    #[cfg(all(test, feature = "test-gpu-noop"))]
    pub(crate) fn test_noop() -> Gpu {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::NOOP;
        descriptor.backend_options.noop.enable = true;
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        Gpu::from_device(instance, adapter, device, queue)
    }

    /// A real GPU when available, or wgpu's noop backend on a GPU-less host when both
    /// `RUNE_ALLOW_NO_GPU=1` and `test-gpu-noop` are active. Use only for CPU-side renderer,
    /// planner, or pool bookkeeping tests whose assertions never depend on backend validation,
    /// shader compilation, submission timing, or rendered pixels; use `test_headless` or
    /// `test_headless_wgpu` otherwise. Skips when `RUNE_ALLOW_NO_GPU=1` is set without
    /// `test-gpu-noop`; panics without `RUNE_ALLOW_NO_GPU=1`.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn test_headless_or_noop() -> Option<Gpu> {
        match Self::try_new(Self::instance(), None) {
            Ok(gpu) => Some(gpu),
            Err(error) => {
                #[cfg(feature = "test-gpu-noop")]
                if crate::gpu::allow_no_gpu_tests() && crate::gpu::is_missing_adapter_error(&error) {
                    return Some(Self::test_noop());
                }
                crate::gpu::skip_test_without_gpu(&error);
                None
            }
        }
    }

    /// Without `VALIDATION_INDIRECT_CALL`: Rune issues no indirect draws or dispatches, and the
    /// validation pipeline wgpu would build for them costs every device a shader compile (about
    /// 40 ms on a Mac's first launch after an install or update). Indirect draws a compositor
    /// records on Rune's device still work; wgpu just doesn't check their argument buffers.
    pub fn instance() -> wgpu::Instance {
        #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
        if crate::gpu::metal::enabled() { return crate::gpu::metal::instance(); }
        Self::wgpu_instance()
    }

    /// `instance`, or wgpu's when the native Metal backend is only the default and lacks one of
    /// the features a compositor wants (`Compositor::features`).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn instance_for(wanted: wgpu::Features) -> wgpu::Instance {
        if crate::gpu::backend::wgpu_for(wanted) { Self::wgpu_instance() } else { Self::instance() }
    }

    /// wgpu's own backends, whatever `RUNE_GPU_BACKEND` says: DX12 alone on Windows. It's the
    /// backend that composites transparent surfaces, and a Vulkan instance loads every GPU's Vulkan
    /// driver, where a broken one (Intel's, on a hybrid laptop) crashed the process before its first
    /// window (Hex #662). A machine without a hardware D3D12 adapter gets WARP, D3D12's software
    /// adapter, from the same instance.
    #[cfg(target_os = "windows")]
    pub(crate) fn wgpu_instance() -> wgpu::Instance {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        let instance = Self::wgpu_instance_on(wgpu::Backends::DX12, false);
        crate::marks::mark("gpu.instance");
        instance
    }

    /// wgpu's own backends, whatever `RUNE_GPU_BACKEND` says.
    #[cfg(not(target_os = "windows"))]
    pub(crate) fn wgpu_instance() -> wgpu::Instance {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        let unlimited = Self::direct_vulkan_bypassed();
        Self::wgpu_instance_on(wgpu::Backends::all(), unlimited)
    }

    /// An unlimited wgpu instance with every installed Vulkan driver loaded (used when a Linux
    /// direct-driver instance cannot serve a device or surface and the fallback chain needs to
    /// walk remaining drivers). Identical to `wgpu_instance` on non-Linux platforms.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn wgpu_instance_unlimited() -> wgpu::Instance {
        #[cfg(target_os = "linux")]
        {
            let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
            Self::wgpu_instance_on(wgpu::Backends::all(), true)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::wgpu_instance()
        }
    }

    /// One instance per (backends, direct-vs-unlimited scope) for the whole process: a new one per
    /// device was a new DXGI factory and adapter list each time a window or overlay made a device
    /// (hex#1268), while `unlimited_vulkan` keeps a Linux direct-driver instance separate from the
    /// all-drivers fallback instance.
    fn wgpu_instance_on(backends: wgpu::Backends, unlimited_vulkan: bool) -> wgpu::Instance {
        #[cfg(not(target_arch = "wasm32"))]
        {
            static MADE: std::sync::Mutex<Vec<(wgpu::Backends, bool, wgpu::Instance)>> = std::sync::Mutex::new(Vec::new());
            let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, _, instance)) = made.iter().find(|(b, is_direct, _)| {
                *b == backends && (!unlimited_vulkan || !*is_direct)
            }) {
                return instance.clone();
            }
            let (instance, is_direct) = Self::new_wgpu_instance(backends, unlimited_vulkan);
            made.push((backends, is_direct, instance.clone()));
            instance
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = unlimited_vulkan;
            // With `wgpu/webgl`, wgpu's wasm handles are single-threaded (`!Send + !Sync`), so
            // keep the per-backend instance cache in thread-local storage.
            thread_local! {
                static MADE: std::cell::RefCell<Vec<(wgpu::Backends, wgpu::Instance)>> = const { std::cell::RefCell::new(Vec::new()) };
            }
            MADE.with(|made| {
                let mut made = made.borrow_mut();
                if let Some((_, instance)) = made.iter().find(|(b, _)| *b == backends) {
                    return instance.clone();
                }
                let (instance, _) = Self::new_wgpu_instance(backends, false);
                made.push((backends, instance.clone()));
                instance
            })
        }
    }

    fn new_wgpu_instance(backends: wgpu::Backends, unlimited_vulkan: bool) -> (wgpu::Instance, bool) {
        #[cfg(target_os = "linux")]
        {
            super::vulkan_icd::create_linux_instance(
                backends,
                unlimited_vulkan || Self::direct_vulkan_bypassed(),
                |allow_noncompliant| Self::instance_descriptor(backends, allow_noncompliant),
                || {
                    Self::select_wgpu_instance(
                        |allow_noncompliant| {
                            wgpu::Instance::new(Self::instance_descriptor(backends, allow_noncompliant))
                        },
                        |instance| !pollster::block_on(instance.enumerate_adapters(backends)).is_empty(),
                    )
                },
            )
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = unlimited_vulkan;
            (wgpu::Instance::new(Self::instance_descriptor(backends, false)), false)
        }
    }

    /// Builds a `wgpu::InstanceDescriptor` for `backends`. When `allow_noncompliant` is true,
    /// `ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER` is set so Vulkan drivers reporting conformance
    /// version `0.0.0.0` (such as Mesa Honeykrisp/Aurora on Apple silicon Linux) are enumerated.
    fn instance_descriptor(
        backends: wgpu::Backends,
        allow_noncompliant: bool,
    ) -> wgpu::InstanceDescriptor {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = backends;
        descriptor.flags.remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
        if allow_noncompliant {
            descriptor.flags.insert(wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER);
        }
        // An HWND's DXGI swapchain is opaque only; DirectComposition composites alpha on every GPU.
        // Honour `WGPU_DX12_PRESENTATION_SYSTEM` (`hwnd` enables RenderDoc and Wine/Proton).
        descriptor.backend_options.dx12.presentation_system = Self::dx12_swapchain_kind();
        // A window waits for room in its swap chain itself, in the loop rather than in
        // `acquire` (`window::slot`).
        descriptor.backend_options.dx12.latency_waitable_object = wgpu::Dx12UseFrameLatencyWaitableObject::DontWait;
        // FXC ships with Windows (d3dcompiler_47.dll in System32). wgpu's default looks for
        // dxcompiler.dll on PATH, where another app's incompatible copy crashes the first
        // shader compile (Hex #587).
        descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
        descriptor
    }

    /// Builds a conformant-only instance first and returns it whenever it sees at least one
    /// adapter, so machines with a conformant adapter never expose non-conformant drivers. Only
    /// when the conformant instance finds no adapters at all does it retry with
    /// `ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER` (Apple silicon Linux Asahi/Aurora Honeykrisp).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn select_wgpu_instance<I>(
        mut build: impl FnMut(bool) -> I,
        mut has_adapters: impl FnMut(&I) -> bool,
    ) -> I {
        let compliant = build(false);
        if has_adapters(&compliant) {
            return compliant;
        }
        let noncompliant = build(true);
        if has_adapters(&noncompliant) {
            return noncompliant;
        }
        compliant
    }

    /// The GPU for a window's first surface, which `surface` makes from the instance:
    /// `prewarm_gpu`'s device, with the renderer it made, when its adapter presents to that
    /// surface and has `wanted`'s features; otherwise a new device on the same instance, so the
    /// window never gets a second surface.
    /// A `translucent` surface gets an adapter that composites translucent pixels when any can.
    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn for_surface<'w>(wanted: wgpu::Features, translucent: bool,
        surface: impl FnOnce(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>) -> Result<(Gpu, wgpu::Surface<'w>, Option<Renderer>), String> {
        Self::for_surface_from(prewarm::take(), wanted, translucent, surface)
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(super) fn for_surface_from<'w>(early: Option<(Gpu, Renderer)>, wanted: wgpu::Features, translucent: bool,
        surface: impl FnOnce(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>) -> Result<(Gpu, wgpu::Surface<'w>, Option<Renderer>), String> {
        let made = Self::surface_gpu_from(early, wanted, translucent, surface)?;
        if std::env::var_os("RUNE_GPU_LOG").is_some_and(|v| v != "0") {
            eprintln!("rune: a surface draws with {}", crate::gpu::backend::describe(&made.0.device));
        }
        Ok(made)
    }

    #[cfg(all(test, target_os = "macos"))]
    fn surface_gpu_from<'w>(early: Option<(Gpu, Renderer)>, wanted: wgpu::Features, translucent: bool,
        surface: impl FnOnce(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>) -> Result<(Gpu, wgpu::Surface<'w>, Option<Renderer>), String> {
        // A prewarmed native Metal GPU lacking a wanted feature, or one that died since it was
        // made, is dropped before it makes a surface.
        let Some((early, renderer)) = early.filter(|(gpu, _)| !gpu.is_lost() && crate::gpu::backend::serves(&gpu.device, wanted)) else {
            let instance = Self::instance_for(wanted);
            let surface = surface(&instance)?;
            gpu_events::starting();
            return Ok((Self::try_for(instance, Some(&surface), wanted, translucent)?, surface, None));
        };
        let surface = surface(&early.instance)?;
        if crate::render::translucency::serves(&early, &surface, wanted, translucent) {
            early.install_surface_error_policy();
            return Ok((early, surface, Some(renderer)));
        }
        let instance = early.instance.clone();
        drop((renderer, early));
        gpu_events::starting();
        Ok((Self::try_for(instance, Some(&surface), wanted, translucent)?, surface, None))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn pick_first_adapter(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        policy_override: Option<crate::gpu::fallback::Policy>,
        translucent: bool,
    ) -> Result<(wgpu::Adapter, crate::gpu::fallback::Policy, Option<String>), super::SurfaceSetupError> {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        crate::native_surface::release_removed_device();
        let pick = crate::gpu::pick::pick(instance, surface, false)
            .map_err(|message| super::SurfaceSetupError { kind: crate::gpu::fallback::Failure::DeviceCreate, message })?;
        let policy = policy_override.unwrap_or_else(|| pick.policy());
        let compositor = pick.compositor().map(str::to_owned);
        crate::marks::mark("gpu.adapter");
        if !crate::marks::noted("adapter") {
            crate::marks::note("adapter", Box::leak(pick.adapter.get_info().name.into_boxed_str()));
        }
        let adapter = match surface {
            Some(s) if translucent && !pick.from_env() => crate::render::translucency::prefer(instance, s, &pick),
            _ => pick.adapter,
        };
        Ok((adapter, policy, compositor))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn pick_software_adapter(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<wgpu::Adapter, super::SurfaceSetupError> {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        crate::native_surface::release_removed_device();
        let pick = crate::gpu::pick::pick_software(instance, surface)
            .map_err(|message| super::SurfaceSetupError::unsupported(message))?;
        Ok(pick.adapter)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn create_device_serialized(adapter: &wgpu::Adapter, wanted: wgpu::Features) -> Result<(wgpu::Device, wgpu::Queue), String> {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        Self::device_on(adapter, wanted)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn enumerate_adapters_serialized(instance: &wgpu::Instance) -> Vec<wgpu::Adapter> {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
    }
}
/// Starts the GPU device and Rune's render pipeline on a thread, so they're ready when the first
/// window opens instead of being made after AppKit starts, which is most of a first launch after
/// an install or update (when Metal compiles the shaders). Call it first thing in `main`; Rune's
/// run functions call it too, but only once they start the event loop.
///
/// A window or overlay opened at launch picks the result up, waiting for the thread if it's still working;
/// if none opens, it's dropped. Calling again does nothing. Only on macOS, where any Metal adapter
/// presents to any window; elsewhere the window's surface decides the adapter, and this does
/// nothing.
///
/// It starts on the backend the app draws with when nothing chose one: Rune's native Metal
/// backend with the `metal-backend` feature (on by default), wgpu without it. An app that opts
/// out with `GpuBackend::Wgpu` should call `prewarm_gpu_on(GpuBackend::Wgpu)` instead, so the
/// thread starts on wgpu rather than on the native backend and starting over.
pub fn prewarm_gpu() {
    prewarm::start();
}

/// Chooses `backend` (as `App::gpu_backend` does), then starts `prewarm_gpu`'s thread on it, so an
/// app that picks a backend other than the default never prewarms on the default and restarts.
/// `App::gpu_backend` with the same backend afterwards changes nothing. On iOS it only chooses: call
/// it before the first `embed::LayerHost` (and `embed::prewarm`). Elsewhere, or without
/// `metal-backend`, it is `prewarm_gpu`.
pub fn prewarm_gpu_on(backend: crate::GpuBackend) {
    choose_then_start(backend, choose_gpu_backend, prewarm::start);
}

/// `choose` before `start`: a thread started first would run on the old backend.
pub(super) fn choose_then_start(backend: crate::GpuBackend, choose: impl FnOnce(crate::GpuBackend), start: impl FnOnce()) {
    choose(backend);
    start();
}

/// Makes GPUs from now on with `backend`, process-wide, and restarts a `prewarm_gpu` thread
/// running on the old backend.
pub(crate) fn choose_gpu_backend(backend: crate::GpuBackend) {
    let changed = crate::gpu::backend::choose(backend);
    #[cfg(all(target_os = "macos", feature = "metal-backend"))]
    if changed {
        prewarm::restart();
    }
    #[cfg(not(all(target_os = "macos", feature = "metal-backend")))]
    let _ = changed;
}

/// Sets the startup GPU suballocation policy for wgpu devices created from now on, process-wide.
///
/// Call this before [`prewarm_gpu`], [`crate::warm_gpu`], [`crate::warm_gpu_cache`], or
/// [`crate::App::run`] (or use [`crate::App::gpu_memory_policy`]). When the effective policy
/// changes while a prewarmed or parked GPU is already in flight or waiting, that cached device
/// is restarted (macOS) or discarded (Windows/Linux) so the next window or overlay creates its
/// device under the new policy.
///
/// Honored by wgpu's D3D12 (Windows) and Vulkan (Linux) suballocators; ignored by Rune's native
/// Metal backend, wgpu's Metal backend, and browser backends (WebGPU and WebGL2). Does not
/// affect devices that are already live. In [`crate::GpuMemoryPolicy::Manual`], `block_size`
/// bounds suballocator block sizes rather than resource sizes: larger textures and buffers
/// still succeed via dedicated allocations.
///
/// # Example
///
/// ```no_run
/// rune::set_gpu_memory_policy(rune::GpuMemoryPolicy::Manual {
///     block_size: (4 * 1024 * 1024)..(8 * 1024 * 1024),
/// });
/// ```
pub fn set_gpu_memory_policy(policy: crate::GpuMemoryPolicy) {
    apply_gpu_memory_policy(policy, crate::gpu::memory::choose, invalidate_prewarm_for_policy);
}

/// Updates the policy before invalidating any prewarmed or parked GPU so a restarted thread
/// always reads the new policy.
pub(super) fn apply_gpu_memory_policy(
    policy: crate::GpuMemoryPolicy,
    choose: impl FnOnce(crate::GpuMemoryPolicy) -> bool,
    invalidate: impl FnOnce(),
) {
    if choose(policy) {
        invalidate();
    }
}

fn invalidate_prewarm_for_policy() {
    #[cfg(target_os = "macos")]
    prewarm::restart();
    #[cfg(not(target_os = "macos"))]
    prewarm::discard();
}

/// Drops what `prewarm_gpu` made if no window took it.
pub(crate) fn discard_prewarm() {
    #[cfg(not(test))]
    prewarm::discard();
}

/// Non-blocking early cancellation called when the event loop starts exiting so in-flight GPU
/// warm-up skips follow-up shader/frame/done work while windows tear down.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn cancel_prewarm() {
    prewarm::cancel();
}

/// Cancels any in-flight GPU warm-up and waits up to `SHUTDOWN_TIMEOUT` for its worker
/// threads to exit before process teardown.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn shutdown_prewarm() -> super::warm_machine::ShutdownOutcome {
    prewarm::shutdown(super::warm_machine::SHUTDOWN_TIMEOUT)
}

/// Re-opens a cleanly drained warm-up slot when a new event loop starts in the same process.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn reopen_prewarm() {
    prewarm::reopen();
}

/// With `alloc-sites` (a measuring build), every heap dump also lists what the device holds by kind
/// (`textures <count> <bytes>`, `buffers <count> <bytes>`, views, bind groups, pipelines, command
/// encoders, shader modules, memory allocations) and its suballocator's allocations by label.
#[cfg(all(feature = "alloc-sites", not(target_arch = "wasm32")))]
fn report_gpu_allocations(device: wgpu::Device) {
    procwatch::alloc::sites::contribute("gpu", move || {
        // What the device holds, by kind (wgpu's counters: committed resources count here too).
        let c = device.get_internal_counters().hal;
        let mut out = format!(
            "textures {} {}\nbuffers {} {}\nviews {}\nbind_groups {}\npipelines {}\nencoders {}\nshader_modules {}\nallocations {}\n",
            c.textures.read(),
            c.texture_memory.read(),
            c.buffers.read(),
            c.buffer_memory.read(),
            c.texture_views.read(),
            c.bind_groups.read(),
            c.render_pipelines.read(),
            c.command_encoders.read(),
            c.shader_modules.read(),
            c.memory_allocations.read()
        );
        if let Some(r) = device.generate_allocator_report() {
            out.push_str(&format!("gpu {} {} {}\n", r.total_allocated_bytes, r.total_reserved_bytes, r.blocks.len()));
            for a in &r.allocations {
                out.push_str(&format!("alloc {} {}\n", a.size, a.name));
            }
        }
        out
    });
}

#[cfg(test)]
#[path = "device_tests.rs"]
mod tests;
