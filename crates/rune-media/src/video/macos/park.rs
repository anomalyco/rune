//! Parked paused player pool: keeps at most `PARKED_PLAYERS` native players for fast resume
//! and evicts the oldest without dropping clip owners while holding the pool lock.

use crate::video::Shared;
use crate::video::phases::{Event, step};
use std::sync::{Arc, Mutex, Weak};

/// Paused players that keep a native player (and its decoder) for a quick resume. Older paused
/// players are released; a video's last frame stays on screen and resuming recreates it.
const PARKED_PLAYERS: usize = 4;

/// Paused players holding a native player, oldest first.
static PARKED: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());

pub(super) fn unpark(shared: &Arc<Shared>) {
    PARKED.lock().unwrap().retain(|w| w.strong_count() > 0 && !std::ptr::eq(w.as_ptr(), Arc::as_ptr(shared)));
}

/// Keep at most `PARKED_PLAYERS` paused players; release the oldest. Main thread only.
pub(super) fn park(shared: &Arc<Shared>) {
    for clip in parked_with(shared) {
        // A video's last frame stays on its surface; resuming recreates the player.
        let player = {
            let mut native = clip.native.lock().unwrap();
            step(&mut clip.ctl.lock().unwrap(), Event::Parked);
            native.player.take()
        };
        drop(player);
    }
}

/// Parks `shared` and returns the clips to release: the oldest past `PARKED_PLAYERS`. A clip
/// that plays again (before its next pull unparks it) leaves the list and is never released.
pub(in crate::video) fn parked_with(shared: &Arc<Shared>) -> Vec<Arc<Shared>> {
    // A clip being edited on another thread right now counts as playing.
    let playing = |clip: &Shared| clip.ctl.try_lock().map_or(true, |c| c.playing());
    // Every upgrade outlives PARKED: dropping the last owner here would run the clip's
    // destructor, its app waker captures possibly playing another parked clip and re-entering
    // unpark. The buffer is taken out for the pass, so it keeps its capacity and a pass over a
    // parked clip that changes nothing allocates nothing.
    let mut held = HELD.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
    let mut parked = PARKED.lock().unwrap();
    parked.retain(|w| match w.upgrade() {
        Some(clip) => {
            let keep = Arc::ptr_eq(&clip, shared) || !playing(&clip);
            held.push(clip);
            #[cfg(test)]
            kept();
            keep
        }
        None => false,
    });
    if !parked.iter().any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(shared))) {
        parked.push(Arc::downgrade(shared));
    }
    let excess = parked.len().saturating_sub(PARKED_PLAYERS);
    let released = parked.drain(..excess).filter_map(|w| w.upgrade()).collect();
    drop(parked);
    // The last owners drop here, after the lock. Empty rather than dropped, so the capacity
    // survives: a destructor that parks again on this thread gets the empty buffer.
    held.clear();
    HELD.with(|slot| *slot.borrow_mut() = held);
    released
}

thread_local! {
    /// The parking pass's upgraded owners, kept between passes so a warm redraw allocates nothing.
    static HELD: std::cell::RefCell<Vec<Arc<Shared>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Test-only: the scratch buffer's capacity, which a pass that allocates nothing leaves alone.
#[cfg(test)]
pub(in crate::video) fn held_capacity() -> usize {
    HELD.with(|slot| slot.borrow().capacity())
}

/// Test-only: runs once inside the next parking pass, while PARKED is held, so a test can drop a
/// clip's last owner at the moment the pass holds an upgrade.
#[cfg(test)]
pub(in crate::video) fn on_kept(hook: impl FnOnce() + 'static) {
    KEPT.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
thread_local! {
    static KEPT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn kept() {
    let hook = KEPT.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

/// Test-only: whether PARKED is free right now, without waiting for it.
#[cfg(test)]
pub(in crate::video) fn parked_free() -> bool {
    PARKED.try_lock().is_ok()
}
