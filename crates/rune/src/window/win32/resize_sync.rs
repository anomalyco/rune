//! Windows opt-in synchronized drag resize (`RUNE_RESIZE_SYNC=1`).
//!
//! Off by default until verified visually on a live Windows 11 display. With `RUNE_RESIZE_SYNC`
//! unset or `0`, every Windows window stays on its normal DirectComposition flip-model path with
//! zero D3D11 devices, zero shared textures, zero second swap chains, and unchanged `Slot`
//! behaviour.
//!
//! When opted in on an opaque app window with a settled Rune-owned DirectComposition `Handover`,
//! resizing inside `WM_ENTERSIZEMOVE..WM_EXITSIZEMOVE` renders drag frames off screen into a
//! capacited 2-slot D3D12 shared texture ring synchronized via shared fences into a D3D11
//! `DXGI_SWAP_EFFECT_DISCARD` copy-model HWND swap chain presented inside `WM_PAINT`
//! (`BeginPaint`/`EndPaint`), with zero presents to the hidden D3D12 composition swap chain
//! during the drag. On `WM_EXITSIZEMOVE`, one normal D3D12 flip-model frame is presented and
//! completed before re-attaching DirectComposition and releasing all drag textures and the D3D11
//! swap chain.

use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

mod frame;
pub(crate) mod gpu;
pub(crate) mod machine;
mod present;

#[cfg(test)]
mod tests;

use gpu::{DragDevice, DragSeam, DragSlot, DragTargetRing, SharedDragTarget};
pub(crate) use machine::{Effect, Eligibility, Event, State, step, valid_rects};
use present::{DxgiDragSwapChain, Win32DragSeam};

use crate::machine::Seq;
use crate::render::Gpu;
use crate::time::Instant;
use crate::window::WakeEvent;
use crate::window::host::Host;
use crate::window::host::stepping::FrameMove;
use crate::window::surface::{configure, drawable_size, max_surface_dim};
use crate::window::win32::handover::SurfaceMode;

/// Per-window synchronized drag resize state and lazily created D3D11/shared-texture resources.
///
/// Fields drop in declaration order: the copy-model swap chain and shared texture ring drop
/// before the D3D11 device they were opened on.
#[derive(Default)]
pub(in crate::window) struct DragResizeSync {
    state: State,
    seq: Seq,
    swap_chain: Option<DxgiDragSwapChain>,
    ring: DragTargetRing,
    device: Option<DragDevice>,
    dcomp_detached: bool,
    /// Hidden D3D12 swap-chain size at the start of the active drag, restored on mid-drag
    /// fallback so `Win::resize_surface` never skips `ResizeBuffers`.
    surface_size_before_drag: Option<(u32, u32)>,
    logged_error: bool,
}

impl DragResizeSync {
    /// Whether the window is currently presenting drag frames through the D3D11 copy-model path.
    pub(in crate::window) fn is_dragging(&self) -> bool {
        self.state.is_dragging()
    }

    /// Whether the window is waiting for one completed post-drag D3D12 flip-model frame before
    /// re-attaching DirectComposition.
    pub(in crate::window) fn is_leaving(&self) -> bool {
        self.state.is_leaving()
    }

    /// The `Leaving` safety timer deadline, when armed.
    pub(in crate::window) fn deadline(&self) -> Option<Instant> {
        self.state.deadline()
    }

    /// Ensures a live D3D11 device matching `device` and acquires the next capacited shared
    /// texture slot covering `(size, format)`.
    pub(super) fn ensure_gpu_target(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
        format: wgpu::TextureFormat,
        exact: bool,
    ) -> Result<(&mut DragDevice, usize, &DragSlot), String> {
        if self.device.as_ref().is_none_or(|d| !d.matches(device)) {
            self.swap_chain = None;
            self.ring.release();
            self.device = Some(DragDevice::for_wgpu(device)?);
        }
        let drag = self.device.as_mut().ok_or("drag device missing")?;
        let (idx, slot) = self.ring.acquire_slot(device, drag, size, format, exact)?;
        Ok((drag, idx, slot))
    }

