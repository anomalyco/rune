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
        *self.0.lock().unwrap() = Some((waker, 0));
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
}
