//! Host-side `GpuLossPolicy::Terminal` state, guard points, same-device surface recreation,
//! frozen-window teardown, and one-shot `AppSpec::poll` wake handling.

use std::sync::{Arc, Weak};

use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;

use super::Host;
use super::create::{alpha_mode, alpha_pass};
use crate::app::{Activation, WindowRequest};
use crate::render::loss_latch::{GpuFailureKind, GpuLossLatch, GpuLossPolicy};
use crate::render::{Gpu, SurfaceSetupError};
use crate::window::chrome;
use crate::window::file_drop::FileDrop;
use crate::window::keys::map_key_event;
use crate::window::state::MinimizeEvent;
use crate::window::surface::{
    BackoffEvent, configure_checked, drawable_size, max_surface_dim, surface_config,
};

/// Per-window terminal GPU-loss state owned by [`Host`].
///
/// Under [`GpuLossPolicy::Recover`] (the default), `latch` is `None` and every fast-path guard
/// returns `false` with zero atomic loads or locks.
pub(in crate::window) struct TerminalLoss {
    policy: GpuLossPolicy,
    latch: Option<Arc<GpuLossLatch>>,
    /// Whether [`Host::enter_terminal_gpu_loss`] has already torn down GPU/surface resources.
    entered: bool,
}

impl TerminalLoss {
    pub(in crate::window) fn new(
        policy: GpuLossPolicy,
        latch: Option<Arc<GpuLossLatch>>,
    ) -> Self {
        let latch = match policy {
            GpuLossPolicy::Terminal => Some(latch.unwrap_or_else(GpuLossLatch::new)),
            GpuLossPolicy::Recover => None,
        };
        Self { policy, latch, entered: false }
    }

    #[inline]
    pub(in crate::window) fn is_terminal(&self) -> bool {
        self.policy == GpuLossPolicy::Terminal
    }

    #[inline]
    pub(in crate::window) fn is_failed(&self) -> bool {
        self.latch.as_ref().is_some_and(|l| l.is_failed())
    }

    pub(in crate::window) fn latch(&self) -> Option<&Arc<GpuLossLatch>> {
        self.latch.as_ref()
    }

    pub(in crate::window) fn reason(&self) -> Option<String> {
        self.latch.as_ref().and_then(|l| l.reason())
    }

    /// Connects the one-shot wake hook: publishes the first failure reason into
    /// `SharedCore::window_error` before waking the event loop so `AppHandle::window_error()` is
    /// already populated when `AppSpec::poll` runs.
    pub(in crate::window) fn connect_wake(
        &self,
        shared: Arc<crate::app::SharedCore>,
        waker: crate::runtime::Waker,
    ) {
        let Some(latch) = &self.latch else { return };
        latch.add_wake(Arc::new(move |reason| {
            shared.set_window_error(Some(reason.to_owned()));
            waker();
        }));
    }

    /// Attaches this terminal latch to `gpu` and re-arms after any intentional clean release.
    pub(in crate::window) fn attach_gpu(&self, gpu: &Gpu) {
        if let Some(latch) = &self.latch {
            latch.rearm();
            gpu.attach_terminal_latch(Arc::clone(latch));
        }
    }

    /// Detaches before an intentional clean GPU drop (`release_gpu_when_closed` or normal quit)
    /// so wgpu's drop-triggered `DeviceLostReason::Destroyed` never falsely latches.
    pub(in crate::window) fn prepare_clean_gpu_drop(&self, gpu: Option<&Gpu>) {
        if let Some(latch) = &self.latch && !latch.is_failed() {
            if let Some(gpu) = gpu {
                gpu.mark_clean_drop();
            }
            latch.detach();
        }
    }
}

impl Host {
    #[inline]
    pub(in crate::window) fn is_terminal_gpu_policy(&self) -> bool {
        self.terminal_loss.is_terminal()
    }

    #[inline]
    pub(in crate::window) fn gpu_loss_failed(&self) -> bool {
        self.terminal_loss.is_failed()
    }

