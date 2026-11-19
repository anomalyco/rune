//! The instanced wgpu renderer, with lazy bounded group layers and offscreen readback.

use std::sync::{Arc, atomic::AtomicBool};

use crate::pixels::ImageAtlas;
use crate::text::{ATLAS_SIZE, Atlas};
/// Rows the first glyph atlas page's texture starts with: a window that shows a few labels holds
/// 512 KiB of it, not 4 MiB. It doubles as its shelves fill, up to `ATLAS_SIZE` (`prepare`).
pub(crate) const ATLAS_START_ROWS: u32 = 256;

mod backdrop_blur;
mod breaker;
#[cfg(any(target_os = "windows", target_arch = "wasm32"))]
mod damaged;
// The kept frame it checks exists only where partial frames do (`damaged`).
#[cfg(any(target_os = "windows", target_arch = "wasm32"))]
mod pixel_check;

/// `RUNE_LOOP_CHECK=2` found a picture that differed from the whole frame's (`pixel_check`), so the
/// process's exit code is 3. Never on macOS and Linux, which have no loop-only frames.
pub(crate) fn loop_pixel_check_failed() -> bool {
    #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
    return pixel_check::failed();
    #[cfg(not(any(target_os = "windows", target_arch = "wasm32")))]
    false
}
#[cfg(target_os = "windows")]
pub(crate) use damaged::MAX_RECTS;
pub mod cpu_boot;
pub(crate) mod csd_corners;
mod dxbc;
mod fill;
#[doc(hidden)]
pub use fill::FillDrift;
mod first_use;
pub(crate) mod gpu_events;
mod gpu_loss;
mod instance;
pub(crate) mod layer_blur;
mod layers;
mod metallib;
mod outlines;
mod pass;
use pass::{ActivePass, Backdrop};
mod pipelines;
pub(crate) mod surface_alpha;
pub(crate) mod translucency;
mod uber_wgsl;
mod upload;
mod variant;
#[cfg(target_os = "macos")]
mod windfoil_metallib;
pub use backdrop_blur::MAX_BACKDROP_BLURS;
pub use first_use::{make_layer_blur, make_straight_alpha, precompiled_misses};
pub(crate) use instance::{Globals, INSTANCE_WORDS, effect_mark, end_layer, pack_u64_f32, turned_bounds, unpack_u64_f32, word};
pub use instance::{
    Instance, KIND_BACKDROP_BLUR, KIND_DASHED, KIND_EFFECT, KIND_GLYPH, KIND_GRADIENT, KIND_IMAGE, KIND_INSET, KIND_LAYER, KIND_LAYER_END, KIND_RADIAL,
    KIND_RECT, KIND_SEGMENT, KIND_SHADOW, KIND_TRIANGLE, Kind, NO_CLIP, pack_clip_radii, pack_fade, unpack_clip_radii, unpack_fade,
};
pub use layers::LayerStats;
/// The kinds the frame pass has a pipeline of its own for (`regen_dxbc` compiles each).
#[doc(hidden)]
pub const PINNED_KINDS: [u32; 3] = variant::PINNED;

/// BLAKE3 of the WGSL the frame pass's pipeline pinned to `kind` compiles from (`variant::pinned_wgsl`, `\r\n` read as
/// `\n`): what `regen_dxbc` records beside each pinned fragment and `dxbc::uber_pinned` checks before using one.
#[doc(hidden)]
pub fn pinned_wgsl_blake3(kind: u32) -> Option<String> {
    variant::pinned_wgsl(kind).map(|wgsl| blake3::hash(wgsl.replace("\r\n", "\n").as_bytes()).to_hex().to_string())
}

pub use rune_core::Blend;

pub(crate) fn blend_state(blend: Blend) -> wgpu::BlendState {
    match blend {
        Blend::SourceOver => wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        Blend::PlusLighter => {
            let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
            wgpu::BlendState { color: add, alpha: add }
        }
    }
}

/// Composition properties of an isolated subtree. Filters can extend this contract without
/// changing painter-stream scopes or the target pool.
#[derive(Clone, Copy, Debug)]
pub struct LayerSpec {
    pub opacity: f32,
    pub blend: Blend,
    /// Gaussian standard deviation in device pixels, clamped to 64. Zero bypasses filtering.
    pub blur: f32,
    /// Radius of the rounded `clip` rectangle, in device pixels; zero is rectangular and a
    /// negative radius rounds with continuous corners (`El::continuous_corners`). Applied to
    /// the completed layer after filtering. The caller supplies finite clip edges.
    pub clip_radius: f32,
}

impl Default for LayerSpec {
    fn default() -> Self { Self { opacity: 1.0, blend: Blend::SourceOver, blur: 0.0, clip_radius: 0.0 } }
}

/// WGSL for clipping like Rune does: `sd_rounded`, `ramp` and `clip_alpha(p, clip, fade)`. `p` is
/// a window device-pixel position, `clip` an instance's `clip` and `fade` (`vec4<u32>`) its packed
/// soft edges and rounded-clip radius, so a compositor shader cuts its effect with the same
/// rectangle, feathers and circular or continuous corners as the primitives around it. Prepend it
/// to the shader source.
pub const CLIP_WGSL: &str = include_str!("shaders/clip.wgsl");

