//! Linux: the GPU made in the background for a window that opened on the CPU (`cpu_frame`), and
//! the hand-over of the window to it.
//!
//! After the first CPU frame, on the loop's first idle turn, a thread makes what a window's GPU
//! frames start from (`render::warm`: the Vulkan instance, which loads every installed driver,
//! the device, Rune's renderer), so none of that runs before the first frame or on the UI thread.
//! When it's there the UI thread makes the window's Vulkan surface and swap chain on the same
//! `wl_surface` or X11 window (`Host::reattach`, as after a lost device) and draws the next frame
//! with the GPU. The display shows the CPU's last frame until that one presents: on Wayland a
//! surface keeps its buffer until the next is committed, on X11 a window keeps its pixels.
//!
//! One side at a time may send the window's pixels to the display, and the window changes sides
//! once, at one point, on the UI thread: `SurfaceMode::switch` in `Host::gpu_ready`, before the
//! Vulkan surface is made. The switch retires the CPU's frames (`platform::cpu_frames::Retired`
//! can send nothing), so no resize, scale change, redraw, timer or input that comes after it can
//! put a CPU frame beside the driver's swap chain. On a Wayland compositor with explicit
//! synchronization that would be a protocol error, which ends the app (`platform::cpu_frames`).
//!
//! A GPU that gives no device leaves the window on the CPU, fully live, its surface untouched.
//! So does a software one (lavapipe, llvmpipe: `wgpu::DeviceType::Cpu`) under `CpuFirst::Auto`
//! with no adapter named (`WGPU_ADAPTER_NAME`): `CpuPainter` draws the same frames for a fraction
//! of its CPU time, so the window stays on the CPU (`GpuJob::SoftwareDeferred`) until a frame
//! needs what only the GPU draws, and that frame takes the software GPU.
//! A device whose surface can't be made (after the switch) gives the window back to the CPU only
//! where a surface that came and went is known to leave nothing behind (`Retired::revive`); with
//! explicit synchronization the window stays the GPU's and its surface is tried again as a lost
//! one is (`Host::replace_lost_surface`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::cpu_frame::{Cpu, RetiredCpu};
use crate::gpu::fallback::Policy;
use crate::machine::Effects;
use crate::render::{Gpu, Instance, Renderer};
use crate::time::Instant;
use crate::window::CpuFirst;
use crate::window::WakeEvent;
use crate::window::host::Host;
use crate::window::surface::Win;
use crate::window::warm_frame::{warm_frame, warm_instances};

/// Where a window's frames go. Windows that need the GPU from their first frame (a translucent
/// window, a second window) are `Gpu` from the start; a visible translucent window whose
/// pre-swapchain GPU startup fails under `CpuFirst::Auto`/`Only` (automatic adapter selection)
/// falls back to `SurfaceMode::Cpu { job: GpuJob::Failed, .. }`.
#[derive(Default)]
pub(in crate::window) enum SurfaceMode {
    #[default]
    Gpu,
    /// The CPU renderer draws; `job` is the GPU being made for it.
    Cpu { cpu: Cpu, job: GpuJob },
    /// The window is the GPU's (`switch`): it has the window's surface, or is making it, and draws
    /// the next frame; the display shows the CPU's last one until that presents (`handed`).
    /// `tries`: how many times a GPU was made for this window.
    Handing { cpu: RetiredCpu, tries: u8 },
}

/// The GPU a CPU window is waiting for. Every way it ends is logged: handed over
/// (`gpu.window.handed`), or failed with a reason (`gpu.window.failed`, note `gpu.window.why`)
/// and tried again after `RETRY_AFTER`, `TRIES` times in all, then the CPU for good.
pub(in crate::window) enum GpuJob {
    /// Not started: the first frame isn't out yet, or a failed try (`tries` so far) waits for
    /// `retry_at`.
    Waiting { tries: u8, retry_at: Option<Instant> },
    /// Being made on a thread, which wakes the loop when it's done (`WakeEvent::GpuReady`).
    /// `latest`: what the window draws now, for the frame that thread draws once before it
    /// reports (`warm_frame`); each CPU frame leaves its own here. `pick`: what the thread found
    /// and whether a software GPU is to be left alone (`Pick`).
    Building { tries: u8, latest: Arc<Mutex<Latest>>, pick: Arc<Pick> },
    /// The GPU made after the first frame was a software one, and the window stays on the CPU
    /// (`defers_software`). Not tried again on idle; a frame that needs the GPU takes it
    /// (`Host::start_gpu_for_needs_gpu`).
    SoftwareDeferred,
    /// It failed `TRIES` times: the window stays on the CPU.
    Failed,
}

