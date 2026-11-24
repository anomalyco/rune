//! A launch by Launch Services (the Dock, Finder, `open`): what Rune starts beside AppKit's
//! own start, and the scheduling role the task holds while it launches.
//!
//! `NSApplication.shared` spends most of its time waiting for other processes (the window
//! server, Launch Services, `cfprefsd`), one after another on the main thread. What needs no
//! `NSApplication` is asked on threads meanwhile: whether there is a window server session at
//! all, the process's connection to the window server, and what AppKit loads once per process
//! (its symbol images, CoreText's font registry). (The first read of the app's
//! preferences is the first thing `NSApplication` does: a thread started here only waits for
//! the same answer.)
//!
//! macOS keeps an app Launch Services started in the role "default" (not known to be in
//! front) until AppKit has registered it as the front app, some 30,000 µs into
//! `NSApplication.shared`: until then its threads run on efficiency cores, and the performance
//! cores then start from their lowest clock. An app that takes activation at launch asks for the
//! foreground role itself as it starts, and gives it back if it isn't the active app once it
//! has launched (`open -g`, `open -j`, another app clicked meanwhile).

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// `task_role_t` of `<mach/task_policy.h>`.
pub(super) const TASK_FOREGROUND_APPLICATION: i32 = 1;
pub(super) const TASK_DEFAULT_APPLICATION: i32 = 7;
const TASK_CATEGORY_POLICY: u32 = 1;
/// `AU_SESSION_FLAG_HAS_GRAPHIC_ACCESS` of `<bsm/audit_session.h>`.
const HAS_GRAPHIC_ACCESS: u64 = 0x0010;
const NO_SESSION: &str = "event loop: no window server session (no one is logged in to this Mac)";

const UNRAISED: i32 = i32::MIN;
/// The role the task had before `begin` raised it; `UNRAISED` when it didn't, or gave it back.
static RAISED_FROM: AtomicI32 = AtomicI32::new(UNRAISED);
/// Whether the event loop has read what the session thread found.
static TAKEN: AtomicBool = AtomicBool::new(false);

/// `struct auditinfo_addr` of `<bsm/audit.h>`.
#[repr(C)]
struct AuditInfo { auid: u32, mask: [u32; 2], port: i32, kind: u32, addr: [u32; 4], asid: i32, flags: u64 }

unsafe extern "C" {
    static mach_task_self_: u32;
    fn task_policy_set(task: u32, flavor: u32, info: *const i32, count: u32) -> i32;
    fn task_policy_get(task: u32, flavor: u32, info: *mut i32, count: *mut u32, get_default: *mut i32) -> i32;
    fn getaudit_addr(info: *mut AuditInfo, length: i32) -> i32;
    fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> *const c_void;
    fn CGMainDisplayID() -> u32;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}
#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontCreateUIFontForLanguage(kind: u32, size: f64, language: *const c_void) -> *const c_void;
}
/// `kCTFontUIFontSystem`.
const UI_FONT_SYSTEM: u32 = 2;

/// What `begin` started; the event loop asks it for the session before it runs.
pub(crate) struct Beside { session: Option<mpsc::Receiver<bool>> }

impl Beside {
    /// Whether this process can reach the window server. Waits for the thread that asks.
    pub(crate) fn session(self) -> bool {
        let Some(found) = self.session else { return true };
        let found = found.recv().unwrap_or_else(|_| window_server_session());
        TAKEN.store(true, Ordering::Release);
        found
    }
}

