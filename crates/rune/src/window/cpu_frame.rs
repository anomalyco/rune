//! Windows: a window that is live on the CPU from its first frame, and hands its frames to the GPU
//! once one is ready, without a seam.
//!
//! The window opens with no GPU: the CPU renderer (`render::cpu_boot`) paints the real view into a
//! DIB section, the changed rectangles are copied to the window with GDI, and the window shows
//! with that frame in it. Input, hover, typing and animation run as on any frame. After the first
//! frame, on the loop's first idle turn, a thread at below-normal priority makes the GPU device,
//! Rune's renderer, an uncommitted DirectComposition visual on the window and a swap chain on it
//! (`platform::win32::dcomp_handover`), so none of that competes with the first frame. When it's
//! ready both renderers draw every frame from the same instances; on the first turn that starts
//! with the GPU's previous frame finished, the visual is committed and DWM shows the GPU's swap
//! chain from its next composition, the same pose the CPU last showed or the one after. The
//! CPU's buffer goes on a worker a turn later. A GPU that can't be made leaves the window on the
//! CPU, fully live.
//!
//! The view's text and images stay where the CPU frames put them: masks the CPU filled upload on
//! the GPU's first frame, so the swap repaints nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::platform::win32::dcomp_handover::Handover;
use crate::platform::win32::dib::{Blit, Dib, Prefault, fill_window, free_later};
use crate::render::cpu_boot::{CpuPainter, HalfRounding, Scene, Target};

/// The CPU renderer's side of a window: the buffer it paints in place and the painter that
/// knows what the buffer last held.
pub(super) struct Cpu {
    buffer: Buffer,
    painter: CpuPainter,
    /// How the GPU that takes over rounds half floats: the primary display's GPU's until that GPU
    /// exists, then its own.
    half: HalfRounding,
    /// What the window has of the buffer.
    shown: Shown,
    /// Until the first frame is painted: the window shows with it, then, activating when `true`.
    show: Option<bool>,
}

/// What a CPU window has of its buffer. GDI draws nothing into a hidden window, so the first
/// frame waits in the buffer until the window shows (cloaked: DWM composes it only once it's in).
enum Shown {
    /// Nothing painted yet.
    Nothing,
    /// The first frame is painted: when the window shows, it gets `colour` everywhere (the
    /// tiles the painter left to it, `CpuPainter::window_shows`), then `rects`.
    Waiting { rects: Vec<[u32; 4]>, colour: Option<[u8; 4]> },
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
    fn into_dib(self) -> Option<Dib> {
        match self.buffer {
            Buffer::Ready(dib) => Some(dib),
            Buffer::Touching(prefault, _) => prefault.take(),
            Buffer::Missing => None,
        }
    }
}

/// Where a window's frames go. Windows that need the GPU from their first frame (a compositor,
/// a translucent window, an overlay) are `Gpu` from the start, as on every other platform.
#[derive(Default)]
pub(super) enum SurfaceMode {
    #[default]
    Gpu,
    /// The CPU renderer draws; `job` is the GPU being made for it.
    Cpu { cpu: Cpu, job: GpuJob },
    /// The GPU's swap chain is on the window's visual, uncommitted. The next frame is drawn by
    /// both renderers, and then nothing is drawn until the GPU has finished it (`done`): a
    /// window that animates would otherwise replace it every frame and never hand over. The turn
    /// that finds it done commits. `readback` is that frame's pixels on their way back, to count
    /// how many differ from the CPU's.
    HandingOver { cpu: Cpu, handover: Handover, done: Option<Arc<AtomicBool>>, readback: Option<Readback> },
    /// The GPU draws through the committed visual, which lives as long as the window's surface.
    Handed { handover: Handover },
    /// The GPU's swap chain was made again (a lost device) on the same visual: committed once its
    /// first frame has presented and finished (`Host::draw`).
    Rebinding { handover: Handover },
}

/// A GPU frame copied into a buffer the CPU can read: rows `row` bytes apart, `size` pixels.
pub(super) struct Readback {
    buffer: wgpu::Buffer,
    row: u32,
    size: (u32, u32),
}

