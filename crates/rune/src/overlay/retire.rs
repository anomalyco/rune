//! Non-blocking retirement of GPU resources dropped by a hidden or parked overlay.
//!
//! Dropping GPU textures or a renderer on a shared device polls once with `PollType::Poll` so
//! already-finished submissions retire immediately without waiting on the UI thread. When work
//! is still in flight on the queue, `Retire` arms a work-done flag on that queue and schedules
//! non-blocking retries on later event-loop turns with bounded backoff until that submission
//! retires (via a retry poll or another surface's submit) or the device is lost or dropped.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use crate::machine::Effects;
use crate::render::Gpu;
use crate::time::Instant;

/// First retry interval after a non-blocking poll finds work still in flight.
const INITIAL_RETRY: Duration = Duration::from_millis(16);
/// Upper bound on the backoff while waiting for a busy or stalled GPU.
const MAX_RETRY: Duration = Duration::from_millis(250);

/// State of deferred non-blocking GPU retirement for an overlay.
#[derive(Default)]
pub(super) enum Retire<G = Weak<Gpu>> {
    /// Nothing is waiting for the GPU to finish.
    #[default]
    Idle,
    /// Work submitted before a hide or park is still in flight on `gpu`.
    /// `done` flips true once wgpu triages that submission (either on our retry poll or on
    /// another surface's submit).
    Pending {
        gpu: G,
        done: Arc<AtomicBool>,
        due: Instant,
        delay: Duration,
    },
}

enum RetireEvent<G = Weak<Gpu>> {
    /// An immediate non-blocking poll after dropping resources found work still in flight.
    Busy { gpu: G, done: Arc<AtomicBool>, now: Instant },
    /// A retry deadline may have arrived.
    Due { now: Instant },
    /// The due poll ran and found the work still in flight (`done` still false).
    StillBusy { now: Instant },
    /// The work retired, the device was lost, or the device was dropped.
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetireEffect {
    /// Perform one non-blocking `PollType::Poll` on the pending device and feed back
    /// `Finished` or `StillBusy`.
    PollNow,
}

impl<G> Retire<G> {
    fn step(&mut self, event: RetireEvent<G>) -> Effects<RetireEffect> {
        match event {
            RetireEvent::Busy { gpu, done, now } => {
                *self = Retire::Pending { gpu, done, due: now + INITIAL_RETRY, delay: INITIAL_RETRY };
                Effects::none()
            }
            RetireEvent::Due { now } => match self {
                Retire::Idle => Effects::none(),
                Retire::Pending { done, .. } if done.load(Ordering::Acquire) => {
                    *self = Retire::Idle;
                    Effects::none()
                }
                Retire::Pending { due, .. } if *due <= now => Effects::one(RetireEffect::PollNow),
                Retire::Pending { .. } => Effects::none(),
            },
            RetireEvent::StillBusy { now } => {
                match self {
                    Retire::Pending { done, .. } if done.load(Ordering::Acquire) => {
                        *self = Retire::Idle;
                    }
                    Retire::Pending { due, delay, .. } => {
                        *delay = (*delay * 2).min(MAX_RETRY);
                        *due = now + *delay;
                    }
                    Retire::Idle => {}
                }
                Effects::none()
            }
            RetireEvent::Finished => {
                *self = Retire::Idle;
                Effects::none()
            }
        }
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        match self {
            Retire::Idle => None,
            Retire::Pending { done, due, .. } => (!done.load(Ordering::Acquire)).then_some(*due),
        }
    }

    pub(super) fn clear(&mut self) {
        self.step(RetireEvent::Finished);
    }
}

impl Retire<Weak<Gpu>> {
    /// Polls `gpu` once without waiting. When work is still in flight, arms a deferred
    /// non-blocking retry so dropped textures and renderer resources still retire once that work
    /// ends, even if every surface on the device is idle afterwards.
    pub(super) fn retire_now(&mut self, gpu: &Arc<Gpu>, now: Instant) {
        if gpu.is_lost() {
            self.clear();
            return;
        }
        match rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll) {
            Ok(wgpu::PollStatus::Poll) => {
                let done = Arc::new(AtomicBool::new(false));
                let flag = done.clone();
                gpu.queue.on_submitted_work_done(move || flag.store(true, Ordering::Release));
                if matches!(rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll), Ok(wgpu::PollStatus::QueueEmpty) | Err(_))
                    || done.load(Ordering::Acquire)
                {
                    self.clear();
                } else {
                    self.step(RetireEvent::Busy { gpu: Arc::downgrade(gpu), done, now });
                }
            }
            Ok(wgpu::PollStatus::QueueEmpty | wgpu::PollStatus::WaitSucceeded) | Err(_) => {
                self.clear();
            }
        }
    }

    /// Runs a due non-blocking poll, clearing once the tracked submission has retired or backing
    /// off while it is still in flight.
    pub(super) fn poll_due(&mut self, now: Instant) {
        for effect in self.step(RetireEvent::Due { now }) {
            match effect {
                RetireEffect::PollNow => {
                    let (gpu, done) = match self {
                        Retire::Pending { gpu, done, .. } => (gpu.upgrade(), done.clone()),
                        Retire::Idle => return,
                    };
                    let Some(gpu) = gpu.filter(|gpu| !gpu.is_lost()) else {
                        self.clear();
                        return;
                    };
                    match rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll) {
                        Ok(wgpu::PollStatus::Poll) if !done.load(Ordering::Acquire) => {
                            self.step(RetireEvent::StillBusy { now });
                        }
                        _ => self.clear(),
                    }
                }
            }
        }
    }

    /// Upgrades and clears any pending shared device for one last non-blocking poll on drop.
    pub(super) fn take_gpu(&mut self) -> Option<Arc<Gpu>> {
        match std::mem::take(self) {
            Retire::Pending { gpu, done, .. } if !done.load(Ordering::Acquire) => gpu.upgrade(),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "retire_tests.rs"]
mod tests;
