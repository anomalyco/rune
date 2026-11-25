use super::*;
use super::native::Worker;

#[derive(Default)]
struct State { text: String, revision: u64, clears: usize, drops: usize }
struct Fake { state: Arc<Mutex<State>>, dropped: Option<mpsc::Sender<()>> }
impl Backend for Fake {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.text = copy.to_string(); s.revision += 1;
        Ok(())
    }
    fn text(&mut self) -> Option<Zeroizing<String>> { Some(Zeroizing::new(self.state.lock().unwrap().text.clone())) }
    fn revision(&self) -> Option<u64> { Some(self.state.lock().unwrap().revision) }
    fn clear(&mut self) -> Result<(), String> { let mut s = self.state.lock().unwrap(); s.text.clear(); s.clears += 1; Ok(()) }
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.state.lock().unwrap().drops += 1;
        if let Some(dropped) = &self.dropped { dropped.send(()).ok(); }
    }
}

const LONG: Duration = Duration::from_secs(3600);

fn copy(m: &MemoryClipboard, text: &str, clear_after: Duration) -> SecretCopy {
    let (write, handle) = ClipboardWrite::secret(text.into(), clear_after);
    write.bind(Route::Memory(m.clone()));
    m.handle(Request::Write(write, None));
    handle
}
fn text(m: &MemoryClipboard) -> Option<String> { m.text().map(|t| t.to_string()) }

#[test]
fn shutdown_expires_only_our_pending_secret_before_dropping_backend() {
    for replacement in [None, Some("new value"), Some("fixture")] {
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let worker = Worker::spawn(move || Some(Fake { state: shared.clone(), dropped: None }));
        worker.sender.send(Request::Write(ClipboardWrite::secret("fixture".into(), LONG).0, None)).unwrap();
        let (reply, received) = mpsc::channel();
        worker.sender.send(Request::Read { purpose: Purpose::Paste(None, Forms::Text), reply, wake: Arc::new(|| {}) }).unwrap();
        received.recv().unwrap(); // Write and revision recorded.
        if let Some(text) = replacement {
            let mut s = state.lock().unwrap(); s.text = text.into(); s.revision += 1;
        }
        worker.stop(); worker.stop();
        let s = state.lock().unwrap();
        assert_eq!(s.text, replacement.unwrap_or(""));
        assert_eq!(s.clears, usize::from(replacement.is_none()));
        assert_eq!(s.drops, 1, "normal shutdown joins after clipboard handoff/drop");
    }
}

#[test]
fn shutdown_does_not_wait_indefinitely_for_a_blocked_backend() {
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (dropped, finished) = mpsc::channel();
    let worker = Worker::spawn(move || {
        entered.send(()).unwrap();
        released.recv().unwrap();
        Some(Fake { state: Arc::default(), dropped: Some(dropped.clone()) })
    });
    worker.sender.send(Request::Write(ClipboardWrite::secret("fixture".into(), LONG).0, None)).unwrap();
    waiting.recv().unwrap();
    worker.stop();
    assert!(finished.try_recv().is_err(), "stop must return while the backend is still blocked");
    release.send(()).unwrap();
    finished.recv().unwrap();
}

/// The OS worker reports the write, then notices another app's change on its own.
#[test]
fn worker_reports_the_write_and_watches_for_outside_changes() {
    let state = Arc::new(Mutex::new(State::default()));
    let shared = state.clone();
    let worker = Worker::spawn(move || Some(Fake { state: shared.clone(), dropped: None }));
    let (woke, wakes) = mpsc::channel();
    let notify = Notify { changed: Arc::default(), wake: Arc::new(move || { woke.send(()).ok(); }) };
    let changed = notify.changed.clone();
    let (write, handle) = ClipboardWrite::secret("fixture".into(), LONG);
    write.bind(Route::System(worker.sender.clone()));
    worker.sender.send(Request::Write(write, Some(notify))).unwrap();
    wakes.recv().unwrap();
    assert!(handle.holds() && changed.load(Ordering::SeqCst));
    { let mut s = state.lock().unwrap(); s.text = "theirs".into(); s.revision += 1; }
    wakes.recv().unwrap();
    assert_eq!(handle.state(), SecretState::Replaced);
    handle.clear_now();
    worker.stop();
    assert_eq!(state.lock().unwrap().text, "theirs");
}

