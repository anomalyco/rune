//! Native Wayland file drops via `wl_data_device` / `wl_data_offer`.
//!
//! The drag state machine, RFC 2483 URI-list decoder, surface routing table, and bounded pipe-read
//! loop live in `machine` so they compile and run unit tests on every platform; the Wayland wire
//! binding and reader thread live in `wire` on desktop Unix targets.

pub(crate) mod machine;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod pipe;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod wire;

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) use wire::{DropTarget, install, take};

#[cfg(test)]
#[path = "wayland_drop/tests.rs"]
mod tests;
