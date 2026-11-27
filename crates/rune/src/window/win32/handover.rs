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
use crate::gpu::fallback::Policy;
use crate::render::cpu_boot::{HalfRounding, NeedsGpu};
use crate::render::{Gpu, Renderer, SurfaceSetupError, format_window_setup_error, with_wine_hwnd_hint};
use crate::window::CpuFirst;
use crate::time::Instant;
pub(super) use crate::window::warm_frame::{warm_frame, warm_instances};
use crate::window::WakeEvent;
use crate::window::host::{CompositorFrame, Host};
use crate::window::surface::{Win, acquire, configure_checked, drawable_size, max_surface_dim, reblend_core, rebuild_renderer_core, surface_config};

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
    /// Automatic idle handover skipped because only software adapters (WARP) exist on this
    /// machine. Never retries on idle; transitions to urgent `Building` when a frame hits
    /// `NeedsGpu`.
    SoftwareDeferred,
    /// Being made on `thread` since `since`; it reports through `slot`. `raised`: the thread runs
    /// at normal priority now (`RAISE_AFTER`). `latest`: the last CPU frame's instances, which the
    /// thread draws once before it reports (`warm_frame`). `animating`: the last CPU frame moved
    /// something. `ready`: since when the GPU has waited for a frame that doesn't (`gpu_ready`).
    /// `urgent`: started or escalated by a `NeedsGpu` frame (bypasses `QUIET_WAIT` and allows
    /// software adapters).
    Building { slot: Arc<Mutex<Option<PrepareOutcome>>>, latest: Arc<Mutex<Vec<crate::render::Instance>>>, thread: std::thread::JoinHandle<()>, since: Instant, raised: bool, tries: u8, animating: bool, ready: Option<Instant>, urgent: bool },
    /// It failed `TRIES` times: the window stays on the CPU.
    Failed,
}

/// Outcome of the background GPU preparation thread (`prepare`).
pub(in crate::window) enum PrepareOutcome {
    Ready(Box<Prepared>),
    /// Automatic idle handover fell through to a software GPU (`DeviceType::Cpu`): dropped before
    /// configuring a swap chain or compiling shaders.
    SoftwareDeferred,
    Failed(String),
}

/// Pure idle-handover decision for a Windows CPU window whose `GpuJob` is `Waiting`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IdleHandover {
    /// Not due yet or suppressed (`CpuFirst::Only`).
    Skip,
    /// Only software adapters exist under `CpuFirst::Auto` + `Policy::Automatic`: transition to
    /// `GpuJob::SoftwareDeferred` with zero device, swapchain, or shader work.
    DeferSoftware,
    /// Start background GPU handover; when `defer_software` is true, a fallback to
    /// `DeviceType::Cpu` inside `prepare` drops cleanly into `GpuJob::SoftwareDeferred`.
    Start { defer_software: bool },
}

/// Pure decision on how to respond when `CpuPainter` refuses a frame with `NeedsGpu`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NeedsGpuFrameAction {
    /// First frame (`!first_frame.ready()`): keep the window cloaked until the GPU draws it.
    WaitForGpu,
    /// Mid-session degradable feature: paint live without the GPU-only modifier while the GPU
    /// builds off the UI thread.
    PaintDegraded,
    /// Mid-session non-degradable feature (`SlugText`, `OutlineGlyph`, `UnknownKind`, `Target`):
    /// keep the previous frame until the GPU takes over.
    KeepPreviousFrame,
}

/// Pure idle-handover policy for Windows `CpuFirst` modes and adapter fallback policies.
pub(super) fn idle_handover_action(
    mode: CpuFirst,
    policy: Policy,
    ready_and_due: bool,
    probe_hardware_dxgi: impl FnOnce() -> Option<bool>,
) -> IdleHandover {
    if !mode.gpu_owed(ready_and_due) {
        return IdleHandover::Skip;
    }
    if mode == CpuFirst::Auto && policy == Policy::Automatic {
        if probe_hardware_dxgi() == Some(false) {
            return IdleHandover::DeferSoftware;
        }
        return IdleHandover::Start { defer_software: true };
    }
    IdleHandover::Start { defer_software: false }
}

/// Whether `prepare` should abort and defer when the picked adapter is a software GPU.
pub(super) const fn should_defer_prepared_adapter(
    defer_software: bool,
    device_type: wgpu::DeviceType,
) -> bool {
    defer_software && matches!(device_type, wgpu::DeviceType::Cpu)
}

