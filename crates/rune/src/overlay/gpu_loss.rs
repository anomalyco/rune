//! Owned-overlay `GpuLossPolicy::Terminal` latch propagation, failure tripping, teardown, and
//! permanent refusal of future GPU/surface creation.

use std::sync::{Arc, Weak, atomic::Ordering};

use super::{Availability, OverlayApp, OverlayCore};
use crate::render::loss_latch::{
    FrameTeardownEffect, FrameTeardownEvent, GpuFailureKind,
};

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
    /// Marks that an overlay frame acquired a live `wgpu::SurfaceTexture`.
    #[inline]
    pub(super) fn note_frame_texture_acquired(&mut self) {
        let _ = self.frame_teardown.step(FrameTeardownEvent::TextureAcquired);
    }

    /// Marks that the overlay frame's `wgpu::SurfaceTexture` was presented or dropped, running
    /// any deferred terminal GPU teardown now that no texture is outstanding.
    #[inline]
    pub(super) fn note_frame_texture_released(&mut self, app: &mut dyn OverlayApp) {
        let effects = self.frame_teardown.step(FrameTeardownEvent::TextureReleased);
        if effects.into_iter().any(|e| e == FrameTeardownEffect::TeardownNow) {
            self.enter_terminal_gpu_loss(app);
        }
    }

    /// Debug self-check at every overlay surface teardown or reconfiguration point: no
    /// `SurfaceTexture` acquired from this overlay's surface may still be alive.
    #[inline]
    pub(super) fn assert_no_live_frame_texture(&self) {
        debug_assert!(
            std::thread::panicking() || !self.frame_teardown.has_live_texture(),
            "overlay surface teardown or reconfiguration while a SurfaceTexture is still alive",
        );
    }

    fn request_terminal_teardown(&mut self, app: &mut dyn OverlayApp) {
        let effects = self.frame_teardown.step(FrameTeardownEvent::LossNoticed);
        if effects.into_iter().any(|e| e == FrameTeardownEffect::TeardownNow) {
            self.enter_terminal_gpu_loss(app);
        }
    }

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
            self.request_terminal_teardown(app);
            return true;
        }
        false
    }

    /// Polls `self.gpu` once under `Terminal` so any synchronous validation or device-lost
    /// callback produced by encoding, `Compositor::features()`, or submission runs before the
    /// next GPU boundary. Treats `PollFailed::Lost` as real device loss rather than ignoring it.
    pub(super) fn poll_device_for_terminal_loss(&mut self, app: &mut dyn OverlayApp) -> bool {
        if !self.is_terminal_gpu_policy() {
            return false;
        }
        if let Some(gpu) = self.gpu.as_ref() {
            if matches!(rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll), Err(rune_gpu::health::PollFailed::Lost)) {
                gpu.mark_lost();
                if let Some(latch) = self.gpu_loss_latch.clone() && !latch.is_failed() {
                    let _ = latch.record_device_lost(false, "its GPU device was lost");
                }
            }
        }
        self.poll_terminal_gpu_loss(app)
    }

    /// Trips the app's shared terminal latch from an overlay GPU failure (preserving any earlier
    /// first reason) and tears down this overlay immediately. Only real `DeviceLost`/`DeviceRemoved`
    /// marks the shared `Gpu` lost; overlay-local `SurfaceFailed`/`InitFailed` leave a healthy
    /// shared `Gpu` usable by sibling `Recover` windows.
    pub(super) fn latch_terminal_loss_latch(
        &mut self,
        app: &mut dyn OverlayApp,
        kind: GpuFailureKind,
        reason: impl Into<String>,
    ) {
        if matches!(kind, GpuFailureKind::DeviceLost | GpuFailureKind::DeviceRemoved)
            && let Some(gpu) = self.gpu.as_ref()
        {
            gpu.mark_lost();
        }
        if let Some(latch) = self.gpu_loss_latch.clone() {
            let _ = latch.record_failure(kind, reason);
        }
        self.request_terminal_teardown(app);
    }

    /// Tears down an owned overlay once its shared terminal latch has failed: drops GPU and
    /// renderer, calls `release_compositor_gpu()`, clears `gpu_pool` if it still points at the
    /// failed GPU, hides and releases the native surface, records `failure()`, and marks
    /// `Availability::Unavailable` (notifying `on_availability` once).
    pub(super) fn enter_terminal_gpu_loss(&mut self, app: &mut dyn OverlayApp) {
        self.assert_no_live_frame_texture();
        let reason = self
            .gpu_loss_latch
            .as_ref()
            .and_then(|l| l.reason())
            .unwrap_or_else(|| "the GPU device was lost".into());
        let had_gpu = self.gpu.is_some() || self.renderer.is_some();
        if let Some(gpu) = self.gpu.take() {
            if let Some(latch) = &self.gpu_loss_latch {
                gpu.detach_terminal_latch(latch);
            }
            if gpu.is_lost() && self.gpu_pool.borrow().upgrade().is_some_and(|p| Arc::ptr_eq(&p, &gpu)) {
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
            || self.has_pending_layer()
            || self.presence.shown()
            || self.shared.availability.load(Ordering::Acquire) != Self::availability_code(Availability::Unavailable)
            || self.shared.failure.lock().unwrap().is_none();
        self.terminal_gpu_entered = true;
        #[cfg(any(test, target_os = "macos"))]
        app.rt_mut().paint.set_native_materials(false);
        #[cfg(any(test, all(target_os = "windows", feature = "windows-system-material")))]
        app.rt_mut().paint.set_windows_acrylic_materials(false);
        self.close_after_hidden(app);
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
            && let Some(gpu) = self.gpu.as_ref()
        {
            gpu.prepare_clean_drop_for(latch, Arc::strong_count(gpu) == 1);
        }
    }
}

#[cfg(test)]
#[path = "gpu_loss_tests.rs"]
mod tests;
