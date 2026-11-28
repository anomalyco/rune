//! Embedded GPU surface host for native views that own their own window and frame loop:
//! `CAMetalLayer` on Apple (UIKit / SwiftUI / AppKit driven by `CADisplayLink`) and
//! `ANativeWindow` on Android (`SurfaceView` driven by `Choreographer`).

#![allow(unsafe_code)]

use std::ffi::c_void;
#[cfg(target_os = "android")]
use std::ptr::NonNull;
#[cfg(target_vendor = "apple")]
use std::sync::OnceLock;
#[cfg(target_os = "android")]
use std::sync::Mutex;
use std::sync::Arc;

use crate::compose::{Compositor, Frame};
use crate::frame::clock::FrameClock;
use crate::frame::host_step::{self, Acquire};
use crate::platform::embed_frame::{self, EmbedSurface, EmbedSurfaceEvent, EmbedSurfaceHealth, FrameSummary};
use crate::render::{Gpu, Renderer};
use crate::runtime::{AppDriver, Env, Runtime, RuntimeCore, Waker};
use crate::time::Instant;
use crate::tooling::AppSpec;
#[cfg(target_vendor = "apple")]
use crate::tooling::runtime_for;
#[cfg(target_os = "android")]
use crate::tooling::runtime_for_configured;
use crate::window::Missed;

#[cfg(target_vendor = "apple")]
static SHARED_GPU: OnceLock<Result<Arc<Gpu>, String>> = OnceLock::new();

#[cfg(target_vendor = "apple")]
fn shared_gpu() -> Result<Arc<Gpu>, String> {
    SHARED_GPU
        .get_or_init(|| Gpu::try_for_display(Gpu::instance(), wgpu::Features::empty()).map(Arc::new))
        .clone()
}

#[cfg(target_os = "android")]
static ANDROID_SHARED_GPU: Mutex<Option<Arc<Gpu>>> = Mutex::new(None);

#[cfg(target_os = "android")]
unsafe fn create_android_surface(
    instance: &wgpu::Instance,
    native_window: *mut c_void,
) -> Result<wgpu::Surface<'static>, String> {
    use winit::raw_window_handle::{
        AndroidDisplayHandle, AndroidNdkWindowHandle, RawDisplayHandle, RawWindowHandle,
    };
    let window = NonNull::new(native_window)
        .ok_or_else(|| "null ANativeWindow pointer".to_owned())?;
    let target = wgpu::SurfaceTargetUnsafe::RawHandle {
        raw_display_handle: Some(RawDisplayHandle::Android(AndroidDisplayHandle::new())),
        raw_window_handle: RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(window)),
    };
    // SAFETY: caller guarantees `native_window` is a valid `ANativeWindow*` that outlives
    // the returned surface until `LayerHost::detach_surface` or `LayerHost` drop.
    unsafe {
        instance
            .create_surface_unsafe(target)
            .map_err(|e| format!("create_surface_unsafe(AndroidNdk): {e}"))
    }
}

#[cfg(target_os = "android")]
unsafe fn android_gpu_and_surface(
    native_window: *mut c_void,
) -> Result<(Arc<Gpu>, wgpu::Surface<'static>), String> {
    let mut slot = ANDROID_SHARED_GPU
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(gpu) = slot.as_ref() {
        // SAFETY: caller guarantees `native_window` is a valid `ANativeWindow*`.
        let surface = unsafe { create_android_surface(&gpu.instance, native_window) }?;
        return Ok((Arc::clone(gpu), surface));
    }
    let instance = Gpu::instance();
    // SAFETY: caller guarantees `native_window` is a valid `ANativeWindow*`.
    let surface = unsafe { create_android_surface(&instance, native_window) }?;
    let gpu = Arc::new(Gpu::try_with_features(
        instance,
        Some(&surface),
        wgpu::Features::empty(),
    )?);
    *slot = Some(Arc::clone(&gpu));
    Ok((gpu, surface))
}

