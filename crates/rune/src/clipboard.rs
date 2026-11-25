//! One OS clipboard worker shared by windows and overlays. No OS reads on the event thread.
//!
//! A secret copy hands back a `SecretCopy` the app can ask about or clear. The worker keeps a
//! keyed fingerprint of the secret, never its text, and watches the clipboard's change count
//! while it holds one. Tests can give runtimes a shared `memory()` clipboard instead; without
//! one, headless runtimes never touch the OS clipboard.
//!
//! Copies and pastes can carry HTML and RTF beside the plain text ([`ClipboardContent`]).
//! HTML read from the clipboard passes through [`sanitize_html`] on the worker; on macOS an
//! RTF-only clipboard (TextEdit, Notes, Pages) is converted to HTML there too.
use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
#[cfg(any(test, not(target_arch = "wasm32")))]
use std::sync::OnceLock;
use std::time::Duration;
use crate::time::Instant;
use zeroize::Zeroizing;
use crate::machine::Seq;
#[cfg(any(test, target_arch = "wasm32"))]
use crate::Runtime;
use crate::Waker;

mod content;
mod html;
mod secret;
#[cfg(test)]
mod secret_tests;
pub(crate) mod tx;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_os = "linux")]
mod primary;
#[cfg(target_arch = "wasm32")]
mod web;
pub(crate) mod web_file;
#[cfg(target_os = "macos")]
pub(crate) use macos::tiff_to_png;
pub use content::{ClipboardContent, ClipboardImage, CopyImageError, MAX_IMAGE_BYTES};
use content::valid_image_size;
pub use html::sanitize_html;
pub use secret::{SecretCopy, SecretState};
pub use web_file::{MAX_WEB_FILES, WebFile};
use secret::{Alarm, Hold, HoldEffect, HoldEvent, Slot, Unwritten, step_hold};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

/// Monotonic generation and OS clipboard revision captured when a write was queued, so a slow
/// background encode never overwrites a newer in-app or external-app clipboard write.
#[derive(Clone)]
struct WriteStamp {
    generation: u64,
    latest: Arc<AtomicU64>,
    revision_before: Option<u64>,
}
impl WriteStamp {
    fn superseded_in_app(&self) -> bool {
        self.latest.load(Ordering::Acquire) != self.generation
    }
    fn superseded(&self, current_rev: Option<u64>, last_own_rev: Option<u64>) -> bool {
        if self.superseded_in_app() { return true; }
        let (Some(before), Some(now)) = (self.revision_before, current_rev) else { return false };
        now != before && last_own_rev != Some(now)
    }
}

