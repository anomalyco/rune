//! Embedded `CAMetalLayer` host for UIKit, SwiftUI and AppKit views that own their own window
//! and event loop (e.g. a `UIViewRepresentable` driven by `CADisplayLink` on iOS).

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::{Arc, OnceLock};

use crate::compose::{Compositor, Frame};
use crate::frame::clock::FrameClock;
use crate::frame::host_step::{self, Acquire};
use crate::platform::embed_frame::{self, EmbedSurface, FrameSummary};
use crate::render::{Gpu, Renderer};
use crate::runtime::{AppDriver, Env, Runtime, RuntimeCore, Waker};
use crate::time::Instant;
use crate::tooling::{AppSpec, runtime_for};
use crate::window::Missed;

static SHARED_GPU: OnceLock<Result<Arc<Gpu>, String>> = OnceLock::new();

fn shared_gpu() -> Result<Arc<Gpu>, String> {
    SHARED_GPU
        .get_or_init(|| Gpu::try_new(Gpu::instance(), None).map(Arc::new))
        .clone()
}

/// Warms the shared Metal device and compiles the default `Bgra8Unorm` uber pipeline on a
/// background thread so the first embedded `LayerHost` opens without shader-compile latency.
pub fn prewarm() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        std::thread::Builder::new().name("rune-embed-prewarm".into()).spawn(|| {
            if let Ok(gpu) = shared_gpu() {
                let _ = Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
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

struct LayerHostCore {
    gpu: Arc<Gpu>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    clock: FrameClock,
    suspended: bool,
}

impl LayerHostCore {
    unsafe fn new(layer: *mut c_void, viewport: (f32, f32), scale: f32) -> Result<(Self, (f32, f32), f32, Instant), String> {
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
        let scale = scale.max(1.0);
        let max_dim = gpu.device.limits().max_texture_dimension_2d.max(1);
        let px_w = ((viewport.0.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let px_h = ((viewport.1.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let srgb_blend = true;
        let (config, _) = crate::window::surface_config(&surface, &gpu.adapter, (px_w, px_h), srgb_blend)
            .ok_or_else(|| "CAMetalLayer has no supported wgpu surface configuration".to_owned())?;
        crate::window::configure(&surface, &gpu, &config)?;
        let target_format = config.format.remove_srgb_suffix();
        let mut renderer = Renderer::new(&gpu.device, target_format);
        renderer.set_srgb_blend(true);
        let size = (px_w as f32 / scale, px_h as f32 / scale);
        let now = crate::time::now();
        let core = Self {
            gpu,
            surface,
            config,
            renderer,
            clock: FrameClock::new(now),
            suspended: viewport.0 <= 0.0 || viewport.1 <= 0.0,
        };
        Ok((core, size, scale, now))
    }

    fn configure_runtime(&mut self, rt: &mut RuntimeCore, now: Instant) {
        rt.set_srgb_blend(true);
        rt.set_os_drag(false);
        rt.wall_start = Some(now);
        rt.frame.set_record_events(false);
        let target_format = if rt.srgb_blend() { self.config.format.remove_srgb_suffix() } else { self.config.format };
        crate::window::rebuild_renderer_core(&self.gpu.device, &mut self.renderer, target_format, rt);
    }

    fn resize(&mut self, rt: &mut RuntimeCore, viewport: (f32, f32), scale: f32) {
        if viewport.0 <= 0.0 || viewport.1 <= 0.0 {
            self.suspended = true;
            return;
        }
        self.suspended = false;
        let scale = scale.max(1.0);
        let max_dim = self.gpu.device.limits().max_texture_dimension_2d.max(1);
        let px_w = ((viewport.0 * scale).round() as u32).clamp(1, max_dim);
        let px_h = ((viewport.1 * scale).round() as u32).clamp(1, max_dim);
        if self.config.width != px_w || self.config.height != px_h {
            self.config.width = px_w;
            self.config.height = px_h;
            if let Err(error) = crate::window::configure(&self.surface, &self.gpu, &self.config) {
                eprintln!("rune: embedded layer surface refused its new size: {error}");
            }
        }
        rt.set_viewport((px_w as f32 / scale, px_h as f32 / scale), scale);
    }

    /// Acquires the next drawable, reconfiguring and retrying once on `Outdated`/`Lost`.
    fn acquire(&mut self) -> Acquire<wgpu::SurfaceTexture> {
        let mut retries = 1u8;
        loop {
            match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => return Acquire::Ready(f),
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    if let Err(error) = crate::window::configure(&self.surface, &self.gpu, &self.config) {
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
        let mut alpha = None;
        crate::window::reblend_core(&self.surface, &self.gpu, &mut self.config, &mut self.renderer, &mut alpha, rt)
    }

    fn prepare_draw(&mut self, rt: &mut RuntimeCore, frame: wgpu::SurfaceTexture) -> PreparedEmbedFrame {
        let target_format = if rt.srgb_blend() { self.config.format.remove_srgb_suffix() } else { self.config.format };
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: (target_format != self.config.format).then_some(target_format),
            ..Default::default()
        });
        let size = (self.config.width, self.config.height);
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
        crate::time::now()
    }
}

/// Drives one Rune [`Runtime`] and [`Renderer`] directly onto a `CAMetalLayer`.
///
/// Pause display-link calls while the host app is backgrounded on iOS: Metal forbids drawable
/// acquisition and command-buffer submission after `didEnterBackground`.
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
    /// `layer` must be a valid `CAMetalLayer` pointer that outlives this `LayerHost`, and surface
    /// creation / presentation must run on a thread allowed to touch that layer (the UI thread).
    pub unsafe fn from_core_animation_layer(
        layer: *mut c_void,
        spec: &AppSpec<S>,
        state: S,
        viewport: (f32, f32),
        scale: f32,
        dark: bool,
        wake: Waker,
    ) -> Result<Self, String> {
        // SAFETY: caller upholds `from_core_animation_layer`'s safety contract (`layer` is a valid `CAMetalLayer` outliving `LayerHost`, on the UI thread).
        let (mut core, size, scale, now) = unsafe { LayerHostCore::new(layer, viewport, scale) }?;
        let mut rt = runtime_for(spec, state, size, Env::new(dark, scale), wake);
        core.configure_runtime(rt.core_mut(), now);
        Ok(Self { core, compositor: None, rt })
    }

    /// Attaches a custom [`Compositor`] (e.g. for [`El::effect`](crate::El::effect) shader passes)
    /// and turns on `Runtime::track_marks` so `rt.marks` aligns with `rt.instances`.
    pub fn with_compositor(mut self, compositor: impl Compositor<S>) -> Self {
        self.rt.track_marks = true;
        self.compositor = Some(Box::new(compositor));
        self
    }

    pub fn runtime(&self) -> &Runtime<S> {
        &self.rt
    }

    pub fn runtime_mut(&mut self) -> &mut Runtime<S> {
        &mut self.rt
    }

    /// Updates the viewport in points and display scale, reconfiguring the Metal swapchain only
    /// when its clamped device-pixel dimensions change.
    pub fn resize(&mut self, viewport: (f32, f32), scale: f32) {
        self.core.resize(self.rt.core_mut(), viewport, scale);
    }

    /// Steps animations and layout to `Instant::now()` and presents one Metal frame, following
    /// the shared [`crate::frame::host_step`] order.
    pub fn frame(&mut self) -> EmbedFrame {
        embed_frame::run_embed_frame(self).into()
    }
}

impl<S: 'static> EmbedSurface for LayerHost<S> {
    type Drawable = wgpu::SurfaceTexture;
    type Prepared = PreparedEmbedFrame;

    fn suspended(&self) -> bool { self.core.suspended }
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
}
