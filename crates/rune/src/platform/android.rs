//! Native Android host (`RuneView` / `RuneActivity` over JNI and `ANativeWindow`).
//!
//! The pure lifecycle state machine (`machine`), in-activity page stack (`stack`), document
//! picker state (`picker`), text input (`input`), touch translation (`touch`), and haptic
//! constant mapper (`haptics`) compile on every test target so surface loss, repeated
//! background trips, rotation, insets, keyboard overlap, dark mode, `open_window` stacking,
//! `WindowHandle` routing, `on_close_request`, `on_close`, Escape-first Back navigation,
//! document picking, touch input, text input, and API-level haptic dispatch are verified
//! off-device too.

#[cfg(target_os = "android")]
mod a11y;
#[cfg(target_os = "android")]
pub(crate) mod bridge;
#[cfg(target_os = "android")]
mod document;
pub(crate) mod editor;
#[cfg(test)]
mod editor_tests;
pub(crate) mod haptics;
#[cfg(target_os = "android")]
mod host;
pub(crate) mod ime_window;
#[cfg(test)]
mod ime_window_tests;
pub(crate) mod input;
#[cfg(test)]
mod input_tests;
#[cfg(target_os = "android")]
mod logcat;
pub(crate) mod machine;
#[cfg(test)]
mod machine_tests;
mod page;
pub(crate) mod picker;
#[cfg(test)]
mod picker_tests;
pub(crate) mod selection;
#[cfg(target_os = "android")]
mod selection_jni;
#[cfg(test)]
mod selection_tests;
pub(crate) mod stack;
#[cfg(test)]
mod stack_tests;
#[cfg(target_os = "android")]
mod surface;
pub(crate) mod touch;
#[cfg(test)]
mod touch_tests;

#[cfg(target_os = "android")]
pub(crate) use bridge::{clipboard_revision, open_url};
#[cfg(target_os = "android")]
pub use host::jni_on_load;
#[cfg(target_os = "android")]
pub(crate) use host::{notify_opened, run, run_windows};

/// `winit`'s Android backend links `android-activity`'s `NativeActivity` glue, whose exported
/// `ANativeActivity_onCreate` calls `android_main`. Rune hosts Android apps through
/// `dev.rune.RuneView` (`JNI_OnLoad`), so that glue never runs; this no-op satisfies the
/// symbol so Android's dynamic loader accepts the `.so`.
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
fn android_main(_: winit::platform::android::activity::AndroidApp) {}
