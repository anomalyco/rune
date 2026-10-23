//! One OS clipboard worker shared by windows and overlays. No OS reads on the event thread.
use std::sync::{OnceLock, mpsc};
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

fn worker() -> &'static mpsc::Sender<Request> {
    static WORKER: OnceLock<mpsc::Sender<Request>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (send, recv) = mpsc::channel();
        std::thread::Builder::new().name("rune-clipboard".into()).spawn(move || {
            let mut clipboard = None;
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
                    Ok(request) => request,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if clipboard.is_none() { clipboard = arboard::Clipboard::new().ok(); }
                let Some(cb) = &mut clipboard else { continue };
                match request {
                    Request::Write(copy) => {
                        if write(cb, &copy).is_ok() {
                            expiry = copy.clear_after.and_then(|delay| Instant::now().checked_add(delay))
                                .map(|deadline| Expiry { deadline, text: copy.text, revision: revision() });
                        }
                    }
                    Request::Read { target, reply, wake } => {
                        if let Ok(text) = cb.get_text() {
                            if reply.send(Reply { target, text: Zeroizing::new(text) }).is_ok() { wake(); }
                        }
                    }
                }
            }
        }).expect("clipboard worker");
        send
    })
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
    fn clear_if_unchanged(self, cb: &mut arboard::Clipboard) {
        if self.revision != revision() { return; }
        if let Ok(text) = cb.get_text() {
            let text = Zeroizing::new(text);
            if self.matches(&text, revision()) { let _ = cb.clear(); }
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
