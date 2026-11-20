//! Windows: a window that is live on the CPU from its first frame, and hands its frames to the GPU
//! once one is ready, without a seam (`handover`).
//!
//! The window opens with no GPU: the CPU renderer (`render::cpu_boot`) paints the real view into a
//! DIB section, the changed rectangles are copied to the window with GDI, and the window shows
//! with that frame in it. Input, hover, typing and animation run as on any frame. The view's text
//! and images stay where the CPU frames put them: masks the CPU filled upload on the GPU's first
//! frame, so the swap repaints nothing.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use super::covered_draw::{CoveredDraw, CoveredDrawEvent};
use super::handover::{GpuJob, SurfaceMode, warm_instances};
use crate::frame::clock::FrameClock;
use crate::frame::host_step::{self, Acquire, FrameHost, Gate, RenderPrep, StepInfo};
use crate::platform::win32::dib::{Blit, Dib, Prefault};
use crate::render::cpu_boot::{CpuPainter, HalfRounding, Scene, Target};
use crate::runtime::RuntimeCore;
use crate::time::Instant;
use crate::window::chrome::{self, sync_chrome_core, sync_platform_core};
use crate::window::handle::native_window_number;
use crate::window::host::Host;
use crate::window::host::stepping::FrameMove;
use crate::window::state::FirstFrame;
use crate::window::surface::{
    Backoff, BackoffEvent, FRAME_LATENCY, HiddenPace, LoopFrame, Native, Presented, Redraw,
    SurfaceRetries, TIMER_FRAME, TimerFrame, Win, native, win,
};
use crate::window::{WakeEvent, sync_screen_origin_core};

/// The CPU renderer's side of a window: the buffer it paints in place and the painter that
/// knows what the buffer last held.
pub(in crate::window) struct Cpu {
    buffer: Buffer,
    painter: CpuPainter,
    /// How the GPU that takes over rounds half floats: the primary display's GPU's until that GPU
    /// exists, then its own.
    pub(super) half: HalfRounding,
    /// What the window has of the buffer.
    pub(super) shown: Shown,
    /// Until the first frame is painted: the window shows with it, then, activating when `true`.
    show: Option<bool>,
}

/// What a CPU window has of its buffer. GDI draws nothing into a hidden window, so the first
/// frame waits in the buffer until the window shows (cloaked: DWM composes it only once it's in).
pub(super) enum Shown {
    /// Nothing painted yet.
    Nothing,
    /// The first frame is painted; the window gets all of it when it shows.
    Waiting,
    /// The last frame copied was `size` big.
    Copied((u32, u32)),
}

/// A window's frame buffer, as big as the window's display (or the window, if larger): a new one
/// has the first frame's rows touched on a worker while that frame is laid out, then it's the
/// window's for every size that fits, with one row length.
enum Buffer {
    /// Being touched; `capacity` is its size.
    Touching(Prefault, (u32, u32)),
    Ready(Dib),
    /// GDI had no memory for one: the next frame tries again.
    Missing,
}

/// The buffer for a client area of `size`: the one there is while it fits; a bigger one (the
/// painter then repaints every tile) when the window outgrew it.
fn ready<'a>(buffer: &'a mut Buffer, painter: &mut CpuPainter, size: (u32, u32)) -> Option<&'a mut Dib> {
    let was = std::mem::replace(buffer, Buffer::Missing);
    let grown = |at: (u32, u32), painter: &mut CpuPainter| {
        painter.forget();
        Dib::new(size.0.max(at.0), size.1.max(at.1)).map_or(Buffer::Missing, Buffer::Ready)
    };
    *buffer = match was {
        Buffer::Ready(dib) if dib.fits(size) => Buffer::Ready(dib),
        Buffer::Ready(dib) => grown((dib.stride() as u32 / 4, 0), painter),
        Buffer::Touching(prefault, at) if size.0 <= at.0 && size.1 <= at.1 => match prefault.take() {
            Some(dib) => Buffer::Ready(dib),
            None => grown(at, painter),
        },
        Buffer::Touching(_, at) => grown(at, painter),
        Buffer::Missing => grown((0, 0), painter),
    };
    match buffer {
        Buffer::Ready(dib) => Some(dib),
        Buffer::Touching(..) | Buffer::Missing => None,
    }
}

