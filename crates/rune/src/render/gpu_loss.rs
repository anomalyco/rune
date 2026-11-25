//! Device loss: the callback that tells Rune a device went away, and what a surface does about
//! it (`window/host/create.rs::Host::reattach`). wgpu reports a dead device only through
//! `Device::set_device_lost_callback`; `Surface::configure`'s `DeviceLost` is dropped, so a lost
//! device would otherwise surface as a panic on the surface's next drawable.
//!
//! The native Metal backend (`gpu/metal/device.rs`) reports both failed command buffers and
//! `Device::destroy` through `set_device_lost_callback`, setting `is_lost`.

use super::{Gpu, Renderer};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

/// The distinct messages `Gpu::made` has printed, bounded: an error inside the frame would
/// otherwise print on every frame.
#[cfg(not(target_arch = "wasm32"))]
const PRINTED: usize = 16;

/// Shared callback state owned by a [`Gpu`] and cloned into a [`Renderer`]'s [`GpuHost`].
///
/// Sharing `lost`, `uncaptured_logged`, and the deduplication log lets
/// [`crate::compose::Frame::restore_device_callbacks`] re-register Rune's single-slot wgpu
/// callbacks against the same state after an embedded engine replaces them, without storing a
/// `wgpu::Device` or `Arc<Gpu>` inside `Renderer`.
#[cfg(not(target_arch = "wasm32"))]
type TerminalLatches = Arc<Mutex<Vec<Arc<super::loss_latch::GpuLossLatch>>>>;

#[derive(Clone)]
pub(crate) struct GpuCallbacks {
    lost: Arc<AtomicBool>,
    uncaptured_logged: Arc<AtomicBool>,
    #[cfg(not(target_arch = "wasm32"))]
    printed: Arc<Mutex<Printed>>,
    #[cfg(not(target_arch = "wasm32"))]
    terminal: TerminalLatches,
    #[cfg(not(target_arch = "wasm32"))]
    clean_drop: Arc<AtomicBool>,
    #[cfg(target_arch = "wasm32")]
    web: Arc<Mutex<Option<crate::platform::web::health::WebRestore>>>,
}

/// Host GPU handles and callback state attached to a [`Renderer`] for [`crate::compose::Frame`].
///
/// Intentionally holds only `wgpu::Instance`, `wgpu::Adapter`, and [`GpuCallbacks`] — never a
/// `wgpu::Device` or `Arc<Gpu>` — so dropping `Gpu` and `Renderer` on Windows D3D12 never pins
/// a removed `ID3D12Device`.
pub(crate) struct GpuHost {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    callbacks: GpuCallbacks,
}

impl GpuCallbacks {
    pub(crate) fn new(device: &wgpu::Device, draws_surfaces: bool) -> Self {
        let lost = Arc::new(AtomicBool::new(false));
        #[cfg(not(target_arch = "wasm32"))]
        let terminal: TerminalLatches = Arc::default();
        #[cfg(not(target_arch = "wasm32"))]
        let clean_drop = Arc::new(AtomicBool::new(false));
        #[cfg(not(target_arch = "wasm32"))]
        install_lost_callback(device, &lost, Arc::clone(&terminal), Arc::clone(&clean_drop));
        #[cfg(target_arch = "wasm32")]
        install_lost_callback_web(device, &lost);
        let callbacks = Self {
            lost,
            uncaptured_logged: Arc::new(AtomicBool::new(false)),
            #[cfg(not(target_arch = "wasm32"))]
            printed: Arc::default(),
            #[cfg(not(target_arch = "wasm32"))]
            terminal,
            #[cfg(not(target_arch = "wasm32"))]
            clean_drop,
            #[cfg(target_arch = "wasm32")]
            web: Arc::default(),
        };
        if draws_surfaces {
            callbacks.install_surface_error_policy(device);
        }
        callbacks
    }

