use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use super::*;
use crate::{Cx, El, Env, Runtime, el, memo, text};

#[test]
fn sanitizes_and_clamps_text_scale() {
    assert_eq!(sanitize(1.0), Some(1.0));
    assert_eq!(sanitize(1.5), Some(1.5));
    assert_eq!(sanitize(2.25), Some(2.25));
    assert_eq!(sanitize(3.12), Some(3.12));
    assert_eq!(sanitize(0.1), Some(MIN));
    assert_eq!(sanitize(10.0), Some(MAX));
    assert_eq!(sanitize(0.0), None);
    assert_eq!(sanitize(-1.5), None);
    assert_eq!(sanitize(f32::NAN), None);
    assert_eq!(sanitize(f32::INFINITY), None);
    assert_eq!(sanitize(f32::NEG_INFINITY), None);

    assert_eq!(from_f64(1.25), Some(1.25));
    assert_eq!(from_f64(0.05), Some(MIN));
    assert_eq!(from_f64(8.0), Some(MAX));
    assert_eq!(from_f64(0.0), None);
    assert_eq!(from_f64(-0.5), None);
    assert_eq!(from_f64(f64::NAN), None);
    assert_eq!(from_f64(f64::INFINITY), None);
}

#[test]
fn parses_rune_text_scale_env_preset() {
    assert_eq!(parse_env("1.0"), Some(1.0));
    assert_eq!(parse_env("1.5"), Some(1.5));
    assert_eq!(parse_env("  2.25  "), Some(2.25));
    assert_eq!(parse_env("0.1"), Some(MIN));
    assert_eq!(parse_env("9.5"), Some(MAX));
    assert_eq!(parse_env("0"), None);
    assert_eq!(parse_env("-1.5"), None);
    assert_eq!(parse_env("NaN"), None);
    assert_eq!(parse_env("inf"), None);
    assert_eq!(parse_env("bogus"), None);
    assert_eq!(parse_env(""), None);
}

#[test]
fn converts_windows_text_scale_percent() {
    assert_eq!(from_windows_percent(0), None);
    assert_eq!(from_windows_percent(10), Some(MIN));
    assert_eq!(from_windows_percent(100), Some(1.0));
    assert_eq!(from_windows_percent(125), Some(1.25));
    assert_eq!(from_windows_percent(150), Some(1.5));
    assert_eq!(from_windows_percent(225), Some(2.25));
    assert_eq!(from_windows_percent(1000), Some(MAX));
}

#[cfg(target_os = "windows")]
#[test]
fn windows_ui_settings_reads_valid_scale_on_calling_and_worker_thread() {
    let direct = win::read_ui_settings();
    if let Some(scale) = direct {
        assert!((MIN..=MAX).contains(&scale), "unexpected UISettings text scale: {scale}");
    }
    let worker = std::thread::spawn(win::read_ui_settings).join().expect("worker thread panicked");
    if let Some(scale) = worker {
        assert!((MIN..=MAX).contains(&scale), "unexpected worker UISettings text scale: {scale}");
    }
    assert_eq!(direct, worker);
}

#[test]
fn maps_ios_dynamic_type_categories_and_body_points() {
    let cases = [
        ("UICTContentSizeCategoryXS", 14.0 / 17.0),
        ("UIContentSizeCategoryExtraSmall", 14.0 / 17.0),
        ("UICTContentSizeCategoryS", 15.0 / 17.0),
        ("UICTContentSizeCategoryM", 16.0 / 17.0),
        ("UICTContentSizeCategoryL", 1.0),
        ("UIContentSizeCategoryLarge", 1.0),
        ("UICTContentSizeCategoryXL", 19.0 / 17.0),
        ("UICTContentSizeCategoryXXL", 21.0 / 17.0),
        ("UICTContentSizeCategoryXXXL", 23.0 / 17.0),
        ("UICTContentSizeCategoryAccessibilityM", 28.0 / 17.0),
        ("UICTContentSizeCategoryAccessibilityL", 33.0 / 17.0),
        ("UICTContentSizeCategoryAccessibilityXL", 40.0 / 17.0),
        ("UICTContentSizeCategoryAccessibilityXXL", 47.0 / 17.0),
        ("UICTContentSizeCategoryAccessibilityXXXL", 53.0 / 17.0),
        ("UIContentSizeCategoryAccessibilityExtraExtraExtraLarge", 53.0 / 17.0),
    ];
    for (name, expected) in cases {
        let got = from_ios_category(name).unwrap_or_else(|| panic!("missing mapping for {name}"));
        assert!((got - expected as f32).abs() < 1e-6, "{name}: got {got}, expected {expected}");
    }
    assert_eq!(from_ios_category("UICTContentSizeCategoryUnspecified"), None);
    assert_eq!(from_ios_category(""), None);
    assert_eq!(from_ios_body_points(17.0), Some(1.0));
    assert_eq!(from_ios_body_points(34.0), Some(2.0));
    assert_eq!(from_ios_body_points(0.0), None);
    assert_eq!(from_ios_body_points(-17.0), None);
}