    /// Samples the active GPU (including Windows D3D12 removal) and ensures terminal GPU teardown
    /// has run if the latch tripped. Returns `true` when this host is permanently latched.
    ///
    /// Fast path for `GpuLossPolicy::Recover`: single branch, zero atomics, returns `false`.
    pub(in crate::window) fn poll_gpu_loss(&mut self) -> bool {
        if !self.terminal_loss.is_terminal() {
            return false;
        }
        if let Some((gpu, _)) = self.gpu.as_ref() {
            if gpu.sample_removal() {
                // Already recorded DeviceRemoved on attached terminal latches.
            } else if gpu.is_lost() && !self.terminal_loss.is_failed() {
                if let Some(latch) = self.terminal_loss.latch().cloned() {
                    let _ = latch.record_device_lost(false, "the GPU device was lost");
                }
            }
        }
        if self.terminal_loss.is_failed() {
            self.enter_terminal_gpu_loss();
            return true;
        }
        false
    }

    /// Polls `gpu.device` once under `Terminal` so any synchronous validation or device-lost
    /// callback produced by encoding or submission runs before the next GPU boundary.
    pub(in crate::window) fn poll_device_for_terminal_loss(&mut self) -> bool {
        if !self.terminal_loss.is_terminal() {
            return false;
        }
        if let Some((gpu, _)) = self.gpu.as_ref() {
            rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
        }
        self.poll_gpu_loss()
    }

    /// Latches a terminal GPU failure (preserving any earlier first reason), marks the active GPU
    /// lost so shared overlays/siblings stop using it, and runs terminal teardown immediately.
    pub(in crate::window) fn latch_loss_latch(&mut self, kind: GpuFailureKind, reason: impl Into<String>) {
        if !self.terminal_loss.is_terminal() {
            return;
        }
        if let Some((gpu, _)) = self.gpu.as_ref() {
            gpu.mark_lost();
        }
        if let Some(latch) = self.terminal_loss.latch().cloned() {
            let _ = latch.record_failure(kind, reason);
        }
        self.enter_terminal_gpu_loss();
    }

    /// Single place carrying out the after-latch transition on a `GpuLossPolicy::Terminal` window:
    /// keeps an already-open window open and closable on its last presented picture (`self.win`
    /// stays `Some`, `self.life` stays `Shown`), drops its surface, GPU, and compositor GPU
    /// resources (`release_compositor_gpu`), clears `gpu_pool` if pointing at the failed GPU,
    /// stops pacing/timers/redraws, and cancels active input state without hiding the window.
    pub(in crate::window) fn enter_terminal_gpu_loss(&mut self) {
        if let Some(reason) = self.terminal_loss.reason() {
            self.shared.set_window_error(Some(reason));
        }
        if self.terminal_loss.entered {
            return;
        }
        self.terminal_loss.entered = true;
        if let Some((gpu, _)) = self.gpu.as_ref() {
            gpu.mark_lost();
            let clear_pool = self.gpu_pool.borrow().upgrade().is_some_and(|p| Arc::ptr_eq(&p, gpu));
            if clear_pool {
                *self.gpu_pool.borrow_mut() = Weak::default();
            }
        } else if self.gpu_pool.borrow().upgrade().is_some_and(|p| p.is_lost()) {
            *self.gpu_pool.borrow_mut() = Weak::default();
        }
        #[cfg(target_os = "windows")]
        self.on_resize_sync_event(crate::window::win32::resize_sync::Event::DeviceLost);
        if let Some(w) = self.win.as_mut() {
            w.stop_pacing();
            w.backoff_step(BackoffEvent::Lapsed);
            w.alpha = None;
            w.corners = None;
            w.presented = None;
            w.surface_ok = false;
            #[cfg(target_os = "macos")]
            { w.native.prime.stop(); }
            #[cfg(target_os = "windows")]
            {
                #[cfg(feature = "windows-system-material")]
                {
                    let native = &mut w.native;
                    native.materials.on_device_loss(native.mode.handover_mut());
                }
                w.native.video_layers.release(w.native.mode.handover());
                w.native.slot.reset();
                w.native.damage.reset();
                let _ = w.native.poll_pace.step(crate::window::poll_pace::PollPaceEvent::Reset);
            }
            drop(w.surface.take());
        }
        #[cfg(target_os = "macos")]
        self.frame_presented.reset();
        self.gpu = None;
        self.app.release_compositor_gpu();
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        self.app.rt_mut().paint.set_native_materials(false);

        // Cancel active input state (`wind_down` without hiding the window or changing `life`).
        self.app.rt_mut().keys_lost();
        self.secure_input.set(false);
        self.ime.clear();
        if let Some(w) = &self.win { w.window.set_ime_allowed(false); }
        self.app.set_live_visible(false);
        self.app.rt_mut().cancel_timer_frame();
        self.app.popup_cancel(&mut self.popup);
        self.app.file_dialog_cancel(&mut self.file_dialog);
        self.app.rt_mut().cancel_pointer();
        self.files = FileDrop::default();
        if let Some(w) = &mut self.win {
            w.apply_cursor_core(self.app.rt(), false);
            w.apply_pointer_lock_core(self.app.rt_mut());
        }
        self.mods = ModifiersState::empty();
        self.idle = true;
    }