impl Cpu {
    pub(super) fn into_dib(self) -> Option<Dib> {
        match self.buffer {
            Buffer::Ready(dib) => Some(dib),
            Buffer::Touching(prefault, _) => prefault.take(),
            Buffer::Missing => None,
        }
    }
}

/// The last frame copied to the window, whole, again (`draw_cpu`).
fn copy_last(cpu: &Cpu, window: &Window) {
    let (Shown::Copied(size), Buffer::Ready(dib)) = (&cpu.shown, &cpu.buffer) else { return };
    if let Some(hwnd) = crate::platform::win32::hwnd(window) { dib.present(hwnd, *size, Blit::All); }
}

/// The first frame, painted while the window was hidden, into the window that now shows, whole
/// (nothing composes the window before its cloak lifts).
fn copy_first(cpu: &mut Cpu, window: &Window, size: (u32, u32)) {
    let (Shown::Waiting, Buffer::Ready(dib)) = (&cpu.shown, &cpu.buffer) else { return };
    let Some(hwnd) = crate::platform::win32::hwnd(window) else { return };
    dib.present(hwnd, size, Blit::All);
    cpu.shown = Shown::Copied(size);
    crate::marks::mark("frame.first.copied");
}

/// Threads the CPU renderer paints on: the UI thread and up to five workers that park between
/// frames.
fn painter_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 6)
}

/// Whether a window opens on the CPU: an opaque window while no window has a GPU yet (with one,
/// a new window's swap chain is ready in a few milliseconds), unless turned off
/// (`RUNE_CPU_FIRST=0`).
pub(in crate::window) fn opens_on_cpu(host: &Host) -> bool {
    !host.options.transparent && host.gpu.is_none()
        && host.gpu_pool.borrow().upgrade().is_none_or(|gpu| gpu.is_lost())
        && std::env::var_os("RUNE_CPU_FIRST").is_none_or(|v| v != "0")
}

impl Host {
    /// The CPU side for a new window of `size` physical pixels on a display of `display`: a
    /// buffer as big as the larger, whose first frame's rows a worker touches while that frame is
    /// laid out, and a painter. The text system queues glyph masks for the painter's fill from
    /// now on. Returns the buffer's size with it.
    pub(in crate::window) fn cpu_side(&mut self, size: (u32, u32), display: (u32, u32)) -> Option<(Prefault, (u32, u32), CpuPainter)> {
        let capacity = (size.0.max(display.0), size.1.max(display.1));
        let dib = Dib::new(capacity.0, capacity.1)?;
        crate::render::cpu_boot::attach_text(&mut self.app.rt_mut().text.atlas);
        Some((Prefault::start(dib, size.1), capacity, CpuPainter::new(painter_threads())))
    }

