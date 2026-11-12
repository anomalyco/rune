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
use crate::platform::win32::dib::{Blit, Dib, Prefault, free_later};
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
    /// `since`: when the GPU came; `nudged`: when a frame was last asked for, while none has been
    /// drawn by both (`Host::watch_handover`); `done`'s instant: when that frame was submitted.
    HandingOver { cpu: Cpu, handover: Handover, done: Option<(Arc<AtomicBool>, Instant)>, readback: Option<Readback>, since: Instant, nudged: Option<Instant>, tries: u8 },
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

/// The GPU a CPU window is waiting for. Every step has a deadline (`Host::watch_handover`), and
/// every way it ends is logged: handed over (`gpu.window.handed`), or failed with a reason
/// (`gpu.window.failed`, note `gpu.window.why`) and tried again after `RETRY_AFTER`, `TRIES`
/// times in all, then the CPU for good.
pub(super) enum GpuJob {
    /// Not started: the first frame isn't out yet, or a failed try (`tries` so far) waits for
    /// `retry_at`.
    Waiting { tries: u8, retry_at: Option<Instant> },
    /// Being made on `thread` since `since`; it reports through `slot`. `raised`: the thread runs
    /// at normal priority now (`RAISE_AFTER`).
    Building { slot: Arc<Mutex<Option<Result<Prepared, String>>>>, thread: std::thread::JoinHandle<()>, since: Instant, raised: bool, tries: u8 },
    /// It failed `TRIES` times: the window stays on the CPU.
    Failed,
}