/// A pending clipboard write. Debug output never includes its contents.
#[derive(Clone)]
pub struct ClipboardWrite {
    content: ClipboardContent,
    clear_after: Option<Duration>,
    secret: Option<Arc<Unwritten>>,
    /// A decoded image to copy, encoded as PNG where the write is carried out.
    image: Option<crate::Image>,
    /// Straight-alpha sRGB RGBA8 retained beside the encoded PNG so Windows (`CF_DIBV5`) and
    /// Linux (`arboard`) write without decoding the PNG back to pixels.
    #[cfg(any(target_os = "windows", all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android")))))]
    encoded_rgba: Option<(u32, u32, Arc<[u8]>)>,
    stamp: Option<WriteStamp>,
    copy_image_id: Option<u64>,
    copy_reply: Option<CopyReply>,
}
impl ClipboardWrite {
    pub(crate) fn new(content: ClipboardContent) -> Self {
        Self {
            content,
            clear_after: None,
            secret: None,
            image: None,
            #[cfg(any(target_os = "windows", all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android")))))]
            encoded_rgba: None,
            stamp: None,
            copy_image_id: None,
            copy_reply: None,
        }
    }
    /// `Tx::copy_image`: the image alone, encoded off the event thread on native platforms.
    pub(crate) fn image(image: crate::Image) -> Self {
        Self { image: Some(image), ..Self::new(ClipboardContent::default()) }
    }
    #[cfg(target_arch = "wasm32")]
    fn has_pending_image(&self) -> bool { self.image.is_some() }
    pub(crate) fn set_copy_image_id(&mut self, id: u64) { self.copy_image_id = Some(id); }
    pub(crate) fn take_copy_image_id(&mut self) -> Option<u64> { self.copy_image_id.take() }
    fn copy_image_id(&self) -> Option<u64> { self.copy_image_id }
    fn attach_copy_reply(&mut self, reply: CopyReply) { self.copy_reply = Some(reply); }
    #[cfg(not(target_arch = "wasm32"))]
    fn take_copy_reply(&mut self) -> Option<CopyReply> { self.copy_reply.take() }
    #[cfg(not(target_arch = "wasm32"))]
    fn stamp(&mut self, generation: u64, latest: Arc<AtomicU64>, revision_before: Option<u64>) {
        self.stamp = Some(WriteStamp { generation, latest, revision_before });
    }
    /// Encodes the decoded image this write copies as PNG, into its content. Returns an error
    /// when the image is still decoding, failed to decode, or exceeds clipboard size limits;
    /// the write then leaves the clipboard as it was.
    fn encode_image(&mut self) -> Result<(), CopyImageError> {
        let Some(image) = self.image.take() else { return Ok(()) };
        if let Some(err) = image.error() { return Err(CopyImageError::DecodeFailed(err.clone())); }
        if !image.is_ready() { return Err(CopyImageError::NotReady); }
        let Some((w, h, rgba)) = image.to_rgba() else { return Err(CopyImageError::NotReady); };
        if !valid_image_size(w, h) { return Err(CopyImageError::TooLarge); }
        let png_bytes = crate::drag_out::encode_png(w, h, &rgba)
            .ok_or_else(|| CopyImageError::Failed("failed to encode PNG".into()))?;
        if png_bytes.len() > MAX_IMAGE_BYTES { return Err(CopyImageError::TooLarge); }
        let png = ClipboardImage::from_png(png_bytes).ok_or(CopyImageError::TooLarge)?;
        #[cfg(any(target_os = "windows", all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android")))))]
        { self.encoded_rgba = Some((w, h, Arc::from(rgba))); }
        self.content = std::mem::take(&mut self.content).with_image(png);
        Ok(())
    }
    /// Secrets are plain text only: no HTML or RTF copy of a password lingers.
    pub(crate) fn secret(text: String, clear_after: Duration) -> (Self, SecretCopy) {
        let slot = Arc::new(Slot::default());
        let write = Self {
            content: ClipboardContent::new(text),
            clear_after: Some(clear_after),
            secret: Some(Arc::new(Unwritten(slot.clone()))),
            image: None,
            #[cfg(any(target_os = "windows", all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android")))))]
            encoded_rgba: None,
            stamp: None,
            copy_image_id: None,
            copy_reply: None,
        };
        (write, SecretCopy(slot))
    }
    /// Everything this write puts on the clipboard. A `Tx::copy_image` write has no image here
    /// until it is carried out, when its PNG is encoded.
    pub fn content(&self) -> &ClipboardContent { &self.content }
    pub(crate) fn is_secret(&self) -> bool { self.clear_after.is_some() }
    fn slot(&self) -> Option<&Arc<Slot>> { self.secret.as_ref().map(|u| &u.0) }
    fn bind(&self, route: Route) {
        if let Some(slot) = self.slot() { slot.route.set(route).ok(); }
    }
}
impl std::ops::Deref for ClipboardWrite {
    type Target = str;
    fn deref(&self) -> &str { self.content.text() }
}
impl std::fmt::Debug for ClipboardWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardWrite").field("clear_after", &self.clear_after).finish_non_exhaustive()
    }
}

#[derive(Clone)]
enum Route {
    #[cfg(not(target_arch = "wasm32"))]
    System(mpsc::Sender<Request>),
    Memory(MemoryClipboard),
}
impl Route {
    fn send(&self, request: Request) {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Route::System(worker) => { worker.send(request).ok(); }
            Route::Memory(m) => m.handle(request),
        }
    }
}

/// Why the clipboard is read.
#[derive(Clone, Debug, PartialEq)]
enum Purpose {
    /// Cmd/Ctrl-V or `Tx::request_paste`, for this focused field or (`None`) the app, and the
    /// forms that target takes.
    Paste(Option<String>, Forms),
    /// `Tx::read_clipboard`'s callback with this id. It always hears back, empty or not.
    Read(u64),
}