    /// Drains updates, runs one `AppSpec::poll` (`poll_hidden`, no view rebuild/layout/draw) when
    /// woken, and applies non-GPU shell requests (`Tx::set_title`, `close_window`, `quit`,
    /// `set_color_scheme`, clipboard) while latched—never requesting a redraw or creating a GPU.
    pub(in crate::window) fn drain_terminal(&mut self, event_loop: &ActiveEventLoop, wake: bool) {
        let _ = self.clipboard.receive_core(self.app.rt_mut());
        let _ = self.app.popup_answer(&mut self.popup) | self.file_dialog.answer_core(self.app.rt_mut());
        let (requests, _, _) = self.app.drain_window(&self.shortcuts, &mut self.menus);
        self.clipboard.flush_core(self.app.rt_mut());
        if let Some(after) = requests.quit {
            let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
            self.app.popup_cancel(&mut self.popup);
            self.app.file_dialog_cancel(&mut self.file_dialog);
            event_loop.exit();
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(visible) = requests.dock {
            crate::platform::macos::set_dock_icon(visible);
        }
        if let Some(scheme) = requests.color_scheme {
            self.set_app_color_scheme(scheme);
        }
        if wake && !event_loop.exiting() {
            // Run `AppSpec::poll` even while `self.shown_win().is_some()` so the app can read
            // `AppHandle::window_error()`, update the window title, or quit with zero drawing.
            let _ = self.app.poll_hidden();
            self.clipboard.flush_core(self.app.rt_mut());
        }
        if let Some(position) = requests.position { self.set_position(event_loop, position); }
        if let Some(place) = requests.place { self.place(event_loop, place); }
        if let Some(request) = requests.window {
            match request {
                WindowRequest::Close | WindowRequest::Hide => self.window_request(event_loop, request),
                // Refused permanently once latched.
                WindowRequest::Show | WindowRequest::Open(_) => {}
            }
        }
        if requests.activation == Some(Activation::Focus) { self.focus(event_loop); }
        if requests.activation == Some(Activation::Minimize) {
            self.app.rt_mut().minimize_step(MinimizeEvent::Asked);
        }
        self.shell(event_loop);
        self.idle = true;
    }

    /// Handles `WindowEvent::KeyboardInput` while latched: never delivers keys to app view
    /// handlers, while preserving platform close/window-menu shortcuts (`Cmd+W` / `performClose:`
    /// via `native_menu::take_unhandled`, `Alt+Space` window menu on Windows).
    pub(in crate::window) fn handle_latched_keyboard(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        let Some(l) = self.win.as_ref() else { return };
        let logical = map_key_event(event, self.mods);
        let unhandled = crate::native_menu::take_unhandled(logical.as_ref()).or_else(|| {
            logical
                .as_ref()
                .filter(|ev| ev.is_window_menu_chord() && chrome::has_window_menu(&l.window))
                .map(|_| crate::runtime::Unhandled::WindowMenu)
        });
        match unhandled {
            Some(crate::runtime::Unhandled::Close) => self.close_requested(event_loop),
            Some(crate::runtime::Unhandled::Minimize) => self.minimize(),
            Some(crate::runtime::Unhandled::Menu(run)) => run(),
            Some(crate::runtime::Unhandled::WindowMenu) => {
                if let Some(w) = self.shown_win() {
                    chrome::window_menu(&w.window, crate::runtime::WindowMenu::Keyboard, None);
                }
            }
            None => {}
        }
    }

    /// Same-device surface recreation under `GpuLossPolicy::Terminal` when the GPU device itself
    /// is still healthy (`!gpu.is_lost() && !latch.is_failed()`) and only the surface failed.
    /// Keeps the exact same `Gpu`, `DeviceKey`, and `Renderer` without calling
    /// `release_compositor_gpu()`. Latches permanently if recreation or `configure_checked` fails.
    #[allow(unsafe_code)]
    pub(in crate::window) fn recreate_surface_same_device(&mut self) -> bool {
        if self.poll_gpu_loss() {
            return false;
        }
        let (Some(w), Some((gpu, _))) = (self.win.as_mut(), self.gpu.as_ref()) else {
            self.latch_loss_latch(GpuFailureKind::DeviceLost, "the GPU device is gone");
            return false;
        };
        if gpu.is_lost() {
            self.latch_loss_latch(GpuFailureKind::DeviceLost, "the GPU device was lost");
            return false;
        }
        let window = w.window.clone();
        w.alpha = None;
        w.corners = None;
        w.presented = None;
        #[cfg(target_os = "windows")]
        {
            w.native.slot.reset();
            w.native.damage.reset();
        }
        drop(w.surface.take());
        #[cfg(target_os = "windows")]
        let visual = w.native.mode.visual();
        let surface_res = {
            #[cfg(target_os = "windows")]
            if let Some(visual) = visual {
                // SAFETY: `w.native.mode` keeps the visual's `Handover` alive with this surface.
                unsafe { gpu.instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(visual)) }
                    .map_err(|e| e.to_string())
            } else {
                gpu.instance.create_surface(window.clone()).map_err(|e| e.to_string())
            }
            #[cfg(not(target_os = "windows"))]
            gpu.instance.create_surface(window.clone()).map_err(|e| e.to_string())
        };
        let surface = match surface_res {
            Ok(s) => s,
            Err(err) => {
                self.latch_loss_latch(GpuFailureKind::SurfaceFailed, format!("could not recreate surface: {err}"));
                return false;
            }
        };
        let raw_size = window.inner_size();
        let prev = self.win.as_ref().map(|w| (w.config.width, w.config.height));
        let srgb_blend = self.app.rt().srgb_blend();
        let frame_latency = crate::window::frame_latency(self.frame_latency);
        let transparent = self.options.transparent;
        let client_frame = self.client_frame(&window);
        let setup = || -> Result<wgpu::SurfaceConfiguration, SurfaceSetupError> {
            let Some((gpu, _)) = self.gpu.as_ref() else {
                return Err(SurfaceSetupError::unsupported("the GPU device is gone"));
            };
            let size = drawable_size(prev, (raw_size.width, raw_size.height), max_surface_dim(Some(gpu)));
            let (mut config, translucent) = surface_config(&surface, &gpu.adapter, size, srgb_blend)
                .ok_or_else(|| SurfaceSetupError::unsupported("the GPU adapter can't present to this window"))?;
            config.desired_maximum_frame_latency = frame_latency;
            if transparent || client_frame {
                config.alpha_mode = alpha_mode(transparent, client_frame, translucent);
            }
            configure_checked(&surface, gpu, &config)
                .map_err(|error| SurfaceSetupError::from_configure("the window's GPU surface refused its configuration", error))?;
            Ok(config)
        };
        let config = match setup() {
            Ok(cfg) => cfg,
            Err(err) => {
                drop(surface);
                let kind = if err.kind == crate::gpu::fallback::Failure::DeviceLost {
                    GpuFailureKind::DeviceLost
                } else {
                    GpuFailureKind::SurfaceFailed
                };
                self.latch_loss_latch(kind, err.message);
                return false;
            }
        };
        if self.poll_gpu_loss() {
            drop(surface);
            return false;
        }
        let target_format = if srgb_blend { config.format.remove_srgb_suffix() } else { config.format };
        let alpha = self.gpu.as_ref().and_then(|(gpu, _)| alpha_pass(gpu, transparent, &config, target_format));
        #[cfg(target_os = "macos")]
        crate::platform::macos::pin_contents_top_left(&surface, self.macos_window.color_space);
        #[cfg(target_os = "macos")]
        let present = self.win.as_ref()
            .and_then(|w| w.native.present_log.clone())
            .and_then(|log| crate::frame::present_time::metal::LayerHook::attach(&surface, log));
        let Some(w) = self.win.as_mut() else { return false };
        w.surface = Some(surface);
        w.config = config;
        w.surface_ok = true;
        w.alpha = alpha;
        w.backoff_step(BackoffEvent::Lapsed);
        #[cfg(target_os = "windows")]
        { w.native.slot.reset(); w.native.damage.reset(); w.native.mode.rebind(); }
        #[cfg(target_os = "macos")]
        { w.native.present = present; w.native.prime.stop(); }
        self.sync_window_backdrop(true);
        true
    }
}