    /// Borrows the live D3D11 device and slot `idx` simultaneously for encode/present.
    pub(super) fn borrow_slot(&mut self, idx: usize) -> Result<(&mut DragDevice, &SharedDragTarget), String> {
        let drag = self.device.as_mut().ok_or("drag device missing")?;
        let slot = self.ring.slot(idx).ok_or("shared drag target missing")?;
        Ok((drag, &slot.target))
    }

    /// Records the D3D11 release-fence value after presenting slot `idx`.
    pub(super) fn note_slot_release(&mut self, idx: usize, release_value: u64) {
        self.ring.note_release(idx, release_value);
    }

    /// Frees transient drag resources (the copy-model swap chain and shared render-target
    /// textures) while keeping the lazily created D3D11 device for subsequent drags on the same
    /// live adapter.
    pub(super) fn release_drag_resources(&mut self) {
        self.swap_chain = None;
        self.ring.release();
        self.surface_size_before_drag = None;
    }

    /// Drops the D3D11 device and all resources opened on it (used after D3D12 device loss or
    /// window destruction).
    pub(super) fn drop_d3d11_device(&mut self) {
        self.swap_chain = None;
        self.ring.release();
        self.surface_size_before_drag = None;
        self.device = None;
    }

    /// Logs the first fallback error once per window so a broken driver never spams stderr.
    fn log_error_once(&mut self, error: &str) {
        if !self.logged_error {
            self.logged_error = true;
            eprintln!("rune: RUNE_RESIZE_SYNC fell back to normal resize: {error}");
        }
    }
}

impl Host {
    /// Registers the `dpi_drag` size-move wake hook on `window` when `RUNE_RESIZE_SYNC` is on
    /// and the window is opaque.
    pub(in crate::window) fn register_resize_sync(&self, window: &Window) {
        if !self.resize_sync_enabled() || self.options.transparent {
            return;
        }
        let handle = self.native_handle.clone();
        crate::platform::win32::register_resize_sync(window, move || {
            handle.send(WakeEvent::ResizeSync);
        });
    }

    /// Computes the current [`Eligibility`] for this window.
    pub(in crate::window) fn resize_sync_eligibility(&self) -> Eligibility {
        let opt_in = self.resize_sync_enabled();
        let opaque = !self.options.transparent;
        let dxgi_from_visual =
            matches!(Gpu::dx12_swapchain_kind(), wgpu::Dx12SwapchainKind::DxgiFromVisual);
        let handed_over = self
            .win
            .as_ref()
            .is_some_and(|w| matches!(w.native.mode, SurfaceMode::Handed { .. }));
        let d3d12_live = self.gpu.as_ref().is_some_and(|(gpu, _)| {
            !gpu.is_lost() && rune_gpu::d3d11_interop::d3d12_device_luid(&gpu.device).is_some()
        });
        let hardware_adapter = self
            .gpu
            .as_ref()
            .is_some_and(|(gpu, _)| gpu.adapter.get_info().device_type != wgpu::DeviceType::Cpu);
        let srgb_blend = self.app.rt().srgb_blend();
        let supported_format = self.win.as_ref().is_some_and(|w| {
            let fmt = if srgb_blend { w.config.format.remove_srgb_suffix() } else { w.config.format };
            gpu::dxgi_format_for(fmt).is_ok()
        });
        Eligibility {
            opt_in,
            opaque,
            overlay: false,
            dxgi_from_visual,
            handed_over,
            d3d12_live,
            hardware_adapter,
            supported_format,
        }
    }

    /// Pulls any queued `WM_ENTERSIZEMOVE` / `WM_EXITSIZEMOVE` / `WM_NCDESTROY` signal from
    /// `dpi_drag` and steps the drag state machine.
    pub(in crate::window) fn sync_resize_signals(&mut self) {
        if !self.resize_sync_enabled() {
            return;
        }
        let Some(hwnd) = self.win.as_ref().and_then(|w| crate::platform::win32::hwnd(&w.window)) else {
            return;
        };
        let signal = crate::platform::win32::take_resize_sync_signal(hwnd);
        if signal.destroyed {
            self.drive_resize_sync(Event::Destroy);
            return;
        }
        if signal.entered {
            let eligibility = self.resize_sync_eligibility();
            self.drive_resize_sync(Event::EnterSizeMove { eligibility });
        }
        if signal.exited {
            self.drive_resize_sync(Event::ExitSizeMove { now: crate::time::now() });
        }
    }

