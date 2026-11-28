use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use rune_core::tok::TextStyle;

use crate::TextSystem;

use super::super::blobs;
use super::{FallbackResult, SlotLoadState};

fn bundled_font(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rune/assets").join(name)
}

#[test]
fn default_text_system_keeps_async_fallbacks_disabled_for_deterministic_headless_runs() {
    let ts = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    assert!(!ts.async_fallbacks_enabled());
    assert!(!ts.async_fallbacks_ready());
    assert!(!ts.families_behind());
}

#[test]
fn async_fallback_arrival_registers_preloaded_blobs_bumps_epoch_and_clears_ready() {
    let mut ts = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    ts.set_async_fallback_waker(Some(Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    })));
    assert!(ts.async_fallbacks_enabled());
    assert!(!ts.async_fallbacks_ready());

    let mono_path = bundled_font("IBMPlexMono-Regular.ttf");
    let mono_blob = blobs::load_blob(&mono_path).expect("bundled IBMPlexMono-Regular.ttf");
    let epoch_before = ts.shaped_epoch;

    ts.fallbacks.async_load.push_for_test(FallbackResult::InstalledFamily {
        entries: vec![(mono_path.clone(), mono_blob)],
    });

    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert!(ts.async_fallbacks_ready());
    assert!(ts.families_behind());

    assert!(ts.follow_async_fallbacks(), "first follow must register arrived faces and request relayout");
    assert_ne!(ts.shaped_epoch, epoch_before, "follow must invalidate shaped layouts");
    assert!(!ts.async_fallbacks_ready());
    assert!(ts.font_cx.collection.family_id("IBM Plex Mono").is_some());
    assert!(ts.font_files.contains_key(&mono_path));

    // Second follow with no new arrivals must be a no-op (no redraw loop).
    assert!(!ts.follow_async_fallbacks());
}

#[test]
fn empty_non_deferred_async_result_does_not_trigger_spurious_relayout() {
    let mut ts = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    ts.set_async_fallback_waker(Some(Arc::new(|| {})));
    let epoch_before = ts.shaped_epoch;

    ts.fallbacks.async_load.push_for_test(FallbackResult::Cjk {
        entries: Vec::new(),
    });
    assert!(ts.async_fallbacks_ready());
    assert!(!ts.follow_async_fallbacks(), "empty CJK result adds no faces and needs no relayout");
    assert_eq!(ts.shaped_epoch, epoch_before);
    assert!(!ts.async_fallbacks_ready());
}

#[test]
fn slot_load_state_deduplicates_pending_dispatches() {
    let mut state = super::AsyncFallbacks::default();
    assert_eq!(state.math, SlotLoadState::Idle);
    assert!(state.mark_math_pending());
    assert!(state.math_pending());
    assert!(!state.mark_math_pending(), "second pending mark while in-flight must be ignored");
    state.mark_math_loaded();
    assert!(!state.math_pending());
    assert!(!state.mark_math_pending(), "loaded slot must not re-enter pending");
}

#[test]
fn async_fallback_worker_resolves_cjk_off_thread_and_wakes_caller() {
    if !super::super::system_fonts_enabled() {
        return;
    }
    let mut ts = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    let (tx, rx) = mpsc::channel();
    ts.set_async_fallback_waker(Some(Arc::new(move || {
        let _ = tx.send(());
    })));

    let style = TextStyle::new(14.0, 400.0);
    // Frame 0: uncached CJK dispatches off-thread and returns immediately without loading CJK files synchronously.
    let _ = ts.layout_rich("han \u{6F22}", &[], style, 1.0, None);

    // Wait for the background worker to finish and fire the wake callback.
    rx.recv_timeout(Duration::from_secs(5))
        .expect("background fallback worker must wake caller once CJK lookup finishes");
    assert!(ts.async_fallbacks_ready());

    let files_before = ts.font_files.len();
    let changed = ts.follow_async_fallbacks();
    // On hosts with a system CJK font, the fallback registers with zero extra wake loops;
    // on minimal containers with no CJK font installed, changed may be false.
    if changed {
        assert!(ts.font_files.len() >= files_before);
    }
    assert!(!ts.async_fallbacks_ready());

    // Frame 1: laying out the same CJK text must not re-dispatch a second background job.
    let _ = ts.layout_rich("han \u{6F22}", &[], style, 1.0, None);
    assert!(rx.recv_timeout(Duration::from_millis(50)).is_err(), "already-loaded CJK must not re-dispatch");
}
