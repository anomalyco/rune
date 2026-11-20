//! Keyboard focus for an overlay on a window of its own (X11, Windows), and what the macOS panel
//! and those windows report as `nonactivating_keys`, kept cross-platform so it compiles and runs
//! tests on every host. As with the macOS panel and a layer-shell surface, `KeyPolicy::OnDemand`
//! lets the window take the keys when `OverlayHandle::request_keys` asks or the user clicks it,
//! and showing alone never does; `KeyPolicy::None` never takes them.

use crate::{KeyPolicy, OverlaySupport};

/// Whether an overlay window with `policy` may take keyboard focus. Windows makes one that may
/// without `WS_EX_NOACTIVATE`; X11 gives it the focus on `request_keys` and on a click.
#[cfg_attr(not(any(target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) fn can_take_keys(policy: KeyPolicy) -> bool {
    policy == KeyPolicy::OnDemand
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