/// The WGSL the precompiled DX12 bytecode is compiled from (Windfoil's, CRLF read as LF), its
/// BLAKE3 and the hash of its pipeline layout, as the bytecode records them;
/// `examples/regen_dxbc.rs` regenerates that bytecode from them.
#[cfg(target_os = "windows")]
#[doc(hidden)]
pub fn uber_shader_identity() -> (&'static str, &'static str, u64) {
    (variant::windfoil_wgsl(), uber_wgsl::WINDFOIL_BLAKE3, variant::windfoil_abi_hash())
}

/// Where a compositor is asked to draw an effect slot (`Renderer::draw_range_with`).
pub struct EffectSite<'a> {
    /// The slot's position in the painter stream.
    pub index: u32,
    /// The slot's record (`KIND_EFFECT`): rect, radii, clip and fade.
    pub instance: &'a Instance,
    /// The slot's `El::effect` tag, so a compositor can find the resources it keeps for it.
    pub mark: u32,
    /// The slot's `El::effect_data` payload, as the element was last built. A slot playing its
    /// exit transition keeps the payload it had when it was removed.
    pub data: [f32; 4],
    /// Device pixels per point at this slot (`instance.params[3]`: `Env::scale` times any
    /// element or ancestor `scale` / `live_scale`).
    pub scale: f32,
    /// Format of the pass being recorded.
    pub format: wgpu::TextureFormat,
    /// Window device-pixel position of this pass's top-left pixel: zero in the frame target,
    /// the layer's bounds origin inside an isolated layer. Subtract it from window coordinates.
    pub origin: [f32; 2],
    /// Size of this pass's target in device pixels.
    pub size: [u32; 2],
    /// True inside an isolated layer: the pass is that layer's offscreen target.
    pub in_layer: bool,
    /// Whether colours are sRGB-encoded for blending (`Renderer::srgb_blend`). Choose a
    /// pipeline's encoding from this: encode the shader's output when it's true, write linear
    /// values when it's false. Don't use `format.is_srgb()`: an isolated layer's `Rgba16Float`
    /// target is neither, and holds whichever encoding the frame uses.
    pub srgb_blend: bool,
    /// Viewport-sized copy of the pass's target up to this slot when the slot asked for a
    /// backdrop (`El::effect_backdrop(true)`, `instance.params[0] != 0.0`) and the pass was run
    /// with a command encoder (`Renderer::draw_with`, `Renderer::draw_with_backdrop`,
    /// `Renderer::draw_target_range_with`, or `Renderer::encode_layers_with`). Has the same
    /// `format` as the pass (`*Srgb` / `Rgba16Float` in linear frames, non-sRGB / `Rgba16Float`
    /// holding sRGB-encoded premultiplied values when `srgb_blend` is true; pass it directly to
    /// `FxRenderer::blur_backdrop` or `FxRenderer::set_layer_backdrop`). Inside an isolated layer
    /// (`in_layer: true`), holds that layer's offscreen target copied at `origin` in window
    /// coordinates.
    pub backdrop: Option<&'a wgpu::TextureView>,
}

/// Called at each effect slot, with the pass open and everything painted before the slot already
/// recorded, and the scissor set to the slot's clip rectangle (a slot clipped away entirely is
/// skipped). Soft edges and rounded corners on a slot outside a layer are the hook's to apply
/// (`CLIP_WGSL`). It may record draws; the viewport and scissor are reset to the full target after.
pub type EffectHook<'h> = dyn FnMut(&mut wgpu::RenderPass<'_>, &EffectSite<'_>) + 'h;

