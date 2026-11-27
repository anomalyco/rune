//! Keyboard focus for an overlay on a window of its own (X11, Windows), what the macOS panel
//! and those windows report as `nonactivating_keys`, and whether a shown overlay currently holds
//! the keyboard from turn to turn (so a Wayland layer surface recreated while shown asks for
//! keyboard interactivity in its first commit). Kept cross-platform so it compiles and runs
//! tests on every host. As with the macOS panel and a layer-shell surface, `KeyPolicy::OnDemand`
//! lets the window take the keys when `OverlayHandle::request_keys` asks or the user clicks it,
//! and showing alone never does; `KeyPolicy::None` never takes them.

use super::requests::{KeyRequest, Visibility};
use crate::{KeyPolicy, OverlaySupport};

/// Whether a shown overlay currently holds the keyboard (from `OverlayHandle::request_keys`,
/// cleared by `release_keys` or hiding). Preserved across Wayland layer-surface recreation so
/// moving a key-holding overlay to another display never maps its new surface without the keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum KeyHold {
    #[default]
    Released,
    Held,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyHoldEvent {
    #[cfg_attr(not(test), allow(dead_code))]
    Take,
    Release,
}

impl KeyHold {
    pub(crate) fn is_held(self) -> bool {
        matches!(self, KeyHold::Held)
    }

    pub(crate) fn step(&mut self, event: KeyHoldEvent) {
        *self = match event {
            KeyHoldEvent::Take => KeyHold::Held,
            KeyHoldEvent::Release => KeyHold::Released,
        };
    }
}

/// Whether the overlay will be shown after applying this turn's `visibility` request, starting
/// from `was_shown` (before any temporary hide performed to recreate a Wayland layer surface).
pub(in crate::overlay) fn will_be_shown(was_shown: bool, visibility: Option<Visibility>) -> bool {
    match visibility {
        Some(Visibility::Show(_)) => true,
        Some(Visibility::Hide(_)) => false,
        None => was_shown,
    }
}

/// Target `KeyHold` state for this event-loop turn before creating or showing a surface.
/// A shown key-taking overlay remembers `was_held` across turns (including display-move layer
/// recreation with no new `request_keys`); hiding or `release_keys` clears it, and `request_keys`
/// while hidden without a show does nothing.
pub(in crate::overlay) fn next_key_hold(
    was_held: bool,
    will_show: bool,
    policy: KeyPolicy,
    request: Option<KeyRequest>,
) -> KeyHold {
    if !will_show || !policy.takes_keys() {
        return KeyHold::Released;
    }
    match request {
        Some(KeyRequest::Take) => KeyHold::Held,
        Some(KeyRequest::Release) => KeyHold::Released,
        None if was_held => KeyHold::Held,
        None => KeyHold::Released,
    }
}

/// Whether a live Wayland layer surface needs an explicit `request_keys` / `release_keys` call
/// at the end of `sync`. Surfaces newly created during this turn (`!had_live_layer`) already
/// committed their initial keyboard interactivity in `LayerSurface::new`, so they must not
/// send a duplicate `set_keyboard_interactivity` request.
#[cfg_attr(
    not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
    allow(dead_code)
)]
pub(in crate::overlay) fn should_apply_layer_key_request(
    had_live_layer: bool,
    request: Option<KeyRequest>,
    shown: bool,
    policy: KeyPolicy,
) -> Option<KeyRequest> {
    if !had_live_layer || !shown || !policy.takes_keys() {
        return None;
    }
    request
}

/// Whether an overlay window with `policy` may take keyboard focus. Windows makes one that may
/// without `WS_EX_NOACTIVATE`; X11 gives it the focus on `request_keys` and on a click.
#[cfg_attr(not(any(target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) fn can_take_keys(policy: KeyPolicy) -> bool {
    policy.takes_keys()
}

/// What an overlay with `policy` reports as `OverlayCapabilities::nonactivating_keys`.
/// `focuser`: the platform piece that focuses it is there (X11's own connection, which a server
/// without the SHAPE extension doesn't get; always there for the macOS panel and on Windows).
#[cfg_attr(not(any(target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) fn keys_support(policy: KeyPolicy, focuser: bool) -> OverlaySupport {
    if can_take_keys(policy) && focuser { OverlaySupport::Supported } else { OverlaySupport::Unsupported }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