#[test]
fn cx_system_text_scale_defaults_to_one_rebuilds_view_and_memos_and_short_circuits_repeats() {
    struct State {
        views: Rc<Cell<usize>>,
        memos: Rc<Cell<usize>>,
        seen: Rc<Cell<f32>>,
        memo_seen: Rc<Cell<f32>>,
    }

    let views = Rc::new(Cell::new(0));
    let memos = Rc::new(Cell::new(0));
    let seen = Rc::new(Cell::new(0.0));
    let memo_seen = Rc::new(Cell::new(0.0));

    let state = State {
        views: views.clone(),
        memos: memos.clone(),
        seen: seen.clone(),
        memo_seen: memo_seen.clone(),
    };

    let mut rt = Runtime::new(
        state,
        |s: &State, cx: &mut Cx<State>| -> El {
            s.views.set(s.views.get() + 1);
            let scale = cx.system_text_scale();
            s.seen.set(scale);
            let m = s.memos.clone();
            let ms = s.memo_seen.clone();
            el().size(200.0, 80.0).child(memo("scaled-label", 0u8, move || {
                m.set(m.get() + 1);
                ms.set(scale);
                text(format!("{scale:.2}"))
            }))
        },
        (200.0, 80.0),
        Env::new(true, 1.0),
    );

    assert_eq!(rt.system_text_scale(), 1.0);
    rt.frame(0.0);
    assert_eq!(seen.get(), 1.0);
    assert_eq!(memo_seen.get(), 1.0);
    assert_eq!(views.get(), 1);
    assert_eq!(memos.get(), 1);

    // Repeating the same value does not mark dirty or rebuild view/memo.
    assert!(!rt.core_mut().set_system_text_scale(1.0));
    rt.frame(0.0);
    assert_eq!(views.get(), 1);
    assert_eq!(memos.get(), 1);

    // Changing to 1.5 marks dirty, re-runs view, and invalidates memos.
    assert!(rt.core_mut().set_system_text_scale(1.5));
    assert_eq!(rt.system_text_scale(), 1.5);
    rt.frame(0.0);
    assert_eq!(seen.get(), 1.5);
    assert_eq!(memo_seen.get(), 1.5);
    assert_eq!(views.get(), 2);
    assert_eq!(memos.get(), 2);

    // Repeating 1.5 is a no-op.
    assert!(!rt.core_mut().set_system_text_scale(1.5));
    rt.frame(0.0);
    assert_eq!(views.get(), 2);
    assert_eq!(memos.get(), 2);

    // Invalid (NaN / 0 / negative) values fall back to 1.0 and trigger a rebuild from 1.5.
    assert!(rt.core_mut().set_system_text_scale(f32::NAN));
    assert_eq!(rt.system_text_scale(), 1.0);
    rt.frame(0.0);
    assert_eq!(seen.get(), 1.0);
    assert_eq!(memo_seen.get(), 1.0);
    assert_eq!(views.get(), 3);
    assert_eq!(memos.get(), 3);

    assert!(!rt.core_mut().set_system_text_scale(-2.0));
    assert_eq!(rt.system_text_scale(), 1.0);
}

#[test]
fn refresh_accessibility_preferences_updates_system_text_scale() {
    let mut rt = Runtime::new(
        (),
        |_: &(), cx: &mut Cx<()>| text(format!("{}", cx.system_text_scale())),
        (160.0, 40.0),
        Env::new(true, 1.0),
    );
    let _g = test_overrides::set(1.75);
    crate::input::host_events::refresh_accessibility_preferences(rt.core_mut());
    assert_eq!(rt.system_text_scale(), 1.75);
}

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
fn cached_linux_text_scale_fallback_reads_once_off_calling_thread_and_notifies_on_late_change() {
    static CACHE: CachedTextScale = CachedTextScale::new();
    let gate = Gate::new();
    let gate_worker = Arc::clone(&gate);
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_worker = Arc::clone(&calls);
    let notified = Arc::new(AtomicUsize::new(0));
    let notified_worker = Arc::clone(&notified);
    let caller_thread = std::thread::current().id();
    let worker_thread = Arc::new(Mutex::new(None));
    let worker_thread_out = Arc::clone(&worker_thread);
    let (late_tx, late_rx) = mpsc::channel();

    let initial = CACHE.get_or_init_with(
        Duration::ZERO,
        move || {
            calls_worker.fetch_add(1, Ordering::SeqCst);
            *worker_thread_out.lock().unwrap() = Some(std::thread::current().id());
            gate_worker.enter_and_wait();
            Some(1.25)
        },
        move || {
            notified_worker.fetch_add(1, Ordering::SeqCst);
            late_tx.send(()).ok();
        },
    );
    assert_eq!(initial, DEFAULT, "timed-out launch falls back to 1.0");
    gate.wait_entered();
    assert_eq!(CACHE.get_or_init(|| panic!("must not respawn while pending")), DEFAULT);
    assert_eq!(notified.load(Ordering::SeqCst), 0);

    gate.release();
    late_rx.recv().expect("late non-default text scale must notify windows");
    CACHE.wait_ready();
    assert!(CACHE.late_changed());
    assert_eq!(CACHE.get_or_init(|| Some(2.0)), 1.25);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(notified.load(Ordering::SeqCst), 1);
    assert_ne!(*worker_thread.lock().unwrap(), Some(caller_thread));
}
