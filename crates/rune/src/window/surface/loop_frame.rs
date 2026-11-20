//! A frame only a loop asked for: a spinner's next dot, a shimmer's next step. The runtime
//! samples the loops into the instances they drew and nothing else runs (`Runtime::loop_step`:
//! no poll, view, layout or paint), and the window redraws and presents only where they draw
//! (`Renderer::arm_loop`). Anything that reached the window since its last frame (input, a
//! wake, a request, an assistive client's action) makes the next one an ordinary frame, so
//! nothing waits behind a loop.

#[derive(Default)]
pub(in crate::window) struct LoopFrame {
    touched: bool,
}

impl LoopFrame {
    /// Something other than the window's own frames reached it.
    pub(in crate::window) fn touched(&mut self) { self.touched = true; }

    /// A frame begins: whether only its loops asked for it since the last. Counts afresh.
    pub(in crate::window) fn only_loops(&mut self) -> bool { !std::mem::replace(&mut self.touched, false) }
}

/// frame-profile, `RUNE_MISS_LOOP_FRAMES=<n>`: this loop-only frame is the nth since the last
/// forced miss, and acts as if its drawable never came (a diagnostic).
#[cfg(feature = "frame-profile")]
pub(in crate::window) fn forced_miss() -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    static EVERY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    static SEEN: AtomicU64 = AtomicU64::new(0);
    let every = *EVERY.get_or_init(|| rune_core::env::var("RUNE_MISS_LOOP_FRAMES").and_then(|v| v.parse().ok()).unwrap_or(0));
    every > 0 && SEEN.fetch_add(1, Ordering::Relaxed) % every == every - 1
}
