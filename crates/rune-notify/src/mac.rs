//! `UNUserNotificationCenter`. macOS only shows notifications for a bundled, signed app, so
//! outside one (`cargo run`, tests) nothing posts. The first post asks the person for
//! permission; until they allow it, posts are dropped. A notification's buttons and text box are a
//! category of its own (one per set of actions, registered with the center before the post); a
//! press comes back as the click, with the action and the words typed, without the app coming
//! forward (the actions don't ask for the foreground).

use super::{Action, Click, Notification};
use block2::{Block, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, msg_send_id, sel};

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

// UNAuthorizationOptionAlert: banners only, never a sound or badge.
const AUTH_ALERT: usize = 1 << 2;
// UNNotificationPresentationOptionList | Banner: shown even while the app is in front.
const PRESENT_LIST_BANNER: usize = (1 << 3) | (1 << 4);

pub(super) fn bundled() -> bool {
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        let id: *mut AnyObject = msg_send![bundle, bundleIdentifier];
        let path: *mut AnyObject = msg_send![bundle, bundlePath];
        !id.is_null() && string(path).is_some_and(|p| p.ends_with(".app"))
    }
}

unsafe fn center() -> *mut AnyObject {
    unsafe { msg_send![class!(UNUserNotificationCenter), currentNotificationCenter] }
}

pub(super) unsafe fn ns(s: &str) -> *mut AnyObject {
    let c = std::ffi::CString::new(s.replace('\0', "")).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

pub(super) unsafe fn string(s: *mut AnyObject) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let utf8: *const std::ffi::c_char = unsafe { msg_send![s, UTF8String] };
    (!utf8.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(utf8) }.to_string_lossy().into_owned())
}

pub(super) fn post(n: Notification) {
    let done = RcBlock::new(move |granted: Bool, _error: *mut AnyObject| {
        if granted.as_bool() {
            unsafe { add(&n) };
        }
    });
    unsafe {
        let _: () = msg_send![center(), requestAuthorizationWithOptions: AUTH_ALERT, completionHandler: &*done];
    }
}

unsafe fn add(n: &Notification) {
    objc2::rc::autoreleasepool(|_| unsafe {
        let content: Retained<AnyObject> = msg_send_id![class!(UNMutableNotificationContent), new];
        let content = Retained::as_ptr(&content).cast_mut();
        let _: () = msg_send![content, setTitle: ns(&n.title)];
        let _: () = msg_send![content, setBody: ns(&n.body)];
        let _: () = msg_send![content, setUserInfo: dictionary(&n.info)];
        if let Some(category) = category(&n.actions) {
            let _: () = msg_send![content, setCategoryIdentifier: ns(&category)];
        }
        let request: *mut AnyObject = msg_send![
            class!(UNNotificationRequest),
            requestWithIdentifier: ns(&n.id),
            content: content,
            trigger: std::ptr::null::<AnyObject>()
        ];
        let _: () = msg_send![center(), addNotificationRequest: request, withCompletionHandler: std::ptr::null::<Block<dyn Fn(*mut AnyObject)>>()];
    });
}