/// Whether `gpu_ready` should defer attaching an already-built GPU while the view animates.
/// Handovers started or escalated by `NeedsGpu` (`urgent == true`) bypass `QUIET_WAIT` (0 ms).
pub(super) fn should_defer_quiet_wait(
    ready_ok: bool,
    animating: bool,
    urgent: bool,
    now: Instant,
    ready_since: Instant,
) -> bool {
    ready_ok && animating && !urgent && now < ready_since + QUIET_WAIT
}

/// Whether `hand_over_if_done` should run `count_drift` against the last CPU buffer.
/// Skipped when the last painted CPU frame was a degraded fallback (different visual output by
/// design) or when no CPU frame was copied to the window.
pub(super) const fn should_count_drift(shown_copied: bool, degraded: bool) -> bool {
    shown_copied && !degraded
}

/// How `prepare_render` handles a `NeedsGpu` error from `CpuPainter`.
pub(super) const fn needs_gpu_frame_action(
    first_frame_ready: bool,
    reason: NeedsGpu,
) -> NeedsGpuFrameAction {
    if !first_frame_ready {
        NeedsGpuFrameAction::WaitForGpu
    } else if reason.is_degradable() {
        NeedsGpuFrameAction::PaintDegraded
    } else {
        NeedsGpuFrameAction::KeepPreviousFrame
    }
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

/// What the GPU thread makes for a window. `handover` is `Some` on DirectComposition and
/// `None` when `WGPU_DX12_PRESENTATION_SYSTEM=hwnd` selects a plain HWND swap chain (opaque only).
pub(in crate::window) struct Prepared {
    gpu: Gpu,
    renderer: Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    handover: Option<Handover>,
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

    #[cfg(feature = "windows-system-material")]
    pub(in crate::window) fn handover_mut(&mut self) -> Option<&mut Handover> {
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
    /// The first idle turn after the window's first CPU frame: starts background GPU handover
    /// unless suppressed (`CpuFirst::Only`) or deferred on a software-only machine under
    /// `CpuFirst::Auto` + `Policy::Automatic`.
    pub(in crate::window) fn cpu_idle(&mut self) {
        if self.poll_gpu_loss() { return; }
        let now = crate::time::now();
        let due = self.win.as_ref().is_some_and(|w| matches!(w.native.mode, SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at, .. }, .. } if retry_at.is_none_or(|at| at <= now)) && w.first_frame.ready());
        match idle_handover_action(
            self.effective_cpu_first(),
            crate::gpu::pick::env_policy(),
            due,
            crate::gpu::pick::dxgi_has_hardware_adapter,
        ) {
            IdleHandover::Skip => {}
            IdleHandover::DeferSoftware => {
                if let Some(w) = self.win.as_mut()
                    && let SurfaceMode::Cpu { job, .. } = &mut w.native.mode
                {
                    *job = GpuJob::SoftwareDeferred;
                    crate::marks::mark("gpu.window.software_deferred");
                }
            }
            IdleHandover::Start { defer_software } => self.start_gpu_with(true, false, defer_software),
        }
    }

    /// Whether the GPU is due to start now (`cpu_idle`).
    pub(in crate::window) fn gpu_owed(&self) -> bool {
        let now = crate::time::now();
        let due = self.win.as_ref().is_some_and(|w| matches!(w.native.mode, SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at, .. }, .. } if retry_at.is_none_or(|at| at <= now)) && w.first_frame.ready());
        self.effective_cpu_first().gpu_owed(due)
    }

    /// Starts or escalates GPU handover when a frame hits `NeedsGpu`: allows software adapters,
    /// runs at normal priority, and bypasses `QUIET_WAIT` deferral.
    pub(super) fn start_gpu_for_needs_gpu(&mut self) {
        if self.poll_gpu_loss() { return; }
        let mut wake_ready = false;
        let should_start = {
            let Some(w) = self.win.as_mut() else { return };
            let SurfaceMode::Cpu { job, .. } = &mut w.native.mode else { return };
            match job {
                GpuJob::Waiting { .. } => true,
                GpuJob::SoftwareDeferred => {
                    *job = GpuJob::Waiting { tries: 0, retry_at: None };
                    true
                }
                GpuJob::Building { urgent, raised, thread, ready, .. } => {
                    *urgent = true;
                    if !*raised {
                        *raised = true;
                        raise_priority(thread);
                        crate::marks::mark("gpu.window.raised");
                    }
                    if ready.is_some() {
                        wake_ready = true;
                    }
                    false
                }
                GpuJob::Failed => false,
            }
        };
        if wake_ready {
            self.native_handle.send(WakeEvent::GpuReady);
        }
        if should_start {
            self.start_gpu_with(false, true, false);
        }
    }

    /// Starts making the GPU for this window on a thread (`below`: at below-normal priority, as
    /// after the first frame; a frame that needs the GPU makes it at normal priority with
    /// `urgent = true` and `defer_software = false`).
    fn start_gpu_with(&mut self, below: bool, urgent: bool, defer_software: bool) {
        if self.poll_gpu_loss() { return; }
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
            let made = prepare(hwnd, size, srgb, features, &instances, defer_software);
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(made);
            handle.send(WakeEvent::GpuReady);
        });
        match spawned {
            Ok(thread) => { crate::marks::mark("gpu.window.start"); *job = GpuJob::Building { slot, latest, thread, since: crate::time::now(), raised: !below, tries: tries + 1, animating: false, ready: None, urgent }; }
            Err(error) => self.fail_gpu(format!("its thread couldn't start: {error}"), tries + 1),
        }
    }

    /// The GPU thread finished: attach its device, renderer and surface and start handing over,
    /// or stay on the CPU if it failed or deferred a software GPU. While handing over: the GPU
    /// finished a frame.
    pub(in crate::window) fn gpu_ready(&mut self) {
        if self.poll_gpu_loss() { return; }
        if self.win.as_ref().is_some_and(|w| matches!(w.native.mode, SurfaceMode::HandingOver { .. })) { return self.hand_over_if_done(); }
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { job: GpuJob::Building { slot, tries, animating, ready, urgent, .. }, .. } = &mut w.native.mode else { return };
        let (tries, urgent) = (*tries, *urgent);
        let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
        // Ready while the CPU frames move something (a view fading in): an idle hand-over waits
        // for a frame at rest, so the frame both renderers draw costs what a frame at rest does.
        // A hand-over triggered or escalated by `NeedsGpu` (`urgent`) skips `QUIET_WAIT` (0 ms).
        let now = crate::time::now();
        let ready_ok = matches!(slot.as_ref(), Some(PrepareOutcome::Ready(_)));
        if ready_ok && *animating && !urgent {
            let ready_since = *ready.get_or_insert(now);
            if should_defer_quiet_wait(true, true, false, now, ready_since) {
                crate::marks::mark("gpu.window.deferred");
                return;
            }
        }
        let Some(made) = slot.take() else { return };
        drop(slot);
        let prepared = match made {
            PrepareOutcome::Ready(prepared) => *prepared,
            PrepareOutcome::SoftwareDeferred => {
                if urgent {
                    // A NeedsGpu frame escalated this idle handover while it was checking adapters:
                    // restart immediately allowing software adapters.
                    if let SurfaceMode::Cpu { job, .. } = &mut w.native.mode {
                        *job = GpuJob::Waiting { tries: 0, retry_at: None };
                    }
                    self.start_gpu_for_needs_gpu();
                    return;
                }
                crate::marks::mark("gpu.window.software_deferred");
                if let SurfaceMode::Cpu { job, .. } = &mut w.native.mode {
                    *job = GpuJob::SoftwareDeferred;
                }
                return;
            }
            PrepareOutcome::Failed(error) => return self.fail_gpu(error, tries),
        };
        crate::marks::mark("gpu.window.ready");
        let Prepared { gpu, mut renderer, surface, mut config, handover } = prepared;
        let gpu = Arc::new(gpu);
        if self.terminal_loss.is_terminal() {
            if gpu.is_lost() || gpu.sample_removal() {
                drop((surface, renderer, gpu));
                self.latch_loss_latch(crate::render::loss_latch::GpuFailureKind::InitFailed, "the GPU device was lost before handover");
                return;
            }
            self.terminal_loss.attach_gpu(&gpu);
        }
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
        match handover {
            Some(handover) => {
                cpu.half = HalfRounding::of_vendor(gpu.adapter.get_info().vendor);
                w.native.video_layers.hold_premultiplied(vrr_guard(gpu.adapter.get_info().vendor));
                w.native.mode = SurfaceMode::HandingOver { cpu, handover, done: None, readback: None, since: crate::time::now(), nudged: None, tries, paused: false };
            }
            None => {
                // Plain HWND swap chain (`DxgiFromHwnd`): no DirectComposition visual to commit
                // seamlessly, so free the CPU DIB, skip the premultiplied video-layer VRR hold,
                // and transition straight to `SurfaceMode::Gpu`.
                if let Some(dib) = cpu.into_dib() {
                    free_later(dib);
                }
                if w.native.cloak.held() {
                    w.native.cloak.lift(&w.window);
                    crate::marks::mark("window.shown");
                }
                w.presented = None;
                self.app.rt_mut().set_renderer_kind(crate::RendererKind::Gpu);
                crate::marks::mark("gpu.window.handed");
            }
        }
        w.native.slot.reset();
        w.native.dxgi_present.reset();
        w.damage.reset();
        self.frame_presented.reset();
        // D3D12's idle-priority threads compile the first frame's shaders: on a busy machine they
        // would hold the hand-over frame off.
        crate::platform::win32::unidle_d3d_threads_soon();
        w.surface = Some(surface);
        w.config = config;
        w.surface_ok = true;
        if gpu.adapter.get_info().device_type == wgpu::DeviceType::Cpu {
            w.surface_retries.note_software();
        }
        // The window changed size while the GPU was made.
        if size.width > 0 && size.height > 0 { w.resize_surface(Some(&*gpu), size); }
        // A window that hands over names its effects when the hand-over is committed
        // (`hand_over_if_done`). Named here, the hand-over frame's own prepare would start the
        // warm worker, and while the GPU finishes that frame nothing is prepared: a frame it is
        // slow to finish would read to the worker as a window at rest.
        if !matches!(w.native.mode, SurfaceMode::HandingOver { .. }) { renderer.set_prewarm_effects(self.prewarm_effects); }
        self.gpu = Some((gpu, renderer));
        w.request_redraw();
    }

    /// While handing over: the GPU draws the instances the CPU just painted, into its swap chain
    /// on the uncommitted visual.
    pub(super) fn gpu_draws_same_frame(&mut self) {
        if self.poll_gpu_loss() { return; }
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
        self.terminal_loss.note_frame_texture_acquired_direct();
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
        if self.terminal_loss.is_terminal() {
            if matches!(rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll), Err(rune_gpu::health::PollFailed::Lost)) {
                gpu.mark_lost();
            }
            if gpu.sample_removal() || gpu.is_lost() || self.terminal_loss.is_failed() {
                drop((enc, view, frame));
                self.note_frame_texture_released();
                let _ = self.poll_gpu_loss();
                return;
            }
        }
        renderer.submit(&gpu.queue, enc);
        drop(view);
        if gpu.is_lost() {
            drop(frame);
            self.note_frame_texture_released();
            let _ = self.poll_gpu_loss();
            return;
        }
        gpu.queue.present(frame);
        self.terminal_loss.note_frame_texture_released_direct();
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
        if self.poll_gpu_loss() { return; }
        let (Some(w), Some((gpu, renderer))) = (self.win.as_mut(), self.gpu.as_mut()) else { return };
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
                // The window is the GPU's: its effects are named now (`gpu_ready`), and the warm
                // worker starts its rest from here.
                renderer.set_prewarm_effects(self.prewarm_effects);
                renderer.warm_effects(&gpu.device);
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
                let shown = should_count_drift(matches!(cpu.shown, Shown::Copied(_)), cpu.degraded);
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
                self.app.rt_mut().set_renderer_kind(crate::RendererKind::Gpu);
                self.sync_window_backdrop(false);
                self.on_resize_sync_event(super::resize_sync::Event::Handover);
            }
            SurfaceMode::HandingOver { done: None, .. } | SurfaceMode::Gpu | SurfaceMode::Cpu { .. } | SurfaceMode::Handed { .. } | SurfaceMode::Rebinding { .. } => {}
        }
    }

    /// The GPU couldn't take over (`why`), on its `tries`th try: the window goes on, or back, to
    /// the CPU renderer, and tries again after `RETRY_AFTER`, or stays there after `TRIES` tries.
    fn fail_gpu(&mut self, why: String, tries: u8) {
        let policy = if self.is_terminal_gpu_policy() { crate::GpuLossPolicy::Terminal } else { crate::GpuLossPolicy::Recover };
        if matches!(crate::window::host::gpu_loss::decide_handover_failure(policy, tries, TRIES), crate::window::host::gpu_loss::HandoverFailureAction::LatchTerminal) {
            if let Some(w) = self.win.as_mut() && let SurfaceMode::Cpu { job, .. } = &mut w.native.mode {
                *job = GpuJob::Failed;
            }
            self.latch_loss_latch(crate::render::loss_latch::GpuFailureKind::InitFailed, why);
            return;
        }
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
        w.native.dxgi_present.reset();
        w.surface = None;
        self.frame_presented.reset();
        self.gpu = None;
        self.app.release_compositor_gpu();
        self.app.rt_mut().set_renderer_kind(crate::RendererKind::Cpu);
        w.request_redraw();
    }

    /// The hand-over's deadlines, at the start of a turn (`Win::handover_deadline` wakes the loop
    /// for them): a GPU thread still making the GPU is raised to normal priority, then logged as
    /// slow; a frame both renderers draw is asked for until one is, in sight or not; a GPU that
    /// doesn't finish that frame fails; a failed try is made again.
    pub(in crate::window) fn watch_handover(&mut self, now: Instant) {
        if self.poll_gpu_loss() { return; }
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
                if now >= *at && w.first_frame.ready() { self.cpu_idle(); }
            }
            SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at: None, .. } | GpuJob::SoftwareDeferred | GpuJob::Failed, .. }
            | SurfaceMode::Gpu | SurfaceMode::Handed { .. } | SurfaceMode::Rebinding { .. } => {}
        }
    }
}

