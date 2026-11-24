//! Linux: the GPU made in the background for a window that opened on the CPU (`cpu_frame`), and
//! the hand-over of the window to it.
//!
//! After the first CPU frame, on the loop's first idle turn, a thread makes what a window's GPU
//! frames start from (`render::warm`: the Vulkan instance, which loads every installed driver,
//! the device, Rune's renderer), so none of that runs before the first frame or on the UI thread.
//! When it's there the UI thread makes the window's Vulkan surface and swap chain on the same
//! `wl_surface` or X11 window (`Host::reattach`, as after a lost device) and draws the next frame
//! with the GPU. The display shows the CPU's last frame until that one presents: on Wayland a
//! surface keeps its buffer until the next is committed, on X11 a window keeps its pixels. From
//! the moment the swap chain exists the CPU sends nothing more (the driver's present owns the
//! surface's state), so no frame of the wrong kind, size or age can land between them. A GPU that
//! can't be made leaves the window on the CPU, fully live.

use std::sync::Arc;
use std::time::Duration;

use super::cpu_frame::Cpu;
use crate::render::Gpu;
use crate::time::Instant;
use crate::window::WakeEvent;
use crate::window::host::Host;
use crate::window::surface::Win;

/// Where a window's frames go. Windows that need the GPU from their first frame (a translucent
/// window, a second window) are `Gpu` from the start.
#[derive(Default)]
pub(in crate::window) enum SurfaceMode {
    #[default]
    Gpu,
    /// The CPU renderer draws; `job` is the GPU being made for it.
    Cpu { cpu: Cpu, job: GpuJob },
    /// The GPU has the window's surface and draws the next frame; the display shows the CPU's
    /// last one until it presents (`handed`).
    Handing { cpu: Cpu },
}

/// The GPU a CPU window is waiting for. Every way it ends is logged: handed over
/// (`gpu.window.handed`), or failed with a reason (`gpu.window.failed`, note `gpu.window.why`)
/// and tried again after `RETRY_AFTER`, `TRIES` times in all, then the CPU for good.
pub(in crate::window) enum GpuJob {
    /// Not started: the first frame isn't out yet, or a failed try (`tries` so far) waits for
    /// `retry_at`.
    Waiting { tries: u8, retry_at: Option<Instant> },
    /// Being made on a thread, which wakes the loop when it's done (`WakeEvent::GpuReady`).
    Building { tries: u8 },
    /// It failed `TRIES` times: the window stays on the CPU.
    Failed,
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
}

impl Host {
    /// The first idle turn after the window's first CPU frame: the GPU starts on its thread
    /// (`RUNE_CPU_FIRST=only`: not until a frame needs it, for checking the CPU window alone).
    pub(in crate::window) fn cpu_idle(&mut self) {
        if self.gpu_owed() { self.start_gpu(); }
    }

    /// Whether the GPU is due to start now (`cpu_idle`).
    pub(in crate::window) fn gpu_owed(&self) -> bool {
        let Some(w) = self.win.as_ref() else { return false };
        let SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at, .. }, .. } = &w.native.mode else { return false };
        w.first_frame.ready() && retry_at.is_none_or(|at| at <= crate::time::now())
            && std::env::var_os("RUNE_CPU_FIRST").is_none_or(|v| v != "only")
    }

    /// Starts making the GPU for this window on a thread.
    pub(super) fn start_gpu(&mut self) {
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { job, .. } = &mut w.native.mode else { return };
        let GpuJob::Waiting { tries, retry_at } = *job else { return };
        if retry_at.is_some() { crate::marks::mark("gpu.window.retry"); }
        let handle = self.native_handle.clone();
        match crate::render::warm_gpu_then(self.app.compositor_features(), move || handle.send(WakeEvent::GpuReady)) {
            Ok(()) => {
                crate::marks::mark("gpu.window.start");
                *job = GpuJob::Building { tries: tries + 1 };
            }
            Err(error) => self.fail_gpu(format!("its thread couldn't start: {error}"), tries + 1),
        }
    }

    /// The GPU thread finished: the window's surface and swap chain are made with what it made
    /// (or on this thread, if it made nothing), and the next frame is the GPU's; or the window
    /// stays on the CPU if that failed.
    pub(in crate::window) fn gpu_ready(&mut self) {
        let Some(w) = self.win.as_mut() else { return };
        let SurfaceMode::Cpu { cpu, job: GpuJob::Building { tries } } = &mut w.native.mode else { return };
        let tries = *tries;
        crate::marks::mark("gpu.window.ready");
        // X11: the server has every CPU frame before the driver's connection presents one.
        cpu.frames.settle();
        if let Err(error) = self.reattach(crate::gpu::fallback::GpuMode::Hardware) { return self.fail_gpu(error.message, tries); }
        let on_software = self.gpu.as_ref().is_some_and(|(gpu, _)| gpu.adapter.get_info().device_type == wgpu::DeviceType::Cpu);
        let Some(w) = self.win.as_mut() else { return };
        if on_software { w.surface_retries.note_software(); }
        let SurfaceMode::Cpu { cpu, .. } = std::mem::take(&mut w.native.mode) else { unreachable!("checked above") };
        w.native.mode = SurfaceMode::Handing { cpu };
        crate::marks::mark("gpu.window.attached");
        w.request_redraw();
    }

    /// The GPU couldn't take over (`why`), on its `tries`th try: the window goes on with the CPU
    /// renderer, and tries again after `RETRY_AFTER`, or stays there after `TRIES` tries.
    fn fail_gpu(&mut self, why: String, tries: u8) {
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
        w.request_redraw();
    }

    /// At the start of a turn (`Win::handover_deadline` wakes the loop for it): a failed try is
    /// made again when its time comes.
    pub(in crate::window) fn watch_handover(&mut self, now: Instant) {
        let Some(w) = self.win.as_ref() else { return };
        if let SurfaceMode::Cpu { job: GpuJob::Waiting { retry_at: Some(at), .. }, .. } = &w.native.mode && now >= *at && w.first_frame.ready() {
            self.start_gpu();
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
    let SurfaceMode::Handing { cpu } = &l.native.mode else { return None };
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
