//! Guarded Linux/glibc heap trimming after a window or overlay finishes resting.
//!
//! When a window or overlay settles at rest (after [`REST_AFTER`](super::rest::REST_AFTER) or
//! going out of sight), Rune frees off-screen text layouts, glyph runs, unused images, dormant
//! scene slots, and GPU caches. On macOS (`libmalloc`), Windows (`HeapAlloc`), and Firefox's
//! `mozjemalloc`, freed pages return to the OS automatically. On Linux with glibc, fragmented
//! free chunks in arenas stay resident in `VmRSS` until `malloc_trim(0)` runs.
//!
//! This module calls `malloc_trim(0)` once off the UI thread on Rune's shared task pool after
//! rest releases finish, guarded by:
//! - target gate: `#[cfg(all(target_os = "linux", target_env = "gnu"))]` (compiles to nothing
//!   on macOS, Windows, musl, Android, iOS, and `wasm32`);
//! - runtime allocator probe (`OnceLock`): resolves `mallinfo2` (falling back to `mallinfo` for
//!   glibc < 2.33) and `malloc_trim` via `dlsym(RTLD_DEFAULT, ...)`, allocates a 32 KiB block
//!   through Rust's global allocator, and verifies glibc's `uordblks + hblkhd` grows and drops by
//!   at least 32 KiB — automatically skipping custom `#[global_allocator]`s (mimalloc, jemalloc)
//!   and `LD_PRELOAD` allocators;
//! - explicit state machine (`TrimState`): coalesces concurrent multi-window/overlay rests and
//!   enforces a 30 s cooldown (`TRIM_MIN_GAP`), running once per rest and never on a periodic idle
//!   timer;
//! - opt-outs: [`App::trim_heap_at_rest(false)`](crate::App::trim_heap_at_rest) and
//!   `RUNE_MALLOC_TRIM=0`.

#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
use std::alloc::Layout;
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
use std::time::Duration;

#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
use crate::machine::Effects;
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
use crate::time::Instant;

/// Minimum cooldown between completed `malloc_trim(0)` runs across all windows and overlays.
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
pub(crate) const TRIM_MIN_GAP: Duration = Duration::from_secs(30);

/// Probe block size (32 KiB): above glibc's 1,032 B tcache limit and below its 128 KiB
/// `MMAP_THRESHOLD`, so a glibc-backed Rust global allocator always reflects it in
/// `uordblks + hblkhd`.
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
pub(super) const PROBE_SIZE: usize = 32 * 1024;
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
const PROBE_MAX_DELTA: usize = 1 << 30;
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
const PROBE_ATTEMPTS: usize = 2;

/// Process-wide enable flag (default `true`; cleared by `App::trim_heap_at_rest(false)`).
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
static TRIM_ENABLED: AtomicBool = AtomicBool::new(true);

/// Explicit state machine for the process-wide background heap trim.
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrimState {
    /// No trim in flight; `last_done` records when the most recent trim completed.
    Idle { last_done: Option<Instant> },
    /// A `malloc-trim` task is currently scheduled or executing on the shared task pool.
    Running,
}

#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
impl Default for TrimState {
    fn default() -> Self {
        Self::Idle { last_done: None }
    }
}

/// Inputs to [`step`].
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrimEvent {
    /// A window or overlay finished its rest releases at `now`.
    Rested { now: Instant },
    /// The background `malloc_trim(0)` task finished at `now`.
    Finished { now: Instant },
}

/// Side effect requested by [`step`].
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrimEffect {
    /// Spawn one background `malloc_trim(0)` task on the shared task pool.
    Trim,
}

