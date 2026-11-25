//! macOS native full-screen transition state machine and titlebar-inset guard.
//!
//! AppKit posts four public `NSWindow` notifications around a native Space transition:
//! `NSWindowWillEnterFullScreenNotification`, `NSWindowDidEnterFullScreenNotification`,
//! `NSWindowWillExitFullScreenNotification`, and `NSWindowDidExitFullScreenNotification`.
//! Following Chromium's `NativeWidgetMac::IsFullscreen()` (`target_fullscreen_state_`), Rune
//! flips `Cx::is_full_screen()` at the `Will*` notifications so custom titlebars adapt before
//! AppKit's Space animation starts, and reconciles on `Did*` and OS state sync when a
//! transition settles or fails.
#![cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]

use crate::machine::Effects;

#[cfg(test)]
#[path = "full_screen_tests.rs"]
mod tests;

/// Full-screen transition phase of a macOS window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::window) enum State {
    #[default]
    Windowed,
    Entering,
    FullScreen,
    Exiting,
}

impl State {
    /// Initial phase from the live window's winit and AppKit full-screen flags.
    pub(in crate::window) const fn from_os(winit_full_screen: bool, style_mask_full_screen: bool) -> Self {
        if style_mask_full_screen {
            Self::FullScreen
        } else if winit_full_screen {
            Self::Entering
        } else {
            Self::Windowed
        }
    }

    /// Whether the window is entering or in full screen (`Cx::is_full_screen`).
    pub(in crate::window) const fn is_full_screen(self) -> bool {
        matches!(self, Self::Entering | Self::FullScreen)
    }
}

/// Events driving the macOS full-screen transition state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum Event {
    /// `NSWindowWillEnterFullScreenNotification`.
    WillEnter,
    /// `NSWindowDidEnterFullScreenNotification`.
    DidEnter,
    /// `NSWindowWillExitFullScreenNotification`.
    WillExit,
    /// `NSWindowDidExitFullScreenNotification`.
    DidExit,
    /// Observed OS window state from `sync_chrome_core`: winit's `window.fullscreen().is_some()`
    /// and AppKit's `NSWindowStyleMask::FullScreen`. Reconciles missed notifications and failed
    /// enter transitions where winit's `windowDidFailToEnterFullScreen:` clears `fullscreen`
    /// without a `DidExit` notification.
    Sync { winit_full_screen: bool, style_mask_full_screen: bool },
}

/// Side effect requested by a transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum Effect {
    /// Mark full-screen chrome dirty and ask AppKit's content view to redraw so
    /// `sync_chrome_core` updates `Cx::is_full_screen`, `Cx::titlebar_inset`, and
    /// `Cx::chrome_insets`.
    Redraw,
}

/// Advances the macOS full-screen transition state machine.
pub(in crate::window) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let next = match (*state, event) {
        (State::Windowed | State::Exiting, Event::WillEnter) => State::Entering,
        (State::Entering | State::FullScreen, Event::WillEnter) => return Effects::none(),

        (State::Windowed | State::Entering | State::Exiting, Event::DidEnter) => State::FullScreen,
        (State::FullScreen, Event::DidEnter) => return Effects::none(),

        (State::FullScreen | State::Entering, Event::WillExit) => State::Exiting,
        (State::Windowed | State::Exiting, Event::WillExit) => return Effects::none(),

        (State::Entering | State::FullScreen | State::Exiting, Event::DidExit) => State::Windowed,
        (State::Windowed, Event::DidExit) => return Effects::none(),

        (State::Windowed, Event::Sync { style_mask_full_screen: true, .. }) => State::FullScreen,
        (State::Windowed, Event::Sync { style_mask_full_screen: false, .. }) => return Effects::none(),

        (
            State::Entering | State::FullScreen | State::Exiting,
            Event::Sync { winit_full_screen: false, style_mask_full_screen: false },
        ) => State::Windowed,
        (State::Entering | State::FullScreen | State::Exiting, Event::Sync { .. }) => return Effects::none(),
    };
    *state = next;
    Effects::one(Effect::Redraw)
}

/// Whether `Win::is_full_screen()` reports full screen for a macOS window. Adopted panels never
/// enter full screen.
pub(in crate::window) const fn window_is_full_screen(state: State, panel: bool) -> bool {
    !panel && state.is_full_screen()
}

/// Target full-screen state for `Tx::toggle_full_screen()`. Returns `None` for adopted panels;
/// otherwise flips the current transition target so mid-transition toggles reverse direction.
pub(in crate::window) const fn toggle_target(state: State, panel: bool) -> Option<bool> {
    if panel { None } else { Some(!state.is_full_screen()) }
}

/// Whether `mac::titlebar_inset` may measure the traffic-light zoom button rect.
///
/// Returns `false` (yielding `0.0`) while `Entering` or `FullScreen` (even before AppKit sets
/// `NSWindowStyleMask::FullScreen`), while AppKit still reports `NSWindowStyleMask::FullScreen`
/// (including while `Exiting` before buttons return), when `FullSizeContentView` is absent, when
/// the button is hidden, or when the zoom button is attached to a different window (such as
/// AppKit's detached full-screen titlebar window during `Exiting`).
pub(in crate::window) const fn can_read_titlebar_inset(
    state: State,
    style_mask_full_screen: bool,
    full_size_content_view: bool,
    button_visible: bool,
    button_in_window: bool,
) -> bool {
    !state.is_full_screen()
        && !style_mask_full_screen
        && full_size_content_view
        && button_visible
        && button_in_window
}
