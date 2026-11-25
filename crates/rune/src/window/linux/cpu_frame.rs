//! Linux: a window that is live on the CPU from its first frame, and hands its frames to the GPU
//! once one is ready (`handover`). The design is Windows' (`window::win32::cpu_frame`).
//!
//! The window opens with no GPU: no Vulkan driver is loaded, no device or pipeline made. The
//! CPU renderer (`render::cpu_boot`) paints the real view into a buffer the display takes as it
//! is (`platform::cpu_frames`: a `wl_shm` buffer on Wayland, an image put to the window on X11),
//! and the window comes on screen with that frame. Input, hover, typing and animation run as on
//! any frame. The view's text and images stay where the CPU frames put them: masks the CPU filled
//! upload on the GPU's first frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use super::handover::{GpuJob, Latest, SurfaceMode};
use crate::frame::clock::FrameClock;
use crate::frame::host_step::{self, Acquire, FrameHost, Gate, RenderPrep, StepInfo};
use crate::platform::cpu_frames::{Frames, Put, Retired};
use crate::render::cpu_boot::{CpuPainter, HalfRounding, Scene, Target};
use crate::window::CpuFirst;
use crate::runtime::RuntimeCore;
use crate::time::Instant;
use crate::window::chrome::{sync_chrome_core, sync_platform_core};
use crate::window::handle::native_window_number;
use crate::window::host::Host;
use crate::window::host::stepping::FrameMove;
use crate::window::state::FirstFrame;
use crate::window::warm_frame::warm_instances;
use crate::window::surface::{
    Backoff, BackoffEvent, FRAME_LATENCY, HiddenPace, LoopFrame, Native, Presented, Redraw,
    SurfaceRetries, TIMER_FRAME, TimerFrame, Win, drawable_size, max_surface_dim,
};
use crate::window::sync_screen_origin_core;

/// The CPU renderer's side of a window: where its frames go and the painter that knows what
/// the buffer last held.
pub(in crate::window) struct Cpu {
    pub(super) frames: Frames,
    painter: CpuPainter,
    /// How the GPU that takes over rounds half floats.
    half: HalfRounding,
    /// The radius the frames' bottom corners were last cleared at (`clear_corners`).
    radius: f32,
}

/// A window's CPU side after the window went to the GPU (`Cpu::retire`): it draws nothing and
/// can send the display nothing. What it showed last stays readable, and its buffers stay alive,
/// until the GPU's first frame is on screen.
pub(in crate::window) struct RetiredCpu {
    pub(super) frames: Retired,
    painter: CpuPainter,
    half: HalfRounding,
    radius: f32,
}

impl Cpu {
    /// The window goes to the GPU (`handover::SurfaceMode::switch`).
    pub(super) fn retire(self) -> RetiredCpu {
        RetiredCpu { frames: self.frames.retire(), painter: self.painter, half: self.half, radius: self.radius }
    }
}

#[cfg(test)]
impl Cpu {
    /// A CPU side with no display behind it, and the count of frames a display would have had.
    pub(super) fn fake(explicit_sync: bool) -> (Cpu, Arc<std::sync::atomic::AtomicUsize>) {
        let (frames, sent) = crate::platform::cpu_frames::fake::Frames::new(explicit_sync);
        (Cpu { frames, painter: CpuPainter::new(1), half: HalfRounding::default(), radius: 0. }, sent)
    }
}

impl RetiredCpu {
    /// The CPU side back, where the display allows that (`Retired::revive`); its next frame is
    /// painted and sent whole.
    pub(super) fn revive(self) -> Result<Cpu, RetiredCpu> {
        let RetiredCpu { frames, mut painter, half, radius } = self;
        match frames.revive() {
            Ok(mut frames) => {
                frames.forget();
                painter.forget();
                Ok(Cpu { frames, painter, half, radius })
            }
            Err(frames) => Err(RetiredCpu { frames, painter, half, radius }),
        }
    }
}

