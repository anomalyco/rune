//! Native Wayland compositor behind-window blur (`ext_background_effect_v1`, with
//! `org_kde_kwin_blur_manager` fallback on KDE Plasma 5.x–6.6.x).
//!
//! The eligibility policy, protocol state machine (`policy`), and surface-local logical region
//! geometry (`region`) compile and run unit tests on every platform behind a narrow
//! `WaylandBlurSeam` trait. The Wayland wire backend (`wire`) compiles on desktop Unix targets
//! only, sharing one per-display controller with zero background threads.

pub(crate) mod policy;
pub(crate) mod region;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod wire;

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) use wire::{SurfaceBlur, shutdown};

#[cfg(test)]
#[path = "wayland_backdrop/tests.rs"]
mod tests;
