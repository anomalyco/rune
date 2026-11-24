use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{Glue, Inbox};
use crate::el::el;
use crate::runtime::{Cx, Env, Runtime};
use crate::{App, AppSpec};

type Links = Vec<String>;

/// A get-URL handler that records what was asked of it.
#[derive(Default)]
struct Fake {
    elsewhere: bool,
    refuse: bool,
    installs: Cell<usize>,
    removes: Cell<usize>,
}

impl Glue for Fake {
    fn handled_elsewhere(&self) -> bool { self.elsewhere }
    fn install(&self) -> bool { self.installs.set(self.installs.get() + 1); !self.refuse }
    fn remove(&self) { self.removes.set(self.removes.get() + 1); }
}

fn inbox() -> &'static Inbox { Box::leak(Box::new(Inbox::new())) }

fn app() -> App<Links> {
    let spec = AppSpec::new("links", (100.0, 100.0), Links::new, |_: &Links, _: &mut Cx<Links>| el());
    App::new(spec).on_open_url(|links, url, _| links.push(url.to_owned()))
}

fn plain() -> App<Links> {
    App::new(AppSpec::new("plain", (100.0, 100.0), Links::new, |_: &Links, _: &mut Cx<Links>| el()))
}

/// What a window host does as it starts and when woken: run the queued updates.
fn deliver(app: &App<Links>, rt: &mut Runtime<Links>) -> usize {
    let updates = app.shared.take_updates();
    let n = updates.len();
    for f in updates { rt.update_if(f); }
    n
}

fn runtime() -> Runtime<Links> {
    Runtime::new(Links::new(), |_: &Links, _: &mut Cx<Links>| el(), (100.0, 100.0), Env::default())
}

#[test]
fn without_a_hook_nothing_listens_and_the_launch_handler_goes() {
    let (inbox, glue) = (inbox(), Fake::default());
    inbox.prepare(&glue);
    inbox.receive("app://kept".into());
    assert!(inbox.listen(&plain(), &glue).is_none());
    inbox.settle(&glue);
    assert_eq!((glue.installs.get(), glue.removes.get()), (1, 1), "installed for launch, removed after it");
    let late = app();
    inbox.receive("app://dropped".into());
    let _listening = inbox.listen(&late, &glue).unwrap();
    assert!(late.shared.take_updates().is_empty(), "links from before settling are gone");
}

#[test]
fn links_before_the_first_window_arrive_once_in_order_as_it_starts() {
    let (inbox, glue, app) = (inbox(), Fake::default(), app());
    inbox.prepare(&glue);
    inbox.receive("app://one".into());
    inbox.receive("app://two?x=1".into());
    let _listening = inbox.listen(&app, &glue).unwrap();
    inbox.settle(&glue);
    let mut rt = runtime();
    assert!(rt.state.is_empty(), "nothing runs before the host starts");
    assert_eq!(deliver(&app, &mut rt), 2);
    assert_eq!(rt.state, ["app://one", "app://two?x=1"]);
    assert!(rt.needs_commit(), "the transaction commits and redraws");
    assert_eq!(deliver(&app, &mut rt), 0, "delivered once");
    assert_eq!((glue.installs.get(), glue.removes.get()), (1, 0));
}

#[test]
fn a_later_window_with_the_hook_gets_the_launch_link() {
    let (inbox, glue, first, second) = (inbox(), Fake::default(), plain(), app());
    inbox.prepare(&glue);
    inbox.receive("app://launch".into());
    assert!(inbox.listen(&first, &glue).is_none());
    let _listening = inbox.listen(&second, &glue).unwrap();
    inbox.settle(&glue);
    assert_eq!(second.shared.take_updates().len(), 1);
    assert_eq!(glue.removes.get(), 0);
}

#[test]
fn links_while_running_wake_the_loop_and_arrive() {
    let (inbox, glue, app) = (inbox(), Fake::default(), app());
    let _listening = inbox.listen(&app, &glue).unwrap();
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    app.shared.connect(Arc::new(move || { w.fetch_add(1, Ordering::SeqCst); }));
    let before = wakes.load(Ordering::SeqCst);
    inbox.receive("app://later".into());
    assert_eq!(wakes.load(Ordering::SeqCst), before + 1);
    let mut rt = runtime();
    assert_eq!(deliver(&app, &mut rt), 1);
    assert_eq!(rt.state, ["app://later"]);
}

#[test]
fn when_the_listening_window_goes_the_next_one_takes_over() {
    let (inbox, glue, first, second) = (inbox(), Fake::default(), app(), app());
    let listening = inbox.listen(&first, &glue).unwrap();
    let _second = inbox.listen(&second, &glue).unwrap();
    inbox.receive("app://a".into());
    drop(listening);
    inbox.receive("app://b".into());
    assert_eq!(first.shared.take_updates().len(), 1, "a closed window gets nothing more");
    assert_eq!(second.shared.take_updates().len(), 1);
    assert_eq!(glue.installs.get(), 1, "installed once");
}

