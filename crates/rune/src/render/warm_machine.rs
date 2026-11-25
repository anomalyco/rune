//! Explicit state machine and single owner for background GPU warm-up threads.
//!
//! Keeps launch speed untouched (zero new work or waits on the normal path) while giving exit
//! paths (`orderly_exit()`, startup/display errors) one owner that can cancel in-flight GPU
//! warm-up, skip follow-up callbacks (`prepare`, `frame`, `done`, linger waits), and wait with
//! a hard bound for every tracked worker thread to exit before process destructors run.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rune_core::machine::Effects;

use crate::time::Instant;

/// Last-resort maximum time exit waits for in-flight GPU warm-up threads before ending the
/// process without running destructors under a wedged driver call. Generous enough that
/// ordinary slow cold Vulkan/shader/software startup under parallel load drains cleanly and
/// returns from `App::run()` / `App::try_run()`, while matching `relaunch::WAIT` and
/// `open_url::instance::HANDED_WAIT` (both 10 s) so a hung driver does not linger holding
/// locks past a relaunch wait.
pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Clock source for warm-up deadlines and bounded shutdown waits (injected in tests).
pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Real monotonic process clock.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        crate::time::now()
    }
}

/// Result of [`WarmSlot::shutdown`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShutdownOutcome {
    /// Every warm-up worker exited before the bound; safe to drop windows and run normal exit.
    Drained,
    /// At least one worker stayed blocked inside a driver call past the bound; the caller must
    /// end the process without running destructors underneath it.
    TimedOut { remaining: usize },
}

/// Explicit lifecycle state of GPU warm-up ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// No warm-up running and no device parked. `draining` counts previously spawned workers
    /// that are unwinding to exit (0 at rest).
    NotStarted { draining: usize },
    /// A warm-up build (`make`) or offscreen warm-frame (`frame`) is in flight.
    Running { until: Instant, workers: usize },
    /// A ready device is parked in the slot until `until`.
    Parked { until: Instant, workers: usize },
    /// Exit cancelled warm-up while `workers > 0` threads are still finishing; once
    /// `Shutdown` starts the bounded wait, `deadline` is `Some(at)`.
    Cancelled { deadline: Option<Instant>, workers: usize },
    /// Shutdown finished (`timed_out_workers == 0` when drained; `> 0` when the bound expired).
    Done { timed_out_workers: usize },
}

impl Default for State {
    fn default() -> Self {
        Self::NotStarted { draining: 0 }
    }
}

impl State {
    /// Number of tracked worker threads currently alive.
    #[cfg(test)]
    pub(crate) const fn live_workers(self) -> usize {
        match self {
            Self::NotStarted { draining } => draining,
            Self::Running { workers, .. }
            | Self::Parked { workers, .. }
            | Self::Cancelled { workers, .. } => workers,
            Self::Done { timed_out_workers } => timed_out_workers,
        }
    }

    /// Whether exit has cancelled or shut down warm-up.
    pub(crate) const fn is_stopped(self) -> bool {
        matches!(self, Self::Cancelled { .. } | Self::Done { .. })
    }
}

/// Inputs to [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// Start warming a GPU (or extend the linger deadline if already running/parked).
    Start { until: Instant },
    /// Attach an auxiliary tracked worker (`rune-gpu-window` or live-device `prepare`) unless
    /// warm-up is cancelled or done.
    AttachWorker,
    /// Park a finished overlay device until `until` with a linger watcher thread.
    Park { until: Instant },
    /// The warm-up build finished (`ok` = device/renderer produced).
    BuildFinished { ok: bool, now: Instant },
    /// `warm_then` claims the parked device to draw its offscreen warm frame.
    BeginWarmFrame,
    /// `warm_then` finished its offscreen warm frame (`ok` = device still usable).
    FinishWarmFrame { ok: bool, now: Instant },
    /// A surface claims the parked device (`take`).
    Take,
    /// Non-blocking discard (during launch or headless): drops parked work or expires in-flight
    /// linger without blocking.
    Discard { now: Instant },
    /// Non-blocking early cancellation as soon as `Windows::exiting` begins, before the event
    /// loop returns and `Shutdown` starts the bounded wait.
    Cancel,
    /// Exit-time cancellation with a bounded wait until `deadline`.
    Shutdown { deadline: Instant },
    /// A tracked worker thread exited (or failed to spawn).
    WorkerExited,
    /// Clock check for parked linger expiry or shutdown wait bound.
    Tick { now: Instant },
    /// Re-opens a cleanly drained slot (`Done { timed_out_workers: 0 }`) for a fresh run.
    Reopen,
}

