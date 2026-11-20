#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::CreateEventW;

use super::{Slot, Take};

struct Event(HANDLE);

impl Event {
    fn new(signalled: bool) -> Self {
        // SAFETY: anonymous manual-reset event owned by this guard.
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, i32::from(signalled), std::ptr::null()) };
        assert!(!handle.is_null());
        Self(handle)
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: closes the handle created in `Event::new`.
        unsafe { CloseHandle(self.0); }
    }
}

#[test]
fn slot_resets_when_swap_chain_waitable_changes_or_disappears() {
    let old_waitable = Event::new(false);
    let new_waitable = Event::new(true);

    let mut slot = Slot::default();
    slot.presented(1);
    assert_eq!(slot.take(1, Some(old_waitable.0), || {}), Take::Later, "unsignalled swap chain waits");
    assert!(slot.waiting());

    // Swap chain was reconfigured or replaced (new waitable object, or surface lost/none).
    assert_eq!(
        slot.take(1, Some(new_waitable.0), || {}),
        Take::Draw,
        "a new swap chain must not stay blocked on the replaced swap chain's wait"
    );
    assert!(!slot.waiting());

    slot.presented(2);
    assert_eq!(slot.take(2, Some(old_waitable.0), || {}), Take::Later);
    assert_eq!(
        slot.take(2, None, || {}),
        Take::Draw,
        "losing the waitable handle must clear the stale wait"
    );
    assert!(!slot.waiting());
}

#[test]
fn slot_resets_when_surface_configure_generation_advances() {
    let waitable = Event::new(false);
    let mut slot = Slot::default();

    // Waiting on generation 1, then swap chain reconfigured (generation 2) with the same handle.
    slot.presented(1);
    assert_eq!(slot.take(1, Some(waitable.0), || {}), Take::Later);
    assert!(slot.waiting());
    assert_eq!(slot.take(2, Some(waitable.0), || {}), Take::Draw);
    assert!(!slot.waiting());

    // Owed on generation 2, then swap chain reconfigured (generation 3) before next take.
    slot.presented(2);
    assert_eq!(slot.take(3, Some(waitable.0), || {}), Take::Draw);
    assert!(!slot.waiting());

    // Explicit reset clears both Owed and Waiting immediately.
    slot.presented(3);
    assert_eq!(slot.take(3, Some(waitable.0), || {}), Take::Later);
    slot.reset();
    assert!(!slot.waiting());
    assert_eq!(slot.take(3, Some(waitable.0), || {}), Take::Draw);
}

#[test]
fn slot_draws_immediately_when_signalled_or_after_wait_completes() {
    let signalled = Event::new(true);
    let unsignalled = Event::new(false);
    let mut slot = Slot::default();

    slot.presented(1);
    assert_eq!(slot.take(1, Some(signalled.0), || {}), Take::Draw);

    slot.presented(1);
    assert_eq!(slot.take(1, Some(unsignalled.0), || {}), Take::Later);
    slot.came_back();
    assert!(!slot.waiting());
    assert_eq!(slot.take(1, Some(unsignalled.0), || {}), Take::Draw);
}
