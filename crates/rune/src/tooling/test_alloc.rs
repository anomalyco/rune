//! The lib test binary's allocator: the system one, counting this thread's allocations and
//! its live and peak bytes, so a test can assert what a call allocates.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// Outcome of watching a live heap buffer across `dealloc` or `realloc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReleaseCheck {
    NotReleased,
    Zeroed,
    Dirty,
}

struct Counting;
thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
    static WATCH_PTR: Cell<usize> = const { Cell::new(0) };
    static WATCH_LEN: Cell<usize> = const { Cell::new(0) };
    static WATCH_RESULT: Cell<ReleaseCheck> = const { Cell::new(ReleaseCheck::NotReleased) };
}
fn track(allocs: usize, bytes: isize) {
    ALLOCS.try_with(|c| c.set(c.get() + allocs)).ok();
    LIVE.try_with(|live| {
        live.set(live.get() + bytes);
        PEAK.try_with(|peak| peak.set(peak.get().max(live.get()))).ok();
    }).ok();
}
/// Checks whether `p` is the watched buffer on this thread and records if its watched prefix is all
/// zero before the allocation is freed or reallocated.
///
/// SAFETY: `p` must be live and initialized for at least `WATCH_LEN.min(l.size())` bytes.
unsafe fn check_watch(p: *mut u8, l: Layout) {
    WATCH_PTR.try_with(|w| {
        if w.get() == 0 || w.get() != p as usize { return; }
        w.set(0);
        let len = WATCH_LEN.try_with(Cell::get).unwrap_or(0).min(l.size());
        // SAFETY: caller upholds `p..p+len` is live and initialized before dealloc/realloc.
        let zeroed = unsafe { std::slice::from_raw_parts(p, len) }.iter().all(|&b| b == 0);
        WATCH_RESULT.try_with(|r| r.set(if zeroed { ReleaseCheck::Zeroed } else { ReleaseCheck::Dirty })).ok();
    }).ok();
}
// SAFETY: forwards to the system allocator, counting calls and bytes on this thread.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        track(1, l.size() as isize);
        // SAFETY: caller upholds `GlobalAlloc::alloc` preconditions (`l` has non-zero size).
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: `p` is live for `l.size()` bytes until `System.dealloc` runs.
        unsafe { check_watch(p, l); }
        track(0, -(l.size() as isize));
        // SAFETY: caller upholds `GlobalAlloc::dealloc` preconditions (`p` came from `alloc` with `l`).
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        // SAFETY: `p` is live for `l.size()` bytes until `System.realloc` runs.
        unsafe { check_watch(p, l); }
        track(1, n as isize - l.size() as isize);
        // SAFETY: caller upholds `GlobalAlloc::realloc` preconditions (`p`/`l` live, `n` non-zero).
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static COUNTING: Counting = Counting;

/// Allocations made on this thread so far.
pub(crate) fn allocs() -> usize { ALLOCS.with(Cell::get) }

/// Watches a live heap allocation (`ptr..ptr+len`, all initialized bytes) on this thread while `f`
/// runs, returning whether those bytes were zero immediately before `dealloc` or `realloc`.
pub(crate) fn watch_release<T>(ptr: *const u8, len: usize, f: impl FnOnce() -> T) -> (T, ReleaseCheck) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            WATCH_PTR.with(|w| w.set(0));
            WATCH_LEN.with(|l| l.set(0));
        }
    }
    WATCH_PTR.with(|w| w.set(ptr as usize));
    WATCH_LEN.with(|l| l.set(len));
    WATCH_RESULT.with(|r| r.set(ReleaseCheck::NotReleased));
    let _reset = Reset;
    let out = f();
    (out, WATCH_RESULT.with(Cell::get))
}

/// Runs `f`, returning its result and the most bytes it held allocated at once on this thread
/// beyond what was live when it started.
#[allow(dead_code)]
pub(crate) fn peak<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let start = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(start));
    let out = f();
    (out, (PEAK.with(Cell::get) - start).max(0) as usize)
}
