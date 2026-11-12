//! What a window's taskbar button says about the work behind it: a progress state and a count
//! badge, set with [`crate::WindowHandle::set_taskbar`], and a bounded flash for something that
//! needs the person ([`crate::WindowHandle::flash`]).
//!
//! Windows draws both on the window's taskbar button (`ITaskbarList3`): the progress bar under it,
//! and the badge as an overlay icon over its corner. Explorer animates an indeterminate bar
//! itself, so a window showing work wakes no more than one showing nothing. Nothing is made
//! until the first call that shows something: a window that never sets a state never starts
//! the taskbar's COM object. Explorer restarting gives every window a new button; each gets
//! its state again.
//!
//! macOS shows the badge on the Dock icon, for the app as a whole: the sum of its windows'
//! counts (`NSDockTile.badgeLabel`); the Dock has no progress bar, so progress shows nothing there.
//! Linux's counterpart, Unity's `LauncherEntry` over D-Bus, isn't built: the calls do nothing.

/// The bar under a window's taskbar button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TaskbarProgress {
    /// No bar.
    #[default]
    None,
    /// Work runs with no known end: Windows' indeterminate bar.
    Busy,
    /// Work waits on the person: a full amber bar.
    Paused,
    /// Work failed: a full red bar.
    Error,
}

/// A window's taskbar state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Taskbar {
    pub(crate) progress: TaskbarProgress,
    /// The count shown, 0 for none, and what a screen reader says for it.
    pub(crate) badge: (u32, String),
}

impl Taskbar {
    pub fn new() -> Self { Self::default() }

    pub fn progress(mut self, progress: TaskbarProgress) -> Self {
        self.progress = progress;
        self
    }

    /// A count over the button's corner (above 9 it reads "9+"); 0 shows none. `description` is
    /// what the taskbar tells a screen reader, such as "2 sessions need you".
    pub fn badge(mut self, count: u32, description: impl Into<String>) -> Self {
        self.badge = (count, if count == 0 { String::new() } else { description.into() });
        self
    }

    pub fn count(&self) -> u32 { self.badge.0 }
    pub fn shown_progress(&self) -> TaskbarProgress { self.progress }
}
