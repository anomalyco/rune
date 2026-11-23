//! Files dragged in from other apps, through Rune's own OLE drop target. winit's (its
//! `with_drag_and_drop`) initialises OLE and registers its target inside `CreateWindowExW`, which
//! costs a new window about 8,400 µs and six shell DLLs before its first frame; Rune creates
//! windows without it and registers this target once the window has shown (`install`), so a
//! drag can only start after a frame a person has seen anyway. The steps reach the host as
//! macOS's do (`platform::macos::drop`): with the pointer's place in window points, drained each
//! turn by `Host::flush_files`.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, POINTL};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::core::{Ref, implement};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::window::FileStep;

/// What a registered window's target has seen since its host last looked.
struct Dest {
    /// Identifies one registration so an older guard or COM object cannot touch a later window
    /// that reused the same HWND.
    token: u64,
    steps: Vec<FileStep>,
    /// The drag over it carries files, so it may drop them.
    files: bool,
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
        d.insert(hwnd, Dest { token, steps: Vec::new(), files: false });
        Some(token)
    })
}

fn forget(hwnd: isize, token: u64) -> bool {
    DESTS.try_with(|d| {
        let mut d = d.borrow_mut();
        if d.get(&hwnd).is_some_and(|dest| dest.token == token) {
            d.remove(&hwnd);
            true
        } else {
            false
        }
    }).unwrap_or(false)
}

fn uninstall_hwnd(hwnd: isize, token: u64) {
    if !forget(hwnd, token) {
        return;
    }
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

fn push_step(hwnd: isize, token: u64, step: FileStep) -> bool {
    DESTS.with(|d| {
        let mut d = d.borrow_mut();
        let Some(dest) = d.get_mut(&hwnd).filter(|dest| dest.token == token) else { return false };
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
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd as _, windows_sys::Win32::UI::WindowsAndMessaging::WM_NULL, 0, 0);
    }
}

fn set_files_for(hwnd: isize, token: u64, files: bool) {
    DESTS.with(|d| {
        if let Some(dest) = d.borrow_mut().get_mut(&hwnd).filter(|dest| dest.token == token) {
            dest.files = files;
        }
    });
}

fn files_for(hwnd: isize, token: u64) -> bool {
    DESTS.with(|d| d.borrow().get(&hwnd).is_some_and(|d| d.token == token && d.files))
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

    fn set_files(&self, files: bool) {
        set_files_for(self.hwnd, self.token, files);
    }

    fn files(&self) -> bool {
        files_for(self.hwnd, self.token)
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

fn effect(out: *mut DROPEFFECT, files: bool) {
    if !out.is_null() {
        // SAFETY: OLE's out parameter for the effect the target offers.
        unsafe { *out = if files { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, pt: &POINTL, out: *mut DROPEFFECT) -> windows::core::Result<()> {
        let files = paths(data.as_ref());
        self.set_files(!files.is_empty());
        effect(out, !files.is_empty());
        if !files.is_empty() {
            push(self.hwnd, self.token, FileStep::Enter(files, Some(self.at(pt))));
        }
        Ok(())
    }

    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, pt: &POINTL, out: *mut DROPEFFECT) -> windows::core::Result<()> {
        let files = self.files();
        effect(out, files);
        if files {
            push(self.hwnd, self.token, FileStep::Move(self.at(pt)));
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if self.files() {
            self.set_files(false);
            push(self.hwnd, self.token, FileStep::Leave);
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, pt: &POINTL, out: *mut DROPEFFECT) -> windows::core::Result<()> {
        let files = paths(data.as_ref());
        self.set_files(false);
        effect(out, !files.is_empty());
        if !files.is_empty() {
            push(self.hwnd, self.token, FileStep::Drop(files, Some(self.at(pt))));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "drop_target_tests.rs"]
mod tests;
