//! Making a GPU device ahead of a surface (`warm_gpu`), so a first surface waits less for its
//! device.

use super::{
    BatchBind, Instance, Kind, Renderer, backdrop_blur, breaker, dxbc, effect_warm, fill, layers, pass::Backdrop,
    shader_check, upload, variant,
};

#[cfg(any(target_os = "windows", target_arch = "wasm32"))]
use super::damaged;

/// The `Send` state of a [`Renderer`]: every field except `surfaces` (the lazy native surface
/// renderer, which holds `!Send` platform handles such as macOS's `CVMetalTextureCache`).
/// Prewarm and warm-up threads construct and transfer a `WarmRenderer` directly so `Send` is
/// proved by the compiler without `unsafe`, then attach it via [`Renderer::from_warm`].
pub(super) struct WarmRenderer {
    pub(super) effect_warm: effect_warm::EffectWarm,
    pub(super) first_frame_submitted: std::sync::atomic::AtomicBool,
    pub(super) surfaces_building: Option<crate::native_surface::Building>,
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) format: wgpu::TextureFormat,
    pub(super) shader: dxbc::Shaders,
    pub(super) layout: wgpu::PipelineLayout,
    pub(super) vertex_entry: &'static str,
    pub(super) extra: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) bind_layout: wgpu::BindGroupLayout,
    pub(super) globals: wgpu::Buffer,
    pub(super) atlas: wgpu::Texture,
    pub(super) uber: variant::Uber,
    pub(super) lcd: variant::Lcd,
    pub(super) pinned: Option<Vec<Option<wgpu::RenderPipeline>>>,
    pub(super) pinned_extra: Vec<(wgpu::TextureFormat, Vec<Option<wgpu::RenderPipeline>>)>,
    pub(super) pinned_building: Option<(wgpu::TextureFormat, std::sync::mpsc::Receiver<Vec<Option<wgpu::RenderPipeline>>>)>,
    pub(super) kinds: Vec<Option<Kind>>,
    pub(super) lcd_frame: bool,
    pub(super) frame_wake: Option<std::sync::Arc<crate::live::Wake>>,
    pub(super) stand_in: wgpu::Texture,
    pub(super) atlas_pages: Vec<(wgpu::Texture, wgpu::BindGroup)>,
    pub(super) images: Vec<Option<(wgpu::Texture, wgpu::BindGroup)>>,
    pub(super) image_batches: Vec<(std::ops::Range<u32>, BatchBind)>,
    pub(super) instances: rune_staging::GrowBuffer,
    pub(super) count: u32,
    pub(super) springs: Option<crate::gpu::springs::Bindings>,
    pub(super) spring_instances: usize,
    pub(super) layers: layers::Layers,
    pub(super) srgb_blend: bool,
    pub(super) effects: Vec<(u32, Instance)>,
    pub(super) backdrops: Vec<Backdrop>,
    pub(super) backdrop_used: bool,
    pub(super) backdrop_blur: Option<backdrop_blur::BackdropBlur>,
    pub(super) breakers: breaker::Breakers,
    pub(super) uber_check: shader_check::UberCheck,
    pub(super) uber_check_seq: crate::machine::Seq,
    #[cfg(target_arch = "wasm32")]
    pub(super) uber_check_wake: shader_check::WakeSlot,
    pub(super) uploads: upload::Uploads,
    pub(super) fills: std::sync::Mutex<Option<fill::PendingFills>>,
    pub(super) device_key: crate::compose::DeviceKey,
    pub(super) max_texture_dim: u32,
    #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
    pub(super) clear_quad: std::sync::OnceLock<wgpu::Buffer>,
    #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
    pub(super) damage: std::sync::Mutex<damaged::DamageState>,
}

#[cfg(not(target_arch = "wasm32"))]
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<super::Gpu>();
    assert_send::<WarmRenderer>();
};