/// As the event loop starts, before `NSApplication.shared`. `Err` when there is no window
/// server session to launch in. `takes_activation`: the app comes to the front as it launches
/// (not an accessory app, not a tool's inactive launch). `RUNE_LAUNCH_PLAIN` keeps everything
/// on the main thread and the role alone.
pub(crate) fn begin(takes_activation: bool) -> Result<Beside, String> {
    // Not a login session with a screen (ssh, a daemon): asked before anything of AppKit starts.
    if std::env::var_os("RUNE_LAUNCH_PLAIN").is_some() || !graphic_session() {
        return if window_server_session() { Ok(Beside { session: None }) } else { Err(NO_SESSION.into()) };
    }
    if raises(role(), takes_activation) && set_role(TASK_FOREGROUND_APPLICATION) {
        RAISED_FROM.store(TASK_DEFAULT_APPLICATION, Ordering::Release);
    }
    let (found, session) = mpsc::channel();
    let asked = std::thread::Builder::new().name("rune-launch-session".into()).spawn(move || {
        interactive();
        let session = window_server_session();
        found.send(session).ok();
        if session { return; }
        // Without a session AppKit may never come back to the loop that reads the answer.
        std::thread::sleep(Duration::from_secs(2));
        if !TAKEN.load(Ordering::Acquire) {
            eprintln!("{NO_SESSION}");
            std::process::exit(1);
        }
    });
    if asked.is_err() {
        return if window_server_session() { Ok(Beside { session: None }) } else { Err(NO_SESSION.into()) };
    }
    // The connection AppKit asks for once it has registered with Launch Services, 15,000 us in.
    std::thread::Builder::new().name("rune-launch-display".into()).spawn(|| {
        interactive();
        // SAFETY: no arguments; CoreGraphics makes the connection once, under its own lock.
        unsafe { CGMainDisplayID() };
    }).ok();
    // What AppKit loads once per process on whichever thread asks first: its symbol images
    // (the first menu item has one) and CoreText's font registry (the first title bar).
    std::thread::Builder::new().name("rune-launch-symbols".into()).spawn(|| {
        interactive();
        symbol_image();
    }).ok();
    std::thread::Builder::new().name("rune-launch-fonts".into()).spawn(|| {
        interactive();
        // SAFETY: a Create-rule CF function; a non-null result is released once.
        unsafe {
            let font = CTFontCreateUIFontForLanguage(UI_FONT_SYSTEM, 13.0, std::ptr::null());
            if !font.is_null() { CFRelease(font); }
        }
    }).ok();
    Ok(Beside { session: Some(session) })
}

/// As the event loop starts, before `NSApplication.shared`. AppKit keeps each window's state
/// for the next launch (Resume) and, when it finds any, holds every window of that launch off
/// the screen until its restorer has run, after `applicationDidFinishLaunching:`: a first
/// frame handed to the window server in `applicationWillFinishLaunching:` then reaches the
/// screen 50,000 to 65,000 µs later. AppKit can't restore a Rune window (it has no restoration
/// class), so the launch ignores that state, as `-ApplePersistenceIgnoreState YES` does. A
/// registered default: the user's own setting and that argument still win.
/// `RUNE_APPKIT_RESTORES` keeps AppKit's order.
pub(crate) fn ignore_saved_windows() {
    use objc2::msg_send;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::{NSString, NSUserDefaults};
    if !ignores_saved_windows() { return; }
    autoreleasepool(|_| {
        let (Some(number), Some(dictionary)) = (AnyClass::get(c"NSNumber"), AnyClass::get(c"NSDictionary")) else { return };
        let key = NSString::from_str(IGNORE_SAVED_WINDOWS);
        // SAFETY: Foundation class methods with an object and a key; `registerDefaults:` takes
        // the dictionary they make.
        unsafe {
            let yes: Option<Retained<AnyObject>> = msg_send![number, numberWithBool: Bool::YES];
            let Some(yes) = yes else { return };
            let values: Option<Retained<AnyObject>> = msg_send![dictionary, dictionaryWithObject: &*yes, forKey: &*key];
            let Some(values) = values else { return };
            let _: () = msg_send![&*NSUserDefaults::standardUserDefaults(), registerDefaults: &*values];
        }
    });
}

/// Whether Rune keeps AppKit's window restoration out of the app (not under `RUNE_APPKIT_RESTORES`).
pub(crate) fn ignores_saved_windows() -> bool {
    static IGNORES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IGNORES.get_or_init(|| std::env::var_os("RUNE_APPKIT_RESTORES").is_none())
}

/// The default AppKit reads before it looks for saved window state.
pub(super) const IGNORE_SAVED_WINDOWS: &str = "ApplePersistenceIgnoreState";

