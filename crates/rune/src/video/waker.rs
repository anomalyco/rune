//! The app's waker on a media handle ([`Player`](crate::Player), [`Recorder`](crate::Recorder)),
//! told once per change of the handle's state.

use std::sync::Mutex;

/// The app's waker and the generation it last heard. The generation it compares against is the
/// handle's own, so a change made while no waker was set is told when one is.
#[derive(Default)]
pub(crate) struct WakerSlot(Mutex<Option<(crate::Waker, u64)>>);

impl WakerSlot {
    /// Installs `waker`, which hears the next [`tell`](Self::tell) of any generation but 0.
    pub(crate) fn set(&self, waker: crate::Waker) {
        // The replaced waker's captures drop after the lock: one may read this handle and would
        // deadlock here.
        let old = self.0.lock().unwrap().replace((waker, 0));
        drop(old);
    }

    /// Calls the waker if `generation` is news to it. Call with no other locks held: the waker
    /// runs app code.
    pub(crate) fn tell(&self, generation: u64) {
        let waker = match &mut *self.0.lock().unwrap() {
            Some((waker, heard)) if *heard != generation => {
                *heard = generation;
                Some(waker.clone())
            }
            _ => None,
        };
        if let Some(waker) = waker {
            waker();
        }
    }

    /// Whether the slot's lock is free right now, without waiting for it.
    #[cfg(test)]
    pub(crate) fn free(&self) -> bool {
        self.0.try_lock().is_ok()
    }
}