impl Win {
    /// When the hand-over next has something to do (`Host::watch_handover`).
    pub(in crate::window) fn handover_deadline(&self, now: Instant) -> Option<Instant> {
        match &self.native.mode {
            SurfaceMode::Cpu { job: GpuJob::Building { ready: Some(at), urgent: false, .. }, .. } => Some(*at + QUIET_WAIT),
            SurfaceMode::Cpu { job: GpuJob::Building { since, raised: false, .. }, .. } => Some(*since + RAISE_AFTER),
            SurfaceMode::Cpu { job: GpuJob::Building { since, .. }, .. } => Some(*since + BUILD_SLOW).filter(|&at| at > now),
            SurfaceMode::HandingOver { done: None, since, nudged, .. } => Some(nudged.unwrap_or(*since) + NUDGE_EVERY),
            SurfaceMode::HandingOver { done: Some((_, at)), .. } => Some(*at + FINISH_LIMIT),
            SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at, .. }, .. } => retry_at.filter(|&at| at > now),
            SurfaceMode::Cpu { job: GpuJob::SoftwareDeferred | GpuJob::Failed, .. } | SurfaceMode::Gpu | SurfaceMode::Handed { .. } | SurfaceMode::Rebinding { .. } => None,
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

/// Creates a plain HWND `wgpu::Surface` from a raw Win32 `hwnd` on a `DxgiFromHwnd` instance.
#[allow(unsafe_code)]
fn create_hwnd_surface(instance: &wgpu::Instance, hwnd: isize) -> Result<wgpu::Surface<'static>, String> {
    let hwnd = std::num::NonZeroIsize::new(hwnd).ok_or_else(|| "window handle is null".to_owned())?;
    let raw_window_handle = winit::raw_window_handle::RawWindowHandle::Win32(
        winit::raw_window_handle::Win32WindowHandle::new(hwnd),
    );
    let raw_display_handle = winit::raw_window_handle::RawDisplayHandle::Windows(
        winit::raw_window_handle::WindowsDisplayHandle::new(),
    );
    // SAFETY: `hwnd` is a live window of this process, and `Win` drops its `wgpu::Surface` before
    // dropping the native `winit::window::Window`.
    unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(raw_display_handle),
            raw_window_handle,
        })
    }
    .map_err(|e| e.to_string())
}

