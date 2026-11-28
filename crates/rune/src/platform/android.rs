//! Native Android host (`RuneView` / `RuneActivity` over JNI and `ANativeWindow`).
//!
//! The pure lifecycle state machine (`machine`) and haptic constant mapper (`haptics`) compile on
//! every test target so surface loss, repeated background trips, rotation, insets, keyboard
//! overlap, dark mode, back navigation, and API-level haptic dispatch are verified off-device too.

pub(crate) mod haptics;
#[cfg(target_os = "android")]
mod host;
#[cfg(target_os = "android")]
mod logcat;
pub(crate) mod machine;
#[cfg(test)]
mod machine_tests;

#[cfg(target_os = "android")]
pub use host::jni_on_load;
#[cfg(target_os = "android")]
pub(crate) use host::run;