/// Pure policy decision for `Host::replace_lost_surface` (testable without a window).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum LostSurfaceAction {
    /// Everything is healthy and configured: proceed to draw.
    Healthy,
    /// Default `GpuLossPolicy::Recover`: run bounded Chromium-style `SurfaceRetries` / `reattach`.
    Recover,
    /// `GpuLossPolicy::Terminal`, device healthy, surface missing/unconfigured: recreate surface
    /// on the exact same `Gpu`/`DeviceKey`/`Renderer` without calling `release_compositor_gpu`.
    RecreateSurfaceSameDevice,
    /// `GpuLossPolicy::Terminal`, device lost or latch already tripped: latch permanently and stop.
    LatchTerminal,
}

pub(in crate::window) const fn decide_lost_surface(
    policy: GpuLossPolicy,
    latch_failed: bool,
    gpu_lost: bool,
    surface_missing: bool,
) -> LostSurfaceAction {
    match policy {
        GpuLossPolicy::Recover => {
            if !gpu_lost && !surface_missing {
                LostSurfaceAction::Healthy
            } else {
                LostSurfaceAction::Recover
            }
        }
        GpuLossPolicy::Terminal => {
            if latch_failed || gpu_lost {
                LostSurfaceAction::LatchTerminal
            } else if surface_missing {
                LostSurfaceAction::RecreateSurfaceSameDevice
            } else {
                LostSurfaceAction::Healthy
            }
        }
    }
}

