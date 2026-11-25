//! System haptic feedback: [`Tx::haptic`](crate::Tx::haptic).
//!
//! A handler asks for a [`Haptic`] by what happened (a value stepped, something snapped, an
//! outcome), not by a platform pattern, so view code stays the same on every platform. There is
//! intentionally no `is_available()` query (matching AppKit `NSHapticFeedbackManager`, UIKit
//! `UIFeedbackGenerator`, SwiftUI `SensoryFeedback`, Flutter `HapticFeedback` and Android
//! `HapticFeedbackConstants`, because hardware presence does not reflect System Haptics, Low Power
//! Mode or active input device settings); requests are fire-and-forget no-ops where unsupported.
//!
//! # Timing, cooldown and bounds
//!
//! - Requests made during a frame (including `on_mount` and deferred field menus) are committed
//!   once at the end of the frame so feedback lines up with the presented pixels. Requests made
//!   from [`Runtime::update`](crate::Runtime::update) / [`Runtime::update_if`](crate::Runtime::update_if)
//!   are committed immediately and mark `frame_work` without dirtying the view (compatible with
//!   [`Tx::live_only`](crate::Tx::live_only)).
//! - A 30 ms same-kind cooldown on the runtime clock suppresses repeated identical [`Haptic`]
//!   variants (collapsing bursts such as ten slider detents in one frame to one tick and capping
//!   120 Hz scrubbing at ~33 Hz with identical feel on 60 Hz and 120 Hz displays) while letting
//!   any different [`Haptic`] variant fire immediately.
//! - Pending requests live in a fixed inline 32-entry ring buffer with zero heap allocation; a
//!   host that never drains them drops the oldest entry, never leaking memory.
//!
//! # Platform mapping
//!
//! | Rune [`Haptic`] | iOS (embedding host after `LayerHost::frame()`) | Android (embedding host on main thread via `View.performHapticFeedback`) | macOS (Rune windows & overlays via `NSHapticFeedbackManager`) | Windows / Linux / Web |
//! | --- | --- | --- | --- | --- |
//! | `Selection` | `UISelectionFeedbackGenerator.selectionChanged()` | `SEGMENT_TICK` (API 34+, fallback `CLOCK_TICK`) | `LevelChange` (`PerformanceTime::Now`) | no-op |
//! | `Impact(Light)` | `UIImpactFeedbackGenerator(style: .light, view:).impactOccurred()` | `VIRTUAL_KEY` | `Alignment` (`PerformanceTime::Now`) | no-op |
//! | `Impact(Medium)` | `UIImpactFeedbackGenerator(style: .medium, view:).impactOccurred()` | `KEYBOARD_TAP` | `Generic` (`PerformanceTime::Now`) | no-op |
//! | `Impact(Heavy)` | `UIImpactFeedbackGenerator(style: .heavy, view:).impactOccurred()` | `CONTEXT_CLICK` (API 23+, fallback `LONG_PRESS`) | `Generic` (`PerformanceTime::Now`) | no-op |
//! | `Impact(Soft)` | `UIImpactFeedbackGenerator(style: .soft, view:).impactOccurred()` | `SEGMENT_FREQUENT_TICK` (API 34+, fallback `CLOCK_TICK`) | `Alignment` (`PerformanceTime::Now`) | no-op |
//! | `Impact(Rigid)` | `UIImpactFeedbackGenerator(style: .rigid, view:).impactOccurred()` | `CONTEXT_CLICK` (API 23+, fallback `KEYBOARD_TAP`) | `Generic` (`PerformanceTime::Now`) | no-op |
//! | `Notify(Success)` | `UINotificationFeedbackGenerator(view:).notificationOccurred(.success)` | `CONFIRM` (API 30+, compat fallback `VIRTUAL_KEY`) | no-op | no-op |
//! | `Notify(Warning)` | `UINotificationFeedbackGenerator(view:).notificationOccurred(.warning)` | `KEYBOARD_TAP` | no-op | no-op |
//! | `Notify(Error)` | `UINotificationFeedbackGenerator(view:).notificationOccurred(.error)` | `REJECT` (API 30+, compat fallback `LONG_PRESS`) | no-op | no-op |
//!
//! - **iOS**: prefer iOS 17.5+ view-bound `UIFeedbackGenerator(view:)` initialisers and call
//!   `prepare()` on touch-down or after firing; UIKit honours System Haptics and Low Power Mode.
//! - **Android**: use `HapticFeedbackConstants` / `HapticFeedbackConstantsCompat` on the host
//!   `View` so no `VIBRATE` permission is needed.
//! - **macOS**: Rune's own shown windows and overlays drain and perform haptics right after
//!   presentation (and on unchanged frames for boundary bumps). Hidden, covered or prewarming
//!   windows discard pending haptics. AppKit has no notification patterns, so `Notify(_)` is a
//!   silent no-op matching SwiftUI on macOS.
//! - **Web**: always a no-op; `Navigator.vibrate` is non-Baseline, Android-only raw millisecond
//!   buzzes unsupported on iOS Safari, so Rune never calls it.

