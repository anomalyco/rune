//! Windows: background GPU creation and seamless DirectComposition hand-over from a CPU-opened
//! window (`cpu_frame`) to the GPU renderer.
//!
//! After the first CPU frame, on the loop's first idle turn, a thread at below-normal priority
//! makes the GPU device, Rune's renderer, an uncommitted DirectComposition visual on the window
//! and a swap chain on it (`platform::win32::dcomp_handover`), so none of that competes with the
//! first frame. When it's ready both renderers draw every frame from the same instances; on the
//! first turn that starts with the GPU's previous frame finished, the visual is committed and DWM
//! shows the GPU's swap chain from its next composition, the same pose the CPU last showed or the
//! one after. The CPU's buffer goes on a worker a turn later. A GPU that can't be made leaves the
//! window on the CPU, fully live.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::cpu_frame::{Cpu, Shown};
use crate::platform::win32::dcomp_handover::Handover;
use crate::platform::win32::dib::{Dib, free_later};
use crate::render::cpu_boot::HalfRounding;
use crate::render::{Gpu, Renderer};
use crate::time::Instant;
use crate::window::WakeEvent;
use crate::window::host::{CompositorFrame, Host};
use crate::window::surface::{Win, acquire, configure, drawable_size, max_surface_dim, reblend_core, rebuild_renderer_core, surface_config};

/// Where a window's frames go. Windows that need the GPU from their first frame (a compositor,
/// a translucent window, an overlay) are `Gpu` from the start, as on every other platform.
#[derive(Default)]
pub(in crate::window) enum SurfaceMode {
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
    /// `paused`: input came while handing over, so the CPU draws it and the hand-over waits for a
    /// frame at rest (the visual is uncommitted: DWM goes on showing the CPU's frames).
    HandingOver { cpu: Cpu, handover: Handover, done: Option<(Arc<AtomicBool>, Instant)>, readback: Option<Readback>, since: Instant, nudged: Option<Instant>, tries: u8, paused: bool },
    /// The GPU draws through the committed visual, which lives as long as the window's surface.
    Handed { handover: Handover },
    /// The GPU's swap chain was made again (a lost device) on the same visual: committed once its
    /// first frame has presented and finished (`Host::draw`).
    Rebinding { handover: Handover },
}

/// A GPU frame copied into a buffer the CPU can read: rows `row` bytes apart, `size` pixels.
pub(in crate::window) struct Readback {
    buffer: wgpu::Buffer,
    row: u32,
    size: (u32, u32),
}

/// The GPU a CPU window is waiting for. Every step has a deadline (`Host::watch_handover`), and
/// every way it ends is logged: handed over (`gpu.window.handed`), or failed with a reason
/// (`gpu.window.failed`, note `gpu.window.why`) and tried again after `RETRY_AFTER`, `TRIES`
/// times in all, then the CPU for good.
pub(in crate::window) enum GpuJob {
    /// Not started: the first frame isn't out yet, or a failed try (`tries` so far) waits for
    /// `retry_at`.
    Waiting { tries: u8, retry_at: Option<Instant> },
    /// Being made on `thread` since `since`; it reports through `slot`. `raised`: the thread runs
    /// at normal priority now (`RAISE_AFTER`). `latest`: the last CPU frame's instances, which the
    /// thread draws once before it reports (`warm_frame`). `animating`: the last CPU frame moved
    /// something. `ready`: since when the GPU has waited for a frame that doesn't (`gpu_ready`).
    Building { slot: Arc<Mutex<Option<Result<Prepared, String>>>>, latest: Arc<Mutex<Vec<crate::render::Instance>>>, thread: std::thread::JoinHandle<()>, since: Instant, raised: bool, tries: u8, animating: bool, ready: Option<Instant> },
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
/// The longest a GPU that's ready waits for a CPU frame that moves nothing to hand over at: an
/// animation that doesn't end (a spinner) hands over within it.
const QUIET_WAIT: Duration = Duration::from_secs(1);
/// A failed try is made again after this long.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Tries in all before the window stays on the CPU.
const TRIES: u8 = 3;

/// What the GPU thread makes for a window.
pub(in crate::window) struct Prepared {
    gpu: Gpu,
    renderer: Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    handover: Handover,
}

// SAFETY: wgpu's objects are `Send`; a new `Renderer` has no native surface renderer (the one
// part that isn't), and `Handover` is used by one thread at a time.
#[allow(unsafe_code)]
unsafe impl Send for Prepared {}

impl SurfaceMode {
    /// Whether the CPU renderer draws this window's frames now.
    pub(in crate::window) fn cpu_draws(&self) -> bool { matches!(self, SurfaceMode::Cpu { .. } | SurfaceMode::HandingOver { .. }) }

