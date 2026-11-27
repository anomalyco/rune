//! Linux's primary selection on the OS: native `zwp_primary_selection_v1` on Wayland when the
//! compositor advertises it (GNOME/Mutter, Flatpak, KDE, Sway, Hyprland), falling back to a
//! dedicated arboard worker (`rune-primary`) on X11 or Wayland compositors without
//! `zwp_primary_selection_device_manager_v1` (data-control). Kept separate from the clipboard
//! worker so primary reads never wait behind clipboard reads and secret holds never see it.
//! What goes on it and where middle-click pastes live in `runtime/input/primary_selection.rs`.
use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};

use super::*;
use crate::platform::wayland::primary_selection::wire::{ReadDispatch, WriteDispatch};

enum Request {
    Write(String),
    /// Paste it into field `target`: the answer comes back as that field's paste.
    Read { target: String, reply: mpsc::Sender<Reply>, wake: Waker },
}

static WORKER: OnceLock<mpsc::Sender<Request>> = OnceLock::new();

fn worker() -> &'static mpsc::Sender<Request> {
    if cfg!(test) {
        panic!("unit test reached the OS primary selection; install a test clipboard (`rt.use_clipboard` or `Client::use_memory`)");
    }
    WORKER.get_or_init(|| {
        let (sender, requests) = mpsc::channel();
        std::thread::Builder::new().name("rune-primary".into()).spawn(move || run(requests)).ok();
        sender
    })
}

fn run(requests: mpsc::Receiver<Request>) {
    let mut board: Option<arboard::Clipboard> = None;
    for request in requests {
        if board.is_none() { board = arboard::Clipboard::new().ok(); }
        let Some(cb) = board.as_mut() else { continue };
        match request {
            Request::Write(text) => { cb.set().clipboard(LinuxClipboardKind::Primary).text(text).ok(); }
            Request::Read { target, reply, wake } => {
                let text = cb.get().clipboard(LinuxClipboardKind::Primary).text().ok().filter(|t| !t.is_empty());
                let answer = Purpose::Paste(Some(target), Forms::Text).reply(text.map(|t| ClipboardContent::from(Zeroizing::new(t))));
                if let Some(answer) = answer && reply.send(answer).is_ok() { wake(); }
            }
        }
    }
}

impl Client {
    /// Hands the Wayland `zwp_primary_selection_v1` backend (or arboard fallback worker) what
    /// the runtime asked of the primary selection.
    pub(super) fn flush_primary(&self, rt: &mut crate::runtime::RuntimeCore) {
        if let Some(text) = rt.edit.primary.write.take() {
            match crate::platform::wayland::primary_selection::wire::try_write(&text) {
                WriteDispatch::Handled => {}
                WriteDispatch::Fallback => { worker().send(Request::Write(text)).ok(); }
            }
        }
        if let Some(target) = rt.edit.primary.paste.take() {
            let reply_tx = self.sender.clone();
            let wake_reply = self.wake.clone();
            let deliver = Box::new(move |target: String, text: String, wake_needed: bool| {
                if text.is_empty() { return; }
                let answer = Purpose::Paste(Some(target), Forms::Text)
                    .reply(Some(ClipboardContent::from(Zeroizing::new(text))));
                if let Some(answer) = answer && reply_tx.send(answer).is_ok() && wake_needed {
                    wake_reply();
                }
            });
            match crate::platform::wayland::primary_selection::wire::try_read(target, self.wake.clone(), deliver) {
                ReadDispatch::Handled => {}
                ReadDispatch::Fallback(target) => {
                    worker().send(Request::Read { target, reply: self.sender.clone(), wake: self.wake.clone() }).ok();
                }
            }
        }
    }
}
