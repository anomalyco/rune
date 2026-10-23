//! winit 0.30 keeps its original NSWindow in the view/delegate. Forward the handful of
//! readbacks that must describe the adopted panel. Preserve KVO's actual superclass.

use std::{cell::RefCell, collections::HashMap, ffi::{CString, c_char, c_void}};
use objc2::{msg_send, sel, runtime::{AnyClass, AnyObject, Bool, Sel}};
use objc2_app_kit::{NSPanel, NSView, NSWindow};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::rc::Rc;

unsafe extern "C" {
    fn object_getClass(object: *const AnyObject) -> *const AnyClass;
    fn object_setClass(object: *mut AnyObject, class: *const AnyClass) -> *const AnyClass;
    fn objc_allocateClassPair(parent: *const AnyClass, name: *const c_char, extra: usize) -> *const AnyClass;
    fn objc_registerClassPair(class: *const AnyClass);
    fn class_getInstanceMethod(class: *const AnyClass, selector: Sel) -> *const c_void;
    fn method_getTypeEncoding(method: *const c_void) -> *const c_char;
    fn class_addMethod(class: *const AnyClass, selector: Sel, imp: *const c_void, types: *const c_char) -> bool;
}

thread_local! {
    static WINDOWS: RefCell<HashMap<usize, (usize, usize)>> = RefCell::default();
    static VIEWS: RefCell<HashMap<usize, (usize, Rc<crate::macos::PointerState>)>> = RefCell::default();
}

fn handles(window: &AnyObject) -> (usize, usize) {
    WINDOWS.with(|windows| windows.borrow().get(&(window as *const _ as usize)).copied().unwrap_or_default())
}
extern "C" fn content_view(window: &AnyObject, _: Sel) -> *mut AnyObject { handles(window).0 as *mut AnyObject }
extern "C" fn visible(window: &AnyObject, _: Sel) -> Bool {
    unsafe { let panel = handles(window).1 as *mut AnyObject; msg_send![panel, isVisible] }
}
extern "C" fn occlusion(window: &AnyObject, _: Sel) -> usize {
    unsafe { let panel = handles(window).1 as *mut AnyObject; msg_send![panel, occlusionState] }
}
extern "C" fn scale(window: &AnyObject, _: Sel) -> f64 {
    unsafe { let panel = handles(window).1 as *mut AnyObject; msg_send![panel, backingScaleFactor] }
}
extern "C" fn screen(window: &AnyObject, _: Sel) -> *mut AnyObject {
    unsafe { let panel = handles(window).1 as *mut AnyObject; msg_send![panel, screen] }
}

// Ordinary panel hosts retain winit's geometry/appearance API. Only their source window
// gets these forwards; overlay placement continues to manage both native frames itself.
extern "C" fn frame(window: &AnyObject, _: Sel) -> NSRect {
    unsafe { let panel = handles(window).1 as *mut AnyObject; msg_send![panel, frame] }
}
extern "C" fn key(window: &AnyObject, _: Sel) -> Bool {
    unsafe { let panel = handles(window).1 as *mut AnyObject; msg_send![panel, isKeyWindow] }
}
extern "C" fn set_frame(window: &AnyObject, _: Sel, rect: NSRect, display: Bool) {
    unsafe { let panel = handles(window).1 as *mut AnyObject; let _: () = msg_send![panel, setFrame: rect, display: display]; }
}
extern "C" fn set_origin(window: &AnyObject, _: Sel, point: NSPoint) {
    unsafe { let panel = handles(window).1 as *mut AnyObject; let _: () = msg_send![panel, setFrameOrigin: point]; }
}
extern "C" fn set_size(window: &AnyObject, _: Sel, size: NSSize) {
    unsafe { let panel = handles(window).1 as *mut AnyObject; let _: () = msg_send![panel, setContentSize: size]; }
}
extern "C" fn set_title(window: &AnyObject, _: Sel, title: *mut AnyObject) {
    unsafe { let panel = handles(window).1 as *mut AnyObject; let _: () = msg_send![panel, setTitle: title]; }
}
extern "C" fn set_level(window: &AnyObject, _: Sel, level: isize) {
    unsafe { let panel = handles(window).1 as *mut AnyObject; let _: () = msg_send![panel, setLevel: level]; }
}
extern "C" fn set_hit(window: &AnyObject, _: Sel, ignore: Bool) {
    unsafe { let panel = handles(window).1 as *mut AnyObject; let _: () = msg_send![panel, setIgnoresMouseEvents: ignore]; }
}

