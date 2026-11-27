use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use super::{DisplayPool, DrainTracker};

struct FakeGlobals {
    key: usize,
    id: usize,
    alive: AtomicBool,
    pool: Arc<DisplayPool<FakeGlobals>>,
    drops: Arc<AtomicUsize>,
}

impl Drop for FakeGlobals {
    fn drop(&mut self) {
        self.pool.release(self.key, self);
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn display_pool_reuses_live_entry_across_show_hide_cycles_and_drops_with_last_holder() {
    let pool = Arc::new(DisplayPool::<FakeGlobals>::new());
    let created = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));

    let make = |key: usize| {
        let pool_ref = pool.clone();
        let created_ref = created.clone();
        let dropped_ref = dropped.clone();
        pool.acquire(
            key,
            |g| g.alive.load(Ordering::Acquire),
            || {
                let id = created_ref.fetch_add(1, Ordering::SeqCst) + 1;
                Some(FakeGlobals {
                    key,
                    id,
                    alive: AtomicBool::new(true),
                    pool: pool_ref,
                    drops: dropped_ref,
                })
            },
        )
        .expect("created")
    };

    // First overlay shows and retains a persistent handle across hides.
    let persistent_holder = make(0x1000);
    assert_eq!(persistent_holder.id, 1);
    assert_eq!(created.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len(), 1);

    // Repeated transient show/hide cycles reuse the exact same entry without re-creating.
    for _ in 0..10 {
        let transient = make(0x1000);
        assert_eq!(transient.id, 1);
        assert!(Arc::ptr_eq(&persistent_holder, &transient));
        drop(transient);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
    }
    assert_eq!(created.load(Ordering::SeqCst), 1);

    // Distinct display pointer gets its own isolated entry.
    let other_display = make(0x2000);
    assert_eq!(other_display.id, 2);
    assert_eq!(pool.len(), 2);
    drop(other_display);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len(), 1);

    // Dropping the last holder tears down the entry and removes its key from the pool.
    drop(persistent_holder);
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
    assert_eq!(pool.len(), 0);

    // A later connection reusing the same display pointer address starts fresh cleanly.
    let fresh = make(0x1000);
    assert_eq!(fresh.id, 3);
    assert_eq!(pool.len(), 1);
    drop(fresh);
    assert_eq!(pool.len(), 0);
}

#[test]
fn display_pool_evicts_dead_entry_and_late_drop_does_not_remove_replacement() {
    let pool = Arc::new(DisplayPool::<FakeGlobals>::new());
    let created = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));

    let make = |key: usize| {
        let pool_ref = pool.clone();
        let created_ref = created.clone();
        let dropped_ref = dropped.clone();
        pool.acquire(
            key,
            |g| g.alive.load(Ordering::Acquire),
            || {
                let id = created_ref.fetch_add(1, Ordering::SeqCst) + 1;
                Some(FakeGlobals {
                    key,
                    id,
                    alive: AtomicBool::new(true),
                    pool: pool_ref,
                    drops: dropped_ref,
                })
            },
        )
        .expect("created")
    };

    let dead = make(0x3000);
    assert_eq!(dead.id, 1);
    dead.alive.store(false, Ordering::Release);

    // Acquiring while the dead entry is still held evicts it and creates a fresh replacement.
    let live = make(0x3000);
    assert_eq!(live.id, 2);
    assert_eq!(pool.len(), 1);

    // Dropping the dead entry later must NOT evict the live replacement at the same key.
    drop(dead);
    assert_eq!(pool.len(), 1);
    let reused = make(0x3000);
    assert_eq!(reused.id, 2);
    assert!(Arc::ptr_eq(&live, &reused));

    drop(reused);
    drop(live);
    assert_eq!(pool.len(), 0);
}

#[test]
fn display_pool_failed_create_leaves_no_entry() {
    let pool = DisplayPool::<usize>::new();
    let res = pool.acquire(0x4000, |_| true, || None);
    assert!(res.is_none());
    assert_eq!(pool.len(), 0);
}

#[test]
fn drain_tracker_flags_peer_queues_when_sibling_drains_shared_inner_queue() {
    let tracker = DrainTracker::new();
    let mut surface_a_gen = tracker.generation();
    let mut surface_b_gen = tracker.generation();

    // Idle: neither surface has pending events.
    assert!(!tracker.has_pending(surface_a_gen, false));
    assert!(!tracker.has_pending(surface_b_gen, false));

    // Zero-event inner drain does not trigger false pending flags.
    tracker.note_inner_drain(0);
    assert!(!tracker.has_pending(surface_a_gen, false));
    assert!(!tracker.has_pending(surface_b_gen, false));

    // Unread event sitting in C queue reports pending on both until drained.
    assert!(tracker.has_pending(surface_a_gen, true));
    assert!(tracker.has_pending(surface_b_gen, true));

    // Surface A drains the shared C queue (moving B's event into B's Rust buffer) and updates A's gen.
    tracker.note_inner_drain(1);
    surface_a_gen = tracker.generation();
    assert!(!tracker.has_pending(surface_a_gen, false));
    assert!(tracker.has_pending(surface_b_gen, false), "peer B must see pending even after C queue is empty");

    // Once B dispatches its own Rust buffer and records the generation, it settles cleanly.
    tracker.note_inner_drain(0);
    surface_b_gen = tracker.generation();
    assert!(!tracker.has_pending(surface_b_gen, false));
}
