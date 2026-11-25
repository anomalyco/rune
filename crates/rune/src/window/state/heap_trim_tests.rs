use std::cell::Cell;
use std::sync::Mutex;
use std::time::Duration;

use super::heap_trim::{
    PROBE_SIZE, TRIM_MIN_GAP, TrimEffect, TrimEvent, TrimState, env_allows_trim,
    maybe_trim_heap, probe_glibc_used, set_trim_enabled, step, test_support, trim_enabled,
    uses_glibc_allocator,
};
use super::rest::{self, REST_AFTER, Rest, RestEffect, RestEvent};
use crate::time::Instant;

static OPT_OUT_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn trim_state_first_rest_trims_concurrent_coalesces_and_respects_30s_cooldown() {
    let t0 = Instant::now();
    let mut state = TrimState::default();

    // First rest triggers one trim.
    let eff: Vec<_> = step(&mut state, TrimEvent::Rested { now: t0 }).into_iter().collect();
    assert_eq!(eff, vec![TrimEffect::Trim]);
    assert_eq!(state, TrimState::Running);

    // Multiple windows / overlays resting while already running coalesce into zero extra trims.
    let eff2 = step(&mut state, TrimEvent::Rested { now: t0 + Duration::from_millis(5) });
    assert!(eff2.is_empty());
    let eff3 = step(&mut state, TrimEvent::Rested { now: t0 + Duration::from_secs(60) });
    assert!(eff3.is_empty());

    // Completion returns to Idle with last_done stamped.
    let t_done = t0 + Duration::from_millis(10);
    let eff_fin = step(&mut state, TrimEvent::Finished { now: t_done });
    assert!(eff_fin.is_empty());
    assert_eq!(state, TrimState::Idle { last_done: Some(t_done) });

    // Straggler Finished while Idle is ignored.
    assert!(step(&mut state, TrimEvent::Finished { now: t_done + Duration::from_secs(1) }).is_empty());

    // Repeat rest inside 30 s cooldown skips.
    let just_before = t_done + TRIM_MIN_GAP - Duration::from_millis(1);
    assert!(step(&mut state, TrimEvent::Rested { now: just_before }).is_empty());
    assert_eq!(state, TrimState::Idle { last_done: Some(t_done) });

    // Repeat rest at/after 30 s trims again.
    let at_boundary = t_done + TRIM_MIN_GAP;
    assert_eq!(
        step(&mut state, TrimEvent::Rested { now: at_boundary }).into_iter().collect::<Vec<_>>(),
        vec![TrimEffect::Trim]
    );
    assert_eq!(state, TrimState::Running);
}

#[test]
fn rest_triggers_trim_once_and_arms_no_periodic_idle_timer() {
    let t0 = Instant::now();
    let mut rest_state = Rest::default();
    let mut trim_state = TrimState::default();

    // Frame drawn -> deadline set to t0 + REST_AFTER.
    assert!(rest::step(&mut rest_state, RestEvent::Drew { now: t0, low: false }).is_empty());
    assert_eq!(rest_state.deadline(), Some(t0 + REST_AFTER));

    // At REST_AFTER, Rest fires GiveBackUnused once -> TrimState triggers Trim once.
    let rest_eff: Vec<_> = rest::step(&mut rest_state, RestEvent::Woke { now: t0 + REST_AFTER }).into_iter().collect();
    assert_eq!(rest_eff, vec![RestEffect::GiveBackUnused]);
    assert_eq!(rest_state.deadline(), None, "idle window must arm no periodic timer");

    let mut trims = 0usize;
    for _ in rest_eff {
        if !step(&mut trim_state, TrimEvent::Rested { now: t0 + REST_AFTER }).is_empty() {
            trims += 1;
            let _ = step(
                &mut trim_state,
                TrimEvent::Finished { now: t0 + REST_AFTER + Duration::from_millis(4) },
            );
        }
    }
    assert_eq!(trims, 1);

    // Even minutes later while idle, waking Rest does nothing and triggers zero additional trims.
    for mins in 1..=5 {
        let later = t0 + REST_AFTER + Duration::from_secs(mins * 60);
        let eff = rest::step(&mut rest_state, RestEvent::Woke { now: later });
        assert!(eff.is_empty());
        assert_eq!(rest_state.deadline(), None);
    }
    assert_eq!(trims, 1);
}

