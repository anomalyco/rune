//! Owned-overlay `GpuLossPolicy::Terminal` latch propagation, failure tripping, teardown, and
//! permanent refusal of future GPU/surface creation.

use std::sync::{Arc, Weak, atomic::Ordering};

use super::{Availability, OverlayApp, OverlayCore};
use crate::render::loss_latch::GpuFailureKind;

impl OverlayCore {
    #[inline]
    pub(super) fn is_terminal_gpu_policy(&self) -> bool {
        self.gpu_loss_latch.is_some()
    }

    #[inline]
    pub(super) fn gpu_loss_failed(&self) -> bool {
        self.gpu_loss_latch.as_ref().is_some_and(|l| l.is_failed())
    }

    /// Registers a wake hook on the shared `GpuLossLatch` when connected to the event loop so
    /// a main-window failure wakes this overlay immediately to drop its GPU and hide.
    pub(super) fn connect_terminal_gpu_wake(&self) {
        let Some(latch) = &self.gpu_loss_latch else { return };
        let shared = self.shared.clone();
        latch.add_wake(Arc::new(move |_| {
            shared.wake();
        }));
    }

    /// Attaches the shared terminal latch onto `gpu` when the overlay acquires or reuses it.
    pub(super) fn attach_terminal_gpu_latch(&self, gpu: &crate::render::Gpu) {
        if let Some(latch) = &self.gpu_loss_latch {
            latch.rearm();
            gpu.attach_terminal_latch(Arc::clone(latch));
        }
    }

    /// Samples `self.gpu` (including Windows D3D12 removal) and tears down the overlay if the
    /// shared latch has tripped. Returns `true` when permanently latched.
    ///
    /// Fast path for `GpuLossPolicy::Recover`: single branch, zero atomics, returns `false`.
    pub(super) fn poll_terminal_gpu_loss(&mut self, app: &mut dyn OverlayApp) -> bool {
        let Some(latch) = self.gpu_loss_latch.clone() else {
            return false;
        };
        if let Some(gpu) = self.gpu.as_ref() {
            if gpu.sample_removal() {
                // Already recorded DeviceRemoved on attached terminal latches.
            } else if gpu.is_lost() && !latch.is_failed() {
                let _ = latch.record_device_lost(false, "its GPU device was lost");
            }
        }
        if latch.is_failed() {
            self.enter_terminal_gpu_loss(app);
            return true;
        }
        false
    }

    /// Trips the app's shared terminal latch from an overlay GPU failure (preserving any earlier
    /// first reason) and tears down this overlay immediately.
    pub(super) fn latch_terminal_loss_latch(
        &mut self,
        app: &mut dyn OverlayApp,
        kind: GpuFailureKind,
        reason: impl Into<String>,
    ) {
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.mark_lost();
        }
        if let Some(latch) = self.gpu_loss_latch.clone() {
            let _ = latch.record_failure(kind, reason);
        }
        self.enter_terminal_gpu_loss(app);
    }

    /// Tears down an owned overlay once its shared terminal latch has failed: drops GPU and
    /// renderer, calls `release_compositor_gpu()`, clears `gpu_pool` if it still points at the
    /// failed GPU, hides and releases the native surface, records `failure()`, and marks
    /// `Availability::Unavailable` (notifying `on_availability` once).
    pub(super) fn enter_terminal_gpu_loss(&mut self, app: &mut dyn OverlayApp) {
        let reason = self
            .gpu_loss_latch
            .as_ref()
            .and_then(|l| l.reason())
            .unwrap_or_else(|| "the GPU device was lost".into());
        let had_gpu = self.gpu.is_some() || self.renderer.is_some();
        if let Some(gpu) = self.gpu.take() {
            gpu.mark_lost();
            let clear_pool = self.gpu_pool.borrow().upgrade().is_some_and(|p| Arc::ptr_eq(&p, &gpu));
            if clear_pool {
                *self.gpu_pool.borrow_mut() = Weak::default();
            }
        } else if self.gpu_pool.borrow().upgrade().is_some_and(|p| p.is_lost()) {
            *self.gpu_pool.borrow_mut() = Weak::default();
        }
        self.renderer = None;
        self.retire.clear();
        if had_gpu || !self.terminal_gpu_entered {
            app.release_compositor_gpu();
        }
        let needs_refuse = !self.terminal_gpu_entered
            || self.surface.live().is_some()
            || self.surface.parked().is_some()
            || self.presence.shown()
            || self.shared.availability.load(Ordering::Acquire) != Self::availability_code(Availability::Unavailable)
            || self.shared.failure.lock().unwrap().is_none();
        self.terminal_gpu_entered = true;
        if needs_refuse {
            self.surface_refused(app, reason);
        }
        self.dirty = false;
        self.idle = true;
    }

    /// Marks an intentional drop of the last healthy GPU reference during clean shutdown so
    /// drop-triggered `DeviceLostReason::Destroyed` does not falsely trip the terminal latch.
    pub(super) fn prepare_clean_gpu_drop(&self) {
        if let Some(latch) = &self.gpu_loss_latch
            && !latch.is_failed()
            && let Some(gpu) = self.gpu.as_ref()
            && Arc::strong_count(gpu) == 1
        {
            gpu.mark_clean_drop();
            latch.detach();
        }
    }
}

#[cfg(test)]
#[path = "gpu_loss_tests.rs"]
mod tests;
