//! Per-display weak pool and shared-queue drain generation tracker for Wayland layer-shell
//! globals, kept cross-platform so its lifecycle, dead-entry eviction, pointer-reuse safety, and
//! peer-queue pending detection are tested on every host without a Wayland compositor.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// Weakly caches one shared value per foreign display pointer (`usize`) so every layer surface
/// and overlay on the same connection reuses one registry and one bound set of globals, and drops
/// them cleanly once the last holder goes away.
#[cfg_attr(
    not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
    allow(dead_code)
)]
pub(crate) struct DisplayPool<T> {
    entries: Mutex<HashMap<usize, Weak<T>>>,
}

#[cfg_attr(
    not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
    allow(dead_code)
)]
impl<T> DisplayPool<T> {
    pub(crate) fn new() -> Self {
        Self { entries: Mutex::new(HashMap::new()) }
    }

    /// Returns the live shared entry for `key` if one exists and passes `is_alive`, otherwise
    /// invokes `create` **without holding the pool lock** (so a blocking Wayland roundtrip never
    /// blocks other keys or re-entrant teardowns) and caches the result weakly.
    pub(crate) fn acquire(
        &self,
        key: usize,
        is_alive: impl Fn(&T) -> bool,
        create: impl FnOnce() -> Option<T>,
    ) -> Option<Arc<T>> {
        {
            let mut map = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(weak) = map.get(&key) {
                match weak.upgrade() {
                    Some(existing) if is_alive(&existing) => return Some(existing),
                    _ => {
                        map.remove(&key);
                    }
                }
            }
        }
        let created = Arc::new(create()?);
        let (chosen, unused) = {
            let mut map = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(existing) = map.get(&key).and_then(Weak::upgrade).filter(|e| is_alive(e)) {
                (existing, Some(created))
            } else {
                map.insert(key, Arc::downgrade(&created));
                (created, None)
            }
        };
        drop(unused);
        Some(chosen)
    }

    /// Removes `key` from the pool if and only if its stored weak entry still points to `ptr`,
    /// preventing a late-dropping evicted entry from removing a fresh replacement at the same
    /// display address.
    pub(crate) fn release(&self, key: usize, ptr: *const T) {
        let mut map = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if map.get(&key).is_some_and(|w| std::ptr::eq(w.as_ptr(), ptr)) {
            map.remove(&key);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
    }
}

/// Tracks inner-queue drains on a shared `wayland_client::Connection`.
///
/// When multiple `EventQueue`s share one foreign `Backend`, any queue's `dispatch_inner_queue`
/// moves buffered libwayland events for *every* peer queue into those peers' Rust buffers, leaving
/// libwayland's C queue empty so `prepare_read()` on a peer returns `Some`. Bumping a monotonic
/// generation whenever `dispatch_inner_queue` moves `> 0` events ensures each peer still reports
/// pending events until it dispatches its own queue.
#[cfg_attr(
    not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
    allow(dead_code)
)]
pub(crate) struct DrainTracker {
    inner_drain_gen: AtomicU64,
}

#[cfg_attr(
    not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
    allow(dead_code)
)]
impl DrainTracker {
    pub(crate) const fn new() -> Self {
        Self { inner_drain_gen: AtomicU64::new(0) }
    }

    /// Records that `dispatch_inner_queue` moved `moved` events from libwayland's C queue into
    /// Rust `EventQueue` buffers.
    pub(crate) fn note_inner_drain(&self, moved: usize) {
        if moved > 0 {
            self.inner_drain_gen.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Current inner-drain generation, saved by each queue after draining its own buffer.
    pub(crate) fn generation(&self) -> u64 {
        self.inner_drain_gen.load(Ordering::Acquire)
    }

    /// True when either libwayland's C queue still has unread events (`c_queue_pending`) or
    /// another queue drained the C queue since this queue last dispatched (`seen_gen != current`).
    pub(crate) fn has_pending(&self, seen_gen: u64, c_queue_pending: bool) -> bool {
        c_queue_pending || seen_gen != self.generation()
    }
}

#[cfg(test)]
#[path = "globals_pool_tests.rs"]
mod tests;