    /// The rest of `try_create` for a window that opens on the CPU: no GPU, no surface. Its first
    /// frame paints into the buffer and the window shows with it, activating when it should
    /// (`focus`); a first frame only the GPU can draw shows cloaked until the GPU has drawn it.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::window) fn open_on_cpu(&mut self, event_loop: &ActiveEventLoop, window: Arc<Window>, a11y: Option<crate::a11y::Adapter>, a11y_start: native::A11yStart,
        frame: Option<crate::platform::win32::CustomFrame>, cloak: crate::platform::win32::Cloak, focus: bool, open_as: crate::windows::OpenAs, built: bool) -> Result<(), String> {
        // Maximizing (or full screen) shows a window: cloaked, so it shows only with its frame.
        if open_as != crate::windows::OpenAs::Normal {
            cloak.hold(&window, crate::time::now() + win::READY_FALLBACK, focus);
            window.set_visible(true);
            chrome::open_as(&window, open_as);
        }
        let phys = window.inner_size();
        let size = (phys.width.max(1), phys.height.max(1));
        let display = window.current_monitor().map_or((0, 0), |m| (m.size().width, m.size().height));
        let Some((prefault, capacity, painter)) = self.cpu_side(size, display) else { return Err("no memory for the window's frame buffer".into()) };
        let half = HalfRounding::of_vendor(crate::gpu::pick::display_vendor().unwrap_or(0));
        crate::marks::mark("cpu.side");
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST,
            format: wgpu::TextureFormat::Bgra8Unorm,
            width: size.0,
            height: size.1,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: FRAME_LATENCY,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            color_space: Default::default(),
        };
        let scale = window.scale_factor() as f32;
        {
            let rt = self.app.rt_mut();
            rt.set_system_dark(crate::platform::preferences::window_dark(&window, self.args.theme.as_deref()));
            rt.set_viewport((size.0 as f32 / scale, size.1 as f32 / scale), scale);
            // Reopening paints the settled state in the first frame (guarantee 8).
            if self.shown && !built { rt.settle_all(); }
        }
        self.shown = true;
        self.life.show();
        self.idle = true;
        self.app.set_live_visible(true);
        *self.native_handle.native.window.lock().unwrap() = Arc::downgrade(&window);
        self.native_handle.native.number.store(native_window_number(&window).unwrap_or(0), Ordering::Release);
        let native = {
            let mode = SurfaceMode::Cpu { cpu: Cpu { buffer: Buffer::Touching(prefault, capacity), painter, half, shown: Shown::Nothing, show: Some(focus) }, job: GpuJob::Waiting { tries: 0, retry_at: None } };
            Native { cloak, frame, mode, a11y_start, ..Default::default() }
        };
        self.win = Some(Win {
            a11y,
            native,
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
            covered_draw: CoveredDraw::Current,
            redraw: std::cell::Cell::new(Redraw::Idle),
        });
        self.apply_placement(event_loop);
        crate::marks::mark("window.created");
        if let Some(created) = self.on_created.take() { created(self.native_handle.clone()); }
        if let Some(on_window) = self.on_window.as_mut() { on_window(&self.native_handle); }
        if let Some(w) = self.win.as_mut() {
            let size = w.window.inner_size();
            let scale = w.window.scale_factor() as f32;
            let rt = self.app.rt_mut();
            if size.width > 0 && size.height > 0 {
                w.resize_surface(None, size);
                rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
            }
            sync_chrome_core(rt, w, self.chrome, self.titlebar);
            sync_screen_origin_core(rt, &w.window);
        }
        // The first frame, into the hidden window, then the window with it.
        crate::marks::mark("frame.first.begin");
        self.draw_cpu(false);
        crate::marks::mark("frame.first.done");
        let Some(w) = self.win.as_mut() else { return Ok(()) };
        if !w.first_frame.ready() {
            // Only the GPU draws this frame (`start_gpu` began it): shown cloaked until it has.
            if let SurfaceMode::Cpu { cpu, .. } = &mut w.native.mode { cpu.show = None; }
            w.native.cloak.hold(&w.window, crate::time::now() + win::READY_FALLBACK, focus);
            w.window.set_visible(true);
            return Ok(());
        }
        // `draw_cpu` showed it with the frame.
        if w.window.has_focus() { self.app.window_focus(true); }
        Ok(())
    }

    /// A frame drawn by the CPU renderer: the runtime steps, the painter repaints the tiles that
    /// changed, and only those rectangles go to the window. While handing over, the GPU draws the
    /// same instances too.
    pub(in crate::window) fn draw_cpu(&mut self, resizing: bool) {
        let mut frame = CpuWindowFrame {
            host: self,
            resizing,
            owed: false,
            hidden: false,
            now: None,
            encoded: None,
        };
        host_step::run(&mut frame);
    }
}