/// Requests kept for the host before the oldest is dropped.
pub(crate) const MAX_PENDING: usize = 32;

/// Minimum runtime-clock interval (30 ms) between repeated identical [`Haptic`] requests.
pub(crate) const COOLDOWN_SECS: f64 = 0.030;

/// Haptic feedback a handler asks for, by what happened.
///
/// ```
/// # use rune::{Haptic, HapticOutcome, ImpactStrength};
/// let step = Haptic::Selection;
/// let snap = Haptic::Impact(ImpactStrength::Light);
/// let done = Haptic::Notify(HapticOutcome::Success);
/// assert_ne!(step, snap);
/// assert_ne!(snap, done);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Haptic {
    /// A value changed by one step: a picker or slider detent, a tab or segment change
    /// (iOS `UISelectionFeedbackGenerator`; Android `SEGMENT_TICK` / `CLOCK_TICK`;
    /// macOS `NSHapticFeedbackPattern::LevelChange`).
    Selection,
    /// Something landed or snapped: a sheet resting on a stop, a long press opening a menu
    /// (iOS `UIImpactFeedbackGenerator`; Android `VIRTUAL_KEY` / `KEYBOARD_TAP` /
    /// `CONTEXT_CLICK` / `SEGMENT_FREQUENT_TICK`; macOS `Alignment` or `Generic`).
    Impact(ImpactStrength),
    /// An outcome (iOS `UINotificationFeedbackGenerator`; Android `CONFIRM` / `KEYBOARD_TAP` /
    /// `REJECT`; silent no-op on macOS).
    Notify(HapticOutcome),
}

/// How hard a [`Haptic::Impact`] lands (`UIImpactFeedbackGenerator.FeedbackStyle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImpactStrength {
    /// Light snap (macOS `Alignment`; Android `VIRTUAL_KEY`).
    Light,
    /// Medium snap, e.g. long-press menu (macOS `Generic`; Android `KEYBOARD_TAP`).
    Medium,
    /// Heavy impact (macOS `Generic`; Android `CONTEXT_CLICK` / `LONG_PRESS`).
    Heavy,
    /// Soft cushioned impact (macOS `Alignment`; Android `SEGMENT_FREQUENT_TICK` / `CLOCK_TICK`).
    Soft,
    /// Crisp rigid impact (macOS `Generic`; Android `CONTEXT_CLICK` / `KEYBOARD_TAP`).
    Rigid,
}

/// What a [`Haptic::Notify`] reports (`UINotificationFeedbackGenerator.FeedbackType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HapticOutcome {
    /// Action succeeded (Android `CONFIRM` / `VIRTUAL_KEY`).
    Success,
    /// Caution or non-fatal warning (Android `KEYBOARD_TAP`).
    Warning,
    /// Action rejected or failed (Android `REJECT` / `LONG_PRESS`).
    Error,
}