/// Threads the CPU renderer paints on: the UI thread and up to five workers that park between
/// frames.
fn painter_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 6)
}

/// Pure Linux rule for whether a window opens on the CPU renderer.
///
/// Under `CpuFirst::Auto` + `Policy::Automatic`, a live `DeviceType::Cpu` (lavapipe/llvmpipe)
/// device in `gpu_pool` (created by a transparent window or an earlier `NeedsGpu` window) does
/// not pull a second opaque window onto the software GPU. Hardware pool GPUs, pinned adapters,
/// transparent windows, `CpuFirst::Off`, and `CpuFirst::Only` behave identically to before.
pub(super) const fn linux_opens_on_cpu(
    mode: CpuFirst,
    policy: crate::gpu::fallback::Policy,
    transparent: bool,
    has_gpu: bool,
    pool_gpu: Option<wgpu::DeviceType>,
) -> bool {
    let live_pool_gpu = match (mode, policy, pool_gpu) {
        (CpuFirst::Auto, crate::gpu::fallback::Policy::Automatic, Some(wgpu::DeviceType::Cpu)) => false,
        (_, _, Some(_)) => true,
        (_, _, None) => false,
    };
    mode.opens_on_cpu(transparent, has_gpu, live_pool_gpu)
}

/// Whether a window opens on the CPU: an opaque window without its own GPU, following
/// [`Host::effective_cpu_first`] (`RUNE_CPU_FIRST` overrides `App::cpu_first` /
/// `WindowSpec::cpu_first`).
pub(in crate::window) fn opens_on_cpu(host: &Host) -> bool {
    let pool_gpu = host
        .gpu_pool
        .borrow()
        .upgrade()
        .filter(|gpu| !gpu.is_lost())
        .map(|gpu| gpu.adapter.get_info().device_type);
    linux_opens_on_cpu(
        host.effective_cpu_first(),
        crate::gpu::pick::env_policy(),
        host.options.transparent,
        host.gpu.is_some(),
        pool_gpu,
    )
}

impl Host {
    /// The rest of `try_create` for an opaque window that opens on the CPU: no GPU, no surface. A
    /// Wayland window comes on screen with its first frame (it has no buffer, and so isn't
    /// shown, before). `Err` gives the window back when the display takes no CPU frames, for
    /// `try_create` to open it on the GPU.
    pub(in crate::window) fn open_on_cpu(&mut self, event_loop: &ActiveEventLoop, window: Arc<Window>, a11y: Option<crate::a11y::Adapter>, built: bool) -> Result<(), (Arc<Window>, Option<crate::a11y::Adapter>)> {
        self.open_on_cpu_with_job(event_loop, window, a11y, built, GpuJob::Waiting { tries: 0, retry_at: None })
    }

    /// Fallback for a transparent window whose pre-swapchain GPU startup failed: opens on the CPU
    /// with alpha buffers (`wl_shm` ARGB8888 or X11 32-bit ARGB, falling back to 24-bit opaque
    /// with the theme background if the display gives no alpha) and `GpuJob::Failed` so Vulkan
    /// is never retried in a loop.
    pub(in crate::window) fn open_on_cpu_fallback(&mut self, event_loop: &ActiveEventLoop, window: Arc<Window>, a11y: Option<crate::a11y::Adapter>, built: bool) -> Result<(), (Arc<Window>, Option<crate::a11y::Adapter>)> {
        self.open_on_cpu_with_job(event_loop, window, a11y, built, GpuJob::Failed)
    }

