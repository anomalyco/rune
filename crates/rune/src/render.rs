//! The instanced wgpu renderer, with lazy bounded group layers and offscreen readback.

use crate::pixels::ImageAtlas;
use crate::text::{ATLAS_SIZE, Atlas};

mod backdrop_blur;
mod breaker;
mod damaged;
mod device;
mod dxbc;
mod effect_warm;
mod fill;
mod first_use;
pub(crate) mod loss_latch;
mod gpu_loss;
mod instance;
mod layers;
mod metallib;
mod offscreen;
mod outlines;
mod pass;
mod pipelines;
mod renderer_kind;
// The kept frame it checks exists only where partial frames do (`damaged`).
#[cfg(target_os = "windows")]
mod pixel_check;
#[cfg(target_os = "macos")]
mod prewarm;
#[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
#[path = "render/warm.rs"]
mod prewarm;
#[cfg(target_arch = "wasm32")]
mod prewarm {
    pub(super) fn start() {}
    pub(super) fn discard() {}
}
mod shader_check;
mod surface_setup;
mod uber_wgsl;
mod upload;
mod variant;
#[cfg(any(target_os = "linux", all(test, not(target_arch = "wasm32"))))]
mod vulkan_icd;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod warm_machine;
pub(crate) mod warmup;
#[cfg(test)]
mod wgsl_minify;
#[cfg(target_os = "macos")]
mod windfoil_metallib;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod csd_corners;
pub(crate) mod gpu_events;
pub(crate) mod layer_blur;
pub(crate) mod surface_alpha;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod translucency;

pub mod cpu_boot;
pub use loss_latch::GpuLossPolicy;

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
mod opacity_tests;
#[cfg(test)]
mod prewarm_tests;
#[cfg(test)]
#[path = "render/warm_tests.rs"]
mod warm_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod webgl_shader_tests;

use pass::{ActivePass, Backdrop};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use damaged::KeptDraw;
pub(crate) use damaged::MAX_RECTS;
pub(crate) use shader_check::ShaderError;
pub(crate) use surface_setup::SurfaceSetupError;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use surface_setup::format_window_setup_error;
#[cfg(target_os = "windows")]
pub(crate) use surface_setup::with_wine_hwnd_hint;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use device::cancel_prewarm;
pub(crate) use device::choose_gpu_backend;
pub(crate) use device::discard_prewarm;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use device::reopen_prewarm;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use device::shutdown_prewarm;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use warm_machine::ShutdownOutcome;
pub(crate) use instance::Globals;
pub(crate) use instance::INSTANCE_WORDS;
pub(crate) use instance::effect_mark;
pub(crate) use instance::end_layer;
pub(crate) use instance::pack_u64_f32;
pub(crate) use instance::shader_consts;
pub(crate) use instance::turned_bounds;
pub(crate) use instance::unpack_u64_f32;
pub(crate) use instance::word;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use warmup::park_gpu;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use warmup::surface_gpu;
#[cfg(target_os = "linux")]
pub(crate) use warmup::{warm_gpu_made, warm_gpu_then};

pub use backdrop_blur::MAX_BACKDROP_BLURS;
pub use device::Gpu;
pub use device::prewarm_gpu;
pub use device::prewarm_gpu_on;
pub use device::set_gpu_memory_policy;
pub use effect_warm::PrewarmEffects;
pub use renderer_kind::RendererKind;
#[doc(hidden)]
pub use fill::FillDrift;
pub use first_use::builtin_wgsl_hash;
pub use first_use::make_layer_blur;
pub use first_use::make_straight_alpha;
pub use first_use::precompiled_misses;
pub use instance::Instance;
pub use instance::KIND_BACKDROP_BLUR;
pub use instance::KIND_DASHED;
pub use instance::KIND_EFFECT;
pub use instance::KIND_GLYPH;
pub use instance::KIND_GRADIENT;
pub use instance::KIND_IMAGE;
pub use instance::KIND_INSET;
pub use instance::KIND_LAYER;
pub use instance::KIND_LAYER_END;
pub use instance::KIND_RADIAL;
pub use instance::KIND_RECT;
pub use instance::KIND_SEGMENT;
pub use instance::KIND_SHADOW;
pub use instance::KIND_TRIANGLE;
pub use instance::Kind;
pub use instance::NO_CLIP;
pub(crate) use instance::STOP_FADE_BIAS;
pub use instance::pack_clip_radii;
pub use instance::pack_fade;
pub use instance::unpack_clip_radii;
pub use instance::unpack_fade;
pub use layers::LayerStats;
pub use offscreen::OFFSCREEN_FORMAT;
pub use offscreen::OFFSCREEN_FORMAT_LINEAR;
pub use offscreen::Offscreen;
pub use offscreen::write_png;
pub use rune_core::Blend;
pub use warmup::warm_gpu;
pub use warmup::warm_gpu_cache;
pub use warmup::warm_gpu_with;