/// The forms a paste target takes, so a read fetches no more than it needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Forms {
    /// A plain field or a plain `on_paste`.
    Text,
    /// A rich [`TextEdit`](crate::TextEdit) with no hook: text, HTML and RTF.
    Rich,
    /// A hook (`on_input_paste`, `on_paste_content`): every form, images too.
    All,
}
enum Reply {
    Content { purpose: Purpose, content: ClipboardContent },
    CopyImage { id: u64, result: Result<(), CopyImageError> },
}
#[derive(Clone)]
struct CopyReply { id: u64, sender: mpsc::Sender<Reply>, wake: Waker }
impl CopyReply {
    fn send(self, result: Result<(), CopyImageError>) {
        if self.sender.send(Reply::CopyImage { id: self.id, result }).is_ok() { (self.wake)(); }
    }
}
#[derive(Clone)]
struct Notify { changed: Arc<AtomicBool>, wake: Waker }
impl Notify {
    fn send(&self) { self.changed.store(true, Ordering::SeqCst); (self.wake)(); }
}
enum Request {
    #[cfg(not(target_arch = "wasm32"))]
    Shutdown,
    Write(ClipboardWrite, Option<Notify>),
    /// Clear this secret, or whichever secret is held, if the clipboard still holds it.
    Clear(Option<Arc<Slot>>),
    #[cfg(not(target_arch = "wasm32"))]
    Read { purpose: Purpose, reply: mpsc::Sender<Reply>, wake: Waker },
}
impl Purpose {
    fn paste(rt: &crate::runtime::RuntimeCore, target: Option<String>) -> Self {
        let forms = rt.paste_forms(target.as_deref());
        Purpose::Paste(target, forms)
    }
    fn forms(&self) -> Forms {
        match self { Purpose::Paste(_, forms) => *forms, Purpose::Read(_) => Forms::All }
    }
    /// Reads what this purpose needs from `cb` and what to send back: a paste skips an empty
    /// clipboard.
    fn answer<B: Backend>(self, cb: &mut B) -> Option<Reply> {
        let content = match self.forms() {
            Forms::Text => cb.text().map(ClipboardContent::from),
            forms => cb.content(forms == Forms::All),
        };
        self.reply(content)
    }
    fn reply(self, content: Option<ClipboardContent>) -> Option<Reply> {
        let content = match self {
            Purpose::Paste(..) => content?,
            Purpose::Read(_) => content.unwrap_or_default(),
        };
        Some(Reply::Content { purpose: self, content: content.untrusted() })
    }
}

pub(crate) struct Client {
    sender: mpsc::Sender<Reply>,
    replies: mpsc::Receiver<Reply>,
    wake: Waker,
    /// A secret copy's state changed on the worker: run a frame so the app's poll sees it.
    changed: Arc<AtomicBool>,
    /// Monotonic web clipboard write generation for superseding pending async image encodes.
    #[cfg(target_arch = "wasm32")]
    latest_write: std::rc::Rc<std::cell::Cell<u64>>,
    #[cfg(test)]
    memory: Option<MemoryClipboard>,
}
impl Default for Client {
    fn default() -> Self {
        let (sender, replies) = mpsc::channel();
        Self {
            sender,
            replies,
            wake: std::sync::Arc::new(|| {}),
            changed: Arc::default(),
            #[cfg(target_arch = "wasm32")]
            latest_write: std::rc::Rc::new(std::cell::Cell::new(0)),
            #[cfg(test)]
            memory: None,
        }
    }
}
impl Client {
    pub fn connect(&mut self, wake: Waker) { self.wake = wake; }
    /// Routes this client's flushes and reads to `clipboard` instead of the OS worker, without
    /// attaching it to the runtime (so `latch_requests` leaves requests queued until `flush_core`).
    #[cfg(test)]
    pub(crate) fn use_memory(&mut self, clipboard: &MemoryClipboard) { self.memory = Some(clipboard.clone()); }
    #[cfg(test)]
    pub(crate) fn read_core(&self, rt: &crate::runtime::RuntimeCore) {
        let purpose = Purpose::paste(rt, rt.paste_target());
        let mem = rt.edit.clipboard();
        #[cfg(test)]
        let mem = mem.or(self.memory.as_ref());
        if let Some(m) = mem {
            if let Some(reply) = m.answer(purpose) && self.sender.send(reply).is_ok() { (self.wake)(); }
            return;
        }
        self.read_for(purpose);
    }
    pub(crate) fn receive_core(&self, rt: &mut crate::runtime::RuntimeCore) -> bool {
        let mut received = self.changed.swap(false, Ordering::SeqCst);
        for reply in self.replies.try_iter() { deliver(rt, reply); received = true; }
        received
    }
}

