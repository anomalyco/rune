//! Shared Android `ANativeWindow` swapchain and `Gpu` device reused across every page in the
//! activity's [`PageStack`](super::stack::PageStack), avoiding swapchain detach/reattach churn
//! when pages push or pop.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::Arc;

use crate::compose::{Compositor, Frame};
use crate::frame::clock::FrameClock;
use crate::frame::host_step::{self, Acquire};
use crate::platform::embed::{EmbedFrame, android_gpu_and_surface, create_android_surface};
use crate::platform::embed_frame::{self, EmbedSurface, EmbedSurfaceEvent, EmbedSurfaceHealth};
use crate::render::{Gpu, Renderer};
use crate::runtime::{AppDriver, Runtime, RuntimeCore};
use crate::time::Instant;
use crate::window::Missed;

struct AttachedSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

struct PreparedPageFrame {
    frame: wgpu::SurfaceTexture,
    target: wgpu::TextureView,
    encoder: wgpu::CommandEncoder,
    size: (u32, u32),
    format: wgpu::TextureFormat,
}

/// One shared Android `ANativeWindow` swapchain and `Gpu` device reused across every page in the
/// activity's page stack.
pub(crate) struct AndroidSurface {
    gpu: Arc<Gpu>,
    surface: Option<AttachedSurface>,
    surface_srgb_blend: bool,
    generation: u64,
    suspended: bool,
    surface_health: EmbedSurfaceHealth,
}

impl AndroidSurface {
    /// Creates the shared GPU/surface wrapper from an `ANativeWindow *`.
    ///
    /// # Safety
    /// `native_window` must be a valid `ANativeWindow *` outliving the attached surface until
    /// [`Self::detach_surface`] or drop.
    pub(crate) unsafe fn from_native_window(
        native_window: *mut c_void,
        viewport: (f32, f32),
        scale: f32,
        srgb_blend: bool,
    ) -> Result<(Self, (f32, f32), f32), String> {
        // SAFETY: caller upholds `from_native_window`'s safety contract.
        let (gpu, surface) = unsafe { android_gpu_and_surface(native_window) }?;
        let (attached, size, scale) = Self::configure_surface(&gpu, surface, viewport, scale, srgb_blend)?;
        let surface = Self {
            gpu,
            surface: Some(attached),
            surface_srgb_blend: srgb_blend,
            generation: 1,
            suspended: viewport.0 <= 0.0 || viewport.1 <= 0.0,
            surface_health: EmbedSurfaceHealth::default(),
        };
        Ok((surface, size, scale))
    }

    fn configure_surface(
        gpu: &Gpu,
        surface: wgpu::Surface<'static>,
        viewport: (f32, f32),
        scale: f32,
        srgb_blend: bool,
    ) -> Result<(AttachedSurface, (f32, f32), f32), String> {
        let scale = scale.max(1.0);
        let max_dim = gpu.device.limits().max_texture_dimension_2d.max(1);
        let px_w = ((viewport.0.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let px_h = ((viewport.1.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let (config, _) = crate::window::surface_config(&surface, &gpu.adapter, (px_w, px_h), srgb_blend, false)
            .ok_or_else(|| "Android ANativeWindow has no supported wgpu surface configuration".to_owned())?;
        crate::window::configure(&surface, gpu, &config)?;
        let size = (px_w as f32 / scale, px_h as f32 / scale);
        Ok((AttachedSurface { surface, config }, size, scale))
    }

    /// Reattaches a fresh `ANativeWindow *` after [`Self::detach_surface`].
    ///
    /// # Safety
    /// `native_window` must be a valid `ANativeWindow *` outliving the attached surface until
    /// the next [`Self::detach_surface`] or drop.
    pub(crate) unsafe fn attach_native_window(
        &mut self,
        native_window: *mut c_void,
        viewport: (f32, f32),
        scale: f32,
        srgb_blend: bool,
    ) -> Result<((f32, f32), f32), String> {
        self.surface = None;
        // SAFETY: caller upholds `attach_native_window`'s safety contract.
        let surface = unsafe { create_android_surface(&self.gpu.instance, native_window) }?;
        let (attached, size, scale) = Self::configure_surface(&self.gpu, surface, viewport, scale, srgb_blend)?;
        self.surface = Some(attached);
        self.surface_srgb_blend = srgb_blend;
        self.generation = self.generation.wrapping_add(1).max(1);
        self.surface_health.step(EmbedSurfaceEvent::Resized);
        self.suspended = viewport.0 <= 0.0 || viewport.1 <= 0.0;
        Ok((size, scale))
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Re-syncs an existing per-page [`Renderer`] after swapchain reattachment.
    pub(crate) fn sync_renderer(&self, renderer: &mut Renderer, rt: &mut RuntimeCore) {
        if let Some(attached) = &self.surface {
            let target_format = if rt.srgb_blend() {
                attached.config.format.remove_srgb_suffix()
            } else {
                attached.config.format
            };
            crate::window::rebuild_renderer_core(&self.gpu.device, renderer, target_format, rt);
        }
    }

    pub(crate) fn detach_surface(&mut self) {
        self.surface = None;
        self.suspended = true;
    }

    pub(crate) fn resize(&mut self, viewport: (f32, f32), scale: f32) -> Option<((f32, f32), f32)> {
        if viewport.0 <= 0.0 || viewport.1 <= 0.0 {
            self.suspended = true;
            return None;
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
                    eprintln!("rune: android surface refused its new size: {error}");
                }
            }
        }
        Some(((px_w as f32 / scale, px_h as f32 / scale), scale))
    }

    /// Creates a per-page [`Renderer`] sharing this surface's [`Gpu`] device.
    pub(crate) fn new_renderer(&self, rt: &mut RuntimeCore) -> Result<Renderer, String> {
        let target_format = self.surface.as_ref().map_or(wgpu::TextureFormat::Rgba8Unorm, |a| {
            if rt.srgb_blend() { a.config.format.remove_srgb_suffix() } else { a.config.format }
        });
        let mut renderer = Renderer::try_new(&self.gpu.device, target_format)?;
        renderer.attach_gpu(&self.gpu);
        crate::window::rebuild_renderer_core(&self.gpu.device, &mut renderer, target_format, rt);
        Ok(renderer)
    }

    /// Draws one stacked Android page onto this shared swapchain.
    pub(crate) fn draw_page<S: 'static>(
        &mut self,
        renderer: &mut Renderer,
        clock: &mut FrameClock,
        compositor: &mut Option<Box<dyn Compositor<S>>>,
        rt: &mut Runtime<S>,
    ) -> EmbedFrame {
        let mut draw = PageDraw { surface: self, renderer, clock, compositor, rt };
        embed_frame::run_embed_frame(&mut draw).into()
    }

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
                        eprintln!("rune: android surface refused reconfiguration: {error}");
                        return Acquire::Missed(Missed::Unconfigured);
                    }
                    if retries == 0 { return Acquire::Missed(Missed::Outdated); }
                    retries -= 1;
                }
                _ => return Acquire::Missed(Missed::Timeout),
            }
        }
    }

    fn reblend(&mut self, renderer: &mut Renderer, rt: &mut RuntimeCore) -> bool {
        let Some(attached) = &mut self.surface else {
            return false;
        };
        let mut alpha = None;
        let ok = crate::window::reblend_core(&attached.surface, &self.gpu, &mut attached.config, renderer, &mut alpha, rt);
        if ok {
            self.surface_srgb_blend = rt.srgb_blend();
        }
        ok
    }

    fn prepare_draw(&self, renderer: &mut Renderer, rt: &mut RuntimeCore, frame: wgpu::SurfaceTexture) -> PreparedPageFrame {
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
        host_step::prepare_gpu_resources(renderer, &self.gpu, rt, size, true);
        let encoder = self.gpu.device.create_command_encoder(&Default::default());
        PreparedPageFrame { frame, target, encoder, size, format: target_format }
    }

    fn submit_and_present(&self, renderer: &mut Renderer, prepared: PreparedPageFrame) -> Instant {
        renderer.submit(&self.gpu.queue, prepared.encoder);
        self.gpu.queue.present(prepared.frame);
        crate::render::gpu_events::presented();
        if !self.gpu.is_lost() {
            renderer.warm_effects(&self.gpu.device);
        }
        crate::time::now()
    }
}

