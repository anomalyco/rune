//! System notifications through `UNUserNotificationCenter`: post one with an id, title, body and
//! string user info, take it back out of Notification Center, and hear about clicks.
//!
//! macOS only shows notifications for a bundled, signed app, so outside one (`cargo run`, tests)
//! `available` is false and `post` does nothing. Notifications are silent: they never set a sound.
//! The first post asks the person for permission; until they allow it, posts are dropped.

use std::sync::{Mutex, OnceLock};

/// One notification to post, built with `Notification::new(id, title)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notification {
    id: String,
    title: String,
    body: String,
    info: Vec<(String, String)>,
}

impl Notification {
    /// Posting another notification with the same `id` replaces this one.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Notification { id: id.into(), title: title.into(), ..Default::default() }
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// A string the click hands back (`Click::info`), such as what to open.
    pub fn info(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.info.push((key.into(), value.into()));
        self
    }
}

/// A click on one of the app's notifications, or on its default action.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Click {
    pub id: String,
    pub info: Vec<(String, String)>,
}

impl Click {
    /// The user-info value posted under `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.info.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

type Handler = Box<dyn Fn(Click) + Send + Sync>;

fn handler() -> &'static Mutex<Option<Handler>> {
    static HANDLER: OnceLock<Mutex<Option<Handler>>> = OnceLock::new();
    HANDLER.get_or_init(|| Mutex::new(None))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn deliver(click: Click) {
    if let Some(f) = handler().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        f(click);
    }
}

/// Whether this process can post: it runs from a `.app` bundle with a bundle identifier.
pub fn available() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::bundled()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Posts `n` now, asking for permission first if the person hasn't been asked. Returns false
/// when the app can't post at all (not `available`).
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
pub fn post(n: &Notification) -> bool {
    if !available() {
        return false;
    }
    #[cfg(target_os = "macos")]
    mac::post(n.clone());
    true
}

/// Takes the app's delivered notifications with these ids out of Notification Center.
pub fn remove(ids: &[&str]) {
    if ids.is_empty() || !available() {
        return;
    }
    #[cfg(target_os = "macos")]
    mac::remove(ids);
}

/// Calls `f` for every click on one of the app's notifications, from any thread (wake the app's
/// loop from it). macOS brings the app forward on a click; `bring_to_front` also shows a
/// minimized window. Install it at launch, so a click that launched the app arrives too.
pub fn on_click(f: impl Fn(Click) + Send + Sync + 'static) {
    *handler().lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    #[cfg(target_os = "macos")]
    if available() {
        mac::install_delegate();
    }
}

/// Whether the app is in front with one of its windows key, the moment it matters to a
/// notification: one that would show while the person is looking at the app is noise.
pub fn app_focused() -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::{class, msg_send, runtime::AnyObject};
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let active: bool = msg_send![app, isActive];
        let key: *mut AnyObject = msg_send![app, keyWindow];
        active && !key.is_null()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Activates the app and brings its main window forward, out of the Dock if minimized. Call it
/// on the main thread (from `poll`), say after a click.
pub fn bring_to_front() {
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::{class, msg_send, runtime::AnyObject};
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

#[cfg(target_os = "macos")]
mod mac {
    use super::{Click, Notification};
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
            let request: *mut AnyObject = msg_send![
                class!(UNNotificationRequest),
                requestWithIdentifier: ns(&n.id),
                content: content,
                trigger: std::ptr::null::<AnyObject>()
            ];
            let _: () = msg_send![center(), addNotificationRequest: request, withCompletionHandler: std::ptr::null::<Block<dyn Fn(*mut AnyObject)>>()];
        });
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
}

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
