//! Windows: when a window's swap chain can take its next frame. It holds at most one frame the
//! compositor hasn't shown (`FRAME_LATENCY`), and its waitable object is signalled when it can
//! take another. wgpu doesn't wait on it (`Dx12UseFrameLatencyWaitableObject::DontWait`): a
//! frame that blocked in `acquire` would be built from the input that came before the wait, and
//! the input that came during it would wait a frame more. Instead a window that wants a frame
//! while the swap chain is full asks a thread-pool wait to wake the loop when it isn't; the loop
//! then handles every input that came meanwhile, and the frame is built from all of it. Nothing
//! waits, and no thread runs, while the window draws nothing.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{BOOLEAN, CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, RegisterWaitForSingleObject, UnregisterWaitEx, WT_EXECUTEINWAITTHREAD, WT_EXECUTEONLYONCE, WaitForSingleObject,
};

/// How long a registered wait waits before it gives the frame anyway: a swap chain that was
/// made again or lost never signals the object the wait holds.
const GIVE_UP_MS: u32 = 100;

/// One window's swap chain, as the next frame sees it.
#[derive(Default)]
pub(super) enum Slot {
    /// No frame of ours is queued (none yet, or the swap chain was made again).
    #[default]
    Free,
    /// A frame was presented: the next one waits until the object says the queue has room.
    Owed,
    /// Owed, and the wait that asks for the next frame is registered (dropping it unregisters it).
    Waiting(#[allow(dead_code)] Wait),
}

/// What the window's next frame does (`Slot::take`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Take {
    /// Draw now.
    Draw,
    /// Don't: the registered wait asks for the frame (`WakeEvent::FrameSlot`).
    Later,
}

impl Slot {
    /// The frame about to draw takes the swap chain's room, or waits for it. `waitable` is the
    /// swap chain's object; `wake` asks the loop for the frame from the thread-pool thread.
    pub(super) fn take(&mut self, waitable: Option<HANDLE>, wake: impl Fn() + Send + 'static) -> Take {
        match self {
            Slot::Free => Take::Draw,
            Slot::Waiting(_) => Take::Later,
            Slot::Owed => {
                let Some(object) = waitable else { *self = Slot::Free; return Take::Draw };
                // SAFETY: the swap chain's live object; a zero timeout only polls it (and takes its
                // count when it's signalled).
                if unsafe { WaitForSingleObject(object, 0) } == WAIT_OBJECT_0 {
                    *self = Slot::Free;
                    return Take::Draw;
                }
                match Wait::register(object, Box::new(wake)) {
                    Some(wait) => { *self = Slot::Waiting(wait); Take::Later }
                    // No thread-pool wait: draw, and let `acquire` find what it finds.
                    None => { *self = Slot::Free; Take::Draw }
                }
            }
        }
    }

    /// The registered wait came back: the swap chain has room (its count is ours now).
    pub(super) fn came_back(&mut self) {
        if matches!(self, Slot::Waiting(_)) { *self = Slot::Free; }
    }

    /// A frame was presented.
    pub(super) fn presented(&mut self) { *self = Slot::Owed; }

    /// The swap chain was configured again or replaced: nothing of ours is queued on it.
    pub(super) fn reset(&mut self) { *self = Slot::Free; }
}

/// A one-shot thread-pool wait on a copy of the swap chain's object, so wgpu closing its handle
/// (a reconfigure makes a new one) never leaves the wait on a closed handle.
pub(super) struct Wait {
    wait: HANDLE,
    object: HANDLE,
    wake: *mut Box<dyn Fn() + Send>,
}

impl Wait {
    fn register(object: HANDLE, wake: Box<dyn Fn() + Send>) -> Option<Wait> {
        let mut copy: HANDLE = std::ptr::null_mut();
        // SAFETY: duplicates a live handle of this process into this process.
        let ok = unsafe { DuplicateHandle(GetCurrentProcess(), object, GetCurrentProcess(), &mut copy, 0, 0, DUPLICATE_SAME_ACCESS) };
        if ok == 0 { return None; }
        let wake = Box::into_raw(Box::new(wake));
        let mut wait: HANDLE = std::ptr::null_mut();
        // SAFETY: `wake` lives until `drop` has unregistered the wait and its callback is done;
        // the callback only calls it.
        let ok = unsafe { RegisterWaitForSingleObject(&mut wait, copy, Some(came), wake.cast(), GIVE_UP_MS, WT_EXECUTEONLYONCE | WT_EXECUTEINWAITTHREAD) };
        if ok == 0 {
            // SAFETY: both are ours and unused.
            unsafe { drop(Box::from_raw(wake)); CloseHandle(copy); }
            return None;
        }
        Some(Wait { wait, object: copy, wake })
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        // SAFETY: waits for a running callback to finish, so `wake` is no longer in use.
        unsafe {
            UnregisterWaitEx(self.wait, INVALID_HANDLE_VALUE);
            drop(Box::from_raw(self.wake));
            CloseHandle(self.object);
        }
    }
}

unsafe extern "system" fn came(context: *mut c_void, _timed_out: BOOLEAN) {
    // SAFETY: the `Box` `register` made, alive until the wait is unregistered.
    let wake = unsafe { &*(context as *const Box<dyn Fn() + Send>) };
    wake();
}