#[test]
fn an_apps_own_handler_stays_unless_a_window_has_the_hook() {
    let (inbox, glue) = (inbox(), Fake { elsewhere: true, ..Fake::default() });
    inbox.prepare(&glue);
    inbox.receive("app://theirs".into());
    inbox.settle(&glue);
    assert_eq!((glue.installs.get(), glue.removes.get()), (0, 0));
    let app = app();
    let _listening = inbox.listen(&app, &glue).unwrap();
    assert_eq!(glue.installs.get(), 1);
    assert!(app.shared.take_updates().is_empty());
}

#[test]
fn a_handler_that_cant_install_listens_to_nothing() {
    let (inbox, app) = (inbox(), app());
    assert!(inbox.listen(&app, &Fake { refuse: true, ..Fake::default() }).is_none(), "off the main thread, or another platform");
    inbox.receive("app://dropped".into());
    assert!(app.shared.take_updates().is_empty());
    assert!(inbox.listen(&app, &Fake::default()).is_some());
}

type Files = Vec<Vec<std::path::PathBuf>>;

fn files_inbox() -> &'static Inbox<Vec<std::path::PathBuf>> { Box::leak(Box::new(Inbox::new())) }

fn files_app() -> App<Files> {
    let spec = AppSpec::new("files", (100.0, 100.0), Files::new, |_: &Files, _: &mut Cx<Files>| el());
    App::new(spec).on_open_files(|opened, paths, _| opened.push(paths.to_vec()))
}

fn plain_files() -> App<Files> {
    App::new(AppSpec::new("plain-files", (100.0, 100.0), Files::new, |_: &Files, _: &mut Cx<Files>| el()))
}

fn deliver_files(app: &App<Files>, rt: &mut Runtime<Files>) -> usize {
    let updates = app.shared.take_updates();
    let n = updates.len();
    for f in updates { rt.update_if(f); }
    n
}

fn files_runtime() -> Runtime<Files> {
    Runtime::new(Files::new(), |_: &Files, _: &mut Cx<Files>| el(), (100.0, 100.0), Env::default())
}

#[test]
fn files_before_the_first_window_arrive_once_in_order_as_it_starts() {
    use std::path::PathBuf;
    let (inbox, glue, app) = (files_inbox(), Fake::default(), files_app());
    inbox.prepare(&glue);
    inbox.receive(vec![PathBuf::from("/a.torrent"), PathBuf::from("/b.torrent")]);
    inbox.receive(vec![PathBuf::from("/c.torrent")]);
    let _listening = inbox.listen_files(&app, &glue).unwrap();
    inbox.settle(&glue);
    let mut rt = files_runtime();
    assert!(rt.state.is_empty(), "nothing runs before the host starts");
    assert_eq!(deliver_files(&app, &mut rt), 2);
    assert_eq!(rt.state, [
        vec![PathBuf::from("/a.torrent"), PathBuf::from("/b.torrent")],
        vec![PathBuf::from("/c.torrent")],
    ]);
    assert!(rt.needs_commit(), "the transaction commits and redraws");
    assert_eq!(deliver_files(&app, &mut rt), 0, "delivered once");
    assert_eq!((glue.installs.get(), glue.removes.get()), (1, 0));
}

#[test]
fn files_without_a_hook_remove_the_launch_handler_and_drop_later_batches() {
    use std::path::PathBuf;
    let (inbox, glue) = (files_inbox(), Fake::default());
    inbox.prepare(&glue);
    inbox.receive(vec![PathBuf::from("/launch.torrent")]);
    assert!(inbox.listen_files(&plain_files(), &glue).is_none());
    inbox.settle(&glue);
    assert_eq!((glue.installs.get(), glue.removes.get()), (1, 1), "installed for launch, removed after it");
    let late = files_app();
    inbox.receive(vec![PathBuf::from("/dropped.torrent")]);
    let _listening = inbox.listen_files(&late, &glue).unwrap();
    assert!(late.shared.take_updates().is_empty(), "files from before settling are gone");
}

#[test]
fn a_later_window_with_on_open_files_gets_the_launch_batch() {
    use std::path::PathBuf;
    let (inbox, glue, first, second) = (files_inbox(), Fake::default(), plain_files(), files_app());
    inbox.prepare(&glue);
    inbox.receive(vec![PathBuf::from("/launch.torrent")]);
    assert!(inbox.listen_files(&first, &glue).is_none());
    let _listening = inbox.listen_files(&second, &glue).unwrap();
    inbox.settle(&glue);
    let mut rt = files_runtime();
    assert_eq!(deliver_files(&second, &mut rt), 1);
    assert_eq!(rt.state, [vec![PathBuf::from("/launch.torrent")]]);
    assert_eq!(glue.removes.get(), 0);
}

