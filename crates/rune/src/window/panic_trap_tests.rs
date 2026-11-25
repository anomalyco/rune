use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{PanicTrap, is_nested_run_loop_mode};
use crate::runtime::{Env, Runtime};
use crate::window::host::app::{test_app, test_app_with_close_request, test_app_with_requests};
use crate::{Cx, El, Tx, el};

#[derive(Debug, PartialEq, Eq)]
struct CustomPayload(u32);

#[test]
fn guard_passes_through_without_exiting_when_no_panic_occurs() {
    let mut trap = PanicTrap::default();
    let mut ran = 0;
    let mut exited = 0;

    trap.guard(|| exited += 1, || ran += 1);

    assert_eq!(ran, 1);
    assert_eq!(exited, 0);
    assert!(!trap.is_panicking());
    assert!(trap.take().is_none());
}

#[test]
fn guard_captures_first_panic_requests_exit_once_and_skips_later_events_and_exiting() {
    let mut trap = PanicTrap::default();
    let mut exited = 0;
    let mut later_events = 0;
    let mut quit_hooks = 0;
    let mut close_hooks = 0;
    let mut fast_exit = 0;

    trap.guard(|| exited += 1, || std::panic::panic_any(CustomPayload(924)));
    assert!(trap.is_panicking());
    assert_eq!(exited, 1);

    // Subsequent event-loop callbacks do not run and do not ask to exit again.
    trap.guard(|| exited += 1, || later_events += 1);
    trap.guard(|| exited += 1, || panic!("second panic must never run"));
    assert_eq!(later_events, 0);
    assert_eq!(exited, 1);

    // Direct record after a first panic preserves the first payload.
    assert!(!trap.record(Box::new(CustomPayload(1))));

    // Exiting checks `is_panicking()` first and skips quit hooks, close hooks and fast exit.
    if !trap.is_panicking() {
        quit_hooks += 1;
        close_hooks += 1;
        fast_exit += 1;
    }
    assert_eq!(quit_hooks, 0);
    assert_eq!(close_hooks, 0);
    assert_eq!(fast_exit, 0);

    // Resuming unwind re-raises the original typed payload.
    let payload = trap.take().expect("captured panic payload");
    let caught = catch_unwind(AssertUnwindSafe(|| resume_unwind(payload)))
        .expect_err("resume_unwind must panic");
    assert_eq!(caught.downcast_ref::<CustomPayload>(), Some(&CustomPayload(924)));
    assert!(!trap.is_panicking());
}

