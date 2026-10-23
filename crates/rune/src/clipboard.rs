//! One OS clipboard worker shared by windows and overlays. No OS reads on the event thread.
use std::sync::{OnceLock, Mutex, mpsc};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;
use crate::{Runtime, Waker};

/// A pending clipboard write. Debug output never includes its contents.
#[derive(Clone)]
pub struct ClipboardWrite {
    text: Zeroizing<String>,
    clear_after: Option<Duration>,
}
impl ClipboardWrite {
    pub(crate) fn new(text: String, clear_after: Option<Duration>) -> Self {
        Self { text: Zeroizing::new(text), clear_after }
    }
}
impl std::ops::Deref for ClipboardWrite {
    type Target = str;
    fn deref(&self) -> &str { &self.text }
}
impl std::fmt::Debug for ClipboardWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardWrite").field("clear_after", &self.clear_after).finish_non_exhaustive()
    }
}

struct Reply { target: Option<String>, text: Zeroizing<String> }
enum Request {
    Shutdown,
    Write(ClipboardWrite),
    Read { target: Option<String>, reply: mpsc::Sender<Reply>, wake: Waker },
}

pub(crate) struct Client {
    sender: mpsc::Sender<Reply>,
    replies: mpsc::Receiver<Reply>,
    wake: Waker,
}
impl Default for Client {
    fn default() -> Self {
        let (sender, replies) = mpsc::channel();
        Self { sender, replies, wake: std::sync::Arc::new(|| {}) }
    }
}
impl Client {
    pub fn connect(&mut self, wake: Waker) { self.wake = wake; }
    pub fn read<S: 'static>(&self, rt: &Runtime<S>) {
        let _ = worker().send(Request::Read { target: rt.paste_target(), reply: self.sender.clone(), wake: self.wake.clone() });
    }
    pub fn receive<S: 'static>(&self, rt: &mut Runtime<S>) -> bool {
        let mut received = false;
        for reply in self.replies.try_iter() { rt.paste_result(reply.target, reply.text); received = true; }
        received
    }
    pub fn flush<S: 'static>(&self, rt: &mut Runtime<S>) {
        if let Some(copy) = rt.copy_request.take() { let _ = worker().send(Request::Write(copy)); }
        if std::mem::take(&mut rt.paste_request) { self.read(rt); }
    }
}

static WORKER: OnceLock<Worker> = OnceLock::new();
struct Worker {
    sender: mpsc::Sender<Request>,
    join: Mutex<Option<(std::thread::JoinHandle<()>, mpsc::Receiver<()>)>>,
}
impl Worker {
    fn spawn<B: Backend + 'static>(open: impl FnMut() -> Option<B> + Send + 'static) -> Self {
        let (sender, recv) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let handle = std::thread::Builder::new().name("rune-clipboard".into()).spawn(move || {
            run_worker(recv, open);
            // Backend dropped (including Linux handoff) before acknowledging completion.
            let _ = done.send(());
        }).expect("clipboard worker");
        Self { sender, join: Mutex::new(Some((handle, completed))) }
    }
    fn stop(&self) {
        let Some((handle, completed)) = self.join.lock().unwrap().take() else { return };
        let _ = self.sender.send(Request::Shutdown);
        // A hung clipboard owner must not hang application exit. Join only after completion.
        if completed.recv_timeout(Duration::from_millis(250)).is_ok() { let _ = handle.join(); }
    }
}
fn worker() -> &'static mpsc::Sender<Request> {
    &WORKER.get_or_init(|| Worker::spawn(|| arboard::Clipboard::new().ok())).sender
}

/// Called by the shared event-loop exit path, including overlay-only applications.
pub(crate) fn shutdown() {
    if let Some(worker) = WORKER.get() { worker.stop(); }
}

trait Backend {
    fn write(&mut self, copy: &ClipboardWrite) -> bool;
    fn text(&mut self) -> Option<Zeroizing<String>>;
    fn revision(&self) -> Option<u64>;
    fn clear(&mut self);
}
impl Backend for arboard::Clipboard {
    fn write(&mut self, copy: &ClipboardWrite) -> bool { write(self, copy).is_ok() }
    fn text(&mut self) -> Option<Zeroizing<String>> { self.get_text().ok().map(Zeroizing::new) }
    fn revision(&self) -> Option<u64> { revision() }
    fn clear(&mut self) { let _ = arboard::Clipboard::clear(self); }
}