#[test]
fn holds_until_a_later_copy_or_an_outside_change() {
    let m = memory();
    let first = copy(&m, "fixture", LONG);
    assert!(first.holds());
    let second = copy(&m, "fixture", LONG);
    assert_eq!(first.state(), SecretState::Replaced, "identical text from a later copy still supersedes");
    assert!(second.holds());
    m.set_text("theirs");
    assert_eq!(second.state(), SecretState::Replaced);
    let expired = copy(&m, "brief", Duration::ZERO);
    assert_eq!(expired.state(), SecretState::Cleared);
    assert_eq!(text(&m), None);
    assert!(!format!("{expired:?} {m:?}").contains("brief"));
}

#[test]
fn clear_now_clears_only_while_it_holds() {
    let m = memory();
    let replaced = copy(&m, "secret", LONG);
    m.set_text("theirs");
    replaced.clear_now();
    assert_eq!(text(&m).as_deref(), Some("theirs"));
    let held = copy(&m, "secret", LONG);
    held.clear_now();
    held.clear_now();
    assert_eq!((held.state(), text(&m)), (SecretState::Cleared, None));
    let (write, unwritten) = ClipboardWrite::secret("never".into(), LONG);
    write.bind(Route::Memory(m.clone()));
    unwritten.clear_now();
    m.handle(Request::Write(write, None));
    assert_eq!((unwritten.state(), text(&m)), (SecretState::Cleared, None));
    let (write, dropped) = ClipboardWrite::secret("dropped".into(), LONG);
    drop(write);
    assert_eq!(dropped.state(), SecretState::Replaced);
}

#[test]
fn a_failed_write_reports_and_keeps_the_earlier_copy() {
    let m = memory();
    let earlier = copy(&m, "earlier", LONG);
    m.fail_writes(Some("pasteboard denied"));
    let failed = copy(&m, "later", LONG);
    assert_eq!(failed.state(), SecretState::Failed("pasteboard denied".into()));
    assert!(earlier.holds());
    assert_eq!(text(&m).as_deref(), Some("earlier"));
}

#[test]
fn two_runtimes_share_a_memory_clipboard() {
    use crate::{col, Cx, El, Env};
    fn view(_: &Option<SecretCopy>, _: &mut Cx<Option<SecretCopy>>) -> El { col() }
    let m = memory();
    let [mut a, mut b] = [0, 1].map(|_| {
        let mut rt = Runtime::new(None, view, (100.0, 100.0), Env::new(true, 1.0));
        rt.use_clipboard(&m);
        rt.frame(0.0);
        rt
    });
    a.update(|s, tx| *s = Some(tx.copy_secret("from a", LONG))); a.frame(0.0);
    assert!(a.state.as_ref().unwrap().holds());
    b.update(|s, tx| *s = Some(tx.copy_secret("from b", LONG))); b.frame(0.0);
    assert_eq!(text(&m).as_deref(), Some("from b"));
    assert_eq!(a.state.as_ref().unwrap().state(), SecretState::Replaced);
    a.update(|_, tx| tx.clear_secret_clipboard()); a.frame(0.0);
    assert_eq!(text(&m), None, "either window clears the latest secret");
    assert_eq!(b.state.as_ref().unwrap().state(), SecretState::Cleared);

    a.update(|s, tx| *s = Some(tx.copy_secret("no frame", LONG)));
    assert_eq!(text(&m).as_deref(), Some("no frame"), "update writes without waiting for a frame");

    let mut headless = Runtime::new(None, view, (100.0, 100.0), Env::new(true, 1.0));
    headless.update(|s, tx| *s = Some(tx.copy_secret("nowhere", LONG))); headless.frame(0.0);
    assert_eq!(headless.state.as_ref().unwrap().state(), SecretState::Pending, "no clipboard, no write");
}