fn deliver(rt: &mut crate::runtime::RuntimeCore, reply: Reply) {
    match reply {
        Reply::Content { purpose: Purpose::Paste(target, _), content } => rt.paste_result(target, content),
        Reply::Content { purpose: Purpose::Read(id), content } => rt.read_result(id, content),
        Reply::CopyImage { id, result } => rt.copy_image_result(id, result),
    }
}

/// Applies a runtime's clipboard requests to its memory clipboard, if it has one. Runs after
/// every frame, so headless runtimes write too.
pub(crate) fn flush_memory(rt: &mut crate::runtime::RuntimeCore) {
    let Some(m) = rt.edit.clipboard().cloned() else { return };
    flush_to_memory(rt, &m);
}

fn flush_to_memory(rt: &mut crate::runtime::RuntimeCore, m: &MemoryClipboard) {
    if std::mem::take(&mut rt.edit.clear_secret_request) { m.handle(Request::Clear(None)); }
    if let Some(mut copy) = rt.copy_request.take() {
        copy.bind(Route::Memory(m.clone()));
        let copy_rx = copy.copy_image_id().map(|id| {
            let (tx, rx) = mpsc::channel();
            copy.attach_copy_reply(CopyReply { id, sender: tx, wake: Arc::new(|| {}) });
            rx
        });
        m.handle(Request::Write(copy, None));
        if let Some(rx) = copy_rx {
            for reply in rx.try_iter() { deliver(rt, reply); }
        }
    }
    let paste = rt.edit.paste_request.take().map(|target| Purpose::paste(rt, target));
    let reads = paste.into_iter().chain(std::mem::take(&mut rt.edit.read_requests).into_iter().map(Purpose::Read));
    for purpose in reads.collect::<Vec<_>>() {
        if let Some(reply) = m.answer(purpose) { deliver(rt, reply); }
    }
}

impl crate::runtime::RuntimeCore {
    #[inline]
    pub(crate) fn use_clipboard(&mut self, clipboard: &MemoryClipboard) { self.edit.set_clipboard(clipboard.clone()); }
}

/// An in-memory clipboard for tests, shared by its clones and by every runtime given it with
/// `Runtime::use_clipboard`. It behaves like the OS clipboard, synchronously.
#[derive(Clone)]
pub struct MemoryClipboard(Arc<Mutex<Memory>>);
struct Memory { core: Core, board: Board }
#[derive(Default)]
struct Board { content: Option<ClipboardContent>, revision: u64, fail: Option<String> }

/// The OS clipboard's current change counter, when the platform exposes one without reading
/// the clipboard's contents.
///
/// Returns `Some(n)` on macOS (`NSPasteboard.changeCount`) and Windows
/// (`GetClipboardSequenceNumber`), where the counter changes on every write to the system
/// clipboard. Returns `None` on Linux, web, iOS and Android, which expose no portable
/// generation counter.
///
/// Reading the counter never opens or reads the clipboard's contents and starts no background
/// work. Compare two readings for inequality (`!=`) rather than ordering, as the Windows
/// sequence number is a 32-bit counter that can wrap.
///
/// ```
/// if let Some(rev) = rune::clipboard::revision() {
///     // Compare against the last seen revision before requesting a read.
///     let _ = rev;
/// }
/// ```
pub fn revision() -> Option<u64> {
    #[cfg(not(target_arch = "wasm32"))]
    { native::revision() }
    #[cfg(target_arch = "wasm32")]
    { None }
}