/// Each turn of the event loop. Once AppKit has launched the app, a role `begin` raised is
/// looked at again a second later, when AppKit knows whether the app is the active one.
pub(crate) fn settle() {
    if RAISED_FROM.load(Ordering::Relaxed) == UNRAISED || super::launching() { return; }
    let from = RAISED_FROM.swap(UNRAISED, Ordering::AcqRel);
    if from == UNRAISED { return; }
    std::thread::Builder::new().name("rune-launch-role".into()).spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        if let Some(role) = gives_back(from, active(), role()) { set_role(role); }
    }).ok();
}

/// Whether a launching task in `role` asks for the foreground role: only an app Launch
/// Services started that the system hasn't placed yet, and that comes to the front.
pub(super) fn raises(role: Option<i32>, takes_activation: bool) -> bool {
    takes_activation && role == Some(TASK_DEFAULT_APPLICATION)
}

/// The role to go back to after the launch, if any: the one the task had, when the app isn't
/// the active one and the system hasn't given the task another role since.
pub(super) fn gives_back(from: i32, active: bool, now: Option<i32>) -> Option<i32> {
    (!active && now == Some(TASK_FOREGROUND_APPLICATION)).then_some(from)
}

pub(super) fn role() -> Option<i32> {
    let (mut role, mut count, mut default) = (0i32, 1u32, 0i32);
    // SAFETY: this task's own port; one integer out, as `TASK_CATEGORY_POLICY_COUNT` says.
    let result = unsafe { task_policy_get(mach_task_self_, TASK_CATEGORY_POLICY, &mut role, &mut count, &mut default) };
    (result == 0).then_some(role)
}

pub(super) fn set_role(role: i32) -> bool {
    // SAFETY: this task's own port; one integer in. macOS lets a task set its own role.
    unsafe { task_policy_set(mach_task_self_, TASK_CATEGORY_POLICY, &role, 1) == 0 }
}

/// Whether the process is in a login session with a screen (the audit session's flag).
fn graphic_session() -> bool {
    let mut info = AuditInfo { auid: 0, mask: [0; 2], port: 0, kind: 0, addr: [0; 4], asid: 0, flags: 0 };
    // SAFETY: the kernel fills the struct, whose size is passed.
    unsafe { getaudit_addr(&mut info, std::mem::size_of::<AuditInfo>() as i32) == 0 && info.flags & HAS_GRAPHIC_ACCESS != 0 }
}

/// Whether this process can reach the window server. Over ssh to a Mac no one is logged in to,
/// AppKit starts but never finishes launching, so the app would wait forever.
fn window_server_session() -> bool {
    // SAFETY: a Copy-rule CF function; a non-null result is released exactly once.
    unsafe {
        let session = CGSessionCopyCurrentDictionary();
        if session.is_null() { return false; }
        CFRelease(session);
    }
    true
}

/// Whether the app is the active one. `NSRunningApplication` answers on any thread.
fn active() -> bool {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    // SAFETY: a class method and a property read.
    unsafe {
        let Some(running) = AnyClass::get(c"NSRunningApplication") else { return true };
        let current: *const AnyObject = msg_send![running, currentApplication];
        if current.is_null() { return true; }
        let active: Bool = msg_send![current, isActive];
        active.as_bool()
    }
}

/// Makes one system symbol image, which loads AppKit's symbol catalogs. `NSImage` may be made
/// on any thread.
fn symbol_image() {
    use objc2::msg_send;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSString;
    autoreleasepool(|_| {
        let Some(image) = AnyClass::get(c"NSImage") else { return };
        let name = NSString::from_str("info.circle");
        // SAFETY: a class method taking a string and a nullable string; the image is dropped here.
        let _: Option<Retained<AnyObject>> = unsafe { msg_send![image, imageWithSystemSymbolName: &*name, accessibilityDescription: None::<&NSString>] };
    });
}

/// The threads' waits are the main thread's: they run at its priority.
fn interactive() {
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
    // SAFETY: sets the calling thread's own QoS class.
    unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