/// Pure transition for [`TrimState`]:
/// - first rest trims immediately;
/// - concurrent rests while `Running` coalesce into the in-flight trim;
/// - subsequent rests within [`TRIM_MIN_GAP`] (30 s) of `last_done` skip;
/// - once `TRIM_MIN_GAP` has elapsed, the next rest trims once.
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
pub(crate) fn step(state: &mut TrimState, event: TrimEvent) -> Effects<TrimEffect> {
    match (*state, event) {
        (TrimState::Idle { last_done: None }, TrimEvent::Rested { .. }) => {
            *state = TrimState::Running;
            Effects::one(TrimEffect::Trim)
        }
        (TrimState::Idle { last_done: Some(prev) }, TrimEvent::Rested { now })
            if now.saturating_duration_since(prev) >= TRIM_MIN_GAP =>
        {
            *state = TrimState::Running;
            Effects::one(TrimEffect::Trim)
        }
        (TrimState::Running, TrimEvent::Finished { now }) => {
            *state = TrimState::Idle { last_done: Some(now) };
            Effects::none()
        }
        (TrimState::Idle { .. }, TrimEvent::Rested { .. })
        | (TrimState::Running, TrimEvent::Rested { .. })
        | (TrimState::Idle { .. }, TrimEvent::Finished { .. }) => Effects::none(),
    }
}

/// Sets the process-wide opt-out flag behind [`App::trim_heap_at_rest`](crate::App::trim_heap_at_rest).
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
pub(crate) fn set_trim_enabled(enabled: bool) {
    TRIM_ENABLED.store(enabled, Ordering::Relaxed);
}

/// No-op off Linux/glibc in non-test builds.
#[cfg(not(any(test, all(target_os = "linux", target_env = "gnu"))))]
#[inline]
pub(crate) fn set_trim_enabled(_enabled: bool) {}

/// Parses `RUNE_MALLOC_TRIM`: `"0"`, `"false"`, `"off"`, or `"no"` (case-insensitive)
/// disables trimming; unset, empty, or other values leave it allowed.
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
pub(super) fn env_allows_trim(value: Option<&str>) -> bool {
    let Some(raw) = value else { return true };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return true;
    }
    !matches!(trimmed.to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no")
}

/// Whether trimming is currently allowed by both the builder flag and `RUNE_MALLOC_TRIM`.
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
pub(crate) fn trim_enabled() -> bool {
    TRIM_ENABLED.load(Ordering::Relaxed)
        && env_allows_trim(crate::env::var("RUNE_MALLOC_TRIM").as_deref())
}