/// A CPU frame as the GPU thread's warm frame draws it: its instances (`warm_instances`) and
/// the window's size in device pixels.
#[derive(Default)]
pub(in crate::window) struct Latest {
    pub(super) instances: Vec<Instance>,
    pub(super) size: (u32, u32),
}

/// Explicit state machine shared between the background GPU thread and the UI thread for one
/// `GpuJob::Building` attempt, keeping the warm-frame decision and `gpu_ready`'s handover
/// decision in lockstep so `gpu_ready` never switches to an unwarmed software GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickState {
    /// The worker has not yet inspected the adapter (`defer_software`: true for an automatic idle
    /// hand-over, false once a frame needs the GPU or software GPUs are allowed).
    Pending { defer_software: bool },
    /// The worker found a software adapter while `defer_software` was true and skipped
    /// `warm_frame`. `needs_gpu_arrived` records whether a `NeedsGpu` frame arrived after the
    /// worker skipped warming and before `gpu_ready` consumed the result.
    Deferred { needs_gpu_arrived: bool },
    /// The worker accepted the adapter and is running (or finished) `warm_frame` off-thread.
    Warmed,
}

/// Events fed into [`step_pick`] by the GPU thread and the UI thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickEvent {
    /// A frame hit `NeedsGpu` on the UI thread while `GpuJob::Building` is in flight.
    NeedsGpu,
    /// The GPU thread obtained a device and asks whether to run `warm_frame`.
    AdapterFound { software: bool },
    /// The UI thread's `gpu_ready` consumes the finished worker's outcome.
    Ready,
}

/// Effects returned by [`step_pick`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickEffect {
    /// GPU thread: run `warm_frame` off-thread before signalling `GpuReady`.
    RunWarmFrame,
    /// GPU thread: skip `warm_frame` (software GPU deferred on idle).
    SkipWarmFrame,
    /// UI thread (`gpu_ready`): stay on CPU in `GpuJob::SoftwareDeferred`.
    DeferSoftware,
    /// UI thread (`gpu_ready`): switch to the warmed GPU (`SurfaceMode::switch` + `reattach`).
    SwitchToGpu,
    /// UI thread (`gpu_ready`): `NeedsGpu` arrived after the worker skipped `warm_frame`;
    /// restart `start_gpu(false)` so the worker runs `warm_frame` off-thread before switching.
    RestartForWarmFrame,
}

/// Pure transition for [`PickState`]: the warm-frame decision and `gpu_ready`'s switch decision
/// share one state, so `SwitchToGpu` is never emitted after `SkipWarmFrame`.
pub(super) fn step_pick(state: &mut PickState, event: PickEvent) -> Effects<PickEffect> {
    match (*state, event) {
        (PickState::Pending { .. }, PickEvent::NeedsGpu) => {
            *state = PickState::Pending { defer_software: false };
            Effects::none()
        }
        (PickState::Deferred { .. }, PickEvent::NeedsGpu) => {
            *state = PickState::Deferred { needs_gpu_arrived: true };
            Effects::none()
        }
        (PickState::Warmed, PickEvent::NeedsGpu) => Effects::none(),
        (PickState::Pending { defer_software: true }, PickEvent::AdapterFound { software: true }) => {
            *state = PickState::Deferred { needs_gpu_arrived: false };
            Effects::one(PickEffect::SkipWarmFrame)
        }
        (PickState::Pending { .. }, PickEvent::AdapterFound { .. }) => {
            *state = PickState::Warmed;
            Effects::one(PickEffect::RunWarmFrame)
        }
        (PickState::Deferred { .. }, PickEvent::AdapterFound { .. }) => {
            Effects::one(PickEffect::SkipWarmFrame)
        }
        (PickState::Warmed, PickEvent::AdapterFound { .. }) => {
            Effects::one(PickEffect::RunWarmFrame)
        }
        (PickState::Deferred { needs_gpu_arrived: false }, PickEvent::Ready) => {
            Effects::one(PickEffect::DeferSoftware)
        }
        (PickState::Deferred { needs_gpu_arrived: true }, PickEvent::Ready) => {
            Effects::one(PickEffect::RestartForWarmFrame)
        }
        (PickState::Warmed | PickState::Pending { .. }, PickEvent::Ready) => {
            Effects::one(PickEffect::SwitchToGpu)
        }
    }
}

