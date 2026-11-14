//! Windows: what a window does once its first frame is up runs one piece a turn, and none while
//! input waits. The pieces: the app's `on_ready`, the drop target and session-lock hooks, the GPU's
//! start, and the accessibility adapter. Run together, in the first idle turn, they held the UI
//! thread for 30-50 ms, so a click or key that came as the first frame showed waited behind all of
//! them. Between pieces winit takes what's in the queue, and the frame it asks for comes first.
use super::*;

/// The window's own pieces still owed: the app's `on_ready`, then the window's hooks, then none.
/// The GPU's start and the adapter are owed while their own states say so (`GpuJob::Waiting`,
/// `A11yStart::AfterFirstFrame`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Chores {
    #[default]
    Done,
    Ready,
    Hooks,
}

/// Whether a key or mouse message is waiting in this thread's queue, so the next piece waits for
/// the turn after winit has taken it: the user's input, or one another app posted (an assistive
/// tool, a test). winit's own wakes are posted messages outside those ranges, and they keep
/// coming while the app starts, so they don't count. Sent messages aren't dispatched here.
pub(super) fn input_waiting() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_NOREMOVE, PM_QS_INPUT, PM_QS_POSTMESSAGE, PeekMessageW, WM_KEYFIRST, WM_KEYLAST, WM_MOUSEFIRST, WM_MOUSELAST};
    [(WM_KEYFIRST, WM_KEYLAST), (WM_MOUSEFIRST, WM_MOUSELAST)].into_iter().any(|(first, last)| {
        // SAFETY: `msg` is written only; PM_NOREMOVE leaves the message in the queue, and the
        // PM_QS_* flags keep sent messages from being dispatched.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            PeekMessageW(&mut msg, std::ptr::null_mut(), first, last, PM_NOREMOVE | PM_QS_INPUT | PM_QS_POSTMESSAGE) != 0
        }
    })
}

impl<S: 'static> Host<S> {
    /// One owed piece, unless input waits; true while more are owed, so the loop turns again.
    pub(super) fn post_frame_turn(&mut self) -> bool {
        if !self.post_frame_owed() { return false; }
        if input_waiting() { return true; }
        match self.chores {
            Chores::Ready => {
                self.chores = Chores::Hooks;
                self.run_on_ready();
            }
            Chores::Hooks => {
                self.chores = Chores::Done;
                self.run_ready_hooks();
            }
            Chores::Done if self.gpu_owed() => self.cpu_idle(),
            Chores::Done => self.a11y_after_frame(),
        }
        self.post_frame_owed()
    }

    fn post_frame_owed(&self) -> bool {
        self.chores != Chores::Done || self.gpu_owed() || self.a11y_owed()
    }
}
