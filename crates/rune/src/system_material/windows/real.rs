//! Real Win32 / WinRT implementation of [`super::WindowsMaterialSeam`] behind
//! `feature = "windows-system-material"`.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Foundation::Numerics::Plane;
use windows::System::{DispatcherQueue, DispatcherQueueController};
use windows::UI::Color as WinColor;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    CompositionBackdropBrush, CompositionColorBrush, CompositionGeometricClip,
    CompositionRoundedRectangleGeometry, Compositor, ContainerVisual, RectangleClip,
    SpriteVisual,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::core::{HRESULT, Interface};

use super::{
    ClipKind, Controller, EffectsPolicy, MaterialInputs, State, TintMode, VisualSpec,
    WindowsMaterialSeam,
};
use crate::platform::win32::dcomp_handover::Handover;
use crate::system_material::{MaterialDraw, Mode};

/// `DispatcherQueueOptions` from `dispatcherqueue.h`.
#[repr(C)]
struct DispatcherQueueOptions {
    dw_size: u32,
    thread_type: i32,
    apartment_type: i32,
}

/// `DQTYPE_THREAD_CURRENT` (associate a `DispatcherQueue` with the current UI thread).
const DQTYPE_THREAD_CURRENT: i32 = 2;
/// `DQTAT_COM_NONE` (UI thread COM apartment is already initialized by winit/OLE).
const DQTAT_COM_NONE: i32 = 0;

thread_local! {
    static DISPATCHER_QUEUE: RefCell<Option<DispatcherQueueController>> = const { RefCell::new(None) };
}

/// Ensures the current UI thread has a `DispatcherQueue`, loading `CoreMessaging.dll` from
/// System32 lazily on first use (never at startup, never for apps without materials).
fn ensure_dispatcher_queue() -> Result<(), String> {
    if DISPATCHER_QUEUE.with(|cell| cell.borrow().is_some()) {
        return Ok(());
    }
    // Reuse an existing thread DispatcherQueue if the host app already created one.
    if DispatcherQueue::GetForCurrentThread().is_ok() {
        return Ok(());
    }
    type CreateDqc = unsafe extern "system" fn(
        DispatcherQueueOptions,
        *mut *mut std::ffi::c_void,
    ) -> HRESULT;
    // SAFETY: loads CoreMessaging.dll exclusively from System32 and calls the documented
    // `CreateDispatcherQueueController` export from `dispatcherqueue.h`.
    unsafe {
        let lib = crate::platform::win32::load_system_dll("CoreMessaging.dll");
        if lib.is_null() {
            return Err("CoreMessaging.dll did not load from System32".into());
        }
        let Some(proc_addr) = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
            lib,
            c"CreateDispatcherQueueController".as_ptr().cast(),
        ) else {
            return Err("CoreMessaging.dll has no CreateDispatcherQueueController export".into());
        };
        let create: CreateDqc = std::mem::transmute(proc_addr);
        let options = DispatcherQueueOptions {
            dw_size: size_of::<DispatcherQueueOptions>() as u32,
            thread_type: DQTYPE_THREAD_CURRENT,
            apartment_type: DQTAT_COM_NONE,
        };
        let mut raw = std::ptr::null_mut();
        create(options, &mut raw)
            .ok()
            .map_err(|e| format!("CreateDispatcherQueueController: {e}"))?;
        if raw.is_null() {
            return Err("CreateDispatcherQueueController returned null".into());
        }
        let controller = DispatcherQueueController::from_raw(raw);
        DISPATCHER_QUEUE.with(|cell| *cell.borrow_mut() = Some(controller));
        Ok(())
    }
}

/// Constructs a zeroed `Vector2` without adding a direct `windows-numerics` crate dependency.
#[inline]
fn vec2_zero<T: Default>(_: fn(&SpriteVisual) -> windows::core::Result<T>) -> T {
    T::default()
}

