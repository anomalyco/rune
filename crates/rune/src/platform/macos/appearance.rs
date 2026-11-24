//! winit 0.30 (and master) observes `effectiveAppearance` on each window and treats both KVO
//! values as `NSAppearance`. AppKit reports `NSNull` while a window's frame view is rebuilt
//! (ReplayKit, which ScreenCaptureKit starts, calls `setStyleMask:` on every app window), so
//! winit messages `NSNull` inside a callback that cannot unwind and the process aborts.
//! Wrap winit's observer once per process: nil transitions are held back, and the last real
//! appearance stands in for a nil old value, so real changes still reach winit's `ThemeChanged`.

#![allow(unsafe_code)]

use std::{ffi::c_void, sync::atomic::{AtomicUsize, Ordering}};
use objc2::{msg_send, sel, runtime::{AnyClass, AnyObject, Sel}};
use objc2_foundation::ns_string;

type Observe = unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut AnyObject, *mut c_void);

unsafe extern "C" {
    fn class_getInstanceMethod(class: *const AnyClass, selector: Sel) -> *const c_void;
    fn method_getImplementation(method: *const c_void) -> *const c_void;
    fn method_setImplementation(method: *const c_void, imp: *const c_void) -> *const c_void;
    fn objc_getAssociatedObject(object: *const AnyObject, key: *const c_void) -> *mut AnyObject;
    fn objc_setAssociatedObject(object: *const AnyObject, key: *const c_void, value: *const AnyObject, policy: usize);
}
#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    static NSKeyValueChangeOldKey: *mut AnyObject;
    static NSKeyValueChangeNewKey: *mut AnyObject;
}

const RETAIN_NONATOMIC: usize = 1;
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static LAST_APPEARANCE: u8 = 0;

/// Main thread, after winit has created a window (its delegate class registers lazily).
#[doc(hidden)]
pub fn install() {
    if ORIGINAL.load(Ordering::Relaxed) != 0 { return; }
    let (Some(delegate), Some(object)) = (AnyClass::get(c"WinitWindowDelegate"), AnyClass::get(c"NSObject")) else { return };
    let selector = sel!(observeValueForKeyPath:ofObject:change:context:);
    // SAFETY: called on the main thread after winit registers `WinitWindowDelegate`; `delegate` and `object`
    // are valid classes, `ORIGINAL` is stored before swapping the IMP, and `observe` matches the KVO selector ABI.
    unsafe {
        let method = class_getInstanceMethod(delegate, selector);
        // Only winit's own override; never NSObject's inherited implementation.
        if method.is_null() || method == class_getInstanceMethod(object, selector) { return; }
        ORIGINAL.store(method_getImplementation(method) as usize, Ordering::Relaxed);
        method_setImplementation(method, observe as *const c_void);
    }
}

unsafe fn appearance(value: *mut AnyObject) -> bool {
    let Some(class) = AnyClass::get(c"NSAppearance") else { return false };
    // SAFETY: `value` is checked non-null and caller guarantees it is a valid ObjC object; `class` is `NSAppearance`.
    !value.is_null() && unsafe { msg_send![value, isKindOfClass: class] }
}

unsafe fn remember(window: *mut AnyObject, value: *mut AnyObject) {
    // SAFETY: caller guarantees `window` and `value` are valid ObjC objects; `&LAST_APPEARANCE` is a unique static key.
    unsafe { objc_setAssociatedObject(window, (&LAST_APPEARANCE as *const u8).cast(), value, RETAIN_NONATOMIC) }
}

unsafe extern "C" fn observe(this: *mut AnyObject, cmd: Sel, key_path: *mut AnyObject, window: *mut AnyObject, change: *mut AnyObject, context: *mut c_void) {
    // SAFETY: set before this implementation was installed.
    let original: Observe = unsafe { std::mem::transmute(ORIGINAL.load(Ordering::Relaxed)) };
    let key: &AnyObject = ns_string!("effectiveAppearance");
    let watched = !key_path.is_null() && !window.is_null() && !change.is_null()
        // SAFETY: `key_path` is non-null and passed as an `NSString` by KVO; `key` is a valid `NSString`.
        && unsafe { msg_send![key_path, isEqualToString: key] };
    // SAFETY: `original` is winit's original KVO IMP with the same ABI and arguments passed by AppKit.
    if !watched { return unsafe { original(this, cmd, key_path, window, change, context) } }
    // SAFETY: KVO guarantees non-null `window` and `change` (`NSDictionary`) are live ObjC objects,
    // `NSKeyValueChange*Key` are valid Foundation keys, and `patched` from `mutableCopy` is released after forwarding.
    unsafe {
        let old: *mut AnyObject = msg_send![change, objectForKey: NSKeyValueChangeOldKey];
        let new: *mut AnyObject = msg_send![change, objectForKey: NSKeyValueChangeNewKey];
        match (appearance(old), appearance(new)) {
            (true, true) => { remember(window, new); original(this, cmd, key_path, window, change, context) }
            (old_known, false) => if old_known { remember(window, old) },
            (false, true) => {
                let last = objc_getAssociatedObject(window, (&LAST_APPEARANCE as *const u8).cast());
                let patched: *mut AnyObject = msg_send![change, mutableCopy];
                let _: () = msg_send![patched, setObject: if last.is_null() { new } else { last }, forKey: NSKeyValueChangeOldKey];
                remember(window, new);
                original(this, cmd, key_path, window, patched, context);
                let _: () = msg_send![patched, release];
            }
        }
    }
}

/// Applies `scheme` to `NSApplication.sharedApplication.appearance` on the main thread:
/// `nil` for `System`, `NSAppearanceNameAqua` for `Light`, and `NSAppearanceNameDarkAqua`
/// for `Dark`. Native menus, open/save panels, traffic lights, system materials, and
/// windows without a per-window override follow it.
pub(crate) fn set_app_appearance(scheme: crate::ColorScheme) {
    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    set_object_appearance(&app, scheme);
}

/// Applies `scheme` to an `NSWindow` (or `NSPanel`) on the main thread.
pub(crate) fn set_window_appearance(window: &objc2_app_kit::NSWindow, scheme: crate::ColorScheme) {
    set_object_appearance(window, scheme);
}

fn set_object_appearance(target: &AnyObject, scheme: crate::ColorScheme) {
    objc2::rc::autoreleasepool(|_| {
        let appearance = resolve_appearance(scheme);
        // SAFETY: called on the main thread; `target` conforms to `NSAppearanceCustomization`
        // (`NSApplication` or `NSWindow`) and `setAppearance:` accepts an `NSAppearance` or `nil`.
        unsafe { let _: () = msg_send![target, setAppearance: appearance]; }
    });
}

fn resolve_appearance(scheme: crate::ColorScheme) -> *mut AnyObject {
    let name = match scheme.macos_appearance_name() {
        None => return std::ptr::null_mut(),
        Some("NSAppearanceNameAqua") => ns_string!("NSAppearanceNameAqua"),
        Some("NSAppearanceNameDarkAqua") => ns_string!("NSAppearanceNameDarkAqua"),
        Some(_) => return std::ptr::null_mut(),
    };
    let Some(class) = AnyClass::get(c"NSAppearance") else { return std::ptr::null_mut() };
    // SAFETY: `class` is `NSAppearance`, `name` is a static `NSString`, and
    // `+appearanceNamed:` returns an autoreleased `NSAppearance` or `nil`.
    unsafe { msg_send![class, appearanceNamed: name] }
}
