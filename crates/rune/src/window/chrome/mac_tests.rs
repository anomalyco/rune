use super::*;

#[test]
#[allow(unsafe_code)]
fn drag_subclass_changes_only_the_selected_instance() {
    // NSObject exercises the Objective-C class operation without constructing a window
    // or requiring an AppKit event loop on the test thread.
    use objc2::{ffi, rc::Retained};
    let selected = objc2_foundation::NSObject::new();
    let sibling = objc2_foundation::NSObject::new();
    // SAFETY: `selected` and `sibling` are live `NSObject` instances owned by this test; ObjC runtime
    // class queries and `mouseDownCanMoveWindow` match the dynamic subclass installed by `scope_drag_regions`.
    unsafe {
        let selected_ptr = Retained::as_ptr(&selected).cast_mut().cast();
        let sibling_ptr = Retained::as_ptr(&sibling).cast_mut().cast();
        let base = ffi::object_getClass(selected_ptr);
        let selector = sel!(mouseDownCanMoveWindow);
        let original = ffi::class_getInstanceMethod(base, selector);
        scope_drag_regions(&selected);
        let scoped = ffi::object_getClass(selected_ptr);
        assert_ne!(scoped, base);
        assert_eq!(ffi::class_getSuperclass(scoped), base);
        assert_eq!(ffi::class_getInstanceSize(scoped), ffi::class_getInstanceSize(base));
        assert_eq!(ffi::object_getClass(sibling_ptr), base);
        assert_eq!(ffi::class_getInstanceMethod(base, selector), original);
        let moves: Bool = msg_send![&*selected, mouseDownCanMoveWindow];
        assert!(!moves.as_bool());
        scope_drag_regions(&selected);
        assert_eq!(ffi::object_getClass(selected_ptr), scoped);
    }
}

#[test]
#[allow(unsafe_code)]
fn kvo_observed_drag_regions_in_both_orders() {
    use objc2::class;
    use objc2::rc::Retained;
    use objc2_foundation::NSObject;
    // Dedicated base class so parallel test threads do not race on registering RuneDragRegions_NSObject.
    let test_class = ClassBuilder::new(c"RuneDragRegionsKvoTest", class!(NSObject)).unwrap().register();
    let key = NSString::from_str("probe");
    let observer = NSObject::new();

    // Order 1: Rune subclasses first, then KVO observes and removes the observer.
    // SAFETY: `test_class` is a registered `NSObject` subclass; KVO selectors match Foundation.
    unsafe {
        let below: Retained<AnyObject> = msg_send![test_class, new];
        scope_drag_regions(&below);
        let _: () = msg_send![&*below, addObserver: &*observer, forKeyPath: &*key, options: 0usize, context: std::ptr::null_mut::<std::ffi::c_void>()];
        assert!(below.class().name().to_bytes().starts_with(b"NSKVONotifying_"));
        assert!(has_subclass(&below, b"RuneDragRegions_"), "native_titlebar_band still finds RuneDragRegions_ beneath KVO");
        scope_drag_regions(&below);
        assert!(below.class().name().to_bytes().starts_with(b"NSKVONotifying_"));
        let _: () = msg_send![&*below, removeObserver: &*observer, forKeyPath: &*key];
    }

    // Order 2: KVO observes first, then Rune runs, then KVO removes the observer.
    // SAFETY: `test_class` is a registered `NSObject` subclass; KVO selectors match Foundation.
    unsafe {
        let above: Retained<AnyObject> = msg_send![test_class, new];
        let _: () = msg_send![&*above, addObserver: &*observer, forKeyPath: &*key, options: 0usize, context: std::ptr::null_mut::<std::ffi::c_void>()];
        let kvo_class = above.class();
        assert!(kvo_class.name().to_bytes().starts_with(b"NSKVONotifying_"));
        scope_drag_regions(&above);
        assert!(std::ptr::eq(above.class(), kvo_class));
        let _: () = msg_send![&*above, removeObserver: &*observer, forKeyPath: &*key];
    }
}
