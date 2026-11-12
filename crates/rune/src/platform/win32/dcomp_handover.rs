//! The DirectComposition target a window's GPU swap chain shows through once the CPU renderer
//! hands it over. Made uncommitted: DWM keeps composing the window's GDI redirection surface (the
//! CPU's frames) under a visual that has a swap chain and no frame yet, for as many refreshes as
//! the GPU takes. Committing after the GPU's first frame is presented and done makes DWM switch
//! to it within one composition. wgpu's own `VisualFromWndHandle` commits inside
//! `Surface::configure`, before anything is presented (an already visible window would show an
//! empty swap chain for a refresh); a surface on a caller's visual
//! (`SurfaceTargetUnsafe::CompositionVisual`) only sets the visual's content.
//!
//! `dcomp.dll` loads here, when the GPU is ready, not at launch.

use windows::Win32::Graphics::DirectComposition::{IDCompositionDevice, IDCompositionTarget, IDCompositionVisual};
use windows::core::{GUID, HRESULT, Interface};

pub(crate) struct Handover {
    device: IDCompositionDevice,
    _target: IDCompositionTarget,
    /// The target's root: no content of its own; the UI's visual and, under it, any clip's layer.
    root: IDCompositionVisual,
    /// The UI's visual, which the wgpu surface goes on.
    visual: IDCompositionVisual,
}

// SAFETY: DirectComposition objects can be used from any thread one at a time; a `Handover` is
// made on the GPU thread and then used only by the window's thread.
unsafe impl Send for Handover {}

impl Handover {
    /// A device, a target for `hwnd` below its child windows, and a root visual holding the UI's,
    /// uncommitted.
    pub(crate) fn new(hwnd: isize) -> Result<Handover, String> {
        let device = create_device()?;
        // SAFETY: COM calls on objects made here, for a window of this process.
        unsafe {
            let target = device.CreateTargetForHwnd(windows::Win32::Foundation::HWND(hwnd as *mut _), false).map_err(|e| format!("CreateTargetForHwnd: {e}"))?;
            let root = device.CreateVisual().map_err(|e| format!("CreateVisual: {e}"))?;
            let visual = device.CreateVisual().map_err(|e| format!("CreateVisual: {e}"))?;
            root.AddVisual(&visual, true, None).map_err(|e| format!("AddVisual: {e}"))?;
            target.SetRoot(&root).map_err(|e| format!("SetRoot: {e}"))?;
            Ok(Handover { device, _target: target, root, visual })
        }
    }

    /// The visual a wgpu surface goes on (`SurfaceTargetUnsafe::CompositionVisual`).
    pub(crate) fn visual(&self) -> *mut std::ffi::c_void { self.visual.as_raw() }

    /// The visual's swap chain, with the frame it presented, replaces the window's own content
    /// at DWM's next composition.
    pub(crate) fn commit(&self) -> Result<(), String> {
        // SAFETY: a COM call on our device.
        unsafe { self.device.Commit() }.map_err(|e| format!("IDCompositionDevice::Commit: {e}"))
    }

    /// A visual under the UI's that shows the DirectComposition surface `handle` (a playing clip's
    /// own layer, `video::windows::layer`), uncommitted.
    pub(crate) fn add_under(&self, handle: usize) -> Result<IDCompositionVisual, String> {
        // SAFETY: COM calls on our device and visuals; the handle is a live composition surface
        // handle its owner keeps open while the visual shows it.
        unsafe {
            let surface = self.device.CreateSurfaceFromHandle(windows::Win32::Foundation::HANDLE(handle as *mut _)).map_err(|e| format!("CreateSurfaceFromHandle: {e}"))?;
            let visual = self.device.CreateVisual().map_err(|e| format!("CreateVisual: {e}"))?;
            visual.SetContent(&surface).map_err(|e| format!("SetContent: {e}"))?;
            self.root.AddVisual(&visual, false, &self.visual).map_err(|e| format!("AddVisual: {e}"))?;
            Ok(visual)
        }
    }

    /// Takes a visual `add_under` made out of the tree, uncommitted.
    pub(crate) fn remove(&self, visual: &IDCompositionVisual) {
        // SAFETY: a COM call on our root visual.
        let _ = unsafe { self.root.RemoveVisual(visual) };
    }

    /// Commits, and returns once DWM has taken the commit: a frame presented after this shows
    /// over what it committed.
    pub(crate) fn commit_and_wait(&self) -> Result<(), String> {
        self.commit()?;
        // SAFETY: a COM call on our device.
        unsafe { self.device.WaitForCommitCompletion() }.map_err(|e| format!("IDCompositionDevice::WaitForCommitCompletion: {e}"))
    }

}

/// `DCompositionCreateDevice2(NULL, ...)`, from `dcomp.dll` loaded now rather than imported.
fn create_device() -> Result<IDCompositionDevice, String> {
    type Create = unsafe extern "system" fn(*mut std::ffi::c_void, *const GUID, *mut *mut std::ffi::c_void) -> HRESULT;
    let name: Vec<u16> = "dcomp.dll\0".encode_utf16().collect();
    // SAFETY: a system DLL by name; the entry point has the signature dcomp.h declares, and the
    // pointer it writes is an owned reference to the interface asked for.
    unsafe {
        let lib = windows_sys::Win32::System::LibraryLoader::LoadLibraryW(name.as_ptr());
        if lib.is_null() { return Err("dcomp.dll did not load".into()); }
        let Some(create) = windows_sys::Win32::System::LibraryLoader::GetProcAddress(lib, c"DCompositionCreateDevice2".as_ptr().cast()) else {
            return Err("dcomp.dll has no DCompositionCreateDevice2".into());
        };
        let create: Create = std::mem::transmute(create);
        let mut raw = std::ptr::null_mut();
        create(std::ptr::null_mut(), &IDCompositionDevice::IID, &mut raw).ok().map_err(|e| format!("DCompositionCreateDevice2: {e}"))?;
        Ok(IDCompositionDevice::from_raw(raw))
    }
}