#[test]
fn files_while_running_wake_the_loop_and_hand_off_when_first_window_closes() {
    use std::path::PathBuf;
    let (inbox, glue, first, second) = (files_inbox(), Fake::default(), files_app(), files_app());
    let listening = inbox.listen_files(&first, &glue).unwrap();
    let _second = inbox.listen_files(&second, &glue).unwrap();
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    first.shared.connect(Arc::new(move || { w.fetch_add(1, Ordering::SeqCst); }));
    let before = wakes.load(Ordering::SeqCst);
    inbox.receive(vec![PathBuf::from("/one.torrent")]);
    assert_eq!(wakes.load(Ordering::SeqCst), before + 1);
    drop(listening);
    inbox.receive(vec![PathBuf::from("/two.torrent")]);
    let mut rt1 = files_runtime();
    let mut rt2 = files_runtime();
    assert_eq!(deliver_files(&first, &mut rt1), 1);
    assert_eq!(deliver_files(&second, &mut rt2), 1);
    assert_eq!(rt1.state, [vec![PathBuf::from("/one.torrent")]]);
    assert_eq!(rt2.state, [vec![PathBuf::from("/two.torrent")]]);
    assert_eq!(glue.installs.get(), 1, "installed once");
}

#[test]
fn an_apps_own_open_files_handler_stays_unless_a_window_has_the_hook() {
    use std::path::PathBuf;
    let (inbox, glue) = (files_inbox(), Fake { elsewhere: true, ..Fake::default() });
    inbox.prepare(&glue);
    inbox.receive(vec![PathBuf::from("/theirs.torrent")]);
    inbox.settle(&glue);
    assert_eq!((glue.installs.get(), glue.removes.get()), (0, 0));
    let app = files_app();
    let _listening = inbox.listen_files(&app, &glue).unwrap();
    assert_eq!(glue.installs.get(), 1);
    assert!(app.shared.take_updates().is_empty());
}

#[test]
fn a_files_handler_that_cant_install_listens_to_nothing() {
    use std::path::PathBuf;
    let (inbox, app) = (files_inbox(), files_app());
    assert!(inbox.listen_files(&app, &Fake { refuse: true, ..Fake::default() }).is_none());
    inbox.receive(vec![PathBuf::from("/ignored.torrent")]);
    assert!(app.shared.take_updates().is_empty());
    assert!(inbox.listen_files(&app, &Fake::default()).is_some());
}

#[test]
fn on_open_url_and_on_open_files_listen_independently() {
    use std::path::PathBuf;
    #[derive(Default)]
    struct Both {
        urls: Vec<String>,
        files: Vec<Vec<PathBuf>>,
    }
    let spec = AppSpec::new("both", (100.0, 100.0), Both::default, |_: &Both, _: &mut Cx<Both>| el());
    let both = App::new(spec)
        .on_open_url(|s, url, _| s.urls.push(url.to_owned()))
        .on_open_files(|s, paths, _| s.files.push(paths.to_vec()));
    let (url_inbox, file_inbox, url_glue, file_glue) = (inbox(), files_inbox(), Fake::default(), Fake::default());
    let _u = url_inbox.listen(&both, &url_glue).unwrap();
    let _f = file_inbox.listen_files(&both, &file_glue).unwrap();
    url_inbox.receive("magnet:?xt=urn:btih:1".into());
    file_inbox.receive(vec![PathBuf::from("/a.torrent")]);
    let mut rt = Runtime::new(Both::default(), |_: &Both, _: &mut Cx<Both>| el(), (100.0, 100.0), Env::default());
    for f in both.shared.take_updates() { rt.update_if(f); }
    assert_eq!(rt.state.urls, ["magnet:?xt=urn:btih:1"]);
    assert_eq!(rt.state.files, [vec![PathBuf::from("/a.torrent")]]);
}

#[cfg(target_os = "macos")]
#[test]
#[allow(unsafe_code)]
fn the_url_is_the_events_direct_object() {
    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;
    let code = |s: &[u8; 4]| u32::from_be_bytes(*s);
    // SAFETY: Foundation constructors; the event is built, never sent.
    autoreleasepool(|_| unsafe {
        let event: *mut AnyObject = msg_send![class!(NSAppleEventDescriptor), appleEventWithEventClass: code(b"GURL"),
            eventID: code(b"GURL"), targetDescriptor: std::ptr::null_mut::<AnyObject>(), returnID: -1i16, transactionID: 0i32];
        let event = event.as_ref().unwrap();
        assert_eq!(super::mac::url_of(event), None, "no direct object");
        let url = NSString::from_str("myapp://open?id=1");
        let param: *mut AnyObject = msg_send![class!(NSAppleEventDescriptor), descriptorWithString: &*url];
        let _: () = msg_send![event, setParamDescriptor: param, forKeyword: code(b"----")];
        assert_eq!(super::mac::url_of(event).as_deref(), Some("myapp://open?id=1"));
    });
}