/// Phase of an effect slot callback in `Renderer::draw_with_backdrop`,
/// `Renderer::encode_layers_with_backdrop`, and `Renderer::draw_target_range_with`.
pub enum EffectPhase<'a, 'p> {
    /// Outside the render pass, right after the target up to a backdrop slot
    /// (`El::effect_backdrop(true)`) was copied into `site.backdrop` and before the pass re-opens
    /// with `LoadOp::Load` (e.g. for `FxRenderer::blur_backdrop`).
    Backdrop(&'a mut wgpu::CommandEncoder),
    /// Inside the open render pass at the slot's painter position, with the scissor set to the
    /// slot's clip rectangle.
    Draw(&'a mut wgpu::RenderPass<'p>),
}

/// Called at each effect slot: first with `EffectPhase::Backdrop(encoder)` while the pass is
/// closed if the slot requested a backdrop (`El::effect_backdrop(true)`), then with
/// `EffectPhase::Draw(pass)` inside the open pass.
pub type EffectBackdropHook<'h> = dyn FnMut(EffectPhase<'_, '_>, &EffectSite<'_>) + 'h;

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Set by the device's lost callback: a device that went away (a driver reset, a GPU removed,
    /// Windows' Modern Standby) can't configure a surface, and wgpu drops that error, so nothing
    /// else would report it. See `is_lost`.
    lost: Arc<AtomicBool>,
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
    fn try_for_display(instance: wgpu::Instance, wanted: wgpu::Features) -> Result<Gpu, String> {
        Self::try_picked(instance, None, wanted, false, true)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn try_picked(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, wanted: wgpu::Features, translucent: bool, presented: bool) -> Result<Gpu, String> {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        let pick = crate::gpu::pick::pick(&instance, surface, presented)?;
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
        static MADE: std::sync::Mutex<Vec<(wgpu::Backends, wgpu::Instance)>> = std::sync::Mutex::new(Vec::new());
        let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, instance)) = made.iter().find(|(made, _)| *made == backends) { return instance.clone(); }
        let instance = Self::new_wgpu_instance(backends);
        made.push((backends, instance.clone()));
        instance
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
    fn for_surface_from<'w>(early: Option<(Gpu, Renderer)>, wanted: wgpu::Features, translucent: bool,
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
fn choose_then_start(backend: crate::GpuBackend, choose: impl FnOnce(crate::GpuBackend), start: impl FnOnce()) {
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

#[cfg(target_os = "macos")]
mod prewarm;

#[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
#[path = "render/warm.rs"]
mod prewarm;

#[cfg(target_arch = "wasm32")]
mod prewarm {
    pub(super) fn start() {}
    pub(super) fn take() -> Option<(super::Gpu, super::Renderer)> { None }
    pub(super) fn discard() {}
}

mod warmup;
pub use warmup::{warm_gpu, warm_gpu_with};
pub(crate) use warmup::{park_gpu, surface_gpu};

#[derive(Clone, Copy, PartialEq, Eq)]
enum BatchBind {
    /// Glyph atlas page 0 with image atlas page `usize`.
    Image(usize),
    /// Secondary glyph atlas page `usize` (index in `atlas_pages`, i.e. glyph page - 1).
    Glyph(usize),
}

pub struct Renderer {
    surfaces: Option<crate::native_surface::SurfaceRenderer>,
    /// The native surface renderer while its pipeline compiles (`build_surfaces`).
    surfaces_building: Option<crate::native_surface::Building>,
    pipeline: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    shader: dxbc::Shaders,
    layout: wgpu::PipelineLayout,
    vertex_entry: &'static str,
    /// Pipelines for other target formats a compositor draws into (`add_format`).
    extra: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    bind_group: wgpu::BindGroup,
    bind_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    atlas: wgpu::Texture,
    /// Which uber shader draws (`variant::Uber`), with the outline store's texture under Windfoil.
    uber: variant::Uber,
    /// Windfoil's subpixel pipeline for the window (`variant::Lcd`), and whether this frame's
    /// text draws subpixel masks.
    lcd: variant::Lcd,
    /// The frame pass's pipelines pinned to one kind each (`variant::PINNED`), once their thread has made them
    /// (`variant::start_pinned`); empty where there's no bytecode for them, None until they arrive.
    pinned: Option<Vec<Option<wgpu::RenderPipeline>>>,
    /// Those of the formats `extra` keeps a frame pipeline for, so a switch back makes none.
    pinned_extra: Vec<(wgpu::TextureFormat, Vec<Option<wgpu::RenderPipeline>>)>,
    /// The pinned pipelines being made on their thread (`variant::start_pinned`), for this format.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pinned_building: Option<(wgpu::TextureFormat, std::sync::mpsc::Receiver<Vec<Option<wgpu::RenderPipeline>>>)>,
    /// Each instance's kind (`params[1]`), as batched: where a run of one kind starts and ends.
    kinds: Vec<Option<Kind>>,
    lcd_frame: bool,
    /// The window's wake, for work a frame starts on another thread (set_frame_wake).
    frame_wake: Option<std::sync::Arc<crate::live::Wake>>,
    /// The 1×1 image the base bind group holds until the first image page.
    stand_in: wgpu::Texture,
    /// Secondary glyph atlas pages (`1..`).
    atlas_pages: Vec<(wgpu::Texture, wgpu::BindGroup)>,
    /// Stable image pages; the initial bind group uses a 1×1 stand-in.
    images: Vec<Option<(wgpu::Texture, wgpu::BindGroup)>>,
    image_batches: Vec<(std::ops::Range<u32>, BatchBind)>,
    instances: rune_staging::GrowBuffer,
    count: u32,
    springs: Option<crate::gpu::springs::Bindings>,
    spring_instances: usize,
    layers: layers::Layers,
    srgb_blend: bool,
    /// Effect slots of the uploaded instances, by index.
    effects: Vec<(u32, Instance)>,
    /// Pooled viewport-sized backdrop copies, allocated only when a slot sets `effect_backdrop`
    /// or an element sets `backdrop_blur`.
    backdrops: Vec<Backdrop>,
    /// The last prepared frame had an effect reading the backdrop: a rest keeps the copies.
    backdrop_used: bool,
    /// Pipelines and pyramid for backdrop blur records, built on the first one.
    backdrop_blur: Option<backdrop_blur::BackdropBlur>,
    /// Which lazily built pipelines the device took (`breaker`).
    breakers: breaker::Breakers,
    uploads: upload::Uploads,
    /// Atlas fills `prepare` staged, recorded into the frame's encoder with the uploads.
    fills: std::sync::Mutex<Option<fill::PendingFills>>,
    /// Unique to this renderer, so to the device it was made on (`Frame::device_key`).
    device_key: crate::compose::DeviceKey,
    /// The device's largest 2D texture, read once: in a browser each `Device::limits` reads every
    /// limit from JavaScript (about 35 µs), which a frame's layers used to pay.
    max_texture_dim: u32,
    /// The one instance per rectangle that clears a damaged one (`arm_damage`), made on first use.
    #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
    clear_quad: std::sync::OnceLock<wgpu::Buffer>,
    /// The kept frame and the rectangles the next `draw` may redraw alone (`damaged`).
    #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
    damage: std::sync::Mutex<damaged::DamageState>,
}

impl Renderer {
    /// `Frame::device_key`: a renderer belongs to one device, and a new device gets a new one.
    pub fn device_key(&self) -> crate::compose::DeviceKey {
        self.device_key
    }

    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Renderer {
        Self::create(device, format, None, cfg!(target_os = "macos"))
    }

    pub fn set_srgb_blend(&mut self, enabled: bool) {
        self.srgb_blend = enabled;
        if let Some(s) = &mut self.surfaces {
            s.set_srgb_blend(enabled);
        }
    }

    /// Opt-in retained geometry. IDs map instances to shared translation/opacity nodes.
    /// Upload geometry once with `prepare`, then use `present_at` without rebuilding instances.
    pub fn with_springs(device: &wgpu::Device, format: wgpu::TextureFormat, nodes: &[crate::gpu::springs::Node], ids: &[u32]) -> Renderer {
        Self::create(device, format, Some((nodes, ids)), true)
    }

    /// A renderer that starts with the Classic uber shader, as on macOS, and builds Windfoil when
    /// text first draws from outlines (render/variant.rs); for checking that path elsewhere.
    #[doc(hidden)]
    pub fn new_classic(device: &wgpu::Device, format: wgpu::TextureFormat) -> Renderer {
        Self::create(device, format, None, true)
    }

    /// Draws uploaded instances `range` into a pass whose target has `format` (the renderer's
    /// own, or one added with `add_format`). Call `encode_layers` before opening the pass.
    /// Scopes are atomic: list/range boundaries must not bisect a layer.
    pub fn draw_range(&self, pass: &mut wgpu::RenderPass<'_>, format: wgpu::TextureFormat, range: std::ops::Range<u32>) {
        if range.is_empty() && range.end != self.count { return; }
        self.submit_uploads();
        self.validate_layer_range(&range);
        let mut active = ActivePass::borrowed(pass);
        self.draw_layer_range(&mut active, format, range, None, &mut |_, _| {});
    }

    /// `draw_range` that stops at each effect slot (`El::effect`) in `range` and calls `hook`
    /// there, in painter order. Effects inside an isolated layer are reached through
    /// `encode_layers_with`, which draws that layer's pass; scopes nest as in `draw_range`.
    /// Because `pass` is already open, `EffectSite::backdrop` is `None`; use `draw_with`,
    /// `draw_with_backdrop`, or `draw_target_range_with` when slots request a backdrop copy.
    pub fn draw_range_with(&self, pass: &mut wgpu::RenderPass<'_>, format: wgpu::TextureFormat, range: std::ops::Range<u32>, hook: &mut EffectHook<'_>) {
        if range.is_empty() && range.end != self.count { return; }
        self.submit_uploads();
        self.validate_layer_range(&range);
        let mut active = ActivePass::borrowed(pass);
        self.draw_layer_range(&mut active, format, range, None, &mut |phase, site| {
            if let EffectPhase::Draw(pass) = phase { hook(pass, site); }
        });
    }

    /// Encodes isolated layers and draws the frame into `view` (cleared to `clear`), calling
    /// `hook` at each effect slot (`El::effect`) in painter order. At slots with
    /// `El::effect_backdrop(true)`, ends the current pass, copies the target up to that slot into
    /// a pooled backdrop texture, re-opens the pass with `LoadOp::Load`, and hands the backdrop
    /// view in `EffectSite::backdrop`.
    pub fn draw_with(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: [f32; 4], hook: &mut EffectHook<'_>) {
        self.draw_with_backdrop(encoder, view, clear, &mut |phase, site| {
            if let EffectPhase::Draw(pass) = phase { hook(pass, site); }
        });
    }

    /// Like `draw_with`, also calling `hook(EffectPhase::Backdrop(encoder), site)` while the
    /// render pass is closed at each backdrop slot (`El::effect_backdrop(true)`) so the compositor
    /// can blur or pre-process `site.backdrop` (e.g. `FxRenderer::blur_backdrop`) before the pass
    /// re-opens for `EffectPhase::Draw(pass)`.
    pub fn draw_with_backdrop(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear: [f32; 4],
        hook: &mut EffectBackdropHook<'_>,
    ) {
        self.encode_layers_with_backdrop(encoder, hook);
        let clear = wgpu::Color { r: clear[0] as f64, g: clear[1] as f64, b: clear[2] as f64, a: clear[3] as f64 };
        self.draw_target_range_with(encoder, view, self.format, wgpu::LoadOp::Clear(clear), 0..self.count, hook);
    }

    /// Draws `range` into `view` (opening the pass with `load`), splitting the pass on `encoder`
    /// only at backdrop slots (`El::effect_backdrop(true)`) in `range` to copy `view` into
    /// `EffectSite::backdrop` and run `EffectPhase::Backdrop(encoder)` before re-opening with
    /// `LoadOp::Load`. Call `encode_layers_with` / `encode_layers_with_backdrop` before drawing
    /// the first range.
    /// `view` must be a single-sample, full-size, mip-zero 2D target; backdrop slots require
    /// its texture to have `COPY_SRC` usage. These owned passes have no depth/stencil or resolve
    /// attachment. Use `draw_range_with` for a caller-owned pass instead (no backdrop copies).
    pub fn draw_target_range_with(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        load: wgpu::LoadOp<wgpu::Color>,
        range: std::ops::Range<u32>,
        hook: &mut EffectBackdropHook<'_>,
    ) {
        if range.is_empty() && range.end != self.count { return; }
        self.validate_layer_range(&range);
        self.flush_uploads(encoder);
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }).forget_lifetime();
        let mut active = ActivePass::owned(pass, encoder, view);
        self.draw_layer_range(&mut active, format, range, None, hook);
    }

    /// `draw_instances`, split after each effect slot in `range` so `hook` runs at its place.
    fn draw_with_effects(&self, pass: &mut ActivePass<'_, '_>, format: wgpu::TextureFormat, range: std::ops::Range<u32>, target: Option<usize>, hook: &mut EffectBackdropHook<'_>) {
        let first = self.effects.partition_point(|(at, _)| *at < range.start);
        let mut cursor = range.start;
        for (at, instance) in self.effects[first..].iter().take_while(|(at, _)| *at < range.end) {
            self.draw_instances(pass, format, cursor..*at + 1, target);
            cursor = *at + 1;
            if instance.kind() == Some(Kind::BackdropBlur) {
                self.draw_backdrop_blur(pass, format, *at, instance, target);
                continue;
            }
            let (origin, size) = self.target_frame(target);
            // Scissor to the slot's clip rectangle, so a compositor that ignores `clip` still
            // stays inside its scroll view or clipped ancestor.
            let c = instance.clip;
            let edge = |v: f32, o: f32, max: u32| (v - o).clamp(0.0, max as f32) as u32;
            let (x0, y0) = (edge(c[0].floor(), origin[0], size[0]), edge(c[1].floor(), origin[1], size[1]));
            let (x1, y1) = (edge(c[2].ceil(), origin[0], size[0]), edge(c[3].ceil(), origin[1], size[1]));
            if x1 <= x0 || y1 <= y0 { continue; }
            let backdrop = if instance.params[0] != 0.0 {
                pass.split_for_backdrop(self, *at, instance, format, origin, size, target.is_some(), hook)
            } else {
                None
            };
            pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
            hook(EffectPhase::Draw(pass), &EffectSite { index: *at, instance, mark: effect_mark(instance), data: instance.uv, scale: instance.params[3],
                format, origin, size, in_layer: target.is_some(), srgb_blend: self.srgb_blend, backdrop });
            pass.set_viewport(0.0, 0.0, size[0] as f32, size[1] as f32, 0.0, 1.0);
            pass.set_scissor(pass.scissor(), size);
        }
        self.draw_instances(pass, format, cursor..range.end, target);
    }

    fn draw_instances(&self, pass: &mut wgpu::RenderPass<'_>, format: wgpu::TextureFormat, range: std::ops::Range<u32>, target: Option<usize>) {
        if range.is_empty() {
            return;
        }
        let frame_pipeline = self.draw_pipeline(format, target);
        pass.set_pipeline(frame_pipeline);
        // The pinned pipelines draw the frame pass's runs of one kind (`variant::PINNED`), in paint order:
        // only where the frame's own pipeline would draw (not a layer target, an additive layer or subpixel text).
        let pinned = self.pinned.as_deref().filter(|p| p.iter().any(Option::is_some) && std::ptr::eq(frame_pipeline, &self.pipeline) && target.is_none() && format == self.format);
        // Frame-profile builds: `RUNE_PINNED=off` draws every span with the general shader, `all` splits every span
        // whatever its runs; the screen comparison runs both against the general build.
        #[cfg(feature = "frame-profile")]
        let pinned = pinned.filter(|_| pinned_forced() != Some(false));
        let mut bound = frame_pipeline;
        if let Some(properties) = &self.springs { pass.set_bind_group(1, &properties.group, &[]); }
        pass.set_vertex_buffer(0, self.instances.slice(..));
        let start = self.image_batches.partition_point(|(batch, _)| batch.end <= range.start);
        for (batch, bind_kind) in self.image_batches[start..].iter().take_while(|(batch, _)| batch.start < range.end) {
            let bind = match (*bind_kind, target) {
                (BatchBind::Image(page), Some(index)) => {
                    let t = self.layers.targets[index].as_ref().unwrap();
                    t.images.get(page).and_then(Option::as_ref).unwrap_or(&t.base)
                }
                (BatchBind::Image(page), None) => {
                    self.images.get(page).and_then(Option::as_ref).map_or(&self.bind_group, |(_, bind)| bind)
                }
                (BatchBind::Glyph(page), Some(index)) => {
                    let t = self.layers.targets[index].as_ref().unwrap();
                    t.glyphs.get(page).unwrap_or(&t.base)
                }
                (BatchBind::Glyph(page), None) => {
                    self.atlas_pages.get(page).map_or(&self.bind_group, |(_, bind)| bind)
                }
            };
            pass.set_bind_group(0, bind, &[]);
            let span = batch.start.max(range.start)..batch.end.min(range.end);
            // A span whose kinds alternate (a chip behind each token, a terminal's cell backgrounds between its
            // glyphs) draws in one call with the general shader: a draw and a pipeline switch per run cost more
            // than the smaller shader saves.
            #[cfg(not(feature = "frame-profile"))]
            let split = pinned.filter(|_| few_runs(&self.kinds, span.clone()));
            #[cfg(feature = "frame-profile")]
            let split = pinned.filter(|_| pinned_forced() == Some(true) || few_runs(&self.kinds, span.clone()));
            #[cfg(feature = "frame-profile")]
            PINNED_DRAWN[usize::from(split.is_some())].fetch_add(u64::from(span.end - span.start), std::sync::atomic::Ordering::Relaxed);
            let Some(pinned) = split else {
                if !std::ptr::eq(bound, frame_pipeline) { pass.set_pipeline(frame_pipeline); bound = frame_pipeline; }
                pass.draw(0..4, span);
                continue;
            };
            for run in kind_runs(&self.kinds, span) {
                let kind = self.kinds[run.start as usize];
                let pipeline = kind
                    .and_then(|k| variant::PINNED.iter().position(|&p| p == k.as_u32()))
                    .and_then(|at| pinned[at].as_ref())
                    .unwrap_or(&self.pipeline);
                if !std::ptr::eq(pipeline, bound) { pass.set_pipeline(pipeline); bound = pipeline; }
                pass.draw(0..4, run);
            }
        }
    }

    /// The format of the target the renderer was built for (`draw_range` with this format).

    pub fn format(&self) -> wgpu::TextureFormat { self.format }

    /// True when colours are sRGB-encoded for blending: the target is a non-sRGB view and every
    /// pass, offscreen layers included, holds encoded premultiplied values. False means linear
    /// premultiplied values, encoded by the `*Srgb` target on write.
    pub fn srgb_blend(&self) -> bool { self.srgb_blend }

    pub fn submit(&self, queue: &wgpu::Queue, mut encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        self.before_submit(&mut encoder);
        if let Some(s) = &self.surfaces { s.submit(queue,encoder) } else { queue.submit([encoder.finish()]) }
    }

    /// Whether `submit` is `queue.submit` after `before_submit` and nothing more: no native video
    /// surface waits on the frame's submission, so another thread may make it.
    #[cfg(target_os = "macos")]
    pub(crate) fn submits_plainly(&self) -> bool {
        self.surfaces.as_ref().is_none_or(|s| s.draws_nothing())
    }

    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: [f32; 4]) {
        #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
        if self.armed() && self.draw_armed(encoder, &view.texture()) { return; }
        self.draw_timed(encoder, view, clear, None);
    }

    /// Constant-time scheduler check. Present the terminal frame, then stop the display link.
    /// Deadlines are recomputed only on commits, so no per-node CPU sampling occurs on ticks.
    pub fn springs_animating(&self, time: f64) -> bool {
        self.springs.as_ref().is_some_and(|p| time < p.settle_at)
    }

    /// Draws over what `view` already holds instead of clearing it (headless overlays).
    pub(crate) fn draw_over(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        self.draw_load(encoder, view, wgpu::LoadOp::Load, None);
    }

    pub fn draw_timed(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: [f32; 4], timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let clear = wgpu::Color { r: clear[0] as f64, g: clear[1] as f64, b: clear[2] as f64, a: clear[3] as f64 };
        self.draw_load(encoder, view, wgpu::LoadOp::Clear(clear), timestamps);
    }

    fn draw_load(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>, timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let timestamps = timestamps.or_else(|| self.surfaces.as_ref().and_then(|s| s.timestamp_writes()));
        self.encode_layers_timed(encoder, timestamps.as_ref(), &mut |_, _| {});
        let timestamps = timestamps.map(|mut t| {
            if self.layer_stats().extra_passes > 0 { t.beginning_of_pass_write_index = None; }
            t
        });
        let blurs = self.has_backdrop_blur();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps.clone(),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if !blurs {
            self.draw_range(&mut pass, self.format, 0..self.count);
            return;
        }
        // Backdrop blurs split the frame pass, which needs it owned along with the encoder.
        let pass = pass.forget_lifetime();
        let mut active = ActivePass::owned_timed(pass, encoder, view, timestamps);
        self.draw_layer_range(&mut active, self.format, 0..self.count, None, &mut |_, _| {});
    }
}

