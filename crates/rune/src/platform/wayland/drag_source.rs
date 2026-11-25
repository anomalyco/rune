//! Native Wayland `wl_data_source` drag-out backend sharing `platform::wayland::drop`'s
//! per-display `wl_data_device` per `wl_seat` without patching winit.
//!
//! Reuses the shared per-seat serial tracker (`platform::wayland::serial`) and shared
//! non-blocking bounded pipe writer (`platform::wayland::pipe_write`).

pub(crate) mod icon;
pub(crate) mod machine;
pub(crate) mod mime;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod wire;

#[cfg(test)]
#[path = "drag_source/machine_tests.rs"]
mod machine_tests;
#[cfg(test)]
#[path = "drag_source/icon_tests.rs"]
mod icon_tests;
#[cfg(all(test, target_os = "linux"))]
#[path = "drag_source/sway_e2e_tests.rs"]
mod sway_e2e_tests;
