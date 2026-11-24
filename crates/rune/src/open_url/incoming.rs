//! Receiving links (`myapp://...`) and files the system opens with this app:
//! `App::on_open_url` and `App::on_open_files`.
//!
//! winit 0.30 doesn't forward Apple Events, so on macOS Rune listens for both the get-URL event
//! (`kInternetEventClass`/`kAEGetURL`, `'GURL'`) and the open-documents event
//! (`kCoreEventClass`/`kAEOpenDocuments`, `'aevt'`/`'odoc'`). The event loop installs the
//! handlers before it starts (`prepare`) and keeps deliveries that arrive until a window whose
//! app has the matching hook starts (`listen`), so the link or files that launched the app arrive
//! too, whichever window has the hook. Once every window has started (`settle`), any handler with
//! no listening window is removed. An app that installed its own handler before the loop keeps it
//! unless one of its windows has the matching hook.
//!
//! The first listening window that's still running receives deliveries; when it goes the next one
//! does. Each delivery becomes an `AppHandle::update` running the hook: it runs before the
//! window's first frame when it arrived first, in arrival order on the main thread in a normal
//! transaction (state commits and the window redraws), and wakes an idle loop. Links and paths
//! come from outside the app, so the hook must validate them before acting.
//!
//! Windows hands a link to a new process as a launch argument: the links among the first
//! process's arguments are kept as the launching link is on macOS, and `single_instance` sends
//! a later launch's links here from the first process's pipe (`instance`). Links that arrive
//! before the loop starts are kept too. Linux does the same through a Unix socket. Files opened
//! with an app on Windows and Linux arrive as process arguments instead of an Apple Event, so
//! `App::on_open_files` never runs there. iOS (winit owns the scene delegate) is a follow-up.
// The web build runs no native window, so nothing here is reached there.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::app::{App, AppHandle};
use crate::el::Tx;

/// `App::on_open_url`'s hook.
pub(crate) type OnOpenUrl<S> = fn(&mut S, &str, &mut Tx);

/// `App::on_open_files`'s hook.
pub(crate) type OnOpenFiles<S> = fn(&mut S, &[PathBuf], &mut Tx);

type Sink<T> = Arc<dyn Fn(T) + Send + Sync>;

/// The platform's incoming-open handler.
pub(crate) trait Glue {
    /// Something else (the app itself) already handles these events.
    fn handled_elsewhere(&self) -> bool;
    /// Makes Rune's handler the one; false when it can't (off the main thread, another platform).
    fn install(&self) -> bool;
    fn remove(&self);
}

/// Where an incoming link or file batch goes, from launch until every window has started.
enum Funnel<T> {
    /// Nothing installed here (or the handler can't be): deliveries are kept for the first window
    /// whose app has the hook, as on Windows before the loop starts.
    Unprepared { kept: Vec<T> },
    /// The platform handler is Rune's; deliveries wait here for the first window with the hook.
    Launch { kept: Vec<T> },
    /// A window listens: deliveries go to the first one still running, in start order. Nothing
    /// waits for a later window, including after every window has started.
    Listening { listeners: Vec<(u64, Sink<T>)> },
    /// No handler is installed and no window listens: deliveries are dropped.
    Closed,
}

struct State<T> {
    funnel: Funnel<T>,
    next: u64,
}

/// Where incoming links (`T = String`) or file batches (`T = Vec<PathBuf>`) go.
pub(crate) struct Inbox<T: Send + 'static = String> {
    state: Mutex<State<T>>,
}

