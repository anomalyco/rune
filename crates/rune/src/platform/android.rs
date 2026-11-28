//! Native Android host (`RuneView` / `RuneActivity` over JNI and `ANativeWindow`).
//!
//! The pure lifecycle state machine (`machine`) compiles on every test target so surface loss,
//! repeated background trips, rotation, insets, keyboard overlap, dark mode, and back navigation
//! are verified off-device too.

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
