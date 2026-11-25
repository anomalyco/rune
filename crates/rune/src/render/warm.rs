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

use super::{
    Gpu, Renderer,
    loss_latch::{GpuFailureKind, GpuLossLatch},
    warm_machine::{Clock, ShutdownOutcome, SlotHandle, SystemClock, WarmSlot},
    warmup::WarmRenderer,
};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

/// How long a warmed device waits for a surface, from the last `warm_gpu`.
pub(super) const LINGER: Duration = Duration::from_secs(30);

/// How long a parked device waits for the next surface.
pub(crate) const PARKED: Duration = Duration::from_secs(5 * 60);

/// The format overlays and windows draw in on Windows and Linux (`surface_config`, after
/// `srgb_blend` drops the sRGB suffix); a surface in another one rebuilds the pipeline.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

/// Ready device and renderer waiting in a [`WarmSlot`], with optional owning
/// [`GpuLossLatch`] for [`crate::AppHandle::warm_gpu_with`].
pub(super) struct Warmed {
    pub(super) gpu: Option<Gpu>,
    pub(super) renderer: Option<WarmRenderer>,
    pub(super) owner: Option<Arc<GpuLossLatch>>,
    /// Set to `true` only when `take_from_slot` unpacks this payload. Any other drop (clean
    /// `LINGER` expiry, `Discard`, `Cancel`, `Shutdown`) detaches `owner` cleanly first so
    /// drop-triggered `DeviceLostReason::Destroyed` never falsely trips `owner`.
    claimed: bool,
}

impl Warmed {
    pub(super) fn new(gpu: Gpu, renderer: WarmRenderer, owner: Option<Arc<GpuLossLatch>>) -> Self {
        Self { gpu: Some(gpu), renderer: Some(renderer), owner, claimed: false }
    }
}

impl Drop for Warmed {
    fn drop(&mut self) {
        if !self.claimed
            && let (Some(gpu), Some(owner)) = (&self.gpu, &self.owner)
        {
            gpu.prepare_clean_drop_for(owner, true);
        }
    }
}

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
#[cfg(target_os = "linux")]
pub(super) fn live() -> Option<Arc<Gpu>> {
    lock(&LIVE).upgrade().filter(|gpu| !gpu.is_lost())
}

/// Starts making a device with `features` (whichever the adapter has) and the renderer,
/// unless a surface's device lives or one is being made or waits; either way the warmed one
/// waits `LINGER` from now. `prepare` runs on the warming thread with the new device, or the
/// live one, so an app can make its own pipelines off the UI thread.
pub(super) fn warm(features: wgpu::Features, prepare: Option<Box<dyn FnOnce(&wgpu::Device) + Send>>) {
    warm_with_owner(features, None, prepare);
}

/// App-attributed form of [`warm`] used by [`crate::AppHandle::warm_gpu_with`].
///
/// Under `owner = Some(latch)` (`GpuLossPolicy::Terminal`):
/// - refuses immediately once `latch` is failed without touching `SLOT`/`LIVE` or running `prepare`;
/// - on a live GPU, checks `sample_removal()`/`is_lost()`, attaches `latch` around `prepare` with
///   a post-`prepare` `PollType::Poll`, and detaches this worker's hold cleanly afterwards;
/// - on cold start, retains `owner` with the build and parked `Warmed` payload, attaching `owner`
///   as soon as `Gpu` is created (before renderer creation, `prepare`, and parking), latching
///   `InitFailed` on creation error, dropping without parking if `prepare` fails the device, and
///   extending `LINGER` without replacing the initiating owner/callback if a build is already
///   running or parked.
pub(super) fn warm_with_owner(
    features: wgpu::Features,
    owner: Option<Arc<GpuLossLatch>>,
    prepare: Option<Box<dyn FnOnce(&wgpu::Device) + Send>>,
) {
    if owner.as_ref().is_some_and(|l| l.is_failed()) {
        return;
    }
    if let Some(gpu) = lock(&LIVE).upgrade() {
        if let Some(latch) = owner {
            if gpu.sample_removal() || gpu.is_lost() {
                if !latch.is_failed() {
                    let _ = latch.record_device_lost(false, "the live GPU device was lost before warm-up");
                }
                return;
            }
            if let Some(prepare) = prepare {
                run_prepare_on_live(&SLOT, gpu, latch, prepare);
            }
            return;
        }
        if !gpu.is_lost() {
            if let Some(prepare) = prepare {
                WarmSlot::spawn_attached(&SLOT, "rune-gpu-warm", move || prepare(&gpu.device)).ok();
            }
            return;
        }
    }
    let owner_for_make = owner.clone();
    warm_on_slot(
        &SLOT,
        LINGER,
        owner,
        move |owner| make_owned(features, owner),
        prepare,
    );
    let _ = owner_for_make;
}