    /// The visual a handed-over window's swap chain goes on.
    pub(in crate::window) fn visual(&self) -> Option<*mut std::ffi::c_void> {
        match self {
            SurfaceMode::Handed { handover } | SurfaceMode::Rebinding { handover } => Some(handover.visual()),
            SurfaceMode::Gpu | SurfaceMode::Cpu { .. } | SurfaceMode::HandingOver { .. } => None,
        }
    }

    /// The DirectComposition tree of a handed-over window, for a playing clip's own layer
    /// (`video_layer`).
    pub(in crate::window) fn handover(&self) -> Option<&Handover> {
        match self {
            SurfaceMode::Handed { handover } | SurfaceMode::Rebinding { handover } => Some(handover),
            SurfaceMode::Gpu | SurfaceMode::Cpu { .. } | SurfaceMode::HandingOver { .. } => None,
        }
    }

    /// A new swap chain went on the handed-over window's visual (`Host::reattach`).
    pub(in crate::window) fn rebind(&mut self) {
        if let SurfaceMode::Handed { .. } = self {
            let SurfaceMode::Handed { handover } = std::mem::take(self) else { unreachable!() };
            *self = SurfaceMode::Rebinding { handover };
        }
    }
}

impl Host {
    /// The first idle turn after the window's first CPU frame: the GPU starts on its thread
    /// (`RUNE_CPU_FIRST=only`: not until a frame needs it, for checking the CPU window alone).
    pub(in crate::window) fn cpu_idle(&mut self) {
        if self.gpu_owed() { self.start_gpu(true); }
    }