impl<T: Send + 'static> Inbox<T> {
    pub(crate) const fn new() -> Self {
        Inbox { state: Mutex::new(State { funnel: Funnel::Unprepared { kept: Vec::new() }, next: 0 }) }
    }

    /// Before the event loop: install the handler and keep deliveries until a window listens,
    /// unless something else handles them already.
    pub(crate) fn prepare(&self, glue: &dyn Glue) {
        let mut state = self.state.lock().unwrap();
        state.funnel = match std::mem::replace(&mut state.funnel, Funnel::Closed) {
            Funnel::Unprepared { kept } if !glue.handled_elsewhere() && glue.install() => Funnel::Launch { kept },
            funnel => funnel,
        };
    }

    fn listen_sink(&'static self, glue: &dyn Glue, sink: Sink<T>) -> Option<Listening<T>> {
        let mut state = self.state.lock().unwrap();
        let (mut listeners, kept) = match std::mem::replace(&mut state.funnel, Funnel::Closed) {
            // `Listening` and `Launch` already have the handler; `Unprepared` and `Closed`
            // install it here.
            Funnel::Listening { listeners } => (listeners, Vec::new()),
            Funnel::Launch { kept } => (Vec::new(), kept),
            Funnel::Unprepared { kept } => {
                if !glue.install() {
                    state.funnel = Funnel::Unprepared { kept };
                    return None;
                }
                (Vec::new(), kept)
            }
            Funnel::Closed => {
                if !glue.install() {
                    state.funnel = Funnel::Closed;
                    return None;
                }
                (Vec::new(), Vec::new())
            }
        };
        let id = state.next;
        state.next += 1;
        listeners.push((id, sink.clone()));
        state.funnel = Funnel::Listening { listeners };
        drop(state);
        for item in kept { sink(item); }
        Some(Listening { inbox: self, id })
    }

    /// Every window has started: stop keeping deliveries, and remove the handler if none listens.
    pub(crate) fn settle(&self, glue: &dyn Glue) {
        let mut state = self.state.lock().unwrap();
        state.funnel = match std::mem::replace(&mut state.funnel, Funnel::Closed) {
            Funnel::Listening { listeners } if !listeners.is_empty() => Funnel::Listening { listeners },
            Funnel::Launch { .. } | Funnel::Listening { .. } => { glue.remove(); Funnel::Closed },
            Funnel::Unprepared { .. } | Funnel::Closed => Funnel::Closed,
        };
    }

    /// A delivery arrived: to the first listener, kept before one exists, else dropped.
    #[cfg_attr(not(any(test, target_os = "macos", target_os = "windows", target_os = "linux")), allow(dead_code))]
    pub(crate) fn receive(&self, item: T) {
        let sink = {
            let mut state = self.state.lock().unwrap();
            match &mut state.funnel {
                Funnel::Unprepared { kept } | Funnel::Launch { kept } => {
                    kept.push(item);
                    return;
                }
                Funnel::Listening { listeners } => match listeners.first() {
                    Some((_, sink)) => sink.clone(),
                    None => return,
                },
                Funnel::Closed => return,
            }
        };
        sink(item);
    }
}

impl Inbox<String> {
    /// The links the app was launched with, ahead of any that arrived since.
    #[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
    pub(crate) fn launched(&self, urls: Vec<String>) {
        if urls.is_empty() { return; }
        let mut state = self.state.lock().unwrap();
        if let Funnel::Unprepared { kept } | Funnel::Launch { kept } = &mut state.funnel { kept.splice(0..0, urls); }
    }

    /// Sends links to `app`'s hook while the returned guard lives, after the links kept so far,
    /// if the app has the hook. Installs the handler if `prepare` didn't.
    #[inline]
    pub(crate) fn listen<S: 'static>(&'static self, app: &App<S>, glue: &dyn Glue) -> Option<Listening<String>> {
        let hook = app.on_open_url?;
        let handle: AppHandle<S> = app.handle();
        self.listen_sink(glue, Arc::new(move |url: String| {
            handle.update(move |s, tx| {
                #[cfg(feature = "hot")]
                crate::hot::patched_or_original!("on_open_url", hook, s, url.as_str(), tx);
                #[cfg(not(feature = "hot"))]
                hook(s, &url, tx);
            });
        }))
    }
}

impl Inbox<Vec<PathBuf>> {
    /// Sends opened file batches to `app`'s hook while the returned guard lives, after the
    /// batches kept so far, if the app has the hook. Installs the handler if `prepare` didn't.
    #[inline]
    pub(crate) fn listen_files<S: 'static>(&'static self, app: &App<S>, glue: &dyn Glue) -> Option<Listening<Vec<PathBuf>>> {
        let hook = app.on_open_files?;
        let handle: AppHandle<S> = app.handle();
        self.listen_sink(glue, Arc::new(move |paths: Vec<PathBuf>| {
            handle.update(move |s, tx| {
                #[cfg(feature = "hot")]
                crate::hot::patched_or_original!("on_open_files", hook, s, paths.as_slice(), tx);
                #[cfg(not(feature = "hot"))]
                hook(s, &paths, tx);
            });
        }))
    }
}

