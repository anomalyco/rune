//! Files dragged in from other apps, through Rune's own OLE drop target. winit's (its
//! `with_drag_and_drop`) initialises OLE and registers its target inside `CreateWindowExW`, which
//! costs a new window about 8,400 µs and six shell DLLs before its first frame; Rune creates
//! windows without it and registers this target once the window has shown (`install`), so a
//! drag can only start after a frame a person has seen anyway. The steps reach the host as
//! macOS's do (`platform::macos::drop`): with the pointer's place in window points, drained each
//! turn by `Host::flush_files`.
//!
//! Each OLE step forwards to the shell's `IDropTargetHelper` (`CLSID_DragDropHelper`), created
//! lazily on the first drag so window creation stays free, which keeps the drag image visible
//! over Rune windows (both drags from Explorer and `DragItem::within_app`). Apps may also
//! choose Copy/Move/Link/None and set a `CFSTR_DROPDESCRIPTION` label via `El::drop_feedback`
//! or `Tx::set_drop_operation` / `Tx::set_drop_description`, applied in `set_feedback`.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use windows::Win32::Foundation::{HGLOBAL, HWND, POINT, POINTL};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM,
    STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
    IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium,
    RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{
    CFSTR_DROPDESCRIPTION, CLSID_DragDropHelper, DROPDESCRIPTION, DROPIMAGE_COPY,
    DROPIMAGE_INVALID, DROPIMAGE_LABEL, DROPIMAGE_LINK, DROPIMAGE_MOVE, DROPIMAGE_NONE,
    DROPIMAGE_WARNING, DROPIMAGETYPE, IDropTargetHelper,
};
use windows::core::{Ref, implement};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::drag_out::{DragOperation, DragOperations};
use crate::el::{DropDescription, DropDescriptionKind, DropFeedback};
use crate::window::FileStep;

/// Win32 OLE `MK_ALT` modifier flag (`oleidl.h`).
const MK_ALT: u32 = 0x0020;

/// What a registered window's target has seen since its host last looked.
struct Dest {
    /// Identifies one registration so an older guard or COM object cannot touch a later window
    /// that reused the same HWND.
    token: u64,
    steps: Vec<FileStep>,
    /// Lazily created shell drag-image helper (`CLSID_DragDropHelper`).
    helper: Option<IDropTargetHelper>,
    /// `true` while `helper.DragEnter` succeeded and neither `DragLeave` nor `Drop` has run.
    helper_active: bool,
    /// Data object retained from `DragEnter` so `set_feedback`, `DragOver` and `DragLeave` can
    /// update or clear `CFSTR_DROPDESCRIPTION`.
    data: Option<IDataObject>,
    /// The drag over it carries files, so it may drop them.
    files: bool,
    /// Latest screen point seen in `DragEnter` / `DragOver`.
    last_pt: POINT,
    /// Latest allowed `DROPEFFECT` mask passed by OLE on entry.
    last_allowed: DROPEFFECT,
    /// Latest `DROPEFFECT` reported to OLE and `IDropTargetHelper`.
    last_effect: DROPEFFECT,
    /// Latest `(at, mods, allowed)` forwarded to the host, to coalesce stationary `DragOver`s.
    last_pushed: Option<((f32, f32), crate::Mods, DragOperations)>,
    /// Latest `DropFeedback` pushed from the Rune frame via `set_feedback`.
    feedback: Option<DropFeedback>,
    /// Whether this target wrote `CFSTR_DROPDESCRIPTION` on `data` during the active drag.
    wrote_description: bool,
    /// The `DropDescription` last written to `data`.
    applied_description: Option<DropDescription>,
}

impl Dest {
    fn new(token: u64) -> Self {
        Self {
            token,
            steps: Vec::new(),
            helper: None,
            helper_active: false,
            data: None,
            files: false,
            last_pt: POINT::default(),
            last_allowed: DROPEFFECT_NONE,
            last_effect: DROPEFFECT_NONE,
            last_pushed: None,
            feedback: None,
            wrote_description: false,
            applied_description: None,
        }
    }
}