    fn open_on_cpu_with_job(&mut self, event_loop: &ActiveEventLoop, window: Arc<Window>, a11y: Option<crate::a11y::Adapter>, built: bool, job: GpuJob) -> Result<(), (Arc<Window>, Option<crate::a11y::Adapter>)> {
        let phys = window.inner_size();
        let size = drawable_size(None, (phys.width, phys.height), max_surface_dim(None));
        // A client-side frame's window keeps its pixels' alpha for the frame's corner arcs; a
        // transparent window asks for alpha buffers too (falling back to 24-bit opaque on X11
        // servers without a 32-bit ARGB visual).
        let client_frame = self.client_frame(&window);
        let want_alpha = client_frame || self.options.transparent;
        let Some(frames) = Frames::new(&window, size, want_alpha) else { return Err((window, a11y)) };
        let has_alpha = frames.has_alpha();
        crate::render::cpu_boot::attach_text(&mut self.app.rt_mut().text.atlas);
        let cpu = Cpu { frames, painter: CpuPainter::new(painter_threads()), half: HalfRounding::default(), radius: 0. };
        crate::marks::mark("cpu.side");
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8Unorm,
            width: size.0,
            height: size.1,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: FRAME_LATENCY,
            alpha_mode: if has_alpha { wgpu::CompositeAlphaMode::PreMultiplied } else { wgpu::CompositeAlphaMode::Opaque },
            view_formats: vec![],
            color_space: Default::default(),
        };
        if self.options.transparent {
            if !has_alpha {
                eprintln!("transparency unavailable for {}; using an opaque window", self.native_handle.key());
            }
            self.app.rt_mut().transparent = has_alpha;
        }
        let scale = window.scale_factor() as f32;
        {
            let rt = self.app.rt_mut();
            rt.set_system_dark(crate::platform::preferences::window_dark(&window, self.args.theme.as_deref()));
            rt.set_viewport((size.0 as f32 / scale, size.1 as f32 / scale), scale);
            rt.set_renderer_kind(crate::RendererKind::Cpu);
            // Reopening paints the settled state in the first frame (guarantee 8).
            if self.shown && !built { rt.settle_all(); }
        }
        self.shown = true;
        self.life.show();
        self.idle = true;
        self.app.set_live_visible(true);
        *self.native_handle.native.window.lock().unwrap() = Arc::downgrade(&window);
        self.native_handle.native.number.store(native_window_number(&window).unwrap_or(0), Ordering::Release);
        self.win = Some(Win {
            a11y,
            native: Native { mode: SurfaceMode::Cpu { cpu, job }, ..Default::default() },
            first_frame: FirstFrame::opened(crate::time::now()),
            surface: None,
            config,
            surface_ok: true,
            surface_retries: SurfaceRetries::default(),
            window,
            backoff: Backoff::default(),
            hidden_pace: HiddenPace::default(),
            cursor: crate::cursor::os::OsCursor::new(),
            alpha: None,
            corners: None,
            timer_frame: TimerFrame::default(),
            loop_frame: LoopFrame::default(),
            presented: None,
            redraw: std::cell::Cell::new(Redraw::Idle),
        });
        self.sync_window_backdrop(false);
        self.apply_placement(event_loop);
        if !self.announce_created(false) { return Ok(()); }
        if let Some(w) = self.win.as_mut() {
            let size = w.window.inner_size();
            let scale = w.window.scale_factor() as f32;
            let rt = self.app.rt_mut();
            if size.width > 0 && size.height > 0 {
                w.resize_surface(None, size);
                rt.set_viewport((w.config.width as f32 / scale, w.config.height as f32 / scale), scale);
            }
            sync_chrome_core(rt, w, self.chrome, self.titlebar);
            sync_screen_origin_core(rt, &w.window);
            if w.window.has_focus() { self.app.window_focus(true); }
        }
        // The first frame comes with the loop's redraw, as a GPU window's does here: winit draws
        // a Wayland client-side frame (its title bar and shadow, on subsurfaces that show with
        // the window's next commit) just before it asks for that redraw, so the commit that
        // puts the window on screen carries them. Drawn here, the window stood without its
        // title bar until its next frame.
        if let Some(w) = &self.win { w.request_redraw(); }
        Ok(())
    }

    /// A frame drawn by the CPU renderer: the runtime steps, the painter repaints the tiles that
    /// changed, and only those rectangles go to the display.
    pub(in crate::window) fn draw_cpu(&mut self, resizing: bool) {
        let mut frame = CpuWindowFrame { host: self, resizing, owed: false, now: None, encoded: None, sent: false };
        host_step::run(&mut frame);
    }
}

