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

/// A plain paste leaves an empty clipboard alone; a read always answers.
#[test]
fn the_worker_answers_reads_with_sanitised_content() {
    let state = Arc::new(Mutex::new(State::default()));
    let shared = state.clone();
    let worker = Worker::spawn(move || Some(Fake { state: shared.clone(), dropped: None }));
    state.lock().unwrap().text = "<script>".into();
    let (reply, replies) = mpsc::channel();
    worker.sender.send(Request::Read { purpose: Purpose::Read(7), reply, wake: Arc::new(|| {}) }).unwrap();
    let answer = replies.recv().unwrap();
    assert_eq!((answer.purpose, answer.content.text()), (Purpose::Read(7), "<script>"), "plain text is never rewritten");
    worker.stop();
    assert!(Purpose::Paste(None, Forms::Rich).reply(None).is_none());
    let mut board = Board { content: Some(ClipboardContent::new("t").with_html("<b>t</b>")), ..Board::default() };
    let plain = Purpose::Paste(None, Forms::Text).answer(&mut board).unwrap().content;
    assert!(!plain.is_rich() && plain.text() == "t", "a plain target reads only the text, as before");
    let read = Purpose::Read(1).reply(Some(ClipboardContent::new("").with_html("<p onclick=x>t</p>"))).unwrap();
    assert_eq!(read.content.html(), Some("<p>t</p>"));
    assert_eq!(Purpose::Read(2).reply(None).unwrap().content, ClipboardContent::default());
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