/// A window's place among one inbox's listeners; dropping it stops deliveries to that window.
pub(crate) struct Listening<T: Send + 'static = String> {
    inbox: &'static Inbox<T>,
    id: u64,
}

impl<T: Send + 'static> Drop for Listening<T> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.inbox.state.lock()
            && let Funnel::Listening { listeners } = &mut state.funnel
        {
            listeners.retain(|(id, _)| *id != self.id);
        }
    }
}

/// A window's place among the listeners for links (`App::on_open_url`) and files
/// (`App::on_open_files`), held by its host; dropping it stops its deliveries.
pub(crate) struct Listeners {
    _urls: Option<Listening<String>>,
    _files: Option<Listening<Vec<PathBuf>>>,
}

static INBOX: Inbox<String> = Inbox::new();
static FILES_INBOX: Inbox<Vec<PathBuf>> = Inbox::new();

struct NativeUrl;

impl Glue for NativeUrl {
    fn handled_elsewhere(&self) -> bool { platform::handled_elsewhere() }
    fn install(&self) -> bool { platform::install() }
    fn remove(&self) { platform::remove() }
}

struct NativeFiles;

impl Glue for NativeFiles {
    fn handled_elsewhere(&self) -> bool { platform::files_handled_elsewhere() }
    fn install(&self) -> bool { platform::files_install() }
    fn remove(&self) { platform::files_remove() }
}

/// Before the event loop starts (see the module docs).
pub(crate) fn prepare() {
    INBOX.prepare(&NativeUrl);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    INBOX.launched(super::instance::launch_links());
    FILES_INBOX.prepare(&NativeFiles);
}

/// A link from a later launch of the app (Windows, Linux, `single_instance`), or the Dock menu's
/// jump list entry (macOS, `native_menu`).
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn receive(url: String) { INBOX.receive(url); }

/// For a starting window: its app's links and opened files, while the guard lives.
pub(crate) fn listen<S: 'static>(app: &App<S>) -> Listeners {
    Listeners {
        _urls: INBOX.listen(app, &NativeUrl),
        _files: FILES_INBOX.listen_files(app, &NativeFiles),
    }
}

/// After every window at launch has started.
pub(crate) fn settle() {
    INBOX.settle(&NativeUrl);
    FILES_INBOX.settle(&NativeFiles);
}

#[cfg(target_os = "macos")]
use mac as platform;

