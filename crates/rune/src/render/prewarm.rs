//! `prewarm_gpu`'s thread and the slot a window takes its result from.

use super::{Gpu, Renderer, warm_machine::ShutdownOutcome, warmup::WarmRenderer};
use std::sync::{Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// A Mac window's target format (`surface_config`), which blends in sRGB space unless the
/// app turns that off, so the window renders with the pipeline compiled here.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

struct Prewarmed(Gpu, WarmRenderer);

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Running(JoinHandle<Option<Prewarmed>>),
    Done,
}

pub(super) struct Slot {
    state: Mutex<State>,
    active: Mutex<usize>,
    changed: Condvar,
}

impl Slot {
    pub(super) const fn new() -> Self {
        Self { state: Mutex::new(State::Idle), active: Mutex::new(0), changed: Condvar::new() }
    }

    pub(super) fn start(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*state, State::Idle) { return; }
        let is_global = std::ptr::eq(self, &GLOBAL);
        if is_global {
            *self.active.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        }
        let thread = std::thread::Builder::new().name("rune-gpu-prewarm".into()).spawn(move || {
            struct ActiveGuard(bool);
            impl Drop for ActiveGuard {
                fn drop(&mut self) {
                    if self.0 {
                        let mut active = GLOBAL.active.lock().unwrap_or_else(|e| e.into_inner());
                        *active = active.saturating_sub(1);
                        if *active == 0 {
                            GLOBAL.changed.notify_all();
                        }
                    }
                }
            }
            let _guard = ActiveGuard(is_global);
            objc2::rc::autoreleasepool(|_| {
                super::gpu_events::starting();
                let gpu = Gpu::try_for_display(Gpu::instance(), wgpu::Features::empty()).ok()?;
                #[cfg(test)]
                let loads_before = super::metallib::thread_loads();
                let renderer = Renderer::try_new_warm(&gpu.device, FORMAT).ok()?;
                #[cfg(test)]
                if super::metallib::metallib_enabled() {
                    assert_eq!(super::metallib::thread_loads(), loads_before + 1, "prewarm_gpu loads the precompiled metallib");
                }
                Some(Prewarmed(gpu, renderer))
            })
        });
        *state = match thread {
            Ok(thread) => State::Running(thread),
            Err(_) => {
                if is_global {
                    let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
                    *active = active.saturating_sub(1);
                    if *active == 0 { self.changed.notify_all(); }
                }
                State::Done
            }
        };
    }

    pub(super) fn take(&self) -> Option<(Gpu, Renderer)> {
        let state = std::mem::replace(&mut *self.state.lock().unwrap_or_else(|e| e.into_inner()), State::Done);
        let State::Running(thread) = state else { return None };
        thread.join().ok().flatten().map(|Prewarmed(gpu, renderer)| (gpu, Renderer::from_warm(renderer)))
    }

    /// Starts a new thread if one is working; that one drops its result when it finishes.
    pub(super) fn restart(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*state, State::Running(_)) { return; }
        *state = State::Idle;
        drop(state);
        self.start();
    }

    /// The thread, if still working, drops its result when it finishes.
    pub(super) fn discard(&self) {
        let state = std::mem::replace(&mut *self.state.lock().unwrap_or_else(|e| e.into_inner()), State::Done);
        drop(state);
    }

    /// Discards any unclaimed prewarm and waits up to `timeout` for in-flight prewarm threads
    /// to exit before process teardown.
    pub(super) fn shutdown(&self, timeout: Duration) -> ShutdownOutcome {
        self.discard();
        let deadline = crate::time::now() + timeout;
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        while *active > 0 {
            let now = crate::time::now();
            if now >= deadline {
                return ShutdownOutcome::TimedOut { remaining: *active };
            }
            active = self.changed.wait_timeout(active, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        }
        ShutdownOutcome::Drained
    }

    pub(super) fn reopen(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*state, State::Done) && *self.active.lock().unwrap_or_else(|e| e.into_inner()) == 0 {
            *state = State::Idle;
        }
    }
}

static GLOBAL: Slot = Slot::new();

pub(super) fn start() { GLOBAL.start(); }
pub(super) fn take() -> Option<(Gpu, Renderer)> { GLOBAL.take() }
pub(super) fn restart() { GLOBAL.restart(); }
#[cfg_attr(test, allow(dead_code))]
pub(super) fn discard() { GLOBAL.discard(); }
pub(super) fn cancel() { GLOBAL.discard(); }
pub(super) fn shutdown(timeout: Duration) -> ShutdownOutcome { GLOBAL.shutdown(timeout) }
pub(super) fn reopen() { GLOBAL.reopen(); }
