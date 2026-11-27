use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

struct Gate {
    state: Mutex<(bool, bool)>,
    cv: Condvar,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self { state: Mutex::new((false, false)), cv: Condvar::new() })
    }

    fn enter_and_wait(&self) {
        let mut s = self.state.lock().unwrap();
        s.0 = true;
        self.cv.notify_all();
        while !s.1 {
            s = self.cv.wait(s).unwrap();
        }
    }

    fn wait_entered(&self) {
        let mut s = self.state.lock().unwrap();
        while !s.0 {
            s = self.cv.wait(s).unwrap();
        }
    }

    fn release(&self) {
        let mut s = self.state.lock().unwrap();
        s.1 = true;
        self.cv.notify_all();
    }
}

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

    // Trigger the one-shot worker and wait for it to finish via the cache's condvar so a
    // load-delayed thread start cannot race the 100 ms launch wait.
    let _ = CACHE.get_or_init(read_once);
    CACHE.wait_ready();

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

    let _ = CACHE.get_or_init(read_once);
    CACHE.wait_ready();

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

    let _ = NONE_CACHE.get_or_init(|| None);
    NONE_CACHE.wait_ready();
    let _ = ZERO_CACHE.get_or_init(|| Some(0));
    ZERO_CACHE.wait_ready();

    assert_eq!(NONE_CACHE.get_or_init(|| None), DEFAULT_DOUBLE_CLICK_SECS);
    assert_eq!(ZERO_CACHE.get_or_init(|| Some(0)), DEFAULT_DOUBLE_CLICK_SECS);
}

#[test]
fn slow_reduced_motion_fallback_returns_default_then_publishes_and_notifies_on_late_answer() {
    static CHANGED: CachedReducedMotion = CachedReducedMotion::new();
    let gate = Gate::new();
    let gate_worker = Arc::clone(&gate);
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_worker = Arc::clone(&calls);
    let notified = Arc::new(AtomicUsize::new(0));
    let notified_worker = Arc::clone(&notified);
    let (late_tx, late_rx) = mpsc::channel();

    // Zero wait forces the timeout path deterministically without sleeping.
    let initial = CHANGED.get_or_init_with(
        Duration::ZERO,
        move || {
            calls_worker.fetch_add(1, Ordering::SeqCst);
            gate_worker.enter_and_wait();
            true
        },
        move || {
            notified_worker.fetch_add(1, Ordering::SeqCst);
            late_tx.send(()).ok();
        },
    );
    assert!(!initial, "timed-out launch falls back to default false");
    gate.wait_entered();
    assert!(!CHANGED.get_or_init(|| panic!("must not respawn while pending")));
    assert_eq!(notified.load(Ordering::SeqCst), 0);

    gate.release();
    late_rx.recv().expect("late non-default reduced-motion answer must notify windows");
    CHANGED.wait_ready();
    assert!(CHANGED.late_changed());
    assert!(CHANGED.get_or_init(|| false), "subsequent reads must see the worker's real value");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(notified.load(Ordering::SeqCst), 1);

    // A slow read that matches the default (false) must not request a needless redraw.
    static SAME: CachedReducedMotion = CachedReducedMotion::new();
    let gate2 = Gate::new();
    let gate2_worker = Arc::clone(&gate2);
    let notified2 = Arc::new(AtomicUsize::new(0));
    let notified2_worker = Arc::clone(&notified2);
    assert!(!SAME.get_or_init_with(
        Duration::ZERO,
        move || {
            gate2_worker.enter_and_wait();
            false
        },
        move || {
            notified2_worker.fetch_add(1, Ordering::SeqCst);
        },
    ));
    gate2.wait_entered();
    gate2.release();
    SAME.wait_ready();
    assert!(!SAME.late_changed());
    assert!(!SAME.get_or_init(|| true));
    assert_eq!(notified2.load(Ordering::SeqCst), 0);
}