/// Synchronized [`PickState`] shared between the GPU worker thread and the UI thread.
pub(in crate::window) struct Pick {
    state: Mutex<PickState>,
}

impl Default for Pick {
    fn default() -> Self { Self::new(false) }
}

impl Pick {
    pub(super) fn new(defer_software: bool) -> Self {
        Pick { state: Mutex::new(PickState::Pending { defer_software }) }
    }

    pub(super) fn step(&self, event: PickEvent) -> Effects<PickEffect> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        step_pick(&mut state, event)
    }
}

/// Whether an idle hand-over leaves a software GPU (`wgpu::DeviceType::Cpu`) alone: under
/// `CpuFirst::Auto` with the adapter Rune's own choice. A named adapter (`Policy::Pinned`) is
/// what was asked for, and `Only` starts no idle hand-over at all.
pub(super) fn defers_software(mode: CpuFirst, policy: Policy) -> bool {
    mode == CpuFirst::Auto && policy == Policy::Automatic
}

/// A failed try is made again after this long.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Tries in all before the window stays on the CPU.
const TRIES: u8 = 3;

/// The GPU's first frame on its way back, with the CPU frame the window showed until then, to
/// count how many pixels differ (`RUNE_CPU_BOOT_DRIFT=1`).
pub(in crate::window) struct Readback {
    buffer: wgpu::Buffer,
    row: u32,
    size: (u32, u32),
    cpu: Vec<u8>,
}

impl SurfaceMode {
    /// Whether the CPU renderer draws this window's frames now.
    pub(in crate::window) fn cpu_draws(&self) -> bool { matches!(self, SurfaceMode::Cpu { .. }) }

    /// The CPU's side of the window, while the window is the CPU's: the only way to its frames.
    pub(super) fn cpu(&mut self) -> Option<&mut Cpu> {
        match self {
            SurfaceMode::Cpu { cpu, .. } => Some(cpu),
            SurfaceMode::Gpu | SurfaceMode::Handing { .. } => None,
        }
    }

    /// The one point where a window goes from the CPU to the GPU: its CPU frames are retired, and
    /// from here nothing of the CPU's reaches the display. Only a window whose GPU was being made
    /// switches; its try's number comes back.
    pub(super) fn switch(&mut self) -> Option<u8> {
        let SurfaceMode::Cpu { job: GpuJob::Building { tries, .. }, .. } = self else { return None };
        let tries = *tries;
        let SurfaceMode::Cpu { cpu, .. } = std::mem::take(self) else { unreachable!("checked above") };
        *self = SurfaceMode::Handing { cpu: cpu.retire(), tries };
        Some(tries)
    }

    /// A window that switched and whose GPU surface then couldn't be made, and is gone: back to
    /// the CPU with `job`, where the display allows that (`RetiredCpu::revive`). False: it stays
    /// the GPU's.
    pub(super) fn switch_back(&mut self, job: GpuJob) -> bool {
        let SurfaceMode::Handing { .. } = self else { return false };
        let SurfaceMode::Handing { cpu, tries } = std::mem::take(self) else { unreachable!("checked above") };
        match cpu.revive() {
            Ok(cpu) => {
                *self = SurfaceMode::Cpu { cpu, job };
                true
            }
            Err(cpu) => {
                *self = SurfaceMode::Handing { cpu, tries };
                false
            }
        }
    }
}

impl Host {
    /// The first idle turn after the window's first CPU frame: the GPU starts on its thread
    /// (`CpuFirst::Only` or `RUNE_CPU_FIRST=only`: not until a frame needs it).
    pub(in crate::window) fn cpu_idle(&mut self) {
        if self.poll_gpu_loss() { return; }
        if self.gpu_owed() { self.start_gpu(self.idle_defers_software()); }
    }

    /// Whether a hand-over started on idle (or retried) leaves a software GPU alone.
    fn idle_defers_software(&self) -> bool {
        defers_software(self.effective_cpu_first(), crate::gpu::pick::env_policy())
    }