thread_local! {
    /// By HWND: drop targets live on the UI thread that registered them, as OLE calls them there.
    static DESTS: RefCell<HashMap<isize, Dest>> = RefCell::new(HashMap::new());
    static NEXT_TOKEN: Cell<u64> = const { Cell::new(1) };
    /// A drag out of this app runs that its windows don't take (`DragItem::within_app` off).
    static MUTED: Cell<bool> = const { Cell::new(false) };
}

/// Revokes the window's OLE drop target and prunes its `DESTS` entry when dropped, before the
/// window's HWND is destroyed, so a later window that reuses the same handle registers cleanly
/// and inherits no stale drag state.
pub(crate) struct DropTarget {
    hwnd: isize,
    token: u64,
}

impl Drop for DropTarget {
    fn drop(&mut self) {
        uninstall_hwnd(self.hwnd, self.token);
    }
}

/// While a drag out of the app runs, its windows refuse it, as they refuse it on macOS, so a
/// drag let go over the app ends cancelled instead of handing the app its own files.
pub(crate) fn mute(on: bool) {
    MUTED.with(|m| m.set(on));
}

fn hwnd(window: &winit::window::Window) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut _)),
        _ => None,
    }
}

fn claim(hwnd: isize) -> Option<u64> {
    let token = NEXT_TOKEN.with(|n| {
        let next = n.get();
        n.set(next.wrapping_add(1).max(1));
        next
    });
    DESTS.with(|d| {
        let mut d = d.borrow_mut();
        if d.contains_key(&hwnd) {
            return None;
        }
        d.insert(hwnd, Dest::new(token));
        Some(token)
    })
}

fn forget(hwnd: isize, token: u64) -> Option<Dest> {
    DESTS.try_with(|d| {
        let mut d = d.borrow_mut();
        if d.get(&hwnd).is_some_and(|dest| dest.token == token) {
            d.remove(&hwnd)
        } else {
            None
        }
    }).ok().flatten()
}

fn uninstall_hwnd(hwnd: isize, token: u64) {
    let Some(mut dest) = forget(hwnd, token) else {
        return;
    };
    if dest.wrote_description && let Some(data) = dest.data.as_ref() {
        write_drop_description(data, None);
    }
    if dest.helper_active && let Some(helper) = dest.helper.as_ref() {
        // SAFETY: STA COM call on the UI thread; `DESTS` is not borrowed.
        unsafe { helper.DragLeave().ok(); }
    }
    dest.data = None;
    dest.helper = None;
    // SAFETY: `hwnd` was registered by `install_hwnd` on this UI thread and `Native` drops before
    // `Win::window`, so `RevokeDragDrop` runs while the window is still live (and OLE ignores an
    // already-destroyed handle).
    unsafe {
        RevokeDragDrop(HWND(hwnd as *mut _)).ok();
    }
}

/// Registers the window's drop target, once; after its first frame has shown. The returned guard
/// revokes the target and prunes `DESTS` when the window goes.
pub(crate) fn install(window: &winit::window::Window) -> Option<DropTarget> {
    let hwnd = hwnd(window)?;
    install_hwnd(hwnd)
}

fn install_hwnd(hwnd: HWND) -> Option<DropTarget> {
    let key = hwnd.0 as isize;
    let token = claim(key)?;
    // SAFETY: the UI thread, which winit leaves without COM (drag and drop off), becomes OLE's
    // single-threaded apartment; `DropTarget` calls `RevokeDragDrop` before the window is
    // destroyed so OLE releases its reference to `target`.
    unsafe {
        if OleInitialize(None).is_err() {
            forget(key, token);
            return None;
        }
        let target: IDropTarget = Target { hwnd: key, token }.into();
        if RegisterDragDrop(hwnd, &target).is_err() {
            forget(key, token);
            return None;
        }
    }
    Some(DropTarget { hwnd: key, token })
}

/// What the window's target saw since the last call.
pub(crate) fn take(window: &winit::window::Window) -> Vec<FileStep> {
    let Some(hwnd) = hwnd(window) else { return Vec::new() };
    take_hwnd(hwnd.0 as isize)
}

fn take_hwnd(hwnd: isize) -> Vec<FileStep> {
    DESTS.with(|d| d.borrow_mut().get_mut(&hwnd).map(|d| std::mem::take(&mut d.steps)).unwrap_or_default())
}

