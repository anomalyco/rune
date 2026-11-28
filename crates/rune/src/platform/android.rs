//! Native Android host (`RuneView` / `RuneActivity` over JNI and `ANativeWindow`).
//!
//! The pure lifecycle state machine (`machine`) and haptic constant mapper (`haptics`) compile on
//! every test target so surface loss, repeated background trips, rotation, insets, keyboard
//! overlap, dark mode, back navigation, and API-level haptic dispatch are verified off-device too.

pub(crate) mod haptics;
#[cfg(target_os = "android")]
pub(crate) mod bridge;
#[cfg(target_os = "android")]
mod host;
#[cfg(target_os = "android")]
mod logcat;
pub(crate) mod machine;
#[cfg(test)]
mod machine_tests;

#[cfg(target_os = "android")]
pub(crate) use bridge::{clipboard_revision, open_url};
#[cfg(target_os = "android")]
pub use host::jni_on_load;
#[cfg(target_os = "android")]
pub(crate) use host::run;

/// `winit`'s Android backend links `android-activity`'s `NativeActivity` glue, whose exported
/// `ANativeActivity_onCreate` calls `android_main`. Rune hosts Android apps through
/// `dev.rune.RuneView` (`JNI_OnLoad`), so that glue never runs; this no-op satisfies the
/// symbol so Android's dynamic loader accepts the `.so`.
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
fn android_main(_: winit::platform::android::activity::AndroidApp) {}
