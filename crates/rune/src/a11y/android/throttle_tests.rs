//! Host-side unit tests for Android accessibility scroll pacing.

use std::time::Duration;

use crate::machine::Seq;
use crate::time::Instant;

use super::throttle::{Effect, Event, SCROLL_SYNC_INTERVAL, State, step};

fn run(seq: &mut Seq, state: &mut State, event: Event) -> Vec<Effect> {
    step(seq, state, event).into_iter().collect()
}

#[test]
fn rest_syncs_every_frame_immediately() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let t0 = Instant::now();

    for i in 0..5 {
        let now = t0 + Duration::from_millis(i * 8);
        assert_eq!(run(&mut seq, &mut state, Event::Frame { now, scrolling: false }), [Effect::SyncNow]);
        assert_eq!(state, State::Rest);
    }
}

#[test]
fn scroll_holds_within_interval_and_schedules_one_wake() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let t0 = Instant::now();

    // First scroll frame syncs immediately and starts the 100 ms cooldown.
    assert_eq!(
        run(&mut seq, &mut state, Event::Frame { now: t0, scrolling: true }),
        [Effect::SyncNow],
    );
    assert_eq!(state, State::Cooldown { last_sync: t0 });

    // Next frame 8 ms later enters Holding and schedules one WakeAfter for the remaining 92 ms.
    let e = run(
        &mut seq,
        &mut state,
        Event::Frame { now: t0 + Duration::from_millis(8), scrolling: true },
    );
    assert_eq!(
        e,
        [Effect::WakeAfter {
            attempt: seq.current(),
            delay: Duration::from_millis(92),
        }],
    );

    // Further frames before 100 ms emit nothing (no redundant syncs or wake churn).
    for ms in [16, 33, 50, 99] {
        assert!(
            run(
                &mut seq,
                &mut state,
                Event::Frame { now: t0 + Duration::from_millis(ms), scrolling: true },
            )
            .is_empty(),
            "frame at {ms} ms must stay held",
        );
    }

    // At 100 ms the next frame syncs again.
    assert_eq!(
        run(
            &mut seq,
            &mut state,
            Event::Frame { now: t0 + SCROLL_SYNC_INTERVAL, scrolling: true },
        ),
        [Effect::SyncNow],
    );
    assert_eq!(state, State::Cooldown { last_sync: t0 + SCROLL_SYNC_INTERVAL });
}

#[test]
fn one_second_fling_at_120hz_syncs_about_ten_times() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let t0 = Instant::now();
    let frame_us = 1_000_000u64 / 120;

    let mut sync_times = Vec::new();
    for frame in 0..120u64 {
        let now = t0 + Duration::from_micros(frame * frame_us);
        for effect in run(&mut seq, &mut state, Event::Frame { now, scrolling: true }) {
            if effect == Effect::SyncNow {
                sync_times.push(now);
            }
        }
    }

    assert!(
        (9..=11).contains(&sync_times.len()),
        "expected ~10 syncs during a 1 s fling at 120 Hz, got {}",
        sync_times.len(),
    );
    for pair in sync_times.windows(2) {
        let gap = pair[1].saturating_duration_since(pair[0]);
        assert!(
            gap >= SCROLL_SYNC_INTERVAL,
            "consecutive scroll syncs spaced by {gap:?} < {SCROLL_SYNC_INTERVAL:?}",
        );
    }
}

#[test]
fn resting_frame_flushes_held_updates_at_once() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let t0 = Instant::now();

    assert_eq!(run(&mut seq, &mut state, Event::Frame { now: t0, scrolling: true }), [Effect::SyncNow]);
    let e = run(
        &mut seq,
        &mut state,
        Event::Frame { now: t0 + Duration::from_millis(16), scrolling: true },
    );
    let Effect::WakeAfter { attempt, .. } = e[0] else {
        panic!("expected WakeAfter, got {e:?}");
    };

    // Scrolling stops at 25 ms: flush immediately without waiting for the 100 ms deadline.
    assert_eq!(
        run(
            &mut seq,
            &mut state,
            Event::Frame { now: t0 + Duration::from_millis(25), scrolling: false },
        ),
        [Effect::SyncNow],
    );
    assert_eq!(state, State::Rest);

    // The earlier wake timer firing later is stale and does nothing.
    assert!(
        run(
            &mut seq,
            &mut state,
            Event::Wake {
                attempt,
                now: t0 + SCROLL_SYNC_INTERVAL,
                scrolling: false,
            },
        )
        .is_empty(),
    );
}

#[test]
fn scheduled_wake_flushes_when_frames_stop_and_ignores_stale_attempts() {
    let mut seq = Seq::default();
    let mut state = State::default();
    let t0 = Instant::now();

    let _ = run(&mut seq, &mut state, Event::Frame { now: t0, scrolling: true });
    let e = run(
        &mut seq,
        &mut state,
        Event::Frame { now: t0 + Duration::from_millis(20), scrolling: true },
    );
    let Effect::WakeAfter { attempt, delay } = e[0] else {
        panic!("expected WakeAfter, got {e:?}");
    };
    assert_eq!(delay, Duration::from_millis(80));

    // A stale attempt is ignored.
    let stale = crate::machine::Attempt(attempt.0.wrapping_add(99));
    assert!(
        run(
            &mut seq,
            &mut state,
            Event::Wake {
                attempt: stale,
                now: t0 + SCROLL_SYNC_INTERVAL,
                scrolling: true,
            },
        )
        .is_empty(),
    );

    // An early wake while still scrolling re-arms for the remaining time.
    let e2 = run(
        &mut seq,
        &mut state,
        Event::Wake {
            attempt,
            now: t0 + Duration::from_millis(60),
            scrolling: true,
        },
    );
    let Effect::WakeAfter { attempt: attempt2, delay: delay2 } = e2[0] else {
        panic!("expected re-armed WakeAfter, got {e2:?}");
    };
    assert_ne!(attempt2, attempt);
    assert_eq!(delay2, Duration::from_millis(40));

    // The re-armed wake at due flushes and transitions to Rest when scrolling stopped.
    assert_eq!(
        run(
            &mut seq,
            &mut state,
            Event::Wake {
                attempt: attempt2,
                now: t0 + SCROLL_SYNC_INTERVAL,
                scrolling: false,
            },
        ),
        [Effect::SyncNow],
    );
    assert_eq!(state, State::Rest);

    // Reset clears any pending hold without emitting SyncNow.
    let _ = run(&mut seq, &mut state, Event::Frame { now: t0 + Duration::from_millis(200), scrolling: true });
    let _ = run(&mut seq, &mut state, Event::Frame { now: t0 + Duration::from_millis(210), scrolling: true });
    assert!(run(&mut seq, &mut state, Event::Reset).is_empty());
    assert_eq!(state, State::Rest);
}