/// Applies the latest evaluated [`DropFeedback`] from the runtime frame to the window's active
/// OLE drag target, immediately updating `CFSTR_DROPDESCRIPTION` and `IDropTargetHelper` when
/// a drag is in flight over `window`.
pub(crate) fn set_feedback(window: &winit::window::Window, feedback: Option<&DropFeedback>) {
    let Some(hwnd) = hwnd(window) else { return };
    set_feedback_hwnd(hwnd.0 as isize, feedback);
}

fn set_feedback_hwnd(hwnd: isize, feedback: Option<&DropFeedback>) {
    let update = DESTS.with(|d| {
        let mut d = d.borrow_mut();
        let Some(dest) = d.get_mut(&hwnd) else { return None };
        dest.feedback = feedback.cloned();
        if !dest.helper_active {
            return None;
        }
        let new_effect = resolve_effect(dest.files, dest.last_allowed, dest.feedback.as_ref());
        let desired = if dest.files {
            dest.feedback.as_ref().and_then(|f| f.drop_description()).cloned()
        } else {
            None
        };
        let desc_changed = desired != dest.applied_description && (desired.is_some() || dest.wrote_description);
        let effect_changed = new_effect != dest.last_effect;
        if !desc_changed && !effect_changed {
            return None;
        }
        dest.last_effect = new_effect;
        Some((
            dest.token,
            dest.data.clone(),
            dest.helper.clone(),
            dest.last_pt,
            new_effect,
            desired,
            desc_changed,
        ))
    });
    let Some((token, data, helper, last_pt, new_effect, desired, desc_changed)) = update else { return };
    if desc_changed && let Some(data) = data.as_ref() {
        apply_description(hwnd, token, data, desired.as_ref());
    }
    if let Some(helper) = helper {
        // SAFETY: STA COM call on the UI thread; `DESTS` is not borrowed.
        unsafe { helper.DragOver(&last_pt, new_effect).ok(); }
    }
}

fn push_step(hwnd: isize, token: u64, step: FileStep) -> bool {
    DESTS.with(|d| {
        let mut d = d.borrow_mut();
        let Some(dest) = d.get_mut(&hwnd).filter(|dest| dest.token == token) else { return false };
        if matches!(step, FileStep::Move(_) | FileStep::MoveWith { .. })
            && matches!(dest.steps.last(), Some(FileStep::Move(_) | FileStep::MoveWith { .. }))
        {
            dest.steps.pop();
        }
        dest.steps.push(step);
        true
    })
}

fn push(hwnd: isize, token: u64, step: FileStep) {
    if !push_step(hwnd, token, step) {
        return;
    }
    // Turns the loop, whose `about_to_wait` drains the step (`Host::flush_files`).
    // SAFETY: a message with no meaning to a window we own.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
            hwnd as _,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_NULL,
            0,
            0,
        );
    }
}

#[cfg(test)]
fn set_files_for(hwnd: isize, token: u64, files: bool) {
    DESTS.with(|d| {
        if let Some(dest) = d.borrow_mut().get_mut(&hwnd).filter(|dest| dest.token == token) {
            dest.files = files;
        }
    });
}

#[cfg(test)]
fn files_for(hwnd: isize, token: u64) -> bool {
    DESTS.with(|d| d.borrow().get(&hwnd).is_some_and(|d| d.token == token && d.files))
}

#[cfg(test)]
fn set_helper_for(hwnd: isize, token: u64, helper: IDropTargetHelper) {
    DESTS.with(|d| {
        if let Some(dest) = d.borrow_mut().get_mut(&hwnd).filter(|dest| dest.token == token) {
            dest.helper = Some(helper);
        }
    });
}

