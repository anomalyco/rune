//! OLE drag source for [`super::begin`]: `DoDragDrop` with a shell data object and the shell's
//! drag image (`IDragSourceHelper`), so Explorer, browsers and chat apps take the drag as they
//! take one from Explorer.
//!
//! `DoDragDrop` runs its own modal loop until the drop. Started inside winit's handler for the
//! pointer move, the windows would stop drawing and their timers would wait until it ended, so
//! the session is posted to the window and runs from the message loop instead, as the window
//! menu does (`platform::win32::frame`): the app keeps drawing (the source's dragging style,
//! hovers) while the drag is out.
//!
//! The data: files as the shell's own list of items when they share a folder (what Explorer
//! itself puts on a drag, `CFSTR_SHELLIDLIST` and `CF_HDROP` among others), else a `CF_HDROP`;
//! PNG data under the registered `PNG` format; text as `CF_UNICODETEXT`. The picture is the
//! element's snapshot or the item's image, handed to the shell's drag image manager, which draws
//! it with the drop description every Windows drag shows; a file drag without a picture gets
//! the shell's own image of the files.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::sync::{Mutex, OnceLock, atomic::{AtomicUsize, Ordering}};

use windows::Win32::Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HGLOBAL, POINT, SIZE, S_OK};
use windows::Win32::Graphics::Gdi::HBITMAP;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE, DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, IDragSourceHelper, ILClone, ILCreateFromPathW, ILFindLastID, ILFree, ILIsParent, ILRemoveLastID, SHCreateDataObject, SHDRAGIMAGE};
use windows::core::{BOOL, HRESULT, PCWSTR, implement};

use super::{DragImage, DragItem, DragOperation, DragOutStart, DragOutcome, path_text};

type Done = Box<dyn FnOnce(DragOutcome)>;

/// A session waiting for its posted message.
struct Pending {
    hwnd: usize,
    id: usize,
    start: DragOutStart,
    picture: Option<DragImage>,
    done: Done,
    scale: f32,
}

static NEXT_DRAG: AtomicUsize = AtomicUsize::new(0x5255_4e47);

thread_local! {
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
    static DRAGGING: Cell<bool> = const { Cell::new(false) };
}

struct DragGuard;

impl Drop for DragGuard {
    fn drop(&mut self) {
        DRAGGING.with(|d| d.set(false));
    }
}

fn message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: the name is NUL-terminated.
    *MESSAGE.get_or_init(|| unsafe { windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(wide("Rune::DragOut").as_ptr()) })
}

pub(super) fn begin(window: &winit::window::Window, start: DragOutStart, picture: Option<DragImage>, done: Done) {
    // Refuse re-entrant drag loops while `DoDragDrop` is already on the stack.
    if DRAGGING.with(Cell::get) { return done(DragOutcome::Cancelled); }
    let Some(hwnd) = crate::platform::win32::hwnd(window) else { return done(DragOutcome::Cancelled) };
    let id = NEXT_DRAG.fetch_add(1, Ordering::Relaxed);
    let pending = Pending { hwnd: hwnd as usize, id, start, picture, done, scale: window.scale_factor() as f32 };
    use windows_sys::Win32::UI::Shell::{RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindow, PostMessageW};
    // One drag at a time: remove any earlier window's pending subclass and cancel it.
    if let Some(old) = PENDING.with(|p| p.borrow_mut().replace(pending)) {
        // SAFETY: removes the old drag's subclass if its window is still live.
        unsafe {
            if IsWindow(old.hwnd as _) != 0 {
                RemoveWindowSubclass(old.hwnd as _, Some(procedure), old.id);
            }
        }
        (old.done)(DragOutcome::Cancelled);
    }
    // SAFETY: Win32 calls on a live window this thread owns; the subclass keeps no pointer.
    let posted = unsafe {
        SetWindowSubclass(hwnd, Some(procedure), id, 0) != 0
            && (PostMessageW(hwnd, message(), id, 0) != 0
                || { RemoveWindowSubclass(hwnd, Some(procedure), id); false })
    };
    if !posted {
        if let Some(p) = PENDING.with(|p| {
            let mut slot = p.borrow_mut();
            slot.as_ref().is_some_and(|cur| cur.id == id).then(|| slot.take()).flatten()
        }) {
            (p.done)(DragOutcome::Cancelled);
        }
    }
}

