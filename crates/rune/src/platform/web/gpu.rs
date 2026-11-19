//! WebGPU and WebGL2 backend setup, canvas surface configuration, GPU frame submission, and
//! hand-over pixel readback.

use std::{cell::{Cell, RefCell}, rc::Rc, time::Duration};
use wasm_bindgen::JsCast;
use winit::event_loop::EventLoopProxy;
use crate::{render::{Gpu, Renderer}, runtime::RuntimeCore, time::Instant};
use super::{Wake, app::{CompositorFrame, WebApp}, cpu_canvas, health};

/// The GPU's side of the page: its device, the canvas's surface and Rune's renderer.
pub(super) struct GpuSide {
    pub(super) gpu: Gpu,
    pub(super) surface: wgpu::Surface<'static>,
    pub(super) config: wgpu::SurfaceConfiguration,
    pub(super) renderer: Renderer,
}

/// Who draws the page's frames, and how far the GPU is (the native CPU-first window's
/// `SurfaceMode`, with two canvases for its two surfaces).
pub(super) enum Draw {
    /// The CPU paints into its canvas under the GPU's, which stays see-through and takes the
    /// input. `failed`: the GPU won't come (no WebGPU, or it was lost), so this stays.
    Cpu { cpu: cpu_canvas::CpuCanvas, failed: bool },
    /// The GPU is made and its canvas configured, still see-through. `built` turns true once
    /// the queue has answered a submit made after the renderer's pipelines, so the browser has
    /// compiled them; the CPU paints on meanwhile. Then one frame is drawn by both from the same
    /// instances, and nothing new until the GPU's is done (`done`): its canvas then shows over
    /// the CPU's, which goes.
    HandingOver { cpu: cpu_canvas::CpuCanvas, side: GpuSide, built: Rc<Cell<bool>>, done: Option<Rc<Cell<bool>>>, since: Instant },
    Gpu(GpuSide),
    /// Only while one state is taken apart for the next: no frame sees it.
    Moving,
}

impl Draw {
    pub(super) fn gpu(&self) -> Option<&GpuSide> {
        match self {
            Draw::HandingOver { side, .. } | Draw::Gpu(side) => Some(side),
            Draw::Cpu { .. } | Draw::Moving => None,
        }
    }

    pub(super) fn gpu_mut(&mut self) -> Option<&mut GpuSide> {
        match self {
            Draw::HandingOver { side, .. } | Draw::Gpu(side) => Some(side),
            Draw::Cpu { .. } | Draw::Moving => None,
        }
    }

    /// The GPU's hand-over frame is drawn and not finished: nothing new is drawn until it is.
    pub(super) fn waiting_for_gpu(&self) -> bool {
        matches!(self, Draw::HandingOver { done: Some(done), .. } if !done.get())
    }
}

async fn try_backend(canvas: &web_sys::HtmlCanvasElement, backends: wgpu::Backends, features: wgpu::Features, webgl: bool)
    -> Result<(Gpu, wgpu::Surface<'static>), String> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends;
    let instance = wgpu::Instance::new(descriptor);
    let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone())).map_err(|e| e.to_string())?;
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower, compatible_surface: Some(&surface),
        force_fallback_adapter: false, apply_limit_buckets: false,
    }).await.map_err(|e| e.to_string())?;
    let required_limits = if webgl {
        wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
    } else {
        wgpu::Limits::default().using_resolution(adapter.limits())
    };
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rune web"), required_features: adapter.features() & features,
        required_limits, experimental_features: Default::default(),
        memory_hints: Default::default(), trace: Default::default(),
    }).await.map_err(|e| format!("GPU device: {e}"))?;
    Ok((Gpu::from_device(instance, adapter, device, queue), surface))
}

pub(super) async fn gpu(canvas: web_sys::HtmlCanvasElement, features: wgpu::Features) -> Result<(Gpu, wgpu::Surface<'static>), String> {
    match try_backend(&canvas, wgpu::Backends::BROWSER_WEBGPU, features, false).await {
        Ok(pair) => Ok(pair),
        #[cfg(feature = "webgl")]
        Err(webgpu_err) => try_backend(&canvas, wgpu::Backends::GL, features, true)
            .await
            .map_err(|gl_err| format!("WebGPU ({webgpu_err}) and WebGL2 ({gl_err}) unavailable")),
        #[cfg(not(feature = "webgl"))]
        Err(e) => Err(format!("WebGPU adapter unavailable (HTTPS and WebGPU are required): {e}")),
    }
}

