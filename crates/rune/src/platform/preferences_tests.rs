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

#[test]
fn clamps_double_click_milliseconds() {
    assert_eq!(clamp_double_click_ms(0), None);
    assert_eq!(clamp_double_click_ms(1), Some(MIN_DOUBLE_CLICK_MS));
    assert_eq!(clamp_double_click_ms(49), Some(MIN_DOUBLE_CLICK_MS));
    assert_eq!(clamp_double_click_ms(50), Some(50));
    assert_eq!(clamp_double_click_ms(400), Some(400));
    assert_eq!(clamp_double_click_ms(1200), Some(1200));
    assert_eq!(clamp_double_click_ms(5000), Some(5000));
    assert_eq!(clamp_double_click_ms(5001), Some(MAX_DOUBLE_CLICK_MS));
    assert_eq!(clamp_double_click_ms(u32::MAX), Some(MAX_DOUBLE_CLICK_MS));
}

#[test]
fn parses_kdeglobals_double_click_interval() {
    assert_eq!(parse_kdeglobals_double_click_ms("[KDE]\nDoubleClickInterval=400\n"), Some(400));
    assert_eq!(parse_kdeglobals_double_click_ms("\u{feff}[KDE]\nDoubleClickInterval = 1200\r\n"), Some(1200));
    assert_eq!(parse_kdeglobals_double_click_ms("[KDE]\nDoubleClickInterval=10\n"), Some(50));
    assert_eq!(parse_kdeglobals_double_click_ms("[KDE]\nDoubleClickInterval=10000\n"), Some(5000));
    assert_eq!(parse_kdeglobals_double_click_ms("[KDE]\nDoubleClickInterval=999999999999\n"), Some(5000));
    assert_eq!(parse_kdeglobals_double_click_ms("[KDE]\nDoubleClickInterval=0\n"), None);
    assert_eq!(parse_kdeglobals_double_click_ms("[KDE]\nDoubleClickInterval=-200\n"), None);
    assert_eq!(parse_kdeglobals_double_click_ms("[KDE]\nDoubleClickInterval=bogus\n"), None);
    assert_eq!(parse_kdeglobals_double_click_ms("[KDE]\nSingleClick=false\n"), None);
    assert_eq!(parse_kdeglobals_double_click_ms("[General]\nDoubleClickInterval=900\n[KDE]\nSingleClick=false\n"), None);
    assert_eq!(
        parse_kdeglobals_double_click_ms("[General]\nDoubleClickInterval=900\n[KDE]\n# DoubleClickInterval=800\n; DoubleClickInterval=700\nDoubleClickInterval=650\n"),
        Some(650),
    );
    assert_eq!(parse_kdeglobals_double_click_ms(""), None);
}

#[test]
fn double_click_fallback_prefers_kdeglobals_on_kde_and_gsettings_elsewhere() {
    let mut gsettings_called = false;
    assert_eq!(
        resolve_double_click_fallback(true, || Some(600), || { gsettings_called = true; Some(1200) }),
        Some(600),
    );
    assert!(!gsettings_called, "KDE fallback must not read GNOME GSettings");

    // On KDE with no kdeglobals key, stay unknown (0.5 s default) instead of reading GNOME GSettings.
    let mut gsettings_called = false;
    assert_eq!(
        resolve_double_click_fallback(true, || None, || { gsettings_called = true; Some(1200) }),
        None,
    );
    assert!(!gsettings_called);

    // Outside KDE, GSettings wins and skips kdeglobals when present.
    let mut kde_called = false;
    assert_eq!(
        resolve_double_click_fallback(false, || { kde_called = true; Some(600) }, || Some(1200)),
        Some(1200),
    );
    assert!(!kde_called);

    // Outside KDE, fall back to kdeglobals when GSettings is unavailable.
    assert_eq!(resolve_double_click_fallback(false, || Some(650), || None), Some(650));
    assert_eq!(resolve_double_click_fallback(false, || None, || None), None);
}

#[test]
fn cached_double_click_fallback_reads_once_off_the_calling_thread() {
    static CACHE: CachedDoubleClick = CachedDoubleClick::new();
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static CALLER_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();
    static WORKER_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

    CALLER_THREAD.set(std::thread::current().id()).ok();
    fn read_once() -> Option<u32> {
        CALLS.fetch_add(1, Ordering::Relaxed);
        WORKER_THREAD.set(std::thread::current().id()).ok();
        Some(1200)
    }

    for _ in 0..8 {
        assert_eq!(CACHE.get_or_init(read_once), 1.2);
    }
    assert_eq!(CALLS.load(Ordering::Relaxed), 1);
    assert_ne!(WORKER_THREAD.get(), CALLER_THREAD.get(), "fallback must run off the calling UI thread");
}

#[test]
fn cached_double_click_fallback_defaults_to_half_second_when_unknown() {
    static NONE_CACHE: CachedDoubleClick = CachedDoubleClick::new();
    static ZERO_CACHE: CachedDoubleClick = CachedDoubleClick::new();

    assert_eq!(NONE_CACHE.get_or_init(|| None), DEFAULT_DOUBLE_CLICK_SECS);
    assert_eq!(ZERO_CACHE.get_or_init(|| Some(0)), DEFAULT_DOUBLE_CLICK_SECS);
}