    /// A frame needs what only the GPU draws: the GPU starts now if it hasn't, software or not.
    /// A window left on the CPU by a software GPU starts again; a thread at work keeps the GPU it
    /// finds.
    pub(super) fn start_gpu_for_needs_gpu(&mut self) {
        if self.poll_gpu_loss() { return; }
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { job, .. } = &mut w.native.mode else { return };
        match job {
            GpuJob::Building { pick, .. } => {
                for _ in pick.step(PickEvent::NeedsGpu) {}
                return;
            }
            GpuJob::SoftwareDeferred => *job = GpuJob::Waiting { tries: 0, retry_at: None },
            GpuJob::Waiting { .. } | GpuJob::Failed => {}
        }
        self.start_gpu(false);
    }

    /// Whether the GPU is due to start now (`cpu_idle`).
    pub(in crate::window) fn gpu_owed(&self) -> bool {
        let Some(w) = self.win.as_ref() else { return false };
        let SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at, .. }, .. } = &w.native.mode else { return false };
        let due = w.first_frame.ready() && retry_at.is_none_or(|at| at <= crate::time::now());
        self.effective_cpu_first().gpu_owed(due)
    }

    /// Starts making the GPU for this window on a thread. `defer`: a software GPU leaves the
    /// window on the CPU (`defers_software`).
    pub(super) fn start_gpu(&mut self, defer: bool) {
        if self.poll_gpu_loss() { return; }
        let Some(w) = self.win.as_ref() else { return };
        let SurfaceMode::Cpu { job: GpuJob::Waiting { tries, retry_at }, .. } = &w.native.mode else { return };
        let (tries, retried) = (*tries, retry_at.is_some());
        if retried { crate::marks::mark("gpu.window.retry"); }
        let handle = self.native_handle.clone();
        // The frame the GPU thread draws once, offscreen, before it reports: what the window
        // shows by then. In an app whose view has layers the GPU's first frame makes their
        // pipelines, and made on this thread at the hand-over they held the window still for as
        // long as the driver took (a quarter of a second on a software GPU, far more through a
        // translation layer).
        let srgb = self.app.rt().srgb_blend();
        let latest = Arc::new(Mutex::new(Latest { instances: warm_instances(&self.app.rt().instances), size: (w.config.width, w.config.height) }));
        let drawn = latest.clone();
        let pick = Arc::new(Pick::new(defer));
        let found = pick.clone();
        let frame = move |gpu: &Gpu, renderer: &mut Renderer| {
            let software = gpu.adapter.get_info().device_type == wgpu::DeviceType::Cpu;
            let run_warm = found
                .step(PickEvent::AdapterFound { software })
                .into_iter()
                .any(|e| e == PickEffect::RunWarmFrame);
            // A GPU the window won't take draws nothing for it.
            if !run_warm { return; }
            let Latest { instances, size } = std::mem::take(&mut *drawn.lock().unwrap_or_else(|e| e.into_inner()));
            // The format a window's swap chain has here (`surface_config`); in another one the
            // hand-over frame makes its pipelines itself, as it did.
            let format = if srgb { wgpu::TextureFormat::Bgra8Unorm } else { wgpu::TextureFormat::Bgra8UnormSrgb };
            warm_frame(gpu, renderer, format, srgb, size, &instances);
            crate::marks::mark("gpu.window.warmed");
        };
        let started = crate::render::warm_gpu_then(self.app.compositor_features(), frame, move || handle.send(WakeEvent::GpuReady));
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { job, .. } = &mut w.native.mode else { return };
        match started {
            Ok(()) => {
                crate::marks::mark("gpu.window.start");
                *job = GpuJob::Building { tries: tries + 1, latest, pick };
            }
            Err(error) => self.fail_gpu(format!("its thread couldn't start: {error}"), tries + 1),
        }
    }

    /// The GPU thread finished. With a device, the window goes to the GPU (`SurfaceMode::switch`),
    /// its surface and swap chain are made with that device, and the next frame is the GPU's.
    /// With none, the window's surface is left alone and the CPU goes on drawing.
    pub(in crate::window) fn gpu_ready(&mut self) {
        if self.poll_gpu_loss() { return; }
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { job, .. } = &mut w.native.mode else { return };
        let GpuJob::Building { tries, pick, .. } = job else { return };
        let (tries, pick) = (*tries, pick.clone());
        crate::marks::mark("gpu.window.ready");
        // Every adapter refused a device: a surface made now would only ask them again, on this
        // thread, and would put a driver on a window the CPU keeps drawing.
        if !crate::render::warm_gpu_made() { return self.fail_gpu("no GPU gave a device".into(), tries); }
        for effect in pick.step(PickEvent::Ready) {
            match effect {
                PickEffect::DeferSoftware => {
                    // The only GPU is a software one: the CPU renderer goes on drawing the window,
                    // and the device waits in the warm slot until it lapses or a frame needs it.
                    if let SurfaceMode::Cpu { job, .. } = &mut w.native.mode {
                        *job = GpuJob::SoftwareDeferred;
                    }
                    crate::marks::mark("gpu.window.software_deferred");
                    if std::env::var_os("RUNE_GPU_LOG").is_some_and(|v| v != "0") { eprintln!("rune: the GPU is a software one: the window stays on the CPU renderer until a frame needs the GPU"); }
                    return;
                }
                PickEffect::RestartForWarmFrame => {
                    // NeedsGpu arrived after the worker skipped warm_frame and before gpu_ready
                    // ran: restart with software allowed so warm_frame compiles layer/effect
                    // shaders off-thread (reusing the warmed device) before switching.
                    if let SurfaceMode::Cpu { job, .. } = &mut w.native.mode {
                        *job = GpuJob::Waiting { tries: 0, retry_at: None };
                    }
                    self.start_gpu(false);
                    return;
                }
                PickEffect::SwitchToGpu | PickEffect::RunWarmFrame | PickEffect::SkipWarmFrame => {}
            }
        }
        // The switch comes before the Vulkan surface, not after it: a driver may put its own
        // objects on the window as soon as its surface or swap chain exists.
        if w.native.mode.switch().is_none() { return; }
        crate::marks::mark("gpu.window.switched");
        // In the log where a protocol trace has it in order with the display's requests.
        if std::env::var_os("RUNE_GPU_LOG").is_some_and(|v| v != "0") { eprintln!("rune: the window is the GPU's from here: the CPU sends the display nothing more"); }
        if let Err(error) = self.reattach(crate::gpu::fallback::GpuMode::Hardware) { return self.fail_surface(error.message, tries); }
        let on_software = self.gpu.as_ref().is_some_and(|(gpu, _)| gpu.adapter.get_info().device_type == wgpu::DeviceType::Cpu);
        if on_software && let Some(w) = self.win.as_mut() { w.surface_retries.note_software(); }
        self.app.rt_mut().set_renderer_kind(crate::RendererKind::Gpu);
        crate::marks::mark("gpu.window.attached");
        if let Some(w) = self.win.as_ref() { w.request_redraw(); }
    }

    /// The window switched to the GPU and its surface couldn't be made (`why`); what the try
    /// made of one is gone again (`reattach` drops it). Where the display allows, the CPU takes
    /// the window back and the GPU is tried again later (`fail_gpu`). With explicit
    /// synchronization it doesn't: the window stays the GPU's, shows the CPU's last frame, and
    /// its surface is made again as a lost one is, with that path's tries and its end
    /// (`replace_lost_surface`: the window is given up and the app told why).
    fn fail_surface(&mut self, why: String, tries: u8) {
        if self.is_terminal_gpu_policy() {
            self.latch_loss_latch(crate::render::loss_latch::GpuFailureKind::SurfaceFailed, why);
            return;
        }
        let Some(w) = self.win.as_mut() else { return };
        if w.native.mode.switch_back(GpuJob::Waiting { tries, retry_at: None }) { return self.fail_gpu(why, tries); }
        eprintln!("rune: the window's GPU surface couldn't be made; it is tried again (the compositor takes no CPU frame once a GPU surface was tried): {why}");
        crate::marks::mark("gpu.window.failed");
        if !crate::marks::noted("gpu.window.why") { crate::marks::note("gpu.window.why", Box::leak(why.into_boxed_str())); }
        w.surface = None;
        w.surface_ok = false;
        self.gpu = None;
        self.app.release_compositor_gpu();
        w.request_redraw();
    }

    /// The GPU couldn't take over (`why`), on its `tries`th try: the window goes on with the CPU
    /// renderer, and tries again after `RETRY_AFTER`, or stays there after `TRIES` tries.
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
        if let SurfaceMode::Cpu { job, .. } = &mut w.native.mode {
            *job = if tries < TRIES { GpuJob::Waiting { tries, retry_at: Some(crate::time::now() + RETRY_AFTER) } } else { GpuJob::Failed };
        }
        // A window whose frames only the GPU draws never showed one, and now never will: it's
        // given up, as one whose surface can't be made is.
        if tries >= TRIES && !w.first_frame.ready() {
            self.give_up_surface("the window needs a GPU to draw, and none could be made".into());
            return;
        }
        // Whatever the try made of a surface and device goes; the display kept the CPU's frame.
        w.surface = None;
        self.gpu = None;
        self.app.release_compositor_gpu();
        self.app.rt_mut().set_renderer_kind(crate::RendererKind::Cpu);
        w.request_redraw();
    }

    /// At the start of a turn (`Win::handover_deadline` wakes the loop for it): a failed try is
    /// made again when its time comes.
    pub(in crate::window) fn watch_handover(&mut self, now: Instant) {
        if self.poll_gpu_loss() { return; }
        let Some(w) = self.win.as_ref() else { return };
        if let SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at: Some(at), .. }, .. } = &w.native.mode && now >= *at && w.first_frame.ready() {
            let defer = self.idle_defers_software();
            self.start_gpu(defer);
        }
    }
}

