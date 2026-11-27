//! `UNUserNotificationCenter`. macOS only shows notifications for a bundled, signed app, so
//! outside one (`cargo run`, tests) nothing posts. The first post asks the person for
//! permission; until they allow it, posts are dropped. A notification's buttons and text box are a
//! category of its own (one per set of actions, registered with the center before the post); a
//! press comes back as the click, with the action and the words typed, without the app coming
//! forward (the actions don't ask for the foreground).

#![allow(unsafe_code)]

use super::{Action, Click, Notification};
use block2::{Block, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

// UNAuthorizationOptionBadge | UNAuthorizationOptionAlert: banners and Dock icon badges.
// Notifications themselves never set `content.badge`; including the badge option keeps macOS from
// suppressing `NSDockTile.badgeLabel` (`WindowHandle::set_taskbar`) and exposes the "Badge
// application icon" toggle in System Settings. UNAuthorizationOptionSound is added only when a
// notification opts in via `Notification::sound(true)`.
const AUTH_OPTIONS: usize = (1 << 0) | (1 << 2);
const AUTH_SOUND: usize = 1 << 1;
// UNNotificationPresentationOptionList | Banner: shown even while the app is in front.
const PRESENT_LIST_BANNER: usize = (1 << 3) | (1 << 4);
const PRESENT_SOUND: usize = 1 << 1;

pub(super) fn auth_options(sound: bool) -> usize {
    if sound { AUTH_OPTIONS | AUTH_SOUND } else { AUTH_OPTIONS }
}

pub(super) fn bundled() -> bool {
    // SAFETY: `NSBundle.mainBundle`, `bundleIdentifier`, and `bundlePath` are thread-safe
    // Foundation getters; `string` checks for null.
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        let id: *mut AnyObject = msg_send![bundle, bundleIdentifier];
        let path: *mut AnyObject = msg_send![bundle, bundlePath];
        !id.is_null() && string(path).is_some_and(|p| p.ends_with(".app"))
    }
}

unsafe fn center() -> *mut AnyObject {
    // SAFETY: callers gate on `bundled()`, so `currentNotificationCenter` returns the app's
    // singleton `UNUserNotificationCenter` without throwing.
    unsafe { msg_send![class!(UNUserNotificationCenter), currentNotificationCenter] }
}

pub(super) unsafe fn ns(s: &str) -> *mut AnyObject {
    let c = std::ffi::CString::new(s.replace('\0', "")).unwrap_or_default();
    // SAFETY: `c` is a NUL-terminated UTF-8 C string that outlives the call; caller holds an
    // autorelease pool for the returned autoreleased `NSString`.
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

pub(super) unsafe fn string(s: *mut AnyObject) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: caller guarantees non-null `s` is a live `NSString`; `UTF8String` returns a
    // NUL-terminated buffer valid for `s`'s lifetime (or null).
    let utf8: *const std::ffi::c_char = unsafe { msg_send![s, UTF8String] };
    // SAFETY: `utf8` is checked non-null and points to a NUL-terminated C string owned by `s`.
    (!utf8.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(utf8) }.to_string_lossy().into_owned())
}

pub(super) unsafe fn request_authorization(center: *mut AnyObject, done: &Block<dyn Fn(Bool, *mut AnyObject)>) {
    // SAFETY: caller passes a live `UNUserNotificationCenter` (or test stand-in) and a valid
    // ObjC completion block matching `requestAuthorizationWithOptions:completionHandler:`.
    unsafe { request_authorization_with_options(center, AUTH_OPTIONS, done) }
}

pub(super) unsafe fn request_authorization_with_options(
    center: *mut AnyObject,
    options: usize,
    done: &Block<dyn Fn(Bool, *mut AnyObject)>,
) {
    // SAFETY: caller passes a live `UNUserNotificationCenter` (or test stand-in) and a valid
    // ObjC completion block matching `requestAuthorizationWithOptions:completionHandler:`.
    unsafe {
        let _: () = msg_send![center, requestAuthorizationWithOptions: options, completionHandler: done];
    }
}

pub(super) fn post(n: Notification) {
    let options = auth_options(n.sound);
    let done = RcBlock::new(move |granted: Bool, _error: *mut AnyObject| {
        if granted.as_bool() {
            // SAFETY: `post` is gated on `bundled()`, and `add` runs inside its own autorelease pool.
            unsafe { add(&n) };
        }
    });
    // SAFETY: `post` is gated on `bundled()`; `&*done` is a valid ObjC block copied by the center.
    unsafe {
        request_authorization_with_options(center(), options, &done);
    }
}