/// How far a corner of radius `radius` reaches along each edge. Radii are signed: a negative
/// radius draws Apple's continuous corner, whose curve starts 1.528 times the radius from the
/// corner (`CONTINUOUS_K` in shader.wgsl).
pub(crate) fn corner_extent(radius: f32) -> f32 {
    if radius < 0.0 { -radius * CONTINUOUS_K } else { radius }
}

pub(crate) const CONTINUOUS_K: f32 = 1.528_664_8;

/// An offscreen sRGB target with CPU readback.
pub struct Offscreen {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    readback: rune_staging::Readback,
    pub width: u32,
    pub height: u32,
}

pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl Offscreen {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Offscreen {
        Self::with_format(device, width, height, OFFSCREEN_FORMAT)
    }

    pub fn with_format(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> Offscreen {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let readback = rune_staging::Readback::new(device, "readback", width, height, 4);
        Offscreen { texture, view, readback, width, height }
    }

    pub fn copy(&self, encoder: &mut wgpu::CommandEncoder) {
        self.readback.copy_from(encoder, &self.texture);
    }

    /// Underlying offscreen texture, for readback rings (`tooling::record`).
    pub(crate) fn texture(&self) -> &wgpu::Texture { &self.texture }

    /// Tightly packed sRGB RGBA8 after `copy` was submitted.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read(&self, device: &wgpu::Device) -> Vec<u8> {
        self.readback.read(device)
    }

    #[cfg(target_arch = "wasm32")]
    pub fn read(&self, _: &wgpu::Device) -> Vec<u8> {
        panic!("blocking GPU readback is unsupported on web")
    }
}

