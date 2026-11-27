//! Real Win32 / WinRT implementation of [`super::WindowsMaterialSeam`] behind
//! `feature = "windows-system-material"`.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use windows::Foundation::Numerics::Plane;
use windows::System::{DispatcherQueue, DispatcherQueueController};
use windows::UI::Color as WinColor;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    CompositionBackdropBrush, CompositionColorBrush, CompositionEffectBrush,
    CompositionGeometricClip, CompositionRoundedRectangleGeometry, Compositor, ContainerVisual,
    RectangleClip, SpriteVisual,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::core::{HRESULT, Interface};

use super::acrylic::{self, AcrylicBrushKey, AcrylicRecipe, BrushChoice};
use super::probe::{self, AcrylicBrushCache, HostAvailability, HostProbeSeam};
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

/// Queries `DwmIsCompositionEnabled` (`dwmapi.h`). Synchronous, requires no COM init, and
/// returns `Ok(false)` or an `HRESULT` error on Session 0 / headless runners without DWM.
fn dwm_composition_enabled() -> Result<bool, String> {
    use windows_sys::Win32::Graphics::Dwm::DwmIsCompositionEnabled;
    use windows_sys::core::BOOL;
    let mut enabled: BOOL = 0;
    // SAFETY: `DwmIsCompositionEnabled` writes one Win32 `BOOL` and never blocks.
    let hr = unsafe { DwmIsCompositionEnabled(&raw mut enabled) };
    if hr >= 0 {
        Ok(enabled != 0)
    } else {
        Err(format!("0x{:08x}", hr as u32))
    }
}

struct RealProbeSeam;

impl HostProbeSeam for RealProbeSeam {
    type Compositor = Compositor;
    type BackdropBrush = CompositionBackdropBrush;

    fn windows_build(&mut self) -> Option<u32> {
        crate::platform::win32::dark_mode::windows_build()
    }

    fn dwm_composition_enabled(&mut self) -> Result<bool, String> {
        dwm_composition_enabled()
    }

    fn ensure_dispatcher_queue(&mut self) -> Result<(), String> {
        ensure_dispatcher_queue()
    }

    fn create_compositor(&mut self) -> Result<Self::Compositor, String> {
        Compositor::new().map_err(|e| format!("Compositor::new: {e}"))
    }

    fn create_host_backdrop_brush(
        &mut self,
        compositor: &Self::Compositor,
    ) -> Result<Self::BackdropBrush, String> {
        compositor
            .CreateHostBackdropBrush()
            .map_err(|e| format!("CreateHostBackdropBrush: {e}"))
    }

    fn close_backdrop_brush(&mut self, brush: Self::BackdropBrush) {
        brush.Close().ok();
    }

    fn close_compositor(&mut self, compositor: Self::Compositor) {
        compositor.Close().ok();
    }
}

#[cfg(test)]
impl probe::AcrylicFactorySeam for RealProbeSeam {
    type EffectBrush = CompositionEffectBrush;

    fn create_acrylic_brush(
        &mut self,
        compositor: &Self::Compositor,
        backdrop: &Self::BackdropBrush,
        recipe: AcrylicRecipe,
    ) -> Result<Self::EffectBrush, String> {
        acrylic::winrt_graph::create_acrylic_brush(compositor, backdrop, recipe)
    }

    fn close_effect_brush(&mut self, brush: Self::EffectBrush) {
        brush.Close().ok();
    }
}

static HOST_AVAILABILITY: OnceLock<HostAvailability> = OnceLock::new();

/// Single cached host-availability probe shared by production (`RealSeam`) and the live COM
/// tests. Runs lazily at most once per process, closes every COM object it creates, and never
/// panics or hangs.
pub(super) fn probe_host() -> HostAvailability {
    HOST_AVAILABILITY
        .get_or_init(|| probe::probe_host_with(&mut RealProbeSeam))
        .clone()
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
    /// Last applied [`BrushChoice`] so springing frames skip `SetBrush`.
    applied_brush: Option<BrushChoice>,
}

struct TargetState {
    compositor: Compositor,
    target: DesktopWindowTarget,
    root: ContainerVisual,
    brush: CompositionBackdropBrush,
    /// Bounded LRU cache of lazily created Acrylic effect brushes keyed by [`AcrylicBrushKey`];
    /// closes cached brushes and latches flat-tint fallback on failure.
    acrylic_cache: AcrylicBrushCache<CompositionEffectBrush>,
    visuals: HashMap<usize, RealSlot>,
}

impl TargetState {
    fn close(mut self) {
        // Best-effort teardown of WinRT composition objects (may already be closed on device loss).
        for (_, slot) in self.visuals {
            slot.clip.close();
            if let Some(tint) = slot.tint {
                tint.visual.Close().ok();
                tint.brush.Close().ok();
            }
            slot.visual.Close().ok();
        }
        self.acrylic_cache.clear_with(|brush| {
            brush.Close().ok();
        });
        self.target.SetRoot(None).ok();
        self.target.Close().ok();
        self.root.Close().ok();
        self.brush.Close().ok();
        self.compositor.Close().ok();
    }

