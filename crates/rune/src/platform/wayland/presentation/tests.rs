//! Wayland `wp_presentation` clock mapping, `presented` vs `discarded` delivery, bounded
//! in-flight tracking, idle-clean wake flag, and `FramePresentReports` integration.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use super::machine::{SurfacePresentationState, timespec_to_instant};
use crate::frame::presented::{FramePresentReports, FramePresented, MAX_PENDING, OnFramePresented};

fn logging() -> (Rc<RefCell<Vec<FramePresented>>>, OnFramePresented) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    (seen, Box::new(move |p: &FramePresented| sink.borrow_mut().push(p.clone())))
}

#[test]
fn timespec_to_instant_combines_64_bit_seconds_and_rejects_invalid_inputs() {
    let now = crate::time::now() + Duration::from_secs(10);
    // 64-bit seconds with non-zero tv_sec_hi (year > 2106 or high monotonic offset).
    let base_sec: u64 = (2u64 << 32) + 500;
    let hi = (base_sec >> 32) as u32;
    let lo = base_sec as u32;

    // 6.25 ms before clock_now
    assert_eq!(
        timespec_to_instant(now, base_sec as i64, 500_000_000, hi, lo, 493_750_000),
        Some(now - Duration::from_micros(6_250))
    );
    // 3.5 ms after clock_now across a second boundary
    let next_sec = base_sec + 1;
    assert_eq!(
        timespec_to_instant(
            now,
            base_sec as i64,
            999_000_000,
            (next_sec >> 32) as u32,
            next_sec as u32,
            2_500_000
        ),
        Some(now + Duration::from_micros(3_500))
    );
    // Invalid tv_nsec >= 1_000_000_000 rejected
    assert_eq!(timespec_to_instant(now, base_sec as i64, 0, hi, lo, 1_000_000_000), None);
    // Invalid clock_now_nsec rejected
    assert_eq!(timespec_to_instant(now, base_sec as i64, -1, hi, lo, 0), None);
    // > 60s skew rejected
    let far = base_sec + 61;
    assert_eq!(
        timespec_to_instant(now, base_sec as i64, 0, (far >> 32) as u32, far as u32, 0),
        None
    );
}

#[test]
fn presented_and_discarded_deliver_in_submission_order_and_clear_wake_when_idle() {
    let base = crate::time::now() + Duration::from_secs(10);
    let mut state = SurfacePresentationState::default();
    let (seen, hook) = logging();
    let mut reports = FramePresentReports::new(Some(hook), Some("main"));

    assert!(!state.needs_wake());
    assert_eq!(state.request_feedback(false), None, "no feedback without clock_id");

    let f0 = state.request_feedback(true).unwrap();
    let f1 = state.request_feedback(true).unwrap();
    let f2 = state.request_feedback(true).unwrap();
    assert!(state.needs_wake());
    reports.push_pending(f0, 0);
    reports.push_pending(f1, 1);
    reports.push_pending(f2, 2);

    // Frame 1 discarded before frame 0 arrives: held for submission order.
    state.on_discarded(f1);
    let mut buf = Vec::new();
    state.drain(&mut buf);
    reports.apply_completions(buf.drain(..));
    assert!(seen.borrow().is_empty());
    assert!(state.needs_wake());

    // Frame 0 presented: flushes frame 0 (Some) and frame 1 (None) in order.
    state.on_presented(f0, Some(base + Duration::from_millis(8)));
    state.drain(&mut buf);
    reports.apply_completions(buf.drain(..));
    assert_eq!(seen.borrow().len(), 2);
    assert_eq!((seen.borrow()[0].seq(), seen.borrow()[0].presented_at()), (0, Some(base + Duration::from_millis(8))));
    assert_eq!((seen.borrow()[1].seq(), seen.borrow()[1].presented_at()), (1, None));
    assert!(state.needs_wake(), "frame 2 still in flight");

    // Frame 2 presented: idle again, wake clears.
    state.on_presented(f2, Some(base + Duration::from_millis(24)));
    state.drain(&mut buf);
    reports.apply_completions(buf.drain(..));
    assert_eq!(seen.borrow().len(), 3);
    assert_eq!((seen.borrow()[2].seq(), seen.borrow()[2].presented_at()), (2, Some(base + Duration::from_millis(24))));
    assert!(!state.needs_wake(), "idle window must not wake on Wayland reads");
}

#[test]
fn reset_ignores_late_compositor_events_and_queue_stays_bounded() {
    let base = crate::time::now() + Duration::from_secs(10);
    let mut state = SurfacePresentationState::default();
    let old = state.request_feedback(true).unwrap();
    state.reset();
    assert!(!state.needs_wake());

    // Late compositor event for pre-reset feedback object must be ignored.
    state.on_presented(old, Some(base));
    let mut buf = Vec::new();
    state.drain(&mut buf);
    assert!(buf.is_empty());

    // Bounded at MAX_PENDING even if compositor never replies.
    for _ in 0..(MAX_PENDING + 16) {
        assert!(state.request_feedback(true).is_some());
    }
    assert_eq!(state.in_flight_len(), MAX_PENDING);
}