fn ensure_helper(hwnd: isize, token: u64) -> Option<IDropTargetHelper> {
    let existing = DESTS.with(|d| {
        let d = d.borrow();
        let dest = d.get(&hwnd).filter(|dest| dest.token == token)?;
        Some(dest.helper.clone())
    })?;
    if let Some(helper) = existing {
        return Some(helper);
    }
    // SAFETY: OLE/COM is initialized on this UI thread (by `install_hwnd`); `DESTS` is not borrowed.
    let created = unsafe {
        CoCreateInstance::<_, IDropTargetHelper>(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok()
    }?;
    DESTS.with(|d| {
        if let Some(dest) = d.borrow_mut().get_mut(&hwnd).filter(|dest| dest.token == token) {
            if dest.helper.is_none() {
                dest.helper = Some(created.clone());
            }
            dest.helper.clone()
        } else {
            None
        }
    })
}

fn apply_description(hwnd: isize, token: u64, data: &IDataObject, desired: Option<&DropDescription>) {
    let ok = write_drop_description(data, desired);
    DESTS.with(|d| {
        if let Some(dest) = d.borrow_mut().get_mut(&hwnd).filter(|dest| dest.token == token) {
            match desired {
                Some(desc) if ok => {
                    dest.wrote_description = true;
                    dest.applied_description = Some(desc.clone());
                }
                None => {
                    dest.wrote_description = false;
                    dest.applied_description = None;
                }
                Some(_) => {}
            }
        }
    });
}

/// Converts OLE `grfKeyState` (`MK_SHIFT`, `MK_CONTROL`, `MK_ALT`) into Rune's `Mods`.
/// On Windows `cmd` mirrors `ctrl` per `Mods`'s cross-platform convention.
fn mods_from_key_state(keys: MODIFIERKEYS_FLAGS) -> crate::Mods {
    let ctrl = keys.0 & MK_CONTROL.0 != 0;
    crate::Mods {
        shift: keys.0 & MK_SHIFT.0 != 0,
        cmd: ctrl,
        alt: keys.0 & MK_ALT != 0,
        ctrl,
        logo: false,
    }
}

fn operations_from_effect(effect: DROPEFFECT) -> DragOperations {
    let mut ops = DragOperations::NONE;
    if effect.0 & DROPEFFECT_COPY.0 != 0 { ops = ops.with(DragOperation::Copy); }
    if effect.0 & DROPEFFECT_MOVE.0 != 0 { ops = ops.with(DragOperation::Move); }
    if effect.0 & DROPEFFECT_LINK.0 != 0 { ops = ops.with(DragOperation::Link); }
    ops
}

fn operation_from_effect(effect: DROPEFFECT) -> Option<DragOperation> {
    if effect.0 & DROPEFFECT_COPY.0 != 0 { Some(DragOperation::Copy) }
    else if effect.0 & DROPEFFECT_MOVE.0 != 0 { Some(DragOperation::Move) }
    else if effect.0 & DROPEFFECT_LINK.0 != 0 { Some(DragOperation::Link) }
    else { None }
}

fn effect_for_operation(op: Option<DragOperation>, allowed: DROPEFFECT) -> DROPEFFECT {
    let want = match op {
        Some(DragOperation::Copy) => DROPEFFECT_COPY,
        Some(DragOperation::Move) => DROPEFFECT_MOVE,
        Some(DragOperation::Link) => DROPEFFECT_LINK,
        None => DROPEFFECT_NONE,
    };
    DROPEFFECT(want.0 & allowed.0)
}

/// Resolves the OLE `DROPEFFECT` to return to the source and pass to `IDropTargetHelper`:
/// - No files (or muted drag): always `DROPEFFECT_NONE`.
/// - Evaluated `feedback`: its operation intersected with `allowed`.
/// - Before the first hover frame evaluates (`feedback == None`): copy-only default intersected
///   with `allowed` so existing copy-only targets keep unchanged defaults.
fn resolve_effect(has_files: bool, allowed: DROPEFFECT, feedback: Option<&DropFeedback>) -> DROPEFFECT {
    if !has_files {
        return DROPEFFECT_NONE;
    }
    match feedback {
        Some(fb) => effect_for_operation(fb.operation(), allowed),
        None => DROPEFFECT(DROPEFFECT_COPY.0 & allowed.0),
    }
}

fn read_allowed(out: *mut DROPEFFECT) -> DROPEFFECT {
    if out.is_null() {
        DROPEFFECT_NONE
    } else {
        // SAFETY: OLE passes the allowed drop-effect bitmask in `*pdwEffect` on entry.
        unsafe { *out }
    }
}

fn write_effect(out: *mut DROPEFFECT, effect: DROPEFFECT) {
    if !out.is_null() {
        // SAFETY: OLE's out parameter for the effect the target offers.
        unsafe { *out = effect; }
    }
}

fn drop_image_type(kind: DropDescriptionKind) -> DROPIMAGETYPE {
    match kind {
        DropDescriptionKind::None => DROPIMAGE_NONE,
        DropDescriptionKind::Copy => DROPIMAGE_COPY,
        DropDescriptionKind::Move => DROPIMAGE_MOVE,
        DropDescriptionKind::Link => DROPIMAGE_LINK,
        DropDescriptionKind::Label => DROPIMAGE_LABEL,
        DropDescriptionKind::Warning => DROPIMAGE_WARNING,
    }
}

/// Copies `text` into a NUL-terminated fixed UTF-16 buffer, stripping interior NULs and truncating
/// without splitting surrogate pairs.
fn write_wide_fixed<const N: usize>(text: &str, out: &mut [u16; N]) {
    out.fill(0);
    let cap = N.saturating_sub(1);
    if cap == 0 { return; }
    let mut len = 0usize;
    for ch in text.chars() {
        if ch == '\0' { continue; }
        let mut units = [0u16; 2];
        let encoded = ch.encode_utf16(&mut units);
        if len + encoded.len() > cap { break; }
        out[len..len + encoded.len()].copy_from_slice(encoded);
        len += encoded.len();
    }
}

fn drop_description_format() -> u16 {
    static FORMAT: OnceLock<u16> = OnceLock::new();
    // SAFETY: `CFSTR_DROPDESCRIPTION` is a static NUL-terminated UTF-16 string ("DropDescription").
    *FORMAT.get_or_init(|| unsafe {
        windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(CFSTR_DROPDESCRIPTION.as_ptr()) as u16
    })
}

fn build_drop_description(desc: Option<&DropDescription>) -> DROPDESCRIPTION {
    let mut raw = DROPDESCRIPTION::default();
    match desc {
        Some(d) => {
            let mut message = [0u16; 260];
            let mut insert = [0u16; 260];
            write_wide_fixed(d.message(), &mut message);
            write_wide_fixed(d.insert_text(), &mut insert);
            raw.r#type = drop_image_type(d.kind());
            raw.szMessage = message;
            raw.szInsert = insert;
        }
        None => {
            raw.r#type = DROPIMAGE_INVALID;
        }
    }
    raw
}

/// Writes (or clears with `DROPIMAGE_INVALID`) `CFSTR_DROPDESCRIPTION` on `data`. Returns `true`
/// if `data.SetData` accepted it; frees the HGLOBAL on failure.
fn write_drop_description(data: &IDataObject, desc: Option<&DropDescription>) -> bool {
    let fmt = drop_description_format();
    if fmt == 0 { return false; }
    let raw = build_drop_description(desc);
    let size = std::mem::size_of::<DROPDESCRIPTION>();
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    // SAFETY: allocates a movable HGLOBAL holding `DROPDESCRIPTION`, populates it while locked,
    // and transfers ownership to `data.SetData` with `fRelease = true` (freeing on error).
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, size);
        if h.is_null() { return false; }
        let ptr = GlobalLock(h);
        if ptr.is_null() {
            GlobalFree(h);
            return false;
        }
        std::ptr::write(ptr.cast::<DROPDESCRIPTION>(), raw);
        GlobalUnlock(h);
        let format = FORMATETC {
            cfFormat: fmt,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let stg = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: HGLOBAL(h) },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        };
        if data.SetData(&format, &stg, true).is_err() {
            GlobalFree(h);
            return false;
        }
        true
    }
}

