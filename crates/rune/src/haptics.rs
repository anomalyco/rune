//! System haptic feedback: [`Tx::haptic`](crate::Tx::haptic).
//!
//! A handler asks for a [`Haptic`] by what happened (a value stepped, something snapped, an
//! outcome), not by a platform pattern, so view code stays the same on every platform. The
//! request is taken once the handler's frame is done.
//!
//! Who plays it:
//!
//! - An embedding host (`embed::LayerHost` on Apple platforms) takes the requests after each
//!   frame (or at any time: a request alone doesn't ask for a frame) with `LayerHost::take_haptics` (or [`Runtime::take_haptics`](crate::Runtime::take_haptics))
//!   and plays them with the platform's generators (`UISelectionFeedbackGenerator`,
//!   `UIImpactFeedbackGenerator`, `UINotificationFeedbackGenerator` on iOS), which also follow the
//!   user's haptics setting.
//! - Headless runs and tests keep them for `Runtime::take_haptics`, so a test can assert on them.
//! - Desktop windows, overlays and the web drop them: they have no player yet.
//!
//! The queue holds at most 32 requests; a host that never takes them loses the
//! oldest, never memory.

use std::collections::VecDeque;

/// Requests kept for the host before the oldest is dropped.
pub(crate) const MAX_PENDING: usize = 32;

/// Haptic feedback a handler asks for, by what happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Haptic {
    /// A value changed by one step: a picker or slider detent, a tab or segment change
    /// (iOS `UISelectionFeedbackGenerator`; Android `CLOCK_TICK` / `SEGMENT_TICK`).
    Selection,
    /// Something landed or snapped: a sheet resting on a stop, a long press opening a menu
    /// (iOS `UIImpactFeedbackGenerator`; Android `CONFIRM` / `VIRTUAL_KEY`).
    Impact(ImpactStrength),
    /// An outcome (iOS `UINotificationFeedbackGenerator`; Android `CONFIRM` / `REJECT`).
    Notify(HapticOutcome),
}

/// How hard a [`Haptic::Impact`] lands (`UIImpactFeedbackGenerator.FeedbackStyle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImpactStrength {
    Light,
    Medium,
    Heavy,
    Soft,
    Rigid,
}

/// What a [`Haptic::Notify`] reports (`UINotificationFeedbackGenerator.FeedbackType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HapticOutcome {
    Success,
    Warning,
    Error,
}

/// The runtime's haptic requests waiting for a host to play them.
#[derive(Debug, Default)]
pub(crate) struct Haptics {
    pending: VecDeque<Haptic>,
}

impl Haptics {
    /// Keeps `requests` for the host, or drops them where nothing plays them (`keep` false).
    pub(crate) fn push(&mut self, requests: Vec<Haptic>, keep: bool) {
        if !keep { return; }
        for haptic in requests {
            if self.pending.len() == MAX_PENDING { self.pending.pop_front(); }
            self.pending.push_back(haptic);
        }
    }

    pub(crate) fn take(&mut self) -> Vec<Haptic> { self.pending.drain(..).collect() }
}

impl crate::Tx {
    /// Plays `haptic` after this handler's commit, where the platform and the user's settings
    /// allow it. An embedding host plays what [`Runtime::take_haptics`](crate::Runtime::take_haptics)
    /// hands it; headless runs and tests record the requests there; desktop windows, overlays
    /// and the web play nothing.
    ///
    /// ```
    /// # use rune::{El, Haptic, Tx, el};
    /// # struct App { tab: u8 }
    /// # fn _tab_button() -> El {
    /// el().on_press_tx(|s: &mut App, tx: &mut Tx| {
    ///     s.tab += 1;
    ///     tx.haptic(Haptic::Selection);
    /// })
    /// # }
    /// ```
    pub fn haptic(&mut self, haptic: Haptic) { self.haptics.push(haptic); }
}

impl<S: 'static> crate::Runtime<S> {
    /// Takes the haptics handlers asked for since the last call, oldest first: for an
    /// embedding host to play after each frame, and for tests to assert on. At most 32
    /// are kept; live desktop windows, overlays and the web keep none.
    pub fn take_haptics(&mut self) -> Vec<Haptic> { self.core_mut().host.haptics.take() }
}

#[cfg(test)]
#[path = "haptics_tests.rs"]
mod tests;