/// Runs `prepare` on a live GPU for a `Terminal` owner on a tracked attached worker, polling
/// `gpu.device` after `prepare` so synchronous validation/destroy errors trip `latch` immediately.
pub(super) fn run_prepare_on_live<H: SlotHandle<Warmed, C>, C: Clock + 'static>(
    handle: H,
    gpu: Arc<Gpu>,
    latch: Arc<GpuLossLatch>,
    prepare: Box<dyn FnOnce(&wgpu::Device) + Send>,
) {
    if latch.is_failed() || gpu.sample_removal() || gpu.is_lost() {
        if !latch.is_failed() {
            let _ = latch.record_device_lost(false, "the live GPU device was lost before warm-up");
        }
        return;
    }
    latch.rearm();
    gpu.attach_terminal_latch(Arc::clone(&latch));
    let gpu_worker = Arc::clone(&gpu);
    let latch_worker = Arc::clone(&latch);
    let spawned = WarmSlot::spawn_attached(handle, "rune-gpu-warm", move || {
        prepare_on_live_inline(&gpu_worker, &latch_worker, prepare);
    });
    if !matches!(spawned, Ok(true)) {
        gpu.detach_terminal_latch(&latch);
    }
}

/// Synchronous body of live-GPU `Terminal` `prepare` (also directly unit-testable without threads).
pub(super) fn prepare_on_live_inline(
    gpu: &Gpu,
    latch: &Arc<GpuLossLatch>,
    prepare: impl FnOnce(&wgpu::Device),
) {
    if latch.is_failed() || gpu.sample_removal() || gpu.is_lost() {
        if !latch.is_failed() {
            let _ = latch.record_device_lost(false, "the live GPU device was lost before warm-up");
        }
        gpu.detach_terminal_latch(latch);
        return;
    }
    prepare(&gpu.device);
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
    if gpu.sample_removal() || gpu.is_lost() {
        if !latch.is_failed() {
            let _ = latch.record_device_lost(false, "the GPU device was lost during warm-up preparation");
        }
    }
    gpu.detach_terminal_latch(latch);
}

/// Parameterized cold-start warm helper over any [`WarmSlot`] (used by `warm_with_owner` and by
/// isolated unit/noop tests with injected clocks and mock `make` functions).
pub(super) fn warm_on_slot<H: SlotHandle<Warmed, C>, C: Clock + 'static>(
    handle: H,
    linger: Duration,
    owner: Option<Arc<GpuLossLatch>>,
    make: impl FnOnce(Option<Arc<GpuLossLatch>>) -> Option<Warmed> + Send + 'static,
    prepare: Option<Box<dyn FnOnce(&wgpu::Device) + Send>>,
) {
    if owner.as_ref().is_some_and(|l| l.is_failed()) {
        return;
    }
    let owner_for_make = owner.clone();
    let check_prepare: Option<Box<dyn FnOnce(&Warmed) -> bool + Send>> =
        if prepare.is_some() || owner.is_some() {
            Some(Box::new(move |warmed: &Warmed| {
                if let Some(owner) = &warmed.owner {
                    if owner.is_failed() || warmed.gpu.as_ref().expect("warmed gpu").sample_removal() || warmed.gpu.as_ref().expect("warmed gpu").is_lost() {
                        if !owner.is_failed() {
                            let _ = owner.record_device_lost(
                                false,
                                "the warmed GPU device was lost before preparation",
                            );
                        }
                        return false;
                    }
                }
                if let Some(prepare) = prepare {
                    prepare(&warmed.gpu.as_ref().expect("warmed gpu").device);
                }
                if let Some(owner) = &warmed.owner {
                    rune_gpu::health::poll(&warmed.gpu.as_ref().expect("warmed gpu").device, wgpu::PollType::Poll).ok();
                    if warmed.gpu.as_ref().expect("warmed gpu").sample_removal() || warmed.gpu.as_ref().expect("warmed gpu").is_lost() || owner.is_failed() {
                        if !owner.is_failed() {
                            let _ = owner.record_device_lost(
                                false,
                                "the warmed GPU device was lost during preparation",
                            );
                        }
                        return false;
                    }
                }
                !warmed.gpu.as_ref().expect("warmed gpu").is_lost()
            }))
        } else {
            None
        };
    WarmSlot::start_warm_checked(
        handle,
        linger,
        move || make(owner_for_make),
        check_prepare,
    );
}