/// On the GPU thread: the device (the one `warm_gpu` made, if it did), the renderer, the
/// uncommitted DirectComposition visual (when DirectComposition is active) and a swap chain
/// configured for `size`.
enum SetupOutcome {
    Ready(Box<(wgpu::SurfaceConfiguration, Renderer)>),
    SoftwareDeferred,
}

#[allow(unsafe_code)]
fn prepare(hwnd: isize, size: (u32, u32), srgb: bool, features: wgpu::Features, latest: &Mutex<Vec<crate::render::Instance>>, defer_software: bool) -> PrepareOutcome {
    let handover = match Gpu::dx12_swapchain_kind() {
        wgpu::Dx12SwapchainKind::DxgiFromHwnd => None,
        wgpu::Dx12SwapchainKind::DxgiFromVisual => match Handover::new(hwnd).map_err(with_wine_hwnd_hint) {
            Ok(h) => Some(h),
            Err(error) => return PrepareOutcome::Failed(error),
        },
    };
    let setup = Gpu::for_surface_setup(
        features,
        false,
        |instance| match &handover {
            Some(handover) => {
                // SAFETY: the visual stays alive while the surface does: the surface takes its own
                // reference to it, and the window keeps the `Handover` as long as it keeps the surface.
                unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(handover.visual())) }
                    .map_err(|e| e.to_string())
            }
            None => create_hwnd_surface(instance, hwnd),
        },
        |gpu, surface, prewarmed| {
            // Belt-and-suspenders: if an automatic idle handover on a non-D3D12 virtual adapter
            // fell through to a software GPU (WARP), stop before configuring any swap chain or
            // compiling shaders.
            if should_defer_prepared_adapter(defer_software, gpu.adapter.get_info().device_type) {
                return Ok(SetupOutcome::SoftwareDeferred);
            }
            let size = drawable_size(None, size, max_surface_dim(Some(gpu)));
            let (config, _) = surface_config(surface, &gpu.adapter, size, srgb)
                .ok_or_else(|| SurfaceSetupError::unsupported("the GPU can't present to this window"))?;
            configure_checked(surface, gpu, &config).map_err(|error| SurfaceSetupError::from_configure("", error))?;
            let target_format = if srgb { config.format.remove_srgb_suffix() } else { config.format };
            let mut renderer = match prewarmed {
                Some(mut renderer) => {
                    renderer.try_set_format(&gpu.device, target_format).map_err(SurfaceSetupError::from_shader)?;
                    renderer
                }
                None => Renderer::try_new_classified(&gpu.device, target_format).map_err(SurfaceSetupError::from_shader)?,
            };
            renderer.attach_gpu(gpu);
            Ok(SetupOutcome::Ready(Box::new((config, renderer))))
        },
    ).map_err(format_window_setup_error);
    let (gpu, surface, outcome) = match setup {
        Ok(res) => res,
        Err(error) => return PrepareOutcome::Failed(error),
    };
    let (config, mut renderer) = match outcome {
        SetupOutcome::Ready(ready) => *ready,
        SetupOutcome::SoftwareDeferred => {
            drop((surface, gpu, handover));
            return PrepareOutcome::SoftwareDeferred;
        }
    };
    let target_format = if srgb { config.format.remove_srgb_suffix() } else { config.format };
    let instances = std::mem::take(&mut *latest.lock().unwrap_or_else(|e| e.into_inner()));
    warm_frame(&gpu, &mut renderer, target_format, srgb, (config.width, config.height), &instances);
    crate::marks::mark("gpu.window.warmed");
    PrepareOutcome::Ready(Box::new(Prepared { gpu, renderer, surface, config, handover }))
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

/// `crate::warm_gpu_cache` on Windows: a window that is never shown, the GPU thread's own
/// `prepare` on it with no frame to draw, and the pinned pipelines waited for.
#[allow(unsafe_code)]
pub(crate) fn warm_gpu_cache() -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_POPUP};
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    // SAFETY: a window of the system's own class with no parent, menu or data; never shown.
    let hwnd = unsafe { CreateWindowExW(0, class.as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, 1280, 800, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) };
    if hwnd.is_null() { return Err("no window could be made to warm the GPU on".into()); }
    // What a window's hand-over makes: the device, the frame and fill pipelines for its swap
    // chain's format, then (started by the warm frame's prepare) the pinned ones.
    let made = prepare(hwnd as isize, (1280, 800), true, wgpu::Features::empty(), &Mutex::new(Vec::new()), false);
    let result = match made {
        PrepareOutcome::Ready(mut prepared) => { prepared.renderer.wait_pinned(); Ok(()) }
        PrepareOutcome::SoftwareDeferred => Ok(()),
        PrepareOutcome::Failed(error) => Err(error),
    };
    // SAFETY: the window made above, on this thread; the surface on it is gone with `prepared`.
    unsafe { DestroyWindow(hwnd) };
    result
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
