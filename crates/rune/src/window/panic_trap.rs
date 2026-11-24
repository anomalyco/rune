//! Captures panics at Rune's macOS `ApplicationHandler` boundary so an AppKit delegate or view
//! callback (`extern "C"` in winit) exits the event loop instead of aborting in
//! `panic_cannot_unwind`, and re-raises the original panic once `event_loop.run_app` returns.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(super) type PanicPayload = Box<dyn Any + Send + 'static>;

/// Holds the first panic caught at an event-loop callback boundary until `run_app` returns.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
#[derive(Default)]
pub(super) struct PanicTrap {
    payload: Option<PanicPayload>,
}

#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
impl PanicTrap {
    /// Whether a panic has already been captured; later callbacks and `exiting` skip work.
    #[inline]
    pub(super) fn is_panicking(&self) -> bool {
        self.payload.is_some()
    }

    /// Stores the first panic payload and returns true; later panics are ignored so the first
    /// root cause is preserved.
    pub(super) fn record(&mut self, payload: PanicPayload) -> bool {
        if self.payload.is_some() {
            return false;
        }
        self.payload = Some(payload);
        true
    }

    /// Runs `f` unless a panic was already captured. If `f` panics, stores the first payload
    /// and invokes `on_panic` (typically `event_loop.exit()`).
    #[cfg_attr(not(test), allow(dead_code))]
    #[inline]
    pub(super) fn guard(&mut self, on_panic: impl FnOnce(), f: impl FnOnce()) {
        if self.payload.is_some() {
            return;
        }
        if let Err(payload) = catch_unwind(AssertUnwindSafe(f)) && self.record(payload) {
            on_panic();
        }
    }

    /// Takes the captured panic payload, if any, to re-raise after `run_app` returns.
    pub(super) fn take(&mut self) -> Option<PanicPayload> {
        self.payload.take()
    }
}

/// Returns whether `mode` is a nested run-loop mode (such as `NSEventTrackingRunLoopMode` or
/// `NSModalPanelRunLoopMode`) where `event_loop.exit()` cannot return from `[NSApp run]` on its
/// own without waiting for the tracking or modal loop to finish. `None` (during launch before the
/// run loop starts) and `kCFRunLoopDefaultMode` can exit normally.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
pub(super) fn is_nested_run_loop_mode(mode: Option<&str>) -> bool {
    matches!(mode, Some(mode) if mode != "kCFRunLoopDefaultMode")
}

/// On macOS, returns the main `CFRunLoop`'s current mode if it is a nested tracking or modal mode
/// where `event_loop.exit()` cannot return from `run_app` without hanging. Called only after a
/// panic is caught, never on the non-panicking path.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(super) fn nested_run_loop_mode() -> Option<String> {
    use std::ffi::c_void;
    use objc2_foundation::NSString;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRunLoopGetMain() -> *mut c_void;
        fn CFRunLoopCopyCurrentMode(rl: *mut c_void) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }

    // SAFETY: called on the AppKit main thread inside a caught panic; `CFRunLoopCopyCurrentMode`
    // returns null or a retained `CFStringRef` (toll-free bridged to `NSString`) released once.
    let mode = unsafe {
        let raw = CFRunLoopCopyCurrentMode(CFRunLoopGetMain());
        if raw.is_null() {
            return None;
        }
        let name = (&*(raw as *const NSString)).to_string();
        CFRelease(raw);
        name
    };
    is_nested_run_loop_mode(Some(&mode)).then_some(mode)
}

#[cfg(test)]
#[path = "panic_trap_tests.rs"]
mod tests;