/// Phase of the per-runtime haptic cooldown state machine on `RuntimeCore::clock`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum CooldownState {
    /// No active cooldown window; any haptic fires immediately.
    #[default]
    Idle,
    /// `last` fired at `since`; repeats of `last` are suppressed until `until`, while any
    /// different [`Haptic`] variant fires immediately and starts a fresh window.
    Cooling { last: Haptic, since: f64, until: f64 },
}

/// Events fed into [`step_cooldown`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CooldownEvent {
    /// A handler requested `haptic` at runtime clock `now`.
    Request { haptic: Haptic, now: f64 },
    /// Runtime clock advanced to `now` (returns `Cooling` to `Idle` once due).
    #[cfg(test)]
    Tick { now: f64 },
}

/// Effects returned by [`step_cooldown`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CooldownEffect {
    /// Enqueue `haptic` for the host to play.
    Emit(Haptic),
}

/// Pure transition for the 30 ms same-kind haptic cooldown on the runtime clock.
pub(crate) fn step_cooldown(
    state: &mut CooldownState,
    event: CooldownEvent,
) -> crate::machine::Effects<CooldownEffect> {
    match event {
        #[cfg(test)]
        CooldownEvent::Tick { now } => {
            if let CooldownState::Cooling { since, until, .. } = *state
                && (now < since || now >= until)
            {
                *state = CooldownState::Idle;
            }
            crate::machine::Effects::none()
        }
        CooldownEvent::Request { haptic, now } => {
            let suppressed = matches!(
                *state,
                CooldownState::Cooling { last, since, until }
                    if last == haptic && now >= since && now < until
            );
            if suppressed {
                crate::machine::Effects::none()
            } else {
                *state = CooldownState::Cooling {
                    last: haptic,
                    since: now,
                    until: now + COOLDOWN_SECS,
                };
                crate::machine::Effects::one(CooldownEffect::Emit(haptic))
            }
        }
    }
}

/// The runtime's haptic cooldown and fixed inline pending ring buffer (zero heap allocation).
#[derive(Debug)]
pub(crate) struct Haptics {
    cooldown: CooldownState,
    ring: [Haptic; MAX_PENDING],
    head: usize,
    len: usize,
}

impl Default for Haptics {
    fn default() -> Self {
        Self {
            cooldown: CooldownState::Idle,
            ring: [Haptic::Selection; MAX_PENDING],
            head: 0,
            len: 0,
        }
    }
}

impl Haptics {
    /// Passes `requests` through the 30 ms same-kind cooldown at runtime clock `now` and appends
    /// surviving haptics into the bounded inline ring buffer (dropping oldest on overflow).
    pub(crate) fn push_at(&mut self, requests: impl IntoIterator<Item = Haptic>, now: f64) {
        for haptic in requests {
            for effect in step_cooldown(&mut self.cooldown, CooldownEvent::Request { haptic, now }) {
                match effect {
                    CooldownEffect::Emit(emitted) => self.enqueue(emitted),
                }
            }
        }
    }

    fn enqueue(&mut self, haptic: Haptic) {
        if self.len == MAX_PENDING {
            self.ring[self.head] = haptic;
            self.head = (self.head + 1) % MAX_PENDING;
        } else {
            let tail = (self.head + self.len) % MAX_PENDING;
            self.ring[tail] = haptic;
            self.len += 1;
        }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool { self.len == 0 }

    #[inline]
    pub(crate) fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Drains all pending haptics in FIFO order into `f` with zero heap allocation.
    #[cfg(any(test, target_os = "macos"))]
    pub(crate) fn drain_into(&mut self, mut f: impl FnMut(Haptic)) {
        let len = self.len;
        let head = self.head;
        self.head = 0;
        self.len = 0;
        for i in 0..len {
            f(self.ring[(head + i) % MAX_PENDING]);
        }
    }

    /// Drains all pending haptics in FIFO order into a `Vec`. Returns an empty `Vec` without
    /// allocating when nothing is queued.
    pub(crate) fn take(&mut self) -> Vec<Haptic> {
        if self.len == 0 {
            return Vec::new();
        }
        let len = self.len;
        let head = self.head;
        self.head = 0;
        self.len = 0;
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            out.push(self.ring[(head + i) % MAX_PENDING]);
        }
        out
    }
}