#[implement(IDropTarget)]
struct Target {
    hwnd: isize,
    token: u64,
}

impl Target {
    /// `pt` (screen pixels) in the window's points.
    fn at(&self, pt: &POINTL) -> (f32, f32) {
        let hwnd = self.hwnd as windows_sys::Win32::Foundation::HWND;
        let mut p = windows_sys::Win32::Foundation::POINT { x: pt.x, y: pt.y };
        // SAFETY: our own window, on its thread.
        let scale = unsafe {
            windows_sys::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
            windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) as f32 / 96.0
        };
        let scale = if scale > 0.0 { scale } else { 1.0 };
        (p.x as f32 / scale, p.y as f32 / scale)
    }
}

/// The paths `data` carries as a shell file list (`CF_HDROP`); none for any other drag, or
/// while the app's own drag is muted.
fn paths(data: Option<&IDataObject>) -> Vec<PathBuf> {
    let Some(data) = data.filter(|_| !MUTED.with(|m| m.get())) else { return Vec::new() };
    let format = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    // SAFETY: a `CF_HDROP` medium holds an HDROP in its HGLOBAL, read and released here.
    unsafe {
        let Ok(mut medium) = data.GetData(&format) else { return Vec::new() };
        use windows_sys::Win32::UI::Shell::DragQueryFileW;
        let drop = medium.u.hGlobal.0 as windows_sys::Win32::UI::Shell::HDROP;
        let count = DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0);
        let out = (0..count).filter_map(|i| {
            let len = DragQueryFileW(drop, i, std::ptr::null_mut(), 0) as usize;
            let mut buf = vec![0u16; len + 1];
            (DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32) as usize == len).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
        }).collect();
        ReleaseStgMedium(&mut medium);
        out
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, out: *mut DROPEFFECT) -> windows::core::Result<()> {
        let allowed = read_allowed(out);
        let mods = mods_from_key_state(keys);
        let allowed_ops = operations_from_effect(allowed);
        let files = paths(data.as_ref());
        let has_files = !files.is_empty();
        let chosen = resolve_effect(has_files, allowed, None);
        write_effect(out, chosen);

        let point = POINT { x: pt.x, y: pt.y };
        let helper = ensure_helper(self.hwnd, self.token);
        let at = has_files.then(|| self.at(pt));
        DESTS.with(|d| {
            if let Some(dest) = d.borrow_mut().get_mut(&self.hwnd).filter(|dest| dest.token == self.token) {
                dest.files = has_files;
                dest.helper_active = false;
                dest.data = data.as_ref().cloned();
                dest.last_pt = point;
                dest.last_allowed = allowed;
                dest.last_effect = chosen;
                dest.feedback = None;
                dest.wrote_description = false;
                dest.applied_description = None;
                dest.last_pushed = at.map(|p| (p, mods, allowed_ops));
            }
        });

        if let (Some(helper), Some(data)) = (helper, data.as_ref()) {
            // SAFETY: STA COM call on the UI thread with live HWND and IDataObject; `DESTS` is not borrowed.
            if unsafe { helper.DragEnter(HWND(self.hwnd as *mut _), data, &point, chosen) }.is_ok() {
                DESTS.with(|d| {
                    if let Some(dest) = d.borrow_mut().get_mut(&self.hwnd).filter(|dest| dest.token == self.token) {
                        dest.helper_active = true;
                    }
                });
            }
        }

        if let Some(at) = at {
            push(self.hwnd, self.token, FileStep::EnterWith { paths: files, at, mods, allowed: allowed_ops });
        }
        Ok(())
    }

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, out: *mut DROPEFFECT) -> windows::core::Result<()> {
        let allowed = read_allowed(out);
        let mods = mods_from_key_state(keys);
        let allowed_ops = operations_from_effect(allowed);
        let point = POINT { x: pt.x, y: pt.y };

        let state = DESTS.with(|d| {
            let mut d = d.borrow_mut();
            let Some(dest) = d.get_mut(&self.hwnd).filter(|dest| dest.token == self.token) else { return None };
            let has_files = dest.files;
            let chosen = resolve_effect(has_files, allowed, dest.feedback.as_ref());
            dest.last_pt = point;
            dest.last_allowed = allowed;
            dest.last_effect = chosen;
            let desired = if has_files {
                dest.feedback.as_ref().and_then(|f| f.drop_description()).cloned()
            } else {
                None
            };
            let desc_changed = desired != dest.applied_description && (desired.is_some() || dest.wrote_description);
            let helper = dest.helper_active.then(|| dest.helper.clone()).flatten();
            let data = dest.data.clone();
            Some((has_files, chosen, helper, data, desired, desc_changed))
        });

        let Some((has_files, chosen, helper, data, desired, desc_changed)) = state else {
            write_effect(out, DROPEFFECT_NONE);
            return Ok(());
        };
        write_effect(out, chosen);

        if desc_changed && let Some(data) = data.as_ref() {
            apply_description(self.hwnd, self.token, data, desired.as_ref());
        }
        if let Some(helper) = helper {
            // SAFETY: STA COM call on the UI thread; `DESTS` is not borrowed.
            unsafe { helper.DragOver(&point, chosen).ok(); }
        }

        if has_files {
            let at = self.at(pt);
            let next_key = (at, mods, allowed_ops);
            let changed = DESTS.with(|d| {
                let mut d = d.borrow_mut();
                let Some(dest) = d.get_mut(&self.hwnd).filter(|dest| dest.token == self.token) else { return false };
                if dest.last_pushed == Some(next_key) {
                    return false;
                }
                dest.last_pushed = Some(next_key);
                true
            });
            if changed {
                push(self.hwnd, self.token, FileStep::MoveWith { at, mods, allowed: allowed_ops });
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        let state = DESTS.with(|d| {
            let mut d = d.borrow_mut();
            let Some(dest) = d.get_mut(&self.hwnd).filter(|dest| dest.token == self.token) else { return None };
            let was_files = std::mem::replace(&mut dest.files, false);
            let helper = std::mem::replace(&mut dest.helper_active, false)
                .then(|| dest.helper.clone())
                .flatten();
            let data = dest.data.take();
            let wrote = std::mem::replace(&mut dest.wrote_description, false);
            dest.applied_description = None;
            dest.feedback = None;
            dest.last_pushed = None;
            dest.last_allowed = DROPEFFECT_NONE;
            dest.last_effect = DROPEFFECT_NONE;
            Some((was_files, helper, data, wrote))
        });

        let Some((was_files, helper, data, wrote)) = state else { return Ok(()) };
        if wrote && let Some(data) = data.as_ref() {
            write_drop_description(data, None);
        }
        if let Some(helper) = helper {
            // SAFETY: STA COM call on the UI thread; `DESTS` is not borrowed.
            unsafe { helper.DragLeave().ok(); }
        }
        drop(data);
        if was_files {
            push(self.hwnd, self.token, FileStep::Leave);
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, out: *mut DROPEFFECT) -> windows::core::Result<()> {
        let allowed = read_allowed(out);
        let mods = mods_from_key_state(keys);
        let allowed_ops = operations_from_effect(allowed);
        let files = paths(data.as_ref());
        let point = POINT { x: pt.x, y: pt.y };

        let state = DESTS.with(|d| {
            let mut d = d.borrow_mut();
            let Some(dest) = d.get_mut(&self.hwnd).filter(|dest| dest.token == self.token) else { return None };
            let was_files = std::mem::replace(&mut dest.files, false);
            let feedback = dest.feedback.take();
            let helper = std::mem::replace(&mut dest.helper_active, false)
                .then(|| dest.helper.clone())
                .flatten();
            let saved_data = dest.data.take();
            let wrote = std::mem::replace(&mut dest.wrote_description, false);
            dest.applied_description = None;
            dest.last_pushed = None;
            dest.last_allowed = DROPEFFECT_NONE;
            dest.last_effect = DROPEFFECT_NONE;
            Some((was_files, feedback, helper, saved_data, wrote))
        });

        let Some((was_files, feedback, helper, saved_data, wrote)) = state else {
            write_effect(out, DROPEFFECT_NONE);
            return Ok(());
        };

        let chosen = resolve_effect(!files.is_empty(), allowed, feedback.as_ref());
        write_effect(out, chosen);

        if let (Some(helper), Some(data)) = (helper, data.as_ref()) {
            // SAFETY: STA COM call on the UI thread; `DESTS` is not borrowed.
            unsafe { helper.Drop(data, &point, chosen).ok(); }
        }
        if wrote && let Some(target_data) = data.as_ref().or(saved_data.as_ref()) {
            write_drop_description(target_data, None);
        }
        drop(saved_data);

        if !files.is_empty() && chosen != DROPEFFECT_NONE {
            push(self.hwnd, self.token, FileStep::DropWith {
                paths: files,
                at: self.at(pt),
                mods,
                allowed: allowed_ops,
                operation: operation_from_effect(chosen),
            });
        } else if was_files {
            push(self.hwnd, self.token, FileStep::Leave);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "drop_target_tests.rs"]
mod tests;