pub fn memory() -> MemoryClipboard {
    MemoryClipboard(Arc::new(Mutex::new(Memory { core: Core::default(), board: Board::default() })))
}
impl MemoryClipboard {
    pub fn text(&self) -> Option<Zeroizing<String>> { self.content().filter(ClipboardContent::has_plain).map(|mut c| Zeroizing::new(std::mem::take(&mut c.text.0))) }
    /// Every form on the clipboard, as written: pastes from it are sanitised, this isn't.
    pub fn content(&self) -> Option<ClipboardContent> { self.ticked(|_, board| board.content.clone()) }
    /// Current monotonic revision counter, incremented on every write, clear, or secret expiry.
    ///
    /// Starts at `0` on a fresh [`memory()`] clipboard and advances by `1` for each change.
    /// Failed writes (while [`fail_writes`](Self::fail_writes) is set) leave the revision
    /// unchanged.
    ///
    /// ```
    /// let board = rune::clipboard::memory();
    /// assert_eq!(board.revision(), 0);
    /// board.set_text("hello");
    /// assert_eq!(board.revision(), 1);
    /// ```
    pub fn revision(&self) -> u64 { self.ticked(|_, board| board.revision) }
    /// Changes the clipboard as another app would.
    pub fn set_text(&self, text: &str) { self.set_content(ClipboardContent::new(text)); }
    /// Puts several forms on the clipboard as another app would, e.g. HTML from a browser or
    /// RTF alone from a word processor.
    pub fn set_content(&self, content: ClipboardContent) {
        let mut m = lock(&self.0);
        let Memory { core, board } = &mut *m;
        board.content = Some(content);
        board.revision += 1;
        core.tick(board);
    }
    /// Makes later writes fail with `error`, or succeed again with `None`.
    pub fn fail_writes(&self, error: Option<&str>) { lock(&self.0).board.fail = error.map(Into::into); }
    fn tick(&self) { self.ticked(|_, _| ()) }
    fn answer(&self, purpose: Purpose) -> Option<Reply> { self.ticked(|core, board| core.answer(board, purpose)) }
    fn handle(&self, request: Request) { self.ticked(|core, board| core.handle(board, request)) }
    /// Lets a held secret expire or notice a change first, as the OS worker does per request.
    fn ticked<R>(&self, f: impl FnOnce(&mut Core, &mut Board) -> R) -> R {
        let mut m = lock(&self.0);
        let Memory { core, board } = &mut *m;
        core.tick(board);
        f(core, board)
    }
}
impl std::fmt::Debug for MemoryClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("MemoryClipboard").finish_non_exhaustive() }
}
impl Backend for Board {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> {
        if let Some(error) = &self.fail { return Err(error.clone()); }
        self.content = Some(copy.content.clone());
        self.revision += 1;
        Ok(())
    }
    fn text(&mut self) -> Option<Zeroizing<String>> {
        self.content.as_ref().filter(|c| c.has_plain()).map(|c| Zeroizing::new(c.text.0.clone()))
    }
    fn content(&mut self, all: bool) -> Option<ClipboardContent> {
        self.content.clone().and_then(|c| if all { Some(c) } else { c.text_forms() })
    }
    fn revision(&self) -> Option<u64> { Some(self.revision) }
    fn clear(&mut self) -> Result<(), String> { self.content = None; self.revision += 1; Ok(()) }
}

/// Wipes the secret Rune copied last, if the clipboard still holds it, from outside any handler
/// (the session locking, `platform::win32::session_lock`). Nothing was copied yet: nothing to do.
#[cfg(target_os = "windows")]
pub(crate) fn clear_secret_now() {
    native::clear_secret_now();
}

/// Called by the shared event-loop exit path, including overlay-only applications.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn shutdown() {
    native::shutdown();
}

trait Backend {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String>;
    fn text(&mut self) -> Option<Zeroizing<String>>;
    /// Every form a paste can use, unsanitised, and the image and files only when asked (`all`):
    /// decoding an image can take a while.
    fn content(&mut self, _all: bool) -> Option<ClipboardContent> { self.text().map(ClipboardContent::from) }
    fn revision(&self) -> Option<u64>;
    fn clear(&mut self) -> Result<(), String>;
}
/// How often the worker checks that a held secret is still on the clipboard. Where there is no
/// change count (Linux) it compares the text's fingerprint, so it checks less often.
#[cfg(not(target_arch = "wasm32"))]
const WATCH: Duration = if cfg!(any(target_os = "macos", target_os = "windows")) {
    Duration::from_millis(250)
} else {
    Duration::from_secs(1)
};

