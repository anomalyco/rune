use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use crate::{App, AppSpec, Env, Runtime, col, text};

#[test]
fn headless_runtimes_keep_async_system_fallbacks_disabled_and_deterministic() {
    let rt = Runtime::new(
        (),
        |_: &(), _| col().child(text("snow \u{2603} han \u{6F22}")),
        (320.0, 120.0),
        Env::new(true, 1.0),
    );
    assert!(!rt.text.async_fallbacks_enabled(), "Runtime::new must stay synchronous");

    let spec = AppSpec::new("headless-fonts", (320.0, 120.0), || (), |_: &(), _| col().child(text("han \u{6F22}")));
    let headless = App::new(spec).headless_runtime((320.0, 120.0), Env::new(true, 1.0));
    assert!(!headless.text.async_fallbacks_enabled(), "App::headless_runtime must stay synchronous");
}

#[test]
fn live_runtime_async_system_fallbacks_wake_relayout_and_preserve_waker_on_replace() {
    let mut rt = Runtime::new(
        (),
        |_: &(), _| col().child(text("han \u{6F22}")),
        (320.0, 120.0),
        Env::new(true, 1.0),
    );
    let old_wakes = Arc::new(AtomicUsize::new(0));
    let ow = old_wakes.clone();
    rt.set_live_waker(Arc::new(move || {
        ow.fetch_add(1, Ordering::SeqCst);
    }));
    rt.core_mut().enable_async_system_fallbacks();
    assert!(rt.text.async_fallbacks_enabled());

    // Replacing the live waker must keep async fallbacks enabled and switch to the new wake lane.
    let (tx, rx) = mpsc::channel();
    rt.set_live_waker(Arc::new(move || {
        let _ = tx.send(());
    }));
    assert!(rt.text.async_fallbacks_enabled());

    // Frame 0: dispatches uncached CJK off-thread without blocking.
    rt.frame(0.0);
    assert_eq!(old_wakes.load(Ordering::SeqCst), 0, "replaced waker must not fire");

    // Background worker finishes and wakes the live lane.
    rx.recv_timeout(Duration::from_secs(5))
        .expect("async system fallback worker must wake the live runtime");
    assert!(rt.live_pending(), "wake must mark live lane pending for host redraw");
    assert!(rt.text.async_fallbacks_ready());

    // Frame 1: picks up arrived fallback faces via follow_font_families, relayouts, and settles.
    let commits_before = rt.stats.commits;
    rt.frame(0.0);
    assert!(!rt.text.async_fallbacks_ready());
    assert!(rt.stats.commits >= commits_before);
    assert_eq!(rt.wake_deadline(), None, "settled runtime must leave no idle wake");
}
