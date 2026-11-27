//! Native Wayland compositor presentation timestamps (`wp_presentation` / `presentation-time`).
//!
//! Associates one `wp_presentation_feedback` object with each drawn GPU frame's `wl_surface`
//! commit (just before `gpu.queue.present`), mapping compositor-reported `presented` timestamps
//! from the compositor's `clock_id` domain onto `crate::time::Instant` and `discarded` events to
//! `None`.
//!
//! The clock-conversion math and bounded in-flight state machine (`machine`) compile and run
//! unit tests on every platform. The Wayland wire backend (`wire`) compiles on desktop Unix
//! targets only, sharing one per-display controller with zero background threads and waking winit
//! via `wayland_wake` only while feedback objects are in flight.

pub(crate) mod machine;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod wire;

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) use wire::{SurfacePresentation, shutdown};

#[cfg(test)]
#[path = "presentation/tests.rs"]
mod tests;