/// Windows and Linux: links come in launch arguments and down `single_instance`'s pipe or socket,
/// so there is no handler to install, and nothing else can have taken it. Files come as launch
/// arguments, so the open-files handler stays inactive.
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod platform {
    pub(super) fn handled_elsewhere() -> bool { false }
    pub(super) fn install() -> bool { true }
    pub(super) fn remove() {}
    pub(super) fn files_handled_elsewhere() -> bool { false }
    pub(super) fn files_install() -> bool { false }
    pub(super) fn files_remove() {}
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod platform {
    pub(super) fn handled_elsewhere() -> bool { false }
    pub(super) fn install() -> bool { false }
    pub(super) fn remove() {}
    pub(super) fn files_handled_elsewhere() -> bool { false }
    pub(super) fn files_install() -> bool { false }
    pub(super) fn files_remove() {}
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use std::ffi::c_void;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, NSObject, Sel};
    use objc2::{ClassType, MainThreadMarker, class, msg_send, sel};
    use objc2_foundation::{NSString, NSURL};

    /// `'GURL'`: `kInternetEventClass` and `kAEGetURL`.
    const GET_URL: u32 = u32::from_be_bytes(*b"GURL");
    /// `'aevt'`: `kCoreEventClass`.
    const CORE_EVENT: u32 = u32::from_be_bytes(*b"aevt");
    /// `'odoc'`: `kAEOpenDocuments`.
    const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
    /// `'----'`: `keyDirectObject`, the event's direct parameter.
    const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
    /// `'list'`: `typeAEList`, an Apple Event descriptor list.
    const TYPE_AE_LIST: u32 = u32::from_be_bytes(*b"list");
    /// File descriptor types valid in an open-documents event (`typeFileURL`, `typeBookmarkData`,
    /// `typeAlias`, `typeFSRef`, `typeFSS`). Checking the type before `fileURLValue` avoids
    /// Apple Event Manager's legacy HFS-path coercion turning arbitrary `typeUnicodeText` strings
    /// into bogus root-relative file URLs.
    const TYPE_FILE_URL: u32 = u32::from_be_bytes(*b"furl");
    const TYPE_BOOKMARK_DATA: u32 = u32::from_be_bytes(*b"bmrk");
    const TYPE_ALIAS: u32 = u32::from_be_bytes(*b"alis");
    const TYPE_FS_REF: u32 = u32::from_be_bytes(*b"fsrf");
    const TYPE_FSS: u32 = u32::from_be_bytes(*b"fss ");
    /// Posted by AppKit after installing its default core Apple Event handlers and before
    /// dispatching queued launch Apple Events.
    const WILL_FINISH_LAUNCHING: &str = "NSApplicationWillFinishLaunchingNotification";

    static FILES_INSTALLED: AtomicBool = AtomicBool::new(false);

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn AEGetEventHandler(class: u32, id: u32, handler: *mut *const c_void, refcon: *mut *mut c_void, system: u8) -> i16;
    }

    /// The URL string of a get-URL event.
    pub(super) fn url_of(event: &AnyObject) -> Option<String> {
        // SAFETY: `event` is an NSAppleEventDescriptor; both messages are nil-safe and return
        // an autoreleased object or nil, released with the pool.
        autoreleasepool(|_| unsafe {
            let param: *mut AnyObject = msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT];
            let string: *mut NSString = msg_send![param.as_ref()?, stringValue];
            Some(string.as_ref()?.to_string())
        })
    }

    /// Extracts one file path from an `NSAppleEventDescriptor`, coercing `typeFileURL`,
    /// `typeAlias`, `typeFSRef` and `typeBookmarkData` via `fileURLValue`. Skips non-file URLs.
    fn path_of_descriptor(desc: &AnyObject) -> Option<PathBuf> {
        // SAFETY: `desc` is an NSAppleEventDescriptor; `descriptorType` returns its OSType.
        let desc_type: u32 = unsafe { msg_send![desc, descriptorType] };
        if !matches!(desc_type, TYPE_FILE_URL | TYPE_BOOKMARK_DATA | TYPE_ALIAS | TYPE_FS_REF | TYPE_FSS) {
            return None;
        }
        // SAFETY: `desc` is an NSAppleEventDescriptor; `fileURLValue` returns an autoreleased
        // `NSURL` or nil valid for the caller's autorelease pool.
        let url: &NSURL = unsafe {
            let ptr: *mut NSURL = msg_send![desc, fileURLValue];
            ptr.as_ref()?
        };
        if !url.isFileURL() { return None; }
        let resolved;
        let url = if url.isFileReferenceURL() {
            resolved = url.filePathURL()?;
            &*resolved
        } else {
            url
        };
        let path = url.path()?.to_string();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }

    /// The file paths of an open-documents (`'aevt'`/`'odoc'`) Apple Event.
    pub(super) fn files_of(event: &AnyObject) -> Vec<PathBuf> {
        // SAFETY: `event` is an NSAppleEventDescriptor; every message is nil-safe and returns
        // autoreleased objects released with the pool.
        autoreleasepool(|_| unsafe {
            let param: *mut AnyObject = msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT];
            let Some(param) = param.as_ref() else { return Vec::new() };
            let desc_type: u32 = msg_send![param, descriptorType];
            if desc_type == TYPE_AE_LIST {
                let count: isize = msg_send![param, numberOfItems];
                if count <= 0 { return Vec::new(); }
                let mut out = Vec::with_capacity((count as usize).min(1024));
                // Apple Event descriptor lists are 1-indexed.
                for index in 1..=count {
                    let item: *mut AnyObject = msg_send![param, descriptorAtIndex: index];
                    if let Some(path) = item.as_ref().and_then(path_of_descriptor) {
                        out.push(path);
                    }
                }
                out
            } else {
                path_of_descriptor(param).into_iter().collect()
            }
        })
    }

    extern "C" fn get_url(_: &AnyObject, _: Sel, event: *mut AnyObject, _reply: *mut AnyObject) {
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: AppKit passes the event, or nil.
            if let Some(url) = unsafe { event.as_ref() }.and_then(url_of) { super::INBOX.receive(url); }
        }));
        if caught.is_err() { eprintln!("rune: open-url handler panicked"); }
    }

    extern "C" fn open_documents(_: &AnyObject, _: Sel, event: *mut AnyObject, _reply: *mut AnyObject) {
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: AppKit passes the event, or nil.
            let files = unsafe { event.as_ref() }.map(files_of).unwrap_or_default();
            if !files.is_empty() { super::FILES_INBOX.receive(files); }
        }));
        if caught.is_err() { eprintln!("rune: open-files handler panicked"); }
    }

    extern "C" fn will_finish_launching(_: &AnyObject, _: Sel, _notification: *mut AnyObject) {
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // AppKit installs its own default core Apple Event handlers (`'aevt'`/`'odoc'`)
            // during `finishLaunching` right before posting this notification and dispatching
            // queued launch events; re-install Rune's handler while active so launch files arrive.
            if FILES_INSTALLED.load(Ordering::Acquire) {
                let _ = install_files_event_handler();
            }
        }));
        if caught.is_err() { eprintln!("rune: open-files launch observer panicked"); }
    }

    pub(super) fn handled_elsewhere() -> bool {
        let (mut handler, mut refcon) = (std::ptr::null(), std::ptr::null_mut());
        // SAFETY: reads the installed handler into the two out-pointers; 0 is noErr.
        unsafe { AEGetEventHandler(GET_URL, GET_URL, &mut handler, &mut refcon, 0) == 0 }
    }

    pub(super) fn files_handled_elsewhere() -> bool {
        let (mut handler, mut refcon) = (std::ptr::null(), std::ptr::null_mut());
        // SAFETY: reads the installed handler into the two out-pointers; 0 is noErr.
        unsafe { AEGetEventHandler(CORE_EVENT, OPEN_DOCUMENTS, &mut handler, &mut refcon, 0) == 0 }
    }

    /// Rune's handler object, made once and never released (the event manager doesn't retain it).
    fn handler() -> Option<*mut AnyObject> {
        static HANDLER: OnceLock<usize> = OnceLock::new();
        let made = *HANDLER.get_or_init(|| {
            const NAME: &std::ffi::CStr = c"RuneOpenUrlHandler";
            let class = AnyClass::get(NAME).or_else(|| {
                let mut builder = ClassBuilder::new(NAME, NSObject::class())?;
                // SAFETY: the function matches `handleGetURLEvent:withReplyEvent:` (two objects, void).
                unsafe { builder.add_method(sel!(handleGetURLEvent:withReplyEvent:), get_url as extern "C" fn(_, _, _, _)); }
                Some(builder.register())
            });
            let Some(class) = class else { return 0 };
            // SAFETY: `new` on an NSObject subclass; the +1 reference is kept for the process.
            let made: *mut AnyObject = unsafe { msg_send![class, new] };
            made as usize
        });
        (made != 0).then_some(made as *mut AnyObject)
    }

    /// Rune's open-documents handler object, made once and never released (the event manager
    /// doesn't retain it).
    fn files_handler() -> Option<*mut AnyObject> {
        static HANDLER: OnceLock<usize> = OnceLock::new();
        let made = *HANDLER.get_or_init(|| {
            const NAME: &std::ffi::CStr = c"RuneOpenFilesHandler";
            let class = AnyClass::get(NAME).or_else(|| {
                let mut builder = ClassBuilder::new(NAME, NSObject::class())?;
                // SAFETY: the functions match `handleOpenDocuments:withReplyEvent:` (two objects,
                // void) and `willFinishLaunching:` (one object, void).
                unsafe {
                    builder.add_method(sel!(handleOpenDocuments:withReplyEvent:), open_documents as extern "C" fn(_, _, _, _));
                    builder.add_method(sel!(willFinishLaunching:), will_finish_launching as extern "C" fn(_, _, _));
                }
                Some(builder.register())
            });
            let Some(class) = class else { return 0 };
            // SAFETY: `new` on an NSObject subclass; the +1 reference is kept for the process.
            let made: *mut AnyObject = unsafe { msg_send![class, new] };
            made as usize
        });
        (made != 0).then_some(made as *mut AnyObject)
    }

    fn manager() -> *mut AnyObject {
        // SAFETY: a class method returning the process's shared manager.
        unsafe { msg_send![class!(NSAppleEventManager), sharedAppleEventManager] }
    }

    /// Makes Rune's object the handler of get-URL Apple events. Main thread only.
    pub(super) fn install() -> bool {
        if MainThreadMarker::new().is_none() { return false; }
        let Some(handler) = handler() else { return false };
        // SAFETY: Foundation calls on the main thread, with the handler alive for the process.
        autoreleasepool(|_| unsafe {
            let Some(manager) = manager().as_ref() else { return false };
            let _: () = msg_send![manager, setEventHandler: handler, andSelector: sel!(handleGetURLEvent:withReplyEvent:),
                forEventClass: GET_URL, andEventID: GET_URL];
            true
        })
    }

    pub(super) fn remove() {
        if MainThreadMarker::new().is_none() { return; }
        // SAFETY: Foundation calls on the main thread.
        autoreleasepool(|_| unsafe {
            if let Some(manager) = manager().as_ref() {
                let _: () = msg_send![manager, removeEventHandlerForEventClass: GET_URL, andEventID: GET_URL];
            }
        })
    }

    fn install_files_event_handler() -> bool {
        if MainThreadMarker::new().is_none() { return false; }
        let Some(handler) = files_handler() else { return false };
        // SAFETY: Foundation calls on the main thread, with the handler alive for the process.
        autoreleasepool(|_| unsafe {
            let Some(manager) = manager().as_ref() else { return false };
            let _: () = msg_send![manager, setEventHandler: handler, andSelector: sel!(handleOpenDocuments:withReplyEvent:),
                forEventClass: CORE_EVENT, andEventID: OPEN_DOCUMENTS];
            true
        })
    }

    /// Makes Rune's object the handler of open-documents Apple events, and re-applies it on
    /// `NSApplicationWillFinishLaunchingNotification` after AppKit installs its default core
    /// event handlers. Main thread only.
    pub(super) fn files_install() -> bool {
        if !install_files_event_handler() { return false; }
        let Some(handler) = files_handler() else { return false };
        FILES_INSTALLED.store(true, Ordering::Release);
        // SAFETY: Foundation calls on the main thread; remove any prior observer on this singleton
        // before adding it so repeated installs never double-register.
        autoreleasepool(|_| unsafe {
            let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
            if let Some(center) = center.as_ref() {
                let name = NSString::from_str(WILL_FINISH_LAUNCHING);
                let _: () = msg_send![center, removeObserver: handler, name: &*name, object: std::ptr::null_mut::<AnyObject>()];
                let _: () = msg_send![center, addObserver: handler, selector: sel!(willFinishLaunching:),
                    name: &*name, object: std::ptr::null_mut::<AnyObject>()];
            }
        });
        true
    }

    pub(super) fn files_remove() {
        if MainThreadMarker::new().is_none() { return; }
        FILES_INSTALLED.store(false, Ordering::Release);
        // SAFETY: Foundation calls on the main thread.
        autoreleasepool(|_| unsafe {
            if let Some(handler) = files_handler() {
                let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
                if let Some(center) = center.as_ref() {
                    let name = NSString::from_str(WILL_FINISH_LAUNCHING);
                    let _: () = msg_send![center, removeObserver: handler, name: &*name, object: std::ptr::null_mut::<AnyObject>()];
                }
            }
            if let Some(manager) = manager().as_ref() {
                let _: () = msg_send![manager, removeEventHandlerForEventClass: CORE_EVENT, andEventID: OPEN_DOCUMENTS];
            }
        });
    }
}

#[cfg(test)]
#[path = "incoming_tests.rs"]
mod tests;