    /// Whether the GPU is due to start now (`cpu_idle`).
    pub(in crate::window) fn gpu_owed(&self) -> bool {
        if std::env::var_os("RUNE_CPU_FIRST").is_some_and(|v| v == "only") { return false; }
        let now = crate::time::now();
        self.win.as_ref().is_some_and(|w| matches!(w.native.mode, SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at, .. }, .. } if retry_at.is_none_or(|at| at <= now)) && w.first_frame.ready())
    }

    /// Starts making the GPU for this window on a thread (`below`: at below-normal priority, as
    /// after the first frame; a frame that needs the GPU makes it at normal priority).
    pub(super) fn start_gpu(&mut self, below: bool) {
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { job, .. } = &mut w.native.mode else { return };
        let GpuJob::Waiting { tries, retry_at } = *job else { return };
        if retry_at.is_some() { crate::marks::mark("gpu.window.retry"); }
        let Some(hwnd) = crate::platform::win32::hwnd(&w.window) else { *job = GpuJob::Failed; return };
        let slot = Arc::new(Mutex::new(None));
        let (out, handle, srgb, size) = (slot.clone(), self.native_handle.clone(), self.app.rt().srgb_blend(), (w.config.width, w.config.height));
        // What the window draws, for the GPU thread's first frame of its own (`warm_frame`): each CPU
        // frame until then replaces it.
        let latest = Arc::new(Mutex::new(warm_instances(&self.app.rt().instances)));
        let instances = latest.clone();
        let features = self.app.compositor_features();
        let hwnd = hwnd as isize;
        let spawned = std::thread::Builder::new().name("rune-gpu-window".into()).spawn(move || {
            if below { lower_priority(); }
            let made = prepare(hwnd, size, srgb, features, &instances);
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(made);
            handle.send(WakeEvent::GpuReady);
        });
        match spawned {
            Ok(thread) => { crate::marks::mark("gpu.window.start"); *job = GpuJob::Building { slot, latest, thread, since: crate::time::now(), raised: !below, tries: tries + 1, animating: false, ready: None }; }
            Err(error) => self.fail_gpu(format!("its thread couldn't start: {error}"), tries + 1),
        }
    }

    /// The GPU thread finished: attach its device, renderer and surface and start handing over,
    /// or stay on the CPU if it failed. While handing over: the GPU finished a frame.
    pub(in crate::window) fn gpu_ready(&mut self) {
        if self.win.as_ref().is_some_and(|w| matches!(w.native.mode, SurfaceMode::HandingOver { .. })) { return self.hand_over_if_done(); }
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { job: GpuJob::Building { slot, tries, animating, ready, .. }, .. } = &mut w.native.mode else { return };
        let tries = *tries;
        let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
        // Ready while the CPU frames move something (a view fading in): the hand-over waits for a
        // frame at rest, so the frame both renderers draw is one that costs what a frame at rest
        // does, not the first of the GPU's frames to draw every layer of an entrance.
        let now = crate::time::now();
        if slot.as_ref().is_some_and(Result::is_ok) && *animating && now < *ready.get_or_insert(now) + QUIET_WAIT {
            crate::marks::mark("gpu.window.deferred");
            return;
        }
        let Some(made) = slot.take() else { return };
        drop(slot);
        let prepared = match made {
            Ok(prepared) => prepared,
            Err(error) => return self.fail_gpu(error, tries),
        };
        crate::marks::mark("gpu.window.ready");
        let Prepared { gpu, mut renderer, surface, mut config, handover } = prepared;
        let gpu = Arc::new(gpu);
        if renderer.srgb_blend() != self.app.rt().srgb_blend() {
            if !reblend_core(&surface, &gpu, &mut config, &mut renderer, &mut w.alpha, self.app.rt_mut()) {
                return self.fail_gpu("couldn't reconfigure the surface for the window's blend mode".into(), tries);
            }
        } else {
            let target_format = if self.app.rt().srgb_blend() { config.format.remove_srgb_suffix() } else { config.format };
            rebuild_renderer_core(&gpu.device, &mut renderer, target_format, self.app.rt_mut());
        }
        *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
        crate::render::surface_gpu(&gpu);
        let size = w.window.inner_size();
        let SurfaceMode::Cpu { mut cpu, .. } = std::mem::replace(&mut w.native.mode, SurfaceMode::Gpu) else { unreachable!() };
        cpu.half = HalfRounding::of_vendor(gpu.adapter.get_info().vendor);
        w.native.video_layers.hold_premultiplied(vrr_guard(gpu.adapter.get_info().vendor));
        w.native.mode = SurfaceMode::HandingOver { cpu, handover, done: None, readback: None, since: crate::time::now(), nudged: None, tries, paused: false };
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
    pub(super) fn gpu_draws_same_frame(&mut self) {
        let (Some(l), Some((gpu, renderer))) = (self.win.as_mut(), self.gpu.as_mut()) else { return };
        if !matches!(l.native.mode, SurfaceMode::HandingOver { paused: false, .. }) { return; }
        if renderer.srgb_blend() != self.app.rt().srgb_blend() {
            let Some(surface) = l.surface.as_ref() else { return };
            let ok = reblend_core(surface, gpu, &mut l.config, renderer, &mut l.alpha, self.app.rt_mut());
            l.native.slot.reconfigured(ok);
            if !ok {
                l.surface_ok = false;
                return;
            }
        }
        let SurfaceMode::HandingOver { done, readback, paused: false, .. } = &mut l.native.mode else { return };
        let Some(surface) = l.surface.as_ref() else { return };
        let Ok(frame) = acquire(surface, gpu, &l.config, l.surface_ok, 1, &mut self.meter) else { return };
        let target_format = if self.app.rt().srgb_blend() { l.config.format.remove_srgb_suffix() } else { l.config.format };
        debug_assert_eq!(renderer.format(), target_format);
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor { format: (target_format != l.config.format).then_some(target_format), ..Default::default() });
        {
            let rt = self.app.rt_mut();
            renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            renderer.build_surfaces(&gpu.device, &rt.surfaces, rt.live_notifier());
            renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (l.config.width, l.config.height));
            renderer.set_frame_wake(rt.live_wake());
        }
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        // The app's compositor draws the same instances the CPU painted (the CPU frame drew none of
        // its effect slots, or it would have asked for the GPU).
        let size = (l.config.width, l.config.height);
        if self.app.has_compositor() {
            let _ = self.app.render_compositor(CompositorFrame {
                device: &gpu.device,
                queue: &gpu.queue,
                encoder: &mut enc,
                target: &view,
                format: target_format,
                size,
                dt: 0.0,
                update_ms: 0.0,
                ui: &mut *renderer,
            });
        } else {
            let rt = self.app.rt_mut();
            renderer.set_fade_stops(rt.fade_stops());
            renderer.prepare_ui(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
            renderer.draw(&mut enc, &view, rt.clear_color());
        }
        // A copy of the frame, for the drift count once it's the one the window commits to: only
        // when it's asked for (`RUNE_CPU_BOOT_DRIFT`), as the count is a diagnostic.
        *readback = (crate::render::cpu_boot::drift_wanted() && l.config.usage.contains(wgpu::TextureUsages::COPY_SRC)).then(|| {
            let (width, height) = size;
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
        *done = Some((flag, crate::time::now()));
        // A thread waits for the GPU and wakes the loop, which commits (`hand_over_if_done`).
        let (waiting, handle) = (gpu.clone(), self.native_handle.clone());
        let spawned = std::thread::Builder::new().name("rune-gpu-wait".into()).spawn(move || {
            rune_gpu::health::poll(&waiting.device, wgpu::PollType::wait_indefinitely()).ok();
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
    pub(in crate::window) fn hand_over_if_done(&mut self) {
        let (Some(w), Some((gpu, _))) = (self.win.as_mut(), self.gpu.as_ref()) else { return };
        match &mut w.native.mode {
            SurfaceMode::HandingOver { done: Some((done, _)), tries, .. } => {
                let tries = *tries;
                rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
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
        let job = if tries < TRIES { GpuJob::Waiting { tries, retry_at: Some(crate::time::now() + RETRY_AFTER) } } else { GpuJob::Failed };
        let (SurfaceMode::Cpu { cpu, .. } | SurfaceMode::HandingOver { cpu, .. }) = std::mem::replace(&mut w.native.mode, SurfaceMode::Gpu) else { unreachable!("only a CPU window hands over") };
        w.native.mode = SurfaceMode::Cpu { cpu, job };
        // From a hand-over: its swap chain and GPU go. DWM kept the window's own content, which
        // the CPU goes on drawing.
        w.native.slot.reset();
        w.surface = None;
        self.gpu = None;
        w.request_redraw();
    }

    /// The hand-over's deadlines, at the start of a turn (`Win::handover_deadline` wakes the loop
    /// for them): a GPU thread still making the GPU is raised to normal priority, then logged as
    /// slow; a frame both renderers draw is asked for until one is, in sight or not; a GPU that
    /// doesn't finish that frame fails; a failed try is made again.
    pub(in crate::window) fn watch_handover(&mut self, now: Instant) {
        let Some(w) = self.win.as_mut() else { return };
        match &mut w.native.mode {
            SurfaceMode::Cpu { job: GpuJob::Building { thread, since, raised, ready, .. }, .. } => {
                if ready.is_some_and(|at| now >= at + QUIET_WAIT) { return self.gpu_ready(); }
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
    pub(in crate::window) fn handover_deadline(&self, now: Instant) -> Option<Instant> {
        match &self.native.mode {
            SurfaceMode::Cpu { job: GpuJob::Building { ready: Some(at), .. }, .. } => Some(*at + QUIET_WAIT),
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
#[allow(unsafe_code)]
fn raise_priority(thread: &std::thread::JoinHandle<()>) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Threading::{SetThreadPriority, THREAD_PRIORITY_NORMAL};
    // SAFETY: the GPU thread's own handle, open as long as `thread` is.
    unsafe { SetThreadPriority(thread.as_raw_handle(), THREAD_PRIORITY_NORMAL) };
}

static FIRST_GPU: AtomicBool = AtomicBool::new(false);

/// On the GPU thread: the device (the one `warm_gpu` made, if it did), the renderer, the
/// uncommitted visual and a swap chain on it, configured for `size`.
#[allow(unsafe_code)]
fn prepare(hwnd: isize, size: (u32, u32), srgb: bool, features: wgpu::Features, latest: &Mutex<Vec<crate::render::Instance>>) -> Result<Prepared, String> {
    let handover = Handover::new(hwnd)?;
    let visual = handover.visual();
    // SAFETY: the visual stays alive while the surface does: the surface takes its own reference
    // to it, and the window keeps the `Handover` as long as it keeps the surface.
    let (gpu, surface, renderer) = Gpu::for_surface(features, false,
        |instance| unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(visual)) }.map_err(|e| e.to_string()))?;
    let size = drawable_size(None, size, max_surface_dim(Some(&gpu)));
    let (config, _) = surface_config(&surface, &gpu.adapter, size, srgb).ok_or("the GPU can't present to this window")?;
    configure(&surface, &gpu, &config)?;
    let target_format = if srgb { config.format.remove_srgb_suffix() } else { config.format };
    let mut renderer = renderer.unwrap_or_else(|| Renderer::new(&gpu.device, target_format));
    let instances = std::mem::take(&mut *latest.lock().unwrap_or_else(|e| e.into_inner()));
    warm_frame(&gpu, &mut renderer, target_format, srgb, (config.width, config.height), &instances);
    crate::marks::mark("gpu.window.warmed");
    Ok(Prepared { gpu, renderer, surface, config, handover })
}

/// On the GPU thread, before the hand-over: the renderer draws the window's latest CPU frame once,
/// into a target of the swap chain's format, and waits for the GPU. Its first frame's costs are
/// paid here, so the hand-over frame on the app's thread costs what a frame does: the pipelines
/// its layers' formats need, its layers' targets (the hand-over frame's layers are the latest
/// frame's, or nearly), the staging space and the instance buffer. Its own target goes as soon as
/// the GPU is done with it. The atlas is an empty one: the glyphs land with the hand-over frame's
/// own prepare.
fn warm_frame(gpu: &Gpu, renderer: &mut Renderer, format: wgpu::TextureFormat, srgb: bool, size: (u32, u32), instances: &[crate::render::Instance]) {
    renderer.set_format(&gpu.device, format);
    renderer.set_srgb_blend(srgb);
    debug_assert_eq!(renderer.format(), format);
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rune warm frame"),
        size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut atlas = crate::text::Atlas::new();
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.prepare_ui(&gpu.device, &gpu.queue, &mut atlas, instances, size);
    renderer.draw(&mut enc, &view, [0.0; 4]);
    renderer.submit(&gpu.queue, enc);
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok();
    target.destroy();
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
}

/// A CPU frame's instances as the warm frame draws them: what reads the app's own state (an
/// image's slot, a glyph's atlas page, a compositor's effect) draws as a plain box, so the frame
/// needs nothing but the renderer.
pub(super) fn warm_instances(instances: &[crate::render::Instance]) -> Vec<crate::render::Instance> {
    use crate::render::{KIND_RECT, Kind};
    instances
        .iter()
        .map(|i| {
            let mut i = *i;
            match i.kind() {
                Some(Kind::Image | Kind::Effect) => i.params[1] = KIND_RECT,
                Some(Kind::Glyph) => i.params[2] = 0.0,
                _ => {}
            }
            i
        })
        .collect()
}

/// On a worker: maps the GPU frame's copy, counts the pixels that differ from the CPU's buffer
/// (`cpu_boot::drift` logs one line and keeps `last_drift`), and frees both.
fn count_drift(gpu: Arc<Gpu>, readback: Readback, mut dib: Dib) {
    let spawned = std::thread::Builder::new().name("rune-drift".into()).spawn(move || {
        let slice = readback.buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        if rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).is_err() { return; }
        let (width, height) = readback.size;
        let stride = dib.stride();
        let Ok(gpu_px) = slice.get_mapped_range() else { return };
        crate::render::cpu_boot::drift(crate::render::cpu_boot::Pixels::bgra(dib.bytes(), width, height, stride), crate::render::cpu_boot::Pixels::bgra(&gpu_px, width, height, readback.row as usize));
    });
    if let Err(error) = spawned { eprintln!("rune: could not count the hand-over's drift: {error}"); }
}

#[allow(unsafe_code)]
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

#[cfg(test)]
#[path = "handover_tests.rs"]
mod tests;
