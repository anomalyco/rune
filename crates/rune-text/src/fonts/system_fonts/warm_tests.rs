use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Barrier, OnceLock, mpsc};
use std::time::Duration;

use parley::fontique::{Collection, CollectionOptions};

use super::WarmSlot;

#[test]
fn warm_slot_shares_single_instance_across_threads() {
    static SLOT: WarmSlot<usize> = WarmSlot::new();
    static INITS: AtomicUsize = AtomicUsize::new(0);

    fn init() -> usize {
        INITS.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(20));
        42
    }

    let barrier = std::sync::Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let b = barrier.clone();
        handles.push(std::thread::spawn(move || {
            b.wait();
            SLOT.with(init, |val| {
                *val += 1;
                *val
            })
        }));
    }

    let mut seen: Vec<usize> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    seen.sort_unstable();
    assert_eq!(INITS.load(Ordering::SeqCst), 1, "init must run exactly once across all threads");
    assert_eq!(seen, vec![43, 44, 45, 46], "all threads must observe mutations to the shared instance");
}

#[test]
fn warm_slot_prewarm_is_nonblocking_and_coalesces_with_caller() {
    static SLOT: WarmSlot<String> = WarmSlot::new();
    static INITS: AtomicUsize = AtomicUsize::new(0);
    static ENTERED: OnceLock<mpsc::Sender<String>> = OnceLock::new();
    static RELEASE: OnceLock<std::sync::Mutex<mpsc::Receiver<()>>> = OnceLock::new();

    let (enter_tx, enter_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    ENTERED.set(enter_tx).expect("set entered channel");
    RELEASE.set(std::sync::Mutex::new(release_rx)).expect("set release channel");

    fn init() -> String {
        INITS.fetch_add(1, Ordering::SeqCst);
        let thread_name = std::thread::current().name().unwrap_or("").to_owned();
        ENTERED.get().unwrap().send(thread_name.clone()).expect("signal worker entry");
        RELEASE.get().unwrap().lock().unwrap().recv().expect("wait for release");
        thread_name
    }

    let start = rune_core::time::now();
    SLOT.prewarm("rune-font-prewarm", || true, init);
    // Repeated prewarm while in-flight must return immediately without spawning a second init.
    SLOT.prewarm("rune-font-prewarm", || true, init);
    assert!(rune_core::time::now().duration_since(start) < Duration::from_secs(1), "prewarm must not block the calling thread");

    // Wait until the background worker has entered init.
    let worker_name = enter_rx.recv_timeout(Duration::from_secs(2)).expect("worker entered init");
    assert_eq!(worker_name, "rune-font-prewarm");

    // A layout caller arriving while prewarm is still running must wait for that exact build
    // rather than starting a second one.
    let caller = std::thread::spawn(|| SLOT.with(init, |val| val.clone()));
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(INITS.load(Ordering::SeqCst), 1, "concurrent caller must coalesce onto the in-flight prewarm");
    assert!(!SLOT.is_initialized(), "slot must stay uninitialized until init completes");

    release_tx.send(()).unwrap();
    let got = caller.join().unwrap();
    assert_eq!(got, "rune-font-prewarm");
    assert_eq!(INITS.load(Ordering::SeqCst), 1, "init must still have run exactly once");
    assert!(SLOT.is_initialized());

    // Calling prewarm again once initialized is a no-op.
    SLOT.prewarm("rune-font-prewarm", || true, init);
    assert_eq!(INITS.load(Ordering::SeqCst), 1);
}

#[test]
fn warm_slot_prewarm_respects_disabled_flag_and_retries_when_enabled() {
    static SLOT: WarmSlot<u32> = WarmSlot::new();
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static INITS: AtomicUsize = AtomicUsize::new(0);
    static DONE: OnceLock<mpsc::Sender<()>> = OnceLock::new();

    let (done_tx, done_rx) = mpsc::channel();
    DONE.set(done_tx).expect("set done channel");

    fn enabled() -> bool {
        ENABLED.load(Ordering::Acquire)
    }

    fn init() -> u32 {
        INITS.fetch_add(1, Ordering::SeqCst);
        DONE.get().unwrap().send(()).expect("signal done");
        7
    }

    // Disabled: prewarm must not spawn or initialize.
    SLOT.prewarm("rune-font-prewarm-test", enabled, init);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(INITS.load(Ordering::SeqCst), 0, "disabled prewarm must not run init");
    assert!(!SLOT.is_initialized());

    // Re-enabled: a subsequent prewarm runs init on a background thread.
    ENABLED.store(true, Ordering::Release);
    SLOT.prewarm("rune-font-prewarm-test", enabled, init);
    done_rx.recv_timeout(Duration::from_secs(2)).expect("re-enabled prewarm should run init");
    assert_eq!(SLOT.with(init, |v| *v), 7);
    assert_eq!(INITS.load(Ordering::SeqCst), 1);
}

#[test]
fn warm_slot_prewarm_handles_collection_across_threads() {
    static SLOT: WarmSlot<Collection> = WarmSlot::new();
    static INITS: AtomicUsize = AtomicUsize::new(0);

    fn build() -> Collection {
        INITS.fetch_add(1, Ordering::SeqCst);
        let mut col = Collection::new(CollectionOptions { shared: false, system_fonts: false });
        const INTER: &[u8] = include_bytes!("../../../../rune/assets/Inter-Regular.ttf");
        col.register_fonts(parley::fontique::Blob::new(std::sync::Arc::new(INTER)), None);
        col
    }

    SLOT.prewarm("rune-font-prewarm-collection", || true, build);

    let has_inter = std::thread::spawn(|| SLOT.with(build, |col| col.family_id("Inter").is_some()))
        .join()
        .unwrap();
    assert!(has_inter, "shared collection prewarmed on one thread must serve other threads");
    assert_eq!(INITS.load(Ordering::SeqCst), 1);
}

#[test]
fn prewarm_system_fonts_is_idempotent_and_nonblocking() {
    let start = rune_core::time::now();
    crate::prewarm_system_fonts();
    crate::prewarm_system_fonts();
    assert!(rune_core::time::now().duration_since(start) < Duration::from_secs(1));
}

#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
#[test]
fn fontconfig_empty_queries_do_not_initialize_or_fail() {
    assert!(super::super::fontconfig::family_files(&[]).is_empty());
    // Common-script-only characters have no script-specific fontconfig fallback.
    assert!(super::super::fontconfig::fallback_family_files(&['\u{2603}'], None).is_empty());
}