/// Core allocator probe shared by the Linux/glibc runtime check and cross-platform unit tests.
///
/// Allocates a 32 KiB block via `alloc`, touches its first and last bytes with volatile writes,
/// and checks that `sample_used` (masked with `mask`) grows by at least 32 KiB and drops back
/// by at least 32 KiB after `dealloc`. Runs up to two attempts to tolerate a concurrent C free.
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
#[allow(unsafe_code)]
pub(super) fn probe_glibc_used(
    mask: usize,
    sample_used: impl Fn() -> usize,
    alloc: impl Fn(Layout) -> *mut u8,
    dealloc: impl Fn(*mut u8, Layout),
) -> bool {
    let Ok(layout) = Layout::from_size_align(PROBE_SIZE, 8) else {
        return false;
    };
    for _ in 0..PROBE_ATTEMPTS {
        let before = sample_used();
        let ptr = alloc(layout);
        if ptr.is_null() {
            continue;
        }
        // SAFETY: `ptr` is a non-null `PROBE_SIZE`-byte allocation from `alloc(layout)`.
        unsafe {
            std::ptr::write_volatile(ptr, 0xA5);
            std::ptr::write_volatile(ptr.add(PROBE_SIZE - 1), 0x5A);
        }
        std::hint::black_box(ptr);
        let during = sample_used();
        dealloc(ptr, layout);
        let after = sample_used();
        let grew = during.wrapping_sub(before) & mask;
        let shrank = during.wrapping_sub(after) & mask;
        if grew >= PROBE_SIZE
            && grew < PROBE_MAX_DELTA
            && shrank >= PROBE_SIZE
            && shrank < PROBE_MAX_DELTA
        {
            return true;
        }
    }
    false
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
mod glibc {
    use super::{TrimEffect, TrimEvent, TrimState, probe_glibc_used, step, trim_enabled};
    use std::ffi::{CStr, c_int};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
    use std::time::Duration;

    /// `struct mallinfo2` from glibc `<malloc.h>` (glibc >= 2.33).
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Mallinfo2 {
        arena: usize,
        ordblks: usize,
        smblks: usize,
        hblks: usize,
        hblkhd: usize,
        usmblks: usize,
        fsmblks: usize,
        uordblks: usize,
        fordblks: usize,
        keepcost: usize,
    }

    /// Legacy `struct mallinfo` from glibc `<malloc.h>` (glibc < 2.33 fallback).
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Mallinfo {
        arena: c_int,
        ordblks: c_int,
        smblks: c_int,
        hblks: c_int,
        hblkhd: c_int,
        usmblks: c_int,
        fsmblks: c_int,
        uordblks: c_int,
        fordblks: c_int,
        keepcost: c_int,
    }

    type Mallinfo2Fn = unsafe extern "C" fn() -> Mallinfo2;
    type MallinfoFn = unsafe extern "C" fn() -> Mallinfo;
    type MallocTrimFn = unsafe extern "C" fn(pad: libc::size_t) -> c_int;

    struct Coordinator {
        state: Mutex<TrimState>,
        done: Condvar,
    }

    static GLIBC_TRIM: OnceLock<Option<MallocTrimFn>> = OnceLock::new();
    static COORD: OnceLock<Coordinator> = OnceLock::new();
    static TRIM_CALLS: AtomicU64 = AtomicU64::new(0);
    static LAST_TRIM_NANOS: AtomicU64 = AtomicU64::new(0);

    fn coord() -> &'static Coordinator {
        COORD.get_or_init(|| Coordinator {
            state: Mutex::new(TrimState::default()),
            done: Condvar::new(),
        })
    }

    unsafe fn dlsym_default<F: Copy>(symbol: &CStr) -> Option<F> {
        debug_assert_eq!(size_of::<F>(), size_of::<*mut libc::c_void>());
        // SAFETY: `RTLD_DEFAULT` searches the default loaded symbol scope; `symbol` is
        // NUL-terminated.
        let ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr()) };
        if ptr.is_null() {
            return None;
        }
        // SAFETY: caller guarantees `F` matches the C ABI of `symbol` and `ptr` is non-null.
        Some(unsafe { std::mem::transmute_copy(&ptr) })
    }

    fn resolve_glibc_trim() -> Option<MallocTrimFn> {
        // SAFETY: `malloc_trim`, `mallinfo2`, and `mallinfo` match glibc `<malloc.h>` ABI.
        let trim_fn: MallocTrimFn = unsafe { dlsym_default(c"malloc_trim")? };
        let probed = if let Some(info2) = unsafe { dlsym_default::<Mallinfo2Fn>(c"mallinfo2") } {
            probe_glibc_used(
                usize::MAX,
                || {
                    // SAFETY: `mallinfo2` takes no arguments and returns by value.
                    let info = unsafe { info2() };
                    info.uordblks.wrapping_add(info.hblkhd)
                },
                |layout| {
                    // SAFETY: `layout` has non-zero size (32 KiB) and valid alignment (8).
                    unsafe { std::alloc::alloc(layout) }
                },
                |ptr, layout| {
                    // SAFETY: `ptr` was returned by `std::alloc::alloc(layout)`.
                    unsafe { std::alloc::dealloc(ptr, layout) };
                },
            )
        } else if let Some(info1) = unsafe { dlsym_default::<MallinfoFn>(c"mallinfo") } {
            probe_glibc_used(
                u32::MAX as usize,
                || {
                    // SAFETY: `mallinfo` takes no arguments and returns by value.
                    let info = unsafe { info1() };
                    (info.uordblks as u32).wrapping_add(info.hblkhd as u32) as usize
                },
                |layout| {
                    // SAFETY: `layout` has non-zero size (32 KiB) and valid alignment (8).
                    unsafe { std::alloc::alloc(layout) }
                },
                |ptr, layout| {
                    // SAFETY: `ptr` was returned by `std::alloc::alloc(layout)`.
                    unsafe { std::alloc::dealloc(ptr, layout) };
                },
            )
        } else {
            false
        };
        probed.then_some(trim_fn)
    }

    pub(super) fn uses_glibc_allocator() -> bool {
        GLIBC_TRIM.get_or_init(resolve_glibc_trim).is_some()
    }

    pub(super) fn maybe_trim_heap() {
        if !trim_enabled() {
            return;
        }
        let Some(trim_fn) = *GLIBC_TRIM.get_or_init(resolve_glibc_trim) else {
            return;
        };
        let c = coord();
        let should_spawn = {
            let mut g = c.state.lock().unwrap_or_else(PoisonError::into_inner);
            step(&mut g, TrimEvent::Rested { now: crate::time::now() })
                .into_iter()
                .any(|e| matches!(e, TrimEffect::Trim))
        };
        if !should_spawn {
            return;
        }
        let spawned = rune_core::task::spawn("malloc-trim", move || {
            let t0 = crate::time::now();
            // SAFETY: glibc `malloc_trim(0)` takes no pointers and returns free arena pages to
            // the OS without invalidating any live allocations.
            let _ = unsafe { trim_fn(0) };
            let elapsed_ns = t0.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
            LAST_TRIM_NANOS.store(elapsed_ns, Ordering::Relaxed);
            TRIM_CALLS.fetch_add(1, Ordering::Relaxed);
            let c = coord();
            {
                let mut g = c.state.lock().unwrap_or_else(PoisonError::into_inner);
                let _ = step(&mut g, TrimEvent::Finished { now: crate::time::now() });
            }
            c.done.notify_all();
        });
        if spawned.is_err() {
            let mut g = c.state.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = step(&mut g, TrimEvent::Finished { now: crate::time::now() });
            c.done.notify_all();
        }
    }

    pub(super) fn wait_idle() {
        let c = coord();
        let mut g = c.state.lock().unwrap_or_else(PoisonError::into_inner);
        while matches!(*g, TrimState::Running) {
            g = c.done.wait(g).unwrap_or_else(PoisonError::into_inner);
        }
    }

    pub(super) fn trim_calls() -> u64 {
        TRIM_CALLS.load(Ordering::Relaxed)
    }

    pub(super) fn last_trim_duration() -> Duration {
        Duration::from_nanos(LAST_TRIM_NANOS.load(Ordering::Relaxed))
    }

    pub(super) fn reset_cooldown_for_test() {
        let c = coord();
        let mut g = c.state.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(*g, TrimState::Idle { .. }) {
            *g = TrimState::Idle { last_done: None };
        }
    }
}