pub fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut w = enc.write_header().map_err(std::io::Error::other)?;
    w.write_image_data(rgba).map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod abi_tests;

#[cfg(test)]
mod backdrop_tests;

#[cfg(test)]
mod image_tests;

#[cfg(test)]
#[path = "render/native_surface_tests.rs"]
mod native_surface;

#[cfg(test)]
#[path = "render/warm_tests.rs"]
mod warm_tests;

#[cfg(test)]
mod prewarm_order_tests {
    #[test]
    fn prewarm_gpu_on_chooses_before_it_starts() {
        let log = std::cell::RefCell::new(Vec::new());
        super::choose_then_start(crate::GpuBackend::Metal, |b| log.borrow_mut().push(format!("choose {b:?}")), || log.borrow_mut().push("start".into()));
        assert_eq!(*log.borrow(), ["choose Metal", "start"]);
    }
}

/// `prewarm_gpu` and how a window picks it up, on a bare Metal layer (no window server needed).
#[cfg(all(test, target_os = "macos"))]
mod prewarm_tests {
    use super::{Gpu, Renderer};
    use objc2_video::{rc::Retained, runtime::{AnyClass, AnyObject}, msg_send};

    fn layer() -> Retained<AnyObject> {
        unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] }
    }

    fn surface<'w>(instance: &wgpu::Instance, layer: &Retained<AnyObject>) -> Result<wgpu::Surface<'w>, String> {
        let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(layer) as *mut std::ffi::c_void);
        // SAFETY: the layer outlives the surface in each test.
        unsafe { instance.create_surface_unsafe(target) }.map_err(|e| e.to_string())
    }

    /// The window gets the thread's device and its compiled pipeline, once, in the Mac window format.
    #[test]
    fn a_window_takes_the_prewarmed_gpu_and_renderer_once() {
        crate::prewarm_gpu();
        crate::prewarm_gpu();
        let layer = layer();
        let (gpu, _surface, renderer) = match Gpu::for_surface(wgpu::Features::empty(), false, |i| surface(i, &layer)) {
            Ok(taken) => taken,
            Err(error) => return eprintln!("skipped: {error}"),
        };
        let Some(renderer) = renderer else { return eprintln!("skipped: no GPU for the prewarm thread") };
        // Rune's Metal backend loads its metallib without advertising passthrough.
        #[cfg(feature = "metal-backend")]
        let native = crate::gpu::metal::raw_device(&gpu.device).is_some();
        #[cfg(not(feature = "metal-backend"))]
        let native = false;
        assert!(native || gpu.device.features().contains(wgpu::Features::PASSTHROUGH_SHADERS));
        assert_eq!(renderer.format, wgpu::TextureFormat::Bgra8Unorm);
        drop((renderer, gpu));
        let (_, _surface, again) = Gpu::for_surface(wgpu::Features::empty(), false, |i| surface(i, &layer)).expect("a second GPU");
        assert!(again.is_none(), "the prewarmed GPU goes to one window");

        // A slot that already took or discarded a GPU ignores a later `start`, so a late
        // `prewarm_gpu()` call cannot leave an unclaimed GPU in memory.
        let taken = super::prewarm::Slot::new();
        assert!(taken.take().is_none());
        taken.start();
        assert!(taken.take().is_none());

        let discarded = super::prewarm::Slot::new();
        discarded.start();
        discarded.discard();
        assert!(discarded.take().is_none());
        discarded.start();
        assert!(discarded.take().is_none());

        // `restart` replaces a working thread and leaves an idle or finished slot alone.
        let restarted = super::prewarm::Slot::new();
        restarted.restart();
        restarted.start();
        restarted.restart();
        assert!(restarted.take().is_some(), "the new thread's GPU");
        restarted.restart();
        assert!(restarted.take().is_none(), "a taken slot stays taken");
    }

    /// A prewarmed device without a feature the window wants (and the adapter has) is replaced on
    /// the same instance, with the surface made once: a second surface would add a second layer.
    #[test]
    fn a_prewarmed_gpu_missing_wanted_features_is_redone_on_its_instance() {
        let Ok(early) = Gpu::try_new(Gpu::instance(), None) else { return eprintln!("skipped: no GPU") };
        let plain = wgpu::Features::FLOAT32_FILTERABLE | wgpu::Features::DEPTH32FLOAT_STENCIL8 | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
        let missing = (early.adapter.features() - early.device.features()) & plain;
        let Some(wanted) = missing.iter().next() else { return eprintln!("skipped: the adapter has none of the test's features") };
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let layer = layer();
        let mut made = 0;
        let (gpu, _surface, renderer) = Gpu::for_surface_from(Some((early, renderer)), wanted, false, |i| { made += 1; surface(i, &layer) })
            .expect("a GPU with the wanted feature");
        assert_eq!(made, 1);
        assert!(renderer.is_none(), "the prewarmed renderer belongs to the dropped device");
        assert!(gpu.device.features().contains(wanted));

        let Ok(early) = Gpu::try_new(Gpu::instance(), None) else { return };
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let (_, _surface, kept) = Gpu::for_surface_from(Some((early, renderer)), wgpu::Features::empty(), false, |i| surface(i, &layer)).expect("a GPU");
        assert!(kept.is_some(), "a suitable prewarmed device keeps its renderer");
    }

    /// A compositor wanting a feature the native Metal backend lacks gets wgpu's device, when
    /// Metal is only the default, with the prewarmed native GPU dropped before any surface.
    #[cfg(feature = "metal-backend")]
    #[test]
    fn a_compositor_wanting_what_the_native_backend_lacks_draws_with_wgpu() {
        if !crate::gpu::metal::enabled() || std::env::var_os("RUNE_GPU_BACKEND").is_some() {
            return eprintln!("skipped: the native backend isn't the default here");
        }
        let early = Gpu::headless();
        assert!(crate::gpu::metal::raw_device(&early.device).is_some());
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let layer = layer();
        let mut made = 0;
        let wanted = wgpu::Features::POLYGON_MODE_LINE;
        let (gpu, _surface, renderer) = Gpu::for_surface_from(Some((early, renderer)), wanted, false, |i| { made += 1; surface(i, &layer) })
            .expect("a wgpu GPU");
        assert_eq!(made, 1);
        assert!(renderer.is_none());
        assert!(crate::gpu::metal::raw_device(&gpu.device).is_none(), "wgpu's device");
        assert!(gpu.device.features().contains(wanted));
        let described = crate::gpu::backend::describe(&gpu.device);
        assert!(described.starts_with("wgpu Metal on ") && described.contains("which Rune's Metal backend lacks"), "{described}");
        assert!(crate::gpu::backend::describe(&Gpu::headless().device).ends_with("(the default)"));
        assert!(!crate::gpu::backend::serves(&Gpu::headless().device, wanted));
        assert!(crate::gpu::backend::serves(&Gpu::headless().device, wgpu::Features::TIMESTAMP_QUERY));
    }

    /// A compositor wanting immediates keeps the native backend and the prewarmed device, which
    /// has them whether or not anything asked, with the adapter's immediate size.
    #[cfg(feature = "metal-backend")]
    #[test]
    fn a_compositor_wanting_immediates_keeps_the_native_backend_and_the_prewarmed_gpu() {
        if !crate::gpu::metal::enabled() || std::env::var_os("RUNE_GPU_BACKEND").is_some() {
            return eprintln!("skipped: the native backend isn't the default here");
        }
        let early = Gpu::headless();
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let layer = layer();
        let wanted = wgpu::Features::IMMEDIATES;
        let (gpu, _surface, kept) = Gpu::for_surface_from(Some((early, renderer)), wanted, false, |i| surface(i, &layer)).expect("a GPU");
        assert!(kept.is_some(), "the prewarmed renderer");
        assert!(crate::gpu::metal::raw_device(&gpu.device).is_some(), "the native backend's device");
        assert!(gpu.device.features().contains(wanted));
        assert_eq!(gpu.device.limits().max_immediate_size, 4096);
        let described = crate::gpu::backend::describe(&gpu.device);
        assert!(described.starts_with("Rune's Metal backend on ") && described.ends_with("(the default)"), "{described}");
    }
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