/// The GPU a CPU window is waiting for.
pub(super) enum GpuJob {
    /// Not started: the first frame isn't out yet.
    Waiting,
    /// Being made on its thread; it reports through `slot`.
    Building { slot: Arc<Mutex<Option<Result<Prepared, String>>>> },
    /// It couldn't be made: the window stays on the CPU.
    Failed,
}

/// What the GPU thread makes for a window.
pub(super) struct Prepared {
    gpu: Gpu,
    renderer: Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    handover: Handover,
}

// SAFETY: wgpu's objects are `Send`; a new `Renderer` has no native surface renderer (the one
// part that isn't), and `Handover` is used by one thread at a time.
unsafe impl Send for Prepared {}

impl SurfaceMode {
    /// Whether the CPU renderer draws this window's frames now.
    pub(super) fn cpu_draws(&self) -> bool { matches!(self, SurfaceMode::Cpu { .. } | SurfaceMode::HandingOver { .. }) }

    /// The visual a handed-over window's swap chain goes on.
    pub(super) fn visual(&self) -> Option<*mut std::ffi::c_void> {
        match self {
            SurfaceMode::Handed { handover } | SurfaceMode::Rebinding { handover } => Some(handover.visual()),
            SurfaceMode::Gpu | SurfaceMode::Cpu { .. } | SurfaceMode::HandingOver { .. } => None,
        }
    }

    /// A new swap chain went on the handed-over window's visual (`Host::reattach`).
    pub(super) fn rebind(&mut self) {
        if let SurfaceMode::Handed { .. } = self {
            let SurfaceMode::Handed { handover } = std::mem::take(self) else { unreachable!() };
            *self = SurfaceMode::Rebinding { handover };
        }
    }
}

/// Threads the CPU renderer paints on: the UI thread and up to five workers that park between
/// frames.
/// The last frame copied to the window, whole, again (`draw_cpu`).
fn copy_last(cpu: &Cpu, window: &Window) {
    let (Shown::Copied(size), Buffer::Ready(dib)) = (&cpu.shown, &cpu.buffer) else { return };
    if let Some(hwnd) = crate::platform::win32::hwnd(window) { dib.present(hwnd, *size, Blit::All); }
}

/// The first frame, painted while the window was hidden, into the window that now shows: its own
/// colour first if the painter left tiles to it, then the frame's rects, one by one (nothing
/// composes the window before its cloak lifts).
fn copy_first(cpu: &mut Cpu, window: &Window, size: (u32, u32)) {
    let (Shown::Waiting { rects, colour }, Buffer::Ready(dib)) = (&cpu.shown, &cpu.buffer) else { return };
    let Some(hwnd) = crate::platform::win32::hwnd(window) else { return };
    if let Some(colour) = colour { fill_window(hwnd, size, *colour); }
    dib.present(hwnd, size, Blit::Each(rects));
    cpu.shown = Shown::Copied(size);
    crate::marks::mark("frame.first.copied");
}

/// Whether a window's first frame goes into a window already filled with its clear colour, which
/// the painter then needn't write (`CpuPainter::window_shows`): `RUNE_CPU_COLOUR=1`.
fn window_colour() -> bool {
    std::env::var_os("RUNE_CPU_COLOUR").is_some_and(|v| v == "1")
}

fn painter_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 6)
}

/// Whether a window opens on the CPU: an opaque window while no window has a GPU yet (with one,
/// a new window's swap chain is ready in a few milliseconds), unless turned off
/// (`RUNE_CPU_FIRST=0`).
pub(super) fn opens_on_cpu<S: 'static>(host: &Host<S>) -> bool {
    !host.options.transparent && host.gpu.is_none()
        && host.gpu_pool.borrow().upgrade().is_none_or(|gpu| gpu.is_lost())
        && std::env::var_os("RUNE_CPU_FIRST").is_none_or(|v| v != "0")
}