/// The GPU's side for a canvas of `size`: the surface configured, the renderer made, and the
/// text system told its masks are the renderer's to fill from now on. `lost` records the reason
/// and wakes `proxy` if the device is lost.
pub(super) fn gpu_side(gpu: Gpu, surface: wgpu::Surface<'static>, size: (u32, u32), rt: &mut RuntimeCore, lost: Rc<RefCell<Option<String>>>, proxy: EventLoopProxy<Wake>) -> Result<GpuSide, String> {
    let Some((config, format)) = canvas_config(&gpu, &surface, size, rt.srgb_blend()) else {
        return Err("WebGPU canvas has no supported configuration".into());
    };
    surface.configure(&gpu.device, &config);
    health::watch(&gpu.device, move |why| {
        *lost.borrow_mut() = Some(why);
        let _ = proxy.send_event(Wake::App);
    });
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(!format.is_srgb());
    renderer.attach_text(&mut rt.text.atlas);
    Ok(GpuSide { gpu, surface, config, renderer })
}

/// The canvas's configuration for `size`, and the format the renderer draws in: the canvas's,
/// or its sRGB twin directly (when supported, as on WebGL2) or as a view format (on WebGPU) for
/// the blend `srgb_blend` asks.
pub(super) fn canvas_config(gpu: &Gpu, surface: &wgpu::Surface<'static>, size: (u32, u32), srgb_blend: bool) -> Option<(wgpu::SurfaceConfiguration, wgpu::TextureFormat)> {
    let max_dim = gpu.device.limits().max_texture_dimension_2d.max(1);
    let mut config = surface.get_default_config(&gpu.adapter, size.0.clamp(1, max_dim), size.1.clamp(1, max_dim))?;
    config.present_mode = wgpu::PresentMode::Fifo;
    let caps = surface.get_capabilities(&gpu.adapter);
    let backend = gpu.adapter.get_info().backend;
    config.usage = super::web_math::surface_usage(caps.usages, backend, drift_wanted());
    let chosen = super::web_math::select_surface_formats(config.format, &caps.formats, backend, srgb_blend);
    config.format = chosen.surface;
    if let Some(view) = chosen.view_format {
        config.view_formats.push(view);
    }
    Some((config, chosen.render))
}

/// A frame drawn on the GPU from the runtime's instances, presented; `Err` when the canvas gave
/// no texture to draw into.
pub(super) fn gpu_frame(side: &mut GpuSide, app: &mut dyn WebApp, now: Instant, dt: f64, readback: bool, tick: bool) -> Result<Option<Readback>, String> {
    {
        let rt = app.rt_mut();
        if side.renderer.srgb_blend() != rt.srgb_blend() {
            let caps = side.surface.get_capabilities(&side.gpu.adapter);
            let backend = side.gpu.adapter.get_info().backend;
            let chosen = super::web_math::select_surface_formats(side.config.format, &caps.formats, backend, rt.srgb_blend());
            let mut reconfigure = side.config.format != chosen.surface;
            side.config.format = chosen.surface;
            if let Some(view) = chosen.view_format {
                if !side.config.view_formats.contains(&view) {
                    side.config.view_formats.push(view);
                    reconfigure = true;
                }
            } else if !side.config.view_formats.is_empty() {
                side.config.view_formats.clear();
                reconfigure = true;
            }
            if reconfigure {
                side.surface.configure(&side.gpu.device, &side.config);
            }
            if side.renderer.format() != chosen.render {
                crate::window::rebuild_renderer_core(&side.gpu.device, &mut side.renderer, chosen.render, rt);
            }
            side.renderer.set_srgb_blend(chosen.srgb_blend());
        }
    }
    let frame = match side.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
        _ => return Err("WebGPU canvas acquisition failed".into()),
    };
    let size = (side.config.width, side.config.height);
    let armed = {
        let rt = app.rt_mut();
        side.renderer.upload_images(&side.gpu.device, &side.gpu.queue, &mut rt.images);
        // A loop-only step (`runtime/paint/cycle.rs`) writes only the dots it changed, and redraws
        // only where they are, into the last frame the renderer keeps, which is copied to the canvas.
        let (paint, slots) = rt.cycle_paint();
        let armed = tick && side.config.usage.contains(wgpu::TextureUsages::COPY_DST)
            && side.renderer.arm_cycle(&side.gpu.device, &side.gpu.queue, paint, &rt.instances, &slots, rt.clear_color(), size, side.config.format).is_ok();
        side.renderer.next_instances(paint, tick.then_some(slots));
        armed
    };
    let mut encoder = side.gpu.device.create_command_encoder(&Default::default());
    if armed {
        // Rune's default frame (a compositor that drew its own would have kept this a whole one):
        // nothing draws into the canvas but the copy of the kept frame.
        let rt = app.rt_mut();
        side.renderer.prepare_ui(&side.gpu.device, &side.gpu.queue, &mut rt.text.atlas, &rt.instances, size);
        side.renderer.draw_armed(&mut encoder, &frame.texture);
    } else if app.has_compositor() {
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(side.renderer.format()), ..Default::default() });
        app.render_compositor(CompositorFrame {
            device: &side.gpu.device,
            queue: &side.gpu.queue,
            encoder: &mut encoder,
            target: &target,
            format: side.renderer.format(),
            size,
            dt,
            update_ms: now.elapsed().as_secs_f64() * 1000.0,
            ui: &mut side.renderer,
        });
    } else {
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(side.renderer.format()), ..Default::default() });
        let rt = app.rt_mut();
        side.renderer.prepare_ui(&side.gpu.device, &side.gpu.queue, &mut rt.text.atlas, &rt.instances, size);
        side.renderer.draw(&mut encoder, &target, rt.clear_color());
    }
    // A copy of the frame for counting how many of its pixels the CPU's differ in (drift).
    let copy = (readback && side.config.usage.contains(wgpu::TextureUsages::COPY_SRC)).then(|| {
        let (width, height) = (side.config.width, side.config.height);
        let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = side.gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("rune hand-over readback"), size: u64::from(row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        encoder.copy_texture_to_buffer(frame.texture.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) } },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
        Readback { buffer, row, size: (width, height), rgba: matches!(side.config.format, wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb) }
    });
    // Every frame: one drawn whole leaves the kept frame behind.
    let _ = side.renderer.damage_drawn();
    side.renderer.submit(&side.gpu.queue, encoder);
    side.gpu.queue.present(frame);
    Ok(copy)
}