/// Sets `DwmSetWindowAttribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, &enabled, 4)`.
fn set_dwm_host_backdrop(hwnd: isize, enabled: bool) -> Result<(), String> {
    use windows_sys::Win32::Foundation::HWND as SysHwnd;
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_HOSTBACKDROPBRUSH, DwmSetWindowAttribute};
    use windows_sys::core::BOOL;
    let value: BOOL = i32::from(enabled);
    // SAFETY: `hwnd` is a live top-level window owned by this thread; `value` is a 4-byte
    // Win32 `BOOL` matching documented `DWMWA_USE_HOSTBACKDROPBRUSH` (Windows 11 Build 22000+).
    let hr = unsafe {
        DwmSetWindowAttribute(
            hwnd as SysHwnd,
            DWMWA_USE_HOSTBACKDROPBRUSH as u32,
            (&raw const value).cast(),
            size_of::<BOOL>() as u32,
        )
    };
    if hr >= 0 {
        Ok(())
    } else {
        Err(format!("DwmSetWindowAttribute(DWMWA_USE_HOSTBACKDROPBRUSH={enabled}): 0x{:08x}", hr as u32))
    }
}

enum RealClip {
    None,
    Rect(RectangleClip),
    Geo(CompositionGeometricClip, CompositionRoundedRectangleGeometry),
}

impl RealClip {
    fn close(self) {
        match self {
            Self::None => {}
            Self::Rect(rc) => {
                // Best-effort COM close during teardown or clip replacement.
                rc.Close().ok();
            }
            Self::Geo(gc, geo) => {
                // Best-effort COM close during teardown or clip replacement.
                gc.Close().ok();
                geo.Close().ok();
            }
        }
    }
}

struct RealTint {
    visual: SpriteVisual,
    brush: CompositionColorBrush,
    color: [u8; 4],
}

struct RealSlot {
    visual: SpriteVisual,
    clip: RealClip,
    tint: Option<RealTint>,
}

struct TargetState {
    compositor: Compositor,
    target: DesktopWindowTarget,
    root: ContainerVisual,
    brush: CompositionBackdropBrush,
    visuals: HashMap<usize, RealSlot>,
}

impl TargetState {
    fn close(self) {
        // Best-effort teardown of WinRT composition objects (may already be closed on device loss).
        for (_, slot) in self.visuals {
            slot.clip.close();
            if let Some(tint) = slot.tint {
                tint.visual.Close().ok();
                tint.brush.Close().ok();
            }
            slot.visual.Close().ok();
        }
        self.target.SetRoot(None).ok();
        self.target.Close().ok();
        self.root.Close().ok();
        self.brush.Close().ok();
        self.compositor.Close().ok();
    }
}

struct RealSeam<'a> {
    target: &'a mut Option<TargetState>,
    handover: Option<&'a mut Handover>,
}