struct CpuWindowFrame<'a> {
    host: &'a mut Host,
    resizing: bool,
    owed: bool,
    hidden: bool,
    now: Option<Instant>,
    encoded: Option<Instant>,
}

impl FrameHost for CpuWindowFrame<'_> {
    type Drawable = ();
    type Prepared = Option<Vec<[u32; 4]>>;

    fn gate(&mut self) -> Gate {
        // The system threw away what the window showed (it was covered whole, then came back) and
        // asks for it again: the window is in sight, whatever the occlusion tracker said last, so
        // this frame is drawn and copied whole. Skipped, the system would count it painted, and the
        // window would show what's behind it until something changed.
        let owed = self.host.win.as_ref().is_some_and(|w| crate::platform::win32::repaint_owed(&w.window));
        // Handing over: input that comes gives way to nothing. The CPU draws it now and the
        // hand-over waits for a frame at rest; otherwise, with the GPU's frame not finished yet,
        // nothing new until it is (a repaint the system asks for meanwhile gets the last frame).
        if let Some(w) = self.host.win.as_mut() && let SurfaceMode::HandingOver { cpu, done, paused, since, readback, .. } = &mut w.native.mode {
            if self.host.app.rt().has_pending_input() {
                if !*paused { crate::marks::mark("gpu.window.paused"); }
                (*done, *readback, *paused, *since) = (None, None, true, crate::time::now());
            } else if done.is_some() {
                if owed { copy_last(cpu, &w.window); }
                return Gate::Skip;
            }
        }
        let launcher = self.host.launcher();
        let Some(l) = self.host.win.as_ref() else { return Gate::Skip };
        let hidden = launcher && !self.host.life.shown();
        // The frame both renderers draw to hand over draws in sight or not: covered, the window
        // would stay on the CPU until it showed again.
        let handing = matches!(l.native.mode, SurfaceMode::HandingOver { done: None, .. });
        // A covered window's owed frame (`CoveredDraw`) draws, settled.
        let cover_owed = !l.minimized() && l.covered_draw.drawing();
        let covered = !owed && !handing && !cover_owed && ((!launcher && !self.host.life.uncovered()) || l.minimized());
        if covered {
            self.host.covered_frame();
            return Gate::Skip;
        }
        if cover_owed {
            self.host.app.rt_mut().settle_all();
            if let Some(w) = self.host.win.as_mut() { w.covered_draw = w.covered_draw.step(CoveredDrawEvent::Drawn); }
        }
        let Some(l) = self.host.win.as_ref() else { return Gate::Skip };
        if !owed && !handing && !self.resizing && l.backing_off(crate::time::now()) { return Gate::Skip; }
        let Some(l) = self.host.win.as_mut() else { return Gate::Skip };
        l.timer_frame.drawn();
        if self.host.native_handle.native.number.load(Ordering::Relaxed) == 0 {
            self.host.native_handle.native.number.store(native_window_number(&l.window).unwrap_or(0), Ordering::Release);
        }
        if !hidden { self.host.meter.redraw(l.paced_for_stats()); }
        self.owed = owed;
        self.hidden = hidden;
        Gate::Draw
    }

    fn before_acquire(&mut self) {
        if !self.hidden && !self.resizing { self.host.step_frame(crate::time::now()); }
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
        if self.hidden && self.host.win.as_ref().is_some_and(|l| l.first_frame.settles()) {
            self.host.app.rt_mut().settle_all();
        }
        let (animating, dt) = if self.hidden {
            self.host.clock.hold(now);
            (self.host.app.frame(0.0), 0.0)
        } else {
            self.host.app.clock_advance(&mut self.host.clock, now, false)
        };
        if self.hidden && let Some(l) = self.host.win.as_mut() { l.hidden_pace.rendered(now); }
        crate::marks::mark_once(&FIRST_LAYOUT, "frame.first.layout");
        StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.0, presents_at: None }
    }

    fn sync_after_step(&mut self, info: &mut StepInfo) {
        let now = self.now.unwrap_or_else(crate::time::now);
        info.update_ms = now.elapsed().as_secs_f64() * 1000.0;
        let Some(l) = self.host.win.as_mut() else { return };
        let rt = self.host.app.rt_mut();
        sync_platform_core(l, rt, &mut self.host.secure_input, &mut self.host.ime_allowed, &self.host.clipboard);
        let mut quiet_now = false;
        if let SurfaceMode::Cpu { job: GpuJob::Building { latest, animating: moving, ready, .. }, .. } = &mut l.native.mode {
            *latest.lock().unwrap_or_else(|e| e.into_inner()) = warm_instances(&rt.instances);
            *moving = info.animating;
            // The GPU waited for this: a frame at rest, which the hand-over frame draws again.
            quiet_now = !info.animating && ready.is_some();
        }
        if quiet_now { self.host.native_handle.send(WakeEvent::GpuReady); }
    }

    fn prepare_render(&mut self, _drawable: (), _info: &mut StepInfo) -> RenderPrep<Option<Vec<[u32; 4]>>> {
        let has_comp = self.host.app.has_compositor();
        let Some(l) = self.host.win.as_mut() else { return RenderPrep::Abort };
        let (width, height) = (l.config.width, l.config.height);
        let rt = self.host.app.rt_mut();
        let (SurfaceMode::Cpu { cpu, .. } | SurfaceMode::HandingOver { cpu, .. }) = &mut l.native.mode else { return RenderPrep::Abort };
        cpu.painter.fill_masks(&mut rt.text.atlas);
        crate::marks::mark_once(&FIRST_MASKS, "frame.first.masks");
        let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (width, height), rt.clear_color())
            .srgb_blend(rt.srgb_blend())
            .native_surfaces(!rt.surfaces.is_empty())
            .compositor(has_comp)
            .half_rounding(cpu.half)
            .font_smoothing(rt.font_smoothing());
        let Cpu { buffer, painter, .. } = &mut *cpu;
        let Some(dib) = ready(buffer, painter, (width, height)) else {
            eprintln!("rune: no memory for a {width} x {height} frame buffer");
            l.backoff_step(BackoffEvent::At(crate::time::now() + TIMER_FRAME));
            return RenderPrep::Abort;
        };
        let stride = dib.stride();
        match painter.paint(&scene, &mut Target::with_stride(dib.bytes(), width, height, stride)) {
            Ok(painted) => {
                crate::marks::mark_once(&FIRST_PAINT, "frame.first.painted");
                RenderPrep::Ready(Some(painted.rects))
            }
            Err(_why) => {
                // The frame needs what only the GPU draws (a native surface, a compositor's scene):
                // the GPU starts now if it hasn't, and the window keeps its last frame until then.
                if !matches!(self.host.win.as_ref().map(|w| &w.native.mode), Some(SurfaceMode::HandingOver { .. })) {
                    crate::marks::mark("cpu_first.needs_gpu");
                    self.host.start_gpu(false);
                    self.host.idle = false;
                    if let Some(l) = self.host.win.as_mut() { l.backoff_step(BackoffEvent::At(crate::time::now() + TIMER_FRAME)); }
                    return RenderPrep::Abort;
                }
                RenderPrep::Ready(None)
            }
        }
    }

    fn render(&mut self, prepared: &mut Option<Vec<[u32; 4]>>, _info: &StepInfo) -> bool {
        let Some(rects) = prepared.as_deref() else { return false };
        let Some(l) = self.host.win.as_mut() else { return false };
        let (width, height) = (l.config.width, l.config.height);
        let (SurfaceMode::Cpu { cpu, .. } | SurfaceMode::HandingOver { cpu, .. }) = &mut l.native.mode else { return false };
        if let Buffer::Ready(dib) = &cpu.buffer {
            cpu.shown = match std::mem::replace(&mut cpu.shown, Shown::Nothing) {
                Shown::Copied(size) => {
                    let blit = if size == (width, height) && !self.owed { Blit::Bounds(rects) } else { Blit::All };
                    if let Some(hwnd) = crate::platform::win32::hwnd(&l.window) { dib.present(hwnd, (width, height), blit); }
                    Shown::Copied((width, height))
                }
                Shown::Waiting | Shown::Nothing => Shown::Waiting,
            };
        }
        if let Some(focus) = cpu.show.take() {
            // The window shows with its first frame, before the frame's hooks run
            // (`App::on_frame_drawn`). GDI draws nothing into a hidden window, and DWM
            // composes a window that just showed with whatever its surface holds (white):
            // it shows cloaked, takes the frame, then comes on screen, activating as the
            // cloak lifts.
            if !l.native.cloak.held() {
                l.native.cloak.hold(&l.window, crate::time::now() + win::READY_FALLBACK, focus);
                crate::marks::mark("window.cloaked");
                l.window.set_visible(true);
                crate::marks::mark("window.visible");
            }
            copy_first(cpu, &l.window, (width, height));
            l.native.cloak.lift(&l.window);
            crate::marks::mark("window.shown");
        } else if l.window.is_visible() == Some(true) {
            // A frame after the show that came before the first copy.
            copy_first(cpu, &l.window, (width, height));
        }
        false
    }

    fn submit_and_present(&mut self, prepared: Option<Vec<[u32; 4]>>, info: &StepInfo) -> Instant {
        self.encoded = self.host.frame_reports.stamp();
        crate::window::frame_presented();
        if prepared.is_some() && let Some(l) = self.host.win.as_mut() {
            crate::marks::mark_once(&FIRST_CPU, "frame.first.cpu");
            if !l.first_frame.ready() { l.first_frame.gpu_done(); }
        }
        self.host.gpu_draws_same_frame();
        // The input the hand-over paused for is drawn: the next frame at rest hands over.
        if let Some(l) = self.host.win.as_mut()
            && let SurfaceMode::HandingOver { paused, since, .. } = &mut l.native.mode
            && *paused && !info.animating && !self.host.app.rt().has_pending_input()
        {
            (*paused, *since) = (false, crate::time::now());
        }
        crate::time::now()
    }

    fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore) {
        (Some(&mut self.host.clock), self.host.app.rt_mut())
    }

    fn settle(&mut self, submitted: Instant, info: &StepInfo) {
        let now = self.now.unwrap_or(submitted);
        self.host.meter.presented();
        self.host.frame_reports.drawn(self.encoded, None, now, info.update_ms);
        self.host.app.inspect_frame();
        let Some(l) = self.host.win.as_mut() else { return };
        let next_tick = l.next_tick(now).filter(|_| !self.hidden);
        self.host.idle = Presented::settle_core(
            &mut l.presented,
            self.host.app.rt(),
            &l.config,
            info.animating || FrameMove::moving(&self.host.frame_move),
            next_tick,
            !self.hidden,
            false,
        );
        // Motion's next frame comes at the display's next refresh: a CPU frame presents at once,
        // so nothing else paces it.
        if !self.hidden && !self.host.idle {
            l.backoff_step(BackoffEvent::At(crate::platform::win32::next_vblank(crate::time::now())));
            self.host.meter.set_pacing(l.pacing_state(true));
        } else {
            l.stop_pacing();
            self.host.meter.set_pacing(l.pacing_state(false));
        }
    }
}

static FIRST_CPU: AtomicBool = AtomicBool::new(false);
static FIRST_LAYOUT: AtomicBool = AtomicBool::new(false);
static FIRST_MASKS: AtomicBool = AtomicBool::new(false);
static FIRST_PAINT: AtomicBool = AtomicBool::new(false);
