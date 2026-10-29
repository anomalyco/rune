//! Relaunching behind the app in front. winit activates every app it starts, `open -g` or not, so
//! an unattended rebuild took focus, and a window manager that follows focus switched to the
//! app's workspace. `scripts/mac-app` launches with `MAC_APP_QUIET` set when
//! the app wasn't in front, and `launch` keeps the app from activating while it starts.

const VAR: &str = "MAC_APP_QUIET";

/// Call first thing in `main`, before the event loop.
#[expect(clippy::disallowed_methods, reason = "reads MAC_APP_QUIET handoff variable at startup")]
pub fn launch() {
    if std::env::var_os(VAR).is_none() {
        return;
    }
    // SAFETY: called first in `main`, before the app starts any thread.
    unsafe { std::env::remove_var(VAR) };
    #[cfg(target_os = "macos")]
    // SAFETY: called on the main thread before winit builds the event loop.
    unsafe {
        use objc2::{class, msg_send, runtime::AnyObject};
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        // NSApplicationActivationPolicyProhibited: winit's activation at launch does nothing.
        let _: bool = msg_send![app, setActivationPolicy: 2isize];
        after(0);
    }
}

/// Launched that way, macOS hides the app within a few hundred milliseconds. Once it has (or
/// after 5 s), the app turns regular again and shows its window without activating, so it lands
/// behind whatever is in front.
#[cfg(target_os = "macos")]
extern "C" fn check(tries: *mut std::ffi::c_void) {
    // SAFETY: dispatched onto the main queue by `after`; `tries` carries an integer count.
    unsafe {
        use objc2::{class, msg_send, runtime::AnyObject};
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let hidden: bool = msg_send![app, isHidden];
        let tries = tries as usize;
        if !hidden && tries < 20 {
            return after(tries + 1);
        }
        let _: bool = msg_send![app, setActivationPolicy: 0isize];
        let _: () = msg_send![app, unhideWithoutActivation];
    }
}

#[cfg(target_os = "macos")]
fn after(tries: usize) {
    // SAFETY: `_dispatch_main_q` is the static main queue and `check` has matching C ABI.
    unsafe { dispatch_after_f(dispatch_time(0, 250_000_000), &raw const _dispatch_main_q, tries as *mut std::ffi::c_void, check) };
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    static _dispatch_main_q: std::ffi::c_void;
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_after_f(when: u64, queue: *const std::ffi::c_void, context: *mut std::ffi::c_void, work: extern "C" fn(*mut std::ffi::c_void));
}
