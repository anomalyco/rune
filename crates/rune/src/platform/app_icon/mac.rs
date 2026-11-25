//! macOS: shows the chosen icon in the Dock and swaps a light/dark pair when the app's
//! `effectiveAppearance` changes, with or without a window open.

#![allow(unsafe_code)]

use super::{CURRENT, decode};
use objc2::{ClassType, MainThreadMarker, msg_send, rc::autoreleasepool, sel};
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSArray, NSObject, NSString, ns_string};
use std::{cell::Cell, ffi::c_void, sync::OnceLock};

/// What the Dock shows, so a repeated call or an Appearance change that doesn't change the
/// side sets nothing: which request it came from, and for a pair whether it's the dark side.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Shown { asked: u64, dark: Option<bool> }

thread_local! {
    static SHOWN: Cell<Option<Shown>> = const { Cell::new(None) };
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

/// Shows the icon the app asked for as the app's Appearance draws it. Main thread.
pub(super) fn apply() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    autoreleasepool(|_| {
        // Only the bytes are copied under the lock, so `set_app_icon_choice` on another thread
        // never waits for a decode.
        let (shown, bytes) = {
            let current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
            let Some((asked, icon)) = current.as_ref() else { return };
            let follows = icon.follows_appearance();
            let dark = follows && is_dark(&app);
            let shown = Shown { asked: *asked, dark: follows.then_some(dark) };
            if SHOWN.get() == Some(shown) { return; }
            (shown, icon.shown(dark).map(<[u8]>::to_vec))
        };
        if shown.dark.is_some() { watch(&app); }
        let image = match bytes {
            None => None,
            Some(bytes) => match decode(&bytes) { Some(image) => Some(image), None => return },
        };
        SHOWN.set(Some(shown));
        // SAFETY: on the main thread, with an image AppKit decoded or nil for the bundle's icon.
        unsafe { app.setApplicationIconImage(image.as_deref()) };
    });
}

/// Whether the app's effective appearance is dark: the app's own `appearance` when it forces
/// Light or Dark, else the system's.
fn is_dark(app: &NSApplication) -> bool {
    let names = NSArray::from_slice(&[ns_string!("NSAppearanceNameAqua"), ns_string!("NSAppearanceNameDarkAqua")]);
    // SAFETY: main thread; `effectiveAppearance` returns an `NSAppearance`, and
    // `bestMatchFromAppearancesWithNames:` takes an array of names and returns one of them or nil.
    unsafe {
        let appearance: *mut AnyObject = msg_send![app, effectiveAppearance];
        let Some(appearance) = appearance.as_ref() else { return false };
        let best: *mut NSString = msg_send![appearance, bestMatchFromAppearancesWithNames: &*names];
        best.as_ref().is_some_and(|name| name.to_string() == "NSAppearanceNameDarkAqua")
    }
}

/// Observes `NSApp.effectiveAppearance` (KVO on the application, not a window), once per
/// process, so a pair follows the Appearance with no window open.
fn watch(app: &NSApplication) {
    if WATCHING.replace(true) { return; }
    let Some(observer) = observer() else { return };
    // SAFETY: main thread; `NSApplication` is KVO-compliant for `effectiveAppearance`, and the
    // observer lives for the process, so it is never removed.
    unsafe {
        let _: () = msg_send![app, addObserver: observer, forKeyPath: ns_string!("effectiveAppearance"),
            options: 0usize, context: std::ptr::null_mut::<c_void>()];
    }
}

fn observer() -> Option<*mut AnyObject> {
    static OBSERVER: OnceLock<usize> = OnceLock::new();
    let made = *OBSERVER.get_or_init(|| {
        const NAME: &std::ffi::CStr = c"RuneAppIconAppearanceObserver";
        let class = AnyClass::get(NAME).or_else(|| {
            let mut builder = ClassBuilder::new(NAME, NSObject::class())?;
            // SAFETY: `changed` matches `observeValueForKeyPath:ofObject:change:context:` (three
            // objects and a pointer, void).
            unsafe {
                builder.add_method(sel!(observeValueForKeyPath:ofObject:change:context:), changed as extern "C" fn(_, _, _, _, _, _));
            }
            Some(builder.register())
        });
        let Some(class) = class else { return 0 };
        // SAFETY: `new` on an NSObject subclass; the +1 reference is kept for the process.
        let made: *mut AnyObject = unsafe { msg_send![class, new] };
        made as usize
    });
    (made != 0).then_some(made as *mut AnyObject)
}

/// The app's appearance changed. Reads it again rather than the change dictionary, whose values
/// can be `NSNull` while AppKit rebuilds.
extern "C" fn changed(_: &AnyObject, _: Sel, _key: *mut AnyObject, _object: *mut AnyObject, _change: *mut AnyObject, _context: *mut c_void) {
    // A panic must not unwind into AppKit.
    let _ = std::panic::catch_unwind(|| {
        if MainThreadMarker::new().is_some() { apply() } else { crate::platform::main_queue::on_main(apply) }
    });
}