extern "C" fn hit_test(view: &AnyObject, _: Sel, point: NSPoint) -> *mut AnyObject {
    let entry = VIEWS.with(|views| views.borrow().get(&(view as *const _ as usize)).cloned());
    let Some((original, state)) = entry else { return std::ptr::null_mut() };
    unsafe {
        let parent: *mut AnyObject = msg_send![view, superview];
        let local: NSPoint = msg_send![view, convertPoint: point, fromView: parent];
        let bounds: NSRect = msg_send![view, bounds];
        let flipped: Bool = msg_send![view, isFlipped];
        let y = if flipped.as_bool() { local.y - bounds.origin.y } else { bounds.origin.y + bounds.size.height - local.y };
        if !state.hit((local.x - bounds.origin.x) as f32, y as f32) { return std::ptr::null_mut(); }
        msg_send![super(view, &*(original as *const AnyClass)), hitTest: point]
    }
}

pub(crate) struct Bridge { source: *mut AnyObject, original: *const AnyClass, view: Option<(*mut AnyObject, *const AnyClass)> }
impl Bridge {
    /// Main-thread only. The owner retains all three objects until after this bridge drops.
    pub fn new(source: &NSWindow, panel: &NSPanel, view: &NSView, filter: Option<Rc<crate::macos::PointerState>>) -> Option<Self> {
        Self::new_kind(source, panel, view, filter, false)
    }
    pub fn new_window(source: &NSWindow, panel: &NSPanel, view: &NSView) -> Option<Self> {
        Self::new_kind(source, panel, view, None, true)
    }
    fn new_kind(source: &NSWindow, panel: &NSPanel, view: &NSView, filter: Option<Rc<crate::macos::PointerState>>, ordinary: bool) -> Option<Self> {
        let source = (source as *const NSWindow).cast_mut().cast::<AnyObject>();
        unsafe {
            let original = object_getClass(source);
            let name = CString::new(format!("RuneOverlaySource_{:x}_{ordinary}", original as usize)).ok()?;
            let class = if let Some(class) = AnyClass::get(name.to_str().ok()?) { class as *const AnyClass } else {
                let class = objc_allocateClassPair(original, name.as_ptr(), 0);
                if class.is_null() { return None; }
                for (selector, implementation) in [
                    (sel!(contentView), content_view as *const c_void),
                    (sel!(isVisible), visible as *const c_void),
                    (sel!(occlusionState), occlusion as *const c_void),
                    (sel!(backingScaleFactor), scale as *const c_void),
                    (sel!(screen), screen as *const c_void),
                ] {
                    let method = class_getInstanceMethod(original, selector);
                    // All selectors belong to NSWindow, inherited by every winit/KVO source.
                    assert!(!method.is_null());
                    assert!(class_addMethod(class, selector, implementation, method_getTypeEncoding(method)));
                }
                if ordinary {
                    for (selector, implementation) in [
                        (sel!(frame), frame as *const c_void),
                        (sel!(isKeyWindow), key as *const c_void),
                        (sel!(setFrame:display:), set_frame as *const c_void),
                        (sel!(setFrameOrigin:), set_origin as *const c_void),
                        (sel!(setContentSize:), set_size as *const c_void),
                        (sel!(setTitle:), set_title as *const c_void),
                        (sel!(setLevel:), set_level as *const c_void),
                        (sel!(setIgnoresMouseEvents:), set_hit as *const c_void),
                    ] {
                        let method = class_getInstanceMethod(original, selector);
                        assert!(!method.is_null());
                        assert!(class_addMethod(class, selector, implementation, method_getTypeEncoding(method)));
                    }
                }
                objc_registerClassPair(class);
                class
            };
            WINDOWS.with(|windows| windows.borrow_mut().insert(source as usize, (view as *const _ as usize, panel as *const _ as usize)));
            object_setClass(source, class);
            let mut bridge = Self { source, original, view: None };
            if let Some(filter) = filter {
                let view = (view as *const NSView).cast_mut().cast::<AnyObject>();
                let original = object_getClass(view);
                let name = CString::new(format!("RuneOverlayHit_{:x}", original as usize)).ok()?;
                let class = if let Some(class) = AnyClass::get(name.to_str().ok()?) { class as *const AnyClass } else {
                    let class = objc_allocateClassPair(original, name.as_ptr(), 0);
                    if class.is_null() { return None; }
                    let method = class_getInstanceMethod(original, sel!(hitTest:));
                    assert!(!method.is_null());
                    assert!(class_addMethod(class, sel!(hitTest:), hit_test as *const c_void, method_getTypeEncoding(method)));
                    objc_registerClassPair(class);
                    class
                };
                VIEWS.with(|views| views.borrow_mut().insert(view as usize, (original as usize, filter)));
                object_setClass(view, class);
                bridge.view = Some((view, original));
            }
            Some(bridge)
        }
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        // Restore the exact original class before removing mappings or returning the view.
        unsafe { object_setClass(self.source, self.original); }
        WINDOWS.with(|windows| windows.borrow_mut().remove(&(self.source as usize)));
        if let Some((view, original)) = self.view {
            unsafe { object_setClass(view, original); }
            VIEWS.with(|views| views.borrow_mut().remove(&(view as usize)));
        }
    }
}
