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
