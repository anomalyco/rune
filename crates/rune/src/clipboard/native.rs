//! The OS clipboard: one worker thread over arboard, and the per-OS reads and writes.

use super::*;

impl Client {
    pub(super) fn read_for(&self, purpose: Purpose) {
        worker().send(Request::Read { purpose, reply: self.sender.clone(), wake: self.wake.clone() }).ok();
    }
    pub(crate) fn flush_core(&self, rt: &mut crate::runtime::RuntimeCore) {
        if rt.edit.has_clipboard() { return flush_memory(rt); }
        #[cfg(test)]
        if let Some(m) = &self.memory { return flush_to_memory(rt, m); }
        #[cfg(target_os = "linux")]
        self.flush_primary(rt);
        // Nothing to clear before the first write.
        if std::mem::take(&mut rt.edit.clear_secret_request) && let Some(w) = WORKER.get() { w.sender.send(Request::Clear(None)).ok(); }
        if let Some(mut copy) = rt.copy_request.take() {
            let worker = worker_handle();
            copy.bind(Route::System(worker.sender.clone()));
            if let Some(id) = copy.copy_image_id() {
                copy.attach_copy_reply(CopyReply { id, sender: self.sender.clone(), wake: self.wake.clone() });
            }
            worker.send_write(copy, Some(Notify { changed: self.changed.clone(), wake: self.wake.clone() }), revision());
        }
        if let Some(target) = rt.edit.paste_request.take() { self.read_for(Purpose::paste(rt, target)); }
        for id in std::mem::take(&mut rt.edit.read_requests) { self.read_for(Purpose::Read(id)); }
    }
}

static WORKER: OnceLock<Worker> = OnceLock::new();
#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
pub(super) fn worker_started() -> bool { WORKER.get().is_some() }
pub(super) struct Worker {
    pub(super) sender: mpsc::Sender<Request>,
    latest_write: Arc<AtomicU64>,
    join: Mutex<Option<(std::thread::JoinHandle<()>, mpsc::Receiver<()>)>>,
}
impl Worker {
    pub(super) fn spawn<B: Backend + 'static>(open: impl FnMut() -> Option<B> + Send + 'static) -> Self {
        let (sender, recv) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let handle = std::thread::Builder::new().name("rune-clipboard".into()).spawn(move || {
            run_worker(recv, open);
            // Backend dropped (including Linux handoff) before acknowledging completion.
            done.send(()).ok();
        }).expect("clipboard worker");
        Self { sender, latest_write: Arc::new(AtomicU64::new(0)), join: Mutex::new(Some((handle, completed))) }
    }
    pub(super) fn send_write(&self, mut copy: ClipboardWrite, notify: Option<Notify>, revision_before: Option<u64>) {
        let generation = self.latest_write.fetch_add(1, Ordering::AcqRel) + 1;
        copy.stamp(generation, self.latest_write.clone(), revision_before);
        self.sender.send(Request::Write(copy, notify)).ok();
    }
    pub(super) fn stop(&self) {
        let Some((handle, completed)) = self.join.lock().unwrap().take() else { return };
        self.sender.send(Request::Shutdown).ok();
        // A hung clipboard owner must not hang application exit. Join only after completion.
        if completed.recv_timeout(Duration::from_millis(250)).is_ok() { handle.join().ok(); }
    }
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn worker_handle() -> &'static Worker {
    if cfg!(test) {
        panic!("unit test reached the OS clipboard; install a test clipboard (`rt.use_clipboard` or `Client::use_memory`)");
    }
    WORKER.get_or_init(|| Worker::spawn(|| arboard::Clipboard::new().ok()))
}

#[cfg(any(target_os = "ios", target_os = "android"))]
fn worker_handle() -> &'static Worker {
    WORKER.get_or_init(|| Worker::spawn(|| None::<Board>))
}

fn worker() -> &'static mpsc::Sender<Request> {
    &worker_handle().sender
}

