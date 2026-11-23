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
    pub(super) backend: wgpu::Backend,
    health: Rc<RefCell<(crate::machine::Seq, super::web_math::SurfaceHealth)>>,
    retry_due: Rc<Cell<Option<crate::machine::Attempt>>>,
    proxy: EventLoopProxy<Wake>,
}

/// A GPU device or WebGL2 context loss waiting for `Browser::lost_device` to handle.
pub(super) enum Loss {
    WebGpu(String),
    WebGl(String),
}

impl Loss {
    pub(super) fn is_webgl(&self) -> bool {
        matches!(self, Loss::WebGl(_))
    }

    pub(super) fn reason(&self) -> &str {
        match self {
            Loss::WebGpu(why) | Loss::WebGl(why) => why,
        }
    }
}

/// Who draws the page's frames, and how far the GPU is (the native CPU-first window's
/// `SurfaceMode`, with two canvases for its two surfaces).
pub(super) enum Draw {
    /// The CPU paints into its canvas under the GPU's, which stays see-through and takes the
    /// input. `failed`: the GPU won't come automatically (no WebGPU, or it was lost until restore).
    Cpu { cpu: cpu_canvas::CpuCanvas, failed: bool },
    /// The GPU is made and its canvas configured, still see-through. `built` turns true once
    /// the queue has answered a submit made after the renderer's pipelines, so the browser has
    /// compiled them; the CPU paints on meanwhile. Then one frame is drawn by both from the same
    /// instances, and nothing new until the GPU's is done (`done`): its canvas then shows over
    /// the CPU's, which goes.
    HandingOver { cpu: cpu_canvas::CpuCanvas, side: GpuSide, built: Rc<Cell<bool>>, done: Option<Rc<Cell<bool>>>, since: Instant },
    Gpu(GpuSide),
    /// WebGL2 context lost on a GPU-first inline or document page that cannot overlay a CPU
    /// canvas; redraws pause until `webglcontextrestored` rebuilds `GpuSide`.
    ContextLost,
    /// Only while one state is taken apart for the next: no frame sees it.
    Moving,
}

impl Draw {
    pub(super) fn gpu(&self) -> Option<&GpuSide> {
        match self {
            Draw::HandingOver { side, .. } | Draw::Gpu(side) => Some(side),
            Draw::Cpu { .. } | Draw::ContextLost | Draw::Moving => None,
        }
    }

    pub(super) fn gpu_mut(&mut self) -> Option<&mut GpuSide> {
        match self {
            Draw::HandingOver { side, .. } | Draw::Gpu(side) => Some(side),
            Draw::Cpu { .. } | Draw::ContextLost | Draw::Moving => None,
        }
    }

    /// The GPU's hand-over frame is drawn and not finished: nothing new is drawn until it is.
    pub(super) fn waiting_for_gpu(&self) -> bool {
        matches!(self, Draw::HandingOver { done: Some(done), .. } if !done.get())
    }

    pub(super) fn kind(&self) -> super::web_math::WebDrawKind {
        use super::web_math::WebDrawKind;
        match self {
            Draw::Cpu { .. } => WebDrawKind::Cpu,
            Draw::HandingOver { .. } => WebDrawKind::HandingOver { waiting_for_gpu: self.waiting_for_gpu() },
            Draw::Gpu(_) => WebDrawKind::Gpu,
            Draw::ContextLost => WebDrawKind::ContextLost,
            Draw::Moving => WebDrawKind::Moving,
        }
    }
}