/// Rows the first glyph atlas page's texture starts with: a window that shows a few labels holds
/// 512 KiB of it, not 4 MiB. It doubles as its shelves fill, up to `ATLAS_SIZE` (`prepare`).
pub(crate) const ATLAS_START_ROWS: u32 = 256;

/// `RUNE_LOOP_CHECK=2` found a picture that differed from the whole frame's (`pixel_check`), so the
/// process's exit code is 3. Never on macOS and Linux, which have no loop-only frames.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn loop_pixel_check_failed() -> bool {
    #[cfg(target_os = "windows")]
    return pixel_check::failed();
    #[cfg(not(target_os = "windows"))]
    false
}

/// The kinds the frame pass has a pipeline of its own for (`regen_dxbc` compiles each).
#[doc(hidden)]
pub const PINNED_KINDS: [u32; 3] = variant::PINNED;

/// BLAKE3 of the WGSL the frame pass's pipeline pinned to `kind` compiles from (`variant::pinned_wgsl`, `\r\n` read as
/// `\n`): what `regen_dxbc` records beside each pinned fragment and `dxbc::uber_pinned` checks before using one.
#[doc(hidden)]
pub fn pinned_wgsl_blake3(kind: u32) -> Option<String> {
    variant::pinned_wgsl(kind).map(|wgsl| blake3::hash(wgsl.replace("\r\n", "\n").as_bytes()).to_hex().to_string())
}

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
pub const CLIP_WGSL: &str = uber_wgsl::CLIP;

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
    /// Packed explicit scroll-fade stops (`Runtime::fade_stops`) for this frame, matching
    /// `clip_alpha_stops` in `CLIP_WGSL` (`[0.0, 0.0]` when no scroller uses `ScrollFade::stops`).
    pub fade_stops: [f32; 2],
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum BatchBind {
    /// Glyph atlas page 0 with image atlas page `usize`.
    Image(usize),
    /// Secondary glyph atlas page `usize` (index in `atlas_pages`, i.e. glyph page - 1).
    Glyph(usize),
}

pub struct Renderer {
    /// Post-first-frame background effect-pipeline warmer: listed first so its Drop drains
    /// any active worker before GPU resources on this renderer drop.
    effect_warm: effect_warm::EffectWarm,
    first_frame_submitted: std::sync::atomic::AtomicBool,
    surfaces: Option<crate::native_surface::SurfaceRenderer>,
    /// Per-device native surface texture importer for compositors (`surface_texture`), adopted
    /// into `surfaces` if a view also draws `El::native_surface`.
    surface_importer: Option<crate::native_surface::SurfaceImporter>,
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
    fade_stops: [f32; 2],
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
    /// Health of the primary uber shader and pipeline (`shader_check`), tracking async WebGPU
    /// error scopes on the web.
    uber_check: shader_check::UberCheck,
    uber_check_seq: crate::machine::Seq,
    #[cfg(target_arch = "wasm32")]
    uber_check_wake: shader_check::WakeSlot,
    uploads: upload::Uploads,
    /// Atlas fills `prepare` staged, recorded into the frame's encoder with the uploads.
    fills: std::sync::Mutex<Option<fill::PendingFills>>,
    /// Unique to this renderer, so to the device it was made on (`Frame::device_key`).
    device_key: crate::compose::DeviceKey,
    /// Host GPU instance, adapter, and callback state attached once per renderer setup.
    gpu_host: Option<gpu_loss::GpuHost>,
    /// The device's largest 2D texture, read once: in a browser each `Device::limits` reads every
    /// limit from JavaScript (about 35 µs), which a frame's layers used to pay.
    max_texture_dim: u32,
    /// The one instance per rectangle that clears a damaged one (`arm_damage`), made on first use.
    clear_quad: std::sync::OnceLock<wgpu::Buffer>,
    /// The kept frame and the rectangles the next `draw` may redraw alone (`damaged`).
    damage: std::sync::Mutex<damaged::DamageState>,
}