unsafe extern "system" fn procedure(hwnd: windows_sys::Win32::Foundation::HWND, msg: u32, w: usize, l: isize, id: usize, _: usize) -> isize {
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY;
    if (msg == message() && w == id) || msg == WM_NCDESTROY {
        // SAFETY: removes this subclass from the window it was set on.
        unsafe { RemoveWindowSubclass(hwnd, Some(procedure), id) };
        if msg == message() {
            run(hwnd as usize, id);
            return 0;
        }
        if let Some(p) = PENDING.with(|p| {
            let mut slot = p.borrow_mut();
            slot.as_ref().is_some_and(|cur| cur.hwnd == hwnd as usize && cur.id == id).then(|| slot.take()).flatten()
        }) {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (p.done)(DragOutcome::Cancelled))).ok();
        }
    }
    // SAFETY: passes every other message on.
    unsafe { DefSubclassProc(hwnd, msg, w, l) }
}

/// Runs the pending session to its end and reports how it ended.
fn run(hwnd: usize, id: usize) {
    let Some(p) = PENDING.with(|p| {
        let mut slot = p.borrow_mut();
        slot.as_ref().is_some_and(|cur| cur.hwnd == hwnd && cur.id == id).then(|| slot.take()).flatten()
    }) else { return };
    if DRAGGING.with(|d| d.replace(true)) {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (p.done)(DragOutcome::Cancelled))).ok();
        return;
    }
    let _guard = DragGuard;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session(&p)))
        .ok()
        .flatten()
        .unwrap_or(DragOutcome::Cancelled);
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (p.done)(outcome))).is_err() {
        eprintln!("rune: drag end callback panicked");
    }
}

fn session(p: &Pending) -> Option<DragOutcome> {
    let item = &p.start.item;
    // SAFETY: the UI thread is OLE's apartment (the drop target made it one; again is a no-op).
    // Every object made here is released when it drops; media handed to `SetData` with
    // `fRelease` belong to the data object.
    unsafe {
        OleInitialize(None).ok();
        let data = populate_data_object(item)?;
        if let Ok(helper) = CoCreateInstance::<_, IDragSourceHelper>(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) {
            let picture = p.picture.as_ref().and_then(|pic| drag_image(pic, &p.start, p.scale));
            match picture {
                Some(shdi) => if helper.InitializeFromBitmap(&shdi, &data).is_err() { windows_sys::Win32::Graphics::Gdi::DeleteObject(shdi.hbmpDragImage.0); },
                // The shell's own picture of the files, under the pointer.
                None if !item.paths().is_empty() => { helper.InitializeFromWindow(None, None, &data).ok(); }
                None => {}
            }
        }
        let within_app = item.drops_within_app();
        if !within_app { crate::platform::win32::drop_target::mute(true); }
        let source: IDropSource = Source.into();
        let mut effect = DROPEFFECT_NONE;
        let result = DoDragDrop(&data, &source, effects(item), &mut effect);
        if !within_app { crate::platform::win32::drop_target::mute(false); }
        Some(if result == DRAGDROP_S_DROP { outcome(effect) } else { DragOutcome::Cancelled })
    }
}

/// Maximum distinct custom MIME clipboard formats registered per process via
/// `RegisterClipboardFormatW`. Windows' session USER atom table holds 16,384 entries and
/// registered clipboard formats cannot be freed until session end, so this cap prevents a
/// caller from exhausting the table (matching Chromium's bounded registration guard).
const MAX_CUSTOM_FORMATS: usize = 256;

struct FormatCache {
    by_name: rustc_hash::FxHashMap<String, u16>,
}

impl FormatCache {
    fn new() -> Self { Self { by_name: rustc_hash::FxHashMap::default() } }

    fn get_or_register(&mut self, mime: &str, register: impl FnOnce(&str) -> u16) -> Option<u16> {
        let key = mime.to_ascii_lowercase();
        if let Some(&id) = self.by_name.get(&key) {
            return Some(id);
        }
        if self.by_name.len() >= MAX_CUSTOM_FORMATS {
            return None;
        }
        let id = register(mime);
        if id == 0 {
            return None;
        }
        self.by_name.insert(key, id);
        Some(id)
    }
}