/// Probes WebGPU for an adapter and device before touching `canvas`.
///
/// Calling `canvas.getContext("webgpu")` permanently locks the canvas to WebGPU even if
/// `requestAdapter` or `requestDevice` then fails, blocking any WebGL2 fallback on the same
/// canvas. wgpu's WebGPU backend does not need a surface to request an adapter or device, so
/// the canvas surface is created only after both succeed.
pub(super) async fn try_webgpu(canvas: &web_sys::HtmlCanvasElement, features: wgpu::Features)
    -> Result<(Gpu, wgpu::Surface<'static>), String> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::BROWSER_WEBGPU;
    let instance = wgpu::Instance::new(descriptor);
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower, compatible_surface: None,
        force_fallback_adapter: false, apply_limit_buckets: false,
    }).await.map_err(|e| e.to_string())?;
    let required_limits = wgpu::Limits::default().using_resolution(adapter.limits());
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rune web"), required_features: adapter.features() & features,
        required_limits, experimental_features: Default::default(),
        memory_hints: Default::default(), trace: Default::default(),
    }).await.map_err(|e| format!("GPU device: {e}"))?;
    let surface = match instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone())) {
        Ok(surface) => surface,
        Err(e) => {
            device.destroy();
            return Err(e.to_string());
        }
    };
    Ok((Gpu::from_device(instance, adapter, device, queue), surface))
}

/// Creates a WebGL2 surface and device on `canvas`. Unlike WebGPU, `wgpu-hal`'s WebGL2 backend
/// discovers its adapter from the surface's `WebGL2RenderingContext`, so the surface comes first.
/// Yields a browser macrotask between synchronous stages (`create_surface`, `request_adapter`,
/// `request_device`) so the browser can paint and handle input between them.
#[cfg(feature = "webgl")]
pub(super) async fn try_webgl2(
    canvas: &web_sys::HtmlCanvasElement,
    features: wgpu::Features,
    quitting: &std::sync::atomic::AtomicBool,
) -> Result<(Gpu, wgpu::Surface<'static>), String> {
    use std::sync::atomic::Ordering;
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::GL;
    let instance = wgpu::Instance::new(descriptor);
    let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone())).map_err(|e| e.to_string())?;
    super::yield_turn().await;
    if quitting.load(Ordering::Acquire) { return Err("app quit".into()); }
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower, compatible_surface: Some(&surface),
        force_fallback_adapter: false, apply_limit_buckets: false,
    }).await.map_err(|e| e.to_string())?;
    super::yield_turn().await;
    if quitting.load(Ordering::Acquire) { return Err("app quit".into()); }
    let adapter_limits = adapter.limits();
    let required_limits = wgpu::Limits::downlevel_webgl2_defaults()
        .using_resolution(adapter_limits.clone())
        .using_alignment(adapter_limits);
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rune web"), required_features: adapter.features() & features,
        required_limits, experimental_features: Default::default(),
        memory_hints: Default::default(), trace: Default::default(),
    }).await.map_err(|e| format!("GPU device: {e}"))?;
    if quitting.load(Ordering::Acquire) {
        device.destroy();
        return Err("app quit".into());
    }
    Ok((Gpu::from_device(instance, adapter, device, queue), surface))
}

pub(super) async fn gpu(
    canvas: web_sys::HtmlCanvasElement,
    features: wgpu::Features,
    quitting: &std::sync::atomic::AtomicBool,
) -> Result<(Gpu, wgpu::Surface<'static>), String> {
    #[cfg(not(feature = "webgl"))]
    let _ = quitting;
    let mut webgpu_err = None;
    for &backend in super::web_math::backend_order(health::has_navigator_gpu(), cfg!(feature = "webgl")) {
        match backend {
            super::web_math::WebBackend::WebGpu => match try_webgpu(&canvas, features).await {
                Ok(pair) => return Ok(pair),
                Err(err) => webgpu_err = Some(err),
            },
            super::web_math::WebBackend::WebGl2 => {
                #[cfg(feature = "webgl")]
                {
                    if webgpu_err.is_some() {
                        super::yield_turn().await;
                        if quitting.load(std::sync::atomic::Ordering::Acquire) {
                            return Err("app quit".into());
                        }
                    }
                    match try_webgl2(&canvas, features, quitting).await {
                        Ok(pair) => return Ok(pair),
                        Err(gl_err) => {
                            let webgpu = webgpu_err.as_deref().unwrap_or("navigator.gpu unavailable");
                            return Err(format!("WebGPU ({webgpu}) and WebGL2 ({gl_err}) unavailable"));
                        }
                    }
                }
            }
        }
    }
    let why = webgpu_err.unwrap_or_else(|| "navigator.gpu unavailable".into());
    Err(format!("WebGPU adapter unavailable (HTTPS and WebGPU are required): {why}"))
}