/// Classification of a `WindowEvent` while a `GpuLossPolicy::Terminal` window is latched and open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum LatchedEvent {
    Close,
    ModifiersChanged(ModifiersState),
    Keyboard,
    Drop,
}

pub(in crate::window) fn classify_latched_window_event(event: &WindowEvent) -> LatchedEvent {
    match event {
        WindowEvent::CloseRequested => LatchedEvent::Close,
        WindowEvent::ModifiersChanged(m) => LatchedEvent::ModifiersChanged(m.state()),
        WindowEvent::KeyboardInput { is_synthetic: false, .. } => LatchedEvent::Keyboard,
        _ => LatchedEvent::Drop,
    }
}

/// Pure policy decision for a `WakeEvent` while a `GpuLossPolicy::Terminal` window is latched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum LatchedWakeAction {
    DrainApp,
    CloseOrHide,
    NonGpuShell,
    Drop,
}

pub(in crate::window) fn classify_latched_wake_event(event: &crate::window::WakeEvent) -> LatchedWakeAction {
    use crate::window::WakeEvent;
    match event {
        WakeEvent::App(_) => LatchedWakeAction::DrainApp,
        WakeEvent::Window(_, WindowRequest::Close | WindowRequest::Hide) => LatchedWakeAction::CloseOrHide,
        WakeEvent::Focus(_)
        | WakeEvent::Position(_, _)
        | WakeEvent::Place(_, _)
        | WakeEvent::Level(_, _)
        | WakeEvent::ExcludeFromCapture(_, _)
        | WakeEvent::ColorScheme(_, _) => LatchedWakeAction::NonGpuShell,
        _ => LatchedWakeAction::Drop,
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.terminal_loss.prepare_clean_gpu_drop(self.gpu.as_ref().map(|(g, _)| &**g));
    }
}

#[cfg(test)]
#[path = "gpu_loss_tests.rs"]
mod tests;
