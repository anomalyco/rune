//! Files dragged in from other apps, through Rune's own OLE drop target. winit's (its
//! `with_drag_and_drop`) initialises OLE and registers its target inside `CreateWindowExW`, which
//! costs a new window about 8,400 µs and six shell DLLs before its first frame; Rune creates
//! windows without it and registers this target once the window has shown (`install`), so a
//! drag can only start after a frame a person has seen anyway. The steps reach the host as
//! macOS's do (`platform::macos::drop`): with the pointer's place in window points, drained each
//! turn by `Host::flush_files`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, POINTL};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::core::{Ref, implement};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// One step of a drag over a window, in window points.
pub(crate) enum Step {
    Enter(Vec<PathBuf>, (f32, f32)),
    Move((f32, f32)),
    Drop(Vec<PathBuf>, (f32, f32)),
    Leave,
}

/// What each window's target has seen since its host last looked, and whether it's registered.
#[derive(Default)]
struct Dest {
    installed: bool,
    steps: Vec<Step>,
    /// The drag over it carries files, so it may drop them.
    files: bool,
}

thread_local! {
    /// By HWND: drop targets live on the UI thread that registered them, as OLE calls them there.
    static DESTS: RefCell<HashMap<isize, Dest>> = RefCell::new(HashMap::new());
}

fn hwnd(window: &winit::window::Window) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// Registers the window's drop target, once; after its first frame has shown.
pub(crate) fn install(window: &winit::window::Window) {
    let Some(hwnd) = hwnd(window) else { return };
    let fresh = DESTS.with(|d| {
        let mut d = d.borrow_mut();
        let dest = d.entry(hwnd.0 as isize).or_default();
        !std::mem::replace(&mut dest.installed, true)
    });
    if !fresh {
        return;
    }
    // SAFETY: the UI thread, which winit leaves without COM (drag and drop off), becomes OLE's
    // single-threaded apartment; the target lives as long as the window (OLE holds a reference
    // until the window is destroyed and revokes it then).
    unsafe {
        if OleInitialize(None).is_err() {
            return;
        }
        let target: IDropTarget = Target { hwnd: hwnd.0 as isize }.into();
        let _ = RegisterDragDrop(hwnd, &target);
    }
}

/// What the window's target saw since the last call.
pub(crate) fn take(window: &winit::window::Window) -> Vec<Step> {
    let Some(hwnd) = hwnd(window) else { return Vec::new() };
    DESTS.with(|d| d.borrow_mut().get_mut(&(hwnd.0 as isize)).map(|d| std::mem::take(&mut d.steps)).unwrap_or_default())
}

fn push(hwnd: isize, step: Step) {
    DESTS.with(|d| d.borrow_mut().entry(hwnd).or_default().steps.push(step));
    // Turns the loop, whose `about_to_wait` drains the step (`Host::flush_files`).
    // SAFETY: a message with no meaning to a window we own.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd as _, windows_sys::Win32::UI::WindowsAndMessaging::WM_NULL, 0, 0);
    }
}

#[implement(IDropTarget)]
struct Target {
    hwnd: isize,
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
        DESTS.with(|d| d.borrow_mut().entry(self.hwnd).or_default().files = files);
    }

    fn files(&self) -> bool {
        DESTS.with(|d| d.borrow().get(&self.hwnd).is_some_and(|d| d.files))
    }
}

/// The paths `data` carries as a shell file list (`CF_HDROP`); none for any other drag.
fn paths(data: Option<&IDataObject>) -> Vec<PathBuf> {
    let Some(data) = data else { return Vec::new() };
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
            push(self.hwnd, Step::Enter(files, self.at(pt)));
        }
        Ok(())
    }

    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, pt: &POINTL, out: *mut DROPEFFECT) -> windows::core::Result<()> {
        let files = self.files();
        effect(out, files);
        if files {
            push(self.hwnd, Step::Move(self.at(pt)));
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if self.files() {
            self.set_files(false);
            push(self.hwnd, Step::Leave);
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, pt: &POINTL, out: *mut DROPEFFECT) -> windows::core::Result<()> {
        let files = paths(data.as_ref());
        self.set_files(false);
        effect(out, !files.is_empty());
        if !files.is_empty() {
            push(self.hwnd, Step::Drop(files, self.at(pt)));
        }
        Ok(())
    }
}