/// Warms the shared Metal device and compiles the default `Bgra8Unorm` uber pipeline on a
/// background thread so the first embedded `LayerHost` opens without shader-compile latency.
#[cfg(target_vendor = "apple")]
pub fn prewarm() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        std::thread::Builder::new().name("rune-embed-prewarm".into()).spawn(|| {
            if let Ok(gpu) = shared_gpu() {
                Renderer::try_new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm).ok();
            }
        }).ok();
    });
}

/// Outcome of one [`LayerHost::frame`] pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbedFrame {
    /// Whether a drawable was acquired and presented.
    pub drew: bool,
    /// True while springs, reveals, pending input or live bindings need another display-link tick.
    pub animating: bool,
    /// Seconds until the next scheduled timer wake (`None` when none is pending).
    pub wake_after_secs: Option<f64>,
    /// CPU time spent stepping layout/motion and encoding GPU work, in microseconds.
    pub cpu_us: u64,
}

const _: fn() = || {
    fn assert_copy<T: Copy>() {}
    assert_copy::<EmbedFrame>();
};

impl From<FrameSummary> for EmbedFrame {
    fn from(s: FrameSummary) -> Self {
        Self {
            drew: s.drew,
            animating: s.animating,
            wake_after_secs: s.wake_after_secs,
            cpu_us: s.cpu_us,
        }
    }
}

pub(crate) struct PreparedEmbedFrame {
    frame: wgpu::SurfaceTexture,
    target: wgpu::TextureView,
    encoder: wgpu::CommandEncoder,
    size: (u32, u32),
    format: wgpu::TextureFormat,
}

struct AttachedSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

struct LayerHostCore {
    gpu: Arc<Gpu>,
    surface: Option<AttachedSurface>,
    renderer: Renderer,
    clock: FrameClock,
    suspended: bool,
    surface_health: EmbedSurfaceHealth,
}

