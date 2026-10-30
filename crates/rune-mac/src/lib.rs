//! Small macOS helpers for Rune apps. Pair them with Rune's `scripts/mac-app`, which relaunches a
//! rebuilt app without its window blinking or taking focus.
//!
//! Off macOS every helper is a no-op: `remember_frame` returns true (so a `poll` that waits on it
//! stops), `saved_size` returns `None`, `is_full_screen` returns false, and `toggle_full_screen`,
//! `launch` and `busy` do nothing. `notify` posts nothing and reports `available() == false`.

pub mod notify;
mod quiet;
pub use quiet::launch;

/// Has AppKit save the app's window frame under `name` and put the next launch's window in the
/// same place, so a rebuild's new window lands exactly over the old one. Call it every frame
/// (from `poll`) until it returns true: the window doesn't exist during `setup`.
pub fn remember_frame(name: &str) -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::{class, msg_send, runtime::AnyObject};
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut AnyObject = msg_send![app, windows];
        let n: usize = msg_send![windows, count];
        let name = std::ffi::CString::new(name).unwrap_or_default();
        let name: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
        for i in 0..n {
            let w: *mut AnyObject = msg_send![windows, objectAtIndex: i];
            let _: bool = msg_send![w, setFrameAutosaveName: name];
        }
        n > 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = name;
        true
    }
}

/// The key window, else the main window; null with neither.
#[cfg(target_os = "macos")]
unsafe fn key_window() -> *mut objc2::runtime::AnyObject {
    use objc2::{class, msg_send, runtime::AnyObject};
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let w: *mut AnyObject = msg_send![app, keyWindow];
        if w.is_null() { msg_send![app, mainWindow] } else { w }
    }
}

/// Takes the app's key window into macOS full screen, or back out (the green button's
/// ⌃⌘F). Headless, with no window, it does nothing.
pub fn toggle_full_screen() {
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::{msg_send, runtime::AnyObject};
        let w = key_window();
        if !w.is_null() {
            let _: () = msg_send![w, toggleFullScreen: std::ptr::null::<AnyObject>()];
        }
    }
}

/// Whether the app's key window is in macOS full screen.
pub fn is_full_screen() -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::{msg_send, runtime::AnyObject};
        let w = key_window();
        if !w.is_null() {
            let mask: usize = msg_send![w, styleMask];
            // NSWindowStyleMaskFullScreen
            return mask & (1 << 14) != 0;
        }
    }
    false
}

/// The size `remember_frame(name)` saved, in points, to open the window at. Applying the saved
/// frame after the first layout resizes it, and the layout animates to the new size at launch.
pub fn saved_size(name: &str) -> Option<(f32, f32)> {
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::{class, msg_send, runtime::AnyObject};
        let key = std::ffi::CString::new(format!("NSWindow Frame {name}")).ok()?;
        let key: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: key.as_ptr()];
        let defaults: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let value: *mut AnyObject = msg_send![defaults, stringForKey: key];
        if value.is_null() {
            return None;
        }
        let utf8: *const std::ffi::c_char = msg_send![value, UTF8String];
        parse_size(std::ffi::CStr::from_ptr(utf8).to_str().ok()?)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = name;
        None
    }
}

/// AppKit's frame string, "x y width height" then the screen's: the width and height.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_size(frame: &str) -> Option<(f32, f32)> {
    let mut n = frame.split_whitespace().skip(2).map(str::parse::<f32>);
    let (w, h) = (n.next()?.ok()?, n.next()?.ok()?);
    (w >= 1.0 && h >= 1.0).then_some((w, h))
}

#[cfg(test)]
mod tests;

/// Holds `scripts/mac-app`'s relaunch while `on`, for work a relaunch would lose (a recording in
/// progress, say). The marker is this process's pid in `~/Library/Caches/mac-app/busy/`, so only
/// this process holds the swap and a crash can't hold it forever.
pub fn busy(on: bool) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else { return };
    let dir = std::path::PathBuf::from(home).join("Library/Caches/mac-app/busy");
    let path = dir.join(std::process::id().to_string());
    if on {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(path, "");
    } else {
        let _ = std::fs::remove_file(path);
    }
}