impl WindowsMaterialSeam for RealSeam<'_> {
    fn set_handover_topmost(&mut self, hwnd: isize, topmost: bool) -> Result<(), String> {
        match self.handover.as_deref_mut() {
            Some(handover) => handover.set_topmost(hwnd, topmost),
            None => Err("no live DirectComposition Handover".into()),
        }
    }

    fn set_host_backdrop_brush(&mut self, hwnd: isize, enabled: bool) -> Result<(), String> {
        set_dwm_host_backdrop(hwnd, enabled)
    }

    fn create_target(&mut self, hwnd: isize) -> Result<(), String> {
        ensure_dispatcher_queue()?;
        let compositor = Compositor::new().map_err(|e| format!("Compositor::new: {e}"))?;
        let interop: ICompositorDesktopInterop =
            compositor.cast().map_err(|e| format!("cast ICompositorDesktopInterop: {e}"))?;
        // SAFETY: `hwnd` is a live window of this process whose non-topmost composition target
        // slot was freed by moving `Handover` to `topmost = true`.
        let target = unsafe { interop.CreateDesktopWindowTarget(HWND(hwnd as *mut _), false) }
            .map_err(|e| format!("CreateDesktopWindowTarget: {e}"))?;
        let root = compositor
            .CreateContainerVisual()
            .map_err(|e| format!("CreateContainerVisual: {e}"))?;
        target.SetRoot(&root).map_err(|e| format!("DesktopWindowTarget::SetRoot: {e}"))?;
        let brush = compositor
            .CreateHostBackdropBrush()
            .map_err(|e| format!("CreateHostBackdropBrush: {e}"))?;
        *self.target = Some(TargetState {
            compositor,
            target,
            root,
            brush,
            visuals: HashMap::new(),
        });
        Ok(())
    }

    fn destroy_target(&mut self) {
        if let Some(ts) = self.target.take() {
            ts.close();
        }
    }

    fn create_visual(&mut self, slot_id: usize) -> Result<(), String> {
        let Some(ts) = self.target.as_mut() else {
            return Err("no active DesktopWindowTarget".into());
        };
        let visual = ts
            .compositor
            .CreateSpriteVisual()
            .map_err(|e| format!("CreateSpriteVisual: {e}"))?;
        visual.SetBrush(&ts.brush).map_err(|e| format!("SetBrush: {e}"))?;
        visual.SetIsVisible(false).map_err(|e| format!("SetIsVisible: {e}"))?;
        visual.SetOpacity(0.0).map_err(|e| format!("SetOpacity: {e}"))?;
        ts.root
            .Children()
            .map_err(|e| format!("root.Children: {e}"))?
            .InsertAtTop(&visual)
            .map_err(|e| format!("InsertAtTop: {e}"))?;
        ts.visuals.insert(slot_id, RealSlot { visual, clip: RealClip::None, tint: None });
        Ok(())
    }

    fn update_visual(&mut self, slot_id: usize, spec: VisualSpec) -> Result<(), String> {
        let Some(ts) = self.target.as_mut() else {
            return Err("no active DesktopWindowTarget".into());
        };
        let Some(slot) = ts.visuals.get_mut(&slot_id) else {
            return Err(format!("missing material slot {slot_id}"));
        };
        let mut offset = Plane::default().Normal;
        offset.X = spec.offset[0];
        offset.Y = spec.offset[1];
        offset.Z = 0.0;
        slot.visual.SetOffset(offset).map_err(|e| format!("SetOffset: {e}"))?;

        let mut size = vec2_zero(SpriteVisual::Size);
        size.X = spec.size[0];
        size.Y = spec.size[1];
        slot.visual.SetSize(size).map_err(|e| format!("SetSize: {e}"))?;
        slot.visual.SetOpacity(spec.opacity).map_err(|e| format!("SetOpacity: {e}"))?;

        if spec.radius <= 0.0 {
            if !matches!(slot.clip, RealClip::None) {
                slot.visual.SetClip(None).map_err(|e| format!("SetClip(None): {e}"))?;
                std::mem::replace(&mut slot.clip, RealClip::None).close();
            }
        } else {
            let mut radius = vec2_zero(SpriteVisual::Size);
            radius.X = spec.radius;
            radius.Y = spec.radius;
            match spec.clip {
                ClipKind::Rectangle => match &slot.clip {
                    RealClip::Rect(rc) => {
                        rc.SetLeft(0.0).map_err(|e| format!("RectangleClip::SetLeft: {e}"))?;
                        rc.SetTop(0.0).map_err(|e| format!("RectangleClip::SetTop: {e}"))?;
                        rc.SetRight(spec.size[0]).map_err(|e| format!("RectangleClip::SetRight: {e}"))?;
                        rc.SetBottom(spec.size[1]).map_err(|e| format!("RectangleClip::SetBottom: {e}"))?;
                        rc.SetTopLeftRadius(radius).map_err(|e| format!("SetTopLeftRadius: {e}"))?;
                        rc.SetTopRightRadius(radius).map_err(|e| format!("SetTopRightRadius: {e}"))?;
                        rc.SetBottomRightRadius(radius).map_err(|e| format!("SetBottomRightRadius: {e}"))?;
                        rc.SetBottomLeftRadius(radius).map_err(|e| format!("SetBottomLeftRadius: {e}"))?;
                    }
                    _ => {
                        let rc = ts
                            .compositor
                            .CreateRectangleClipWithSidesAndRadius(
                                0.0, 0.0, spec.size[0], spec.size[1],
                                radius, radius, radius, radius,
                            )
                            .map_err(|e| format!("CreateRectangleClipWithSidesAndRadius: {e}"))?;
                        slot.visual.SetClip(&rc).map_err(|e| format!("SetClip(RectangleClip): {e}"))?;
                        slot.clip = RealClip::Rect(rc);
                    }
                },
                ClipKind::Geometric => {
                    let origin = vec2_zero(SpriteVisual::Size);
                    match &slot.clip {
                        RealClip::Geo(_, geo) => {
                            geo.SetOffset(origin).map_err(|e| format!("Geometry::SetOffset: {e}"))?;
                            geo.SetSize(size).map_err(|e| format!("Geometry::SetSize: {e}"))?;
                            geo.SetCornerRadius(radius).map_err(|e| format!("Geometry::SetCornerRadius: {e}"))?;
                        }
                        _ => {
                            let geo = ts
                                .compositor
                                .CreateRoundedRectangleGeometry()
                                .map_err(|e| format!("CreateRoundedRectangleGeometry: {e}"))?;
                            geo.SetOffset(origin).map_err(|e| format!("Geometry::SetOffset: {e}"))?;
                            geo.SetSize(size).map_err(|e| format!("Geometry::SetSize: {e}"))?;
                            geo.SetCornerRadius(radius).map_err(|e| format!("Geometry::SetCornerRadius: {e}"))?;
                            let gc = ts
                                .compositor
                                .CreateGeometricClipWithGeometry(&geo)
                                .map_err(|e| format!("CreateGeometricClipWithGeometry: {e}"))?;
                            slot.visual.SetClip(&gc).map_err(|e| format!("SetClip(GeometricClip): {e}"))?;
                            slot.clip = RealClip::Geo(gc, geo);
                        }
                    }
                }
            }
        }

        // Optional diagnostic tint child visual (inherits parent offset, clip and opacity).
        match spec.tint {
            Some(argb) => {
                let win_color = WinColor { A: argb[0], R: argb[1], G: argb[2], B: argb[3] };
                if let Some(tint) = slot.tint.as_mut() {
                    if tint.color != argb {
                        tint.brush.SetColor(win_color).map_err(|e| format!("tint SetColor: {e}"))?;
                        tint.color = argb;
                    }
                } else {
                    let tint_vis = ts
                        .compositor
                        .CreateSpriteVisual()
                        .map_err(|e| format!("tint CreateSpriteVisual: {e}"))?;
                    let brush = ts
                        .compositor
                        .CreateColorBrushWithColor(win_color)
                        .map_err(|e| format!("tint CreateColorBrushWithColor: {e}"))?;
                    tint_vis.SetBrush(&brush).map_err(|e| format!("tint SetBrush: {e}"))?;
                    let mut rel = vec2_zero(SpriteVisual::Size);
                    rel.X = 1.0;
                    rel.Y = 1.0;
                    tint_vis
                        .SetRelativeSizeAdjustment(rel)
                        .map_err(|e| format!("tint SetRelativeSizeAdjustment: {e}"))?;
                    slot.visual
                        .Children()
                        .map_err(|e| format!("slot.Children: {e}"))?
                        .InsertAtTop(&tint_vis)
                        .map_err(|e| format!("tint InsertAtTop: {e}"))?;
                    slot.tint = Some(RealTint { visual: tint_vis, brush, color: argb });
                }
            }
            None => {
                if let Some(tint) = slot.tint.take() {
                    // Best-effort removal and close of the optional diagnostic tint child visual.
                    slot.visual.Children().and_then(|c| c.Remove(&tint.visual)).ok();
                    tint.visual.Close().ok();
                    tint.brush.Close().ok();
                }
            }
        }

        slot.visual.SetIsVisible(true).map_err(|e| format!("SetIsVisible(true): {e}"))?;
        Ok(())
    }

    fn hide_visual(&mut self, slot_id: usize) -> Result<(), String> {
        let Some(ts) = self.target.as_mut() else {
            return Ok(());
        };
        if let Some(slot) = ts.visuals.get(&slot_id) {
            slot.visual.SetIsVisible(false).map_err(|e| format!("SetIsVisible(false): {e}"))?;
            slot.visual.SetOpacity(0.0).map_err(|e| format!("SetOpacity(0): {e}"))?;
        }
        Ok(())
    }

    fn remove_visual(&mut self, slot_id: usize) {
        let Some(ts) = self.target.as_mut() else {
            return;
        };
        if let Some(slot) = ts.visuals.remove(&slot_id) {
            // Best-effort removal and close of a pruned spare pooled visual.
            ts.root.Children().and_then(|c| c.Remove(&slot.visual)).ok();
            slot.clip.close();
            if let Some(tint) = slot.tint {
                tint.visual.Close().ok();
                tint.brush.Close().ok();
            }
            slot.visual.Close().ok();
        }
    }

    fn commit(&mut self) -> Result<(), String> {
        let Some(ts) = self.target.as_ref() else {
            return Ok(());
        };
        // Fire-and-forget commit trigger; DWM processes the batch without blocking the UI thread.
        let action = ts
            .compositor
            .RequestCommitAsync()
            .map_err(|e| format!("RequestCommitAsync: {e}"))?;
        drop(action);
        Ok(())
    }
}

