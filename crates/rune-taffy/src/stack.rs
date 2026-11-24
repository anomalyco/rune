//! Stack segment continuation for deep recursive container layout.

use std::sync::atomic::{AtomicU64, Ordering};

/// Minimum remaining stack bytes before continuing container layout on a new stack segment.
///
/// Sized with ~3× headroom above the largest measured leaf call chain under
/// `LayoutStoreView::compute_child_layout_inner` on x86_64 (~5.3 KiB Taffy container frame +
/// ~9 KiB Parley shaping/line-breaking + 16 KiB `skrifa` outline scratch + OS/unwind slack ≈
/// 44 KiB; `8 KiB` of the margin also absorbs the `16 KiB` fiber stack guarantee on Windows),
/// while leaving ~720 KiB on a 1 MiB thread stack so ordinary trees up to ~50 nested list levels
/// (~150 flex nodes) allocate zero segments.
#[cfg_attr(not(any(windows, target_os = "macos", target_os = "linux")), allow(dead_code))]
pub(crate) const STACK_RED_ZONE: usize = 128 * 1024;

/// Size argument passed to `stacker::grow` when remaining stack drops below [`STACK_RED_ZONE`].
///
/// On macOS and Linux, `stacker` maps `1 MiB` demand-paged with guard pages at both ends. On
/// Windows, `CreateFiber` commits `1 MiB` up front and reserves the executable's default stack
/// size (`/STACK`).
#[cfg_attr(not(any(windows, target_os = "macos", target_os = "linux")), allow(dead_code))]
pub(crate) const STACK_SEGMENT_SIZE: usize = 1024 * 1024;

static STACK_GROW_COUNT: AtomicU64 = AtomicU64::new(0);

/// Total number of stack segment continuations allocated by `rune-taffy` in this process,
/// exposed for launch/frame diagnostics and depth probes.
#[inline]
pub fn stack_grow_count() -> u64 {
    STACK_GROW_COUNT.load(Ordering::Relaxed)
}

#[cfg(windows)]
#[inline]
#[allow(unsafe_code)]
fn reserve_fiber_stack_guarantee() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadStackGuarantee(StackSizeInBytes: *mut u32) -> i32;
    }
    let mut bytes = 16 * 1024u32;
    // SAFETY: `bytes` is a live writable `u32` on the newly allocated fiber's stack.
    unsafe {
        let _ = SetThreadStackGuarantee(&raw mut bytes);
    }
}

/// Runs `f` on the current stack when at least `STACK_RED_ZONE` bytes remain, or continues on
/// a new segment on Windows, macOS, and Linux.
///
/// Every other target (`wasm32`, `android`, `ios`, `tvos`, `watchos`, `visionos`, etc.) runs `f`
/// directly rather than hitting `stacker`'s fallback path, which cannot query stack bounds and
/// would allocate a segment on every uncached container layout.
#[inline(always)]
pub fn maybe_grow<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    {
        if stacker::remaining_stack().is_none_or(|rem| rem >= STACK_RED_ZONE) {
            f()
        } else {
            STACK_GROW_COUNT.fetch_add(1, Ordering::Relaxed);
            stacker::grow(STACK_SEGMENT_SIZE, || {
                #[cfg(windows)]
                reserve_fiber_stack_guarantee();
                f()
            })
        }
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        f()
    }
}