unsafe fn add(n: &Notification) {
    // SAFETY: invoked inside an autorelease pool for a bundled app; every Foundation and
    // `UNUserNotificationCenter` message matches its selector and signature.
    objc2::rc::autoreleasepool(|_| unsafe {
        let content: Retained<AnyObject> = msg_send![class!(UNMutableNotificationContent), new];
        let content = Retained::as_ptr(&content).cast_mut();
        let _: () = msg_send![content, setTitle: ns(&n.title)];
        let _: () = msg_send![content, setBody: ns(&n.body)];
        let _: () = msg_send![content, setUserInfo: dictionary(&n.info)];
        if n.sound {
            let sound: *mut AnyObject = msg_send![class!(UNNotificationSound), defaultSound];
            let _: () = msg_send![content, setSound: sound];
        }
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
    // SAFETY: called from `add` inside an autorelease pool for a bundled app; `made` and `all`
    // stay live for `arrayWithObjects:count:` / `setWithObjects:count:`, and `CATEGORIES` retains
    // every `UNNotificationCategory` object.
    unsafe {
        let made: Vec<*mut AnyObject> = actions.iter().take(10).map(|a| match a {
            // Options 0: no foreground, no authentication, not destructive.
            Action::Button { id, label } => msg_send![class!(UNNotificationAction), actionWithIdentifier: ns(id), title: ns(label), options: 0usize],
            Action::Reply { id, placeholder, label } => msg_send![class!(UNTextInputNotificationAction), actionWithIdentifier: ns(id), title: ns(label), options: 0usize, textInputButtonTitle: ns(label), textInputPlaceholder: ns(placeholder)],
        }).collect();
        let array: *mut AnyObject = msg_send![class!(NSArray), arrayWithObjects: made.as_ptr(), count: made.len()];
        let empty: *mut AnyObject = msg_send![class!(NSArray), array];
        let category: Retained<AnyObject> = msg_send![class!(UNNotificationCategory), categoryWithIdentifier: ns(&id), actions: array, intentIdentifiers: empty, options: 0usize];
        CATEGORIES.with_borrow_mut(|c| c.push((id.clone(), category)));
        let all: Vec<*mut AnyObject> = CATEGORIES.with_borrow(|c| c.iter().map(|(_, k)| Retained::as_ptr(k).cast_mut()).collect());
        let set: *mut AnyObject = msg_send![class!(NSSet), setWithObjects: all.as_ptr(), count: all.len()];
        let _: () = msg_send![center(), setNotificationCategories: set];
    }
    Some(id)
}

pub(super) unsafe fn dictionary(info: &[(String, String)]) -> *mut AnyObject {
    // SAFETY: caller holds an autorelease pool; `keys` and `values` both hold `info.len()` live
    // `NSString` pointers for `dictionaryWithObjects:forKeys:count:`.
    unsafe {
        let keys: Vec<*mut AnyObject> = info.iter().map(|(k, _)| ns(k)).collect();
        let values: Vec<*mut AnyObject> = info.iter().map(|(_, v)| ns(v)).collect();
        msg_send![class!(NSDictionary), dictionaryWithObjects: values.as_ptr(), forKeys: keys.as_ptr(), count: info.len()]
    }
}

pub(super) fn remove(ids: &[&str]) {
    // SAFETY: gated on `bundled()` and wrapped in an autorelease pool; `items` holds `ids.len()`
    // live `NSString` pointers for `arrayWithObjects:count:`.
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
    // SAFETY: caller passes a live `UNNotificationResponse` inside an autorelease pool; keys and
    // values are checked with `isKindOfClass:` before `string`, and `userText` is read only when
    // `response` is a `UNTextInputNotificationResponse`.
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
    // SAFETY: `UNUserNotificationCenter` passes a live `UNNotificationResponse` for this callback,
    // and `click` runs inside this autorelease pool.
    let click = objc2::rc::autoreleasepool(|_| unsafe { click(response) });
    super::deliver(click);
    done.call(());
}

extern "C" fn will_present(_this: *mut AnyObject, _cmd: Sel, _center: *mut AnyObject, notification: *mut AnyObject, done: &Block<dyn Fn(usize)>) {
    // SAFETY: `UNUserNotificationCenter` passes a live `UNNotification` (or null in synthetic
    // calls); each pointer is checked for null before messaging.
    let has_sound = unsafe {
        if notification.is_null() {
            false
        } else {
            let request: *mut AnyObject = msg_send![notification, request];
            let content: *mut AnyObject = if request.is_null() { std::ptr::null_mut() } else { msg_send![request, content] };
            let sound: *mut AnyObject = if content.is_null() { std::ptr::null_mut() } else { msg_send![content, sound] };
            !sound.is_null()
        }
    };
    let options = if has_sound { PRESENT_LIST_BANNER | PRESENT_SOUND } else { PRESENT_LIST_BANNER };
    done.call((options,));
}

pub(super) fn delegate_class() -> &'static AnyClass {
    const NAME: &std::ffi::CStr = c"RuneMacNotificationDelegate";
    if let Some(c) = AnyClass::get(NAME) {
        return c;
    }
    let mut b = ClassBuilder::new(NAME, class!(NSObject)).expect("delegate class");
    if let Some(p) = AnyProtocol::get(c"UNUserNotificationCenterDelegate") {
        b.add_protocol(p);
    }
    // SAFETY: both `did_receive` and `will_present` match the Objective-C ABI and parameter types
    // of `UNUserNotificationCenterDelegate`'s selectors.
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
    // SAFETY: `install_delegate` is gated on `bundled()`; `new` allocates a retained delegate
    // kept alive for the process because the center holds its delegate weakly.
    DELEGATE.call_once(|| unsafe {
        // The center holds its delegate weakly: this one lives as long as the process.
        let delegate: *mut AnyObject = msg_send![delegate_class(), new];
        let _: () = msg_send![center(), setDelegate: delegate];
    });
}

pub(super) fn app_focused() -> bool {
    // SAFETY: standard AppKit `NSApplication` queries (`sharedApplication`, `isActive`, `keyWindow`)
    // on the UI thread; `key` is only tested for null.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let active: bool = msg_send![app, isActive];
        let key: *mut AnyObject = msg_send![app, keyWindow];
        active && !key.is_null()
    }
}

pub(super) fn bring_to_front() {
    // SAFETY: called on the AppKit main thread (`poll`); `objectAtIndex: 0` is guarded by `n > 0`,
    // and window messages are sent only when `w` is non-null.
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