impl<S: 'static> Host<S> {
    /// The CPU side for a new window of `size` physical pixels on a display of `display`: a
    /// buffer as big as the larger, whose first frame's rows a worker touches while that frame is
    /// laid out, and a painter. The text system queues glyph masks for the painter's fill from
    /// now on. Returns the buffer's size with it.
    pub(super) fn cpu_side(&mut self, size: (u32, u32), display: (u32, u32)) -> Option<(Prefault, (u32, u32), CpuPainter)> {
        let capacity = (size.0.max(display.0), size.1.max(display.1));
        let dib = Dib::new(capacity.0, capacity.1)?;
        crate::render::cpu_boot::attach_text(&mut self.rt.text.atlas);
        Some((Prefault::start(dib, size.1), capacity, CpuPainter::new(painter_threads())))
    }

    /// The rest of `try_create` for a window that opens on the CPU: no GPU, no surface. Its first
    /// frame paints into the buffer and the window shows with it, activating when it should
    /// (`focus`); a first frame only the GPU can draw shows cloaked until the GPU has drawn it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_on_cpu(&mut self, event_loop: &ActiveEventLoop, window: Arc<Window>, a11y: Option<crate::a11y::Adapter>, a11y_start: native::A11yStart,
        frame: Option<crate::platform::win32::CustomFrame>, cloak: crate::platform::win32::Cloak, focus: bool, open_as: crate::windows::OpenAs, built: bool) -> Result<(), String> {
        // Maximizing (or full screen) shows a window: cloaked, so it shows only with its frame.
        if open_as != crate::windows::OpenAs::Normal {
            cloak.hold(&window, Instant::now() + win::READY_FALLBACK, focus);
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8Unorm,
            width: size.0,
            height: size.1,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            color_space: Default::default(),
        };
        let scale = window.scale_factor() as f32;
        self.rt.set_system_dark(crate::platform::preferences::window_dark(&window, self.args.theme.as_deref()));
        self.rt.set_viewport((size.0 as f32 / scale, size.1 as f32 / scale), scale);
        // Reopening paints the settled state in the first frame (guarantee 8).
        if self.shown && !built { self.rt.settle_all(); }
        self.shown = true;
        self.life.show();
        self.idle = true;
        self.rt.set_live_visible(true);
        *self.native_handle.native.window.lock().unwrap() = Arc::downgrade(&window);
        self.native_handle.native.number.store(native_window_number(&window).unwrap_or(0), Ordering::Release);
        let native = {
            let handle = self.native_handle.clone();
            let report = Arc::new(move |covered| handle.send(move |slot| WakeEvent::Covered(slot, covered)));
            let mode = SurfaceMode::Cpu { cpu: Cpu { buffer: Buffer::Touching(prefault, capacity), painter, half, shown: Shown::Nothing, show: Some(focus) }, job: GpuJob::Waiting };
            Native { cloak, frame, _occlusion: crate::platform::win32::occlusion::Tracked::new(&window, report), mode, a11y_start }
        };
        self.win = Some(Win {
            a11y,
            native,
            first_frame: FirstFrame::opened(Instant::now()),
            surface: None,
            config,
            surface_ok: true,
            surface_retries: SurfaceRetries::default(),
            window,
            backoff: Backoff::default(),
            hidden_pace: HiddenPace::default(),
            cursor: crate::cursor::os::OsCursor::new(event_loop),
            alpha: None,
            timer_frame: TimerFrame::default(),
            presented: None,
            steady: Steady::Rest,
            redraw: std::cell::Cell::new(Redraw::Idle),
        });
        self.apply_placement(event_loop);
        crate::marks::mark("window.created");
        if let Some(created) = self.on_created.take() { created(self.native_handle.clone()); }
        if let Some(on_window) = self.on_window.as_mut() { on_window(&self.native_handle); }
        if let Some(w) = self.win.as_mut() {
            let size = w.window.inner_size();
            let scale = w.window.scale_factor() as f32;
            if size.width > 0 && size.height > 0 {
                w.resize_surface(None, size);
                self.rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
            }
            sync_chrome(&mut self.rt, w, self.chrome, self.titlebar);
            sync_screen_origin(&mut self.rt, &w.window);
        }
        // The first frame, into the hidden window, then the window with it.
        crate::marks::mark("frame.first.begin");
        self.draw_cpu(false);
        crate::marks::mark("frame.first.done");
        let Some(w) = self.win.as_ref() else { return Ok(()) };
        let Some(w) = self.win.as_mut() else { return Ok(()) };
        if !w.first_frame.ready() {
            // Only the GPU draws this frame (`start_gpu` began it): shown cloaked until it has.
            if let SurfaceMode::Cpu { cpu, .. } = &mut w.native.mode { cpu.show = None; }
            w.native.cloak.hold(&w.window, Instant::now() + win::READY_FALLBACK, focus);
            w.window.set_visible(true);
            return Ok(());
        }
        // `draw_cpu` showed it with the frame.
        if w.window.has_focus() { self.rt.window_focus(true); }
        Ok(())
    }

    /// A frame drawn by the CPU renderer: the runtime steps, the painter repaints the tiles that
    /// changed, and only those rectangles go to the window. While handing over, the GPU draws the
    /// same instances too.
    pub(super) fn draw_cpu(&mut self, resizing: bool) {
        // The system threw away what the window showed (it was covered whole, then came back) and
        // asks for it again: the window is in sight, whatever the occlusion tracker said last, so
        // this frame is drawn and copied whole. Skipped, the system would count it painted, and the
        // window would show what's behind it until something changed.
        let owed = self.win.as_ref().is_some_and(|w| crate::platform::win32::repaint_owed(&w.window));
        // Handing over, with the GPU's frame not finished yet: nothing new until it is (a repaint
        // the system asks for meanwhile gets the last frame).
        if let Some(w) = self.win.as_mut() && let SurfaceMode::HandingOver { cpu, done: Some(_), .. } = &mut w.native.mode {
            if owed { copy_last(cpu, &w.window); }
            return;
        }
        let launcher = self.launcher();
        let Some(l) = self.win.as_ref() else { return };
        let hidden = launcher && !self.life.shown();
        let covered = !owed && ((!launcher && !self.life.uncovered()) || l.minimized());
        if covered { return self.covered_frame(); }
        if !owed && !resizing && l.backing_off(Instant::now()) { return; }
        let Some(l) = self.win.as_mut() else { return };
        l.timer_frame.drawn();
        if self.native_handle.native.number.load(Ordering::Relaxed) == 0 {
            self.native_handle.native.number.store(native_window_number(&l.window).unwrap_or(0), Ordering::Release);
        }
        if !hidden { self.meter.redraw(l.paced_for_stats()); }
        if !hidden && !resizing { self.step_frame(Instant::now()); }
        let Some(l) = self.win.as_mut() else { return };
        l.backoff_step(BackoffEvent::Lapsed);
        l.backoff_step(BackoffEvent::Presented);
        if self.frame_reports.wanted() { self.frame_reports.set_refresh(l.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).map(|hz| hz as f64 / 1000.0)); }
        let now = Instant::now();
        let rt = &mut self.rt;
        if hidden && l.first_frame.settles() { rt.settle_all(); }
        let (animating, _dt) = if hidden {
            self.clock.hold(now);
            (rt.frame(0.0), 0.0)
        } else {
            self.clock.advance(rt, now, false)
        };
        if hidden { l.hidden_pace.rendered(now); }
        crate::marks::mark_once(&FIRST_LAYOUT, "frame.first.layout");
        let update_ms = now.elapsed().as_secs_f64() * 1000.0;
        sync_platform(l, rt, &mut self.secure_input, &mut self.ime_allowed, &self.clipboard);
        let (width, height) = (l.config.width, l.config.height);
        let painted = {
            let (SurfaceMode::Cpu { cpu, .. } | SurfaceMode::HandingOver { cpu, .. }) = &mut l.native.mode else { return };
            cpu.painter.fill_masks(&mut rt.text.atlas);
            crate::marks::mark_once(&FIRST_MASKS, "frame.first.masks");
            let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (width, height), rt.clear_color())
                .srgb_blend(rt.srgb_blend())
                .native_surfaces(!rt.surfaces.is_empty())
                .compositor(self.comp.is_some())
                .half_rounding(cpu.half)
                .font_smoothing(rt.font_smoothing());
            let Cpu { buffer, painter, shown, .. } = &mut *cpu;
            let Some(dib) = ready(buffer, painter, (width, height)) else {
                eprintln!("rune: no memory for a {width} x {height} frame buffer");
                l.backoff_step(BackoffEvent::At(Instant::now() + TIMER_FRAME));
                return;
            };
            let stride = dib.stride();
            // The first frame can leave the tiles all in the window's own colour unwritten; they're
            // written into the buffer before the next frame, whose copy may cover them.
            let colour = match shown {
                Shown::Nothing if window_colour() => {
                    let colour = crate::render::cpu_boot::opaque_bgra(rt.clear_color());
                    painter.window_shows(colour);
                    Some(colour)
                }
                Shown::Nothing => None,
                Shown::Waiting { colour, .. } => *colour,
                Shown::Copied(_) => {
                    painter.fill_absent(&mut Target::with_stride(dib.bytes(), width, height, stride));
                    None
                }
            };
            match painter.paint(&scene, &mut Target::with_stride(dib.bytes(), width, height, stride)) {
                Ok(painted) => {
                    crate::marks::mark_once(&FIRST_PAINT, "frame.first.painted");
                    *shown = match std::mem::replace(shown, Shown::Nothing) {
                        Shown::Copied(size) => {
                            let blit = if size == (width, height) && !owed { Blit::Bounds(&painted.rects) } else { Blit::All };
                            if let Some(hwnd) = crate::platform::win32::hwnd(&l.window) { dib.present(hwnd, (width, height), blit); }
                            Shown::Copied((width, height))
                        }
                        Shown::Waiting { mut rects, .. } => { rects.extend_from_slice(&painted.rects); Shown::Waiting { rects, colour } }
                        Shown::Nothing => Shown::Waiting { rects: painted.rects, colour },
                    };
                    if let Some(focus) = cpu.show.take() {
                        // The window shows with its first frame, before the frame's hooks run
                        // (`App::on_frame_drawn`). GDI draws nothing into a hidden window, and DWM
                        // composes a window that just showed with whatever its surface holds (white):
                        // it shows cloaked, takes the frame, then comes on screen, activating as the
                        // cloak lifts.
                        if !l.native.cloak.held() {
                            l.native.cloak.hold(&l.window, Instant::now() + win::READY_FALLBACK, focus);
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
                    Ok(())
                }
                Err(why) => Err(why),
            }
        };
        if let Err(why) = &painted {
            // The frame needs what only the GPU draws (a native surface, a compositor's scene):
            // the GPU starts now if it hasn't, and the window keeps its last frame until then.
            if !matches!(self.win.as_ref().map(|w| &w.native.mode), Some(SurfaceMode::HandingOver { .. })) {
                let _ = why;
                crate::marks::mark("cpu_first.needs_gpu");
                self.start_gpu(false);
                self.idle = false;
                if let Some(l) = self.win.as_mut() { l.backoff_step(BackoffEvent::At(Instant::now() + TIMER_FRAME)); }
                return;
            }
        }
        let encoded = self.frame_reports.stamp();
        crate::marks::presented();
        let Some(l) = self.win.as_mut() else { return };
        if painted.is_ok() {
            crate::marks::mark_once(&FIRST_CPU, "frame.first.cpu");
            if !l.first_frame.ready() { l.first_frame.gpu_done(); }
        }
        self.gpu_draws_same_frame();
        let Some(l) = self.win.as_mut() else { return };
        let rt = &mut self.rt;
        let submitted = Instant::now();
        self.clock.presented(rt, submitted);
        self.meter.presented();
        self.frame_reports.drawn(encoded, None, now, update_ms);
        rt.interaction_presented(submitted, None);
        self.frame_reports.inspect(rt);
        let next_tick = l.next_tick(now).filter(|_| !hidden);
        let changed = !l.steady.held_frame() || !l.presented.as_ref().is_some_and(|p| p.same(rt, &l.config));
        let event = if hidden { SteadyEvent::Ends } else { SteadyEvent::Drew { changed, focused: rt.has_window_focus(), now } };
        l.steady = l.steady.step(event).0;
        self.idle = Presented::settle(&mut l.presented, rt, &l.config, animating || stepping::FrameMove::moving(&self.frame_move), next_tick, !hidden, false);
        if self.idle && !hidden {
            let (steady, again) = l.steady.step(SteadyEvent::Rests { now });
            l.steady = steady;
            if again { l.request_redraw(); }
        }
        // Motion's next frame comes at the display's next refresh: a CPU frame presents at once,
        // so nothing else paces it.
        if !hidden && !self.idle {
            l.backoff_step(BackoffEvent::At(crate::platform::win32::next_vblank(Instant::now())));
            self.meter.set_pacing(l.pacing_state(true));
        } else {
            l.stop_pacing();
            self.meter.set_pacing(l.pacing_state(false));
        }
    }

    /// The first idle turn after the window's first CPU frame: the GPU starts on its thread
    /// (`RUNE_CPU_FIRST=only`: not until a frame needs it, for checking the CPU window alone).
    pub(super) fn cpu_idle(&mut self) {
        if std::env::var_os("RUNE_CPU_FIRST").is_some_and(|v| v == "only") { return; }
        if self.win.as_ref().is_some_and(|w| matches!(w.native.mode, SurfaceMode::Cpu { job: GpuJob::Waiting, .. }) && w.first_frame.ready()) {
            self.start_gpu(true);
        }
    }

    /// Starts making the GPU for this window on a thread (`below`: at below-normal priority, as
    /// after the first frame; a frame that needs the GPU makes it at normal priority).
    fn start_gpu(&mut self, below: bool) {
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { job, .. } = &mut w.native.mode else { return };
        let GpuJob::Waiting = job else { return };
        let Some(hwnd) = crate::platform::win32::hwnd(&w.window) else { *job = GpuJob::Failed; return };
        let slot = Arc::new(Mutex::new(None));
        let (out, handle, srgb, size) = (slot.clone(), self.native_handle.clone(), self.rt.srgb_blend(), (w.config.width, w.config.height));
        let features = self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
        let hwnd = hwnd as isize;
        let spawned = std::thread::Builder::new().name("rune-gpu-window".into()).spawn(move || {
            if below { lower_priority(); }
            let made = prepare(hwnd, size, srgb, features);
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(made);
            handle.send(WakeEvent::GpuReady);
        });
        *job = match spawned {
            Ok(_) => { crate::marks::mark("gpu.window.start"); GpuJob::Building { slot } }
            Err(error) => { eprintln!("rune: could not start the window's GPU thread: {error}"); GpuJob::Failed }
        };
    }

    /// The GPU thread finished: attach its device, renderer and surface and start handing over,
    /// or stay on the CPU if it failed. While handing over: the GPU finished a frame.
    pub(super) fn gpu_ready(&mut self) {
        if self.win.as_ref().is_some_and(|w| matches!(w.native.mode, SurfaceMode::HandingOver { .. })) { return self.hand_over_if_done(); }
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { job: GpuJob::Building { slot }, .. } = &w.native.mode else { return };
        let Some(made) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let prepared = match made {
            Ok(prepared) => prepared,
            Err(error) => {
                eprintln!("rune: the window stays on the CPU renderer: {error}");
                if let SurfaceMode::Cpu { job, .. } = &mut w.native.mode { *job = GpuJob::Failed; }
                return;
            }
        };
        crate::marks::mark("gpu.window.ready");
        let Prepared { gpu, mut renderer, surface, config, handover } = prepared;
        let gpu = Arc::new(gpu);
        let target_format = if self.rt.srgb_blend() { config.format.remove_srgb_suffix() } else { config.format };
        rebuild_renderer(&gpu.device, &mut renderer, target_format, &mut self.rt);
        *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
        crate::render::surface_gpu(&gpu);
        let size = w.window.inner_size();
        let SurfaceMode::Cpu { mut cpu, .. } = std::mem::replace(&mut w.native.mode, SurfaceMode::Gpu) else { unreachable!() };
        cpu.half = HalfRounding::of_vendor(gpu.adapter.get_info().vendor);
        w.native.mode = SurfaceMode::HandingOver { cpu, handover, done: None, readback: None };
        w.surface = Some(surface);
        w.config = config;
        w.surface_ok = true;
        // The window changed size while the GPU was made.
        if size.width > 0 && size.height > 0 { w.resize_surface(Some(&*gpu), size); }
        self.gpu = Some((gpu, renderer));
        w.request_redraw();
    }

    /// While handing over: the GPU draws the instances the CPU just painted, into its swap chain
    /// on the uncommitted visual.
    fn gpu_draws_same_frame(&mut self) {
        let (Some(l), Some((gpu, renderer))) = (self.win.as_mut(), self.gpu.as_mut()) else { return };
        let SurfaceMode::HandingOver { done, readback, .. } = &mut l.native.mode else { return };
        let Some(surface) = l.surface.as_ref() else { return };
        let Ok(frame) = acquire(surface, gpu, &l.config, l.surface_ok, 1, &mut self.meter) else { return };
        let rt = &mut self.rt;
        let target_format = if rt.srgb_blend() { l.config.format.remove_srgb_suffix() } else { l.config.format };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor { format: (target_format != l.config.format).then_some(target_format), ..Default::default() });
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (l.config.width, l.config.height));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        // The app's compositor draws the same instances the CPU painted (the CPU frame drew none of
        // its effect slots, or it would have asked for the GPU).
        if let Some(c) = self.comp.as_mut() {
            let size = (l.config.width, l.config.height);
            c.render(&mut Frame { device: &gpu.device, queue: &gpu.queue, encoder: &mut enc, target: &view, format: target_format, size, dt: 0.0, update_ms: 0.0, rt: &mut *rt, ui: &mut *renderer });
        } else {
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (l.config.width, l.config.height));
            renderer.draw(&mut enc, &view, rt.clear_color());
        }
        // A copy of the frame, for the drift count once it's the one the window commits to.
        *readback = l.config.usage.contains(wgpu::TextureUsages::COPY_SRC).then(|| {
            let (width, height) = (l.config.width, l.config.height);
            let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("rune hand-over readback"), size: u64::from(row) * u64::from(height), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            enc.copy_texture_to_buffer(frame.texture.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) } },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
            Readback { buffer, row, size: (width, height) }
        });
        renderer.submit(&gpu.queue, enc);
        gpu.queue.present(frame);
        let flag = Arc::new(AtomicBool::new(false));
        let set = flag.clone();
        gpu.queue.on_submitted_work_done(move || set.store(true, Ordering::Release));
        *done = Some(flag);
        // A thread waits for the GPU and wakes the loop, which commits (`hand_over_if_done`).
        let (waiting, handle) = (gpu.clone(), self.native_handle.clone());
        let spawned = std::thread::Builder::new().name("rune-gpu-wait".into()).spawn(move || {
            let _ = waiting.device.poll(wgpu::PollType::wait_indefinitely());
            handle.send(WakeEvent::GpuReady);
        });
        if let Err(error) = spawned { eprintln!("rune: could not wait for the GPU's frame: {error}"); }
        crate::marks::mark_once(&FIRST_GPU, "frame.first.gpu");
        // A first frame only the GPU could draw.
        if !l.first_frame.ready() { l.first_frame.gpu_done(); }
    }

    /// At the start of a turn while handing over: when the GPU's last frame is done, commit the
    /// visual (DWM shows the GPU from its next composition) and leave the CPU behind; the CPU's
    /// buffer goes on a worker after counting the frame's drift.
    pub(super) fn hand_over_if_done(&mut self) {
        let (Some(w), Some((gpu, _))) = (self.win.as_mut(), self.gpu.as_ref()) else { return };
        match &mut w.native.mode {
            SurfaceMode::HandingOver { done: Some(done), .. } => {
                let _ = gpu.device.poll(wgpu::PollType::Poll);
                let lost = gpu.is_lost();
                if !lost && !done.load(Ordering::Acquire) { return; }
                let SurfaceMode::HandingOver { cpu, handover, readback, .. } = std::mem::replace(&mut w.native.mode, SurfaceMode::Gpu) else { unreachable!() };
                let committed = if lost { Err("its device was lost".to_owned()) } else { handover.commit() };
                if let Err(error) = committed {
                    // DWM kept the window's own content: the CPU goes on drawing it.
                    eprintln!("rune: the GPU couldn't take over the window: {error}");
                    w.request_redraw();
                    w.native.mode = SurfaceMode::Cpu { cpu, job: GpuJob::Failed };
                    w.surface = None;
                    self.gpu = None;
                    return;
                }
                crate::marks::mark("gpu.window.handed");
                crate::platform::win32::unidle_d3d_threads_soon();
                // A first frame only the GPU could draw showed cloaked: it comes on screen now.
                if w.native.cloak.held() {
                    w.native.cloak.lift(&w.window);
                    crate::marks::mark("window.shown");
                }
                // What DWM shows is in the window's own surface now, not in the buffer: it goes on a
                // worker, after counting how many of its pixels the GPU's same frame drew otherwise.
                if let Some(dib) = cpu.into_dib() {
                    match readback { Some(r) => count_drift(gpu.clone(), r, dib), None => free_later(dib) }
                }
                w.native.mode = SurfaceMode::Handed { handover };
                w.presented = None;
                // What was asked for while the GPU finished its frame draws now, on the GPU.
                w.request_redraw();
            }
            SurfaceMode::HandingOver { done: None, .. } | SurfaceMode::Gpu | SurfaceMode::Cpu { .. } | SurfaceMode::Handed { .. } | SurfaceMode::Rebinding { .. } => {}
        }
    }
}