/// The GPU's side for a canvas of `size`: the surface configured, the renderer made, and the
/// text system told its masks are the renderer's to fill from now on. `lost` records the reason
/// and wakes `proxy` if the device is lost.
pub(super) fn gpu_side(gpu: Gpu, surface: wgpu::Surface<'static>, size: (u32, u32), rt: &mut RuntimeCore, lost: Rc<RefCell<Option<Loss>>>, proxy: EventLoopProxy<Wake>) -> Result<GpuSide, String> {
    let Some((config, format)) = canvas_config(&gpu, &surface, size, rt.srgb_blend()) else {
        return Err("WebGPU canvas has no supported configuration".into());
    };
    let backend = gpu.adapter.get_info().backend;
    let is_gl = backend == wgpu::Backend::Gl;
    let watch_proxy = proxy.clone();
    health::watch(&gpu.device, move |why| {
        *lost.borrow_mut() = Some(if is_gl { Loss::WebGl(why) } else { Loss::WebGpu(why) });
        watch_proxy.send_event(Wake::App).ok();
    });
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(!format.is_srgb());
    renderer.attach_text(&mut rt.text.atlas);
    // Re-upload every placed glyph row and outline record on a fresh renderer (for example after
    // a WebGL2 context restore, where a previous renderer had already cleared `atlas.dirty`).
    rt.text.atlas.invalidate();
    let side = GpuSide {
        gpu,
        surface,
        config,
        renderer,
        backend,
        health: Rc::new(RefCell::new((crate::machine::Seq::default(), super::web_math::SurfaceHealth::default()))),
        retry_due: Rc::new(Cell::new(None)),
        proxy,
    };
    configure_surface(&side);
    Ok(side)
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
    config.usage = super::web_math::surface_usage(caps.usages, backend);
    let chosen = super::web_math::select_surface_formats(config.format, &caps.formats, backend, srgb_blend);
    config.format = chosen.surface;
    if let Some(view) = chosen.view_format {
        config.view_formats.push(view);
    }
    Some((config, chosen.render))
}

/// Reconfigures the canvas surface and rebuilds the renderer when `rt.srgb_blend()` differs from
/// the renderer's current blend mode.
pub(super) fn reblend(side: &mut GpuSide, rt: &mut RuntimeCore) {
    if side.renderer.srgb_blend() == rt.srgb_blend() { return; }
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
        configure_surface(side);
    }
    if side.renderer.format() != chosen.render {
        crate::window::rebuild_renderer_core(&side.gpu.device, &mut side.renderer, chosen.render, rt);
    }
    side.renderer.set_srgb_blend(chosen.srgb_blend());
}

/// Configures `side.surface` through the guarded web helper and updates `side.health` with its
/// outcome (synchronously on WebGL2, asynchronously on WebGPU).
pub(super) fn configure_surface(side: &GpuSide) {
    use super::web_math::{CONFIGURE_MAX_FAILURES, SurfaceHealthEffect, SurfaceHealthEvent};
    let attempt = {
        let mut h = side.health.borrow_mut();
        let (seq, state) = &mut *h;
        let attempt = seq.next();
        state.step(seq, SurfaceHealthEvent::Begin(attempt));
        attempt
    };
    let health = side.health.clone();
    let retry_due = side.retry_due.clone();
    let proxy = side.proxy.clone();
    crate::window::configure_then(&side.surface, &side.gpu, &side.config, move |res| {
        let effects = {
            let mut h = health.borrow_mut();
            let (seq, state) = &mut *h;
            state.step(seq, SurfaceHealthEvent::Outcome { attempt, error: res.err() })
        };
        for effect in effects {
            match effect {
                SurfaceHealthEffect::ReportFirst(err) => {
                    web_sys::console::error_1(&format!("rune: web canvas surface refused configuration: {err}").into());
                }
                SurfaceHealthEffect::ReportGaveUp(err) => {
                    web_sys::console::error_1(&format!(
                        "rune: web canvas surface gave up after {CONFIGURE_MAX_FAILURES} failures, waiting for resize: {err}"
                    ).into());
                }
                SurfaceHealthEffect::ScheduleRetry { attempt, delay } => {
                    let retry_due = retry_due.clone();
                    let proxy = proxy.clone();
                    super::after(delay, move || {
                        retry_due.set(Some(attempt));
                        proxy.send_event(Wake::App).ok();
                    });
                }
                SurfaceHealthEffect::Configure => {}
            }
        }
    });
}