/// Drains pending haptics for a desktop, overlay or web host. When `play` is true on a live
/// macOS window/overlay (`rt.host.open_urls_natively`), performs them via
/// `NSHapticFeedbackManager`; otherwise (hidden/covered/prewarm/scripted frames, Windows, Linux,
/// Web) discards them with zero heap allocation.
#[inline]
pub(crate) fn flush_live(rt: &mut crate::runtime::RuntimeCore, play: bool) {
    if rt.host.haptics.is_empty() {
        return;
    }
    #[cfg(target_os = "macos")]
    if play && rt.host.open_urls_natively {
        rt.host.haptics.drain_into(crate::platform::macos::haptics::perform);
        return;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = play;
    rt.host.haptics.clear();
}

/// Test seam mirroring [`flush_live`] so headless and macOS unit tests verify visible dispatch
/// vs hidden/covered/scripted discard without touching hardware.
#[cfg(test)]
pub(crate) fn flush_with(
    rt: &mut crate::runtime::RuntimeCore,
    play: bool,
    mut perform: impl FnMut(Haptic),
) {
    if rt.host.haptics.is_empty() {
        return;
    }
    if play && rt.host.open_urls_natively {
        rt.host.haptics.drain_into(&mut perform);
    } else {
        rt.host.haptics.clear();
    }
}

impl crate::Tx {
    /// Plays `haptic` after this handler's frame commits, where the platform and the user's
    /// settings allow it (and silently no-ops elsewhere).
    ///
    /// Identical [`Haptic`] requests within 30 ms on the runtime clock are coalesced; a different
    /// variant fires immediately. Compatible with [`Tx::live_only`](crate::Tx::live_only): a
    /// haptic-only handler does not rebuild the view or re-run layout.
    ///
    /// ```
    /// # use rune::{El, Haptic, Tx, el};
    /// # struct App { detent: u8 }
    /// # fn next_detent(d: u8) -> u8 { (d + 1) % 6 }
    /// # fn _stop_button() -> El {
    /// el().key("stop").on_press_tx(|s: &mut App, tx: &mut Tx| {
    ///     s.detent = next_detent(s.detent);
    ///     tx.haptic(Haptic::Selection);
    /// })
    /// # }
    /// ```
    pub fn haptic(&mut self, haptic: Haptic) { self.haptics.push(haptic); }
}

impl<S: 'static> crate::Runtime<S> {
    /// Takes the haptics handlers asked for since the last call, oldest first: for an
    /// embedding host to play after each frame, and for headless tests to assert on.
    ///
    /// At most 32 requests are kept (oldest dropped first), with a 30 ms same-kind cooldown on
    /// the runtime clock. Shown macOS windows and overlays drain and play them automatically via
    /// `NSHapticFeedbackManager`; Windows, Linux and Web windows drain and discard them.
    ///
    /// ```
    /// # use rune::{Env, Haptic, Runtime, el};
    /// let mut rt = Runtime::new((), |_, _| el().size(10.0, 10.0), (10.0, 10.0), Env::default());
    /// rt.frame(0.0);
    /// rt.update(|_, tx| tx.haptic(Haptic::Selection));
    /// assert_eq!(rt.take_haptics(), [Haptic::Selection]);
    /// assert!(rt.take_haptics().is_empty());
    /// ```
    pub fn take_haptics(&mut self) -> Vec<Haptic> { self.core_mut().host.haptics.take() }
}

#[cfg(test)]
#[path = "haptics_tests.rs"]
mod tests;