/// Actions returned by [`step`] for the owner to carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Spawn the warm-up or linger worker thread (worker count already incremented).
    SpawnWorker,
    /// Auxiliary worker attachment accepted (worker count already incremented).
    WorkerAttached,
    /// Store the caller's payload in the slot.
    StorePayload,
    /// Take the parked payload out of the slot.
    TakePayload,
    /// Drop any parked payload (outside the lock) or drop the caller's local payload.
    DropPayload,
    /// Invoke `warm_then`'s `done` callback (skipped on cancellation).
    NotifyDone,
    /// Wake all condvar waiters.
    NotifyWaiters,
}

/// Pure state transition for the GPU warm-up owner.
pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let mut fx = Effects::none();
    match (*state, event) {
        (State::NotStarted { draining }, Event::Start { until }) => {
            *state = State::Running { until, workers: draining + 1 };
            fx.push(Effect::SpawnWorker);
        }
        (State::Running { until: cur, workers }, Event::Start { until }) => {
            *state = State::Running { until: cur.max(until), workers };
        }
        (State::Parked { until: cur, workers }, Event::Start { until }) => {
            *state = State::Parked { until: cur.max(until), workers };
        }
        (State::Cancelled { .. } | State::Done { .. }, Event::Start { .. }) => {}

        (State::NotStarted { draining }, Event::AttachWorker) => {
            *state = State::NotStarted { draining: draining + 1 };
            fx.push(Effect::WorkerAttached);
        }
        (State::Running { until, workers }, Event::AttachWorker) => {
            *state = State::Running { until, workers: workers + 1 };
            fx.push(Effect::WorkerAttached);
        }
        (State::Parked { until, workers }, Event::AttachWorker) => {
            *state = State::Parked { until, workers: workers + 1 };
            fx.push(Effect::WorkerAttached);
        }
        (State::Cancelled { .. } | State::Done { .. }, Event::AttachWorker) => {}

        (State::NotStarted { draining }, Event::Park { until }) => {
            *state = State::Parked { until, workers: draining + 1 };
            fx.push(Effect::StorePayload);
            fx.push(Effect::SpawnWorker);
            fx.push(Effect::NotifyWaiters);
        }
        (
            State::Running { .. }
            | State::Parked { .. }
            | State::Cancelled { .. }
            | State::Done { .. },
            Event::Park { .. },
        ) => {
            fx.push(Effect::DropPayload);
        }

        (State::Running { until, workers }, Event::BuildFinished { ok: true, now })
            if now < until =>
        {
            *state = State::Parked { until, workers };
            fx.push(Effect::StorePayload);
            fx.push(Effect::NotifyWaiters);
        }
        (State::Running { workers, .. }, Event::BuildFinished { .. }) => {
            *state = State::NotStarted { draining: workers };
            fx.push(Effect::DropPayload);
            fx.push(Effect::NotifyWaiters);
        }
        (_, Event::BuildFinished { .. }) => {
            fx.push(Effect::DropPayload);
        }

        (State::Parked { until, workers }, Event::BeginWarmFrame) => {
            *state = State::Running { until, workers };
            fx.push(Effect::TakePayload);
            fx.push(Effect::NotifyWaiters);
        }
        (_, Event::BeginWarmFrame) => {}

        (State::Running { until, workers }, Event::FinishWarmFrame { ok: true, now })
            if now < until =>
        {
            *state = State::Parked { until, workers };
            fx.push(Effect::StorePayload);
            fx.push(Effect::NotifyWaiters);
            fx.push(Effect::NotifyDone);
        }
        (State::Running { workers, .. }, Event::FinishWarmFrame { .. }) => {
            *state = State::NotStarted { draining: workers };
            fx.push(Effect::DropPayload);
            fx.push(Effect::NotifyWaiters);
            fx.push(Effect::NotifyDone);
        }
        (_, Event::FinishWarmFrame { .. }) => {
            fx.push(Effect::DropPayload);
        }

        (State::Parked { workers, .. }, Event::Take) => {
            *state = State::NotStarted { draining: workers };
            fx.push(Effect::TakePayload);
            fx.push(Effect::NotifyWaiters);
        }
        (_, Event::Take) => {}

        (State::Parked { workers, .. }, Event::Discard { .. }) => {
            *state = State::NotStarted { draining: workers };
            fx.push(Effect::DropPayload);
            fx.push(Effect::NotifyWaiters);
        }
        (State::Running { workers, .. }, Event::Discard { now }) => {
            *state = State::Running { until: now, workers };
            fx.push(Effect::NotifyWaiters);
        }
        (State::NotStarted { .. } | State::Cancelled { .. } | State::Done { .. }, Event::Discard { .. }) => {}

        (State::NotStarted { draining: 0 }, Event::Cancel) => {
            *state = State::Done { timed_out_workers: 0 };
            fx.push(Effect::NotifyWaiters);
        }
        (State::NotStarted { draining }, Event::Cancel) => {
            *state = State::Cancelled { deadline: None, workers: draining };
            fx.push(Effect::NotifyWaiters);
        }
        (State::Running { workers: 0, .. }, Event::Cancel) => {
            *state = State::Done { timed_out_workers: 0 };
            fx.push(Effect::NotifyWaiters);
        }
        (State::Running { workers, .. }, Event::Cancel) => {
            *state = State::Cancelled { deadline: None, workers };
            fx.push(Effect::NotifyWaiters);
        }
        (State::Parked { workers: 0, .. }, Event::Cancel) => {
            *state = State::Done { timed_out_workers: 0 };
            fx.push(Effect::DropPayload);
            fx.push(Effect::NotifyWaiters);
        }
        (State::Parked { workers, .. }, Event::Cancel) => {
            *state = State::Cancelled { deadline: None, workers };
            fx.push(Effect::DropPayload);
            fx.push(Effect::NotifyWaiters);
        }
        (State::Cancelled { .. } | State::Done { .. }, Event::Cancel) => {}

        (State::NotStarted { draining: 0 }, Event::Shutdown { .. }) => {
            *state = State::Done { timed_out_workers: 0 };
            fx.push(Effect::NotifyWaiters);
        }
        (State::NotStarted { draining }, Event::Shutdown { deadline }) => {
            *state = State::Cancelled { deadline: Some(deadline), workers: draining };
            fx.push(Effect::NotifyWaiters);
        }
        (State::Running { workers: 0, .. }, Event::Shutdown { .. }) => {
            *state = State::Done { timed_out_workers: 0 };
            fx.push(Effect::NotifyWaiters);
        }
        (State::Running { workers, .. }, Event::Shutdown { deadline }) => {
            *state = State::Cancelled { deadline: Some(deadline), workers };
            fx.push(Effect::NotifyWaiters);
        }
        (State::Parked { workers: 0, .. }, Event::Shutdown { .. }) => {
            *state = State::Done { timed_out_workers: 0 };
            fx.push(Effect::DropPayload);
            fx.push(Effect::NotifyWaiters);
        }
        (State::Parked { workers, .. }, Event::Shutdown { deadline }) => {
            *state = State::Cancelled { deadline: Some(deadline), workers };
            fx.push(Effect::DropPayload);
            fx.push(Effect::NotifyWaiters);
        }
        (State::Cancelled { deadline: cur, workers }, Event::Shutdown { deadline }) => {
            let bound = cur.map_or(deadline, |existing| existing.min(deadline));
            *state = State::Cancelled { deadline: Some(bound), workers };
            fx.push(Effect::NotifyWaiters);
        }
        (State::Done { .. }, Event::Shutdown { .. }) => {}

        (State::NotStarted { draining }, Event::WorkerExited) => {
            *state = State::NotStarted { draining: draining.saturating_sub(1) };
        }
        (State::Running { until, workers }, Event::WorkerExited) => {
            *state = State::Running { until, workers: workers.saturating_sub(1) };
        }
        (State::Parked { until, workers }, Event::WorkerExited) => {
            *state = State::Parked { until, workers: workers.saturating_sub(1) };
        }
        (State::Cancelled { deadline, workers }, Event::WorkerExited) => {
            let remaining = workers.saturating_sub(1);
            if remaining == 0 {
                *state = State::Done { timed_out_workers: 0 };
                fx.push(Effect::NotifyWaiters);
            } else {
                *state = State::Cancelled { deadline, workers: remaining };
            }
        }
        (State::Done { timed_out_workers }, Event::WorkerExited) => {
            *state = State::Done { timed_out_workers: timed_out_workers.saturating_sub(1) };
        }

        (State::Parked { until, workers }, Event::Tick { now }) if now >= until => {
            *state = State::NotStarted { draining: workers };
            fx.push(Effect::DropPayload);
            fx.push(Effect::NotifyWaiters);
        }
        (State::Cancelled { deadline: Some(deadline), workers }, Event::Tick { now }) if now >= deadline => {
            *state = State::Done { timed_out_workers: workers };
            fx.push(Effect::NotifyWaiters);
        }
        (_, Event::Tick { .. }) => {}

        (State::Done { timed_out_workers: 0 }, Event::Reopen) => {
            *state = State::NotStarted { draining: 0 };
        }
        (_, Event::Reopen) => {}
    }
    fx
}