/// The GPU's hand-over frame, copied where the CPU can read it.
pub(super) struct Readback {
    buffer: wgpu::Buffer,
    row: u32,
    size: (u32, u32),
    /// The canvas's format is RGBA (the CPU paints BGRA).
    rgba: bool,
}

impl Readback {
    /// Counts the pixels of `cpu` (BGRA, `size.0 * 4` bytes a row) this frame drew otherwise, and
    /// appends `[differing, of, max]` to `globalThis.__runeDrift` once the copy is mapped.
    pub(super) fn count(self, cpu: Vec<u8>) {
        let Readback { buffer, row, size: (w, h), rgba } = self;
        let buffer = Rc::new(buffer);
        let mapped = buffer.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            if result.is_err() { return; }
            let (mut differing, mut max) = (0u32, 0u8);
            let mut bbox = [u32::MAX, u32::MAX, 0u32, 0u32];
            let mut worst = [0u32; 8];
            {
                let Ok(gpu) = mapped.slice(..).get_mapped_range() else { return };
                for y in 0..h as usize {
                    let g = &gpu[y * row as usize..y * row as usize + w as usize * 4];
                    let c = &cpu[y * w as usize * 4..(y + 1) * w as usize * 4];
                    for (x, (p, q)) in c.chunks_exact(4).zip(g.chunks_exact(4)).enumerate() {
                        let q = if rgba { [q[2], q[1], q[0]] } else { [q[0], q[1], q[2]] };
                        let d = p[..3].iter().zip(&q).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
                        if d > 1 {
                            let (x, y) = (x as u32, y as u32);
                            bbox = [bbox[0].min(x), bbox[1].min(y), bbox[2].max(x + 1), bbox[3].max(y + 1)];
                        }
                        if d > max { worst = [x as u32, y as u32, u32::from(p[2]), u32::from(p[1]), u32::from(p[0]), u32::from(q[2]), u32::from(q[1]), u32::from(q[0])]; }
                        if d > 0 { differing += 1; max = max.max(d); }
                    }
                }
            }
            mapped.unmap();
            let Ok(list) = js_sys::Reflect::get(&js_sys::global(), &"__runeDrift".into()) else { return };
            if !js_sys::Array::is_array(&list) { return; }
            let list: js_sys::Array = list.unchecked_into();
            let row = js_sys::Array::of3(&differing.into(), &(w * h).into(), &max.into());
            for v in bbox.into_iter().chain(worst) { row.push(&v.into()); }
            list.push(&row);
        });
    }
}

/// Whether the page asked for the hand-over's drift (`globalThis.__runeDrift`, an array).
pub(super) fn drift_wanted() -> bool {
    js_sys::Reflect::get(&js_sys::global(), &"__runeDrift".into()).is_ok_and(|v| js_sys::Array::is_array(&v))
}

/// A flag the GPU's queue sets once the work submitted so far is done, waking the loop.
pub(super) fn when_done(queue: &wgpu::Queue, proxy: &EventLoopProxy<Wake>) -> Rc<Cell<bool>> {
    let flag = Rc::new(Cell::new(false));
    let set = PageOnly((flag.clone(), proxy.clone()));
    queue.on_submitted_work_done(move || {
        let PageOnly((flag, proxy)) = &set;
        flag.set(true);
        let _ = proxy.send_event(Wake::App);
    });
    flag
}

/// A value moved into a callback wgpu wants `Send`. In a page that callback runs on the page's
/// one thread, from a promise.
struct PageOnly<T>(T);
// SAFETY: wasm32 without threads: there is one thread, and every callback runs on it.
unsafe impl<T> Send for PageOnly<T> {}

/// How long the GPU's hand-over frame may take to finish before the page stays on the CPU.
pub(super) const FINISH_LIMIT: Duration = Duration::from_secs(5);
