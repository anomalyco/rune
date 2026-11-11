//! What a drain of other threads' requests asks of the window: one decision, `WakeWork`, from
//! the window as the drain finds it and what its updates found.

/// What a drain asks of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WakeWork {
    /// Nothing: the drain neither polls nor draws.
    Nothing,
    /// A hidden launcher renders what changed into its cached frame, paced (`refresh_hidden`).
    HiddenFrame,
    /// A shown window that can't draw — covered, or minimised — runs its turn without a
    /// drawable (`covered_frame`).
    CoveredFrame,
    /// A shown window draws.
    Paint,
}

/// The window as a drain finds it, and what its updates found.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WakeFacts {
    /// The window exists to be asked.
    pub(crate) window: bool,
    /// A macOS launcher, which keeps a cached frame while hidden.
    pub(crate) launcher: bool,
    /// It shows now (`Life::shown`).
    pub(crate) visible: bool,
    /// It showed when the drain began, before this turn's requests.
    pub(crate) was_shown: bool,
    /// An update changed state the view reads.
    pub(crate) changed: bool,
    /// Only updates that changed nothing woke it.
    pub(crate) quiet: bool,
    /// The shown window is covered (`Life::covered`).
    pub(crate) covered: bool,
}

impl WakeWork {
    /// What this drain asks of the window, decided once. A hidden launcher renders a change into
    /// its cached frame however quiet the turn; any other window draws only for a turn that is
    /// not quiet. A window that can't draw — covered, or minimised — runs its turn without a
    /// drawable, and a launcher's cached frame is paced. `minimized` is asked only when the
    /// answer could be a frame, so a quiet turn reads nothing of the window.
    pub(crate) fn of(f: WakeFacts, minimized: impl FnOnce() -> bool) -> WakeWork {
        if !f.window { return WakeWork::Nothing; }
        if f.launcher && !f.visible {
            return if f.changed { WakeWork::HiddenFrame } else { WakeWork::Nothing };
        }
        if f.quiet { return WakeWork::Nothing; }
        // A shown launcher also draws as it shows; any other window draws because it shows.
        let asked = if f.launcher { f.changed || (f.was_shown && f.visible) } else { f.visible };
        if !asked { return WakeWork::Nothing; }
        if (!f.launcher && f.covered) || minimized() { WakeWork::CoveredFrame } else { WakeWork::Paint }
    }
}
