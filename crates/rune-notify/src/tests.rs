#![allow(unsafe_code)]

use super::*;

#[test]
fn a_bare_test_binary_cannot_post() {
    // Not a .app bundle, so it never reaches UNUserNotificationCenter (which would throw).
    assert!(!available());
    assert!(!post(&Notification::new("n-1", "Response ready").body("Fix the build")));
    remove(&["n-1"]);
}

#[test]
fn click_reads_user_info() {
    let click = Click { id: "n-1".into(), info: vec![("session".into(), "ses_1".into())] };
    assert_eq!(click.get("session"), Some("ses_1"));
    assert_eq!(click.get("server"), None);
}

/// The delegate's click method, called the way UNUserNotificationCenter calls it, with a stand-in
/// response that answers `notification`, `request`, `content`, `identifier` and `userInfo`.
#[cfg(target_os = "macos")]
#[test]
fn a_click_reaches_the_handler_and_completes() {
    use block2::RcBlock;
    use std::sync::{Arc, Mutex};
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
    use objc2::{class, msg_send, sel};

    extern "C" fn this(this: *mut AnyObject, _: Sel) -> *mut AnyObject {
        this
    }
    extern "C" fn identifier(_: *mut AnyObject, _: Sel) -> *mut AnyObject {
        // SAFETY: invoked inside `did_receive`'s autorelease pool, so `mac::ns` returns an autoreleased `NSString`.
        unsafe { mac::ns("n-7") }
    }
    extern "C" fn user_info(_: *mut AnyObject, _: Sel) -> *mut AnyObject {
        // SAFETY: invoked inside `did_receive`'s autorelease pool, so `mac::dictionary` returns an autoreleased `NSDictionary`.
        unsafe { mac::dictionary(&[("session".into(), "ses_7".into()), ("server".into(), "http://127.0.0.1:4096".into())]) }
    }
    extern "C" fn action_identifier(_: *mut AnyObject, _: Sel) -> *mut AnyObject {
        // SAFETY: invoked inside `did_receive`'s autorelease pool, so `mac::ns` returns an autoreleased `NSString`.
        unsafe { mac::ns("com.apple.UNNotificationDefaultActionIdentifier") }
    }

    let fake = AnyClass::get(c"RuneMacFakeResponse").unwrap_or_else(|| {
        let mut b = ClassBuilder::new(c"RuneMacFakeResponse", class!(NSObject)).unwrap();
        // SAFETY: each function matches the Objective-C ABI for a zero-argument getter returning an object pointer.
        unsafe {
            b.add_method(sel!(notification), this as extern "C" fn(_, _) -> _);
            b.add_method(sel!(request), this as extern "C" fn(_, _) -> _);
            b.add_method(sel!(content), this as extern "C" fn(_, _) -> _);
            b.add_method(sel!(identifier), identifier as extern "C" fn(_, _) -> _);
            b.add_method(sel!(userInfo), user_info as extern "C" fn(_, _) -> _);
            b.add_method(sel!(actionIdentifier), action_identifier as extern "C" fn(_, _) -> _);
        }
        b.register()
    });

    let got = Arc::new(Mutex::new(Vec::new()));
    let sink = got.clone();
    *handler().lock().unwrap() = Some(Box::new(move |c| sink.lock().unwrap().push(c)));
    let completed = Arc::new(Mutex::new(false));
    let flag = completed.clone();
    let done = RcBlock::new(move || *flag.lock().unwrap() = true);
    // SAFETY: `fake` and `delegate_class()` are registered `NSObject` subclasses; `response`,
    // `delegate`, and `&*done` match the delegate selector inside this autorelease pool.
    objc2::rc::autoreleasepool(|_| unsafe {
        let response: *mut AnyObject = msg_send![fake, new];
        let delegate: *mut AnyObject = msg_send![mac::delegate_class(), new];
        let center: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![
            delegate,
            userNotificationCenter: center,
            didReceiveNotificationResponse: response,
            withCompletionHandler: &*done
        ];
    });
    *handler().lock().unwrap() = None;

    let got = got.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, "n-7");
    assert_eq!(got[0].get("session"), Some("ses_7"));
    assert_eq!(got[0].info.len(), 2);
    assert!(*completed.lock().unwrap(), "the completion handler runs");
}

/// Authorization requests include both alert and badge (so macOS does not hide the Dock badge set
/// via `NSDockTile.badgeLabel`) and omit sound. Uses a stand-in center object so the test never
/// touches `UNUserNotificationCenter` or asks the user for permission.
#[cfg(target_os = "macos")]
#[test]
fn authorization_requests_alert_and_badge_without_sound() {
    use block2::{Block, RcBlock};
    use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
    use objc2::{class, msg_send, sel};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    static SEEN: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn request_auth(
        _this: *mut AnyObject,
        _cmd: Sel,
        options: usize,
        done: &Block<dyn Fn(Bool, *mut AnyObject)>,
    ) {
        SEEN.store(options, Ordering::SeqCst);
        done.call((Bool::NO, std::ptr::null_mut()));
    }

    let fake = AnyClass::get(c"RuneMacFakeCenter").unwrap_or_else(|| {
        let mut b = ClassBuilder::new(c"RuneMacFakeCenter", class!(NSObject)).unwrap();
        // SAFETY: `request_auth` matches the Objective-C ABI for
        // `requestAuthorizationWithOptions:completionHandler:`.
        unsafe {
            b.add_method(
                sel!(requestAuthorizationWithOptions:completionHandler:),
                request_auth as extern "C" fn(_, _, _, _),
            );
        }
        b.register()
    });

    SEEN.store(0, Ordering::SeqCst);
    let completed = Arc::new(AtomicBool::new(false));
    let flag = completed.clone();
    let done = RcBlock::new(move |_granted: Bool, _error: *mut AnyObject| {
        flag.store(true, Ordering::SeqCst);
    });
    // SAFETY: `fake` is a registered `NSObject` subclass implementing the selector; `&*done` is a
    // valid ObjC completion block invoked synchronously inside this autorelease pool.
    objc2::rc::autoreleasepool(|_| unsafe {
        let center: *mut AnyObject = msg_send![fake, new];
        mac::request_authorization(center, &*done);
    });

    const UN_AUTH_BADGE: usize = 1 << 0;
    const UN_AUTH_SOUND: usize = 1 << 1;
    const UN_AUTH_ALERT: usize = 1 << 2;
    let options = SEEN.load(Ordering::SeqCst);
    assert!(completed.load(Ordering::SeqCst), "the completion handler runs");
    assert_ne!(options & UN_AUTH_BADGE, 0, "must request UNAuthorizationOptionBadge so Dock badges stay visible");
    assert_ne!(options & UN_AUTH_ALERT, 0, "must request UNAuthorizationOptionAlert for banners");
    assert_eq!(options & UN_AUTH_SOUND, 0, "must stay silent and omit UNAuthorizationOptionSound");
    assert_eq!(options, UN_AUTH_BADGE | UN_AUTH_ALERT);
}