/// Triggers the guarded background glibc `malloc_trim(0)` once after a window or overlay rests.
/// Compiles to nothing on every target other than Linux with glibc.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn maybe_trim_heap() {
    glibc::maybe_trim_heap();
}

/// No-op off Linux/glibc (macOS, Windows, musl, Android, iOS).
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
#[inline]
pub(crate) fn maybe_trim_heap() {}

/// Returns `true` only on Linux/glibc when the process-wide Rust global allocator is backed by
/// glibc `malloc`.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn uses_glibc_allocator() -> bool {
    glibc::uses_glibc_allocator()
}

/// Always `false` off Linux/glibc.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
#[inline]
pub(crate) fn uses_glibc_allocator() -> bool {
    false
}

/// Hidden test hooks so integration tests in `rune` and `rune-kit` can exercise Rune's own
/// rest-trim path and verify guards without calling `malloc_trim` directly.
#[doc(hidden)]
pub mod test_support {
    use std::time::Duration;

    /// Invokes Rune's guarded rest-time heap trim entrypoint (same call as `Host::give_back`).
    pub fn on_rest() {
        super::maybe_trim_heap();
    }

    /// Blocks until any in-flight background `malloc-trim` task finishes.
    pub fn wait_idle() {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        super::glibc::wait_idle();
    }

    /// Returns whether the runtime allocator probe detected glibc's allocator.
    pub fn uses_glibc_allocator() -> bool {
        super::uses_glibc_allocator()
    }

    /// Number of completed `malloc_trim(0)` calls made by Rune in this process.
    pub fn trim_calls() -> u64 {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            super::glibc::trim_calls()
        }
        #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
        {
            0
        }
    }

    /// Duration of the most recent completed `malloc_trim(0)` call.
    pub fn last_trim_duration() -> Duration {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            super::glibc::last_trim_duration()
        }
        #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
        {
            Duration::ZERO
        }
    }

    /// Clears the 30 s cooldown when idle so multi-scenario tests can measure back-to-back trims.
    pub fn reset_cooldown() {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        super::glibc::reset_cooldown_for_test();
    }
}