struct Inner<T> {
    state: State,
    payload: Option<T>,
}

/// Single owner for GPU warm-up state, parked payload, and tracked worker threads.
pub(crate) struct WarmSlot<T, C = SystemClock> {
    inner: Mutex<Inner<T>>,
    changed: Condvar,
    clock: C,
}

/// Handle to a [`WarmSlot`] that can be moved into spawned worker threads (`&'static WarmSlot`
/// in production for zero allocation, `Arc<WarmSlot>` in isolated unit tests).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) trait SlotHandle<T: Send + 'static, C: Clock + 'static>: Clone + Send + 'static {
    fn slot(&self) -> &WarmSlot<T, C>;
}

impl<T: Send + 'static, C: Clock + 'static> SlotHandle<T, C> for &'static WarmSlot<T, C> {
    fn slot(&self) -> &WarmSlot<T, C> {
        self
    }
}

impl<T: Send + 'static, C: Clock + 'static> SlotHandle<T, C> for Arc<WarmSlot<T, C>> {
    fn slot(&self) -> &WarmSlot<T, C> {
        self
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
struct WorkerGuard<H: SlotHandle<T, C>, T: Send + 'static, C: Clock + 'static> {
    handle: H,
    _marker: std::marker::PhantomData<fn() -> (T, C)>,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
impl<H: SlotHandle<T, C>, T: Send + 'static, C: Clock + 'static> WorkerGuard<H, T, C> {
    fn new(handle: H) -> Self {
        Self { handle, _marker: std::marker::PhantomData }
    }
}

impl<H: SlotHandle<T, C>, T: Send + 'static, C: Clock + 'static> Drop for WorkerGuard<H, T, C> {
    fn drop(&mut self) {
        self.handle.slot().worker_exited();
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
impl<T: Send + 'static, C: Clock + 'static> WarmSlot<T, C> {
    pub(crate) const fn new(clock: C) -> Self {
        Self {
            inner: Mutex::new(Inner { state: State::NotStarted { draining: 0 }, payload: None }),
            changed: Condvar::new(),
            clock,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<T>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.lock().state.is_stopped()
    }

    #[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
    pub(crate) fn has_ready(&self) -> bool {
        matches!(self.lock().state, State::Parked { .. })
    }

    #[cfg(test)]
    pub(crate) fn live_workers(&self) -> usize {
        self.lock().state.live_workers()
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> State {
        self.lock().state
    }

    #[cfg(test)]
    pub(crate) fn wait_until_stopped(&self) -> State {
        let mut inner = self.lock();
        while !inner.state.is_stopped() {
            inner = self.changed.wait(inner).unwrap_or_else(|e| e.into_inner());
        }
        inner.state
    }

    #[cfg(test)]
    pub(crate) fn wait_until_zero_workers(&self, timeout: Duration) -> usize {
        let deadline = self.clock.now() + timeout;
        let mut inner = self.lock();
        while inner.state.live_workers() > 0 {
            let now = self.clock.now();
            if now >= deadline {
                break;
            }
            inner = self
                .changed
                .wait_timeout(inner, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        inner.state.live_workers()
    }

    fn worker_exited(&self) {
        let mut inner = self.lock();
        let fx = step(&mut inner.state, Event::WorkerExited);
        let notify = fx.into_iter().any(|e| e == Effect::NotifyWaiters);
        #[cfg(test)]
        let notify = notify || inner.state.live_workers() == 0;
        if notify {
            self.changed.notify_all();
        }
    }

    /// Spawns a tracked auxiliary worker (`name`) unless warm-up is cancelled/done.
    pub(crate) fn spawn_attached<H: SlotHandle<T, C>>(
        handle: H,
        name: &str,
        work: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<bool> {
        let attached = {
            let mut inner = handle.slot().lock();
            step(&mut inner.state, Event::AttachWorker)
                .into_iter()
                .any(|e| e == Effect::WorkerAttached)
        };
        if !attached {
            return Ok(false);
        }
        let guard_handle = handle.clone();
        match std::thread::Builder::new().name(name.into()).spawn(move || {
            let _guard = WorkerGuard::new(guard_handle);
            work();
        }) {
            Ok(_) => Ok(true),
            Err(err) => {
                handle.slot().worker_exited();
                Err(err)
            }
        }
    }

    /// Starts a warm-up build (`make`, then optional `prepare`), or extends the linger deadline
    /// if one is already running or parked. No-op once cancelled/shut down.
    pub(crate) fn start_warm<H: SlotHandle<T, C>>(
        handle: H,
        linger: Duration,
        make: impl FnOnce() -> Option<T> + Send + 'static,
        prepare: Option<Box<dyn FnOnce(&T) + Send>>,
    ) {
        let spawn = {
            let slot = handle.slot();
            let until = slot.clock.now() + linger;
            let mut inner = slot.lock();
            step(&mut inner.state, Event::Start { until })
                .into_iter()
                .any(|e| e == Effect::SpawnWorker)
        };
        if !spawn {
            return;
        }
        let thread_handle = handle.clone();
        let spawned = std::thread::Builder::new().name("rune-gpu-warm".into()).spawn(move || {
            let _guard = WorkerGuard::new(thread_handle.clone());
            let slot = thread_handle.slot();
            if slot.is_stopped() {
                return;
            }
            let mut made = make();
            if let (Some(warmed), Some(prepare)) = (&made, prepare) {
                if !slot.is_stopped() {
                    prepare(warmed);
                }
            }
            let now = slot.clock.now();
            let mut inner = slot.lock();
            let fx = step(&mut inner.state, Event::BuildFinished { ok: made.is_some(), now });
            let mut drop_outside: Option<T> = None;
            let mut stored = false;
            let mut notify = false;
            for effect in fx {
                match effect {
                    Effect::StorePayload => {
                        if let Some(val) = made.take() {
                            drop_outside = inner.payload.replace(val);
                            stored = true;
                        }
                    }
                    Effect::DropPayload => {
                        drop_outside = made.take();
                    }
                    Effect::NotifyWaiters => notify = true,
                    _ => {}
                }
            }
            if notify {
                slot.changed.notify_all();
            }
            if !stored {
                drop(inner);
                drop(drop_outside);
                return;
            }
            if let Some(old) = drop_outside {
                drop(inner);
                drop(old);
                slot.wait_for_surface(slot.lock());
            } else {
                slot.wait_for_surface(inner);
            }
        });
        if spawned.is_err() {
            let slot = handle.slot();
            let now = slot.clock.now();
            let mut inner = slot.lock();
            let _ = step(&mut inner.state, Event::BuildFinished { ok: false, now });
            let _ = step(&mut inner.state, Event::WorkerExited);
            slot.changed.notify_all();
        }
    }

    /// Runs `start_warm` (if not already running/parked), waits for the device, draws `frame`
    /// offscreen while holding the slot in `Running`, re-parks the device, invokes `done`, and
    /// lingers until claimed or expired. If cancelled at any point before completion, skips
    /// `frame`, `done`, and linger wait, drops any local payload, and exits promptly.
    #[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
    pub(crate) fn warm_then<H: SlotHandle<T, C>>(
        handle: H,
        linger: Duration,
        make: impl FnOnce() -> Option<T> + Send + 'static,
        frame: impl FnOnce(T) -> Option<T> + Send + 'static,
        done: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<()> {
        let worker_handle = handle.clone();
        Self::spawn_attached(handle, "rune-gpu-window", move || {
            Self::start_warm(worker_handle.clone(), linger, make, None);
            let slot = worker_handle.slot();
            let mut inner = slot.lock();
            while matches!(inner.state, State::Running { .. }) {
                inner = slot.changed.wait(inner).unwrap_or_else(|e| e.into_inner());
            }
            if inner.state.is_stopped() {
                return;
            }
            let fx = step(&mut inner.state, Event::BeginWarmFrame);
            let mut taken: Option<T> = None;
            let mut notify = false;
            for effect in fx {
                match effect {
                    Effect::TakePayload => taken = inner.payload.take(),
                    Effect::NotifyWaiters => notify = true,
                    _ => {}
                }
            }
            if notify {
                slot.changed.notify_all();
            }
            drop(inner);

            if let Some(item) = taken {
                let mut updated = if slot.is_stopped() { None } else { frame(item) };
                let now = slot.clock.now();
                let mut inner = slot.lock();
                let fx = step(&mut inner.state, Event::FinishWarmFrame { ok: updated.is_some(), now });
                let mut drop_outside: Option<T> = None;
                let mut notify_waiters = false;
                let mut notify_done = false;
                for effect in fx {
                    match effect {
                        Effect::StorePayload => {
                            if let Some(val) = updated.take() {
                                drop_outside = inner.payload.replace(val);
                            }
                        }
                        Effect::DropPayload => {
                            drop_outside = updated.take();
                        }
                        Effect::NotifyWaiters => notify_waiters = true,
                        Effect::NotifyDone => notify_done = true,
                        _ => {}
                    }
                }
                if notify_waiters {
                    slot.changed.notify_all();
                }
                drop(inner);
                drop(drop_outside);
                if !notify_done {
                    return;
                }
                done();
                slot.wait_for_surface(slot.lock());
            } else if !slot.is_stopped() {
                done();
            }
        })?;
        Ok(())
    }

    /// Parks `payload` until `linger` elapses and spawns a tracked linger watcher thread.
    /// Drops `payload` immediately if the slot is busy, parked, cancelled, or done.
    pub(crate) fn park<H: SlotHandle<T, C>>(&self, handle: H, payload: T, linger: Duration) {
        let mut payload = Some(payload);
        let until = self.clock.now() + linger;
        let mut inner = self.lock();
        let fx = step(&mut inner.state, Event::Park { until });
        let mut spawn = false;
        let mut notify = false;
        let mut drop_outside: Option<T> = None;
        for effect in fx {
            match effect {
                Effect::StorePayload => {
                    if let Some(val) = payload.take() {
                        drop_outside = inner.payload.replace(val);
                    }
                }
                Effect::DropPayload => {
                    drop_outside = payload.take();
                }
                Effect::SpawnWorker => spawn = true,
                Effect::NotifyWaiters => notify = true,
                _ => {}
            }
        }
        if notify {
            self.changed.notify_all();
        }
        drop(inner);
        drop(drop_outside);
        if !spawn {
            return;
        }
        let thread_handle = handle.clone();
        let spawned = std::thread::Builder::new().name("rune-gpu-parked".into()).spawn(move || {
            let _guard = WorkerGuard::new(thread_handle.clone());
            let slot = thread_handle.slot();
            slot.wait_for_surface(slot.lock());
        });
        if spawned.is_err() {
            self.discard();
            self.worker_exited();
        }
    }

    /// Keeps a parked payload until `until` or until taken/discarded/cancelled, dropping any
    /// expired payload outside the lock.
    fn wait_for_surface(&self, mut inner: std::sync::MutexGuard<'_, Inner<T>>) {
        loop {
            let State::Parked { until, .. } = inner.state else { return };
            let now = self.clock.now();
            if now >= until {
                let fx = step(&mut inner.state, Event::Tick { now });
                let mut expired = None;
                let mut notify = false;
                for effect in fx {
                    match effect {
                        Effect::DropPayload => expired = inner.payload.take(),
                        Effect::NotifyWaiters => notify = true,
                        _ => {}
                    }
                }
                if notify {
                    self.changed.notify_all();
                }
                drop(inner);
                drop(expired);
                return;
            }
            let wait = until - now;
            inner = self.changed.wait_timeout(inner, wait).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Takes the warmed/parked payload, waiting while a build or warm-frame is in flight.
    /// Returns `None` immediately when idle, cancelled, or shut down.
    pub(crate) fn take(&self) -> Option<T> {
        let mut inner = self.lock();
        while matches!(inner.state, State::Running { .. }) {
            inner = self.changed.wait(inner).unwrap_or_else(|e| e.into_inner());
        }
        let fx = step(&mut inner.state, Event::Take);
        let mut taken = None;
        let mut notify = false;
        for effect in fx {
            match effect {
                Effect::TakePayload => taken = inner.payload.take(),
                Effect::NotifyWaiters => notify = true,
                _ => {}
            }
        }
        if notify {
            self.changed.notify_all();
        }
        taken
    }

    /// Non-blocking discard (used at launch and in headless runs): drops any parked payload
    /// outside the lock and expires any in-flight linger deadline without waiting.
    pub(crate) fn discard(&self) {
        let now = self.clock.now();
        let mut inner = self.lock();
        let fx = step(&mut inner.state, Event::Discard { now });
        let mut dropped = None;
        let mut notify = false;
        for effect in fx {
            match effect {
                Effect::DropPayload => dropped = inner.payload.take(),
                Effect::NotifyWaiters => notify = true,
                _ => {}
            }
        }
        if notify {
            self.changed.notify_all();
        }
        drop(inner);
        drop(dropped);
    }

    /// Non-blocking early cancellation as soon as `Windows::exiting` starts: drops any parked
    /// payload outside the lock, wakes idle linger/window waiters, and tells in-flight builders
    /// to skip follow-up shader/frame/done work while the event loop tears down.
    pub(crate) fn cancel(&self) {
        let mut inner = self.lock();
        let fx = step(&mut inner.state, Event::Cancel);
        let mut dropped = None;
        let mut notify = false;
        for effect in fx {
            match effect {
                Effect::DropPayload => dropped = inner.payload.take(),
                Effect::NotifyWaiters => notify = true,
                _ => {}
            }
        }
        if notify {
            self.changed.notify_all();
        }
        drop(inner);
        drop(dropped);
    }

    /// Cancels all warm-up work and waits up to `timeout` for every tracked worker thread to
    /// exit. Returns [`ShutdownOutcome::Drained`] once zero workers remain, or
    /// [`ShutdownOutcome::TimedOut`] if a worker is still blocked past `timeout`.
    pub(crate) fn shutdown(&self, timeout: Duration) -> ShutdownOutcome {
        let deadline = self.clock.now() + timeout;
        let mut inner = self.lock();
        let fx = step(&mut inner.state, Event::Shutdown { deadline });
        let mut dropped = None;
        let mut notify = false;
        for effect in fx {
            match effect {
                Effect::DropPayload => dropped = inner.payload.take(),
                Effect::NotifyWaiters => notify = true,
                _ => {}
            }
        }
        if notify {
            self.changed.notify_all();
        }
        if dropped.is_some() {
            // Drop the parked GPU outside the lock before waiting for workers.
            drop(inner);
            drop(dropped);
            inner = self.lock();
        }
        loop {
            match inner.state {
                State::Done { timed_out_workers: 0 } => return ShutdownOutcome::Drained,
                State::Done { timed_out_workers } => {
                    return ShutdownOutcome::TimedOut { remaining: timed_out_workers };
                }
                State::Cancelled { deadline: Some(deadline), .. } => {
                    let now = self.clock.now();
                    if now >= deadline {
                        let _ = step(&mut inner.state, Event::Tick { now });
                        self.changed.notify_all();
                        continue;
                    }
                    let wait = deadline - now;
                    inner = self
                        .changed
                        .wait_timeout(inner, wait)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
                _ => return ShutdownOutcome::Drained,
            }
        }
    }

    /// Re-opens a cleanly drained slot for a subsequent event loop run in the same process.
    pub(crate) fn reopen(&self) {
        let mut inner = self.lock();
        let _ = step(&mut inner.state, Event::Reopen);
    }
}
