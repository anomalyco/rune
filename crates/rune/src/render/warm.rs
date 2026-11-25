//! `warm_gpu` away from macOS: a GPU device and Rune's renderer made on a thread ahead of a
//! surface that may show soon, and the slot the next surface takes them from (`Gpu::for_surface`).
//! Unclaimed, they go after `LINGER`, so an app that warms on a hint and then shows nothing
//! holds no device for long. A hidden overlay that gives its surface back parks its device and
//! renderer here too (`park`), for `PARKED` rather than `LINGER`: an overlay shown for each
//! dictation takes the same device every time instead of making one per show (hex#1268).
//!
//! Lifecycle and worker ownership live in [`super::warm_machine::WarmSlot`]: launch stays
//! non-blocking, while exit calls [`shutdown`] to cancel in-flight work and wait with a bound
//! for every warm-up worker (`rune-gpu-warm`, `rune-gpu-window`, `rune-gpu-parked`) to exit.

use super::{Gpu, Renderer, warm_machine::{ShutdownOutcome, SystemClock, WarmSlot}, warmup::WarmRenderer};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

/// How long a warmed device waits for a surface, from the last `warm_gpu`.
pub(super) const LINGER: Duration = Duration::from_secs(30);

/// How long a parked device waits for the next surface.
pub(crate) const PARKED: Duration = Duration::from_secs(5 * 60);

/// The format overlays and windows draw in on Windows and Linux (`surface_config`, after
/// `srgb_blend` drops the sRGB suffix); a surface in another one rebuilds the pipeline.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

struct Warmed(Gpu, WarmRenderer);

static SLOT: WarmSlot<Warmed, SystemClock> = WarmSlot::new(SystemClock);
/// The device a window's or overlay's surface draws with, while one does.
static LIVE: Mutex<Weak<Gpu>> = Mutex::new(Weak::new());

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Some surface draws with `gpu`: a warm-up while it lives would make a second device.
pub(crate) fn surface_gpu(gpu: &Arc<Gpu>) {
    *lock(&LIVE) = Arc::downgrade(gpu);
}

/// The device a surface draws with now, if any. A lost one is no device to share, so it reads as
/// none and `warm` makes another.
pub(super) fn live() -> Option<Arc<Gpu>> {
    lock(&LIVE).upgrade().filter(|gpu| !gpu.is_lost())
}

/// Starts making a device with `features` (whichever the adapter has) and the renderer,
/// unless a surface's device lives or one is being made or waits; either way the warmed one
/// waits `LINGER` from now. `prepare` runs on the warming thread with the new device, or the
/// live one, so an app can make its own pipelines off the UI thread.
pub(super) fn warm(features: wgpu::Features, prepare: Option<Box<dyn FnOnce(&wgpu::Device) + Send>>) {
    if let Some(gpu) = live() {
        if let Some(prepare) = prepare {
            WarmSlot::spawn_attached(&SLOT, "rune-gpu-warm", move || prepare(&gpu.device)).ok();
        }
        return;
    }
    let prepare = prepare.map(|prepare| -> Box<dyn FnOnce(&Warmed) + Send> {
        Box::new(move |warmed: &Warmed| prepare(&warmed.0.device))
    });
    WarmSlot::start_warm(&SLOT, LINGER, move || make(features), prepare);
}

/// `warm`, then `frame`, then `done`, on a thread of its own, once a device waits in the slot
/// or none could be made: a window that opened on the CPU takes it from there
/// (`window::linux::handover`), and `Gpu::for_surface` makes one itself when the slot is empty.
///
/// `frame` draws with the device and renderer that wait, on this thread: what a window's first
/// GPU frame would make on the app's thread (the pipelines the frame's layers need, their
/// targets) is made here, while the CPU goes on drawing the window. For as long as it draws,
/// the slot reads as still being made.
#[cfg(target_os = "linux")]
pub(super) fn warm_then(
    features: wgpu::Features,
    frame: impl FnOnce(&Gpu, &mut Renderer) + Send + 'static,
    done: impl FnOnce() + Send + 'static,
) -> std::io::Result<()> {
    WarmSlot::warm_then(
        &SLOT,
        LINGER,
        move || make(features),
        move |Warmed(gpu, renderer)| {
            let mut renderer = Renderer::from_warm(renderer);
            frame(&gpu, &mut renderer);
            let warm = renderer.into_warm()?;
            (!gpu.is_lost()).then_some(Warmed(gpu, warm))
        },
        done,
    )
}