    fn ensure_acrylic_brush(&mut self, key: AcrylicBrushKey) -> Option<&CompositionEffectBrush> {
        let compositor = &self.compositor;
        let backdrop = &self.brush;
        let visuals = &mut self.visuals;
        let mut detached = false;
        match self.acrylic_cache.get_or_create(
            key,
            |k| acrylic::winrt_graph::create_acrylic_brush(compositor, backdrop, AcrylicRecipe::from_key(k)),
            |brush| {
                if !detached {
                    for slot in visuals.values_mut() {
                        if matches!(slot.applied_brush, Some(BrushChoice::Acrylic { .. })) {
                            slot.visual.SetBrush(backdrop).ok();
                            slot.applied_brush = None;
                        }
                    }
                    detached = true;
                }
                brush.Close().ok();
            },
        ) {
            Ok(brush) => brush,
            Err(err) => {
                eprintln!(
                    "rune: Windows Acrylic effect factory unavailable; falling back to flat tint: {err}"
                );
                None
            }
        }
    }

    fn evict_unused_acrylic_brushes(&mut self) {
        let visuals = &self.visuals;
        self.acrylic_cache.evict_unused(
            |k| visuals.values().any(|s| s.applied_brush == Some(BrushChoice::Acrylic { key: *k })),
            |brush| {
                brush.Close().ok();
            },
        );
    }
}

struct RealSeam<'a> {
    target: &'a mut Option<TargetState>,
    handover: Option<&'a mut Handover>,
}

impl WindowsMaterialSeam for RealSeam<'_> {
    fn probe_host(&mut self) -> HostAvailability {
        probe_host()
    }

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
        let interop: ICompositorDesktopInterop = match compositor.cast() {
            Ok(i) => i,
            Err(e) => {
                compositor.Close().ok();
                return Err(format!("cast ICompositorDesktopInterop: {e}"));
            }
        };
        // SAFETY: `hwnd` is a live window of this process whose non-topmost composition target
        // slot was freed by moving `Handover` to `topmost = true`.
        let target = match unsafe { interop.CreateDesktopWindowTarget(HWND(hwnd as *mut _), false) }
        {
            Ok(t) => t,
            Err(e) => {
                compositor.Close().ok();
                return Err(format!("CreateDesktopWindowTarget: {e}"));
            }
        };
        let root = match compositor.CreateContainerVisual() {
            Ok(r) => r,
            Err(e) => {
                target.Close().ok();
                compositor.Close().ok();
                return Err(format!("CreateContainerVisual: {e}"));
            }
        };
        if let Err(e) = target.SetRoot(&root) {
            root.Close().ok();
            target.Close().ok();
            compositor.Close().ok();
            return Err(format!("DesktopWindowTarget::SetRoot: {e}"));
        }
        let brush = match compositor.CreateHostBackdropBrush() {
            Ok(b) => b,
            Err(e) => {
                target.SetRoot(None).ok();
                root.Close().ok();
                target.Close().ok();
                compositor.Close().ok();
                return Err(format!("CreateHostBackdropBrush: {e}"));
            }
        };
        *self.target = Some(TargetState {
            compositor,
            target,
            root,
            brush,
            acrylic_cache: AcrylicBrushCache::new(),
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
        ts.visuals.insert(
            slot_id,
            RealSlot { visual, clip: RealClip::None, tint: None, applied_brush: None },
        );
        Ok(())
    }

    fn update_visual(&mut self, slot_id: usize, spec: VisualSpec) -> Result<(), String> {
        let Some(ts) = self.target.as_mut() else {
            return Err("no active DesktopWindowTarget".into());
        };
        // Resolve Acrylic brush before mutably borrowing `slot`.
        let recipe_key = AcrylicRecipe::resolve(spec.material, spec.dark, spec.params).cache_key();
        let acrylic_brush = if spec.tint_mode == TintMode::Acrylic {
            ts.ensure_acrylic_brush(recipe_key).cloned()
        } else {
            None
        };
        let choice = acrylic::resolve_brush_choice(
            spec.tint_mode,
            spec.material,
            spec.params,
            spec.dark,
            acrylic_brush.is_some(),
        );
        let flat_tint = match choice {
            BrushChoice::FlatTint { argb } => Some(argb),
            BrushChoice::Acrylic { .. } | BrushChoice::RawHostBackdrop => None,
        };
        let Some(slot) = ts.visuals.get_mut(&slot_id) else {
            return Err(format!("missing material slot {slot_id}"));
        };
        if slot.applied_brush != Some(choice) {
            if let Some(effect_brush) = acrylic_brush.as_ref() {
                slot.visual.SetBrush(effect_brush).map_err(|e| format!("SetBrush(Acrylic): {e}"))?;
            } else {
                slot.visual.SetBrush(&ts.brush).map_err(|e| format!("SetBrush(HostBackdrop): {e}"))?;
            }
            slot.applied_brush = Some(choice);
        }
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

        // Flat-tint child visual when `TintMode::Flat` is requested or Acrylic factory fell back.
        match flat_tint {
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
        ts.evict_unused_acrylic_brushes();
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

#[cfg(test)]
pub(super) fn compile_all_acrylic_brushes() -> Result<usize, String> {
    probe::compile_all_acrylic_brushes_with(&mut RealProbeSeam)
}

#[cfg(test)]
pub(super) fn compile_acrylic_recipes(recipes: &[AcrylicRecipe]) -> Result<usize, String> {
    probe::compile_acrylic_recipes_with(&mut RealProbeSeam, recipes)
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
