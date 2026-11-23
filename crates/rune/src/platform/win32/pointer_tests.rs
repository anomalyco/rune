use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};
use std::time::Duration;
use crate::input::pointer_observer::Observer;
use crate::overlay::options::contains;
use super::{HOOK_FRESH, HookTracker, resolve_pointer};

#[test]
fn stale_cursor_after_hook_move_onto_hit_region_keeps_hook_point_and_does_not_requeue_stale() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = wakes.clone();
    let observer = Observer::new(Some(Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); })));
    observer.configure([0, 0, 100, 100], vec![[0, 0, 200, 200]], false);
    let tracker = Mutex::new(HookTracker::default());
    let hit_region = [40.0, 40.0, 80.0, 80.0];
    let t0 = crate::time::now();

    // Pointer approaches across the non-hit panel, then lands on the hit() button.
    let outside = (10, 10);
    let inside = (50, 50);
    tracker.lock().unwrap().record(outside, Some((5, 10)), t0);
    observer.observe(outside);
    assert_eq!(observer.take(), Some(outside));

    let t1 = t0 + Duration::from_millis(8);
    tracker.lock().unwrap().record(inside, Some(outside), t1);
    observer.observe(inside);
    assert_eq!(observer.take(), Some(inside));
    let wakes_before_regions = wakes.load(Ordering::Relaxed);

    // The hover redraw's set_regions runs while GetCursorPos still reports the pre-move point.
    let resolved = resolve_pointer(&tracker, || Some(outside), t1 + Duration::from_millis(1));
    assert_eq!(resolved, Some(inside));
    let (x, y) = resolved.unwrap();
    assert!(contains(hit_region, x as f32, y as f32), "resolved point must stay inside the hit region");

    // Feeding the resolved point back to Observer must not queue the stale point or wake again.
    observer.observe((x, y));
    assert_eq!(observer.take(), None);
    assert_eq!(wakes.load(Ordering::Relaxed), wakes_before_regions);
}

#[test]
fn stale_cursor_after_hook_move_off_hit_region_uses_new_outside_point() {
    let tracker = Mutex::new(HookTracker::default());
    let hit_region = [40.0, 40.0, 80.0, 80.0];
    let t0 = crate::time::now();

    let inside = (50, 50);
    let outside = (10, 10);
    tracker.lock().unwrap().record(outside, Some(inside), t0);

    // Unhover redraw runs while GetCursorPos still reports the old point on the button.
    let resolved = resolve_pointer(&tracker, || Some(inside), t0 + Duration::from_millis(1));
    assert_eq!(resolved, Some(outside));
    let (x, y) = resolved.unwrap();
    assert!(!contains(hit_region, x as f32, y as f32), "moving off a region must not stay hit-tested on stale GetCursorPos");
}

#[test]
fn set_cursor_pos_takes_over_immediately_without_waiting_for_fresh_window() {
    let tracker = Mutex::new(HookTracker::default());
    let t0 = crate::time::now();

    // Hook saw a normal move from (5, 5) to (10, 10).
    tracker.lock().unwrap().record((10, 10), Some((5, 5)), t0);

    // 50 ms later, SetCursorPos teleports the cursor to a button at (60, 60).
    let resolved = resolve_pointer(&tracker, || Some((60, 60)), t0 + Duration::from_millis(50));
    assert_eq!(resolved, Some((60, 60)));
}

#[test]
fn cursor_catching_up_clears_pending_so_later_set_cursor_pos_back_to_old_pixel_works() {
    let tracker = Mutex::new(HookTracker::default());
    let t0 = crate::time::now();

    tracker.lock().unwrap().record((50, 50), Some((10, 10)), t0);
    // GetCursorPos catches up to the hook's point.
    assert_eq!(resolve_pointer(&tracker, || Some((50, 50)), t0 + Duration::from_millis(2)), Some((50, 50)));
    // Now SetCursorPos moves back to (10, 10) within 250 ms and must be respected immediately.
    assert_eq!(resolve_pointer(&tracker, || Some((10, 10)), t0 + Duration::from_millis(20)), Some((10, 10)));
}

#[test]
fn expired_hook_sample_falls_back_to_cursor_position() {
    let tracker = Mutex::new(HookTracker::default());
    let t0 = crate::time::now();

    tracker.lock().unwrap().record((50, 50), Some((10, 10)), t0);
    assert_eq!(resolve_pointer(&tracker, || Some((10, 10)), t0 + HOOK_FRESH), Some((10, 10)));
}

#[test]
fn rapid_burst_treats_both_pre_move_and_previous_hook_point_as_stale() {
    let tracker = Mutex::new(HookTracker::default());
    let t0 = crate::time::now();

    tracker.lock().unwrap().record((20, 20), Some((10, 10)), t0);
    tracker.lock().unwrap().record((50, 50), Some((10, 10)), t0 + Duration::from_millis(1));

    // Both the hook-captured cursor (10, 10) and the previous hook point (20, 20) resolve to (50, 50).
    assert_eq!(resolve_pointer(&tracker, || Some((10, 10)), t0 + Duration::from_millis(2)), Some((50, 50)));
    assert_eq!(resolve_pointer(&tracker, || Some((20, 20)), t0 + Duration::from_millis(3)), Some((50, 50)));
}

#[test]
fn concurrent_hook_move_during_cursor_read_returns_newer_hook_point_and_preserves_pending() {
    let tracker = Mutex::new(HookTracker::default());
    let t0 = crate::time::now();

    tracker.lock().unwrap().record((20, 20), Some((10, 10)), t0);
    let resolved = resolve_pointer(
        &tracker,
        || {
            // Simulate a newer WH_MOUSE_LL move arriving while GetCursorPos is in flight.
            tracker.lock().unwrap().record((50, 50), Some((20, 20)), t0 + Duration::from_millis(1));
            Some((20, 20))
        },
        t0 + Duration::from_millis(2),
    );
    assert_eq!(resolved, Some((50, 50)));
    // The newer pending state must still be intact for the next stale GetCursorPos read.
    assert_eq!(resolve_pointer(&tracker, || Some((20, 20)), t0 + Duration::from_millis(3)), Some((50, 50)));
}

#[test]
fn moving_canvas_under_stationary_pointer_wakes_with_fresh_hook_point() {
    let observer = Observer::new(None);
    observer.configure([0, 0, 100, 100], vec![[0, 0, 200, 200]], false);
    let tracker = Mutex::new(HookTracker::default());
    let t0 = crate::time::now();

    tracker.lock().unwrap().record((50, 50), Some((10, 10)), t0);
    observer.observe((50, 50));
    assert_eq!(observer.take(), Some((50, 50)));

    // Moving the overlay changes its canvas and resets Observer's dedup state.
    observer.configure([20, 20, 120, 120], vec![[0, 0, 200, 200]], false);
    let resolved = resolve_pointer(&tracker, || Some((10, 10)), t0 + Duration::from_millis(1)).unwrap();
    observer.observe(resolved);
    assert_eq!(observer.take(), Some((50, 50)));
}
