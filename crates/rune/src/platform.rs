#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod activation;
#[cfg(target_os = "macos")]
pub(crate) mod display_link;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod layer_shell;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "macos")]
pub(crate) mod main_queue;
#[cfg(all(target_os = "linux", feature = "dbus"))]
pub(crate) mod portal_appearance;
#[cfg(target_arch = "wasm32")]
pub mod web;
#[cfg(target_os = "windows")]
pub(crate) mod win32;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod x11;