/// Per-window or per-overlay Windows 11 system-material host.
///
/// Lazily initializes zero WinRT or DWM resources until an eligible transparent window or
/// overlay actually renders an `El::system_material`.
pub(crate) struct Materials {
    controller: Controller,
    target: Option<TargetState>,
}

impl Default for Materials {
    fn default() -> Self {
        Self::new()
    }
}

impl Materials {
    pub(crate) fn new() -> Self {
        Self {
            controller: Controller::new(Mode::from_env(), ClipKind::from_env(), TintMode::from_env()),
            target: None,
        }
    }

    pub(crate) fn mode(&self) -> Mode {
        self.controller.mode()
    }

    /// Whether elements should skip their fallback fill and cut shadow holes for native visuals.
    pub(crate) fn native(&self) -> bool {
        self.controller.native()
    }

    /// Updates activation state after a frame build. Returns `true` if the host must rebuild
    /// this frame with the updated `native()` flag before rendering.
    pub(crate) fn update(
        &mut self,
        hwnd: Option<isize>,
        inputs: MaterialInputs,
        found: bool,
        handover: Option<&mut Handover>,
    ) -> bool {
        let mut seam = RealSeam { target: &mut self.target, handover };
        self.controller.update(hwnd, inputs, found, &mut seam)
    }

