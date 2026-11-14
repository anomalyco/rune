//! macOS glyph rasterization: CoreGraphics' smoothing pipeline, reproduced.

pub(super) mod smoothing;

/// Whether the user lets text smooth: false only when `AppleFontSmoothing` reads 0 for this app
/// (its own domain or the global one), as AppKit, SwiftUI and GPUI read it. A string counts as
/// its number, as `NSUserDefaults` reads one.
#[cfg(target_os = "macos")]
pub(super) fn smoothing_allowed_by_user() -> bool {
    use objc2_core_foundation::{CFNumber, CFPreferencesCopyAppValue, CFString, kCFPreferencesCurrentApplication};
    // SAFETY: A constant CoreFoundation exports, valid for the life of the process.
    let app = unsafe { kCFPreferencesCurrentApplication };
    let Some(value) = CFPreferencesCopyAppValue(&CFString::from_str("AppleFontSmoothing"), app) else { return true };
    let level = match value.downcast::<CFNumber>() {
        Ok(n) => n.as_i64(),
        Err(value) => value.downcast::<CFString>().ok().and_then(|s| s.to_string().trim().parse::<i64>().ok()),
    };
    level != Some(0)
}
