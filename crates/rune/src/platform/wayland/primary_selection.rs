//! Native Wayland primary selection via `zwp_primary_selection_unstable_v1`.
//!
//! Binds `zwp_primary_selection_device_manager_v1` lazily after frame 1 on winit's shared
//! `wl_display`, reuses the shared unwatched seat serial tracker (`platform::wayland::serial`),
//! bounded pipe reader (`platform::wayland::pipe`), and non-blocking bounded pipe writer
//! (`platform::wayland::pipe_write`), and plugs into `window::wayland_wake` strictly while our
//! own primary-selection source is owned (zero idle wakeups otherwise).
//!
//! The pure device/offer/source state machine and MIME/encoding selection live in `machine` so
//! every transition runs headless unit tests on every platform.

pub(crate) mod machine;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod wire;

#[cfg(all(test, target_os = "linux"))]
mod sway_e2e_tests;
