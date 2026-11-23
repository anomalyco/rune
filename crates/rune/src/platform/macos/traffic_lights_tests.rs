#![allow(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use objc2_foundation::{NSObject, NSString};

use super::*;

fn register_layout_class(name: &std::ffi::CStr, superclass: &AnyClass, imp: Option<unsafe extern "C" fn(*mut AnyObject, Sel)>) -> &'static AnyClass {
    let mut builder = ClassBuilder::new(name, superclass).expect("unique test class");
    if let Some(imp) = imp {
        // SAFETY: matches `-(void)layout`'s C ABI.
        unsafe { builder.add_method(sel!(layout), imp) };
    }
    builder.register()
}

#[test]
fn kvo_observed_before_hold_keeps_kvo_on_top_and_removes_observer() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn count(_: *mut AnyObject, _: Sel) {
        CALLS.fetch_add(1, Ordering::Relaxed);
    }

    let cls = register_layout_class(c"RuneTrafficLightsKvoBefore", class!(NSObject), Some(count));
    let observer = NSObject::new();
    let key = NSString::from_str("probe");
    // SAFETY: `cls` is a registered `NSObject` subclass; KVO and `-layout` selectors match their ABIs.
    unsafe {
        let obj: Retained<AnyObject> = msg_send![cls, new];
        let _: () = msg_send![&*obj, addObserver: &*observer, forKeyPath: &*key, options: 0usize, context: std::ptr::null_mut::<c_void>()];
        let kvo_class = obj.class();
        assert!(kvo_class.name().to_bytes().starts_with(b"NSKVONotifying_"));

        hold_object(&obj);
        assert!(std::ptr::eq(obj.class(), kvo_class), "KVO's class must stay on top");

        let _: () = msg_send![&*obj, layout];
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);

        let _: () = msg_send![&*obj, removeObserver: &*observer, forKeyPath: &*key];
    }
}

#[test]
fn hold_before_kvo_keeps_layout_hook_and_removes_observer() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn count(_: *mut AnyObject, _: Sel) {
        CALLS.fetch_add(1, Ordering::Relaxed);
    }

    let cls = register_layout_class(c"RuneTrafficLightsKvoAfter", class!(NSObject), Some(count));
    let observer = NSObject::new();
    let key = NSString::from_str("probe");
    // SAFETY: `cls` is a registered `NSObject` subclass; KVO and `-layout` selectors match their ABIs.
    unsafe {
        let obj: Retained<AnyObject> = msg_send![cls, new];
        hold_object(&obj);
        assert!(std::ptr::eq(obj.class(), cls), "hold must not swap the instance's isa");

        let _: () = msg_send![&*obj, addObserver: &*observer, forKeyPath: &*key, options: 0usize, context: std::ptr::null_mut::<c_void>()];
        assert!(obj.class().name().to_bytes().starts_with(b"NSKVONotifying_"));

        // Calling hold again while KVO is on top is a no-op and leaves KVO on top.
        let kvo_class = obj.class();
        hold_object(&obj);
        assert!(std::ptr::eq(obj.class(), kvo_class));

        let _: () = msg_send![&*obj, layout];
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);

        let _: () = msg_send![&*obj, removeObserver: &*observer, forKeyPath: &*key];
    }
}

#[test]
fn inherited_layout_hooks_only_subclass_without_recursion() {
    static BASE_CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn base_layout(_: *mut AnyObject, _: Sel) {
        BASE_CALLS.fetch_add(1, Ordering::Relaxed);
    }

    let base_cls = register_layout_class(c"RuneTrafficLightsInheritBase", class!(NSObject), Some(base_layout));
    let sub_cls = register_layout_class(c"RuneTrafficLightsInheritSub", base_cls, None);
    // SAFETY: registered `NSObject` subclasses; `-layout` matches its ABI.
    unsafe {
        let base_imp_before = method_getImplementation(class_getInstanceMethod(base_cls, sel!(layout)));
        let sub: Retained<AnyObject> = msg_send![sub_cls, new];
        hold_object(&sub);

        let base_imp_after = method_getImplementation(class_getInstanceMethod(base_cls, sel!(layout)));
        assert_eq!(base_imp_before, base_imp_after, "holding an inheriting subclass must leave its superclass untouched");

        let _: () = msg_send![&*sub, layout];
        assert_eq!(BASE_CALLS.load(Ordering::Relaxed), 1);

        // Hooking the superclass afterwards must still run the original once, with no recursion.
        let base: Retained<AnyObject> = msg_send![base_cls, new];
        hold_object(&base);
        let _: () = msg_send![&*sub, layout];
        assert_eq!(BASE_CALLS.load(Ordering::Relaxed), 2);
    }
}

#[test]
fn subclass_override_calling_super_does_not_recurse_when_both_hooked() {
    static BASE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static SUB_CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn base_layout(_: *mut AnyObject, _: Sel) {
        BASE_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    unsafe extern "C" fn sub_layout(this: *mut AnyObject, _: Sel) {
        SUB_CALLS.fetch_add(1, Ordering::Relaxed);
        let base = AnyClass::get(c"RuneTrafficLightsOverrideBase").unwrap();
        // SAFETY: `this` is a live instance of the subclass and `base` defines `-layout`.
        let _: () = unsafe { msg_send![super(&*this, base), layout] };
    }

    let base_cls = register_layout_class(c"RuneTrafficLightsOverrideBase", class!(NSObject), Some(base_layout));
    let sub_cls = register_layout_class(c"RuneTrafficLightsOverrideSub", base_cls, Some(sub_layout));
    // SAFETY: registered `NSObject` subclasses; `-layout` matches its ABI.
    unsafe {
        let base: Retained<AnyObject> = msg_send![base_cls, new];
        let sub: Retained<AnyObject> = msg_send![sub_cls, new];
        hold_object(&sub);
        hold_object(&base);

        let _: () = msg_send![&*sub, layout];
        assert_eq!(SUB_CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(BASE_CALLS.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn holding_twice_hooks_class_once() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn count(_: *mut AnyObject, _: Sel) {
        CALLS.fetch_add(1, Ordering::Relaxed);
    }

    let cls = register_layout_class(c"RuneTrafficLightsTwice", class!(NSObject), Some(count));
    // SAFETY: registered `NSObject` subclass; `-layout` matches its ABI.
    unsafe {
        let first: Retained<AnyObject> = msg_send![cls, new];
        let second: Retained<AnyObject> = msg_send![cls, new];
        hold_object(&first);
        let imp_once = method_getImplementation(class_getInstanceMethod(cls, sel!(layout)));
        hold_object(&first);
        hold_object(&second);
        let imp_twice = method_getImplementation(class_getInstanceMethod(cls, sel!(layout)));
        assert_eq!(imp_once, imp_twice, "class should be hooked only once");

        let _: () = msg_send![&*first, layout];
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
    }
}