/// The pill copy Rune wrote last, while the clipboard still holds that exact write.
struct OwnPills {
    revision: Option<u64>,
    text_print: u64,
    pills: Vec<(std::ops::Range<usize>, String)>,
}

/// The outside moments of a write, in the order it reaches them: encoding an image, starting a
/// secret's delay, then hashing a secret's text. Tests slow each one down.
trait WriteSteps {
    fn now(&self) -> Instant;
    fn encoding(&self) {}
    fn hashing(&self) {}
}
struct RealTime;
impl WriteSteps for RealTime {
    fn now(&self) -> Instant { crate::time::now() }
}

/// The clipboard owner's bookkeeping, shared by the OS worker and the memory clipboard.
#[derive(Default)]
struct Core {
    keys: RandomState,
    seq: Seq,
    hold: Hold,
    alarm: Option<Alarm>,
    own_pills: Option<OwnPills>,
    last_own_revision: Option<u64>,
}
impl Core {
    #[cfg(not(target_arch = "wasm32"))]
    fn next_check(&self) -> Option<Instant> {
        self.hold.next_check(crate::time::now() + WATCH)
    }

    fn handle<B: Backend>(&mut self, cb: &mut B, request: Request) {
        match request {
            Request::Write(copy, notify) => self.write(cb, copy, notify, &RealTime),
            Request::Clear(which) => self.clear(cb, which.as_ref()),
            #[cfg(not(target_arch = "wasm32"))]
            Request::Read { purpose, reply, wake } => {
                if let Some(answer) = self.answer(cb, purpose) && reply.send(answer).is_ok() { wake(); }
            }
            #[cfg(not(target_arch = "wasm32"))]
            Request::Shutdown => {}
        }
    }

    fn answer<B: Backend>(&mut self, cb: &mut B, purpose: Purpose) -> Option<Reply> {
        let mut reply = purpose.answer(cb)?;
        if let Reply::Content { content, .. } = &mut reply && let Some(own) = &self.own_pills {
            let rev_ok = match (own.revision, cb.revision()) {
                (Some(a), Some(b)) => a == b,
                (None, None) => true,
                _ => false,
            };
            let html_ok = content.html().is_some_and(|h| {
                let frag = crate::TextEdit::from_html(h);
                frag.pills().count() == own.pills.len()
                    && frag.pills().zip(&own.pills).all(|((r1, id1), (r2, id2))| r1 == *r2 && id1 == id2)
            });
            if rev_ok && self.keys.hash_one(content.text()) == own.text_print && html_ok {
                content.own_pills = true;
            }
        }
        Some(reply)
    }

    fn write<B: Backend>(&mut self, cb: &mut B, mut copy: ClipboardWrite, notify: Option<Notify>, steps: &dyn WriteSteps) {
        let copy_reply = copy.copy_reply.take();
        if copy.stamp.as_ref().is_some_and(|s| s.superseded(cb.revision(), self.last_own_revision)) {
            if let Some(r) = copy_reply { r.send(Err(CopyImageError::Superseded)); }
            return;
        }
        let rev_before_encode = cb.revision();
        let had_image = copy.image.is_some();
        if had_image {
            steps.encoding();
            if let Err(err) = copy.encode_image() {
                if let Some(r) = copy_reply { r.send(Err(err)); }
                return;
            }
            let changed_during_encode = rev_before_encode.is_some_and(|before| cb.revision().is_some_and(|now| now != before));
            if changed_during_encode || copy.stamp.as_ref().is_some_and(|s| s.superseded(cb.revision(), self.last_own_revision)) {
                if let Some(r) = copy_reply { r.send(Err(CopyImageError::Superseded)); }
                return;
            }
        }
        let slot = copy.slot().cloned();
        let notify = notify.filter(|_| slot.is_some());
        if slot.as_ref().is_some_and(|s| s.cancelled()) {
            if let Some(r) = copy_reply { r.send(Err(CopyImageError::Superseded)); }
            return; // `Unwritten` settles it as cleared.
        }
        if let Err(error) = cb.write(&copy) {
            if let Some(r) = copy_reply { r.send(Err(CopyImageError::Failed(error.clone()))); }
            if let Some(s) = &slot { s.set(SecretState::Failed(error)); }
            if let Some(n) = &notify { n.send(); }
            return;
        }
        if let Some(r) = copy_reply { r.send(Ok(())); }
        self.apply(cb, HoldEvent::Lost);
        let rev = cb.revision();
        self.last_own_revision = rev;
        self.own_pills = copy
            .clear_after
            .is_none()
            .then(|| copy.content.html())
            .flatten()
            .filter(|h| h.contains("<span data-pill="))
            .map(|h| {
                let frag = crate::TextEdit::from_html(h);
                OwnPills {
                    revision: rev,
                    text_print: self.keys.hash_one(copy.content.text()),
                    pills: frag.pills().map(|(r, id)| (r, id.to_owned())).collect(),
                }
            })
            .filter(|o| !o.pills.is_empty());
        if let (Some(delay), Some(slot)) = (copy.clear_after, slot) {
            slot.held();
            let due = steps.now().checked_add(delay);
            steps.hashing();
            let print = self.keys.hash_one(copy.content.text());
            self.apply(cb, HoldEvent::Written { slot: slot.clone(), notify, due, revision: rev, print });
            if slot.cancelled() { self.clear(cb, Some(&slot)); }
        }
    }