struct CpuWindowFrame<'a> {
    host: &'a mut Host,
    resizing: bool,
    /// The display asked for the window's pixels again: the whole frame goes.
    owed: bool,
    now: Option<Instant>,
    encoded: Option<Instant>,
    /// This frame sent pixels to the display.
    sent: bool,
}

impl FrameHost for CpuWindowFrame<'_> {
    type Drawable = ();
    type Prepared = Option<Vec<[u32; 4]>>;

    fn gate(&mut self) -> Gate {
        let Some(l) = self.host.win.as_mut() else { return Gate::Skip };
        let Some(cpu) = l.native.mode.cpu() else { return Gate::Skip };
        // The display threw away what the window showed and asks for it again: the window is in
        // sight, so this frame is drawn and sent whole.
        let owed = cpu.frames.exposed(false);
        let covered = !owed && (!self.host.life.uncovered() || l.minimized());
        if covered {
            self.host.covered_frame();
            return Gate::Skip;
        }
        let Some(l) = self.host.win.as_mut() else { return Gate::Skip };
        if !owed && !self.resizing && l.backing_off(crate::time::now()) { return Gate::Skip; }
        l.timer_frame.drawn();
        if self.host.native_handle.native.number.load(Ordering::Relaxed) == 0 {
            self.host.native_handle.native.number.store(native_window_number(&l.window).unwrap_or(0), Ordering::Release);
        }
        self.host.meter.redraw(l.paced_for_stats());
        self.owed = owed;
        Gate::Draw
    }

    fn before_acquire(&mut self) {
        if !self.resizing { self.host.step_frame(crate::time::now()); }
        let Some(l) = self.host.win.as_mut() else { return };
        l.backoff_step(BackoffEvent::Lapsed);
        l.backoff_step(BackoffEvent::Presented);
        if self.host.frame_reports.wanted() {
            self.host.frame_reports.set_refresh(l.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).map(|hz| hz as f64 / 1000.0));
        }
    }

    fn acquire(&mut self) -> Acquire<()> { Acquire::Ready(()) }

    fn step_runtime(&mut self) -> StepInfo {
        let now = crate::time::now();
        self.now = Some(now);
        let (animating, dt) = self.host.app.clock_advance(&mut self.host.clock, now, false);
        crate::marks::mark_once(&FIRST_LAYOUT, "frame.first.layout");
        StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.0, presents_at: None }
    }

    fn sync_after_step(&mut self, info: &mut StepInfo) {
        let now = self.now.unwrap_or_else(crate::time::now);
        info.update_ms = now.elapsed().as_secs_f64() * 1000.0;
        let Some(l) = self.host.win.as_mut() else { return };
        sync_platform_core(l, self.host.app.rt_mut(), &mut self.host.secure_input, &mut self.host.ime, self.host.ime_waits, &self.host.clipboard);
        self.host.sync_wayland_drop();
    }

    fn prepare_render(&mut self, _drawable: (), _info: &mut StepInfo) -> RenderPrep<Option<Vec<[u32; 4]>>> {
        let has_comp = self.host.app.has_compositor();
        let radius = self.host.win.as_ref().map_or(0., |l| self.host.corner_radius(l));
        let Some(l) = self.host.win.as_mut() else { return RenderPrep::Abort };
        let (width, height) = (l.config.width, l.config.height);
        let rt = self.host.app.rt_mut();
        // What the window draws now, for the GPU thread's warm frame (`handover::Latest`).
        if let SurfaceMode::Cpu { job: GpuJob::Building { latest, .. }, .. } = &l.native.mode {
            *latest.lock().unwrap_or_else(|e| e.into_inner()) = Latest { instances: warm_instances(&rt.instances), size: (width, height) };
        }
        let gpu_failed = matches!(l.native.mode, SurfaceMode::Cpu { job: GpuJob::Failed, .. });
        let Some(cpu) = l.native.mode.cpu() else { return RenderPrep::Abort };
        cpu.painter.fill_masks(&mut rt.text.atlas);
        crate::marks::mark_once(&FIRST_MASKS, "frame.first.masks");
        let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (width, height), rt.clear_color())
            .srgb_blend(rt.srgb_blend())
            .fade_stops(rt.fade_stops())
            .native_surfaces(!rt.surfaces.is_empty())
            .compositor(has_comp)
            .half_rounding(cpu.half)
            .font_smoothing(rt.font_smoothing());
        // The corners' arcs moved (the window was maximized, or left it): every pixel is painted
        // again, as nothing says which the last clearing changed.
        if cpu.radius != radius {
            cpu.radius = radius;
            cpu.frames.forget();
        }
        let Cpu { frames, painter, .. } = &mut *cpu;
        let Some(canvas) = frames.begin((width, height)) else {
            // The compositor holds every buffer, or there's no memory for one: tried again soon.
            l.backoff_step(BackoffEvent::At(crate::time::now() + TIMER_FRAME));
            return RenderPrep::Abort;
        };
        if !canvas.kept { painter.forget(); }
        let (kept, stride) = (canvas.kept, canvas.stride);
        let painted = painter.paint(&scene, &mut Target::with_stride(&mut *canvas.bytes, width, height, stride));
        match painted {
            Ok(painted) => {
                clear_corners(canvas.bytes, stride, (width, height), radius, &painted.rects);
                crate::marks::mark_once(&FIRST_PAINT, "frame.first.painted");
                if !kept { self.owed = true; }
                RenderPrep::Ready(Some(painted.rects))
            }
            Err(why) => {
                // When the GPU has permanently failed (no Vulkan driver, or transparent CPU
                // fallback), paint degradable NeedsGpu features without their GPU-only modifier
                // instead of leaving the window blank/frozen forever.
                if CpuFirst::linux_degraded_when_gpu_failed(gpu_failed, why)
                    && let Ok(painted) = painter.paint(&scene.degraded(true), &mut Target::with_stride(&mut *canvas.bytes, width, height, stride))
                {
                    clear_corners(canvas.bytes, stride, (width, height), radius, &painted.rects);
                    crate::marks::mark_once(&FIRST_PAINT, "frame.first.painted");
                    if !kept { self.owed = true; }
                    return RenderPrep::Ready(Some(painted.rects));
                }
                // The frame needs what only the GPU draws (a native surface, a compositor's
                // scene): the GPU starts now if it hasn't, even a software one, and the window
                // keeps its last frame until it's there.
                crate::marks::mark_once(&NEEDS_GPU, "cpu_first.needs_gpu");
                self.host.start_gpu_for_needs_gpu();
                self.host.idle = false;
                if let Some(l) = self.host.win.as_mut() { l.backoff_step(BackoffEvent::At(crate::time::now() + TIMER_FRAME)); }
                RenderPrep::Abort
            }
        }
    }

    fn render(&mut self, prepared: &mut Option<Vec<[u32; 4]>>, _info: &StepInfo) -> bool {
        let Some(rects) = prepared.as_deref() else { return false };
        let Some(l) = self.host.win.as_mut() else { return false };
        let Some(cpu) = l.native.mode.cpu() else { return false };
        // A frame that changed nothing looks again, having waited for the display, whether it
        // wants the window's pixels: the request that asked for this frame may be why.
        let owed = self.owed || (rects.is_empty() && cpu.frames.exposed(true));
        let put = if owed { Put::All } else { Put::Rects(rects) };
        // winit asks the compositor for the next frame's time with what is committed now.
        if owed || !rects.is_empty() { l.window.pre_present_notify(); }
        self.sent = cpu.frames.present(put);
        false
    }

    fn submit_and_present(&mut self, prepared: Option<Vec<[u32; 4]>>, _info: &StepInfo) -> Instant {
        self.encoded = self.host.frame_reports.stamp();
        crate::window::frame_presented();
        if prepared.is_some() && let Some(l) = self.host.win.as_mut() {
            crate::marks::mark_once(&FIRST_CPU, "frame.first.cpu");
            if !l.first_frame.ready() { l.first_frame.gpu_done(); }
        }
        crate::time::now()
    }

    fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore) {
        (Some(&mut self.host.clock), self.host.app.rt_mut())
    }

    fn settle(&mut self, submitted: Instant, info: &StepInfo) {
        let now = self.now.unwrap_or(submitted);
        self.host.meter.presented();
        crate::haptics::flush_live(self.host.app.rt_mut(), false);
        self.host.frame_reports.drawn(self.encoded, None, now, info.update_ms);
        self.host.app.inspect_frame();
        let Some(l) = self.host.win.as_mut() else { return };
        let next_tick = l.next_tick(now);
        self.host.idle = Presented::settle_core(
            &mut l.presented,
            self.host.app.rt(),
            &l.config,
            info.animating || FrameMove::moving(&self.host.frame_move),
            next_tick,
            true,
            false,
        );
        // Motion's next frame: a Wayland compositor says when (its frame callback holds winit's
        // redraw until then), if this frame sent it anything; otherwise the display's refresh
        // interval from now, as nothing else paces a CPU frame.
        let paced = self.sent && l.native.mode.cpu().is_some_and(|cpu| cpu.frames.paced());
        if !self.host.idle && !paced {
            let hz = l.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).map_or(60., |mhz| f64::from(mhz) / 1000.);
            l.backoff_step(BackoffEvent::At(crate::time::now() + std::time::Duration::from_secs_f64(1. / hz.clamp(24., 480.))));
        }
        self.host.meter.set_pacing(l.pacing_state(!self.host.idle));
    }
}