    /// Handles `WakeEvent::ResizeSync` posted when `WM_EXITSIZEMOVE` fires.
    pub(in crate::window) fn handle_resize_sync_wake(&mut self) {
        self.sync_resize_signals();
    }

    /// Checks the `Leaving` safety timer on each event-loop turn.
    pub(in crate::window) fn watch_resize_sync(&mut self, now: Instant) {
        if !self.resize_sync_enabled() {
            return;
        }
        self.sync_resize_signals();
        let due = self.win.as_ref().and_then(|w| {
            let attempt = w.native.resize_sync.state.leaving_attempt()?;
            let deadline = w.native.resize_sync.state.deadline()?;
            (now >= deadline).then_some(attempt)
        });
        if let Some(attempt) = due {
            self.drive_resize_sync(Event::TimerDue { attempt, now });
        }
    }

    /// Called from `WindowEvent::Resized` on Windows before resizing the normal D3D12 swap chain.
    /// Returns `true` when the resize was handled by the drag-sync path (skipping normal D3D12
    /// `ResizeBuffers` and presentation); returns `false` immediately when `RUNE_RESIZE_SYNC` is
    /// off or the window is not eligible.
    pub(in crate::window) fn try_handle_drag_resize(
        &mut self,
        size: PhysicalSize<u32>,
        scale: f32,
        event_loop: &ActiveEventLoop,
    ) -> bool {
        if !self.resize_sync_enabled() {
            return false;
        }
        self.sync_resize_signals();
        let in_loop = self
            .win
            .as_ref()
            .is_some_and(|w| crate::platform::win32::in_size_move(&w.window));
        if !in_loop && !self.win.as_ref().is_some_and(|w| w.native.resize_sync.is_dragging()) {
            return false;
        }
        let eligibility = self.resize_sync_eligibility();
        let max_dim = max_surface_dim(self.gpu.as_ref().map(|(gpu, _)| &**gpu));
        let prev = self.win.as_ref().map(|w| (w.config.width, w.config.height));
        let target_size = if size.width == 0 || size.height == 0 {
            (0, 0)
        } else {
            drawable_size(prev, (size.width, size.height), max_dim)
        };
        if in_loop && self.win.as_ref().is_some_and(|w| w.native.resize_sync.state == State::Idle) {
            self.drive_resize_sync(Event::EnterSizeMove { eligibility });
        }
        let Some(w) = self.win.as_mut() else { return false };
        let effects = step(
            &mut w.native.resize_sync.seq,
            &mut w.native.resize_sync.state,
            Event::Resize { size: target_size, eligibility },
        );
        let mut handled = false;
        for effect in effects {
            match effect {
                Effect::BeginDrag { size: drag_size } => {
                    handled |= self.try_drag_step_with_rollback(drag_size, scale, true);
                }
                Effect::RenderDragFrame { size: drag_size } => {
                    handled |= self.try_drag_step_with_rollback(drag_size, scale, false);
                }
                other => self.apply_resize_sync_effect(other),
            }
        }
        if handled {
            self.shell(event_loop);
        }
        handled
    }

    /// Called from `Host::draw` on Windows: while actively dragging, draws into the shared D3D12
    /// texture ring and presents on the D3D11 copy-model swap chain with zero presents to the
    /// hidden D3D12 swap chain. Always returns `false` when `RUNE_RESIZE_SYNC` is off.
    pub(in crate::window) fn try_draw_drag_resize(&mut self) -> bool {
        if !self.resize_sync_enabled() {
            return false;
        }
        self.sync_resize_signals();
        let Some(w) = self.win.as_mut() else { return false };
        if !w.native.resize_sync.is_dragging() {
            return false;
        }
        let effects = step(
            &mut w.native.resize_sync.seq,
            &mut w.native.resize_sync.state,
            Event::Redraw,
        );
        let mut handled = false;
        for effect in effects {
            if let Effect::RenderDragFrame { size } = effect {
                handled |= self.execute_render_drag_frame(size);
            } else {
                self.apply_resize_sync_effect(effect);
            }
        }
        handled
    }