    /// Steps the hold and carries its effects out, in order.
    fn apply<B: Backend>(&mut self, cb: &mut B, event: HoldEvent) {
        let mut answer = None;
        for effect in step_hold(&mut self.seq, &mut self.hold, event) {
            match effect {
                HoldEffect::Wipe => {
                    let ok = cb.clear().is_ok();
                    if ok { self.last_own_revision = cb.revision(); }
                    self.own_pills = None;
                    answer = Some(HoldEvent::Swept { ok });
                }
                HoldEffect::Release(slot, report) => slot.released(report),
                HoldEffect::Notify(notify) => notify.send(),
                HoldEffect::Arm(alarm) => self.alarm = alarm,
            }
        }
        if let Some(answer) = answer { self.apply(cb, answer); }
    }

    fn unchanged<B: Backend>(&self, cb: &mut B) -> bool {
        let Hold::Holding { revision, print, .. } = &self.hold else { return false };
        *revision == cb.revision() && cb.text().is_some_and(|t| self.keys.hash_one(t.as_str()) == *print)
    }

    fn clear<B: Backend>(&mut self, cb: &mut B, which: Option<&Arc<Slot>>) {
        if !self.hold.answers(which) { return; }
        if which.is_some_and(|s| s.failed()) { return self.recover(cb); }
        let ours = self.unchanged(cb);
        self.apply(cb, HoldEvent::Clear { ours });
    }

    /// Clears what a failed replay left held. Anything short of a confirmed wipe keeps the hold,
    /// so its delay still clears the secret.
    fn recover<B: Backend>(&mut self, cb: &mut B) {
        if !self.unchanged(cb) || cb.clear().is_err() || self.unchanged(cb) { return; }
        self.own_pills = None;
        self.apply(cb, HoldEvent::Wiped);
    }

    /// Expire the held secret, or let it go if the clipboard changed under it.
    fn tick<B: Backend>(&mut self, cb: &mut B) { self.tick_at(cb, crate::time::now()); }

    fn tick_at<B: Backend>(&mut self, cb: &mut B, now: Instant) {
        if let Some(own) = &self.own_pills
            && let Some(rev) = own.revision
            && cb.revision() != Some(rev)
        {
            self.own_pills = None;
        }
        if let Some(alarm) = self.alarm.filter(|a| now >= a.due) {
            self.alarm = None;
            if self.hold.armed(alarm.attempt) {
                let ours = self.unchanged(cb);
                return self.apply(cb, HoldEvent::Due { attempt: alarm.attempt, ours });
            }
        }
        let Hold::Holding { revision, .. } = &self.hold else { return };
        let changed = match *revision {
            Some(revision) => cb.revision() != Some(revision),
            None => !self.unchanged(cb),
        };
        if changed { self.apply(cb, HoldEvent::Lost); }
    }
}

#[cfg(test)]
mod tests;