impl Win {
    /// When the hand-over next has something to do (`Host::watch_handover`).
    pub(in crate::window) fn handover_deadline(&self, now: Instant) -> Option<Instant> {
        match &self.native.mode {
            SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at, .. }, .. } => retry_at.filter(|&at| at > now),
            SurfaceMode::Cpu { .. } | SurfaceMode::Gpu | SurfaceMode::Handing { .. } => None,
        }
    }
}

/// In the frame the GPU draws to take the window over, when the drift is asked for
/// (`RUNE_CPU_BOOT_DRIFT=1`): a copy of that frame is added to `encoder`, and comes back with
/// the CPU frame the window shows.
pub(in crate::window) fn readback(l: &Win, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, frame: &wgpu::SurfaceTexture) -> Option<Readback> {
    let SurfaceMode::Handing { cpu, .. } = &l.native.mode else { return None };
    let wanted = crate::render::cpu_boot::drift_wanted() && l.config.usage.contains(wgpu::TextureUsages::COPY_SRC)
        && l.config.format.remove_srgb_suffix() == wgpu::TextureFormat::Bgra8Unorm;
    if !wanted { return None; }
    let (cpu, size) = cpu.frames.last()?;
    if size != (l.config.width, l.config.height) {
        eprintln!("rune: cpu boot drift: none to count, the window changed size as the GPU took it over");
        return None;
    }
    let (width, height) = size;
    let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("rune hand-over readback"), size: u64::from(row) * u64::from(height), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    encoder.copy_texture_to_buffer(frame.texture.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) } },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
    Some(Readback { buffer, row, size, cpu })
}