    /// Presents the swap chain first, then syncs pooled visuals to `draws` and commits.
    /// Returns `false` if a COM/DWM error forced a rollback to fallback fills.
    pub(crate) fn present(
        &mut self,
        draws: &[MaterialDraw],
        dark: bool,
        handover: Option<&mut Handover>,
        present: impl FnOnce(),
    ) -> bool {
        if !self.controller.native() {
            present();
            return true;
        }
        let mut seam = RealSeam { target: &mut self.target, handover };
        self.controller.present(draws, dark, &mut seam, present)
    }

    /// Handles an OS system-effects policy change event. Returns `true` if the host should redraw.
    pub(crate) fn on_policy_changed(
        &mut self,
        policy: EffectsPolicy,
        handover: Option<&mut Handover>,
    ) -> bool {
        let mut seam = RealSeam { target: &mut self.target, handover };
        self.controller.on_policy_changed(policy, &mut seam)
    }

    /// Cleanly tears down active visuals and returns `Handover` to `topmost = false`.
    pub(crate) fn teardown(&mut self, handover: Option<&mut Handover>) {
        let mut seam = RealSeam { target: &mut self.target, handover };
        self.controller.teardown(&mut seam);
    }

    /// Tears down active visuals on GPU device loss and clears any failure latch.
    pub(crate) fn on_device_loss(&mut self, handover: Option<&mut Handover>) {
        let mut seam = RealSeam { target: &mut self.target, handover };
        self.controller.on_device_loss(&mut seam);
    }
}

impl Drop for Materials {
    fn drop(&mut self) {
        if let Some(ts) = self.target.take() {
            ts.close();
        }
        if let State::Active { hwnd } = self.controller.state {
            // Best-effort clear during Drop; the HWND may already be tearing down.
            set_dwm_host_backdrop(hwnd, false).ok();
            self.controller.state = State::Idle;
        }
    }
}
