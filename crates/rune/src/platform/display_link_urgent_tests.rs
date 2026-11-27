use std::time::Duration;

use super::*;

#[test]
fn urgent_state_machine_coalesces_requests_and_consumes_once() {
    let mut state = UrgentAdmission::default();
    assert_eq!(state, UrgentAdmission::Idle);
    assert_eq!(step_urgent(&mut state, UrgentEvent::Consume), Effects::none());
    assert_eq!(state, UrgentAdmission::Idle);

    // Duplicate requests coalesce into one Armed state.
    assert_eq!(step_urgent(&mut state, UrgentEvent::Request), Effects::none());
    assert_eq!(state, UrgentAdmission::Armed);
    assert_eq!(step_urgent(&mut state, UrgentEvent::Request), Effects::none());
    assert_eq!(state, UrgentAdmission::Armed);

    // First consume yields AdmitOnce and resets to Idle; second consume does nothing.
    assert_eq!(step_urgent(&mut state, UrgentEvent::Consume), Effects::one(UrgentEffect::AdmitOnce));
    assert_eq!(state, UrgentAdmission::Idle);
    assert_eq!(step_urgent(&mut state, UrgentEvent::Consume), Effects::none());
    assert_eq!(state, UrgentAdmission::Idle);
}

#[test]
fn urgent_admission_bypasses_wait_once_advances_target_and_resumes_pacing() {
    let t0 = crate::time::now();
    let period = Duration::from_millis(16); // ~60 Hz
    let ms = |n| t0 + Duration::from_millis(n);

    let mut core = DisplayLinkCore::new(period);
    let _ = core.start();

    // Frame 1 draws on a paced tick aimed at t = 16 ms and finishes at t = 1 ms.
    assert_eq!(core.claim(Some(ms(16)), ms(0)), Opportunity::Tick(ms(16)));
    core.presented(ms(1));

    // Without urgent admission, a redraw at t = 3 ms must wait for the next tick.
    assert_eq!(core.claim(None, ms(3)), Opportunity::Wait);

    // Multiple urgent wakes coalesce into one admission.
    core.admit_once();
    core.admit_once();
    core.admit_once();
    assert!(core.is_urgent());

    // The waiting frame passes pacing once at t = 4 ms, advancing last_target to now.
    let opp = core.claim(None, ms(4));
    assert_eq!(opp, Opportunity::UrgentNow);
    assert_eq!(opp.target(), Some(None));
    assert!(opp.urgent_admitted());
    assert!(!core.is_urgent(), "admission must be consumed by claim");
    core.presented(ms(5));

    // Normal pacing resumes immediately: a redraw right after waits, and a tick within half a
    // refresh of the urgent present also waits before normal paced ticks resume.
    assert_eq!(core.claim(None, ms(6)), Opportunity::Wait);
    assert_eq!(core.claim(Some(ms(24)), ms(8)), Opportunity::Wait);
    let next = core.claim(Some(ms(32)), ms(16));
    assert_eq!(next, Opportunity::Tick(ms(32)));
    assert!(!next.urgent_admitted());
}

#[test]
fn urgent_admission_bypasses_wait_while_coasting_without_rearming_on_failed_acquire() {
    let t0 = crate::time::now();
    let period = Duration::from_nanos(1_000_000_000 / 120); // ~8.33 ms
    let ms = |n| t0 + Duration::from_millis(n);

    let mut core = DisplayLinkCore::new(period);
    let _ = core.start();
    assert_eq!(core.claim(None, ms(0)), Opportunity::Now);
    core.presented(ms(1));
    let _ = core.coast();
    assert!(matches!(core.state(), LinkState::Coasting { .. }));

    // Only 2 ms after present (not rested yet): urgent admission lets the coasting window draw
    // immediately upon resuming Active.
    core.admit_once();
    assert!(!core.defers_start(ms(3)), "an armed urgent frame must not defer link start");
    let _ = core.start();
    assert_eq!(core.claim(None, ms(3)), Opportunity::UrgentNow);

    // Simulate failed drawable acquisition or failed present (no presented() call):
    // urgent admission was already consumed and must NOT rearm.
    assert!(!core.is_urgent());
    assert_eq!(core.claim(None, ms(4)), Opportunity::Wait);

    // Hard stop (e.g. missed-frame backoff or occlusion) also clears any pending urgent flag.
    core.admit_once();
    assert!(core.is_urgent());
    let _ = core.stop();
    assert!(!core.is_urgent(), "hard stop must clear pending urgent admission");
}

#[test]
fn fresh_tick_or_rested_surface_consumes_urgent_without_leaking_to_later_frame() {
    let t0 = crate::time::now();
    let period = Duration::from_millis(16);
    let ms = |n| t0 + Duration::from_millis(n);

    // Rested surface: Opportunity::Now draws immediately and consumes urgent so the next frame waits.
    let mut rested_core = DisplayLinkCore::new(period);
    let _ = rested_core.start();
    rested_core.admit_once();
    let first = rested_core.claim(None, ms(0));
    assert_eq!(first, Opportunity::Now);
    assert!(!first.urgent_admitted());
    assert!(!rested_core.is_urgent());
    rested_core.presented(ms(1));
    assert_eq!(rested_core.claim(None, ms(3)), Opportunity::Wait, "urgent must not leak past a rested draw");

    // Fresh permitted tick: Opportunity::Tick draws on the tick and consumes urgent so the next frame waits.
    let mut tick_core = DisplayLinkCore::new(period);
    let _ = tick_core.start();
    assert_eq!(tick_core.claim(Some(ms(16)), ms(0)), Opportunity::Tick(ms(16)));
    tick_core.presented(ms(1));
    tick_core.admit_once();
    let on_tick = tick_core.claim(Some(ms(32)), ms(16));
    assert_eq!(on_tick, Opportunity::Tick(ms(32)));
    assert!(!on_tick.urgent_admitted());
    assert!(!tick_core.is_urgent());
    tick_core.presented(ms(17));
    assert_eq!(tick_core.claim(None, ms(19)), Opportunity::Wait, "urgent must not leak past a tick draw");
}

#[test]
fn urgent_admission_bypasses_ambient_and_low_power_caps_once_then_returns_to_cap() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let mut core = DisplayLinkCore::new(period_120);
    core.set_capped(true); // Low Power Mode 60 Hz
    core.set_ambient_period(Some(Duration::from_nanos(1_000_000_000 / 30))); // 30 Hz ambient cap
    let _ = core.start();

    assert_eq!(core.claim(Some(t0 + period_120), t0), Opportunity::Tick(t0 + period_120));
    core.presented(t0 + Duration::from_millis(1));

    // At 120 Hz tick #1 (8.33 ms): capped at 30 Hz, so normal pacing waits.
    let t1 = t0 + period_120;
    assert_eq!(core.claim(Some(t1 + period_120), t1), Opportunity::Wait);

    // Urgent frame passes once at 10 ms even while capped.
    let t_urgent = t0 + Duration::from_millis(10);
    core.admit_once();
    assert_eq!(core.claim(None, t_urgent), Opportunity::UrgentNow);
    core.presented(t_urgent + Duration::from_millis(1));

    // Next 120 Hz tick (~16.67 ms) is back under the 30 Hz cap and waits.
    let t2 = t0 + period_120 * 2;
    assert_eq!(core.claim(Some(t2 + period_120), t2), Opportunity::Wait);
}