    /// Attaches a `GpuLossPolicy::Terminal` latch to this shared callback state and installs both
    /// device-lost and uncaptured-error callbacks (even under `cfg(test)`) onto the same latch.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn attach_terminal_latch(
        &self,
        device: &wgpu::Device,
        latch: Arc<super::loss_latch::GpuLossLatch>,
    ) {
        {
            let mut list = self.terminal.lock().unwrap_or_else(|e| e.into_inner());
            if !list.iter().any(|existing| Arc::ptr_eq(existing, &latch)) {
                list.push(latch);
            }
        }
        self.uncaptured_logged.store(true, Ordering::Release);
        install_lost_callback(device, &self.lost, Arc::clone(&self.terminal), Arc::clone(&self.clean_drop));
        install_uncaptured_terminal(
            device,
            Arc::clone(&self.printed),
            Arc::clone(&self.lost),
            Arc::clone(&self.terminal),
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn mark_lost(&self) {
        self.lost.store(true, Ordering::Release);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn mark_clean_drop(&self) {
        self.clean_drop.store(true, Ordering::Release);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn notify_removed(&self) {
        self.lost.store(true, Ordering::Release);
        let latches = self.terminal.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for latch in latches {
            let _ = latch.record_failure(
                super::loss_latch::GpuFailureKind::DeviceRemoved,
                "the GPU device was removed (D3D12)",
            );
        }
    }

    pub(crate) fn install_surface_error_policy(&self, device: &wgpu::Device) {
        if self.uncaptured_logged.swap(true, Ordering::AcqRel) {
            return;
        }
        // On wasm32, `platform::web::health::watch` owns uncaptured error logging for both WebGPU
        // (through JS, avoiding wgpu's panic on `GPUInternalError`) and WebGL2 (to `console.error`,
        // where `eprintln!` is silent).
        #[cfg(not(target_arch = "wasm32"))]
        if should_log_uncaptured() {
            log_uncaptured(device, Arc::clone(&self.printed));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = device;
    }

    #[cfg(test)]
    pub(crate) fn has_surface_error_policy(&self) -> bool {
        self.uncaptured_logged.load(Ordering::Acquire)
    }

    pub(crate) fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn lost_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.lost)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn set_web_restore(&self, restore: crate::platform::web::health::WebRestore) {
        *self.web.lock().unwrap_or_else(|e| e.into_inner()) = Some(restore);
    }

    /// Re-installs Rune's device-lost and (where enabled) uncaptured-error callbacks on `device`.
    pub(crate) fn restore(&self, device: &wgpu::Device) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            install_lost_callback(device, &self.lost, Arc::clone(&self.terminal), Arc::clone(&self.clean_drop));
            let has_terminal = !self.terminal.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
            if has_terminal {
                install_uncaptured_terminal(
                    device,
                    Arc::clone(&self.printed),
                    Arc::clone(&self.lost),
                    Arc::clone(&self.terminal),
                );
            } else if self.uncaptured_logged.load(Ordering::Acquire) && should_log_uncaptured() {
                log_uncaptured(device, Arc::clone(&self.printed));
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let web = self.web.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(web) = web {
                web.restore(device);
            } else if device.as_webgpu().is_some() {
                crate::platform::web::health::restore_webgpu(device);
            } else {
                install_lost_callback_web(device, &self.lost);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn install_lost_callback(
    device: &wgpu::Device,
    lost: &Arc<AtomicBool>,
    terminal: TerminalLatches,
    clean_drop: Arc<AtomicBool>,
) {
    let lost_by_callback = Arc::clone(lost);
    device.set_device_lost_callback(move |reason, message| {
        // Before anything reads it: a lost device's surface can't be configured, and wgpu
        // reports that failure nowhere (`is_lost`).
        lost_by_callback.store(true, Ordering::Release);
        let destroyed = matches!(reason, wgpu::DeviceLostReason::Destroyed);
        let intentional_drop = destroyed && clean_drop.load(Ordering::Acquire);
        if !intentional_drop {
            let latches = terminal.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if !latches.is_empty() {
                let text = if message.trim().is_empty() {
                    format!("the GPU device was lost ({reason:?})")
                } else {
                    format!("the GPU device was lost ({reason:?}): {message}")
                };
                for latch in latches {
                    let _ = latch.record_device_lost(destroyed, text.clone());
                }
            }
        }
        // No panic here, in tests either: wgpu calls this inside its own work (a submission,
        // a queue's drop) and a panic there could abort the process. A test fails at its
        // next wait instead (`rune_gpu::health::poll` returns `Lost`).
        eprintln!("rune: the GPU device was lost ({reason:?}): {message}");
    });
}

#[cfg(target_arch = "wasm32")]
fn install_lost_callback_web(device: &wgpu::Device, lost: &Arc<AtomicBool>) {
    let lost_by_callback = Arc::clone(lost);
    device.set_device_lost_callback(move |_reason, _message| {
        lost_by_callback.store(true, Ordering::Release);
    });
}

impl Gpu {
    /// A GPU over a device made elsewhere (`device_on`, a native backend, an example): Rune
    /// watches it for loss, and on a surface device logs its uncaptured errors rather than
    /// letting wgpu's default handler panic the app. That handler replaces any handler set on
    /// the device before it was handed over.
    pub fn from_device(instance: wgpu::Instance, adapter: wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue) -> Gpu {
        Self::made(instance, adapter, device, queue, true)
    }

    /// `from_device`, saying whether this device draws surfaces (`Host::try_create`,
    /// `Overlay::attach`): only those log uncaptured errors. A headless device is usually a
    /// test's or an offscreen render's, where a validation error should still fail loudly, and
    /// under `cfg(test)` nothing logs so Rune's own tests see wgpu's panic.
    pub(crate) fn made(instance: wgpu::Instance, adapter: wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue,
        draws_surfaces: bool) -> Gpu {
        let callbacks = GpuCallbacks::new(&device, draws_surfaces);
        let removal = rune_gpu::health::Removal::watch(&device);
        Gpu {
            instance,
            adapter,
            device,
            queue,
            callbacks,
            removal,
        }
    }

    /// Installs Rune's uncaptured-error logger on a device that draws surfaces, at most once.
    ///
    /// Headless devices (`Gpu::headless`, `Gpu::try_new(_, None)`, `Gpu::for_headless_setup`)
    /// never call this so offscreen renders and tests keep wgpu's validation panic. Surface
    /// devices call it at creation (`Gpu::made(..., true)`) and on surface adoption
    /// (`Gpu::for_surface_setup`); repeat calls are no-ops so the handler is never installed twice.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn install_surface_error_policy(&self) {
        self.callbacks.install_surface_error_policy(&self.device);
    }

    #[cfg(test)]
    pub(crate) fn has_surface_error_policy(&self) -> bool {
        self.callbacks.has_surface_error_policy()
    }

    /// Whether the device went away: a driver reset, a GPU removed, a resume from Modern Standby.
    /// Nothing can be drawn with it, and its surfaces can't be configured, so its owner makes a
    /// new GPU and surface instead (`Host::reattach`).
    /// On D3D12 the device is asked too: wgpu hears of a removal only at the next submission.
    pub fn is_lost(&self) -> bool { self.callbacks.is_lost() || self.removal.removed() }

    /// Attaches a `GpuLossPolicy::Terminal` latch onto this GPU's shared `GpuCallbacks`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn attach_terminal_latch(&self, latch: Arc<super::loss_latch::GpuLossLatch>) {
        self.callbacks.attach_terminal_latch(&self.device, latch);
    }

    /// Samples Windows D3D12 device removal and latches attached terminal latches immediately.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn sample_removal(&self) -> bool {
        if self.removal.removed() {
            self.callbacks.notify_removed();
            true
        } else {
            false
        }
    }

    /// Marks this GPU lost so shared overlays and sibling windows stop using it.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn mark_lost(&self) {
        self.callbacks.mark_lost();
    }

    /// Marks an upcoming drop of this healthy GPU as intentional so drop-triggered `Destroyed`
    /// does not falsely trip a terminal latch.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn mark_clean_drop(&self) {
        self.callbacks.mark_clean_drop();
    }

    /// Captures the owning instance, adapter, and shared callback state for a [`Renderer`].
    pub(crate) fn host(&self) -> GpuHost {
        GpuHost {
            instance: self.instance.clone(),
            adapter: self.adapter.clone(),
            callbacks: self.callbacks.clone(),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn lost_flag(&self) -> Arc<AtomicBool> {
        self.callbacks.lost_flag()
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn set_web_restore(&self, restore: crate::platform::web::health::WebRestore) {
        self.callbacks.set_web_restore(restore);
    }
}

impl Renderer {
    /// Attaches the owning [`Gpu`]'s instance, adapter, and callback state once at renderer setup
    /// so custom compositors can borrow them through [`crate::compose::Frame`].
    pub(crate) fn attach_gpu(&mut self, gpu: &Gpu) {
        self.gpu_host = Some(gpu.host());
    }

    pub(crate) fn gpu_instance(&self) -> Option<&wgpu::Instance> {
        self.gpu_host.as_ref().map(|host| &host.instance)
    }

    pub(crate) fn gpu_adapter(&self) -> Option<&wgpu::Adapter> {
        self.gpu_host.as_ref().map(|host| &host.adapter)
    }

    pub(crate) fn restore_device_callbacks(&self, device: &wgpu::Device) {
        if let Some(host) = &self.gpu_host {
            host.callbacks.restore(device);
        }
    }
}

/// What `log_uncaptured` has printed: each distinct message once, up to `PRINTED`, then one
/// line saying the rest are silent.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct Printed {
    seen: Vec<String>,
    silenced: bool,
}

/// Prints each uncaptured error once, up to `PRINTED` distinct messages, then one line saying
/// further ones are not shown. A scoped error (a `push_error_scope`, as `window::configure`
/// uses) still goes to its scope first; this is the fallback for errors nothing else caught.
#[cfg(not(target_arch = "wasm32"))]
fn install_uncaptured_terminal(
    device: &wgpu::Device,
    printed: Arc<Mutex<Printed>>,
    lost: Arc<AtomicBool>,
    terminal: TerminalLatches,
) {
    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        let message = error.to_string();
        lost.store(true, Ordering::Release);
        let latches = terminal.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for latch in latches {
            let _ = latch.record_failure(
                super::loss_latch::GpuFailureKind::UncapturedError,
                format!("uncaptured GPU error: {message}"),
            );
        }
        let line = {
            let mut printed = printed.lock().unwrap_or_else(|e| e.into_inner());
            if printed.silenced || printed.seen.contains(&message) {
                None
            } else if printed.seen.len() < PRINTED {
                printed.seen.push(message.clone());
                Some(format!("rune: a GPU error went uncaptured: {message}"))
            } else {
                printed.silenced = true;
                Some(format!("rune: {PRINTED} distinct GPU errors went uncaptured; further uncaptured GPU errors are not shown"))
            }
        };
        if let Some(line) = line { eprintln!("{line}"); }
    }));
}