impl Renderer {
    /// Attaches a `WarmRenderer` as a `Renderer` with no native surface renderer.
    pub(super) fn from_warm(warm: WarmRenderer) -> Renderer {
        let WarmRenderer {
            effect_warm,
            first_frame_submitted,
            surfaces_building,
            pipeline,
            format,
            shader,
            layout,
            vertex_entry,
            extra,
            bind_group,
            bind_layout,
            globals,
            atlas,
            uber,
            lcd,
            pinned,
            pinned_extra,
            pinned_building,
            kinds,
            lcd_frame,
            frame_wake,
            stand_in,
            atlas_pages,
            images,
            image_batches,
            instances,
            count,
            springs,
            spring_instances,
            layers,
            srgb_blend,
            effects,
            backdrops,
            backdrop_used,
            backdrop_blur,
            breakers,
            uber_check,
            uber_check_seq,
            #[cfg(target_arch = "wasm32")]
            uber_check_wake,
            uploads,
            fills,
            device_key,
            max_texture_dim,
            #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
            clear_quad,
            #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
            damage,
        } = warm;
        Renderer {
            effect_warm,
            first_frame_submitted,
            surfaces: None,
            fade_stops: [0.0; 2],
            surfaces_building,
            pipeline,
            format,
            shader,
            layout,
            vertex_entry,
            extra,
            bind_group,
            bind_layout,
            globals,
            atlas,
            uber,
            lcd,
            pinned,
            pinned_extra,
            pinned_building,
            kinds,
            lcd_frame,
            frame_wake,
            stand_in,
            atlas_pages,
            images,
            image_batches,
            instances,
            count,
            springs,
            spring_instances,
            layers,
            srgb_blend,
            effects,
            backdrops,
            backdrop_used,
            backdrop_blur,
            breakers,
            uber_check,
            uber_check_seq,
            #[cfg(target_arch = "wasm32")]
            uber_check_wake,
            uploads,
            fills,
            device_key,
            gpu_host: None,
            max_texture_dim,
            #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
            clear_quad,
            #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
            damage,
        }
    }

    /// Drops surface-specific textures and scratch buffers before parking this renderer for a
    /// future surface (`warm::park`), keeping compiled pipelines and the glyph atlas.
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    pub(super) fn clear_for_park(&mut self) {
        self.clear_images();
        self.layers.clear_all();
        self.backdrops.clear();
        if let Some(blur) = &mut self.backdrop_blur { blur.give_back(); }
        self.uploads.forget_instances();
        self.uploads.trim();
        self.image_batches.clear();
        self.count = 0;
        self.effects.clear();
        #[cfg(target_os = "windows")]
        { *self.damage.get_mut().unwrap_or_else(|e| e.into_inner()) = Default::default(); }
        #[cfg(all(feature = "alloc-sites", not(target_arch = "wasm32")))]
        self.report_pools();
    }

    /// Extracts the `Send` core when no native surface renderer has been created; drops `self` on
    /// the calling thread and returns `None` otherwise.
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    pub(super) fn into_warm(self) -> Option<WarmRenderer> {
        if self.surfaces.is_some() {
            return None;
        }
        let Renderer {
            effect_warm,
            first_frame_submitted,
            surfaces: _,
            fade_stops: _,
            surfaces_building,
            pipeline,
            format,
            shader,
            layout,
            vertex_entry,
            extra,
            bind_group,
            bind_layout,
            globals,
            atlas,
            uber,
            lcd,
            pinned,
            pinned_extra,
            pinned_building,
            kinds,
            lcd_frame,
            frame_wake,
            stand_in,
            atlas_pages,
            images,
            image_batches,
            instances,
            count,
            springs,
            spring_instances,
            layers,
            srgb_blend,
            effects,
            backdrops,
            backdrop_used,
            backdrop_blur,
            breakers,
            uber_check,
            uber_check_seq,
            #[cfg(target_arch = "wasm32")]
            uber_check_wake,
            uploads,
            fills,
            device_key,
            gpu_host: _,
            max_texture_dim,
            #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
            clear_quad,
            #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
            damage,
        } = self;
        Some(WarmRenderer {
            effect_warm,
            first_frame_submitted,
            surfaces_building,
            pipeline,
            format,
            shader,
            layout,
            vertex_entry,
            extra,
            bind_group,
            bind_layout,
            globals,
            atlas,
            uber,
            lcd,
            pinned,
            pinned_extra,
            pinned_building,
            kinds,
            lcd_frame,
            frame_wake,
            stand_in,
            atlas_pages,
            images,
            image_batches,
            instances,
            count,
            springs,
            spring_instances,
            layers,
            srgb_blend,
            effects,
            backdrops,
            backdrop_used,
            backdrop_blur,
            breakers,
            uber_check,
            uber_check_seq,
            #[cfg(target_arch = "wasm32")]
            uber_check_wake,
            uploads,
            fills,
            device_key,
            max_texture_dim,
            #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
            clear_quad,
            #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
            damage,
        })
    }
}