/// Runs a due configure retry if the surface is not suspended.
pub(super) fn drain_configure_retry(side: &GpuSide, suspended: bool) {
    if suspended { return; }
    let Some(attempt) = side.retry_due.take() else { return };
    let should_configure = {
        let mut h = side.health.borrow_mut();
        let (seq, state) = &mut *h;
        state.step(seq, super::web_math::SurfaceHealthEvent::RetryDue(attempt))
            .into_iter()
            .any(|e| matches!(e, super::web_math::SurfaceHealthEffect::Configure))
    };
    if should_configure {
        configure_surface(side);
    }
}

pub(super) fn can_draw(side: &GpuSide) -> bool {
    side.health.borrow().1.can_draw()
}

pub(super) fn on_presented(side: &GpuSide) {
    let mut h = side.health.borrow_mut();
    let (seq, state) = &mut *h;
    state.step(seq, super::web_math::SurfaceHealthEvent::Presented);
}

/// Wakes a surface that was retrying or had given up so a resize can reconfigure it.
pub(super) fn on_resized(side: &GpuSide) -> bool {
    let was_blocked = !can_draw(side);
    let mut h = side.health.borrow_mut();
    let (seq, state) = &mut *h;
    state.step(seq, super::web_math::SurfaceHealthEvent::Resized);
    was_blocked
}

/// Acquires the next canvas texture, reconfiguring and retrying once on `Outdated`/`Lost`.
pub(super) fn acquire_texture(side: &mut GpuSide) -> Result<wgpu::SurfaceTexture, crate::window::Missed> {
    use super::web_math::{WebAcquireAction, WebAcquireStatus, web_acquire_action};
    let mut retries = 1u8;
    loop {
        let (frame, status) = match side.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                (Some(frame), WebAcquireStatus::Ready)
            }
            wgpu::CurrentSurfaceTexture::Timeout => (None, WebAcquireStatus::Timeout),
            wgpu::CurrentSurfaceTexture::Occluded => (None, WebAcquireStatus::Occluded),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                (None, WebAcquireStatus::OutdatedOrLost)
            }
            wgpu::CurrentSurfaceTexture::Validation => (None, WebAcquireStatus::Validation),
        };
        match web_acquire_action(status, retries) {
            WebAcquireAction::UseTexture => return Ok(frame.expect("ready acquisition carries texture")),
            WebAcquireAction::ReconfigureAndRetry => {
                configure_surface(side);
                if !can_draw(side) {
                    return Err(crate::window::Missed::Occluded);
                }
                retries -= 1;
            }
            WebAcquireAction::Miss(missed) => return Err(missed),
        }
    }
}

/// GPU frame resources prepared before encoding and submission.
pub(super) struct PreparedGpu {
    frame: wgpu::SurfaceTexture,
    encoder: wgpu::CommandEncoder,
    size: (u32, u32),
    armed: bool,
    readback: bool,
    copy: Option<Readback>,
}