/// A GPU thread that hasn't made the GPU after this long runs at normal priority: below normal,
/// a busy machine's other work can hold it off for seconds.
const RAISE_AFTER: Duration = Duration::from_millis(500);
/// Past this, a GPU thread still making the GPU is logged as slow (it can't be stopped).
const BUILD_SLOW: Duration = Duration::from_secs(20);
/// While no frame has been drawn by both renderers, one is asked for this often, in sight or not.
const NUDGE_EVERY: Duration = Duration::from_millis(100);
/// The longest the GPU may take to finish the frame both drew: then the hand-over fails.
const FINISH_LIMIT: Duration = Duration::from_secs(5);
/// A failed try is made again after this long.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Tries in all before the window stays on the CPU.
const TRIES: u8 = 3;

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

    /// The DirectComposition tree of a handed-over window, for a playing clip's own layer
    /// (`video_layer`).
    pub(super) fn handover(&self) -> Option<&Handover> {
        match self {
            SurfaceMode::Handed { handover } | SurfaceMode::Rebinding { handover } => Some(handover),
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

/// The first frame, painted while the window was hidden, into the window that now shows, whole
/// (nothing composes the window before its cloak lifts).
fn copy_first(cpu: &mut Cpu, window: &Window, size: (u32, u32)) {
    let (Shown::Waiting, Buffer::Ready(dib)) = (&cpu.shown, &cpu.buffer) else { return };
    let Some(hwnd) = crate::platform::win32::hwnd(window) else { return };
    dib.present(hwnd, size, Blit::All);
    cpu.shown = Shown::Copied(size);
    crate::marks::mark("frame.first.copied");
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
            let mode = SurfaceMode::Cpu { cpu: Cpu { buffer: Buffer::Touching(prefault, capacity), painter, half, shown: Shown::Nothing, show: Some(focus) }, job: GpuJob::Waiting { tries: 0, retry_at: None } };
            Native { cloak, frame, mode, a11y_start, ..Default::default() }
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
            cursor: crate::cursor::os::OsCursor::new(),
            alpha: None,
            timer_frame: TimerFrame::default(),
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
        // The frame both renderers draw to hand over draws in sight or not: covered, the window
        // would stay on the CPU until it showed again.
        let handing = matches!(l.native.mode, SurfaceMode::HandingOver { done: None, .. });
        // A covered window's owed frame (`CoveredDraw`) draws, settled.
        let cover_owed = !l.minimized() && l.covered_draw.drawing();
        let covered = !owed && !handing && !cover_owed && ((!launcher && !self.life.uncovered()) || l.minimized());
        if covered { return self.covered_frame(); }
        if cover_owed {
            self.rt.settle_all();
            if let Some(w) = self.win.as_mut() { w.covered_draw = w.covered_draw.step(CoveredDrawEvent::Drawn); }
        }
        let Some(l) = self.win.as_ref() else { return };
        if !owed && !handing && !resizing && l.backing_off(Instant::now()) { return; }
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
        // A measuring build profiles the first view: its phases, allocations, nodes and measures.
        #[cfg(feature = "frame-profile")]
        let first_view = (!FIRST_LAYOUT.load(Ordering::Relaxed)).then(first_view_start);
        let (animating, _dt) = if hidden {
            self.clock.hold(now);
            (rt.frame(0.0), 0.0)
        } else {
            self.clock.advance(rt, now, false)
        };
        if hidden { l.hidden_pace.rendered(now); }
        crate::marks::mark_once(&FIRST_LAYOUT, "frame.first.layout");
        #[cfg(feature = "frame-profile")]
        if let Some(start) = first_view { first_view_report(rt, start); }
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
            match painter.paint(&scene, &mut Target::with_stride(dib.bytes(), width, height, stride)) {
                Ok(painted) => {
                    crate::marks::mark_once(&FIRST_PAINT, "frame.first.painted");
                    *shown = match std::mem::replace(shown, Shown::Nothing) {
                        Shown::Copied(size) => {
                            let blit = if size == (width, height) && !owed { Blit::Bounds(&painted.rects) } else { Blit::All };
                            if let Some(hwnd) = crate::platform::win32::hwnd(&l.window) { dib.present(hwnd, (width, height), blit); }
                            Shown::Copied((width, height))
                        }
                        Shown::Waiting | Shown::Nothing => Shown::Waiting,
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
        self.idle = Presented::settle(&mut l.presented, rt, &l.config, animating || stepping::FrameMove::moving(&self.frame_move), next_tick, !hidden, false);
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
        let now = Instant::now();
        if self.win.as_ref().is_some_and(|w| matches!(w.native.mode, SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at, .. }, .. } if retry_at.is_none_or(|at| at <= now)) && w.first_frame.ready()) {
            self.start_gpu(true);
        }
    }

    /// Starts making the GPU for this window on a thread (`below`: at below-normal priority, as
    /// after the first frame; a frame that needs the GPU makes it at normal priority).
    fn start_gpu(&mut self, below: bool) {
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { job, .. } = &mut w.native.mode else { return };
        let GpuJob::Waiting { tries, retry_at } = *job else { return };
        if retry_at.is_some() { crate::marks::mark("gpu.window.retry"); }
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
        match spawned {
            Ok(thread) => { crate::marks::mark("gpu.window.start"); *job = GpuJob::Building { slot, thread, since: Instant::now(), raised: !below, tries: tries + 1 }; }
            Err(error) => self.fail_gpu(format!("its thread couldn't start: {error}"), tries + 1),
        }
    }

    /// The GPU thread finished: attach its device, renderer and surface and start handing over,
    /// or stay on the CPU if it failed. While handing over: the GPU finished a frame.
    pub(super) fn gpu_ready(&mut self) {
        if self.win.as_ref().is_some_and(|w| matches!(w.native.mode, SurfaceMode::HandingOver { .. })) { return self.hand_over_if_done(); }
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { job: GpuJob::Building { slot, tries, .. }, .. } = &w.native.mode else { return };
        let tries = *tries;
        let Some(made) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let prepared = match made {
            Ok(prepared) => prepared,
            Err(error) => return self.fail_gpu(error, tries),
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
        w.native.video_layers.hold_premultiplied(vrr_guard(gpu.adapter.get_info().vendor));
        w.native.mode = SurfaceMode::HandingOver { cpu, handover, done: None, readback: None, since: Instant::now(), nudged: None, tries };
        w.native.slot.reset();
        w.native.damage.reset();
        // D3D12's idle-priority threads compile the first frame's shaders: on a busy machine they
        // would hold the hand-over frame off.
        crate::platform::win32::unidle_d3d_threads_soon();
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
        renderer.set_frame_wake(rt.live_wake());
        if let Some(c) = self.comp.as_mut() {
            let size = (l.config.width, l.config.height);
            c.render(&mut Frame { device: &gpu.device, queue: &gpu.queue, encoder: &mut enc, target: &view, format: target_format, size, dt: 0.0, update_ms: 0.0, rt: &mut *rt, ui: &mut *renderer });
        } else {
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (l.config.width, l.config.height));
            renderer.draw(&mut enc, &view, rt.clear_color());
        }
        // A copy of the frame, for the drift count once it's the one the window commits to: only
        // when it's asked for (`RUNE_CPU_BOOT_DRIFT`), as the count is a diagnostic.
        *readback = (crate::render::cpu_boot::drift_wanted() && l.config.usage.contains(wgpu::TextureUsages::COPY_SRC)).then(|| {
            let (width, height) = (l.config.width, l.config.height);
            let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("rune hand-over readback"), size: u64::from(row) * u64::from(height), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            enc.copy_texture_to_buffer(frame.texture.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) } },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
            Readback { buffer, row, size: (width, height) }
        });
        renderer.submit(&gpu.queue, enc);
        gpu.queue.present(frame);
        l.native.slot.presented();
        let flag = Arc::new(AtomicBool::new(false));
        let set = flag.clone();
        gpu.queue.on_submitted_work_done(move || set.store(true, Ordering::Release));
        *done = Some((flag, Instant::now()));
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
            SurfaceMode::HandingOver { done: Some((done, _)), tries, .. } => {
                let tries = *tries;
                let _ = gpu.device.poll(wgpu::PollType::Poll);
                let lost = gpu.is_lost();
                if !lost && !done.load(Ordering::Acquire) { return; }
                let committed = if lost { Err("its device was lost".to_owned()) } else if let SurfaceMode::HandingOver { handover, .. } = &w.native.mode { handover.commit() } else { unreachable!() };
                if let Err(error) = committed { return self.fail_gpu(format!("the GPU couldn't take over the window: {error}"), tries); }
                let SurfaceMode::HandingOver { cpu, handover, readback, .. } = std::mem::replace(&mut w.native.mode, SurfaceMode::Gpu) else { unreachable!() };
                crate::marks::mark("gpu.window.handed");
                crate::platform::win32::unidle_d3d_threads_soon();
                // A first frame only the GPU could draw showed cloaked: it comes on screen now.
                if w.native.cloak.held() {
                    w.native.cloak.lift(&w.window);
                    crate::marks::mark("window.shown");
                }
                // What DWM shows is in the window's own surface now, not in the buffer: it goes on a
                // worker, after counting how many of its pixels the GPU's same frame drew otherwise.
                // A frame only the GPU draws (Slug's text, a native surface) never showed a CPU
                // frame: its buffer holds nothing to count against.
                let shown = matches!(cpu.shown, Shown::Copied(_));
                if let Some(dib) = cpu.into_dib() {
                    match readback {
                        Some(r) if shown => count_drift(gpu.clone(), r, dib),
                        Some(_) => {
                            eprintln!("rune: cpu boot drift: none to count, the CPU showed no frame (the first needed the GPU)");
                            free_later(dib);
                        }
                        None => free_later(dib),
                    }
                }
                w.native.mode = SurfaceMode::Handed { handover };
                w.presented = None;
                // What was asked for while the GPU finished its frame draws now, on the GPU.
                w.request_redraw();
            }
            SurfaceMode::HandingOver { done: None, .. } | SurfaceMode::Gpu | SurfaceMode::Cpu { .. } | SurfaceMode::Handed { .. } | SurfaceMode::Rebinding { .. } => {}
        }
    }

    /// The GPU couldn't take over (`why`), on its `tries`th try: the window goes on, or back, to
    /// the CPU renderer, and tries again after `RETRY_AFTER`, or stays there after `TRIES` tries.
    fn fail_gpu(&mut self, why: String, tries: u8) {
        eprintln!("rune: the window stays on the CPU renderer for now (try {tries} of {TRIES}): {why}");
        crate::marks::mark("gpu.window.failed");
        if !crate::marks::noted("gpu.window.why") { crate::marks::note("gpu.window.why", Box::leak(why.into_boxed_str())); }
        let Some(w) = self.win.as_mut() else { return };
        let job = if tries < TRIES { GpuJob::Waiting { tries, retry_at: Some(Instant::now() + RETRY_AFTER) } } else { GpuJob::Failed };
        let (SurfaceMode::Cpu { cpu, .. } | SurfaceMode::HandingOver { cpu, .. }) = std::mem::replace(&mut w.native.mode, SurfaceMode::Gpu) else { unreachable!("only a CPU window hands over") };
        w.native.mode = SurfaceMode::Cpu { cpu, job };
        // From a hand-over: its swap chain and GPU go. DWM kept the window's own content, which
        // the CPU goes on drawing.
        w.surface = None;
        self.gpu = None;
        w.request_redraw();
    }

    /// The hand-over's deadlines, at the start of a turn (`Win::handover_deadline` wakes the loop
    /// for them): a GPU thread still making the GPU is raised to normal priority, then logged as
    /// slow; a frame both renderers draw is asked for until one is, in sight or not; a GPU that
    /// doesn't finish that frame fails; a failed try is made again.
    pub(super) fn watch_handover(&mut self, now: Instant) {
        let Some(w) = self.win.as_mut() else { return };
        match &mut w.native.mode {
            SurfaceMode::Cpu { job: GpuJob::Building { thread, since, raised, .. }, .. } => {
                if !*raised && now >= *since + RAISE_AFTER {
                    *raised = true;
                    raise_priority(thread);
                    crate::marks::mark("gpu.window.raised");
                }
                if now >= *since + BUILD_SLOW { crate::marks::mark("gpu.window.slow"); }
            }
            SurfaceMode::HandingOver { done: None, since, nudged, tries, .. } => {
                if now >= *since + FINISH_LIMIT {
                    let tries = *tries;
                    return self.fail_gpu(format!("no frame could be drawn by both renderers in {} s", FINISH_LIMIT.as_secs()), tries);
                }
                if now < nudged.unwrap_or(*since) + NUDGE_EVERY { return; }
                *nudged = Some(now);
                crate::marks::mark("gpu.window.nudged");
                w.request_redraw();
            }
            SurfaceMode::HandingOver { done: Some((_, at)), tries, .. } => {
                if now < *at + FINISH_LIMIT { return; }
                let tries = *tries;
                self.fail_gpu(format!("the GPU didn't finish the hand-over frame in {} s", FINISH_LIMIT.as_secs()), tries);
            }
            SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at: Some(at), .. }, .. } => {
                if now >= *at && w.first_frame.ready() { self.start_gpu(true); }
            }
            SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at: None, .. } | GpuJob::Failed, .. }
            | SurfaceMode::Gpu | SurfaceMode::Handed { .. } | SurfaceMode::Rebinding { .. } => {}
        }
    }
}