#[test]
fn rich_copies_carry_every_form_and_secrets_stay_plain() {
    use crate::{col, Cx, El, Env};
    fn view(_: &u8, _: &mut Cx<u8>) -> El { col() }
    let m = memory();
    let mut rt = Runtime::new(0, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    let rich = ClipboardContent::new("Ship it").with_html("<b>Ship</b> it").with_rtf(r"{\rtf1 {\b Ship} it}");
    rt.update(|_, tx| tx.copy_rich(rich.clone()));
    assert_eq!(m.content().as_ref(), Some(&rich), "HTML is written as given; only reads are sanitised");
    assert_eq!(text(&m).as_deref(), Some("Ship it"));
    assert!(!format!("{rich:?} {:?}", ClipboardWrite::new(rich.clone())).contains("Ship"));

    rt.update(|_, tx| { tx.copy_secret("hunter2", LONG); });
    let secret = m.content().unwrap();
    assert!(!secret.is_rich() && secret.text() == "hunter2", "no HTML or RTF copy of a secret");
    rt.update(|_, tx| tx.clear_secret_clipboard());
    assert_eq!(m.content(), None);
}

#[test]
fn copy_image_writes_a_decoded_image_as_png_and_a_failed_one_leaves_the_clipboard_alone() {
    use crate::{col, Cx, El, Env};
    fn view(_: &u8, _: &mut Cx<u8>) -> El { col() }
    let m = memory();
    let mut rt = Runtime::new(0, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    let cache = crate::ImageCache::new(Arc::new(|| {}));
    let rgba: Vec<u8> = (0..12u8).flat_map(|i| [i * 20, 255 - i * 20, 7, 255]).collect();
    let image = cache.rgba(4, 3, rgba.clone());
    image.wait().unwrap();
    rt.update(|_, tx| tx.copy_image(&image));
    let copied = m.content().unwrap().image().cloned().unwrap();
    assert_eq!(copied.size(), (4, 3));
    assert_eq!(crate::image::png_rgba8(copied.png()).unwrap().into_raw(), rgba, "the PNG holds the decoded pixels");

    let bad = cache.load(b"not an image".as_slice());
    assert!(bad.wait().is_err());
    let revision = m.revision();
    rt.update(|_, tx| tx.copy_image(&bad));
    assert_eq!((m.revision(), m.content().unwrap().image()), (revision, Some(&copied)), "nothing to copy, nothing written");
}

/// On Windows and Linux, a corrupt PNG in `ClipboardContent` must return an error without
/// falling through to `set_text("")` and wiping the clipboard, and `Tx::copy_image` must reuse
/// its pre-encoded RGBA buffer without decoding the freshly encoded PNG back to pixels.
#[cfg(any(target_os = "windows", all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android")))))]
#[test]
fn write_one_form_never_clobbers_clipboard_with_empty_text_on_bad_png_and_borrows_preencoded_rgba() {
    // Valid 24-byte PNG signature + IHDR header (2x1), but no IDAT: `from_png` accepts header, decode fails.
    let corrupt_png = [&b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..], &[0, 0, 0, 2, 0, 0, 0, 1]].concat();
    let bad_img = ClipboardImage::from_png(corrupt_png).expect("header-valid PNG");
    let copy = ClipboardWrite::new(ClipboardContent::default().with_image(bad_img));
    let res = native::prepare_one_form(&copy);
    assert!(res.is_err(), "corrupt PNG must return Err instead of falling through to Text");

    // Pre-encoded RGBA from `ClipboardWrite::image` is borrowed directly (no PNG decode pass).
    let cache = crate::ImageCache::new(Arc::new(|| {}));
    let rgba = vec![10, 20, 30, 255, 40, 50, 60, 128];
    let img = cache.rgba(2, 1, rgba.clone());
    img.wait().unwrap();
    let mut write = ClipboardWrite::image(img);
    assert!(write.encode_image().is_ok());
    match native::prepare_one_form(&write).unwrap() {
        native::OneForm::Image { png, width, height, rgba: bytes } => {
            assert_eq!((width, height), (2, 1));
            assert!(png.starts_with(b"\x89PNG"));
            assert!(matches!(bytes, std::borrow::Cow::Borrowed(_)), "copy_image must reuse pre-encoded RGBA instead of decoding PNG back");
            assert_eq!(&bytes[..4], &rgba[..4]);
        }
        _ => panic!("expected OneForm::Image"),
    }
}

#[cfg(target_os = "windows")]
#[test]
fn windows_pack_dibv5_writes_bottom_up_bgra_with_v5_header() {
    let rgba = vec![
        255, 0, 0, 255, 0, 255, 0, 128,
        0, 0, 255, 64, 255, 255, 255, 255,
    ];
    let dib = native::pack_dibv5(2, 2, &rgba).expect("valid 2x2 DIBV5");
    assert_eq!(dib.len(), 124 + 16);
    assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 124);
    assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 2);
    assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(dib[16..20].try_into().unwrap()), 3);
    assert_eq!(&dib[124..132], &[255, 0, 0, 64, 255, 255, 255, 255]);
    assert_eq!(&dib[132..140], &[0, 0, 255, 255, 0, 255, 0, 128]);
    assert!(native::pack_dibv5(0, 2, &[]).is_none());
    assert!(native::pack_dibv5(2, 2, &[0; 4]).is_none());
}