impl Renderer {
    /// `Frame::device_key`: a renderer belongs to one device, and a new device gets a new one.
    pub fn device_key(&self) -> crate::compose::DeviceKey {
        self.device_key
    }

    /// Creates a renderer for `device` drawing into `format`, panicking if the GPU rejects
    /// Rune's last-resort uber shader or pipeline. Windows and headless runs use [`Self::try_new`].
    ///
    /// Blending defaults to sRGB-encoded (`srgb_blend() == true`) for 8-bit non-sRGB targets
    /// (`Rgba8Unorm` / [`OFFSCREEN_FORMAT`] and `Bgra8Unorm`), and to linear light
    /// (`srgb_blend() == false`) for `*Srgb` (`Rgba8UnormSrgb` / [`OFFSCREEN_FORMAT_LINEAR`],
    /// `Bgra8UnormSrgb`) and floating-point HDR (`Rgba16Float`) targets. Override with
    /// [`Self::set_srgb_blend`].
    ///
    /// ```no_run
    /// use rune::render::{Gpu, OFFSCREEN_FORMAT, OFFSCREEN_FORMAT_LINEAR, Renderer};
    ///
    /// let gpu = Gpu::headless();
    /// let srgb = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    /// assert!(srgb.srgb_blend());
    /// let linear = Renderer::new(&gpu.device, OFFSCREEN_FORMAT_LINEAR);
    /// assert!(!linear.srgb_blend());
    /// ```
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Renderer {
        Self::try_new(device, format).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Creates a renderer for `device` drawing into `format`, returning an error instead of
    /// leaving an invalid pipeline if the GPU rejects Rune's last-resort uber shader or pipeline.
    ///
    /// Blending follows the same format default as [`Self::new`]: sRGB-encoded for `Rgba8Unorm`
    /// and `Bgra8Unorm`, linear light for `*Srgb` and floating-point targets.
    ///
    /// On native devices and WebGL2 the shader and pipeline are checked synchronously inside
    /// validation, out-of-memory and internal error scopes. On browser WebGPU, error scopes
    /// resolve asynchronously and are checked by the web host without blocking startup.
    pub fn try_new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Result<Renderer, String> {
        Self::try_new_classified(device, format).map_err(Into::into)
    }

    pub(crate) fn try_new_classified(device: &wgpu::Device, format: wgpu::TextureFormat) -> Result<Renderer, ShaderError> {
        Self::try_create(device, format, None, cfg!(target_os = "macos"))
    }

    /// Overrides whether colours are sRGB-encoded for blending (`true` by default for
    /// [`OFFSCREEN_FORMAT`] / `Rgba8Unorm` and `Bgra8Unorm`; `false` by default for
    /// [`OFFSCREEN_FORMAT_LINEAR`] / `*Srgb` and `Rgba16Float`). Also updates the renderer's
    /// native surface pipeline when present.
    ///
    /// ```no_run
    /// use rune::render::{Gpu, OFFSCREEN_FORMAT_LINEAR, Renderer};
    ///
    /// let gpu = Gpu::headless();
    /// let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT_LINEAR);
    /// renderer.set_srgb_blend(false);
    /// assert!(!renderer.srgb_blend());
    /// ```
    pub fn set_srgb_blend(&mut self, enabled: bool) {
        self.srgb_blend = enabled;
        if let Some(s) = &mut self.surfaces {
            s.set_srgb_blend(enabled);
        }
    }

    pub fn set_fade_stops(&mut self, stops: [f32; 2]) {
        self.fade_stops = stops;
    }

    pub fn fade_stops(&self) -> [f32; 2] {
        self.fade_stops
    }

    /// Opt-in retained geometry. IDs map instances to shared translation/opacity nodes.
    /// Upload geometry once with `prepare`, then use `present_at` without rebuilding instances.
    /// Panics if the GPU rejects the spring uber shader or pipeline; use [`Self::try_with_springs`]
    /// to handle that failure.
    pub fn with_springs(device: &wgpu::Device, format: wgpu::TextureFormat, nodes: &[crate::gpu::springs::Node], ids: &[u32]) -> Renderer {
        Self::try_with_springs(device, format, nodes, ids).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Like [`Self::with_springs`], returning an error if the GPU rejects the shader or pipeline.
    pub fn try_with_springs(device: &wgpu::Device, format: wgpu::TextureFormat, nodes: &[crate::gpu::springs::Node], ids: &[u32]) -> Result<Renderer, String> {
        Self::try_create(device, format, Some((nodes, ids)), true).map_err(Into::into)
    }

    /// A renderer that starts with the Classic uber shader, as on macOS, and builds Windfoil when
    /// text first draws from outlines (render/variant.rs); for checking that path elsewhere.
    #[doc(hidden)]
    pub fn new_classic(device: &wgpu::Device, format: wgpu::TextureFormat) -> Renderer {
        Self::try_new_classic(device, format).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Like [`Self::new_classic`], returning an error if the GPU rejects the Classic shader or
    /// pipeline.
    #[doc(hidden)]
    pub fn try_new_classic(device: &wgpu::Device, format: wgpu::TextureFormat) -> Result<Renderer, String> {
        Self::try_create(device, format, None, true).map_err(Into::into)
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
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
        pass::apply_subrect_viewport(&mut pass, view, self.target_frame(None).1);
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
                format, origin, size, in_layer: target.is_some(), srgb_blend: self.srgb_blend, backdrop, fade_stops: self.fade_stops });
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
        let index = if let Some(s) = &self.surfaces {
            s.submit(queue, encoder)
        } else if let Some(importer) = &self.surface_importer {
            importer.submit(queue, encoder)
        } else {
            queue.submit([encoder.finish()])
        };
        self.first_frame_submitted.store(true, std::sync::atomic::Ordering::Release);
        index
    }

    /// Whether `submit` is `queue.submit` after `before_submit` and nothing more: no native video
    /// surface waits on the frame's submission, so another thread may make it.
    #[cfg(target_os = "macos")]
    pub(crate) fn submits_plainly(&self) -> bool {
        self.surfaces.as_ref().is_none_or(|s| s.draws_nothing())
            && self.surface_importer.as_ref().is_none_or(|i| !i.has_pending_submit())
    }

    /// Advances native-surface importer frame generation when a plain macOS overlay submission
    /// bypasses [`Self::submit`].
    #[cfg(target_os = "macos")]
    pub(crate) fn note_plain_submit(&self) {
        if let Some(s) = &self.surfaces {
            s.note_frame_submitted();
        } else if let Some(importer) = &self.surface_importer {
            importer.note_frame_submitted();
        }
    }

    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: [f32; 4]) {
        if self.armed() && self.draw_armed(encoder, view) { return; }
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
        pass::apply_subrect_viewport(&mut pass, view, self.target_frame(None).1);
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
/// radius draws Apple's continuous corner, whose curve starts `CONTINUOUS_K` times the radius from
/// the corner (`CONTINUOUS_K` in clip.wgsl).
pub(crate) fn corner_extent(radius: f32) -> f32 {
    if radius < 0.0 { -radius * CONTINUOUS_K } else { radius }
}

pub(crate) const CONTINUOUS_K: f32 = instance::shader_consts::CONTINUOUS_K;

/// Frame-profile builds: `RUNE_PINNED` forcing the split on (`all`) or off (`off`), and the instances drawn
/// unsplit and split since the last report (`report_pinned`).
#[cfg(feature = "frame-profile")]
pub(super) fn pinned_forced() -> Option<bool> {
    static FORCED: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| match rune_core::env::var("RUNE_PINNED").as_deref() { Some("all") => Some(true), Some("off") => Some(false), _ => None })
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
    let mut at = span.start as usize;
    let end = span.end as usize;
    let slice = &kinds[..end.min(kinds.len())];
    std::iter::from_fn(move || {
        if at >= end { return None; }
        let start = at;
        let Some(&first) = slice.get(at) else {
            at = end;
            return Some(start as u32..end as u32);
        };
        let mut cur = at + 1;
        while let Some(&k) = slice.get(cur) && k == first { cur += 1; }
        at = cur;
        Some(start as u32..cur as u32)
    })
}