/// The GPU's first frame for a window that opened on the CPU was presented: the window is the
/// GPU's, and the CPU's side goes (its buffers after the driver's, in the display's order).
pub(in crate::window) fn handed(l: &mut Win, gpu: &Arc<Gpu>, readback: Option<Readback>) {
    if !matches!(l.native.mode, SurfaceMode::Handing { .. }) { return; }
    l.native.mode = SurfaceMode::Gpu;
    crate::marks::mark("gpu.window.handed");
    if let Some(readback) = readback { count_drift(gpu.clone(), readback); }
}

/// On a worker: maps the GPU frame's copy and counts the pixels that differ from the CPU frame
/// the window showed before it (`cpu_boot::drift` logs one line and keeps `last_drift`). The two
/// are the same frame only while the view is at rest.
fn count_drift(gpu: Arc<Gpu>, readback: Readback) {
    let spawned = std::thread::Builder::new().name("rune-drift".into()).spawn(move || {
        let slice = readback.buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        if rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).is_err() { return; }
        let (width, height) = readback.size;
        let Ok(gpu_px) = slice.get_mapped_range() else { return };
        crate::render::cpu_boot::drift(crate::render::cpu_boot::Pixels::bgra(&readback.cpu, width, height, width as usize * 4), crate::render::cpu_boot::Pixels::bgra(&gpu_px, width, height, readback.row as usize));
    });
    if let Err(error) = spawned { eprintln!("rune: could not count the hand-over's drift: {error}"); }
}

#[cfg(test)]
#[path = "handover_tests.rs"]
mod tests;