impl Win {
    /// When the hand-over next has something to do (`Host::watch_handover`).
    pub(super) fn handover_deadline(&self, now: Instant) -> Option<Instant> {
        match &self.native.mode {
            SurfaceMode::Cpu { job: GpuJob::Building { since, raised: false, .. }, .. } => Some(*since + RAISE_AFTER),
            SurfaceMode::Cpu { job: GpuJob::Building { since, .. }, .. } => Some(*since + BUILD_SLOW).filter(|&at| at > now),
            SurfaceMode::HandingOver { done: None, since, nudged, .. } => Some(nudged.unwrap_or(*since) + NUDGE_EVERY),
            SurfaceMode::HandingOver { done: Some((_, at)), .. } => Some(*at + FINISH_LIMIT),
            SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at, .. }, .. } => retry_at.filter(|&at| at > now),
            SurfaceMode::Cpu { job: GpuJob::Failed, .. } | SurfaceMode::Gpu | SurfaceMode::Handed { .. } | SurfaceMode::Rebinding { .. } => None,
        }
    }
}

/// Runs the GPU thread at normal priority (`RAISE_AFTER`).
fn raise_priority(thread: &std::thread::JoinHandle<()>) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Threading::{SetThreadPriority, THREAD_PRIORITY_NORMAL};
    // SAFETY: the GPU thread's own handle, open as long as `thread` is.
    unsafe { SetThreadPriority(thread.as_raw_handle(), THREAD_PRIORITY_NORMAL) };
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


