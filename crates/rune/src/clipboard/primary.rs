//! Linux's primary selection on the OS (X11 `PRIMARY`, Wayland's primary selection through
//! data control): its own worker over arboard, beside the clipboard's, so a primary read never
//! waits behind a clipboard read and a secret's hold never sees it. What goes on it and where a
//! middle click pastes it is the runtime's (`runtime/input/primary_selection.rs`).
use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};

use super::*;

enum Request {
    Write(String),
    /// Paste it into field `target`: the answer comes back as that field's paste.
    Read { target: String, reply: mpsc::Sender<Reply>, wake: Waker },
}

static WORKER: OnceLock<mpsc::Sender<Request>> = OnceLock::new();

fn worker() -> &'static mpsc::Sender<Request> {
    WORKER.get_or_init(|| {
        let (sender, requests) = mpsc::channel();
        let _ = std::thread::Builder::new().name("rune-primary".into()).spawn(move || run(requests));
        sender
    })
}

fn run(requests: mpsc::Receiver<Request>) {
    let mut board: Option<arboard::Clipboard> = None;
    for request in requests {
        if board.is_none() { board = arboard::Clipboard::new().ok(); }
        let Some(cb) = board.as_mut() else { continue };
        match request {
            Request::Write(text) => { let _ = cb.set().clipboard(LinuxClipboardKind::Primary).text(text); }
            Request::Read { target, reply, wake } => {
                let text = cb.get().clipboard(LinuxClipboardKind::Primary).text().ok().filter(|t| !t.is_empty());
                let answer = Purpose::Paste(Some(target), Forms::Text).reply(text.map(|t| ClipboardContent::from(Zeroizing::new(t))));
                if let Some(answer) = answer && reply.send(answer).is_ok() { wake(); }
            }
        }
    }
}

impl Client {
    /// Hands the worker what the runtime asked of the primary selection.
    pub(super) fn flush_primary<S: 'static>(&self, rt: &mut Runtime<S>) {
        if let Some(text) = rt.primary.write.take() { let _ = worker().send(Request::Write(text)); }
        if let Some(target) = rt.primary.paste.take() {
            let _ = worker().send(Request::Read { target, reply: self.sender.clone(), wake: self.wake.clone() });
        }
    }
}