/// Whether a surface made now finds a device: a warmed one waits in the slot, or another
/// surface's lives. Without either, every adapter refused a device (`make`), and a surface made
/// now would only ask them again.
#[cfg(target_os = "linux")]
pub(super) fn made() -> bool {
    live().is_some() || SLOT.has_ready()
}

/// A hidden overlay's device and renderer, its surface already dropped, for the next surface to
/// take within `linger`. The device first finishes its work, so what the dropped surface and
/// its swapchain left is freed now rather than whenever the device next runs. When something
/// already waits in the slot, this one goes; so does a device that died while it was hidden.
pub(crate) fn park(gpu: Gpu, mut renderer: Renderer, linger: Duration) {
    if gpu.is_lost() || SLOT.is_stopped() { return; }
    renderer.clear_for_park();
    gpu.queue.submit(std::iter::empty());
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok();
    let Some(renderer) = renderer.into_warm() else { return };
    SLOT.park(&SLOT, Warmed(gpu, renderer), linger);
}

/// A device for a future surface on a GPU that drives a display, with the renderer, and one
/// pass run offscreen, so the device has drawn (`GpuEvent::Drew`).
fn make(features: wgpu::Features) -> Option<Warmed> {
    if SLOT.is_stopped() { return None; }
    super::gpu_events::starting();
    let instance = Gpu::instance_for(features);
    if SLOT.is_stopped() { return None; }
    let gpu = Gpu::try_for_display(instance, features).ok()?;
    if SLOT.is_stopped() { return None; }
    let renderer = Renderer::try_new_warm(&gpu.device, FORMAT).ok()?;
    if SLOT.is_stopped() { return None; }
    let target = super::Offscreen::with_format(&gpu.device, 8, 8, FORMAT);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("rune warm-up"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &target.view, depth_slice: None, resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    }));
    gpu.queue.submit([encoder.finish()]);
    if SLOT.is_stopped() { return None; }
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok();
    if !SLOT.is_stopped() {
        super::gpu_events::presented();
    }
    Some(Warmed(gpu, renderer))
}

/// What `warm` made, waiting for it if it's still making it. A device that died while it waited
/// is dropped: its surface's owner makes a new one.
pub(super) fn take() -> Option<(Gpu, Renderer)> {
    let Warmed(gpu, renderer) = SLOT.take()?;
    if gpu.is_lost() { return None; }
    Some((gpu, Renderer::from_warm(renderer)))
}

/// `prewarm_gpu` is macOS's; elsewhere the surface decides the adapter.
pub(super) fn start() {}

/// Non-blocking discard: what's parked goes, and a thread still making one drops it when done.
#[cfg_attr(test, allow(dead_code))]
pub(super) fn discard() {
    SLOT.discard();
}

/// Non-blocking early cancellation called as soon as `Windows::exiting` starts so in-flight
/// warm-up stops scheduling follow-up work while windows tear down.
pub(super) fn cancel() {
    SLOT.cancel();
}

/// Exit-time shutdown: cancels any in-flight warm-up and waits up to `timeout` for every
/// tracked worker thread to finish.
pub(super) fn shutdown(timeout: Duration) -> ShutdownOutcome {
    SLOT.shutdown(timeout)
}

/// Re-opens a cleanly drained slot when a new event loop starts in the same process.
pub(super) fn reopen() {
    SLOT.reopen();
}