/// Whether the window's swap chain composites premultiplied for good, so the display's refresh
/// rate doesn't follow its presents. With G-SYNC in windowed mode (NVIDIA's, off by default) the
/// display refreshes when the focused window presents an opaque flip-model or DirectComposition
/// swap chain, so lone presents (a hover) swing the rate, and many OLED panels' brightness with it
/// (hex#689). A premultiplied DirectComposition swap chain keeps G-SYNC off (measured on that
/// hardware, as reported there). It costs DWM a blend of the regions the window
/// presents, and nothing while it rests. A fixed-rate display (blade's Iris panel) needs nothing:
/// its refresh doesn't depend on who presents.
fn vrr_guard(vendor: u32) -> bool { vendor == 0x10DE }

/// The first view's starting counts: this thread's allocations and allocated bytes.
#[cfg(feature = "frame-profile")]
fn first_view_start() -> (u64, Option<u64>) {
    let _ = crate::frame::profile::take();
    let _ = crate::runtime::measure_counts::take();
    (procwatch::alloc::thread_allocations(), procwatch::alloc::thread_allocated_bytes())
}

/// The first view's profile, as launch-trace metrics (`first.*`): its phases in µs (view, commit,
/// layout, text; nested, so not to be summed), allocations and bytes, nodes built and those
/// inside the viewport, layout passes, leaf measures and leaves measured more than once, texts
/// shaped.
#[cfg(feature = "frame-profile")]
fn first_view_report<S>(rt: &Runtime<S>, (allocs, bytes): (u64, Option<u64>)) {
    let p = crate::frame::profile::take();
    for (i, name) in ["first.view_us", "first.commit_us", "first.layout_us", "first.text_us"].into_iter().enumerate() {
        crate::marks::add(name, (p[i] * 1000.0) as u64);
    }
    crate::marks::add("first.allocs", procwatch::alloc::thread_allocations().saturating_sub(allocs));
    if let (Some(b0), Some(b1)) = (bytes, procwatch::alloc::thread_allocated_bytes()) { crate::marks::add("first.alloc_bytes", b1.saturating_sub(b0)); }
    let (w, h) = rt.viewport;
    let visible = rt.nodes.iter().filter(|n| { let [x0, y0, x1, y1] = n.target; x1 > x0 && y1 > y0 && x1 > 0.0 && y1 > 0.0 && x0 < w && y0 < h }).count();
    crate::marks::add("first.nodes", rt.nodes.len() as u64);
    crate::marks::add("first.visible_nodes", visible as u64);
    let (passes, measures, measured, again) = crate::runtime::measure_counts::take();
    crate::marks::add("first.layout_passes", passes);
    crate::marks::add("first.measures", measures);
    crate::marks::add("first.leaves_measured", measured);
    crate::marks::add("first.leaves_measured_again", again);
    crate::marks::add("first.shaped", u64::from(rt.stats.shaped));
}