fn custom_format(mime: &str) -> Option<u16> {
    static CACHE: OnceLock<Mutex<FormatCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(FormatCache::new()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_or_register(mime, |name| {
        // SAFETY: `wide(name)` is a NUL-terminated UTF-16 string.
        unsafe { windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(wide(name).as_ptr()) as u16 }
    })
}

/// Builds and populates the shell `IDataObject` with built-in formats (files, PNG, text) and any
/// custom MIME entries from `DragItem::data`.
unsafe fn populate_data_object(item: &DragItem) -> Option<IDataObject> {
    // SAFETY: caller ensures COM/OLE is initialized on this thread; every HGLOBAL handed to
    // `set` is either owned by `data` on success or freed on error.
    unsafe {
        let data = data_object(item)?;
        if let Some(png) = item.png_data() { set(&data, png_format(), hglobal(png)?); }
        let text = item.plain_text().map(str::to_owned).or_else(|| item.paths().first().map(|f| native(f)));
        if let Some(text) = text {
            let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
            set(&data, CF_UNICODETEXT.0, hglobal(std::slice::from_raw_parts(units.as_ptr().cast(), units.len() * 2))?);
        }
        for (mime, bytes) in item.custom_data() {
            let Some(fmt) = custom_format(mime) else { continue };
            let Some(medium) = hglobal(bytes) else { continue };
            set(&data, fmt, medium);
        }
        Some(data)
    }
}

/// The shell's data object for the files (their folder's items, when they share one), else an
/// empty one; the shell's takes any format `SetData` gives it, which the drag image needs.
unsafe fn data_object(item: &DragItem) -> Option<IDataObject> {
    // SAFETY: each ID list made here is freed before returning; the data object copies them.
    unsafe {
        let full: Vec<*mut ITEMIDLIST> = item.paths().iter().map(|f| ILCreateFromPathW(PCWSTR(wide(&native(f)).as_ptr()))).collect();
        let shell = if !full.is_empty() && full.iter().all(|p| !p.is_null()) {
            let parent = ILClone(full[0]);
            let ok = !parent.is_null() && ILRemoveLastID(Some(parent)).as_bool() && full.iter().all(|&f| ILIsParent(parent, f, true).as_bool());
            let children: Vec<*const ITEMIDLIST> = full.iter().map(|&f| ILFindLastID(f) as *const _).collect();
            let data = ok.then(|| SHCreateDataObject::<_, IDataObject>(Some(parent), Some(&children), None).ok()).flatten();
            if !parent.is_null() { ILFree(Some(parent)); }
            data
        } else { None };
        for f in full.into_iter().filter(|p| !p.is_null()) { ILFree(Some(f)); }
        if let Some(data) = shell { return Some(data); }
        let data = SHCreateDataObject::<_, IDataObject>(None, None, None).ok()?;
        if !item.paths().is_empty() { set(&data, CF_HDROP.0, drop_files(item)?); }
        Some(data)
    }
}

/// A `CF_HDROP` medium: `DROPFILES` and the paths, each NUL-terminated, then one more NUL.
fn drop_files(item: &DragItem) -> Option<HGLOBAL> {
    let mut units: Vec<u16> = item.paths().iter().flat_map(|f| native(f).encode_utf16().chain([0]).collect::<Vec<_>>()).collect();
    units.push(0);
    // DROPFILES: pFiles, pt, fNC, fWide.
    let header: [u32; 5] = [20, 0, 0, 0, 1];
    let mut bytes: Vec<u8> = header.iter().flat_map(|v| v.to_le_bytes()).collect();
    bytes.extend(units.iter().flat_map(|u| u.to_le_bytes()));
    hglobal(&bytes)
}

fn hglobal(bytes: &[u8]) -> Option<HGLOBAL> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock};
    // SAFETY: a new zeroed movable block of at least 1 byte (Windows refuses to lock a 0-byte
    // GMEM_MOVEABLE handle), filled while locked and freed if locking fails.
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len().max(1));
        if h.is_null() { return None; }
        let p = GlobalLock(h);
        if p.is_null() {
            GlobalFree(h);
            return None;
        }
        if !bytes.is_empty() {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast(), bytes.len());
        }
        GlobalUnlock(h);
        Some(HGLOBAL(h))
    }
}

/// Hands `medium` to `data` as `format`; the data object owns it on success, otherwise it is
/// freed here so a refused format does not leak.
unsafe fn set(data: &IDataObject, format: u16, medium: HGLOBAL) {
    let format = FORMATETC { cfFormat: format, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    let stg = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: medium }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
    // SAFETY: a valid format and an HGLOBAL medium the call takes ownership of on success.
    if unsafe { data.SetData(&format, &stg, true) }.is_err() {
        // SAFETY: `SetData` failed, so ownership of `medium` remained with the caller.
        unsafe { windows_sys::Win32::Foundation::GlobalFree(medium.0); }
    }
}

