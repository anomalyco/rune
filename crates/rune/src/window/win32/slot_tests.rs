#![allow(unsafe_code)]

use std::sync::mpsc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
use windows_sys::Win32::System::Threading::{CreateEventW, CreateSemaphoreW, GetCurrentProcess, ReleaseSemaphore};

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

struct Semaphore(HANDLE);

impl Semaphore {
    fn new(initial: i32, max: i32) -> Self {
        // SAFETY: anonymous semaphore owned by this guard.
        let handle = unsafe { CreateSemaphoreW(std::ptr::null(), initial, max, std::ptr::null()) };
        assert!(!handle.is_null());
        Self(handle)
    }

    fn duplicate(&self) -> Self {
        let mut copy: HANDLE = std::ptr::null_mut();
        // SAFETY: duplicates a live semaphore handle into this process, modelling DXGI's
        // `GetFrameLatencyWaitableObject` returning a new handle to the same kernel semaphore
        // after `ResizeBuffers`.
        let ok = unsafe { DuplicateHandle(GetCurrentProcess(), self.0, GetCurrentProcess(), &mut copy, 0, 0, DUPLICATE_SAME_ACCESS) };
        assert_ne!(ok, 0);
        Self(copy)
    }

    fn release(&self, count: i32) {
        // SAFETY: releases `count` units on the live semaphore.
        let ok = unsafe { ReleaseSemaphore(self.0, count, std::ptr::null_mut()) };
        assert_ne!(ok, 0);
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        // SAFETY: closes the handle created in `Semaphore::new` or `Semaphore::duplicate`.
        unsafe { CloseHandle(self.0); }
    }
}

#[test]
fn slot_keeps_waiting_and_owed_across_resize_reconfigure() {
    let sem = Semaphore::new(0, 64);
    let resized_handle = sem.duplicate();
    let (tx, rx) = mpsc::channel();
    let mut slot = Slot::default();

    // A frame was presented and the swap chain is full: take registers a wait.
    slot.presented();
    assert_eq!(slot.take(Some(sem.0), move || { let _ = tx.send(()); }), Take::Later);
    assert!(slot.waiting());

    // A resize calls `ResizeBuffers` on the same swap chain: `reconfigured(true)` keeps the
    // wait, and a new handle to the same kernel semaphore must not cancel it.
    slot.reconfigured(true);
    assert!(slot.waiting());
    assert_eq!(
        slot.take(Some(resized_handle.0), || {}),
        Take::Later,
        "an in-place ResizeBuffers reconfigure must keep waiting on the same swap chain"
    );
    assert!(slot.waiting());

    // When the swap chain retires the frame, the registered wait still fires.
    sem.release(1);
    assert!(rx.recv_timeout(Duration::from_millis(500)).is_ok(), "the wait on the duplicated semaphore must still wake");
    slot.came_back();
    assert!(!slot.waiting());
    assert_eq!(slot.take(Some(resized_handle.0), || {}), Take::Draw);

    // Owed across a ResizeBuffers reconfigure must still wait while the semaphore is unsignalled.
    slot.presented();
    slot.reconfigured(true);
    assert_eq!(
        slot.take(Some(resized_handle.0), || {}),
        Take::Later,
        "an owed frame must still wait after an in-place ResizeBuffers reconfigure"
    );
    assert!(slot.waiting());
}

#[test]
fn slot_drains_surplus_semaphore_counts_on_owed_take() {
    let sem = Semaphore::new(5, 64);
    let mut slot = Slot::default();

    // Surplus semaphore counts (e.g. left after a reset or a timed-out wait) are drained when
    // `Slot::Owed` polls a signalled object, so later frames wait for the compositor again.
    slot.presented();
    assert_eq!(slot.take(Some(sem.0), || {}), Take::Draw);
    slot.presented();
    assert_eq!(
        slot.take(Some(sem.0), || {}),
        Take::Later,
        "surplus semaphore counts must be drained so the next frame waits for the compositor"
    );
    assert!(slot.waiting());

    // The drain loop is bounded, so a manual-reset event that stays signalled terminates cleanly.
    let manual = Event::new(true);
    slot.reset();
    slot.presented();
    assert_eq!(slot.take(Some(manual.0), || {}), Take::Draw);
}

#[test]
fn slot_clears_state_on_explicit_reset_failed_reconfigure_or_lost_waitable() {
    let unsignalled = Event::new(false);
    let mut slot = Slot::default();

    // Explicit reset (swap-chain replacement or drop) clears Waiting immediately.
    slot.presented();
    assert_eq!(slot.take(Some(unsignalled.0), || {}), Take::Later);
    assert!(slot.waiting());
    slot.reset();
    assert!(!slot.waiting());
    assert_eq!(slot.take(Some(unsignalled.0), || {}), Take::Draw);

    // Failed configure drops DX12's swap chain and clears both Waiting and Owed.
    slot.presented();
    assert_eq!(slot.take(Some(unsignalled.0), || {}), Take::Later);
    slot.reconfigured(false);
    assert!(!slot.waiting());
    assert_eq!(slot.take(Some(unsignalled.0), || {}), Take::Draw);

    slot.presented();
    slot.reconfigured(false);
    assert_eq!(slot.take(Some(unsignalled.0), || {}), Take::Draw);

    // Losing the waitable handle clears a stale wait immediately.
    slot.presented();
    assert_eq!(slot.take(Some(unsignalled.0), || {}), Take::Later);
    assert_eq!(slot.take(None, || {}), Take::Draw, "losing the waitable handle must clear the stale wait");
    assert!(!slot.waiting());
}

#[test]
fn slot_draws_immediately_when_signalled_or_after_wait_completes() {
    let signalled = Event::new(true);
    let unsignalled = Event::new(false);
    let mut slot = Slot::default();

    slot.presented();
    assert_eq!(slot.take(Some(signalled.0), || {}), Take::Draw);

    slot.presented();
    assert_eq!(slot.take(Some(unsignalled.0), || {}), Take::Later);
    slot.came_back();
    assert!(!slot.waiting());
    assert_eq!(slot.take(Some(unsignalled.0), || {}), Take::Draw);
}