/// Frame-profile builds: `RUNE_PINNED` forcing the split on (`all`) or off (`off`), and the instances drawn
/// unsplit and split since the last report (`report_pinned`).
#[cfg(feature = "frame-profile")]
pub(super) fn pinned_forced() -> Option<bool> {
    static FORCED: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| match std::env::var("RUNE_PINNED").as_deref() { Ok("all") => Some(true), Ok("off") => Some(false), _ => None })
}

#[cfg(feature = "frame-profile")]
static PINNED_DRAWN: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// Frame-profile builds with `RUNE_PINNED` set: every 60th frame, how many instances of the window's pass the
/// pinned pipelines drew and how many went unsplit, so a comparison shows it exercised the pinned path.
#[cfg(feature = "frame-profile")]
pub(crate) fn report_pinned() {
    static FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let frame = FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if std::env::var_os("RUNE_PINNED").is_none() || frame % 60 != 0 { return; }
    let [whole, split] = [0, 1].map(|i| PINNED_DRAWN[i].swap(0, std::sync::atomic::Ordering::Relaxed));
    eprintln!("rune-pinned: {split} instances split into runs, {whole} drawn unsplit (frame {frame})");
}

/// Most runs a span is split into (`few_runs`), and fewest instances a run has on average.
const MAX_RUNS: usize = 64;
const MIN_RUN: usize = 8;

/// Whether `span` splits into few enough runs of one kind to draw them with the pinned pipelines: one run,
/// or at most `MAX_RUNS` of `MIN_RUN` instances or more on average.
fn few_runs(kinds: &[Option<Kind>], span: std::ops::Range<u32>) -> bool {
    let len = span.len();
    let runs = kind_runs(kinds, span).take(MAX_RUNS + 1).count();
    runs <= 1 || (runs <= MAX_RUNS && runs * MIN_RUN <= len)
}

/// `span` split into runs of instances of one kind (`kinds` by instance).
fn kind_runs(kinds: &[Option<Kind>], span: std::ops::Range<u32>) -> impl Iterator<Item = std::ops::Range<u32>> + '_ {
    let mut at = span.start;
    std::iter::from_fn(move || {
        if at >= span.end { return None; }
        let kind = kinds.get(at as usize).copied();
        let start = at;
        while at < span.end && kinds.get(at as usize).copied() == kind { at += 1; }
        Some(start..at)
    })
}