struct PageDraw<'a, S: 'static> {
    surface: &'a mut AndroidSurface,
    renderer: &'a mut Renderer,
    clock: &'a mut FrameClock,
    compositor: &'a mut Option<Box<dyn Compositor<S>>>,
    rt: &'a mut Runtime<S>,
}

impl<S: 'static> EmbedSurface for PageDraw<'_, S> {
    type Drawable = wgpu::SurfaceTexture;
    type Prepared = PreparedPageFrame;

    fn suspended(&self) -> bool { self.surface.suspended || self.surface.surface.is_none() }
    fn surface_health_mut(&mut self) -> &mut EmbedSurfaceHealth { &mut self.surface.surface_health }
    fn renderer_srgb_blend(&self) -> bool {
        let want = self.rt.core().srgb_blend();
        if self.renderer.srgb_blend() == want && self.surface.surface_srgb_blend == want {
            want
        } else {
            !want
        }
    }
    fn rt_srgb_blend(&self) -> bool { self.rt.core().srgb_blend() }
    fn reblend(&mut self) -> bool { self.surface.reblend(self.renderer, self.rt.core_mut()) }
    fn acquire(&mut self) -> Acquire<wgpu::SurfaceTexture> { self.surface.acquire() }
    fn prepare(&mut self, drawable: wgpu::SurfaceTexture) -> PreparedPageFrame {
        self.surface.prepare_draw(self.renderer, self.rt.core_mut(), drawable)
    }
    fn compositor_animating(&self) -> bool { self.compositor.as_ref().is_some_and(|c| c.animating()) }

    fn render(&mut self, prepared: &mut PreparedPageFrame, dt: f64, update_ms: f64) -> bool {
        if let Some(comp) = self.compositor {
            let mut f = Frame {
                device: &self.surface.gpu.device,
                queue: &self.surface.gpu.queue,
                encoder: &mut prepared.encoder,
                target: &prepared.target,
                format: prepared.format,
                size: prepared.size,
                dt,
                update_ms,
                rt: self.rt,
                ui: self.renderer,
            };
            comp.render(&mut f);
            comp.animating()
        } else {
            let clear = self.rt.core().clear_color();
            host_step::draw_ui(
                self.renderer,
                &self.surface.gpu,
                &mut prepared.encoder,
                &prepared.target,
                self.rt.core_mut(),
                prepared.size,
                clear,
            );
            false
        }
    }

    fn submit_and_present(&mut self, prepared: PreparedPageFrame) -> Instant {
        self.surface.submit_and_present(self.renderer, prepared)
    }

    fn clock_and_rt_mut(&mut self) -> (&mut FrameClock, &mut RuntimeCore) {
        (self.clock, self.rt.core_mut())
    }

    fn with_driver<R>(&mut self, f: impl FnOnce(&mut FrameClock, &mut RuntimeCore, &mut dyn AppDriver) -> R) -> R {
        let (rt, mut app) = self.rt.split_mut();
        f(self.clock, rt, &mut app)
    }

    fn preserve_shell_requests(&self) -> bool { true }
}
