//! A player pushes frames with its own locks held, and a surface's waker runs app code that
//! may read that player. While a [`Held`] lives on a thread, a push there keeps the frame it
//! replaced and its waker, and the outermost `Held` drops the frames and calls the wakers once
//! it goes, after the player's locks are released.

use super::Frame;
use std::cell::RefCell;
use std::sync::Arc;

#[derive(Default)]
struct Deferred {
    depth: u32,
    frames: Vec<Arc<Frame>>,
    wakes: Vec<crate::Waker>,
}

thread_local! {
    static DEFERRED: RefCell<Deferred> = RefCell::default();
}

/// Holds surface wakes and replaced frames on this thread until it drops. Create it before
/// taking a player's locks and drop it after releasing them, with no app code between.
pub(crate) struct Held(std::marker::PhantomData<*const ()>);

impl Held {
    pub(crate) fn new() -> Self {
        DEFERRED.with_borrow_mut(|d| d.depth += 1);
        Held(std::marker::PhantomData)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let taken = DEFERRED.try_with(|d| {
            let mut d = d.borrow_mut();
            d.depth -= 1;
            (d.depth == 0 && !(d.frames.is_empty() && d.wakes.is_empty())).then(|| (std::mem::take(&mut d.frames), std::mem::take(&mut d.wakes)))
        });
        let Ok(Some((mut frames, mut wakes))) = taken else { return };
        frames.clear();
        for wake in wakes.drain(..) {
            wake();
        }
        // Keep the buffers, so a push on a later pull doesn't allocate.
        let _ = DEFERRED.try_with(|d| {
            let mut d = d.borrow_mut();
            if d.frames.is_empty() {
                d.frames = frames;
            }
            if d.wakes.is_empty() {
                d.wakes = wakes;
            }
        });
    }
}

/// Keeps `old` and `wake` for the outermost [`Held`] on this thread, or hands them back when
/// none is alive.
pub(super) fn defer(old: Option<Arc<Frame>>, wake: Option<crate::Waker>) -> Option<(Option<Arc<Frame>>, Option<crate::Waker>)> {
    let mut pair = Some((old, wake));
    let _ = DEFERRED.try_with(|d| {
        let mut d = d.borrow_mut();
        if d.depth > 0
            && let Some((old, wake)) = pair.take()
        {
            d.frames.extend(old);
            d.wakes.extend(wake);
        }
    });
    pair
}