impl LayerHostCore {
    fn from_gpu_and_surface(
        gpu: Arc<Gpu>,
        surface: wgpu::Surface<'static>,
        viewport: (f32, f32),
        scale: f32,
    ) -> Result<(Self, (f32, f32), f32, Instant), String> {
        let scale = scale.max(1.0);
        let max_dim = gpu.device.limits().max_texture_dimension_2d.max(1);
        let px_w = ((viewport.0.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let px_h = ((viewport.1.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let srgb_blend = true;
        let (config, _) = crate::window::surface_config(&surface, &gpu.adapter, (px_w, px_h), srgb_blend, false)
            .ok_or_else(|| "embedded native surface has no supported wgpu surface configuration".to_owned())?;
        crate::window::configure(&surface, &gpu, &config)?;
        let target_format = config.format.remove_srgb_suffix();
        let mut renderer = Renderer::try_new(&gpu.device, target_format)?;
        renderer.attach_gpu(&gpu);
        renderer.set_srgb_blend(true);
        let size = (px_w as f32 / scale, px_h as f32 / scale);
        let now = crate::time::now();
        let core = Self {
            gpu,
            surface: Some(AttachedSurface { surface, config }),
            renderer,
            clock: FrameClock::new(now),
            suspended: viewport.0 <= 0.0 || viewport.1 <= 0.0,
            surface_health: EmbedSurfaceHealth::default(),
        };
        Ok((core, size, scale, now))
    }

    #[cfg(target_vendor = "apple")]
    unsafe fn from_core_animation_layer(
        layer: *mut c_void,
        viewport: (f32, f32),
        scale: f32,
    ) -> Result<(Self, (f32, f32), f32, Instant), String> {
        if layer.is_null() {
            return Err("null CAMetalLayer pointer".into());
        }
        let gpu = shared_gpu()?;
        // SAFETY: caller guarantees non-null `layer` is a valid `CAMetalLayer` outliving this surface on an allowed thread.
        let surface = unsafe {
            gpu.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
                .map_err(|e| format!("create_surface_unsafe: {e}"))?
        };
        Self::from_gpu_and_surface(gpu, surface, viewport, scale)
    }

    #[cfg(target_os = "android")]
    unsafe fn from_native_window(
        native_window: *mut c_void,
        viewport: (f32, f32),
        scale: f32,
    ) -> Result<(Self, (f32, f32), f32, Instant), String> {
        // SAFETY: caller guarantees `native_window` is a valid `ANativeWindow*` outliving this surface until detached.
        let (gpu, surface) = unsafe { android_gpu_and_surface(native_window) }?;
        Self::from_gpu_and_surface(gpu, surface, viewport, scale)

    }

    fn configure_runtime(&mut self, rt: &mut RuntimeCore, now: Instant) {
        rt.set_srgb_blend(true);
        rt.set_os_drag(false);
        rt.set_pointer_lock_supported(false);
        rt.wall_start = Some(now);
        rt.frame.set_record_events(false);
        if let Some(attached) = &self.surface {
            let target_format = if rt.srgb_blend() {
                attached.config.format.remove_srgb_suffix()
            } else {
                attached.config.format
            };
            crate::window::rebuild_renderer_core(&self.gpu.device, &mut self.renderer, target_format, rt);
        }
    }

    fn detach_surface(&mut self) {
        self.surface = None;
        self.suspended = true;
    }

    #[cfg(target_os = "android")]
    unsafe fn attach_native_window(
        &mut self,
        rt: &mut RuntimeCore,
        native_window: *mut c_void,
        viewport: (f32, f32),
        scale: f32,
    ) -> Result<(), String> {
        // Drop any previous swapchain before creating a new surface.
        self.surface = None;
        // SAFETY: caller guarantees `native_window` is a valid `ANativeWindow*` outliving this surface until detached.
        let surface = unsafe { create_android_surface(&self.gpu.instance, native_window) }?;
        let scale = scale.max(1.0);
        let max_dim = self.gpu.device.limits().max_texture_dimension_2d.max(1);
        let px_w = ((viewport.0.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let px_h = ((viewport.1.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let srgb_blend = rt.srgb_blend();
        let (config, _) = crate::window::surface_config(&surface, &self.gpu.adapter, (px_w, px_h), srgb_blend, false)
            .ok_or_else(|| "Android ANativeWindow has no supported wgpu surface configuration".to_owned())?;
        crate::window::configure(&surface, &self.gpu, &config)?;
        let target_format = if srgb_blend {
            config.format.remove_srgb_suffix()
        } else {
            config.format
        };
        crate::window::rebuild_renderer_core(&self.gpu.device, &mut self.renderer, target_format, rt);
        self.surface = Some(AttachedSurface { surface, config });
        self.surface_health.step(EmbedSurfaceEvent::Resized);
        self.suspended = viewport.0 <= 0.0 || viewport.1 <= 0.0;
        rt.set_viewport((px_w as f32 / scale, px_h as f32 / scale), scale);
        Ok(())
    }

    fn resize(&mut self, rt: &mut RuntimeCore, viewport: (f32, f32), scale: f32) {
        if viewport.0 <= 0.0 || viewport.1 <= 0.0 {
            self.suspended = true;
            return;
        }
        let scale = scale.max(1.0);
        let max_dim = self.gpu.device.limits().max_texture_dimension_2d.max(1);
        let px_w = ((viewport.0 * scale).round() as u32).clamp(1, max_dim);
        let px_h = ((viewport.1 * scale).round() as u32).clamp(1, max_dim);
        if let Some(attached) = &mut self.surface {
            self.suspended = false;
            self.surface_health.step(EmbedSurfaceEvent::Resized);
            if attached.config.width != px_w || attached.config.height != px_h {
                attached.config.width = px_w;
                attached.config.height = px_h;
                if let Err(error) = crate::window::configure(&attached.surface, &self.gpu, &attached.config) {
                    eprintln!("rune: embedded layer surface refused its new size: {error}");
                }
            }
        }
        rt.set_viewport((px_w as f32 / scale, px_h as f32 / scale), scale);
    }

    /// Acquires the next drawable, reconfiguring and retrying once on `Outdated`/`Lost`.
    fn acquire(&mut self) -> Acquire<wgpu::SurfaceTexture> {
        let Some(attached) = &self.surface else {
            return Acquire::Missed(Missed::Unconfigured);
        };
        let mut retries = 1u8;
        loop {
            match attached.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => return Acquire::Ready(f),
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    if let Err(error) = crate::window::configure(&attached.surface, &self.gpu, &attached.config) {
                        eprintln!("rune: embedded layer surface refused reconfiguration: {error}");
                        return Acquire::Missed(Missed::Unconfigured);
                    }
                    if retries == 0 { return Acquire::Missed(Missed::Outdated); }
                    retries -= 1;
                }
                _ => return Acquire::Missed(Missed::Timeout),
            }
        }
    }

    fn reblend(&mut self, rt: &mut RuntimeCore) -> bool {
        let Some(attached) = &mut self.surface else {
            return false;
        };
        let mut alpha = None;
        crate::window::reblend_core(&attached.surface, &self.gpu, &mut attached.config, &mut self.renderer, &mut alpha, rt)
    }

    fn prepare_draw(&mut self, rt: &mut RuntimeCore, frame: wgpu::SurfaceTexture) -> PreparedEmbedFrame {
        let attached = self.surface.as_ref().expect("surface attached when drawable was acquired");
        let target_format = if rt.srgb_blend() {
            attached.config.format.remove_srgb_suffix()
        } else {
            attached.config.format
        };
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: (target_format != attached.config.format).then_some(target_format),
            ..Default::default()
        });
        let size = (attached.config.width, attached.config.height);
        host_step::prepare_gpu_resources(&mut self.renderer, &self.gpu, rt, size, true);
        let encoder = self.gpu.device.create_command_encoder(&Default::default());
        PreparedEmbedFrame { frame, target, encoder, size, format: target_format }
    }

    fn draw_default(&mut self, rt: &mut RuntimeCore, prepared: &mut PreparedEmbedFrame) {
        let clear = rt.clear_color();
        host_step::draw_ui(&mut self.renderer, &self.gpu, &mut prepared.encoder, &prepared.target, rt, prepared.size, clear);
    }

    fn submit_and_present(&mut self, prepared: PreparedEmbedFrame) -> Instant {
        self.renderer.submit(&self.gpu.queue, prepared.encoder);
        self.gpu.queue.present(prepared.frame);
        crate::render::gpu_events::presented();
        if !self.gpu.is_lost() {
            self.renderer.warm_effects(&self.gpu.device);
        }
        crate::time::now()
    }
}

/// Drives one Rune [`Runtime`] and [`Renderer`] directly onto an embedded native surface
/// (`CAMetalLayer` on Apple, `ANativeWindow` on Android).
///
/// Pause frame ticks while the host app is backgrounded or after [`Self::detach_surface`]:
/// Metal forbids drawable acquisition after `didEnterBackground`, and Android destroys the
/// underlying `ANativeWindow` across `surfaceDestroyed` / `surfaceCreated`.
pub struct LayerHost<S: 'static> {
    core: LayerHostCore,
    compositor: Option<Box<dyn Compositor<S>>>,
    rt: Runtime<S>,
}

impl<S: 'static> LayerHost<S> {
    /// Attaches a new Rune runtime and Metal surface to `layer` (`CAMetalLayer *`), blending in
    /// sRGB space (`srgb_blend = true`, matching Apple's native text compositing).
    ///
    /// # Safety
    /// `layer` must be a valid `CAMetalLayer` pointer that outlives this `LayerHost` (until
    /// [`Self::detach_surface`] or drop), and surface creation / presentation must run on a
    /// thread allowed to touch that layer (the UI thread).
    #[cfg(target_vendor = "apple")]
    pub unsafe fn from_core_animation_layer(
        layer: *mut c_void,
        spec: &AppSpec<S>,
        state: S,
        viewport: (f32, f32),
        scale: f32,
        dark: bool,
        wake: Waker,
    ) -> Result<Self, String> {
        // SAFETY: caller upholds `from_core_animation_layer`'s safety contract.
        let (mut core, size, scale, now) = unsafe { LayerHostCore::from_core_animation_layer(layer, viewport, scale) }?;
        #[cfg(target_os = "ios")]
        let text_scale_wake = wake.clone();
        let mut rt = runtime_for(spec, state, size, Env::new(dark, scale), wake);
        #[cfg(target_os = "ios")]
        {
            rt.set_touch_controls(true);
            rt.set_system_text_scale(crate::platform::text_scale::current());
            crate::platform::text_scale::follow_waker(text_scale_wake);
        }
        core.configure_runtime(rt.core_mut(), now);
        Ok(Self { core, compositor: None, rt })
    }

    /// Attaches a new Rune runtime and GPU surface to an Android `ANativeWindow *`.
    ///
    /// # Safety
    /// `native_window` must be a valid `ANativeWindow` pointer that outlives the attached
    /// surface (call [`Self::detach_surface`] before `ANativeWindow_release` or returning from
    /// `SurfaceHolder.Callback.surfaceDestroyed`).
    #[cfg(target_os = "android")]
    pub unsafe fn from_native_window(
        native_window: *mut c_void,
        spec: &AppSpec<S>,
        state: S,
        viewport: (f32, f32),
        scale: f32,
        dark: bool,
        wake: Waker,
    ) -> Result<Self, String> {
        // SAFETY: caller upholds `from_native_window`'s safety contract.
        unsafe { Self::from_native_window_configured(native_window, spec, state, viewport, scale, dark, wake, None) }
    }

    #[cfg(target_os = "android")]
    pub(crate) unsafe fn from_native_window_configured(
        native_window: *mut c_void,
        spec: &AppSpec<S>,
        state: S,
        viewport: (f32, f32),
        scale: f32,
        dark: bool,
        wake: Waker,
        config: Option<crate::app::RuntimeConfig<S>>,
    ) -> Result<Self, String> {
        // SAFETY: caller guarantees `native_window` is a valid `ANativeWindow*` until detached.
        let (mut core, size, scale, now) = unsafe { LayerHostCore::from_native_window(native_window, viewport, scale) }?;
        let mut rt = runtime_for_configured(spec, state, size, Env::new(dark, scale), wake, config);
        rt.set_touch_controls(true);
        rt.core_mut().host.open_urls_natively = true;
        core.configure_runtime(rt.core_mut(), now);
        Ok(Self { core, compositor: None, rt })
    }

    /// Detaches and drops the active GPU swapchain surface while keeping the Rune [`Runtime`],
    /// app state, GPU device, and renderer alive. Call this before the host destroys its native
    /// surface (for example, inside Android's `surfaceDestroyed` before `ANativeWindow_release`).
    pub fn detach_surface(&mut self) {
        self.core.detach_surface();
    }

    /// Whether a native GPU surface is currently attached.
    pub fn has_surface(&self) -> bool {
        self.core.surface.is_some()
    }

    /// Reattaches a fresh Android `ANativeWindow *` after [`Self::detach_surface`], keeping
    /// the existing [`Runtime`] and app state intact.
    ///
    /// # Safety
    /// `native_window` must be a valid `ANativeWindow` pointer that outlives the attached
    /// surface until the next [`Self::detach_surface`] or drop.
    #[cfg(target_os = "android")]
    pub unsafe fn attach_native_window(
        &mut self,
        native_window: *mut c_void,
        viewport: (f32, f32),
        scale: f32,
    ) -> Result<(), String> {
        // SAFETY: caller upholds `attach_native_window`'s safety contract.
        unsafe { self.core.attach_native_window(self.rt.core_mut(), native_window, viewport, scale) }
    }

    /// Attaches a custom [`Compositor`] (e.g. for [`El::effect`](crate::El::effect) shader passes)
    /// and turns on `Runtime::track_marks` so `rt.marks` aligns with `rt.instances`.
    pub fn with_compositor(mut self, compositor: impl Compositor<S>) -> Self {
        self.rt.track_marks = true;
        self.compositor = Some(Box::new(compositor));
        self
    }

    #[cfg(target_os = "android")]
    pub(crate) fn with_boxed_compositor(mut self, compositor: Box<dyn Compositor<S>>) -> Self {
        self.rt.track_marks = true;
        self.compositor = Some(compositor);
        self
    }

    pub fn runtime(&self) -> &Runtime<S> {
        &self.rt
    }

    pub fn runtime_mut(&mut self) -> &mut Runtime<S> {
        &mut self.rt
    }

    /// Takes the haptics handlers asked for (`Tx::haptic`), oldest first, for the host to play:
    /// Rune does not link UIKit or call platform feedback generators from `LayerHost` because
    /// iOS 17.5+ `UIFeedbackGenerator(view:)` and Android `View.performHapticFeedback` require
    /// the host's native view on the main thread. Drain after each [`Self::frame`] (and after
    /// out-of-frame `Runtime::update` calls if desired). Identical haptics within 30 ms on the
    /// runtime clock are coalesced and at most 32 are retained (oldest dropped first).
    pub fn take_haptics(&mut self) -> Vec<crate::Haptic> {
        self.rt.take_haptics()
    }

    /// Updates the viewport in points and display scale, reconfiguring the swapchain only when
    /// a surface is attached and its clamped device-pixel dimensions change.
    pub fn resize(&mut self, viewport: (f32, f32), scale: f32) {
        self.core.resize(self.rt.core_mut(), viewport, scale);
    }

    /// Routes an Android Back press through the runtime as `Key::Escape` synchronously without
    /// acquiring a GPU drawable, returning whether Rune consumed the key.
    #[cfg(target_os = "android")]
    pub(crate) fn dispatch_back_escape(&mut self) -> bool {
        let was_animating =
            self.rt.core().animating || self.compositor.as_ref().is_some_and(|c| c.animating());
        let (rt, mut app) = self.rt.split_mut();
        let consumed = crate::platform::android::machine::dispatch_back_escape_core(rt, &mut app);
        embed_frame::flush_requests(rt, &mut app);
        if !was_animating && consumed {
            self.core.clock.hold(crate::time::now());
        }
        consumed
    }

    /// Steps animations and layout to `Instant::now()` and presents one frame, following the
    /// shared [`crate::frame::host_step`] order. Returns `drew: false` when suspended or detached.
    pub fn frame(&mut self) -> EmbedFrame {
        #[cfg(target_os = "ios")]
        self.rt.set_system_text_scale(crate::platform::text_scale::cached());
        embed_frame::run_embed_frame(self).into()
    }
}

impl<S: 'static> EmbedSurface for LayerHost<S> {
    type Drawable = wgpu::SurfaceTexture;
    type Prepared = PreparedEmbedFrame;

    fn suspended(&self) -> bool { self.core.suspended || self.core.surface.is_none() }
    fn surface_health_mut(&mut self) -> &mut EmbedSurfaceHealth { &mut self.core.surface_health }
    fn renderer_srgb_blend(&self) -> bool { self.core.renderer.srgb_blend() }
    fn rt_srgb_blend(&self) -> bool { self.rt.core().srgb_blend() }
    fn reblend(&mut self) -> bool { self.core.reblend(self.rt.core_mut()) }
    fn acquire(&mut self) -> Acquire<wgpu::SurfaceTexture> { self.core.acquire() }
    fn prepare(&mut self, drawable: wgpu::SurfaceTexture) -> PreparedEmbedFrame {
        self.core.prepare_draw(self.rt.core_mut(), drawable)
    }
    fn compositor_animating(&self) -> bool { self.compositor.as_ref().is_some_and(|c| c.animating()) }

    fn render(&mut self, prepared: &mut PreparedEmbedFrame, dt: f64, update_ms: f64) -> bool {
        if let Some(comp) = &mut self.compositor {
            let mut f = Frame {
                device: &self.core.gpu.device,
                queue: &self.core.gpu.queue,
                encoder: &mut prepared.encoder,
                target: &prepared.target,
                format: prepared.format,
                size: prepared.size,
                dt,
                update_ms,
                rt: &mut self.rt,
                ui: &mut self.core.renderer,
            };
            comp.render(&mut f);
            comp.animating()
        } else {
            self.core.draw_default(self.rt.core_mut(), prepared);
            false
        }
    }

    fn submit_and_present(&mut self, prepared: PreparedEmbedFrame) -> Instant {
        self.core.submit_and_present(prepared)
    }

    fn clock_and_rt_mut(&mut self) -> (&mut FrameClock, &mut RuntimeCore) {
        (&mut self.core.clock, self.rt.core_mut())
    }

    fn with_driver<R>(&mut self, f: impl FnOnce(&mut FrameClock, &mut RuntimeCore, &mut dyn AppDriver) -> R) -> R {
        let (rt, mut app) = self.rt.split_mut();
        f(&mut self.core.clock, rt, &mut app)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use objc2::{
        msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject},
    };

    use super::LayerHost;
    use crate::compose::{Compositor, Frame};
    use crate::render::KIND_EFFECT;
    use crate::tooling::AppSpec;
    use crate::{Cx, El, col, effect};

    struct MarkProbe(Arc<AtomicU32>);

    impl Compositor<()> for MarkProbe {
        fn render(&mut self, f: &mut Frame<'_, ()>) {
            for (idx, inst) in f.rt.instances.iter().enumerate() {
                if inst.params[1] == KIND_EFFECT {
                    self.0.store(f.rt.marks.get(idx).copied().unwrap_or(0), Ordering::Relaxed);
                }
            }
            f.draw_ui();
        }
    }

    #[test]
    fn layer_host_runs_compositor_with_tracked_marks() {
        // SAFETY: `CAMetalLayer` is a QuartzCore class whose `+new` returns a retained instance.
        let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
        let seen = Arc::new(AtomicU32::new(0));
        let view = |_: &(), _: &mut Cx<()>| -> El { col().size(120.0, 80.0).child(effect(42).size(60.0, 20.0)) };
        let spec = AppSpec::new("embed-test", (120.0, 80.0), || (), view);
        // SAFETY: `layer` is a valid retained `CAMetalLayer` owned by this test and outlives `host` (dropped before `layer`).
        let host = unsafe {
            LayerHost::from_core_animation_layer(
                Retained::as_ptr(&layer) as *mut std::ffi::c_void,
                &spec,
                (),
                (120.0, 80.0),
                2.0,
                true,
                Arc::new(|| {}),
            )
        };
        let Ok(mut host) = host else { return };
        host = host.with_compositor(MarkProbe(seen.clone()));
        let out = host.frame();
        assert!(out.drew);
        assert_eq!(seen.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn layer_host_compositor_receives_instance_and_adapter() {
        use std::sync::Mutex;
        #[derive(Default)]
        struct Seen {
            has_instance: bool,
            adapter_info: Option<wgpu::AdapterInfo>,
            device_info: Option<wgpu::AdapterInfo>,
        }
        struct HostProbe(Arc<Mutex<Seen>>);
        impl Compositor<()> for HostProbe {
            fn render(&mut self, f: &mut Frame<'_, ()>) {
                let mut seen = self.0.lock().unwrap();
                seen.has_instance = f.instance().is_some();
                seen.adapter_info = f.adapter().map(wgpu::Adapter::get_info);
                seen.device_info = Some(f.device.adapter_info());
                f.restore_device_callbacks();
                f.draw_ui();
            }
        }
        // SAFETY: `CAMetalLayer` is a QuartzCore class whose `+new` returns a retained instance.
        let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
        let seen = Arc::new(Mutex::new(Seen::default()));
        let view = |_: &(), _: &mut Cx<()>| -> El { col().size(120.0, 80.0) };
        let spec = AppSpec::new("embed-host-probe", (120.0, 80.0), || (), view);
        // SAFETY: `layer` is a valid retained `CAMetalLayer` owned by this test and outlives `host`.
        let host = unsafe {
            LayerHost::from_core_animation_layer(
                Retained::as_ptr(&layer) as *mut std::ffi::c_void,
                &spec,
                (),
                (120.0, 80.0),
                2.0,
                true,
                Arc::new(|| {}),
            )
        };
        let Ok(mut host) = host else { return };
        host = host.with_compositor(HostProbe(Arc::clone(&seen)));
        let out = host.frame();
        assert!(out.drew);
        let seen = seen.lock().unwrap();
        assert!(seen.has_instance, "LayerHost Frame::instance must be Some");
        assert_eq!(
            seen.adapter_info,
            seen.device_info,
            "LayerHost Frame::adapter info must match Frame::device.adapter_info()",
        );
    }
}
