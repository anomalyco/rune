//! Where a window is in its life: not shown yet, on screen, hidden, or closed for good. The
//! native host and a `--desktop` run's headless window share it, so a close or a show means the
//! same in both. The native host's `Win` holds the window's resources while it has them: a
//! hidden window may have let them go (`App::release_gpu_when_hidden`, Wayland's unmapping).

use super::sight::{Cover, Sight};

/// A window's life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Life {
    /// Not shown yet: created hidden, not created, or its creation failed.
    #[default]
    Unopened,
    /// On screen, possibly minimised, and whether anything says none of it shows (`Sight`).
    Shown { sight: Sight },
    /// Hidden after it showed, `at` in `reopen::stamp` order.
    Hidden { at: u64 },
    /// Closed without `keep_running`, or its `--frames` trace ended: it never shows again.
    Finished,
}

impl Life {
    pub(crate) fn shown(self) -> bool { matches!(self, Life::Shown { .. }) }

    /// Shown, and on screen as far as anything says.
    pub(crate) fn uncovered(self) -> bool { self == Life::Shown { sight: Sight::Seen } }

    pub(crate) fn covered(self) -> bool { matches!(self, Life::Shown { sight: Sight::Covered(_) }) }

    /// Shown, and `cover` says none of it shows.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn covered_by(self, cover: Cover) -> bool { matches!(self, Life::Shown { sight } if sight.has(cover)) }

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
        *self = Life::Shown { sight: Sight::Seen };
        true
    }

    /// Leaves the screen; a window that wasn't shown keeps its state and stamp.
    pub(crate) fn hide(&mut self, stamp: impl FnOnce() -> u64) {
        if self.shown() { *self = Life::Hidden { at: stamp() }; }
    }

    /// `cover` starts or stops saying a shown window can't be seen. Returns whether it was
    /// covered. A window off screen has nothing to cover: showing uncovers it.
    pub(crate) fn cover(&mut self, cover: Cover, on: bool) -> bool {
        let Life::Shown { sight } = self else { return false };
        let was = sight.covered();
        *sight = sight.step(cover, on);
        was
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