fn run_worker<B: Backend>(recv: mpsc::Receiver<Request>, mut open: impl FnMut() -> Option<B>) {
    let mut clipboard: Option<B> = None;
    let mut expiry: Option<Expiry> = None;
    loop {
        let request = match &expiry {
            Some(e) => recv.recv_timeout(e.deadline.saturating_duration_since(Instant::now())),
            None => recv.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        if expiry.as_ref().is_some_and(|e| Instant::now() >= e.deadline) {
            let e = expiry.take().unwrap();
            if let Some(cb) = &mut clipboard { e.clear_if_unchanged(cb); }
        }
        let request = match request {
            Ok(Request::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
        };
        if clipboard.is_none() { clipboard = open(); }
        let Some(cb) = &mut clipboard else { continue };
        match request {
            Request::Write(copy) => {
                if cb.write(&copy) {
                    expiry = copy.clear_after.and_then(|delay| Instant::now().checked_add(delay))
                        .map(|deadline| Expiry { deadline, text: copy.text, revision: cb.revision() });
                }
            }
            Request::Read { target, reply, wake } => {
                if let Some(text) = cb.text() {
                    if reply.send(Reply { target, text }).is_ok() { wake(); }
                }
            }
            Request::Shutdown => unreachable!(),
        }
    }
    if let (Some(expiry), Some(cb)) = (expiry, clipboard.as_mut()) { expiry.clear_if_unchanged(cb); }
    drop(clipboard);
}

fn write(cb: &mut arboard::Clipboard, copy: &ClipboardWrite) -> Result<(), arboard::Error> {
    if copy.clear_after.is_none() { return cb.set_text(copy.text.as_str()); }
    #[cfg(target_os = "macos")]
    { use arboard::SetExtApple; return cb.set().exclude_from_history().text(copy.text.as_str()); }
    #[cfg(target_os = "windows")]
    { use arboard::SetExtWindows; return cb.set().exclude_from_monitoring().text(copy.text.as_str()); }
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    { use arboard::SetExtLinux; return cb.set().exclude_from_history().text(copy.text.as_str()); }
    #[allow(unreachable_code)]
    cb.set_text(copy.text.as_str())
}

struct Expiry { deadline: Instant, text: Zeroizing<String>, revision: Option<u64> }
impl Expiry {
    fn matches(&self, text: &str, current: Option<u64>) -> bool {
        self.revision == current && self.text.as_str() == text
    }
    fn clear_if_unchanged(self, cb: &mut impl Backend) {
        if self.revision != cb.revision() { return; }
        if let Some(text) = cb.text() {
            if self.matches(&text, cb.revision()) { cb.clear(); }
        }
    }
}

fn revision() -> Option<u64> {
    #[cfg(target_os = "macos")]
    { return Some(unsafe { objc2_app_kit::NSPasteboard::generalPasteboard().changeCount() } as u64); }
    #[cfg(target_os = "windows")]
    { return Some(unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() } as u64); }
    // Linux's portable clipboard API has no generation counter; compare the current text.
    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Default)]
    struct State { text: String, revision: u64, clears: usize, drops: usize }
    struct Fake { state: Arc<Mutex<State>>, dropped: Option<mpsc::Sender<()>> }
    impl Backend for Fake {
        fn write(&mut self, copy: &ClipboardWrite) -> bool {
            let mut s = self.state.lock().unwrap();
            s.text = copy.to_string(); s.revision += 1;
            true
        }
        fn text(&mut self) -> Option<Zeroizing<String>> { Some(Zeroizing::new(self.state.lock().unwrap().text.clone())) }
        fn revision(&self) -> Option<u64> { Some(self.state.lock().unwrap().revision) }
        fn clear(&mut self) { let mut s = self.state.lock().unwrap(); s.text.clear(); s.clears += 1; }
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            self.state.lock().unwrap().drops += 1;
            if let Some(dropped) = &self.dropped { let _ = dropped.send(()); }
        }
    }

    #[test]
    fn shutdown_expires_only_our_pending_secret_before_dropping_backend() {
        for replacement in [None, Some("new value"), Some("fixture")] {
            let state = Arc::new(Mutex::new(State::default()));
            let shared = state.clone();
            let worker = Worker::spawn(move || Some(Fake { state: shared.clone(), dropped: None }));
            worker.sender.send(Request::Write(ClipboardWrite::new("fixture".into(), Some(Duration::from_secs(3600))))).unwrap();
            let (reply, received) = mpsc::channel();
            worker.sender.send(Request::Read { target: None, reply, wake: Arc::new(|| {}) }).unwrap();
            received.recv_timeout(Duration::from_secs(1)).unwrap(); // Write and revision recorded.
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
        worker.sender.send(Request::Write(ClipboardWrite::new("fixture".into(), Some(Duration::from_secs(3600))))).unwrap();
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        let start = Instant::now();
        worker.stop();
        assert!(start.elapsed() < Duration::from_secs(1));
        release.send(()).unwrap();
        finished.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn expiry_preserves_newer_clipboards_including_identical_text() {
        let e = Expiry { deadline: Instant::now(), text: Zeroizing::new("fixture".into()), revision: Some(7) };
        assert!(e.matches("fixture", Some(7)));
        assert!(!e.matches("other", Some(7)));
        assert!(!e.matches("fixture", Some(8)));
        let copy = ClipboardWrite::new("fixture".into(), Some(Duration::from_secs(30)));
        assert!(!format!("{copy:?}").contains("fixture"));
    }
}