static FIRST_CPU: AtomicBool = AtomicBool::new(false);
static FIRST_LAYOUT: AtomicBool = AtomicBool::new(false);
static FIRST_MASKS: AtomicBool = AtomicBool::new(false);
static FIRST_PAINT: AtomicBool = AtomicBool::new(false);
static FIRST_GPU: AtomicBool = AtomicBool::new(false);

/// On the GPU thread: the device (the one `warm_gpu` made, if it did), the renderer, the
/// uncommitted visual and a swap chain on it, configured for `size`.
fn prepare(hwnd: isize, size: (u32, u32), srgb: bool, features: wgpu::Features) -> Result<Prepared, String> {
    let handover = Handover::new(hwnd)?;
    let visual = handover.visual();
    // SAFETY: the visual stays alive while the surface does: the surface takes its own reference
    // to it, and the window keeps the `Handover` as long as it keeps the surface.
    let (gpu, surface, renderer) = Gpu::for_surface(features, false,
        |instance| unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(visual)) }.map_err(|e| e.to_string()))?;
    let (config, _) = surface_config(&surface, &gpu.adapter, (size.0.max(1), size.1.max(1)), srgb).ok_or("the GPU can't present to this window")?;
    configure(&surface, &gpu, &config)?;
    let target_format = if srgb { config.format.remove_srgb_suffix() } else { config.format };
    let renderer = renderer.unwrap_or_else(|| Renderer::new(&gpu.device, target_format));
    Ok(Prepared { gpu, renderer, surface, config, handover })
}

/// On a worker: maps the GPU frame's copy, counts the pixels that differ from the CPU's buffer
/// (`cpu_boot::drift` logs one line and keeps `last_drift`), and frees both.
fn count_drift(gpu: Arc<Gpu>, readback: Readback, mut dib: Dib) {
    let spawned = std::thread::Builder::new().name("rune-drift".into()).spawn(move || {
        let slice = readback.buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        if gpu.device.poll(wgpu::PollType::wait_indefinitely()).is_err() { return; }
        let (width, height) = readback.size;
        let stride = dib.stride();
        let Ok(gpu_px) = slice.get_mapped_range() else { return };
        crate::render::cpu_boot::drift(crate::render::cpu_boot::Pixels::bgra(dib.bytes(), width, height, stride), crate::render::cpu_boot::Pixels::bgra(&gpu_px, width, height, readback.row as usize));
    });
    if let Err(error) = spawned { eprintln!("rune: could not count the hand-over's drift: {error}"); }
}

fn lower_priority() {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    // SAFETY: this thread's pseudo-handle.
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
}
