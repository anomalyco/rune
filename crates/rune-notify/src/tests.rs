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
        unsafe { mac::ns("n-7") }
    }
    extern "C" fn user_info(_: *mut AnyObject, _: Sel) -> *mut AnyObject {
        unsafe { mac::dictionary(&[("session".into(), "ses_7".into()), ("server".into(), "http://127.0.0.1:4096".into())]) }
    }

    let fake = AnyClass::get("RuneMacFakeResponse").unwrap_or_else(|| {
        let mut b = ClassBuilder::new("RuneMacFakeResponse", class!(NSObject)).unwrap();
        unsafe {
            b.add_method(sel!(notification), this as extern "C" fn(_, _) -> _);
            b.add_method(sel!(request), this as extern "C" fn(_, _) -> _);
            b.add_method(sel!(content), this as extern "C" fn(_, _) -> _);
            b.add_method(sel!(identifier), identifier as extern "C" fn(_, _) -> _);
            b.add_method(sel!(userInfo), user_info as extern "C" fn(_, _) -> _);
        }
        b.register()
    });

    let got = Arc::new(Mutex::new(Vec::new()));
    let sink = got.clone();
    *handler().lock().unwrap() = Some(Box::new(move |c| sink.lock().unwrap().push(c)));
    let completed = Arc::new(Mutex::new(false));
    let flag = completed.clone();
    let done = RcBlock::new(move || *flag.lock().unwrap() = true);
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
