//! The traffic lights at the app's own spot (`Chrome::traffic_lights`).
//!
//! Since macOS 26, AppKit puts the buttons back at (9, 9) and the titlebar container back to
//! its own height whenever it lays the titlebar out again, which it does on its own (a title
//! or appearance change, a zoom), not only on the resizes the shell sees. AppKit also observes
//! the titlebar container with KVO from launch, so Rune hooks `-layout` on each view's own
//! AppKit class (below any `NSKVONotifying_` class) rather than changing the instance's isa:
//! KVO's class stays on top and the hook runs AppKit's `-layout`, then puts the buttons back.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::{NSView, NSWindow, NSWindowButton, NSWindowStyleMask};
use objc2_foundation::NSPoint;
use winit::window::Window;

use crate::platform::macos::ns_window;

unsafe extern "C" {
    fn class_getInstanceMethod(class: *const AnyClass, selector: Sel) -> *const c_void;
    fn method_getImplementation(method: *const c_void) -> *const c_void;
    fn method_getTypeEncoding(method: *const c_void) -> *const c_char;
    fn method_setImplementation(method: *const c_void, imp: *const c_void) -> *const c_void;
    fn class_addMethod(class: *const AnyClass, selector: Sel, imp: *const c_void, types: *const c_char) -> Bool;
    fn imp_implementationWithBlock(block: *const c_void) -> *const c_void;
    fn imp_removeBlock(imp: *const c_void) -> Bool;
}

thread_local! {
    /// Each window's spot, by its `NSWindow` pointer, for the layout hook.
    static SPOTS: RefCell<HashMap<usize, (f64, f64)>> = RefCell::new(HashMap::new());
    /// Set while the hook places, so the frames it sets don't place again.
    static PLACING: Cell<bool> = const { Cell::new(false) };
}

/// Classes whose `-layout` Rune has already hooked, by class pointer.
static HOOKED: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

/// Drops its window's spot when the window goes, so a later window at the same address
/// doesn't inherit it.
pub(crate) struct Spot(usize);

impl Spot {
    pub(crate) fn new(window: &Window) -> Spot {
        Spot(ns_window(window).map_or(0, |w| Retained::as_ptr(&w) as usize))
    }
}

impl Drop for Spot {
    fn drop(&mut self) {
        SPOTS.try_with(|s| s.borrow_mut().remove(&self.0)).ok();
    }
}

/// Electron's `trafficLightPosition`: the titlebar container grows to the buttons' height
/// plus twice `y`, and the buttons sit `x, y` from the top-left at AppKit's own pitch.
pub(crate) fn place(window: &Window, (x, y): (f32, f32)) {
    let Some(win) = ns_window(window) else { return };
    let spot = (f64::from(x), f64::from(y));
    SPOTS.with(|s| s.borrow_mut().insert(Retained::as_ptr(&win) as usize, spot));
    place_in(&win, spot);
}

fn place_in(win: &NSWindow, (x, y): (f64, f64)) {
    if win.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return;
    }
    let buttons = [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton]
        .map(|b| win.standardWindowButton(b));
    let [Some(close), Some(mini), Some(zoom)] = buttons else { return };
    // SAFETY: AppKit's own view hierarchy, on the main thread.
    let Some(parent) = (unsafe { close.superview() }) else { return };
    // SAFETY: `parent` is a live `NSView` in AppKit's window hierarchy on the main thread.
    let Some(container) = (unsafe { parent.superview() }) else { return };
    // SAFETY: live views on the main thread.
    unsafe {
        hold(&parent);
        hold(&container);
    }
    if PLACING.with(|p| p.replace(true)) {
        return;
    }
    // A right-to-left app's titlebar lays out from the right (`labels::set_direction`), as AppKit
    // lays out a right-to-left localization's: `x` in from the right edge, Close outermost (on macOS
    // 27 a native window's Close sits at 687-701 of 720 points, against 19-33 left to right). AppKit's
    // own `windowTitlebarLayoutDirection` follows the localization and has no setter, so Rune places
    // them, as it does the spot.
    let rtl = crate::labels::rtl();
    let pitch = (mini.frame().origin.x - close.frame().origin.x).abs();
    let mut frame = container.frame();
    frame.size.height = close.frame().size.height + 2.0 * y;
    frame.origin.y = win.frame().size.height - frame.size.height;
    let width = parent.frame().size.width;
    container.setFrame(frame);
    for (i, b) in [close, mini, zoom].iter().enumerate() {
        let at = x + i as f64 * pitch;
        let at = if rtl { width - at - b.frame().size.width } else { at };
        b.setFrameOrigin(NSPoint::new(at, y));
    }
    PLACING.with(|p| p.set(false));
}