    /// Called from `submit_and_present_window` once a post-drag normal D3D12 flip-model frame has
    /// been presented and waited on.
    pub(in crate::window) fn finish_resize_sync_exit_frame(&mut self, completed: bool) {
        if !self.resize_sync_enabled() || !self.win.as_ref().is_some_and(|w| w.native.resize_sync.is_leaving()) {
            return;
        }
        let event = if completed { Event::ExitFrameCompleted } else { Event::DeviceLost };
        self.drive_resize_sync(event);
    }

    /// Feeds a lifecycle event (`DeviceLost`, `Handover`, `Hide`, `Destroy`) into the drag state
    /// machine and executes its cleanup effects.
    pub(in crate::window) fn on_resize_sync_event(&mut self, event: Event) {
        if !self.resize_sync_enabled() && self.win.as_ref().is_none_or(|w| w.native.resize_sync.state == State::Idle) {
            return;
        }
        self.drive_resize_sync(event);
    }

    fn drive_resize_sync(&mut self, event: Event) {
        let Some(w) = self.win.as_mut() else { return };
        let effects = step(&mut w.native.resize_sync.seq, &mut w.native.resize_sync.state, event);
        for effect in effects {
            self.apply_resize_sync_effect(effect);
        }
    }

    fn apply_resize_sync_effect(&mut self, effect: Effect) {
        match effect {
            Effect::BeginDrag { size } => {
                let _ = self.execute_begin_drag(size);
            }
            Effect::RenderDragFrame { size } => {
                let _ = self.execute_render_drag_frame(size);
            }
            Effect::RequestExitFrame => {
                self.prepare_exit_frame();
            }
            Effect::WakeAt(_) => {
                // `AnyHost::wake_deadline` polls `Win::native.resize_sync.deadline()` directly.
            }
            Effect::AttachDcomp => {
                if let Some(w) = self.win.as_mut()
                    && let Some(hwnd) = crate::platform::win32::hwnd(&w.window)
                {
                    let mut seam = Win32DragSeam {
                        hwnd,
                        handover: w.native.mode.handover(),
                        swap_chain: &mut w.native.resize_sync.swap_chain,
                        dcomp_detached: &mut w.native.resize_sync.dcomp_detached,
                    };
                    if let Err(err) = seam.attach_dcomp() {
                        w.native.resize_sync.log_error_once(&err);
                    }
                }
            }
            Effect::ReleaseDragResources => {
                if let Some(w) = self.win.as_mut() {
                    if let Some(hwnd) = crate::platform::win32::hwnd(&w.window) {
                        let mut seam = Win32DragSeam {
                            hwnd,
                            handover: w.native.mode.handover(),
                            swap_chain: &mut w.native.resize_sync.swap_chain,
                            dcomp_detached: &mut w.native.resize_sync.dcomp_detached,
                        };
                        seam.set_valid_rects_active(false);
                        seam.release_swap_chain();
                    }
                    w.native.resize_sync.release_drag_resources();
                }
            }
            Effect::DropD3d11Device => {
                if let Some(w) = self.win.as_mut() {
                    if let Some(hwnd) = crate::platform::win32::hwnd(&w.window) {
                        crate::platform::win32::set_resize_sync_active(hwnd, false);
                    }
                    w.native.resize_sync.drop_d3d11_device();
                }
            }
        }
    }