/// `warm`, then `frame`, then `done`, on a thread of its own, once a device waits in the slot
/// or none could be made: a window that opened on the CPU takes it from there
/// (`window::linux::handover`), and `Gpu::for_surface` makes one itself when the slot is empty.
#[cfg(target_os = "linux")]
pub(super) fn warm_then(
    features: wgpu::Features,
    frame: impl FnOnce(&Gpu, &mut Renderer) + Send + 'static,
    done: impl FnOnce() + Send + 'static,
) -> std::io::Result<()> {
    WarmSlot::warm_then(
        &SLOT,
        LINGER,
        move || make_owned(features, None),
        move |mut warmed| {
            warmed.claimed = true;
            let renderer = warmed.renderer.take()?;
            let mut renderer = Renderer::from_warm(renderer);
            frame(warmed.gpu.as_ref().expect("warmed gpu"), &mut renderer);
            let warm = renderer.into_warm()?;
            if warmed.gpu.as_ref().expect("warmed gpu").is_lost() {
                if let Some(owner) = &warmed.owner {
                    if !owner.is_failed() {
                        let _ = owner.record_device_lost(false, "the warmed GPU device was lost");
                    }
                    warmed.gpu.as_ref().expect("warmed gpu").detach_terminal_latch(owner);
                }
                return None;
            }
            warmed.renderer = Some(warm);
            warmed.claimed = false;
            Some(warmed)
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
    SLOT.park(&SLOT, Warmed::new(gpu, renderer, None), linger);
}

/// A device for a future surface on a GPU that drives a display, with the renderer, and one
/// pass run offscreen, so the device has drawn (`GpuEvent::Drew`).
fn make_owned(features: wgpu::Features, owner: Option<Arc<GpuLossLatch>>) -> Option<Warmed> {
    if SLOT.is_stopped() || owner.as_ref().is_some_and(|l| l.is_failed()) {
        return None;
    }
    super::gpu_events::starting();
    let instance = Gpu::instance_for(features);
    if SLOT.is_stopped() { return None; }
    let gpu = match Gpu::try_for_display(instance, features) {
        Ok(gpu) => gpu,
        Err(err) => {
            if let Some(owner) = &owner {
                let _ = owner.record_failure(GpuFailureKind::InitFailed, err);
            }
            return None;
        }
    };
    if let Some(owner) = &owner {
        owner.rearm();
        gpu.attach_terminal_latch(Arc::clone(owner));
    }
    if SLOT.is_stopped() {
        if let Some(owner) = &owner {
            gpu.prepare_clean_drop_for(owner, true);
        }
        return None;
    }
    let renderer = match Renderer::try_new_warm(&gpu.device, FORMAT) {
        Ok(renderer) => renderer,
        Err(err) => {
            if let Some(owner) = &owner {
                let _ = owner.record_failure(GpuFailureKind::InitFailed, err.to_string());
                gpu.detach_terminal_latch(owner);
            }
            return None;
        }
    };
    if SLOT.is_stopped() {
        if let Some(owner) = &owner {
            gpu.prepare_clean_drop_for(owner, true);
        }
        return None;
    }
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
    if SLOT.is_stopped() {
        if let Some(owner) = &owner {
            gpu.prepare_clean_drop_for(owner, true);
        }
        return None;
    }
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok();
    if let Some(owner) = &owner {
        if gpu.sample_removal() || gpu.is_lost() || owner.is_failed() {
            if !owner.is_failed() {
                let _ = owner.record_device_lost(false, "the GPU device was lost during warm-up");
            }
            gpu.detach_terminal_latch(owner);
            return None;
        }
    }
    if !SLOT.is_stopped() {
        super::gpu_events::presented();
    }
    Some(Warmed::new(gpu, renderer, owner))
}

/// Takes a warmed/parked device from `slot`. If a `Terminal` owner's parked device died while
/// waiting, polls and trips its owner latch before returning `None` so the owning surface refuses
/// fallback device creation. On healthy take, detaches the warm-up's latch registration before
/// transferring ownership to the taking surface.
pub(super) fn take_from_slot<C: Clock + 'static>(slot: &WarmSlot<Warmed, C>) -> Option<(Gpu, Renderer)> {
    let mut warmed = slot.take()?;
    warmed.claimed = true;
    let renderer = warmed.renderer.take()?;
    if let Some(owner) = warmed.owner.take() {
        rune_gpu::health::poll(&warmed.gpu.as_ref().expect("warmed gpu").device, wgpu::PollType::Poll).ok();
        if warmed.gpu.as_ref().expect("warmed gpu").sample_removal() || warmed.gpu.as_ref().expect("warmed gpu").is_lost() || owner.is_failed() {
            if !owner.is_failed() {
                let _ = owner.record_device_lost(false, "the warmed GPU device was lost while waiting");
            }
            warmed.gpu.as_ref().expect("warmed gpu").detach_terminal_latch(&owner);
            drop(renderer);
            return None;
        }
        // Clean handoff to the taking surface: detach the warm-up registration so the surface
        // owns its own attach/detach lifecycle (and a different Recover surface won't inherit it).
        warmed.gpu.as_ref().expect("warmed gpu").detach_terminal_latch(&owner);
    } else if warmed.gpu.as_ref().expect("warmed gpu").is_lost() {
        drop(renderer);
        return None;
    }
    // Safety-free move out of a Drop struct: replace gpu? Warmed implements Drop, so unpack via
    // take helper or Option<Gpu>.
    let gpu = warmed.gpu.take()?;
    Some((gpu, Renderer::from_warm(renderer)))
}

/// What `warm` made, waiting for it if it's still making it. A device that died while it waited
/// trips any owning `Terminal` latch and is dropped.
pub(super) fn take() -> Option<(Gpu, Renderer)> {
    take_from_slot(&SLOT)
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

/// Re-opens the global warm slot after a clean exit shutdown (used when another `App::run`
/// starts in the same process).
pub(super) fn reopen() {
    SLOT.reopen();
}

#[cfg(test)]
#[path = "warm_owner_tests.rs"]
mod owner_tests;