/// Puts the buttons back after AppKit's `-layout` ran, when `this` belongs to a window with a spot.
unsafe fn after_layout(this: *mut AnyObject) {
    if this.is_null() || PLACING.try_with(Cell::get).unwrap_or(true) {
        return;
    }
    if SPOTS.try_with(|s| s.borrow().is_empty()).unwrap_or(true) {
        return;
    }
    // SAFETY: `SPOTS` is only populated on the main thread for live titlebar `NSView`s.
    let view = unsafe { &*(this.cast::<NSView>()) };
    let Some(win) = view.window() else { return };
    let Some(spot) = SPOTS.try_with(|s| s.borrow().get(&(Retained::as_ptr(&win) as usize)).copied()).ok().flatten() else { return };
    place_in(&win, spot);
}

/// The view's own class, below any KVO subclass (`NSKVONotifying_`).
fn own_class(object: &AnyObject) -> Option<&'static AnyClass> {
    std::iter::successors(Some(object.class()), |c| c.superclass())
        .find(|c| !c.name().to_bytes().starts_with(b"NSKVONotifying_"))
}

/// Hooks `-layout` on `view`'s own class once, leaving its isa (and any KVO subclass) intact.
unsafe fn hold(view: &NSView) {
    // SAFETY: `view` is a live `NSView` on the main thread.
    unsafe { hold_object(view) };
}

/// Hooks `-layout` on `object`'s own class (below any KVO subclass) once per class.
unsafe fn hold_object(object: &AnyObject) {
    let Some(class) = own_class(object) else { return };
    let key = class as *const AnyClass as usize;
    let mut guard = HOOKED.lock().unwrap_or_else(|e| e.into_inner());
    let hooked = guard.get_or_insert_with(HashSet::new);
    if hooked.contains(&key) {
        return;
    }
    let selector = sel!(layout);
    // SAFETY: `class` is a registered ObjC class, `HOOKED` serializes installation, and each
    // block trampoline matches `-(void)layout`'s ABI (`void (^)(id)`).
    unsafe {
        let method = class_getInstanceMethod(class, selector);
        if method.is_null() {
            return;
        }
        let parent = class.superclass();
        let parent_method = parent.map_or(std::ptr::null(), |p| class_getInstanceMethod(p, selector));
        if method != parent_method {
            let original = Arc::new(AtomicUsize::new(method_getImplementation(method) as usize));
            let call = original.clone();
            let block = block2::RcBlock::new(move |this: *mut AnyObject| {
                // SAFETY: `call` holds the original `-(void)layout` IMP replaced on `class`.
                unsafe {
                    let imp: unsafe extern "C" fn(*mut AnyObject, Sel) = std::mem::transmute(call.load(Ordering::Acquire));
                    imp(this, sel!(layout));
                    after_layout(this);
                }
            });
            let imp = imp_implementationWithBlock(block2::RcBlock::as_ptr(&block).cast());
            if imp.is_null() {
                return;
            }
            let prev = method_setImplementation(method, imp);
            original.store(prev as usize, Ordering::Release);
            hooked.insert(key);
        } else if let Some(parent) = parent {
            let types = method_getTypeEncoding(method);
            let block = block2::RcBlock::new(move |this: *mut AnyObject| {
                // SAFETY: `this` is a live instance of `class` and `parent` implements `-layout`.
                unsafe {
                    if !this.is_null() {
                        let _: () = msg_send![super(&*this, parent), layout];
                        after_layout(this);
                    }
                }
            });
            let imp = imp_implementationWithBlock(block2::RcBlock::as_ptr(&block).cast());
            if imp.is_null() {
                return;
            }
            if class_addMethod(class, selector, imp, types).as_bool() {
                hooked.insert(key);
            } else {
                imp_removeBlock(imp);
            }
        }
    }
}

#[cfg(test)]
#[path = "traffic_lights_tests.rs"]
mod tests;