/// Called by the shared event-loop exit path, including overlay-only applications.
/// `clipboard::clear_secret_now`: the worker clears the last secret if it still holds the clipboard.
#[cfg(target_os = "windows")]
pub(super) fn clear_secret_now() {
    if let Some(w) = WORKER.get() { w.sender.send(Request::Clear(None)).ok(); }
}

pub(super) fn shutdown() {
    if let Some(worker) = WORKER.get() { worker.stop(); }
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
impl Backend for arboard::Clipboard {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> { write(self, copy) }
    fn text(&mut self) -> Option<Zeroizing<String>> { self.get_text().ok().map(Zeroizing::new) }
    #[cfg(target_os = "macos")]
    fn content(&mut self, all: bool) -> Option<ClipboardContent> {
        macos::read(&objc2_app_kit::NSPasteboard::generalPasteboard(), all)
    }
    /// Text, HTML, and with `all` an image and files; RTF isn't read here. Windows and Linux
    /// images come through arboard as pixels (Windows `PNG` or `CF_DIBV5`, Linux `image/png`),
    /// encoded as PNG again.
    #[cfg(not(target_os = "macos"))]
    fn content(&mut self, all: bool) -> Option<ClipboardContent> {
        let text = self.text();
        #[cfg(target_os = "windows")]
        let html = windows_read_html().or_else(|| self.get().html().ok()).filter(|h| !h.is_empty() && h.len() <= html::MAX_RICH_BYTES);
        #[cfg(not(target_os = "windows"))]
        let html = self.get().html().ok().filter(|h| !h.is_empty() && h.len() <= html::MAX_RICH_BYTES);
        #[cfg(any(target_os = "windows", all(unix, not(any(target_os = "ios", target_os = "android")))))]
        let image = all.then(|| self.get_image().ok()).flatten()
            .and_then(|i| u32::try_from(i.width).ok().zip(u32::try_from(i.height).ok()).and_then(|(w, h)| ClipboardImage::from_rgba(w, h, &i.bytes)));
        #[cfg(not(any(target_os = "windows", all(unix, not(any(target_os = "ios", target_os = "android"))))))]
        let image: Option<ClipboardImage> = None;
        let files = if all { self.get().file_list().unwrap_or_default() } else { Vec::new() };
        if text.is_none() && html.is_none() && image.is_none() && files.is_empty() { return None; }
        let mut content = ClipboardContent::from(text.unwrap_or_default()).with_files(files);
        if let Some(html) = html { content = content.with_html(html); }
        if let Some(image) = image { content = content.with_image(image); }
        Some(content)
    }
    fn revision(&self) -> Option<u64> { revision() }
    fn clear(&mut self) -> Result<(), String> { arboard::Clipboard::clear(self).map_err(|e| e.to_string()) }
}

fn run_worker<B: Backend>(recv: mpsc::Receiver<Request>, mut open: impl FnMut() -> Option<B>) {
    let mut clipboard: Option<B> = None;
    let mut core = Core::default();
    loop {
        let request = match core.next_check() {
            Some(at) => recv.recv_timeout(at.saturating_duration_since(crate::time::now())),
            None => recv.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        if let Some(cb) = &mut clipboard { core.tick(cb); }
        let request = match request {
            Ok(Request::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
        };
        if matches!(request, Request::Clear(_)) && core.hold.is_free() { continue; }
        if clipboard.is_none() { clipboard = open(); }
        let Some(cb) = &mut clipboard else {
            if let Request::Write(mut copy, notify) = request {
                if let Some(r) = copy.take_copy_reply() { r.send(Err(CopyImageError::Failed("no clipboard".into()))); }
                if let Some(slot) = copy.slot() {
                    slot.set(SecretState::Failed("no clipboard".into()));
                    if let Some(n) = notify { n.send(); }
                }
            }
            continue;
        };
        core.handle(cb, request);
    }
    if let Some(cb) = clipboard.as_mut() { core.clear(cb, None); }
    drop(clipboard);
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn write(cb: &mut arboard::Clipboard, copy: &ClipboardWrite) -> Result<(), String> {
    let content = &copy.content;
    if copy.clear_after.is_none() && content.is_rich() {
        #[cfg(target_os = "macos")]
        { return macos::write(&objc2_app_kit::NSPasteboard::generalPasteboard(), content); }
        #[cfg(not(target_os = "macos"))]
        return write_one_form(cb, copy);
    }
    write_text(cb, copy).map_err(|e| e.to_string())
}

/// Windows and Linux hold one of Rune's forms per write: the files, else the image, else HTML
/// beside the text (CF_HTML, `text/html`), else the text. RTF isn't written.
///
/// An image write never falls through to `set_text` when pixel extraction fails: returning an
/// error leaves the existing clipboard contents untouched instead of wiping them with empty text.
#[cfg(all(not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(super) enum OneForm<'a> {
    Files(&'a [std::path::PathBuf]),
    Image { png: &'a [u8], width: u32, height: u32, rgba: std::borrow::Cow<'a, [u8]> },
    Html(&'a str, &'a str),
    Text(&'a str),
}

#[cfg(all(not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(super) fn prepare_one_form(copy: &ClipboardWrite) -> Result<OneForm<'_>, String> {
    let content = &copy.content;
    if !content.files().is_empty() { return Ok(OneForm::Files(content.files())); }
    if let Some(image) = content.image() {
        let (width, height, rgba) = extract_image_rgba(image, copy.encoded_rgba.as_ref())?;
        return Ok(OneForm::Image { png: image.png(), width, height, rgba });
    }
    match content.html() {
        Some(html) => Ok(OneForm::Html(html, content.text())),
        None => Ok(OneForm::Text(content.text())),
    }
}

#[cfg(all(not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
fn write_one_form(cb: &mut arboard::Clipboard, copy: &ClipboardWrite) -> Result<(), String> {
    let error = |e: arboard::Error| e.to_string();
    match prepare_one_form(copy)? {
        OneForm::Files(files) => cb.set().file_list(files).map_err(error),
        OneForm::Image { png, width, height, rgba } => {
            #[cfg(target_os = "windows")]
            {
                let _ = cb;
                windows_write_image(png, width, height, &rgba)
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = png;
                cb.set_image(arboard::ImageData { width: width as usize, height: height as usize, bytes: rgba }).map_err(error)
            }
        }
        OneForm::Html(html, text) => cb.set().html(html, Some(text)).map_err(error),
        OneForm::Text(text) => cb.set_text(text).map_err(error),
    }
}

/// Returns straight-alpha sRGB RGBA8 for `image`, borrowing the pre-encoded buffer carried by
/// `ClipboardWrite` when it matches so `Tx::copy_image` skips decoding its freshly encoded PNG.
#[cfg(any(target_os = "windows", all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android")))))]
fn extract_image_rgba<'a>(
    image: &ClipboardImage,
    pre: Option<&'a (u32, u32, Arc<[u8]>)>,
) -> Result<(u32, u32, std::borrow::Cow<'a, [u8]>), String> {
    if let Some((w, h, bytes)) = pre
        && (*w, *h) == image.size()
        && usize::try_from(*w)
            .ok()
            .and_then(|w| usize::try_from(*h).ok().and_then(|h| w.checked_mul(h)?.checked_mul(4)))
            == Some(bytes.len())
    {
        return Ok((*w, *h, std::borrow::Cow::Borrowed(bytes)));
    }
    let rgba = image.to_rgba().ok_or_else(|| "failed to decode clipboard image".to_string())?;
    Ok((rgba.width(), rgba.height(), std::borrow::Cow::Owned(rgba.into_raw())))
}

/// Packs straight-alpha sRGB RGBA8 into a bottom-up 32-bit `CF_DIBV5` (`BITMAPV5HEADER` + BGRA
/// pixels, `BI_BITFIELDS`, `LCS_sRGB`) so legacy Windows apps (Paint, Word) paste it cleanly
/// alongside the registered `"PNG"` format.
#[cfg(target_os = "windows")]
pub(super) fn pack_dibv5(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    const HEADER: usize = 124;
    const BI_BITFIELDS: u32 = 3;
    const LCS_SRGB: u32 = 0x7352_4742;
    const LCS_GM_IMAGES: u32 = 4;
    if width == 0 || height == 0 { return None; }
    let w_i32 = i32::try_from(width).ok()?;
    let h_i32 = i32::try_from(height).ok()?;
    let row_bytes = usize::try_from(width).ok()?.checked_mul(4)?;
    let pixel_bytes = row_bytes.checked_mul(usize::try_from(height).ok()?)?;
    if rgba.len() != pixel_bytes { return None; }
    let size_image = u32::try_from(pixel_bytes).ok()?;
    let total = HEADER.checked_add(pixel_bytes)?;
    let mut out = vec![0u8; total];
    out[0..4].copy_from_slice(&(HEADER as u32).to_le_bytes());
    out[4..8].copy_from_slice(&w_i32.to_le_bytes());
    out[8..12].copy_from_slice(&h_i32.to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&32u16.to_le_bytes());
    out[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
    out[20..24].copy_from_slice(&size_image.to_le_bytes());
    out[40..44].copy_from_slice(&0x00FF_0000u32.to_le_bytes());
    out[44..48].copy_from_slice(&0x0000_FF00u32.to_le_bytes());
    out[48..52].copy_from_slice(&0x0000_00FFu32.to_le_bytes());
    out[52..56].copy_from_slice(&0xFF00_0000u32.to_le_bytes());
    out[56..60].copy_from_slice(&LCS_SRGB.to_le_bytes());
    out[108..112].copy_from_slice(&LCS_GM_IMAGES.to_le_bytes());
    for y in 0..height as usize {
        let src_row = &rgba[(height as usize - 1 - y) * row_bytes..][..row_bytes];
        let dst_row = &mut out[HEADER + y * row_bytes..][..row_bytes];
        for (src, dst) in src_row.as_chunks::<4>().0.iter().zip(dst_row.as_chunks_mut::<4>().0.iter_mut()) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
    }
    Some(out)
}

/// Writes registered `"PNG"` first and `CF_DIBV5` second in one clipboard transaction, matching
/// Chromium's `ClipboardWin::WriteBitmap`. Allocates both buffers before emptying the clipboard
/// so a failure never wipes what the user already had.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn windows_write_image(png: &[u8], width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{GlobalFree, HGLOBAL};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    const CF_DIBV5: u32 = 17;
    const PNG_NAME: &[u16] = &[b'P' as u16, b'N' as u16, b'G' as u16, 0];

    fn alloc_global(bytes: &[u8]) -> Result<HGLOBAL, String> {
        if bytes.is_empty() { return Err("empty clipboard image buffer".into()); }
        // SAFETY: allocates a movable global memory block of `bytes.len()`, copies `bytes` into
        // the locked pointer, and unlocks before returning ownership of the handle to the caller.
        unsafe {
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if h.is_null() { return Err("GlobalAlloc failed for clipboard image".into()); }
            let ptr = GlobalLock(h) as *mut u8;
            if ptr.is_null() {
                let _ = GlobalFree(h);
                return Err("GlobalLock failed for clipboard image".into());
            }
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            let _ = GlobalUnlock(h);
            Ok(h)
        }
    }

    let dib = pack_dibv5(width, height, rgba).ok_or_else(|| "clipboard image exceeds DIB size limits".to_string())?;
    let png_h = alloc_global(png)?;
    let dib_h = match alloc_global(&dib) {
        Ok(h) => h,
        Err(e) => {
            // SAFETY: `png_h` was allocated by `alloc_global` and has not been handed to Windows.
            unsafe { let _ = GlobalFree(png_h); }
            return Err(e);
        }
    };
    // SAFETY: `PNG_NAME` is NUL-terminated; clipboard is opened, emptied, populated, and closed;
    // handles transferred to `SetClipboardData` are owned by Windows on success and freed here on failure.
    unsafe {
        let png_fmt = RegisterClipboardFormatW(PNG_NAME.as_ptr());
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(core::ptr::null_mut()) != 0 { opened = true; break; }
            std::thread::sleep(Duration::from_millis(5));
        }
        if !opened {
            let _ = GlobalFree(png_h);
            let _ = GlobalFree(dib_h);
            return Err("OpenClipboard failed".into());
        }
        if EmptyClipboard() == 0 {
            let _ = CloseClipboard();
            let _ = GlobalFree(png_h);
            let _ = GlobalFree(dib_h);
            return Err("EmptyClipboard failed".into());
        }
        let png_ok = if png_fmt != 0 && !SetClipboardData(png_fmt, png_h).is_null() {
            true
        } else {
            let _ = GlobalFree(png_h);
            false
        };
        let dib_ok = if !SetClipboardData(CF_DIBV5, dib_h).is_null() {
            true
        } else {
            let _ = GlobalFree(dib_h);
            false
        };
        let _ = CloseClipboard();
        if png_ok || dib_ok { Ok(()) } else { Err("SetClipboardData failed for clipboard image".into()) }
    }
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn write_text(cb: &mut arboard::Clipboard, copy: &ClipboardWrite) -> Result<(), arboard::Error> {
    let text = copy.content.text();
    if copy.clear_after.is_none() { return cb.set_text(text); }
    #[cfg(target_os = "macos")]
    { use arboard::SetExtApple; return cb.set().exclude_from_history().text(text); }
    #[cfg(target_os = "windows")]
    { use arboard::SetExtWindows; return cb.set().exclude_from_monitoring().text(text); }
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    { use arboard::SetExtLinux; return cb.set().exclude_from_history().text(text); }
    #[allow(unreachable_code)]
    cb.set_text(text)
}

#[allow(unsafe_code)]
pub(super) fn revision() -> Option<u64> {
    #[cfg(target_os = "macos")]
    { return Some(objc2_app_kit::NSPasteboard::generalPasteboard().changeCount() as u64); }
    #[cfg(target_os = "windows")]
    // SAFETY: `GetClipboardSequenceNumber` takes no pointers and is safe to call from any thread.
    { return Some(unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() } as u64); }
    // Linux, iOS and Android have no portable generation counter; compare the current text.
    #[allow(unreachable_code)]
    None
}

/// Reads Windows `CF_HTML` (`HTML Format`) directly so `-1` offsets and `<!--StartFragment-->`
/// markers from other apps work instead of failing `clipboard-win`'s unsigned offset parser.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn windows_read_html() -> Option<String> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    const NAME: &[u16] = &[b'H' as u16, b'T' as u16, b'M' as u16, b'L' as u16, b' ' as u16, b'F' as u16, b'o' as u16, b'r' as u16, b'm' as u16, b'a' as u16, b't' as u16, 0];
    // SAFETY: `NAME` is NUL-terminated; the clipboard is held open around `GetClipboardData` and
    // closed before returning, and `ptr` is non-null and locked via `GlobalLock` for `size` bytes.
    unsafe {
        let format = RegisterClipboardFormatW(NAME.as_ptr());
        if format == 0 || IsClipboardFormatAvailable(format) == 0 { return None; }
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(core::ptr::null_mut()) != 0 { opened = true; break; }
            std::thread::sleep(Duration::from_millis(5));
        }
        if !opened { return None; }
        let handle = GetClipboardData(format);
        let out = if !handle.is_null() {
            let size = GlobalSize(handle);
            let ptr = GlobalLock(handle) as *const u8;
            let res = (!ptr.is_null() && size > 0 && size <= html::MAX_RICH_BYTES * 2)
                .then(|| html::unpack_cf_html(core::slice::from_raw_parts(ptr, size)))
                .flatten();
            if !ptr.is_null() { let _ = GlobalUnlock(handle); }
            res
        } else {
            None
        };
        let _ = CloseClipboard();
        out
    }
}