/// A slow image encode must never overwrite newer clipboard content written either by another app
/// (OS revision advanced during encode) or by a newer in-app write (generation superseded), while
/// consecutive in-app writes that snapshotted the same initial revision still both land in order.
#[test]
fn late_image_encode_never_overwrites_newer_external_or_in_app_clipboard_write() {
    #[derive(Clone, Default)]
    struct SharedBoard(Arc<Mutex<Board>>);
    impl Backend for SharedBoard {
        fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> { lock(&self.0).write(copy) }
        fn text(&mut self) -> Option<Zeroizing<String>> { lock(&self.0).text() }
        fn content(&mut self, all: bool) -> Option<ClipboardContent> { lock(&self.0).content(all) }
        fn revision(&self) -> Option<u64> { lock(&self.0).revision() }
        fn clear(&mut self) -> Result<(), String> { lock(&self.0).clear() }
    }
    struct Hook<F: Fn()>(F);
    impl<F: Fn()> WriteSteps for Hook<F> {
        fn now(&self) -> Instant { crate::time::now() }
        fn encoding(&self) { (self.0)(); }
    }

    let cache = crate::ImageCache::new(Arc::new(|| {}));
    let image = cache.rgba(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    image.wait().unwrap();

    // Case 1: External app writes while image is encoding -> OS revision advances -> image write aborted.
    let mut board = SharedBoard::default();
    let mut core = Core::default();
    let latest = Arc::new(AtomicU64::new(1));
    let mut img_write = ClipboardWrite::image(image.clone());
    img_write.stamp(1, latest.clone(), board.revision());
    let board_clone = board.clone();
    let steps = Hook(move || {
        let mut b = lock(&board_clone.0);
        b.content = Some(ClipboardContent::new("copied from browser"));
        b.revision += 1;
    });
    core.write(&mut board, img_write, None, &steps);
    assert_eq!(board.text().as_deref().map(String::as_str), Some("copied from browser"), "external write preserved");
    assert!(board.content(true).unwrap().image().is_none(), "late image must not overwrite external write");

    // Case 2: Newer in-app write is queued while image is encoding -> generation superseded -> image aborted.
    let mut board2 = SharedBoard::default();
    let mut core2 = Core::default();
    let latest2 = Arc::new(AtomicU64::new(1));
    let mut img_write2 = ClipboardWrite::image(image.clone());
    img_write2.stamp(1, latest2.clone(), board2.revision());
    let latest2_bump = latest2.clone();
    let steps2 = Hook(move || { latest2_bump.store(2, Ordering::Release); });
    core2.write(&mut board2, img_write2, None, &steps2);
    assert!(board2.content(true).is_none(), "superseded image must not write");
    let mut newer_text = ClipboardWrite::new(ClipboardContent::new("newer in-app text"));
    newer_text.stamp(2, latest2, Some(0));
    core2.write(&mut board2, newer_text, None, &RealTime);
    assert_eq!(board2.text().as_deref().map(String::as_str), Some("newer in-app text"));

    // Case 3: Consecutive in-app writes that snapshotted the same initial OS revision still land.
    let mut board3 = SharedBoard::default();
    let mut core3 = Core::default();
    let latest3 = Arc::new(AtomicU64::new(1));
    let mut first = ClipboardWrite::new(ClipboardContent::new("first"));
    first.stamp(1, latest3.clone(), Some(0));
    core3.write(&mut board3, first, None, &RealTime);
    assert_eq!(board3.revision(), Some(1));
    // Second write was stamped before `first` finished (so revision_before == Some(0)), but its
    // generation is current and rev 1 is `last_own_revision` (our own write, not another app's).
    latest3.store(2, Ordering::Release);
    let mut second = ClipboardWrite::new(ClipboardContent::new("second"));
    second.stamp(2, latest3, Some(0));
    core3.write(&mut board3, second, None, &RealTime);
    assert_eq!(board3.text().as_deref().map(String::as_str), Some("second"));
}

/// A plain paste leaves an empty clipboard alone; a read always answers.
#[test]
fn the_worker_answers_reads_with_sanitised_content() {
    let state = Arc::new(Mutex::new(State::default()));
    let shared = state.clone();
    let worker = Worker::spawn(move || Some(Fake { state: shared.clone(), dropped: None }));
    state.lock().unwrap().text = "<script>".into();
    let (reply, replies) = mpsc::channel();
    worker.sender.send(Request::Read { purpose: Purpose::Read(7), reply, wake: Arc::new(|| {}) }).unwrap();
    let Reply::Content { purpose, content } = replies.recv().unwrap() else { panic!("expected Reply::Content") };
    assert_eq!((purpose, content.text()), (Purpose::Read(7), "<script>"), "plain text is never rewritten");
    worker.stop();
    assert!(Purpose::Paste(None, Forms::Rich).reply(None).is_none());
    let mut board = Board { content: Some(ClipboardContent::new("t").with_html("<b>t</b>")), ..Board::default() };
    let Reply::Content { content: plain, .. } = Purpose::Paste(None, Forms::Text).answer(&mut board).unwrap() else { panic!("expected Reply::Content") };
    assert!(!plain.is_rich() && plain.text() == "t", "a plain target reads only the text, as before");
    let Reply::Content { content: read, .. } = Purpose::Read(1).reply(Some(ClipboardContent::new("").with_html("<p onclick=x>t</p>"))).unwrap() else { panic!("expected Reply::Content") };
    assert_eq!(read.html(), Some("<p>t</p>"));
    let Reply::Content { content: empty, .. } = Purpose::Read(2).reply(None).unwrap() else { panic!("expected Reply::Content") };
    assert_eq!(empty, ClipboardContent::default());
}

/// `on_close` runs through `update`, and a hidden window only polls: both must reach the
/// clipboard before any frame.
#[test]
fn clears_from_on_close_and_a_hidden_poll_need_no_frame() {
    use crate::{col, Cx, El, Env};
    fn view(_: &u8, _: &mut Cx<u8>) -> El { col() }
    let m = memory();
    let mut rt = Runtime::new(0, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    let on_close: fn(&mut u8, &mut crate::Tx) = |_, tx| tx.clear_secret_clipboard();
    let held = copy(&m, "secret", LONG);
    rt.update(on_close);
    assert_eq!((held.state(), text(&m)), (SecretState::Cleared, None));

    rt.poll = Some(|_, _, tx| { tx.clear_secret_clipboard(); false });
    let held = copy(&m, "secret", LONG);
    rt.poll_hidden();
    assert_eq!((held.state(), text(&m)), (SecretState::Cleared, None));
}

#[test]
fn a_clear_after_an_unwritten_copy_drops_the_stale_secret() {
    use crate::{col, Cx, El, Env};
    fn view(_: &Option<SecretCopy>, _: &mut Cx<Option<SecretCopy>>) -> El { col() }
    // No clipboard: the copy waits for the shell, as it does between window flushes.
    let mut rt = Runtime::new(None, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.update(|s, tx| *s = Some(tx.copy_secret("stale", LONG)));
    rt.update(|_, tx| tx.clear_secret_clipboard());
    assert!(rt.copy_request.is_none() && rt.edit.clear_secret_request);
    assert_eq!(rt.state.as_ref().unwrap().state(), SecretState::Replaced);

    // The same in one handler, and with a memory clipboard on the next flush.
    let m = memory();
    rt.use_clipboard(&m);
    rt.update(|s, tx| { *s = Some(tx.copy_secret("stale", LONG)); tx.clear_secret_clipboard(); });
    rt.frame(0.0);
    assert_eq!(text(&m), None);
    assert_eq!(rt.state.as_ref().unwrap().state(), SecretState::Replaced);
}

#[test]
fn a_staged_pending_copy_stays_pending_and_clear_now_is_a_no_op() {
    let copy = SecretCopy::pending();
    let clone = copy.clone();
    drop(copy);
    assert_eq!(clone.state(), SecretState::Pending);
    clone.clear_now();
    assert_eq!(clone.state(), SecretState::Pending);
    assert!(!clone.holds());
}

#[test]
fn memory_clipboard_revision_tracks_writes_clears_and_failures() {
    use crate::{col, Cx, El, Env};
    fn view(_: &u8, _: &mut Cx<u8>) -> El { col() }

    let m = memory();
    let shared = m.clone();
    assert_eq!(m.revision(), 0);
    assert_eq!(shared.revision(), 0);

    // Reads never advance the revision.
    assert_eq!(m.text(), None);
    assert_eq!(m.content(), None);
    assert_eq!(m.revision(), 0);

    // External writes advance even when the text is unchanged.
    m.set_text("hello");
    assert_eq!((m.revision(), shared.revision()), (1, 1));
    m.set_text("hello");
    assert_eq!(m.revision(), 2);
    m.set_content(ClipboardContent::new("rich").with_html("<b>rich</b>"));
    assert_eq!(m.revision(), 3);
    assert_eq!(m.text().as_deref().map(String::as_str), Some("rich"));
    assert_eq!(m.revision(), 3);

    // Writes and secret clears routed through a Runtime advance the revision.
    let mut rt = Runtime::new(0, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    rt.update(|_, tx| tx.copy("plain"));
    assert_eq!(m.revision(), 4);
    rt.update(|_, tx| tx.copy_rich(ClipboardContent::new("fmt").with_html("<i>fmt</i>")));
    assert_eq!(m.revision(), 5);
    rt.update(|_, tx| { tx.copy_secret("token", LONG); });
    assert_eq!(m.revision(), 6);
    rt.update(|_, tx| tx.clear_secret_clipboard());
    assert_eq!(m.revision(), 7);

    // Clear via SecretCopy advances once; idempotent repeat does not.
    let held = copy(&m, "token2", LONG);
    assert_eq!(m.revision(), 8);
    held.clear_now();
    assert_eq!(m.revision(), 9);
    held.clear_now();
    assert_eq!(m.revision(), 9);

    // Failed writes leave the revision untouched.
    m.fail_writes(Some("pasteboard denied"));
    let failed = copy(&m, "denied", LONG);
    assert_eq!(failed.state(), SecretState::Failed("pasteboard denied".into()));
    assert_eq!(m.revision(), 9);
    m.fail_writes(None);

    // An expired secret clears and advances the revision when revision() is queried.
    let _expired = copy(&m, "brief", Duration::ZERO);
    assert_eq!(m.revision(), 11);
    assert_eq!(m.revision(), 11);
    assert_eq!(m.text(), None);
}

#[test]
fn system_clipboard_revision_matches_platform_support() {
    let first = revision();
    let second = revision();
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        assert!(first.is_some() && second.is_some());
        assert!(!native::worker_started(), "reading the change counter must not start the clipboard worker");
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        assert_eq!((first, second), (None, None));
    }
}

#[test]
fn web_event_files_become_bounded_handles() {
    let many: Vec<_> = (1..=40).map(|id| serde_json::json!({ "id": id, "name": "a.pdf", "type": "application/pdf", "size": 9 })).collect();
    let files = web_file::from_event(&serde_json::Value::Array(many));
    assert_eq!(files.len(), MAX_WEB_FILES);
    assert_eq!((files[0].name(), files[0].mime(), files[0].size()), ("a.pdf", "application/pdf", 9));
    assert!(web_file::from_event(&serde_json::json!([{ "name": "no id" }])).is_empty());
    assert!(web_file::from_event(&serde_json::Value::Null).is_empty());
}

#[test]
fn web_files_are_rich_and_text_forms_drop_them() {
    let files = web_file::from_event(&serde_json::json!([{ "id": 1, "name": "a.zip", "type": "", "size": 3 }]));
    let content = ClipboardContent::new("").with_web_files(files);
    assert!(content.is_rich() && !content.has_plain());
    assert!(!format!("{content:?}").contains("a.zip"), "Debug never shows names");
    assert_eq!(content.web_files().len(), 1);
    assert!(content.text_forms().is_none(), "a rich field without a hook hears nothing of a files-only paste");
}

/// `Tx::copy_image_then` reports `Ok(())`, `DecodeFailed`, `Superseded`, and `Failed`, and
/// `Worker::spawn` runs PNG encoding off the calling thread while reporting `Superseded` on
/// late overwrites.
#[test]
fn copy_image_then_reports_outcomes_and_worker_encodes_off_calling_thread() {
    use crate::{col, Cx, El, Env};
    fn view(_: &Vec<Result<(), CopyImageError>>, _: &mut Cx<Vec<Result<(), CopyImageError>>>) -> El { col() }
    let m = memory();
    let mut rt = Runtime::new(Vec::<Result<(), CopyImageError>>::new(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);

    let cache = crate::ImageCache::new(Arc::new(|| {}));
    let good = cache.rgba(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    good.wait().unwrap();
    rt.update(|_, tx| tx.copy_image_then(&good, |s: &mut Vec<Result<(), CopyImageError>>, r, _| s.push(r)));
    rt.frame(0.0);
    assert_eq!(rt.state.pop(), Some(Ok(())));

    let bad = cache.load(b"not an image".as_slice());
    assert!(bad.wait().is_err());
    rt.update(|_, tx| tx.copy_image_then(&bad, |s: &mut Vec<Result<(), CopyImageError>>, r, _| s.push(r)));
    rt.frame(0.0);
    assert!(matches!(rt.state.pop(), Some(Err(CopyImageError::DecodeFailed(_)))));

    // Same-handler supersession: copy_image_then followed by tx.copy reports Superseded.
    rt.update(|_, tx| {
        tx.copy_image_then(&good, |s: &mut Vec<Result<(), CopyImageError>>, r, _| s.push(r));
        tx.copy("text wins");
    });
    rt.frame(0.0);
    assert_eq!(rt.state.pop(), Some(Err(CopyImageError::Superseded)));
    assert_eq!(text(&m).as_deref(), Some("text wins"));

    // Backend failure reports Failed.
    m.fail_writes(Some("clipboard busy"));
    rt.update(|_, tx| tx.copy_image_then(&good, |s: &mut Vec<Result<(), CopyImageError>>, r, _| s.push(r)));
    rt.frame(0.0);
    assert_eq!(rt.state.pop(), Some(Err(CopyImageError::Failed("clipboard busy".into()))));
    m.fail_writes(None);

    // Worker thread identity: encoding + write run on the worker thread, not the caller's.
    struct ThreadRecorder { caller: std::thread::ThreadId, ran_on: Arc<Mutex<Option<std::thread::ThreadId>>>, rev: u64 }
    impl Backend for ThreadRecorder {
        fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> {
            assert!(copy.content().image().is_some(), "PNG was encoded before Backend::write");
            *self.ran_on.lock().unwrap() = Some(std::thread::current().id());
            assert_ne!(std::thread::current().id(), self.caller, "encode and write must run off the caller thread");
            self.rev += 1;
            Ok(())
        }
        fn text(&mut self) -> Option<Zeroizing<String>> { None }
        fn revision(&self) -> Option<u64> { Some(self.rev) }
        fn clear(&mut self) -> Result<(), String> { Ok(()) }
    }
    let caller = std::thread::current().id();
    let ran_on = Arc::new(Mutex::new(None));
    let ran_on_w = ran_on.clone();
    let worker = Worker::spawn(move || Some(ThreadRecorder { caller, ran_on: ran_on_w.clone(), rev: 0 }));
    let (tx_reply, rx_reply) = mpsc::channel();
    let mut w = ClipboardWrite::image(good.clone());
    w.attach_copy_reply(CopyReply { id: 9, sender: tx_reply, wake: Arc::new(|| {}) });
    worker.send_write(w, None, Some(0));
    match rx_reply.recv_timeout(Duration::from_secs(5)).unwrap() {
        Reply::CopyImage { id: 9, result: Ok(()) } => {}
        _ => panic!("expected Reply::CopyImage Ok"),
    }
    assert_ne!(ran_on.lock().unwrap().unwrap(), caller);
    worker.stop();

    // No-clipboard worker (e.g. iOS/Android or headless) reports CopyImageError::Failed.
    let no_cb = Worker::spawn(|| None::<Board>);
    let (tx_reply2, rx_reply2) = mpsc::channel();
    let mut w2 = ClipboardWrite::image(good);
    w2.attach_copy_reply(CopyReply { id: 10, sender: tx_reply2, wake: Arc::new(|| {}) });
    no_cb.send_write(w2, None, None);
    match rx_reply2.recv_timeout(Duration::from_secs(5)).unwrap() {
        Reply::CopyImage { id: 10, result: Err(CopyImageError::Failed(_)) } => {}
        _ => panic!("expected Failed when no clipboard"),
    }
    no_cb.stop();
}