fn png_format() -> u16 {
    static PNG: OnceLock<u16> = OnceLock::new();
    // SAFETY: the name is NUL-terminated.
    *PNG.get_or_init(|| unsafe { windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(wide("PNG").as_ptr()) as u16 })
}

/// The picture as the drag image manager takes it: a top-down 32-bit DIB of premultiplied BGRA,
/// with where the pointer sits in it.
fn drag_image(picture: &DragImage, start: &DragOutStart, scale: f32) -> Option<SHDRAGIMAGE> {
    let rgba = crate::image::png_rgba8(picture.png_data()).ok()?;
    let (w, h) = (rgba.width() as i32, rgba.height() as i32);
    let [fx, fy, fw, fh] = picture.frame(start.bounds, start.press, (w as f32 / scale, h as f32 / scale));
    let offset = POINT {
        x: (((start.press.0 - fx) / fw.max(1.0)) * w as f32).round().clamp(0.0, w as f32) as i32,
        y: (((start.press.1 - fy) / fh.max(1.0)) * h as f32).round().clamp(0.0, h as f32) as i32,
    };
    use windows_sys::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS};
    // SAFETY: a plain C struct, all zeroes valid.
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, biSizeImage: 0, biXPelsPerMeter: 0, biYPelsPerMeter: 0, biClrUsed: 0, biClrImportant: 0 };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: a DIB section of w x h 32-bit pixels, which `bits` then points into.
    let bitmap = unsafe { CreateDIBSection(std::ptr::null_mut(), &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0) };
    if bitmap.is_null() || bits.is_null() { return None; }
    // SAFETY: the section holds exactly w * h * 4 bytes.
    let out = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u8>(), (w * h * 4) as usize) };
    for (o, p) in out.chunks_exact_mut(4).zip(rgba.pixels()) {
        let a = u32::from(p[3]);
        let pre = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
        o.copy_from_slice(&[pre(p[2]), pre(p[1]), pre(p[0]), p[3]]);
    }
    Some(SHDRAGIMAGE { sizeDragImage: SIZE { cx: w, cy: h }, ptOffset: offset, hbmpDragImage: HBITMAP(bitmap), crColorKey: windows::Win32::Foundation::COLORREF(0xFFFF_FFFF) })
}

fn effects(item: &DragItem) -> DROPEFFECT {
    [(DragOperation::Copy, DROPEFFECT_COPY), (DragOperation::Move, DROPEFFECT_MOVE), (DragOperation::Link, DROPEFFECT_LINK)]
        .into_iter().filter(|&(op, _)| item.offers(op)).fold(DROPEFFECT_NONE, |m, (_, e)| m | e)
}

fn outcome(effect: DROPEFFECT) -> DragOutcome {
    if effect.0 & DROPEFFECT_COPY.0 != 0 { DragOutcome::Copied }
    else if effect.0 & DROPEFFECT_MOVE.0 != 0 { DragOutcome::Moved }
    else if effect.0 & DROPEFFECT_LINK.0 != 0 { DragOutcome::Linked }
    else { DragOutcome::Cancelled }
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain([0]).collect() }

/// A path as the shell parses it: backslashes.
fn native(path: &std::path::Path) -> String { path_text(path).replace('/', "\\") }

