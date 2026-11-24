//! How the effect-warm worker keeps out of the frames' way: it compiles only once the window
//! rests, below the priority of the threads that draw, and it shares with the frames which
//! effect each side builds, so that no pipeline is compiled twice and a frame that needs one
//! mid-compile waits for the rest of that compile at its own priority.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use super::PrewarmEffects;

/// No frame prepared for this long: the window rests, and the worker may compile.
pub(super) const QUIET: Duration = Duration::from_millis(300);
/// A window that never rests is warmed after this long all the same.
pub(super) const LIMIT: Duration = Duration::from_secs(5);

/// What one worker and the renderer's frames share.
#[derive(Default)]
pub(super) struct Link {
    /// Frames prepared since the worker started: while it moves, the window is drawing.
    frames: AtomicU64,
    progress: Mutex<Progress>,
}

#[derive(Default)]
struct Progress {
    /// Effects the worker has begun (or finished) building.
    started: PrewarmEffects,
    /// Effects a frame needed before the worker began them: the frame builds those itself.
    claimed: PrewarmEffects,
}

impl Link {
    /// A frame was prepared.
    pub(super) fn frame(&self) {
        self.frames.fetch_add(1, Ordering::Relaxed);
    }

    /// The worker is about to build `effect`: false when a frame claimed it first.
    pub(super) fn start(&self, effect: PrewarmEffects) -> bool {
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        if progress.claimed.contains(effect) { return false; }
        progress.started |= effect;
        true
    }

    /// A frame needs `effect` now. True when the worker has begun it: the frame waits for the
    /// rest of that build. False when it has not: the frame builds it, and the worker skips it.
    pub(super) fn begun_or_claim(&self, effect: PrewarmEffects) -> bool {
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        if progress.started.contains(effect) { return true; }
        progress.claimed |= effect;
        false
    }
}

/// How long the worker still waits before it compiles: `None` once the window has prepared no
/// frame for `quiet`, or once `limit` has passed since the worker started.
pub(super) fn rest_left(still: Duration, waited: Duration, quiet: Duration, limit: Duration) -> Option<Duration> {
    if still >= quiet || waited >= limit { return None; }
    Some((quiet - still).min(limit - waited))
}

/// Parks the worker until the window rests ([`rest_left`]). False when cancelled meanwhile;
/// whoever cancels unparks the worker, so the wait ends at once.
pub(super) fn rest(link: &Link, cancel: &AtomicBool, quiet: Duration, limit: Duration) -> bool {
    let began = crate::time::now();
    let mut still = began;
    let mut seen = link.frames.load(Ordering::Relaxed);
    loop {
        if cancel.load(Ordering::Acquire) { return false; }
        let frames = link.frames.load(Ordering::Relaxed);
        if frames != seen {
            seen = frames;
            still = crate::time::now();
        }
        match rest_left(still.elapsed(), began.elapsed(), quiet, limit) {
            None => return true,
            Some(left) => std::thread::park_timeout(left),
        }
    }
}

#[cfg(target_vendor = "apple")]
#[allow(unsafe_code)]
mod qos {
    pub(super) const UTILITY: u32 = 0x11;
    pub(super) const USER_INTERACTIVE: u32 = 0x21;
    unsafe extern "C" {
        pub(super) fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
        pub(super) fn pthread_override_qos_class_start_np(
            thread: std::os::unix::thread::RawPthread,
            class: u32,
            relative_priority: i32,
        ) -> *mut std::ffi::c_void;
        pub(super) fn pthread_override_qos_class_end_np(token: *mut std::ffi::c_void) -> i32;
    }
}

/// Runs the calling worker thread below the threads that draw: below normal on Windows (as
/// `variant::start_pinned`), the utility QoS class on Apple systems. Elsewhere it keeps its
/// priority: an unprivileged Linux thread that lowered itself could not be lifted again for a
/// frame that waits on it ([`lift`]).
#[allow(unsafe_code)]
pub(super) fn lower() {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        // SAFETY: `GetCurrentThread` returns the calling thread's pseudo-handle.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
    }
    #[cfg(target_vendor = "apple")]
    {
        // SAFETY: sets the calling thread's own QoS class.
        unsafe { qos::pthread_set_qos_class_self_np(qos::UTILITY, 0) };
    }
}

/// A frame's priority lent to the worker ([`lift`]); given back when dropped.
pub(super) struct Lift {
    #[cfg(target_vendor = "apple")]
    token: *mut std::ffi::c_void,
}

/// Lends the worker `thread` the priority of the frame that waits for it. On Windows the worker
/// lowers itself again before its next effect ([`lower`]); on Apple systems the override ends
/// when the returned [`Lift`] drops.
#[allow(unsafe_code)]
pub(super) fn lift(thread: &JoinHandle<()>) -> Lift {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Threading::{SetThreadPriority, THREAD_PRIORITY_NORMAL};
        // SAFETY: the worker's own handle, open as long as `thread` is.
        unsafe { SetThreadPriority(thread.as_raw_handle(), THREAD_PRIORITY_NORMAL) };
        Lift {}
    }
    #[cfg(target_vendor = "apple")]
    {
        use std::os::unix::thread::JoinHandleExt;
        // SAFETY: `thread` is not joined yet, so its pthread is valid; the token is ended in `Drop`.
        let token = unsafe { qos::pthread_override_qos_class_start_np(thread.as_pthread_t(), qos::USER_INTERACTIVE, 0) };
        Lift { token }
    }
    #[cfg(not(any(target_os = "windows", target_vendor = "apple")))]
    {
        let _ = thread;
        Lift {}
    }
}

#[cfg(target_vendor = "apple")]
#[allow(unsafe_code)]
impl Drop for Lift {
    fn drop(&mut self) {
        if self.token.is_null() { return; }
        // SAFETY: the token `pthread_override_qos_class_start_np` returned, ended once.
        unsafe { qos::pthread_override_qos_class_end_np(self.token) };
    }
}
