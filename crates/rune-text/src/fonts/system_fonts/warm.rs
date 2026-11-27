//! Process-wide shared lazy cache with idempotent background prewarming, used by the fontconfig
//! backend so the system font collection is built at most once per process and can be warmed off
//! the UI thread without ever starting a duplicate build.

use std::sync::atomic::AtomicBool;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};

/// Process-wide lazily initialized `T` behind a `Mutex`, with opt-in background prewarming.
///
/// Concurrent callers of [`Self::with`] and [`Self::prewarm`] coalesce onto a single `init` call
/// through [`OnceLock::get_or_init`], so layout waiting on an in-flight warm-up never starts a
/// second build.
#[cfg_attr(not(any(all(unix, not(target_vendor = "apple"), not(target_os = "android")), test)), allow(dead_code))]
pub(super) struct WarmSlot<T> {
    value: OnceLock<Mutex<T>>,
    started: AtomicBool,
}

#[cfg_attr(not(any(all(unix, not(target_vendor = "apple"), not(target_os = "android")), test)), allow(dead_code))]
impl<T: Send + 'static> WarmSlot<T> {
    pub(super) const fn new() -> Self {
        Self {
            value: OnceLock::new(),
            started: AtomicBool::new(false),
        }
    }

    /// Runs `f` with mutable access to the shared value, initializing it once with `init` on
    /// first use (or waiting if another thread is already initializing it).
    pub(super) fn with<R>(&self, init: fn() -> T, f: impl FnOnce(&mut T) -> R) -> R {
        let mutex = self.value.get_or_init(|| Mutex::new(init()));
        let mut guard = mutex.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    /// Starts initializing the shared value on a background thread named `thread_name` when
    /// `enabled()` is true and warm-up has not already started or completed.
    ///
    /// Returns immediately without blocking the calling thread. Calling again after warm-up has
    /// started (or once the value is already initialized) does nothing.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn prewarm(&'static self, thread_name: &str, enabled: fn() -> bool, init: fn() -> T) {
        if !enabled() || self.value.get().is_some() {
            return;
        }
        if self.started.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                // Reset `started` if `enabled()` turned false before the worker ran or if `init`
                // panics, so a later call can still warm up once enabled.
                struct ResetOnExit<'a>(&'a AtomicBool, bool);
                impl Drop for ResetOnExit<'_> {
                    fn drop(&mut self) {
                        if self.1 {
                            self.0.store(false, Ordering::Release);
                        }
                    }
                }
                let mut reset = ResetOnExit(&self.started, true);
                if !enabled() {
                    return;
                }
                let _ = self.value.get_or_init(|| Mutex::new(init()));
                reset.1 = false;
            });
        if spawned.is_err() {
            self.started.store(false, Ordering::Release);
        }
    }

    #[cfg(test)]
    pub(super) fn is_initialized(&self) -> bool {
        self.value.get().is_some()
    }
}

// Compile-time check that fontique's Collection remains Send so WarmSlot<Collection> is always
// shareable across threads.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<parley::fontique::Collection>();
};

#[cfg(test)]
#[path = "warm_tests.rs"]
mod tests;