/// Ends the drag when the left button comes up, cancels it on Escape; the system's cursors.
#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() { return DRAGDROP_S_CANCEL; }
        if keys.0 & MK_LBUTTON.0 == 0 { return DRAGDROP_S_DROP; }
        S_OK
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT { DRAGDROP_S_USEDEFAULTCURSORS }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::DATADIR_GET;
    use windows::Win32::System::Ole::ReleaseStgMedium;

    #[test]
    fn format_cache_reuses_entries_and_caps_at_256() {
        let mut cache = FormatCache::new();
        let calls = Cell::new(0u16);
        let reg = |_: &str| {
            let n = calls.get() + 1;
            calls.set(n);
            0xC000 + n
        };
        let first = cache.get_or_register("text/uri-list", reg);
        assert_eq!(first, Some(0xC001));
        assert_eq!(cache.get_or_register("text/uri-list", reg), Some(0xC001));
        assert_eq!(cache.get_or_register("TEXT/URI-LIST", reg), Some(0xC001));
        assert_eq!(calls.get(), 1, "exact and case-variant lookups must reuse the cached format");

        // A failed registration (0) is not cached and does not consume a slot.
        assert_eq!(cache.get_or_register("application/x-fail", |_| 0), None);
        assert_eq!(cache.by_name.len(), 1);

        for i in 1..MAX_CUSTOM_FORMATS {
            let name = format!("application/x-rune-cap-{i}");
            assert!(cache.get_or_register(&name, reg).is_some());
        }
        assert_eq!(cache.by_name.len(), MAX_CUSTOM_FORMATS);
        let before = calls.get();
        assert_eq!(cache.get_or_register("application/x-rune-overflow", reg), None);
        assert_eq!(calls.get(), before, "overflow must not call RegisterClipboardFormatW");
        assert_eq!(cache.get_or_register("text/uri-list", reg), Some(0xC001), "existing entries stay reachable at the cap");
    }

    #[test]
    fn populate_data_object_enumerates_and_reads_custom_formats_in_process() {
        let item = DragItem::new()
            .data("text/uri-list", "sftp://host/file.txt\r\n")
            .data("x-special/gnome-copied-files", "copy\nsftp://host/file.txt")
            .data("application/x-kde-cutselection", b"0".as_slice())
            .data("application/x-rune-test-id", b"drag-42".as_slice());

        // SAFETY: in-process COM/OLE data object test (no window, no DoDragDrop, no clipboard).
        unsafe {
            OleInitialize(None).ok();
            let data = populate_data_object(&item).expect("IDataObject");

            let enumerator = data.EnumFormatEtc(DATADIR_GET.0 as u32).expect("EnumFormatEtc");
            let mut enumerated = Vec::new();
            let mut slot = [FORMATETC::default(); 1];
            let mut fetched = 0u32;
            while enumerator.Next(&mut slot, Some(&mut fetched)) == S_OK && fetched == 1 {
                enumerated.push(slot[0].cfFormat);
            }

            for (mime, expected) in [
                ("text/uri-list", b"sftp://host/file.txt\r\n".as_slice()),
                ("x-special/gnome-copied-files", b"copy\nsftp://host/file.txt".as_slice()),
                ("application/x-kde-cutselection", b"0".as_slice()),
                ("application/x-rune-test-id", b"drag-42".as_slice()),
            ] {
                let fmt = custom_format(mime).expect("registered clipboard format");
                assert!(enumerated.contains(&fmt), "EnumFormatEtc missing format {fmt} for {mime}; got {enumerated:?}");
                let query = FORMATETC {
                    cfFormat: fmt,
                    ptd: std::ptr::null_mut(),
                    dwAspect: DVASPECT_CONTENT.0,
                    lindex: -1,
                    tymed: TYMED_HGLOBAL.0 as u32,
                };
                let mut stg = data.GetData(&query).expect("GetData");
                assert_eq!(stg.tymed, TYMED_HGLOBAL.0 as u32);
                let h = stg.u.hGlobal.0;
                let size = windows_sys::Win32::System::Memory::GlobalSize(h);
                let ptr = windows_sys::Win32::System::Memory::GlobalLock(h);
                assert!(!ptr.is_null());
                let got = std::slice::from_raw_parts(ptr.cast::<u8>(), size).to_vec();
                windows_sys::Win32::System::Memory::GlobalUnlock(h);
                ReleaseStgMedium(&mut stg);
                assert_eq!(got, expected, "payload mismatch for {mime}");
            }
        }
    }

    #[test]
    fn drag_image_hotspot_sets_shdragimage_offset() {
        let img = DragImage::rgba(48, 48, &[255; 48 * 48 * 4]).unwrap().size(48.0, 48.0).hotspot(12.0, 18.0);
        let start = DragOutStart {
            path: "row".into(),
            item: DragItem::new().image(img.clone()),
            rect: [0.0, 0.0, 100.0, 30.0],
            bounds: [0.0, 0.0, 100.0, 30.0],
            press: (50.0, 60.0),
        };
        let shdi = drag_image(&img, &start, 1.0).expect("SHDRAGIMAGE");
        assert_eq!((shdi.ptOffset.x, shdi.ptOffset.y), (12, 18));
        // SAFETY: `shdi.hbmpDragImage` was created by `CreateDIBSection` above and is not handed to the shell.
        unsafe { windows_sys::Win32::Graphics::Gdi::DeleteObject(shdi.hbmpDragImage.0); }
    }
}