/// A window's or overlay's surface draws with `gpu`, which `warm_gpu` then leaves alone.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn surface_gpu(gpu: &std::sync::Arc<super::Gpu>) {
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    super::prewarm::surface_gpu(gpu);
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    let _ = gpu;
}

/// A hidden overlay's device and renderer, its surface dropped, for the next window's or
/// overlay's surface to take (`warm::park`). macOS drops them: its prewarmed panels keep theirs.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn take_parked_gpu_for_test() -> Option<(super::Gpu, super::Renderer)> {
    #[cfg(not(target_os = "macos"))]
    { super::prewarm::take() }
    #[cfg(target_os = "macos")]
    { None }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn park_gpu(gpu: super::Gpu, renderer: super::Renderer) {
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    super::prewarm::park(gpu, renderer, super::prewarm::PARKED);
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    drop((renderer, gpu));
}

/// Starts making, on a thread, the GPU device and Rune's renderer the next window or overlay
/// surface would make, so a show soon after (a tray app's HUD, on its shortcut's first key)
/// doesn't wait for them on the UI thread. That surface takes them, waiting for the thread if
/// it's still working; unclaimed, they go half a minute after the last call. Does nothing while a
/// surface's device lives. `features` are the ones that surface's compositor asks for
/// (`Compositor::features`); a device without them isn't taken. Windows and Linux; on macOS
/// use `prewarm_gpu`. On Windows, the first call in a process also makes the DX12-or-Vulkan
/// choice on its thread. Cheap to call often, from any thread.
pub fn warm_gpu(features: wgpu::Features) {
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    super::prewarm::warm(features, None);
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    let _ = features;
}

/// `warm_gpu`, then `prepare` on that thread with the device, new or the one a surface draws
/// with: an app makes its own pipelines there, off the UI thread, for its compositor to find.
/// On Windows, its first call in a process also makes the DX12-or-Vulkan choice on that thread.
pub fn warm_gpu_with(features: wgpu::Features, prepare: impl FnOnce(&wgpu::Device) + Send + 'static) {
    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    super::prewarm::warm(features, Some(Box::new(prepare)));
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    let _ = (features, prepare);
}

/// App-attributed form of [`warm_gpu_with`] used by [`crate::AppHandle::warm_gpu_with`].
#[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
pub(crate) fn warm_gpu_with_owner(
    features: wgpu::Features,
    owner: Option<std::sync::Arc<super::loss_latch::GpuLossLatch>>,
    prepare: Box<dyn FnOnce(&wgpu::Device) + Send>,
) {
    super::prewarm::warm_with_owner(features, owner, Some(prepare));
}

/// `warm_gpu` for a window that opened on the CPU: on a thread, once the device and renderer
/// wait for the window's surface, `frame` draws with them, then `done` runs; with no device,
/// only `done` (`window::linux::handover`).
#[cfg(target_os = "linux")]
pub(crate) fn warm_gpu_then(features: wgpu::Features, frame: impl FnOnce(&super::Gpu, &mut Renderer) + Send + 'static, done: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    super::prewarm::warm_then(features, frame, done)
}

/// Whether `warm_gpu_then` left a device for the window's surface (or another surface's lives).
#[cfg(target_os = "linux")]
pub(crate) fn warm_gpu_made() -> bool {
    super::prewarm::made()
}

/// Makes what a window's GPU frames start from (the device, the renderer's pipelines for a
/// window's swap chain, the pipelines pinned to one kind each) in a process that shows nothing,
/// and returns once the driver has compiled them. Direct3D keeps what the driver compiled, and
/// its own shader-cache databases, per user and per exe path: the next process of the same exe
/// finds them. So a setup can run the installed program once in a mode that calls this and
/// exits, and the person's first launch takes its window over to the GPU as a later launch
/// does, about 0.1 s after its first frame, where a launch that has to make the databases and
/// compile the pipelines takes 1 to 2 s. It draws nothing and opens no window anyone sees.
/// Windows only: elsewhere it does nothing and returns `Ok`.
pub fn warm_gpu_cache() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    return crate::window::warm_gpu_cache();
    #[cfg(not(target_os = "windows"))]
    Ok(())
}