    /// Applies the drag step viewport, executes `BeginDrag` or `RenderDragFrame`, and rolls back
    /// `l.config` to the hidden D3D12 swap chain's true size if the drag step fails so the normal
    /// fallback path in `Win::resize_surface` always runs `ResizeBuffers`.
    fn try_drag_step_with_rollback(&mut self, size: (u32, u32), scale: f32, begin: bool) -> bool {
        let Some(l) = self.win.as_mut() else {
            if begin {
                self.drive_resize_sync(Event::DragFailed);
            }
            return false;
        };
        let surface_size = *l
            .native
            .resize_sync
            .surface_size_before_drag
            .get_or_insert((l.config.width, l.config.height));
        let viewport = (size.0 as f32 / scale, size.1 as f32 / scale);
        let asked = FrameMove::take_jump(&mut self.frame_move, viewport);
        if asked {
            self.app.rt_mut().step_viewport(viewport, scale);
        } else {
            self.app.rt_mut().set_viewport(viewport, scale);
        }
        let ok = if begin {
            self.execute_begin_drag(size)
        } else {
            self.execute_render_drag_frame(size)
        };
        if let Some(l) = self.win.as_mut() {
            commit_or_rollback_config(&mut l.config, size, surface_size, ok);
        }
        ok
    }

    /// Releases active video layers, renders and presents the first drag frame onto the D3D11
    /// copy-model swap chain inside `WM_PAINT`, detaches the DirectComposition root, and enables
    /// `WVR_VALIDRECTS` + `WM_ERASEBKGND` suppression.
    fn execute_begin_drag(&mut self, size: (u32, u32)) -> bool {
        if let Some(w) = self.win.as_mut() {
            let native = &mut w.native;
            native.video_layers.release(native.mode.handover());
        }
        match self.render_and_present_drag(size, true) {
            Ok(()) => {
                self.drive_resize_sync(Event::DragStarted { size });
                true
            }
            Err(err) => {
                if let Some(w) = self.win.as_mut() {
                    w.native.resize_sync.log_error_once(&err);
                }
                self.drive_resize_sync(Event::DragFailed);
                false
            }
        }
    }

    fn execute_render_drag_frame(&mut self, size: (u32, u32)) -> bool {
        match self.render_and_present_drag(size, false) {
            Ok(()) => true,
            Err(err) => {
                if let Some(w) = self.win.as_mut() {
                    w.native.resize_sync.log_error_once(&err);
                }
                let lost = self.gpu.as_ref().is_none_or(|(gpu, _)| gpu.is_lost());
                self.drive_resize_sync(if lost { Event::DeviceLost } else { Event::DragFailed });
                false
            }
        }
    }

    /// Reconfigures the hidden D3D12 composition swap chain once to the final drag size and
    /// schedules one normal flip-model frame while DirectComposition stays detached.
    fn prepare_exit_frame(&mut self) {
        let Some(l) = self.win.as_mut() else { return };
        if let Some(hwnd) = crate::platform::win32::hwnd(&l.window) {
            crate::platform::win32::set_resize_sync_active(hwnd, false);
        }
        l.presented = None;
        l.damage.reset();
        l.native.resize_sync.surface_size_before_drag = None;
        if let (Some(surface), Some((gpu, _))) = (l.surface.as_ref(), self.gpu.as_ref()) {
            let configured = configure(surface, gpu, &l.config);
            l.native.slot.reconfigured(configured.is_ok());
            if let Err(error) = configured {
                eprintln!("rune: a window's surface refused its post-drag size: {error}");
                l.surface_ok = false;
            }
        }
        l.request_redraw();
    }
}

/// Commits `drag_size` into `config` only when the drag step succeeded; on failure, restores
/// `config` to `surface_size_before_drag` (the true dimensions of the hidden D3D12 swap chain)
/// so `Win::resize_surface` never mistakes the swap chain as already at `drag_size` and skips
/// `ResizeBuffers`.
pub(super) fn commit_or_rollback_config(
    config: &mut wgpu::SurfaceConfiguration,
    drag_size: (u32, u32),
    surface_size_before_drag: (u32, u32),
    succeeded: bool,
) {
    if succeeded {
        config.width = drag_size.0;
        config.height = drag_size.1;
    } else {
        config.width = surface_size_before_drag.0;
        config.height = surface_size_before_drag.1;
    }
}