#[test]
fn slow_double_click_fallback_returns_default_then_publishes_and_notifies_on_late_answer() {
    static CHANGED: CachedDoubleClick = CachedDoubleClick::new();
    let gate = Gate::new();
    let gate_worker = Arc::clone(&gate);
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_worker = Arc::clone(&calls);
    let notified = Arc::new(AtomicUsize::new(0));
    let notified_worker = Arc::clone(&notified);
    let (late_tx, late_rx) = mpsc::channel();

    let initial = CHANGED.get_or_init_with(
        Duration::ZERO,
        move || {
            calls_worker.fetch_add(1, Ordering::SeqCst);
            gate_worker.enter_and_wait();
            Some(1200)
        },
        move || {
            notified_worker.fetch_add(1, Ordering::SeqCst);
            late_tx.send(()).ok();
        },
    );
    assert_eq!(initial, DEFAULT_DOUBLE_CLICK_SECS, "timed-out launch falls back to 0.5 s");
    gate.wait_entered();
    assert_eq!(CHANGED.get_or_init(|| panic!("must not respawn while pending")), DEFAULT_DOUBLE_CLICK_SECS);
    assert_eq!(notified.load(Ordering::SeqCst), 0);

    gate.release();
    late_rx.recv().expect("late non-default double-click answer must notify windows");
    CHANGED.wait_ready();
    assert!(CHANGED.late_changed());
    assert_eq!(CHANGED.get_or_init(|| Some(300)), 1.2, "subsequent reads must see the worker's real value");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(notified.load(Ordering::SeqCst), 1);

    // A slow read that resolves to the default 500 ms (or None) must not request a redraw.
    static SAME: CachedDoubleClick = CachedDoubleClick::new();
    let gate2 = Gate::new();
    let gate2_worker = Arc::clone(&gate2);
    let notified2 = Arc::new(AtomicUsize::new(0));
    let notified2_worker = Arc::clone(&notified2);
    assert_eq!(
        SAME.get_or_init_with(
            Duration::ZERO,
            move || {
                gate2_worker.enter_and_wait();
                Some(DEFAULT_DOUBLE_CLICK_MS)
            },
            move || {
                notified2_worker.fetch_add(1, Ordering::SeqCst);
            },
        ),
        DEFAULT_DOUBLE_CLICK_SECS,
    );
    gate2.wait_entered();
    gate2.release();
    SAME.wait_ready();
    assert!(!SAME.late_changed());
    assert_eq!(SAME.get_or_init(|| Some(900)), DEFAULT_DOUBLE_CLICK_SECS);
    assert_eq!(notified2.load(Ordering::SeqCst), 0);
}

#[test]
fn windows_enable_transparency_zero_means_reduced() {
    assert!(windows_enable_transparency_is_reduced(Some(0)));
    assert!(!windows_enable_transparency_is_reduced(Some(1)));
    assert!(!windows_enable_transparency_is_reduced(Some(2)));
    assert!(!windows_enable_transparency_is_reduced(None));
}

#[test]
fn reduced_transparency_test_override_scoped_and_restored() {
    let _off = test_overrides::set_reduced_transparency(false);
    assert!(!reduced_transparency());
    {
        let _on = test_overrides::set_reduced_transparency(true);
        assert!(reduced_transparency());
    }
    assert!(!reduced_transparency());
}

#[test]
fn linux_dark_resolution_honors_cli_override_then_desktop_then_dark_default() {
    assert!(!resolve_linux_dark(Some("light"), Some(true)));
    assert!(!resolve_linux_dark(Some("light"), None));
    assert!(resolve_linux_dark(Some("dark"), Some(false)));
    assert!(resolve_linux_dark(Some("dark"), None));
    assert!(resolve_linux_dark(None, Some(true)));
    assert!(!resolve_linux_dark(None, Some(false)));
    assert!(resolve_linux_dark(None, None));
}
