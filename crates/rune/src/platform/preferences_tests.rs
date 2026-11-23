use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn parses_kdeglobals_animation_duration_factor() {
    assert_eq!(parse_kdeglobals_reduced_motion("[KDE]\nAnimationDurationFactor=0\n"), Some(true));
    assert_eq!(parse_kdeglobals_reduced_motion("[KDE]\nAnimationDurationFactor = 0.0\n"), Some(true));
    assert_eq!(parse_kdeglobals_reduced_motion("[KDE]\nAnimationDurationFactor=1\n"), Some(false));
    assert_eq!(parse_kdeglobals_reduced_motion("[KDE]\nAnimationDurationFactor=0.5\n"), Some(false));
    assert_eq!(parse_kdeglobals_reduced_motion("[KDE]\nSingleClick=false\n"), None);
    assert_eq!(parse_kdeglobals_reduced_motion("[KDE]\nAnimationDurationFactor=bogus\n"), None);
}

#[test]
fn cached_fallback_reads_once_off_the_calling_thread() {
    static CACHE: CachedReducedMotion = CachedReducedMotion::new();
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static CALLER_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();
    static WORKER_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

    CALLER_THREAD.set(std::thread::current().id()).ok();
    fn read_once() -> bool {
        CALLS.fetch_add(1, Ordering::Relaxed);
        WORKER_THREAD.set(std::thread::current().id()).ok();
        true
    }

    // Repeated calls (simulating focus changes) reuse the cached value without re-reading.
    for _ in 0..8 {
        assert!(CACHE.get_or_init(read_once));
    }
    assert_eq!(CALLS.load(Ordering::Relaxed), 1);
    assert_ne!(WORKER_THREAD.get(), CALLER_THREAD.get(), "fallback must run off the calling UI thread");
}
