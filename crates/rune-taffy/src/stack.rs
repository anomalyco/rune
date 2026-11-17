//! Demand-paged stack segment continuation for recursive layout and tree walks.

/// Minimum remaining stack bytes before continuing recursion on a new stack segment.
///
/// Sized with ~3× headroom above the largest measured leaf call chain under
/// `LayoutStoreView::compute_child_layout_inner` on x86_64 (~5.3 KiB Taffy container frame +
/// ~9 KiB Parley shaping/line-breaking + 16 KiB `skrifa` outline scratch + OS/unwind slack ≈
/// 44 KiB), while leaving ~720 KiB on a 1 MiB thread stack so ordinary trees up to ~50 nested
/// list levels (~150 flex nodes) allocate zero segments.
pub const STACK_RED_ZONE: usize = 128 * 1024;

/// Virtual size of each stack segment allocated when remaining stack drops below
/// [`STACK_RED_ZONE`]. Pages fault in lazily on Windows fibers, macOS, and Linux.
pub const STACK_SEGMENT_SIZE: usize = 1024 * 1024;

/// Runs `f` on the current stack when at least [`STACK_RED_ZONE`] bytes remain, or continues on
/// a new [`STACK_SEGMENT_SIZE`] segment on desktop targets.
///
/// On `wasm32` (whose browser execution call stack cannot be switched from Wasm) and `ios`
/// (where `stacker` lacks an OS stack-limit query and would otherwise allocate a segment on
/// every root call), runs `f` directly.
#[inline(always)]
pub fn maybe_grow<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "ios")))]
    {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT_SIZE, f)
    }
    #[cfg(any(target_arch = "wasm32", target_os = "ios"))]
    {
        f()
    }
}
