use std::cell::RefCell;

use super::{Reads, Tracked, tracked_reads};

#[test]
fn tracked_drop_at_thread_exit_after_version_table_destroyed_does_not_abort() {
    thread_local! {
        static HELD: Tracked<bool> = Tracked::new(false);
    }

    let handle = std::thread::spawn(|| {
        // Initialize the user's thread-local before Rune's VERSIONS table so Rust destroys
        // VERSIONS first (LIFO) at thread exit.
        HELD.with(|h| assert!(!**h));
        // Any write initializes VERSIONS after HELD.
        drop(Tracked::new(0u8));
    });
    handle.join().expect("thread with TLS Tracked exited cleanly");
}

#[test]
fn tracked_operations_during_tls_teardown_after_tables_destroyed_do_not_panic() {
    struct OnExit {
        tracked: Tracked<u32>,
        reads: Reads,
    }

    impl Drop for OnExit {
        fn drop(&mut self) {
            // Both VERSIONS and READS are already destroyed on this thread.
            assert_eq!(*self.tracked, 7);
            *self.tracked = 8;
            Tracked::touch(&self.tracked);
            assert_eq!(Tracked::version(&self.tracked), 0);
            assert!(!self.reads.changed());
            self.reads.read();
            let (value, reads) = tracked_reads(|| *self.tracked);
            assert_eq!(value, 8);
            assert!(!reads.changed());
        }
    }

    thread_local! {
        static EXIT: RefCell<Option<OnExit>> = const { RefCell::new(None) };
    }

    let handle = std::thread::spawn(|| {
        // Initialize EXIT first so its destructor runs after VERSIONS and READS are gone.
        EXIT.with(|slot| assert!(slot.borrow().is_none()));
        let tracked = Tracked::new(7u32);
        let (_, reads) = tracked_reads(|| *tracked);
        // Trigger VERSIONS initialization too.
        drop(Tracked::new(0u8));
        EXIT.with(|slot| *slot.borrow_mut() = Some(OnExit { tracked, reads }));
    });
    handle.join().expect("thread with TLS teardown Tracked operations exited cleanly");
}

#[test]
fn tracked_versions_and_reads_track_writes_and_drops() {
    let mut value = Tracked::new(10u32);
    assert_eq!(Tracked::version(&value), 0);

    let (seen, reads) = tracked_reads(|| *value);
    assert_eq!(seen, 10);
    assert!(!reads.changed());

    *value = 20;
    assert_eq!(Tracked::version(&value), 1);
    assert!(reads.changed());

    let (_, after_write) = tracked_reads(|| *value);
    assert!(!after_write.changed());

    Tracked::touch(&value);
    assert_eq!(Tracked::version(&value), 2);
    assert!(after_write.changed());

    let (_, before_drop) = tracked_reads(|| *value);
    assert!(!before_drop.changed());
    let id = Tracked::id(&value);
    drop(value);
    assert_eq!(super::version(id), 3);
    assert!(before_drop.changed());
}
