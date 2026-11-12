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

use std::cell::RefCell;
use std::sync::OnceLock;

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
    start: DragOutStart,
    picture: Option<DragImage>,
    done: Done,
    scale: f32,
}

thread_local! {
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

const SUBCLASS: usize = 0x5255_4e47;

fn message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: the name is NUL-terminated.
    *MESSAGE.get_or_init(|| unsafe { windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(wide("Rune::DragOut").as_ptr()) })
}

pub(super) fn begin(window: &winit::window::Window, start: DragOutStart, picture: Option<DragImage>, done: Done) {
    let Some(hwnd) = crate::platform::win32::hwnd(window) else { return done(DragOutcome::Cancelled) };
    let pending = Pending { start, picture, done, scale: window.scale_factor() as f32 };
    // One drag at a time: one that never got to run ends now.
    if let Some(old) = PENDING.with(|p| p.borrow_mut().replace(pending)) { (old.done)(DragOutcome::Cancelled); }
    use windows_sys::Win32::UI::Shell::{RemoveWindowSubclass, SetWindowSubclass};
    // SAFETY: Win32 calls on a live window this thread owns; the subclass keeps no pointer.
    let posted = unsafe {
        SetWindowSubclass(hwnd, Some(procedure), SUBCLASS, 0) != 0
            && (windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd, message(), 0, 0) != 0
                || { RemoveWindowSubclass(hwnd, Some(procedure), SUBCLASS); false })
    };
    if !posted { run(); }
}

unsafe extern "system" fn procedure(hwnd: windows_sys::Win32::Foundation::HWND, msg: u32, w: usize, l: isize, _: usize, _: usize) -> isize {
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY;
    if msg == message() || msg == WM_NCDESTROY {
        // SAFETY: removes this subclass from the window it was set on.
        unsafe { RemoveWindowSubclass(hwnd, Some(procedure), SUBCLASS) };
        if msg == message() {
            run();
            return 0;
        }
        if let Some(p) = PENDING.with(|p| p.borrow_mut().take()) { (p.done)(DragOutcome::Cancelled); }
    }
    // SAFETY: passes every other message on.
    unsafe { DefSubclassProc(hwnd, msg, w, l) }
}

/// Runs the pending session to its end and reports how it ended.
fn run() {
    let Some(p) = PENDING.with(|p| p.borrow_mut().take()) else { return };
    let outcome = session(&p).unwrap_or(DragOutcome::Cancelled);
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
        let _ = OleInitialize(None);
        let data = data_object(item)?;
        if let Some(png) = item.png_data() { set(&data, png_format(), hglobal(png)?); }
        let text = item.plain_text().map(str::to_owned).or_else(|| item.paths().first().map(|f| native(f)));
        if let Some(text) = text {
            let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
            set(&data, CF_UNICODETEXT.0, hglobal(std::slice::from_raw_parts(units.as_ptr().cast(), units.len() * 2))?);
        }
        if let Ok(helper) = CoCreateInstance::<_, IDragSourceHelper>(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) {
            let picture = p.picture.as_ref().and_then(|pic| drag_image(pic, &p.start, p.scale));
            match picture {
                Some(shdi) => if helper.InitializeFromBitmap(&shdi, &data).is_err() { windows_sys::Win32::Graphics::Gdi::DeleteObject(shdi.hbmpDragImage.0); },
                // The shell's own picture of the files, under the pointer.
                None if !item.paths().is_empty() => { let _ = helper.InitializeFromWindow(None, None, &data); }
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
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    // SAFETY: a new movable block of the bytes' size, filled while locked.
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
        if h.is_null() { return None; }
        let p = GlobalLock(h);
        if p.is_null() { return None; }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast(), bytes.len());
        GlobalUnlock(h);
        Some(HGLOBAL(h))
    }
}

/// Hands `medium` to `data` as `format`; the data object owns it from then on.
unsafe fn set(data: &IDataObject, format: u16, medium: HGLOBAL) {
    let format = FORMATETC { cfFormat: format, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    let medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: medium }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
    // SAFETY: a valid format and an HGLOBAL medium the call takes ownership of.
    unsafe { let _ = data.SetData(&format, &medium, true); }
}

fn png_format() -> u16 {
    static PNG: OnceLock<u16> = OnceLock::new();
    // SAFETY: the name is NUL-terminated.
    *PNG.get_or_init(|| unsafe { windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(wide("PNG").as_ptr()) as u16 })
}

/// The picture as the drag image manager takes it: a top-down 32-bit DIB of premultiplied BGRA,
/// with where the pointer sits in it.
fn drag_image(picture: &DragImage, start: &DragOutStart, scale: f32) -> Option<SHDRAGIMAGE> {
    let rgba = image::load_from_memory_with_format(picture.png_data(), image::ImageFormat::Png).ok()?.to_rgba8();
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
