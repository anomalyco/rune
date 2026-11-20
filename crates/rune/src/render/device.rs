//! GPU instance, adapter and device creation, prewarming, and allocation reporting.

use std::sync::{Arc, atomic::AtomicBool};

use super::{Renderer, gpu_events, prewarm};

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Set by the device's lost callback: a device that went away (a driver reset, a GPU removed,
    /// Windows' Modern Standby) can't configure a surface, and wgpu drops that error, so nothing
    /// else would report it. See `is_lost`.
    pub(super) lost: Arc<AtomicBool>,
    /// D3D12 removes a device before wgpu hears of it (`Gpu::is_lost`).
    pub(super) removal: rune_gpu::health::Removal,
}

/// The most views and buffers a device binds at once (`Limits::max_non_sampler_bindings`).
/// D3D12 sizes its shader-visible heap from it and can't grow it: each layer target holds about
/// five of these, so 4,096 ran out at 1,024 layers in one frame.
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

    /// A device for a surface not made yet (`warm_gpu`): on the adapter a surface would get.
    #[cfg(not(any(target_arch = "wasm32", target_os = "macos")))]
    pub(super) fn try_for_display(instance: wgpu::Instance, wanted: wgpu::Features) -> Result<Gpu, String> {
        Self::try_picked(instance, None, wanted, false, true)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn try_picked(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, wanted: wgpu::Features, translucent: bool, presented: bool) -> Result<Gpu, String> {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        // D3D12 keeps one device per adapter in a process: while a removed one is still held, a
        // new device on that adapter is the removed one again.
        crate::native_surface::release_removed_device();
        let pick = crate::gpu::pick::pick(&instance, surface, presented)?;
        let named = pick.reason == crate::gpu::pick::Reason::EnvName;
        crate::marks::mark("gpu.adapter");
        if !crate::marks::noted("adapter") { crate::marks::note("adapter", Box::leak(pick.adapter.get_info().name.into_boxed_str())); }
        let adapter = match surface {
            Some(s) if translucent && !pick.from_env() => crate::render::translucency::prefer(&instance, s, &pick),
            _ => pick.adapter,
        };
        match Self::device_on(&adapter, wanted) {
            Ok((device, queue)) => {
                crate::marks::mark("gpu.device");
                crate::native_surface::note_device(&device, surface.is_some() || presented);
                Ok(Gpu::made(instance, adapter, device, queue, surface.is_some() || presented))
            }
            // A named adapter (`WGPU_ADAPTER_NAME`) never falls through to another GPU.
            Err(error) if named => Err(error),
            // A driver that can't make a device (a broken integrated GPU on a hybrid laptop)
            // leaves the window to the next adapter that presents to its surface.
            Err(error) => {
                let first = adapter.get_info();
                let others = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
                    .into_iter()
                    .filter(|other| {
                        let info = other.get_info();
                        (info.name != first.name || info.backend != first.backend)
                            && surface.is_none_or(|s| other.is_surface_supported(s))
                            && (!translucent || surface.is_none_or(|s| crate::render::translucency::mode(s, other).is_some()))
                    });
                for other in others {
                    if let Ok((device, queue)) = Self::device_on(&other, wanted) {
                        eprintln!("rune: {} ({:?}) made no device ({error}); using {}", first.name, first.backend, other.get_info().name);
                        crate::native_surface::note_device(&device, surface.is_some() || presented);
                        return Ok(Gpu::made(instance, other, device, queue, surface.is_some() || presented));
                    }
                }
                Err(error)
            }
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
            // `Performance` suballocates from 128 MB device and 64 MB host blocks, so a device
            // that holds a few atlases commits about 190 MB more than it uses: process memory on
            // an integrated GPU, and on Windows on a discrete one too. Rune keeps and reuses its
            // buffers and textures, so the smaller blocks only cost a few more heaps.
            memory_hints: wgpu::MemoryHints::MemoryUsage,
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

    #[cfg(target_arch = "wasm32")]
    fn try_for(i: wgpu::Instance, s: Option<&wgpu::Surface<'_>>, w: wgpu::Features, _: bool) -> Result<Gpu, String> { Self::try_with_features(i, s, w) }

    pub fn headless() -> Gpu {
        Self::new(Self::instance(), None)
    }

    /// A headless GPU on wgpu's backends, for tests of what needs wgpu-hal.
    #[cfg(test)]
    pub(crate) fn headless_wgpu() -> Gpu {
        Self::new(Self::wgpu_instance(), None)
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
    pub(crate) fn instance_for(wanted: wgpu::Features) -> wgpu::Instance {
        if crate::gpu::backend::wgpu_for(wanted) { Self::wgpu_instance() } else { Self::instance() }
    }

    /// wgpu's own backends, whatever `RUNE_GPU_BACKEND` says: DX12 alone on Windows. It's the
    /// backend that composites transparent surfaces, and a Vulkan instance loads every GPU's Vulkan
    /// driver, where a broken one (Intel's, on a hybrid laptop) crashed the process before its first
    /// window (Hex #662). A machine without a hardware D3D12 adapter gets WARP, D3D12's software
    /// adapter, from the same instance.
    #[cfg(target_os = "windows")]
    fn wgpu_instance() -> wgpu::Instance {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        let instance = Self::wgpu_instance_on(wgpu::Backends::DX12);
        crate::marks::mark("gpu.instance");
        instance
    }

    /// wgpu's own backends, whatever `RUNE_GPU_BACKEND` says.
    #[cfg(not(target_os = "windows"))]
    fn wgpu_instance() -> wgpu::Instance {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        Self::wgpu_instance_on(wgpu::Backends::all())
    }

    /// One instance per set of backends for the whole process: a new one per device was a new
    /// DXGI factory and adapter list each time a window or overlay made a device (hex#1268).
    fn wgpu_instance_on(backends: wgpu::Backends) -> wgpu::Instance {
        #[cfg(not(target_arch = "wasm32"))]
        {
            static MADE: std::sync::Mutex<Vec<(wgpu::Backends, wgpu::Instance)>> = std::sync::Mutex::new(Vec::new());
            let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, instance)) = made.iter().find(|(made, _)| *made == backends) { return instance.clone(); }
            let instance = Self::new_wgpu_instance(backends);
            made.push((backends, instance.clone()));
            instance
        }
        #[cfg(target_arch = "wasm32")]
        {
            // With `wgpu/webgl`, wgpu's wasm handles are single-threaded (`!Send + !Sync`), so
            // keep the per-backend instance cache in thread-local storage.
            thread_local! {
                static MADE: std::cell::RefCell<Vec<(wgpu::Backends, wgpu::Instance)>> = const { std::cell::RefCell::new(Vec::new()) };
            }
            MADE.with(|made| {
                let mut made = made.borrow_mut();
                if let Some((_, instance)) = made.iter().find(|(made, _)| *made == backends) { return instance.clone(); }
                let instance = Self::new_wgpu_instance(backends);
                made.push((backends, instance.clone()));
                instance
            })
        }
    }

    fn new_wgpu_instance(backends: wgpu::Backends) -> wgpu::Instance {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = backends;
        descriptor.flags.remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
        // An HWND's DXGI swapchain is opaque only; DirectComposition composites alpha on every GPU.
        descriptor.backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
        // A window waits for room in its swap chain itself, in the loop rather than in
        // `acquire` (`window::slot`).
        descriptor.backend_options.dx12.latency_waitable_object = wgpu::Dx12UseFrameLatencyWaitableObject::DontWait;
        // FXC ships with Windows (d3dcompiler_47.dll in System32). wgpu's default looks for
        // dxcompiler.dll on PATH, where another app's incompatible copy crashes the first
        // shader compile (Hex #587).
        descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
        wgpu::Instance::new(descriptor)
    }

    /// The GPU for a window's first surface, which `surface` makes from the instance:
    /// `prewarm_gpu`'s device, with the renderer it made, when its adapter presents to that
    /// surface and has `wanted`'s features; otherwise a new device on the same instance, so the
    /// window never gets a second surface.
    /// A `translucent` surface gets an adapter that composites translucent pixels when any can.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn for_surface<'w>(wanted: wgpu::Features, translucent: bool,
        surface: impl FnOnce(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>) -> Result<(Gpu, wgpu::Surface<'w>, Option<Renderer>), String> {
        Self::for_surface_from(prewarm::take(), wanted, translucent, surface)
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(super) fn for_surface_from<'w>(early: Option<(Gpu, Renderer)>, wanted: wgpu::Features, translucent: bool,
        surface: impl FnOnce(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>) -> Result<(Gpu, wgpu::Surface<'w>, Option<Renderer>), String> {
        let made = Self::surface_gpu_from(early, wanted, translucent, surface)?;
        if std::env::var_os("RUNE_GPU_LOG").is_some_and(|v| v != "0") {
            eprintln!("rune: a surface draws with {}", crate::gpu::backend::describe(&made.0.device));
        }
        Ok(made)
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
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
            return Ok((early, surface, Some(renderer)));
        }
        let instance = early.instance.clone();
        drop((renderer, early));
        gpu_events::starting();
        Ok((Self::try_for(instance, Some(&surface), wanted, translucent)?, surface, None))
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

/// Drops what `prewarm_gpu` made if no window took it.
pub(crate) fn discard_prewarm() {
    #[cfg(not(test))]
    prewarm::discard();
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