#[cfg(target_os = "macos")]
#[test]
#[allow(unsafe_code)]
fn open_documents_extracts_file_paths_and_skips_non_file_urls() {
    use std::path::PathBuf;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSData, NSString, NSURL};
    let code = |s: &[u8; 4]| u32::from_be_bytes(*s);
    // SAFETY: Foundation constructors only; synthetic events are built in memory, never sent.
    autoreleasepool(|_| unsafe {
        let make_event = || -> *mut AnyObject {
            msg_send![class!(NSAppleEventDescriptor), appleEventWithEventClass: code(b"aevt"),
                eventID: code(b"odoc"), targetDescriptor: std::ptr::null_mut::<AnyObject>(), returnID: -1i16, transactionID: 0i32]
        };
        let make_furl = |raw: &str| -> *mut AnyObject {
            let data = NSData::with_bytes(raw.as_bytes());
            msg_send![class!(NSAppleEventDescriptor), descriptorWithDescriptorType: code(b"furl"), data: &*data]
        };

        // Missing direct object yields nothing.
        let empty_event = make_event();
        assert!(super::mac::files_of(empty_event.as_ref().unwrap()).is_empty());

        // Empty list yields nothing.
        let empty_list: *mut AnyObject = msg_send![class!(NSAppleEventDescriptor), listDescriptor];
        let _: () = msg_send![empty_event, setParamDescriptor: empty_list, forKeyword: code(b"----")];
        assert!(super::mac::files_of(empty_event.as_ref().unwrap()).is_empty());

        // Multi-item list decodes percent-encoded spaces and UTF-8, skipping non-file items.
        let list: *mut AnyObject = msg_send![class!(NSAppleEventDescriptor), listDescriptor];
        let first = make_furl("file:///tmp/first.torrent");
        let https = make_furl("https://example.com/not-a-file");
        let plain_str = NSString::from_str("not-a-file-url");
        let plain: *mut AnyObject = msg_send![class!(NSAppleEventDescriptor), descriptorWithString: &*plain_str];
        let second = make_furl("file:///tmp/my%20folder/caf%C3%A9.txt");
        let _: () = msg_send![list, insertDescriptor: first, atIndex: 1isize];
        let _: () = msg_send![list, insertDescriptor: https, atIndex: 2isize];
        let _: () = msg_send![list, insertDescriptor: plain, atIndex: 3isize];
        let _: () = msg_send![list, insertDescriptor: second, atIndex: 4isize];
        let event = make_event();
        let _: () = msg_send![event, setParamDescriptor: list, forKeyword: code(b"----")];
        assert_eq!(super::mac::files_of(event.as_ref().unwrap()), [
            PathBuf::from("/tmp/first.torrent"),
            PathBuf::from("/tmp/my folder/café.txt"),
        ]);

        // A single non-list file descriptor as keyDirectObject also extracts cleanly.
        let single_event = make_event();
        let solo = make_furl("file:///tmp/solo.torrent");
        let _: () = msg_send![single_event, setParamDescriptor: solo, forKeyword: code(b"----")];
        assert_eq!(super::mac::files_of(single_event.as_ref().unwrap()), [PathBuf::from("/tmp/solo.torrent")]);

        // Coercion from typeBookmarkData ('bmrk'), which Finder/NSWorkspace send in real 'odoc' events.
        let root_url = NSURL::fileURLWithPath(&NSString::from_str("/"));
        let bmrk_data: *mut NSData = msg_send![&*root_url,
            bookmarkDataWithOptions: 0usize,
            includingResourceValuesForKeys: std::ptr::null_mut::<AnyObject>(),
            relativeToURL: std::ptr::null_mut::<AnyObject>(),
            error: std::ptr::null_mut::<*mut AnyObject>()];
        let bmrk_data = bmrk_data.as_ref().expect("bookmark data for /");
        let bmrk: *mut AnyObject = msg_send![class!(NSAppleEventDescriptor), descriptorWithDescriptorType: code(b"bmrk"), data: bmrk_data];
        let bmrk_list: *mut AnyObject = msg_send![class!(NSAppleEventDescriptor), listDescriptor];
        let _: () = msg_send![bmrk_list, insertDescriptor: bmrk, atIndex: 1isize];
        let bmrk_event = make_event();
        let _: () = msg_send![bmrk_event, setParamDescriptor: bmrk_list, forKeyword: code(b"----")];
        assert_eq!(super::mac::files_of(bmrk_event.as_ref().unwrap()), [PathBuf::from("/")]);
    });
}