thread_local! {
    /// The categories registered this run, by identifier: the center keeps one set, replaced whole.
    static CATEGORIES: std::cell::RefCell<Vec<(String, Retained<AnyObject>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The category for `actions` (made and registered the first time): None without actions.
unsafe fn category(actions: &[Action]) -> Option<String> {
    if actions.is_empty() {
        return None;
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{actions:?}").hash(&mut h);
    let id = format!("rune.{:016x}", h.finish());
    if CATEGORIES.with_borrow(|c| c.iter().any(|(k, _)| *k == id)) {
        return Some(id);
    }
    unsafe {
        let made: Vec<*mut AnyObject> = actions.iter().take(10).map(|a| match a {
            // Options 0: no foreground, no authentication, not destructive.
            Action::Button { id, label } => msg_send![class!(UNNotificationAction), actionWithIdentifier: ns(id), title: ns(label), options: 0usize],
            Action::Reply { id, placeholder, label } => msg_send![class!(UNTextInputNotificationAction), actionWithIdentifier: ns(id), title: ns(label), options: 0usize, textInputButtonTitle: ns(label), textInputPlaceholder: ns(placeholder)],
        }).collect();
        let array: *mut AnyObject = msg_send![class!(NSArray), arrayWithObjects: made.as_ptr(), count: made.len()];
        let empty: *mut AnyObject = msg_send![class!(NSArray), array];
        let category: Retained<AnyObject> = msg_send_id![class!(UNNotificationCategory), categoryWithIdentifier: ns(&id), actions: array, intentIdentifiers: empty, options: 0usize];
        CATEGORIES.with_borrow_mut(|c| c.push((id.clone(), category)));
        let all: Vec<*mut AnyObject> = CATEGORIES.with_borrow(|c| c.iter().map(|(_, k)| Retained::as_ptr(k).cast_mut()).collect());
        let set: *mut AnyObject = msg_send![class!(NSSet), setWithObjects: all.as_ptr(), count: all.len()];
        let _: () = msg_send![center(), setNotificationCategories: set];
    }
    Some(id)
}

pub(super) unsafe fn dictionary(info: &[(String, String)]) -> *mut AnyObject {
    unsafe {
        let keys: Vec<*mut AnyObject> = info.iter().map(|(k, _)| ns(k)).collect();
        let values: Vec<*mut AnyObject> = info.iter().map(|(_, v)| ns(v)).collect();
        msg_send![class!(NSDictionary), dictionaryWithObjects: values.as_ptr(), forKeys: keys.as_ptr(), count: info.len()]
    }
}

pub(super) fn remove(ids: &[&str]) {
    objc2::rc::autoreleasepool(|_| unsafe {
        let items: Vec<*mut AnyObject> = ids.iter().map(|id| ns(id)).collect();
        let array: *mut AnyObject = msg_send![class!(NSArray), arrayWithObjects: items.as_ptr(), count: items.len()];
        let center = center();
        let _: () = msg_send![center, removeDeliveredNotificationsWithIdentifiers: array];
        let _: () = msg_send![center, removePendingNotificationRequestsWithIdentifiers: array];
    });
}

/// The click's notification id and string user info, from a `UNNotificationResponse`.
pub(super) unsafe fn click(response: *mut AnyObject) -> Click {
    unsafe {
        let notification: *mut AnyObject = msg_send![response, notification];
        let request: *mut AnyObject = msg_send![notification, request];
        let id = string(msg_send![request, identifier]).unwrap_or_default();
        let content: *mut AnyObject = msg_send![request, content];
        let info: *mut AnyObject = msg_send![content, userInfo];
        let mut pairs = Vec::new();
        if !info.is_null() {
            let keys: *mut AnyObject = msg_send![info, allKeys];
            let n: usize = msg_send![keys, count];
            for i in 0..n {
                let key: *mut AnyObject = msg_send![keys, objectAtIndex: i];
                let value: *mut AnyObject = msg_send![info, objectForKey: key];
                let is_string = |o: *mut AnyObject| -> bool { msg_send![o, isKindOfClass: class!(NSString)] };
                if is_string(key) && !value.is_null() && is_string(value) {
                    pairs.push((string(key).unwrap_or_default(), string(value).unwrap_or_default()));
                }
            }
            pairs.sort();
        }
        // A button: its identifier (not the notification's own default or dismiss action), and a
        // text box's words.
        let action = string(msg_send![response, actionIdentifier]).filter(|a| !a.starts_with("com.apple.UNNotification"));
        if let Some(action) = action {
            let typed: bool = msg_send![response, isKindOfClass: class!(UNTextInputNotificationResponse)];
            if typed && let Some(text) = string(msg_send![response, userText]) {
                pairs.push((format!("{}{action}", super::INPUT), text));
            }
            pairs.push((super::ACTION.to_owned(), action));
        }
        Click { id, info: pairs }
    }
}

pub(super) extern "C" fn did_receive(_this: *mut AnyObject, _cmd: Sel, _center: *mut AnyObject, response: *mut AnyObject, done: &Block<dyn Fn()>) {
    let click = objc2::rc::autoreleasepool(|_| unsafe { click(response) });
    super::deliver(click);
    done.call(());
}

extern "C" fn will_present(_this: *mut AnyObject, _cmd: Sel, _center: *mut AnyObject, _notification: *mut AnyObject, done: &Block<dyn Fn(usize)>) {
    done.call((PRESENT_LIST_BANNER,));
}

pub(super) fn delegate_class() -> &'static AnyClass {
    const NAME: &str = "RuneMacNotificationDelegate";
    if let Some(c) = AnyClass::get(NAME) {
        return c;
    }
    let mut b = ClassBuilder::new(NAME, class!(NSObject)).expect("delegate class");
    if let Some(p) = AnyProtocol::get("UNUserNotificationCenterDelegate") {
        b.add_protocol(p);
    }
    unsafe {
        b.add_method(
            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
            did_receive as extern "C" fn(_, _, _, _, _),
        );
        b.add_method(
            sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:),
            will_present as extern "C" fn(_, _, _, _, _),
        );
    }
    b.register()
}

pub(super) fn install_delegate() {
    static DELEGATE: std::sync::Once = std::sync::Once::new();
    DELEGATE.call_once(|| unsafe {
        // The center holds its delegate weakly: this one lives as long as the process.
        let delegate: *mut AnyObject = msg_send![delegate_class(), new];
        let _: () = msg_send![center(), setDelegate: delegate];
    });
}

pub(super) fn app_focused() -> bool {
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let active: bool = msg_send![app, isActive];
        let key: *mut AnyObject = msg_send![app, keyWindow];
        active && !key.is_null()
    }
}

pub(super) fn bring_to_front() {
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let mut w: *mut AnyObject = msg_send![app, mainWindow];
        if w.is_null() {
            let windows: *mut AnyObject = msg_send![app, windows];
            let n: usize = msg_send![windows, count];
            if n > 0 {
                w = msg_send![windows, objectAtIndex: 0usize];
            }
        }
        if !w.is_null() {
            let _: () = msg_send![w, deminiaturize: std::ptr::null::<AnyObject>()];
            let _: () = msg_send![w, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
        }
        let _: () = msg_send![app, activateIgnoringOtherApps: true];
    }
}
