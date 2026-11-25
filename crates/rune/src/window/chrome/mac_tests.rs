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

#[test]
#[allow(unsafe_code)]
fn is_panel_object_distinguishes_nspanel_from_nswindow() {
    use objc2::ClassType;
    use objc2_app_kit::{NSPanel, NSWindow};
    let win_class = ClassBuilder::new(c"RunePanelCheckWindow", NSWindow::class()).unwrap().register();
    let panel_class = ClassBuilder::new(c"RunePanelCheckPanel", NSPanel::class()).unwrap().register();
    // SAFETY: `alloc` sets the `isa` pointer on registered ObjC classes without opening a window,
    // and `isKindOfClass:` only inspects the class hierarchy.
    unsafe {
        let win: *mut AnyObject = msg_send![win_class, alloc];
        let panel: *mut AnyObject = msg_send![panel_class, alloc];
        assert!(!is_panel_object(&*win), "ordinary NSWindow must not read as an NSPanel");
        assert!(is_panel_object(&*panel), "adopted NSPanel subclass must read as an NSPanel");
    }
}

#[test]
fn double_click_action_ignores_panels_under_every_system_setting() {
    for setting in [Some("Minimize"), Some("Maximize"), Some("None"), None] {
        assert_eq!(
            double_click_action(setting, true),
            DoubleClickAction::None,
            "adopted panel must ignore titlebar double-click for AppleActionOnDoubleClick={setting:?}",
        );
    }
    assert_eq!(double_click_action(Some("Minimize"), false), DoubleClickAction::Minimize);
    assert_eq!(double_click_action(Some("Maximize"), false), DoubleClickAction::Zoom);
    assert_eq!(double_click_action(Some("None"), false), DoubleClickAction::None);
    assert_eq!(double_click_action(None, false), DoubleClickAction::Zoom);
}

#[test]
#[allow(unsafe_code)]
fn full_screen_watch_flips_on_will_notifications_and_recovers_from_failures() {
    use std::cell::Cell;
    use std::rc::Rc;
    use objc2::rc::Retained;
    use objc2_foundation::NSObject;
    use super::super::full_screen::State;

    let post = |name: &str, object: &AnyObject| {
        let center_class = AnyClass::get(c"NSNotificationCenter").unwrap();
        // SAFETY: posts a named notification on the default notification center for `object`.
        unsafe {
            let center: Retained<AnyObject> = msg_send![center_class, defaultCenter];
            let name = NSString::from_str(name);
            let _: () = msg_send![&*center, postNotificationName: &*name, object: object];
        }
    };

    let target = NSObject::new();
    let sibling = NSObject::new();
    let redraws = Rc::new(Cell::new(0usize));
    let count = redraws.clone();
    // SAFETY: `target` is a live `NSObject` owned by this test; `watch` removes all observers on drop.
    let watch = unsafe {
        FullScreenWatch::observe_object(&target, State::Windowed, move || {
            count.set(count.get() + 1);
        })
    };

    assert_eq!(watch.state(), State::Windowed);
    assert!(!watch.is_full_screen());
    assert!(!watch.take());

    // Notifications for another object are ignored.
    post("NSWindowWillEnterFullScreenNotification", &sibling);
    assert_eq!(watch.state(), State::Windowed);
    assert!(!watch.is_full_screen());
    assert_eq!(redraws.get(), 0);

    // Normal enter flips at WillEnter and settles at DidEnter.
    post("NSWindowWillEnterFullScreenNotification", &target);
    assert_eq!(watch.state(), State::Entering);
    assert!(watch.is_full_screen());
    assert!(watch.take());
    assert_eq!(redraws.get(), 1);

    // Duplicate WillEnter does not request another redraw.
    post("NSWindowWillEnterFullScreenNotification", &target);
    assert!(!watch.take());
    assert_eq!(redraws.get(), 1);

    post("NSWindowDidEnterFullScreenNotification", &target);
    assert_eq!(watch.state(), State::FullScreen);
    assert!(watch.is_full_screen());
    assert!(watch.take());
    assert_eq!(redraws.get(), 2);

    // Normal exit flips at WillExit and stays Exiting even while winit still holds fullscreen.
    post("NSWindowWillExitFullScreenNotification", &target);
    assert_eq!(watch.state(), State::Exiting);
    assert!(!watch.is_full_screen());
    assert!(watch.take());
    assert_eq!(redraws.get(), 3);

    watch.sync(true, true);
    assert_eq!(watch.state(), State::Exiting);
    assert!(!watch.is_full_screen());

    post("NSWindowDidExitFullScreenNotification", &target);
    assert_eq!(watch.state(), State::Windowed);
    assert!(!watch.is_full_screen());
    assert!(watch.take());
    assert_eq!(redraws.get(), 4);

    // Failed enter (AppKit posts DidExit): returns to Windowed and redraws.
    post("NSWindowWillEnterFullScreenNotification", &target);
    assert_eq!(watch.state(), State::Entering);
    assert!(watch.is_full_screen());
    assert!(watch.take());
    assert_eq!(redraws.get(), 5);

    post("NSWindowDidExitFullScreenNotification", &target);
    assert_eq!(watch.state(), State::Windowed);
    assert!(!watch.is_full_screen());
    assert!(watch.take());
    assert_eq!(redraws.get(), 6);

    // Failed enter (winit cleared fullscreen without DidExit): sync recovers to Windowed.
    post("NSWindowWillEnterFullScreenNotification", &target);
    assert_eq!(watch.state(), State::Entering);
    assert!(watch.take());
    watch.sync(false, false);
    assert_eq!(watch.state(), State::Windowed);
    assert!(!watch.is_full_screen());

    // Failed exit (AppKit aborts exit and stays in full screen via DidEnter): returns to FullScreen and redraws.
    post("NSWindowDidEnterFullScreenNotification", &target);
    assert!(watch.take());
    post("NSWindowWillExitFullScreenNotification", &target);
    assert_eq!(watch.state(), State::Exiting);
    assert!(!watch.is_full_screen());
    assert!(watch.take());
    let before_abort = redraws.get();
    post("NSWindowDidEnterFullScreenNotification", &target);
    assert_eq!(watch.state(), State::FullScreen);
    assert!(watch.is_full_screen());
    assert!(watch.take());
    assert_eq!(redraws.get(), before_abort + 1);

    // Dropping the watch unregisters all four observers.
    let before_drop = redraws.get();
    drop(watch);
    post("NSWindowWillExitFullScreenNotification", &target);
    post("NSWindowDidExitFullScreenNotification", &target);
    assert_eq!(redraws.get(), before_drop);
}