#[test]
fn on_focus_and_on_close_panics_are_captured_with_original_message_and_skip_quit_hooks() {
    fn view(_: &(), _: &mut Cx<()>) -> El { el() }

    static QUIT_COUNT: AtomicUsize = AtomicUsize::new(0);
    QUIT_COUNT.store(0, Ordering::SeqCst);

    // 1. on_focus panic through Runtime::window_focus.
    let mut rt = Runtime::new((), view, (100.0, 100.0), Env::default());
    rt.hooks.on_focus = Some(|_, focused, _| panic!("on_focus panic ({focused})"));
    let mut app = test_app(rt, None, Some(|_, _| { QUIT_COUNT.fetch_add(1, Ordering::SeqCst); }));

    let mut trap = PanicTrap::default();
    let mut exited = 0;
    trap.guard(|| exited += 1, || app.window_focus(true));
    assert_eq!(exited, 1);
    assert!(trap.is_panicking());
    if !trap.is_panicking() {
        app.run_on_quit();
    }
    assert_eq!(QUIT_COUNT.load(Ordering::SeqCst), 0, "on_quit must not run after on_focus panic");

    let payload = trap.take().expect("on_focus panic payload");
    let caught = catch_unwind(AssertUnwindSafe(|| resume_unwind(payload))).unwrap_err();
    let msg = caught.downcast_ref::<String>().map(String::as_str)
        .or_else(|| caught.downcast_ref::<&str>().copied())
        .expect("string panic message");
    assert_eq!(msg, "on_focus panic (true)");

    // 2. on_close panic through WindowApp::run_on_close.
    let rt = Runtime::new((), view, (100.0, 100.0), Env::default());
    let mut app = test_app(
        rt,
        Some(|_: &mut (), _: &mut Tx| panic!("on_close panic")),
        Some(|_, _| { QUIT_COUNT.fetch_add(1, Ordering::SeqCst); }),
    );
    let mut trap = PanicTrap::default();
    let mut exited = 0;
    trap.guard(|| exited += 1, || app.run_on_close());
    assert_eq!(exited, 1);
    assert!(trap.is_panicking());
    if !trap.is_panicking() {
        app.run_on_quit();
    }
    assert_eq!(QUIT_COUNT.load(Ordering::SeqCst), 0, "on_quit must not run after on_close panic");

    let payload = trap.take().expect("on_close panic payload");
    let caught = catch_unwind(AssertUnwindSafe(|| resume_unwind(payload))).unwrap_err();
    let msg = caught.downcast_ref::<&str>().copied().expect("static str panic message");
    assert_eq!(msg, "on_close panic");

    // 3. on_close_request panic through WindowApp::run_on_close_request.
    let rt = Runtime::new((), view, (100.0, 100.0), Env::default());
    let mut app = test_app_with_close_request(
        rt,
        Some(|_: &mut (), _: &mut Tx| panic!("on_close_request panic")),
        Some(|_, _| { QUIT_COUNT.fetch_add(1, Ordering::SeqCst); }),
        Some(|_, _| { QUIT_COUNT.fetch_add(1, Ordering::SeqCst); }),
    );
    let mut trap = PanicTrap::default();
    let mut exited = 0;
    trap.guard(|| exited += 1, || { let _ = app.run_on_close_request(); });
    assert_eq!(exited, 1);
    assert!(trap.is_panicking());
    if !trap.is_panicking() {
        app.run_on_close();
        app.run_on_quit();
    }
    assert_eq!(QUIT_COUNT.load(Ordering::SeqCst), 0, "on_close/on_quit must not run after on_close_request panic");
    let payload = trap.take().expect("on_close_request panic payload");
    let caught = catch_unwind(AssertUnwindSafe(|| resume_unwind(payload))).unwrap_err();
    let msg = caught.downcast_ref::<&str>().copied().expect("static str panic message");
    assert_eq!(msg, "on_close_request panic");

    // 4. on_quit_request panic through WindowApp::run_on_quit_request.
    let rt = Runtime::new((), view, (100.0, 100.0), Env::default());
    let mut app = test_app_with_requests(
        rt,
        None,
        Some(|_: &mut (), _: &mut Tx| panic!("on_quit_request panic")),
        Some(|_, _| { QUIT_COUNT.fetch_add(1, Ordering::SeqCst); }),
        Some(|_, _| { QUIT_COUNT.fetch_add(1, Ordering::SeqCst); }),
    );
    let mut trap = PanicTrap::default();
    let mut exited = 0;
    trap.guard(|| exited += 1, || { let _ = app.run_on_quit_request(); });
    assert_eq!(exited, 1);
    assert!(trap.is_panicking());
    if !trap.is_panicking() {
        app.run_on_close();
        app.run_on_quit();
    }
    assert_eq!(QUIT_COUNT.load(Ordering::SeqCst), 0, "on_close/on_quit must not run after on_quit_request panic");
    let payload = trap.take().expect("on_quit_request panic payload");
    let caught = catch_unwind(AssertUnwindSafe(|| resume_unwind(payload))).unwrap_err();
    let msg = caught.downcast_ref::<&str>().copied().expect("static str panic message");
    assert_eq!(msg, "on_quit_request panic");
}

#[test]
fn nested_run_loop_mode_allows_launch_and_default_mode_only() {
    assert!(!is_nested_run_loop_mode(None));
    assert!(!is_nested_run_loop_mode(Some("kCFRunLoopDefaultMode")));
    assert!(is_nested_run_loop_mode(Some("NSEventTrackingRunLoopMode")));
    assert!(is_nested_run_loop_mode(Some("NSModalPanelRunLoopMode")));
}
