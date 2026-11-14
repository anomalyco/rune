//! Windows: what a window does once its first frame is up runs one piece a turn, and none while
//! input waits. The pieces: the app's `on_ready`, the drop target and session-lock hooks, the GPU's
//! start, and the accessibility adapter. Run together, in the first idle turn, they held the UI
//! thread for 30-50 ms, so a click or key that came as the first frame showed waited behind all of
//! them. Between pieces winit takes what's in the queue, and the frame it asks for comes first.
use super::*;

/// The pieces still owed: the app's `on_ready`, then the window's hooks. The GPU's start and the
/// adapter are owed while their own states say so (`GpuJob::Waiting`, `A11yStart::AfterFirstFrame`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Chores {
    pub(super) ready: bool,
    pub(super) hooks: bool,
}

/// Whether the user's input (a key, the mouse, raw input) is waiting in this thread's queue, so
/// the next piece waits for the turn after winit has taken it. Posted messages don't count:
/// winit's own wakes are posted, and they keep coming while the app starts.
pub(super) fn input_waiting() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetQueueStatus, QS_INPUT};
    // SAFETY: no arguments beyond the flags; the high word is what's in the queue now.
    let status = unsafe { GetQueueStatus(QS_INPUT) };
    status >> 16 != 0
}
impl<S: 'static> Host<S> {
    /// One owed piece, unless input waits; true while more are owed, so the loop turns again.
    pub(super) fn post_frame_turn(&mut self) -> bool {
        if !self.post_frame_owed() { return false; }
        if input_waiting() { return true; }
        if self.chores.ready {
            self.chores = Chores { ready: false, hooks: true };
            self.run_on_ready();
        } else if self.chores.hooks {
            self.chores.hooks = false;
            self.run_ready_hooks();
        } else if self.gpu_owed() {
            self.cpu_idle();
        } else {
            self.a11y_after_frame();
        }
        self.post_frame_owed()
    }

    fn post_frame_owed(&self) -> bool {
        self.chores.ready || self.chores.hooks || self.gpu_owed() || self.a11y_owed()
    }
}
