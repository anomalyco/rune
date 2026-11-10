//! Where a window is in its life: not shown yet, on screen, hidden, or closed for good. The
//! native host and a `--desktop` run's headless window share it, so a close or a show means the
//! same in both. The native host's `Win` holds the window's resources while it has them: a
//! hidden window may have let them go (`App::release_gpu_when_hidden`, Wayland's unmapping).

/// A window's life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Life {
    /// Not shown yet: created hidden, not created, or its creation failed.
    #[default]
    Unopened,
    /// On screen, possibly minimised. `covered`: an occlusion event, or a drawable refused as
    /// occluded, says none of it shows.
    Shown { covered: bool },
    /// Hidden after it showed, `at` in `reopen::stamp` order.
    Hidden { at: u64 },
    /// Closed without `keep_running`, or its `--frames` trace ended: it never shows again.
    Finished,
}

impl Life {
    pub(crate) fn shown(self) -> bool { matches!(self, Life::Shown { .. }) }

    /// Shown, and on screen as far as anything says.
    pub(crate) fn uncovered(self) -> bool { self == Life::Shown { covered: false } }

    pub(crate) fn covered(self) -> bool { self == Life::Shown { covered: true } }

    pub(crate) fn finished(self) -> bool { self == Life::Finished }

    /// When it last hid, for reopening.
    pub(crate) fn hidden_at(self) -> Option<u64> {
        match self {
            Life::Hidden { at } => Some(at),
            _ => None,
        }
    }

    /// Comes on screen, uncovered. False for a window closed for good, which stays closed.
    pub(crate) fn show(&mut self) -> bool {
        if self.finished() { return false; }
        *self = Life::Shown { covered: false };
        true
    }

    /// Leaves the screen; a window that wasn't shown keeps its state and stamp.
    pub(crate) fn hide(&mut self, stamp: impl FnOnce() -> u64) {
        if self.shown() { *self = Life::Hidden { at: stamp() }; }
    }

    /// A shown window's occlusion changed. Returns whether it was covered. A window off screen
    /// has nothing to cover: showing uncovers it.
    pub(crate) fn cover(&mut self, covered: bool) -> bool {
        let Life::Shown { covered: was } = self else { return false };
        std::mem::replace(was, covered)
    }

    pub(crate) fn finish(&mut self) { *self = Life::Finished; }

    /// Whether a close runs `App::on_close`: the window is shown, or it isn't and
    /// `keep_running` doesn't keep it. A window hidden without its native window closes as
    /// one that kept it does; one never created (`exists`) has nothing to close.
    pub(crate) fn closes(self, keep_running: bool, exists: bool) -> bool {
        match self {
            Life::Shown { .. } => true,
            Life::Hidden { .. } => !keep_running,
            Life::Unopened => exists && !keep_running,
            Life::Finished => false,
        }
    }
}

#[cfg(test)]
#[path = "life_tests.rs"]
mod tests;