/// Uploads dirty images, registers the renderer's frame wake, arms a cycle-only frame when `tick`
/// applies, and opens the command encoder.
pub(super) fn prepare_gpu_frame(
    side: &mut GpuSide,
    rt: &mut RuntimeCore,
    frame: wgpu::SurfaceTexture,
    readback: bool,
    tick: bool,
) -> PreparedGpu {
    let size = (side.config.width, side.config.height);
    side.renderer.upload_images(&side.gpu.device, &side.gpu.queue, &mut rt.images);
    side.renderer.set_frame_wake(rt.live_wake());
    // A loop-only step (`runtime/paint/cycle.rs`) writes only the dots it changed, and redraws
    // only where they are, into the last frame the renderer keeps, which is copied to the canvas.
    let (paint, slots) = rt.cycle_paint();
    let armed = tick && side.config.usage.contains(wgpu::TextureUsages::COPY_DST)
        && side.renderer.arm_cycle(&side.gpu.device, &side.gpu.queue, paint, &rt.instances, &slots, rt.clear_color(), size, side.config.format).is_ok();
    side.renderer.next_instances(paint, tick.then_some(slots));
    let encoder = side.gpu.device.create_command_encoder(&Default::default());
    PreparedGpu { frame, encoder, size, armed, readback, copy: None }
}