#[test]
fn env_override_and_app_builder_opt_out_toggle_trim_enabled() {
    assert!(env_allows_trim(None));
    assert!(env_allows_trim(Some("")));
    assert!(env_allows_trim(Some("1")));
    assert!(env_allows_trim(Some("true")));
    assert!(!env_allows_trim(Some("0")));
    assert!(!env_allows_trim(Some(" 0 ")));
    assert!(!env_allows_trim(Some("false")));
    assert!(!env_allows_trim(Some("OFF")));
    assert!(!env_allows_trim(Some("no")));

    let _guard = OPT_OUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let spec = crate::AppSpec::new("TrimOptOut", (320.0, 200.0), || (), |_, _| crate::el());
    let _app = crate::App::new(spec).trim_heap_at_rest(false);
    assert!(!trim_enabled());

    let spec2 = crate::AppSpec::new("TrimOptIn", (320.0, 200.0), || (), |_, _| crate::el());
    let _app2 = crate::App::new(spec2).trim_heap_at_rest(true);
    assert!(trim_enabled());
    set_trim_enabled(true);
}

#[test]
fn allocator_probe_detects_glibc_rejects_foreign_allocators_and_handles_concurrent_free_and_wrap() {
    let mut backing = vec![0u8; PROBE_SIZE + 64];
    let used = Cell::new(1024usize);

    // 1. Glibc-backed allocator: used rises by PROBE_SIZE on alloc and drops on dealloc.
    let ptr = backing.as_mut_ptr();
    assert!(probe_glibc_used(
        usize::MAX,
        || used.get(),
        |l| {
            used.set(used.get() + l.size() + 16);
            ptr
        },
        |_, l| {
            used.set(used.get() - (l.size() + 16));
        },
    ));

    // 2. Foreign / custom / LD_PRELOAD allocator: glibc used bytes never move -> false.
    let foreign_used = Cell::new(4096usize);
    assert!(!probe_glibc_used(
        usize::MAX,
        || foreign_used.get(),
        |_| ptr,
        |_, _| {},
    ));

    // 3. Concurrent C free spoils attempt 1, attempt 2 succeeds.
    let attempt = Cell::new(0usize);
    let noisy_used = Cell::new(200_000usize);
    assert!(probe_glibc_used(
        usize::MAX,
        || noisy_used.get(),
        |l| {
            let n = attempt.get();
            attempt.set(n + 1);
            if n == 0 {
                // Concurrent free larger than PROBE_SIZE hides the growth on attempt 1.
                noisy_used.set(noisy_used.get() - 65_536);
            } else {
                noisy_used.set(noisy_used.get() + l.size() + 16);
            }
            ptr
        },
        |_, l| {
            noisy_used.set(noisy_used.get().saturating_sub(l.size() + 16));
        },
    ));

    // 4. Legacy 32-bit mallinfo wrap across u32::MAX boundary still detects exact delta.
    let wrap_used = Cell::new((u32::MAX as usize) - 1024);
    assert!(probe_glibc_used(
        u32::MAX as usize,
        || wrap_used.get(),
        |l| {
            wrap_used.set(wrap_used.get().wrapping_add(l.size() + 16) & (u32::MAX as usize));
            ptr
        },
        |_, l| {
            wrap_used.set(wrap_used.get().wrapping_sub(l.size() + 16) & (u32::MAX as usize));
        },
    ));
}

#[test]
fn platform_guard_matches_target_abi_and_coalesces_live_trim() {
    let _guard = OPT_OUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_trim_enabled(true);

    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        assert!(!uses_glibc_allocator());
        maybe_trim_heap();
        assert_eq!(test_support::trim_calls(), 0);
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        assert!(
            uses_glibc_allocator(),
            "default Linux/glibc unit test binary should detect glibc allocator"
        );
        test_support::wait_idle();
        test_support::reset_cooldown();
        let before = test_support::trim_calls();

        // Trigger two rests back-to-back: must coalesce into a single background trim.
        maybe_trim_heap();
        maybe_trim_heap();
        test_support::wait_idle();
        let after_first = test_support::trim_calls();
        assert_eq!(after_first, before + 1);

        // Another rest immediately inside the 30 s cooldown must be skipped.
        maybe_trim_heap();
        test_support::wait_idle();
        assert_eq!(test_support::trim_calls(), after_first);

        // Opting out prevents trimming even after resetting cooldown.
        set_trim_enabled(false);
        test_support::reset_cooldown();
        maybe_trim_heap();
        test_support::wait_idle();
        assert_eq!(test_support::trim_calls(), after_first);
        set_trim_enabled(true);
    }
}