/// What `render::csd_corners` does to a GPU frame, to the pixels a CPU frame just painted
/// (`rects`): outside the arcs of the two bottom corners (`radius` device pixels) the frame
/// composites nothing, and a pixel an arc crosses keeps the arc's coverage of it.
pub(super) fn clear_corners(bytes: &mut [u8], stride: usize, size: (u32, u32), radius: f32, rects: &[[u32; 4]]) {
    if radius <= 0. { return; }
    let (width, height) = (size.0 as f32, size.1 as f32);
    let reach = radius.ceil() as u32;
    let painted = |x: u32, y: u32| rects.iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]);
    for (centre, xs) in [((radius, height - radius), 0..reach.min(size.0)), ((width - radius, height - radius), size.0.saturating_sub(reach)..size.0)] {
        for y in size.1.saturating_sub(reach)..size.1 {
            for x in xs.clone() {
                let at = (x as f32 + 0.5, y as f32 + 0.5);
                // The corner's square: from the arc's centre out to the window's edges.
                let inside = at.1 >= centre.1 && if centre.0 == radius { at.0 <= centre.0 } else { at.0 >= centre.0 };
                if !inside || !painted(x, y) { continue; }
                let distance = ((at.0 - centre.0).powi(2) + (at.1 - centre.1).powi(2)).sqrt();
                let t = (distance - (radius - 0.5)).clamp(0., 1.);
                let keep = 1. - t * t * (3. - 2. * t);
                if keep >= 1. { continue; }
                let pixel = y as usize * stride + x as usize * 4;
                for byte in &mut bytes[pixel..pixel + 4] { *byte = (f32::from(*byte) * keep + 0.5) as u8; }
            }
        }
    }
}

static FIRST_CPU: AtomicBool = AtomicBool::new(false);
static FIRST_LAYOUT: AtomicBool = AtomicBool::new(false);
static FIRST_MASKS: AtomicBool = AtomicBool::new(false);
static FIRST_PAINT: AtomicBool = AtomicBool::new(false);
static NEEDS_GPU: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
#[path = "cpu_frame_tests.rs"]
mod tests;