/// Encodes the UI or custom compositor pass (plus optional hand-over drift readback copy) into
/// `prepared.encoder`. Returns `true` when a custom compositor reported it is still animating.
pub(super) fn encode_gpu_frame(
    side: &mut GpuSide,
    app: &mut dyn WebApp,
    prepared: &mut PreparedGpu,
    dt: f64,
    update_ms: f64,
) -> bool {
    let comp_animating = if prepared.armed {
        // Rune's default frame (a compositor that drew its own would have kept this a whole one):
        // nothing draws into the canvas but the copy of the kept frame.
        let rt = app.rt_mut();
        side.renderer.set_fade_stops(rt.fade_stops());
        side.renderer.prepare_ui(&side.gpu.device, &side.gpu.queue, &mut rt.text.atlas, &rt.instances, prepared.size);
        side.renderer.draw_armed(&mut prepared.encoder, &prepared.frame.texture);
        false
    } else if app.has_compositor() {
        let target = prepared.frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(side.renderer.format()), ..Default::default() });
        app.render_compositor(CompositorFrame {
            device: &side.gpu.device,
            queue: &side.gpu.queue,
            encoder: &mut prepared.encoder,
            target: &target,
            format: side.renderer.format(),
            size: prepared.size,
            dt,
            update_ms,
            ui: &mut side.renderer,
        })
    } else {
        let target = prepared.frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(side.renderer.format()), ..Default::default() });
        let rt = app.rt_mut();
        let clear = rt.clear_color();
        crate::frame::host_step::draw_ui(&mut side.renderer, &side.gpu, &mut prepared.encoder, &target, rt, prepared.size, clear);
        false
    };
    // A copy of the frame for counting how many of its pixels the CPU's differ in (drift).
    if prepared.readback && side.config.usage.contains(wgpu::TextureUsages::COPY_SRC) {
        let (width, height) = (side.config.width, side.config.height);
        let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = side.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rune hand-over readback"),
            size: u64::from(row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        prepared.encoder.copy_texture_to_buffer(
            prepared.frame.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let poll_device = super::web_math::needs_device_poll(side.backend).then(|| side.gpu.device.clone());
        prepared.copy = Some(Readback {
            buffer,
            row,
            size: (width, height),
            rgba: matches!(side.config.format, wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb),
            poll_device,
        });
    }
    // Every frame: one drawn whole leaves the kept frame behind.
    side.renderer.damage_drawn().ok();
    comp_animating
}

/// Submits the encoded GPU commands, runs `before_present` (e.g. `pre_present_notify`), and
/// presents the canvas texture.
pub(super) fn submit_gpu_frame(
    side: &mut GpuSide,
    prepared: PreparedGpu,
    before_present: impl FnOnce(),
) -> Option<Readback> {
    side.renderer.submit(&side.gpu.queue, prepared.encoder);
    before_present();
    side.gpu.queue.present(prepared.frame);
    prepared.copy
}

/// The GPU's hand-over frame, copied where the CPU can read it.
pub(super) struct Readback {
    buffer: wgpu::Buffer,
    row: u32,
    size: (u32, u32),
    /// The canvas's format is RGBA (the CPU paints BGRA).
    rgba: bool,
    /// Present only on backends whose `map_async` needs explicit `Device::poll` (WebGL2).
    poll_device: Option<wgpu::Device>,
}

impl Readback {
    /// Counts the pixels of `cpu` (BGRA, `size.0 * 4` bytes a row) this frame drew otherwise, and
    /// appends `[differing, of, max]` to `globalThis.__runeDrift` once the copy is mapped.
    pub(super) fn count(self, cpu: Vec<u8>) {
        let Readback { buffer, row, size: (w, h), rgba, poll_device } = self;
        let buffer = Rc::new(buffer);
        let done = Rc::new(Cell::new(false));
        let state = PageOnly((buffer.clone(), done.clone()));
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let PageOnly((mapped, mark_done)) = &state;
            mark_done.set(true);
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
        if let Some(device) = poll_device {
            poll_until_done(device, &done, Instant::now() + FINISH_LIMIT);
        }
    }
}

/// Whether the page asked for the hand-over's drift (`globalThis.__runeDrift`, an array).
pub(super) fn drift_wanted() -> bool {
    js_sys::Reflect::get(&js_sys::global(), &"__runeDrift".into()).is_ok_and(|v| js_sys::Array::is_array(&v))
}

/// A flag the GPU's queue sets once the work submitted so far is done, waking the loop.
/// On WebGL2 (`wgpu-core`), schedules lightweight non-blocking `Device::poll` ticks until the
/// callback fires or `FINISH_LIMIT` passes; on WebGPU the browser resolves it from its task queue.
pub(super) fn when_done(side: &GpuSide, proxy: &EventLoopProxy<Wake>) -> Rc<Cell<bool>> {
    let flag = Rc::new(Cell::new(false));
    let set = PageOnly((flag.clone(), proxy.clone()));
    side.gpu.queue.on_submitted_work_done(move || {
        let PageOnly((flag, proxy)) = &set;
        flag.set(true);
        proxy.send_event(Wake::App).ok();
    });
    if super::web_math::needs_device_poll(side.backend) {
        poll_until_done(side.gpu.device.clone(), &flag, Instant::now() + FINISH_LIMIT);
    }
    flag
}

/// Interval between WebGL2 `Device::poll` checks while an async wgpu callback is pending.
const POLL_INTERVAL: Duration = Duration::from_millis(8);

/// Drives `wgpu-core` callbacks on WebGL2 after yielding to the browser event loop (WebGL2 sync
/// objects only transition to `SIGNALED` across turns). Stops as soon as `flag` flips, is dropped,
/// the device reports an error, or `deadline` passes.
fn poll_until_done(device: wgpu::Device, flag: &Rc<Cell<bool>>, deadline: Instant) {
    fn step(device: wgpu::Device, weak: std::rc::Weak<Cell<bool>>, deadline: Instant) {
        let Some(flag) = weak.upgrade() else { return };
        if flag.get() || Instant::now() >= deadline { return; }
        if device.poll(wgpu::PollType::Poll).is_err() || flag.get() { return; }
        super::after(POLL_INTERVAL, move || step(device, weak, deadline));
    }
    let weak = Rc::downgrade(flag);
    super::after(Duration::ZERO, move || step(device, weak, deadline));
}

/// A value moved into a callback wgpu wants `Send`. In a page that callback runs on the page's
/// one thread, from a promise.
struct PageOnly<T>(T);
// SAFETY: wasm32 without threads: there is one thread, and every callback runs on it.
#[allow(unsafe_code)]
unsafe impl<T> Send for PageOnly<T> {}

/// How long the GPU's hand-over frame may take to finish before the page stays on the CPU.
pub(super) const FINISH_LIMIT: Duration = Duration::from_secs(5);