#[cfg(not(target_arch = "wasm32"))]
fn log_uncaptured(device: &wgpu::Device, printed: Arc<Mutex<Printed>>) {
    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        let message = error.to_string();
        let line = {
            let mut printed = printed.lock().unwrap_or_else(|e| e.into_inner());
            if printed.silenced || printed.seen.contains(&message) {
                None
            } else if printed.seen.len() < PRINTED {
                printed.seen.push(message.clone());
                Some(format!("rune: a GPU error went uncaptured: {message}"))
            } else {
                printed.silenced = true;
                Some(format!("rune: {PRINTED} distinct GPU errors went uncaptured; further uncaptured GPU errors are not shown"))
            }
        };
        if let Some(line) = line { eprintln!("{line}"); }
    }));
}

#[cfg(all(test, not(target_arch = "wasm32")))]
static INSTALL_UNCAPTURED_IN_TEST: AtomicBool = AtomicBool::new(false);

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(super) fn enable_uncaptured_logging_for_test() {
    INSTALL_UNCAPTURED_IN_TEST.store(true, Ordering::Release);
}

#[cfg(not(target_arch = "wasm32"))]
fn should_log_uncaptured() -> bool {
    #[cfg(not(test))]
    { true }
    #[cfg(test)]
    { INSTALL_UNCAPTURED_IN_TEST.load(Ordering::Acquire) }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "gpu_loss_tests.rs"]
mod tests;
