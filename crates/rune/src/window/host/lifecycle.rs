//! A window's lifecycle: requests from other threads, showing, focusing, draining updates,
//! polling while hidden, closing, hiding, opening, creating, sight cover transitions and rest.

use std::sync::atomic::Ordering;

use winit::dpi::PhysicalPosition;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;
use winit::window::WindowLevel;

use super::{Host, WindowApp};
use crate::app::{Activation, WindowRequest};
use crate::el::Shell;
use crate::window::chrome::{self, sync_chrome_core};
use crate::window::file_drop::FileDrop;
use crate::window::handle::native_window_number;
use crate::window::is_wayland;
use crate::window::state::minimize::{HiddenPoll, shell_take_core};
use crate::window::state::{
    Cover, Found, Life, MinimizeEvent, RestEffect, RestEvent, Reveal, RevealEffect, WakeFacts,
    WakeWork, rest,
};
use crate::window::surface::{BackoffEvent, Win};
#[cfg(target_os = "windows")]
use crate::window::win32::covered_draw::CoveredDrawEvent;

/// What `Host::shell` does with the pending shell, user-close, and minimize requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum ShellCloseAction {
    Quit(crate::quit::After),
    ProgrammaticClose,
    UserCloseRequest,
    Minimize,
    None,
}

/// Resolves the next shell action with precedence: `Quit`/`Relaunch` > programmatic
/// `Shell::Close` > user-initiated close request (`Tx::request_user_close`) > `Minimize`.
pub(in crate::window) fn take_shell_action(rt: &mut crate::runtime::RuntimeCore) -> ShellCloseAction {
    let minimize = shell_take_core(rt);
    let user_close = rt.take_user_close_request();
    match rt.shell_request.take() {
        Some(shell @ (Shell::Quit | Shell::Relaunch)) => {
            ShellCloseAction::Quit(shell.after().unwrap_or(crate::quit::After::Exit))
        }
        Some(Shell::Close) => ShellCloseAction::ProgrammaticClose,
        None if user_close => ShellCloseAction::UserCloseRequest,
        None if minimize => ShellCloseAction::Minimize,
        None => ShellCloseAction::None,
    }
}

/// What a host does after consulting [`crate::App::on_close_request`] for a native
/// `WindowEvent::CloseRequested`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum CloseDecision {
    /// Proceed to `Host::close` (no hook registered, hook returned `CloseResponse::Close`, the
    /// event loop is already exiting, or the window cannot close).
    Close,
    /// Keep the window open: clipboard has been flushed; apply `Host::shell` and request a
    /// redraw (or run a covered frame / schedule pending wakes) if the window stays open.
    Defer,
}

/// Evaluates [`crate::App::on_quit_request`] for an OS application-level quit request without
/// starting a quit or hiding the window. Flushes clipboard updates produced by the hook.
pub(in crate::window) fn quit_request_core(
    app: &mut dyn WindowApp,
    clipboard: &mut crate::clipboard::Client,
    finished: bool,
    exiting: bool,
) -> Option<crate::QuitResponse> {
    if finished || exiting {
        return None;
    }
    let response = app.run_on_quit_request()?;
    clipboard.flush_core(app.rt_mut());
    Some(response)
}

/// Evaluates [`crate::App::on_close_request`] for a native `WindowEvent::CloseRequested` without
/// hiding the window or changing `Life` or `FirstFrame`.
///
/// Bypasses the hook when the event loop is already exiting or the window would not close
/// (`!closes`), and clears `ends_app` on deferral so a later non-final close never mistakes
/// itself for a fast-exiting last window.
pub(in crate::window) fn close_request_core(
    app: &mut dyn WindowApp,
    clipboard: &mut crate::clipboard::Client,
    closes: bool,
    exiting: bool,
    ends_app: &mut bool,
) -> CloseDecision {
    if exiting || !closes {
        return CloseDecision::Close;
    }
    match app.run_on_close_request() {
        None | Some(crate::CloseResponse::Close) => CloseDecision::Close,
        Some(crate::CloseResponse::Defer) => {
            *ends_app = false;
            clipboard.flush_core(app.rt_mut());
            CloseDecision::Defer
        }
    }
}

/// Whether a window creation reveal should request foreground activation.
/// `App::open_inactive` suppresses activation on the first open only;
/// `WindowsWindowOptions::no_activate` suppresses activation on every reveal.
pub(in crate::window) const fn reveal_focus(reveal: Reveal, open_inactive: bool, shown: bool, no_activate: bool) -> bool {
    matches!(reveal, Reveal::Open) && !(open_inactive && !shown) && !no_activate
}

impl Host {
    pub(in crate::window) fn window_request(&mut self, event_loop: &ActiveEventLoop, request: WindowRequest) {
        match request {
            WindowRequest::Open(token) => self.open(event_loop, token),
            WindowRequest::Show => self.show(event_loop),
            WindowRequest::Hide => self.hide(event_loop),
            WindowRequest::Close => self.close(event_loop),
        }
    }

    pub(in crate::window) fn set_position(&mut self, event_loop: &ActiveEventLoop, position: (i32, i32)) {
        self.placement = None;
        self.options.position = Some(position);
        self.reopen.clear_position();
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_outer_position(PhysicalPosition::new(position.0, position.1));
        }
        self.sync_origin();
    }

    /// Whether this window must never take foreground activation on reveal.
    pub(in crate::window) fn no_activate(&self) -> bool {
        #[cfg(target_os = "windows")]
        return self.windows_window.no_activate;
        #[cfg(not(target_os = "windows"))]
        false
    }

    pub(in crate::window) fn set_always_on_top(&mut self, event_loop: &ActiveEventLoop, on: bool) {
        self.options.always_on_top = on;
        self.macos_window.level = None;
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_window_level(if on { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
            #[cfg(target_os = "windows")]
            crate::platform::win32::apply_window_styles(&w.window, &self.windows_window);
        }
    }

    pub(in crate::window) fn set_exclude_from_capture(&mut self, exclude: bool) {
        self.exclude_from_capture = Some(exclude);
        let Some(w) = &self.win else { return };
        #[cfg(target_os = "windows")]
        let _ = crate::platform::win32::set_exclude_from_capture(&w.window, exclude);
        #[cfg(target_os = "macos")]
        match &w.native.panel {
            Some(panel) => { let _ = panel.set_capture_exclusion(exclude); }
            None => crate::platform::macos::set_exclude_from_capture(&w.window, exclude),
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let _ = w;
    }

    pub(in crate::window) fn set_color_scheme(&mut self, scheme: crate::ColorScheme) {
        self.color_scheme = scheme;
        self.apply_window_color_scheme();
    }

    pub(in crate::window) fn apply_window_color_scheme(&mut self) {
        let target = crate::window::window_theme_for_platform(
            self.color_scheme,
            crate::window::app_color_scheme(),
            cfg!(target_os = "macos"),
        );
        let Some(w) = &self.win else { return };
        let color_scheme = self.color_scheme;
        let changed = crate::window::sync_window_theme(&mut self.applied_theme, target, |theme| {
            #[cfg(target_os = "windows")]
            if theme.is_none() {
                crate::platform::win32::dark_mode::refresh_color_policy();
            }
            w.window.set_theme(theme);
            #[cfg(target_os = "macos")]
            if let Some(panel) = &w.native.panel {
                panel.set_color_scheme(color_scheme);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = color_scheme;
        });
        if changed {
            self.sync_window_backdrop(true);
        }
    }

    pub(in crate::window) fn set_window_backdrop(&mut self, backdrop: crate::WindowBackdrop) {
        self.window_backdrop = backdrop;
        self.sync_window_backdrop(false);
    }

    /// Synchronizes the window's OS system backdrop and updates `Cx::window_backdrop()`.
    /// When `WindowBackdrop::None` has never been activated on this `HWND`, makes zero DWM calls.
    pub(in crate::window) fn sync_window_backdrop(&mut self, force_reapply: bool) {
        #[cfg(target_os = "windows")]
        {
            let requested = self.window_backdrop;
            let effective_dark = self.color_scheme.effective_dark(
                crate::window::app_color_scheme(),
                self.app.rt().env.system_dark,
            );
            let device_type = self.gpu.as_ref().map(|(gpu, _)| gpu.adapter.get_info().device_type);
            let transparent = self.options.transparent;
            let Some(w) = self.win.as_mut() else {
                let rt = self.app.rt_mut();
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(0);
                rt.set_backdrop_opaque_fallback(false);
                return;
            };
            let Some(hwnd) = crate::platform::win32::hwnd(&w.window) else {
                let rt = self.app.rt_mut();
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(0);
                rt.set_backdrop_opaque_fallback(
                    crate::platform::win32::backdrop::needs_opaque_fallback(
                        false,
                        transparent,
                        requested,
                        crate::WindowBackdrop::None,
                    ),
                );
                return;
            };
            let inputs = crate::platform::win32::backdrop::BackdropInputs {
                requested,
                build: crate::platform::win32::dark_mode::windows_build(),
                transparent,
                swapchain: crate::render::Gpu::dx12_swapchain_kind(),
                alpha_mode: w.surface.is_some().then_some(w.config.alpha_mode),
                cpu_painter: matches!(
                    w.native.mode,
                    crate::window::win32::handover::SurfaceMode::Cpu { .. }
                        | crate::window::win32::handover::SurfaceMode::HandingOver { .. }
                ),
                device_type,
            };
            let supported = crate::platform::win32::backdrop::windows_supported_mask(inputs);
            let outcome = w.native.backdrop.sync(
                hwnd as isize,
                inputs,
                false,
                force_reapply,
                Some(effective_dark),
                &mut crate::platform::win32::backdrop::RealBackdropSeam,
            );
            let rt = self.app.rt_mut();
            rt.set_window_backdrop(outcome.active);
            rt.set_window_backdrop_supported_mask(supported);
            rt.set_backdrop_opaque_fallback(outcome.opaque_fallback);
            if rt.stale() {
                w.request_redraw();
            }
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            let requested = self.window_backdrop;
            let transparent = self.options.transparent;
            let borderless = self.options.borderless;
            let wayland = self.wayland;
            let Some(w) = self.win.as_mut() else {
                let rt = self.app.rt_mut();
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(0);
                rt.set_wayland_blur_materials(false);
                rt.set_backdrop_opaque_fallback(false);
                return;
            };
            // Opaque windows and X11/XWayland sessions never bind Wayland blur globals.
            if !wayland || !transparent {
                let fallback = crate::platform::wayland::backdrop::policy::needs_opaque_fallback(
                    false,
                    transparent,
                    requested,
                    crate::WindowBackdrop::None,
                );
                let rt = self.app.rt_mut();
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(0);
                rt.set_wayland_blur_materials(false);
                rt.set_backdrop_opaque_fallback(fallback);
                if rt.stale() {
                    w.request_redraw();
                }
                return;
            }
            #[cfg(target_os = "linux")]
            let cpu_painter = matches!(
                w.native.mode,
                crate::window::linux::handover::SurfaceMode::Cpu { .. }
            );
            #[cfg(not(target_os = "linux"))]
            let cpu_painter = false;
            let alpha_mode = w.surface.is_some().then_some(w.config.alpha_mode);
            let translucent_gpu = !cpu_painter
                && matches!(alpha_mode, Some(m) if m != wgpu::CompositeAlphaMode::Opaque);
            // When WindowBackdrop::Blur is requested at creation or runtime, bind immediately so
            // the first presented frame is blurred; when WindowBackdrop::None is active before
            // frame 1, defer binding until `prepare_wayland_material_blur` (if materials mount)
            // or `run_on_ready`.
            if w.native.wayland_backdrop.is_none()
                && translucent_gpu
                && requested == crate::WindowBackdrop::Blur
            {
                w.native.wayland_backdrop =
                    crate::platform::wayland::backdrop::SurfaceBlur::for_window(&w.window);
            }
            let Some(blur) = w.native.wayland_backdrop.as_mut() else {
                let fallback = crate::platform::wayland::backdrop::policy::needs_opaque_fallback(
                    false,
                    transparent,
                    requested,
                    crate::WindowBackdrop::None,
                );
                let rt = self.app.rt_mut();
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(0);
                rt.set_wayland_blur_materials(false);
                rt.set_backdrop_opaque_fallback(fallback);
                if rt.stale() {
                    w.request_redraw();
                }
                return;
            };
            let scale = w.window.scale_factor() as f32;
            let csd_radius = if !borderless && w.config.alpha_mode != wgpu::CompositeAlphaMode::Opaque {
                winit::platform::wayland::WindowExtWayland::content_corner_radius(&*w.window)
            } else {
                0.0
            };
            let region_spec = crate::platform::wayland::backdrop::region::RegionSpec::from_surface(
                (w.config.width, w.config.height),
                scale,
                csd_radius,
            );
            let outcome = blur.sync_with_materials(
                requested,
                transparent,
                alpha_mode,
                cpu_painter,
                region_spec,
                self.app.rt().paint.wayland_material_blurs(),
                scale,
                false,
                force_reapply,
            );
            let rt = self.app.rt_mut();
            rt.set_window_backdrop(outcome.active);
            rt.set_window_backdrop_supported_mask(outcome.supported_mask);
            rt.set_wayland_blur_materials(outcome.material_blur_enabled);
            rt.set_backdrop_opaque_fallback(outcome.opaque_fallback);
            if rt.stale() || outcome.needs_commit {
                w.request_redraw();
            }
        }
        #[cfg(not(any(
            target_os = "windows",
            all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))
        )))]
        {
            let _ = force_reapply;
            self.app.rt_mut().set_window_backdrop(crate::WindowBackdrop::None);
        }
    }

    pub(in crate::window) fn set_app_color_scheme(&mut self, scheme: crate::ColorScheme) {
        crate::window::set_app_color_scheme_state(scheme);
        #[cfg(target_os = "macos")]
        crate::platform::macos::appearance::set_app_appearance(scheme);
        #[cfg(not(target_os = "macos"))]
        {
            self.apply_window_color_scheme();
            self.native_handle.send(|_| crate::window::WakeEvent::AppColorScheme);
        }
    }

    pub(in crate::window) fn show(&mut self, event_loop: &ActiveEventLoop) {
        if self.launcher() { self.reveal_launcher(event_loop); return; }
        self.reveal(event_loop, Reveal::Show, None);
    }

    pub(in crate::window) fn open(&mut self, event_loop: &ActiveEventLoop, token: Option<String>) {
        if self.launcher() { self.reveal_launcher(event_loop); return; }
        // The launcher's startup token belongs to the first window only; else a handed-on launch's.
        let token = crate::open_url::instance::handed_token(token.or_else(|| self.startup_token(event_loop)));
        self.reveal(event_loop, Reveal::Open, token);
    }

    /// The window shown, or not, so far: the shown one, and nothing while it's hidden.
    pub(in crate::window) fn shown_win(&self) -> Option<&Win> { self.win.as_ref().filter(|_| self.life.shown()) }

    /// The window as a reveal finds it.
    fn found(&self) -> Found {
        let Some(w) = &self.win else { return Found::Missing };
        let minimized = w.is_minimized_native();
        match self.life {
            Life::Shown { sight } => Found::Shown { covered: sight.covered(), minimized },
            _ => Found::Hidden { minimized },
        }
    }

    /// Brings the window on screen (`Reveal`); a window closed for good stays closed.
    pub(in crate::window) fn reveal(&mut self, event_loop: &ActiveEventLoop, reveal: Reveal, mut token: Option<String>) {
        if self.life.finished() { return; }
        let mut built = false;
        for effect in reveal.effects(self.found()) {
            match effect {
                RevealEffect::ReadSystem => {
                    let rt = self.app.rt_mut();
                    rt.set_system_accent(crate::system_accent());
                    rt.set_system_scroll_bars_state(crate::platform::scroll_bars::current().with_override(self.args.scroll_bars));
                    #[cfg(target_os = "linux")]
                    {
                        rt.set_system_reduced_motion(crate::platform::preferences::reduced_motion());
                        rt.double_click_interval = crate::platform::preferences::double_click_interval();
                    }
                }
                RevealEffect::Build => built = self.build_for_show(),
                RevealEffect::ApplyFrame => {
                    if self.take_frame_request(self.frame_secs(crate::time::now()), false) {
                        self.idle = false;
                    }
                }
                RevealEffect::Place => self.apply_placement(event_loop),
                RevealEffect::Create => {
                    let focus = reveal_focus(reveal, self.open_inactive, self.shown, self.no_activate());
                    self.create(event_loop, token.take(), focus);
                    // On macOS a new window becomes key without activating the app, so activate
                    // it like a reopen does. The launch open leaves activation to the platform.
                    #[cfg(target_os = "macos")]
                    if focus && self.started && let Some(w) = self.shown_win() { w.focus(); }
                }
                RevealEffect::OrderIn { focus } => {
                    let focus = focus && !self.no_activate();
                    if let Some(w) = &mut self.win { w.show(focus); }
                    #[cfg(target_os = "windows")]
                    self.sync_window_backdrop(true);
                }
                RevealEffect::Unminimize => if let Some(w) = &self.win && !w.is_panel() { w.window.set_minimized(false) },
                RevealEffect::Settle => {
                    if !built { self.app.rt_mut().settle_all(); }
                    self.idle = true;
                }
                RevealEffect::Shown { live } => {
                    self.life.show();
                    self.app.set_live_visible(live);
                }
                RevealEffect::CancelTimerFrame => self.app.rt_mut().cancel_timer_frame(),
                RevealEffect::Number => if let Some(w) = &self.win {
                    self.native_handle.native.number.store(native_window_number(&w.window).unwrap_or(0), Ordering::Release);
                },
                RevealEffect::Focus => if !self.no_activate() { self.activate(event_loop, token.take()); },
                RevealEffect::Redraw => if let Some(w) = &self.win { w.request_redraw() },
                // No settle, frame, surface configure or acquisition on a cached reveal, unless
                // the refreshed frame left motion running.
                RevealEffect::RedrawIfMoving => if !self.idle && let Some(w) = &self.win { w.request_redraw() },
            }
        }
    }

    /// The first window's startup token (`XDG_ACTIVATION_TOKEN`, `DESKTOP_STARTUP_ID`), which
    /// later windows don't inherit.
    fn startup_token(&self, event_loop: &ActiveEventLoop) -> Option<String> {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if !self.shown {
            use winit::platform::startup_notify::{EventLoopExtStartupNotify, reset_activation_token_env};
            let token = event_loop.read_token_from_env().map(|t| t.into_raw());
            reset_activation_token_env();
            return token;
        }
        let _ = event_loop;
        None
    }

    /// Focuses a shown window; Wayland and X11 use the open's startup token for that.
    fn activate(&mut self, event_loop: &ActiveEventLoop, token: Option<String>) {
        let Some(w) = self.win.as_mut() else { return };
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if is_wayland(event_loop) {
            match token {
                Some(token) => {
                    if w.native.activator.is_none() {
                        w.native.activator = crate::platform::activation::Activator::new(&w.window);
                    }
                    if let Some(a) = w.native.activator.as_mut() {
                        a.activate(&token);
                    }
                }
                // Without a token the compositor may only mark the window urgent.
                None => w.window.request_user_attention(Some(winit::window::UserAttentionType::Informational)),
            }
            return;
        } else if let Some(token) = token.as_deref()
            && crate::platform::x11::activation::activate(&w.window, token)
        {
            return;
        }
        let _ = (event_loop, token);
        w.focus();
    }

    /// A hidden or released window that fits its content builds what changed while it was
    /// away and takes its new height before it is ordered in (`Runtime::build_for_show`).
    /// Returns whether it built, settled.
    pub(in crate::window) fn build_for_show(&mut self) -> bool {
        if !self.app.build_for_show() { return false; }
        self.clipboard.flush_core(self.app.rt_mut());
        // macOS sets the frame and resizes the surface at once; elsewhere `Resized` brings it.
        self.take_frame_request(0.0, false);
        true
    }

    pub(in crate::window) fn focus(&self, event_loop: &ActiveEventLoop) {
        let Some(w) = self.shown_win() else { return };
        if w.is_minimized_native() { w.window.set_minimized(false); }
        if is_wayland(event_loop) {
            w.window.request_user_attention(Some(winit::window::UserAttentionType::Informational));
            return;
        }
        w.focus();
    }

    /// Applies everything other threads asked for since the last wake-up. A `wake` polls a
    /// hidden window first, so a reveal in the same turn sees what `poll` changed.
    pub(in crate::window) fn drain(&mut self, event_loop: &ActiveEventLoop, wake: bool) {
        let shown = self.shown_win().is_some();
        let stale_before = self.app.rt().needs_commit();
        let clipboard = self.clipboard.receive_core(self.app.rt_mut());
        let answered = self.app.popup_answer(&mut self.popup) | self.file_dialog.answer_core(self.app.rt_mut());
        let (requests, had_updates, updated) = self.app.drain_window(&self.shortcuts, &mut self.menus);
        let changed = clipboard || answered || updated || self.app.rt().drag_ends_pending();
        // Only updates that changed nothing (`AppHandle::update_if`), or an empty startup drain
        // after `try_create` already drew the first frame, ran: no poll, no frame.
        let quiet = (had_updates || (!wake && !stale_before)) && !changed && requests.only_updates()
            && !self.app.rt().patch_pending() && (stale_before || !self.app.rt().needs_commit()) && !self.app.compositor_animating();
        self.clipboard.flush_core(self.app.rt_mut());
        // The next frame polls for the wake.
        if wake && !quiet { self.app.rt_mut().cancel_timer_frame(); }
        if let Some(after) = requests.quit {
            // Quitting from now on: a close later in this turn closes for the quit.
            // `Windows::exiting` carries out its effects.
            let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
            // A tracking menu holds the event loop until it ends.
            self.app.popup_cancel(&mut self.popup);
            self.app.file_dialog_cancel(&mut self.file_dialog);
            if !crate::quit::programmatic_quit() {
                event_loop.exit();
            }
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(visible) = requests.dock {
            crate::platform::macos::set_dock_icon(visible);
        }
        if let Some(scheme) = requests.color_scheme {
            self.set_app_color_scheme(scheme);
        }
        let shows = matches!(requests.window, Some(WindowRequest::Show | WindowRequest::Open(_)));
        let polled = wake && !quiet && self.poll_hidden(event_loop, shows);
        if let Some(position) = requests.position { self.set_position(event_loop, position); }
        if let Some(place) = requests.place { self.place(event_loop, place); }
        if let Some(request) = requests.window { self.window_request(event_loop, request); }
        if requests.activation == Some(Activation::Focus) { self.focus(event_loop); }
        // After the show it may follow; `focus` and `minimize` are last-wins (`AppHandle`).
        if requests.activation == Some(Activation::Minimize) { self.app.rt_mut().minimize_step(MinimizeEvent::Asked); }
        self.shell(event_loop);
        // A pop-up or file dialog a quiet update asked for may have been answered at once.
        let answered = quiet && ((!stale_before && self.app.rt().needs_commit()) || self.app.rt().has_pending_input());
        if quiet { self.clipboard.flush_core(self.app.rt_mut()); }
        let (changed, quiet) = (changed || answered, quiet && !answered);
        // A launcher's cached reveal draws nothing; a hidden one renders its changes hidden,
        // paced (`refresh_hidden`). A covered or minimised one runs them without asking for a
        // frame it wouldn't draw.
        let work = WakeWork::of(
            WakeFacts {
                window: self.win.is_some(),
                launcher: self.launcher(),
                visible: self.life.shown(),
                was_shown: shown,
                changed,
                quiet,
                covered: self.life.covered(),
            },
            || self.win.as_ref().is_some_and(Win::minimized),
        );
        let framed = match work {
            WakeWork::Nothing => false,
            WakeWork::HiddenFrame => self.refresh_hidden(),
            WakeWork::CoveredFrame => {
                self.covered_frame();
                self.shell(event_loop);
                true
            }
            WakeWork::Paint => {
                if let Some(w) = &self.win { w.request_redraw(); }
                true
            }
        };
        // Neither a poll nor a frame comes to schedule the updates' `wake_after`s.
        if !polled && !framed { self.app.rt_mut().schedule_pending_wakes(); }
    }

    /// A hidden, keep-running or windowless window still reads its inbox when woken: `poll` runs
    /// with no drawable, present or animation, and its changes render when the window is next
    /// shown. A hidden launcher renders poll's changes hidden, paced, keeping its cached frame
    /// fresh. A minimize waits for a show in the same turn (`Minimize`). Returns whether it
    /// polled.
    pub(in crate::window) fn poll_hidden(&mut self, event_loop: &ActiveEventLoop, shows: bool) -> bool {
        if event_loop.exiting() || self.shown_win().is_some() { return false; }
        let changed = self.app.poll_hidden();
        self.clipboard.flush_core(self.app.rt_mut());
        // A show this turn minimizes after it: this poll holds the ask across its own shell, and
        // a callback's ask during that shell is the one it takes first.
        let poll = HiddenPoll::new(shows);
        poll.enter_core(self.app.rt_mut());
        self.shell(event_loop);
        poll.leave_core(self.app.rt_mut());
        if changed && self.launcher() { self.refresh_hidden(); }
        true
    }

    /// A hidden launcher renders what changed into its cached frame: at once after a quiet
    /// spell, then at most every `HIDDEN_REFRESH` (`HiddenPace`). A show of state newer than
    /// that frame renders it hidden first (`reveal_launcher`), as for an update in the same
    /// turn. A frame being prepared, or one a reveal waits on, renders at once. Returns whether
    /// it asked for a frame.
    pub(in crate::window) fn refresh_hidden(&mut self) -> bool {
        let now_due = self.hidden_launcher_renders();
        let Some(w) = &mut self.win else { return false };
        let render = now_due || w.first_frame.deadline().is_some() || w.hidden_pace.changed(crate::time::now());
        if render { w.request_redraw(); }
        render
    }

    /// What a hidden launcher's frame must render as it commits: queued input and drag ends.
    /// Committed without a render, they'd leave a cached frame that looks fresh.
    pub(in crate::window) fn hidden_launcher_renders(&self) -> bool {
        self.app.rt().has_pending_input() || self.app.rt().drag_ends_pending()
    }

    /// Carries out what a handler or update asked with `Tx::close_window` / `Tx::quit`, then
    /// opens the pop-up menu it asked for, if the window is still there to open it over.
    pub(in crate::window) fn shell(&mut self, event_loop: &ActiveEventLoop) {
        self.apply_tx_macos_document();
        self.show_title();
        self.apply_tx_color_scheme();
        self.apply_tx_window_backdrop();
        if self.app.rt_mut().take_toggle_maximized() && let Some(w) = self.shown_win() {
            w.toggle_maximized();
            #[cfg(target_os = "windows")]
            crate::platform::win32::apply_window_styles(&w.window, &self.windows_window);
        }
        if let Some(req) = self.app.rt_mut().take_full_screen_request() && let Some(w) = self.shown_win() {
            match req {
                crate::el::FullScreenRequest::Toggle => w.toggle_full_screen(),
                crate::el::FullScreenRequest::Set(on) => w.set_full_screen(on),
            }
            #[cfg(target_os = "windows")]
            crate::platform::win32::apply_window_styles(&w.window, &self.windows_window);
        }
        if let Some(menu) = self.app.rt_mut().take_window_menu() && let Some(w) = self.shown_win() {
            let rt = self.app.rt();
            let at = (menu == crate::runtime::WindowMenu::Pointer)
                .then(|| rt.pointer().map(|(x, y)| (x * rt.zoom(), y * rt.zoom()))).flatten();
            chrome::window_menu(&w.window, menu, at);
        }
        // A new `Cx::set_titlebar` or `Cx::titlebar_height` resizes the caption buttons and the band
        // the frame hit-tests, and moves the traffic lights.
        if self.app.rt_mut().take_titlebar_changed() && let Some(w) = &self.win {
            sync_chrome_core(self.app.rt_mut(), w, self.chrome, self.titlebar);
            if self.app.rt().stale() { w.request_redraw(); }
        }
        for _ in 0..8 {
            match take_shell_action(self.app.rt_mut()) {
                ShellCloseAction::Quit(after) => {
                    let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
                    self.app.popup_cancel(&mut self.popup);
                    self.app.file_dialog_cancel(&mut self.file_dialog);
                    if !crate::quit::programmatic_quit() {
                        event_loop.exit();
                    }
                }
                ShellCloseAction::ProgrammaticClose => self.close(event_loop),
                ShellCloseAction::UserCloseRequest => self.close_requested(event_loop),
                ShellCloseAction::Minimize => self.minimize(),
                ShellCloseAction::None => {}
            }
            if self.life.finished() || event_loop.exiting() {
                self.app.popup_cancel(&mut self.popup);
                self.app.file_dialog_cancel(&mut self.file_dialog);
                return;
            }
            let paints = self.shown_win().is_some_and(|w| !self.life.covered() && !w.minimized());
            // A painting macOS window moves its frame over its next frames (`redraw`).
            if self.app.rt().host.window_frame.is_some() && !(paints && cfg!(target_os = "macos")) {
                let resized = self.take_frame_request(self.frame_secs(crate::time::now()), false);
                if resized && !self.life.shown() && self.launcher() && let Some(w) = &mut self.win {
                    // The hidden surface resized, invalidating its old-size cached pixels; draw at
                    // the new height before any pending reveal orders it in.
                    w.first_frame.missed();
                    w.request_redraw();
                }
            }
            let shown = self.win.as_ref().filter(|_| self.life.shown());
            let window = shown.map(|w| &w.window);
            if self.app.popup_open(&mut self.popup, window, paints) && let Some(w) = shown { w.request_redraw(); }
            if self.file_dialog.open_core(self.app.rt_mut(), window.map(|w| &**w)) && let Some(w) = shown { w.request_redraw(); }
            // A callback that ran (a menu that couldn't open) may have asked to close, quit or
            // minimize.
            if self.app.rt().shell_request.is_none()
                && !self.app.rt().has_user_close_request()
                && !self.app.rt().host.minimize.field()
            {
                return;
            }
        }
    }

    /// Handles a native `WindowEvent::CloseRequested`. Runs [`App::on_close_request`] inside a
    /// normal update transaction before hiding the window or changing `Life`/`FirstFrame`;
    /// `CloseResponse::Defer` keeps the window open, applies any `Tx` requests made in the hook,
    /// and schedules a redraw (or covered frame / pending wakes).
    pub(in crate::window) fn close_requested(&mut self, event_loop: &ActiveEventLoop) {
        let closes = self.closes();
        let decision = close_request_core(
            &mut *self.app,
            &mut self.clipboard,
            closes,
            event_loop.exiting(),
            &mut self.ends_app,
        );
        match decision {
            CloseDecision::Close => self.close(event_loop),
            CloseDecision::Defer => {
                self.shell(event_loop);
                if self.life.finished() || event_loop.exiting() {
                    return;
                }
                if let Some(w) = self.shown_win() {
                    if !self.life.covered() && !w.minimized() {
                        w.request_redraw();
                    } else {
                        self.covered_frame();
                        self.shell(event_loop);
                    }
                } else {
                    self.app.rt_mut().schedule_pending_wakes();
                }
            }
        }
    }

    /// Handles an OS application-level quit request on this host. Runs [`App::on_quit_request`]
    /// inside a normal update transaction, applies any `Tx` shell requests (for example `Tx::quit`
    /// or `Tx::show_window`), and on `QuitResponse::Defer` requests a redraw (or covered frame /
    /// pending wakes) so a confirmation dialog draws immediately once AppKit leaves modal mode.
    pub(in crate::window) fn quit_request_in_turn(&mut self, event_loop: &ActiveEventLoop) -> Option<crate::QuitResponse> {
        let response = quit_request_core(
            &mut *self.app,
            &mut self.clipboard,
            self.life.finished(),
            event_loop.exiting(),
        )?;
        self.shell(event_loop);
        if response == crate::QuitResponse::Defer
            && !self.life.finished()
            && !event_loop.exiting()
            && !crate::quit::quitting()
        {
            if let Some(w) = self.shown_win() {
                if !self.life.covered() && !w.minimized() {
                    w.request_redraw();
                } else {
                    self.covered_frame();
                    self.shell(event_loop);
                }
            } else {
                self.app.rt_mut().schedule_pending_wakes();
            }
        }
        Some(response)
    }

    pub(in crate::window) fn close(&mut self, event_loop: &ActiveEventLoop) {
        self.app.rt_mut().minimize_step(MinimizeEvent::Lost);
        if let Some(w) = &mut self.win {
            w.first_frame.hide();
            #[cfg(target_os = "macos")]
            { w.native.prime.stop(); }
        }
        self.secure_input.set(false);
        if !self.closes() { return; }
        #[cfg(target_os = "windows")]
        self.on_resize_sync_event(crate::window::win32::resize_sync::Event::Destroy);
        crate::marks::mark("window.close");
        // The last window of an app that ends fast goes as a bare window's close does: its hook,
        // then the window destroyed at once, with no hide before it. The quit runs after it and
        // the process ends without dropping the window.
        let ends = std::mem::take(&mut self.ends_app) && !self.keep_running && cfg!(target_os = "windows");
        // Snapshot live state before hiding so X11 unmapping or Win32 SW_HIDE cannot clear
        // maximized/full-screen state before `hide` destroys the window.
        if self.keep_running {
            self.record_live_window_state();
        }
        // Off the screen first: the close looks done at once and `on_close` runs unseen.
        if !ends && let Some(w) = self.shown_win() { w.hide(); }
        crate::marks::mark("window.close.hidden");
        self.close_hook();
        crate::marks::mark("window.close.hooked");
        #[cfg(target_os = "windows")]
        if ends && let Some(w) = &mut self.win {
            #[cfg(feature = "windows-system-material")]
            {
                let native = &mut w.native;
                native.materials.teardown(native.mode.handover_mut());
            }
            w.native.drop_target = None;
            crate::platform::win32::destroy_window_now(&w.window);
            crate::marks::mark("window.destroyed");
        }
        self.app.set_live_visible(false);
        // While the window still shows its last frames.
        if let Some(trace) = &mut self.trace { trace.settle_presents(crate::frame::present_time::SETTLE); }
        #[cfg(target_os = "macos")]
        if self.keep_running {
            self.frame_presented.drain();
        } else {
            self.frame_presented.reset();
        }
        if let Some(after) = self.app.rt_mut().shell_request.take().and_then(Shell::after) {
            let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
            self.app.popup_cancel(&mut self.popup);
            self.app.file_dialog_cancel(&mut self.file_dialog);
            if !crate::quit::programmatic_quit() {
                event_loop.exit();
            }
            return;
        }
        if !self.keep_running {
            self.app.popup_cancel(&mut self.popup);
            self.app.file_dialog_cancel(&mut self.file_dialog);
            self.app.rt_mut().cancel_pointer();
            self.files = FileDrop::default();
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            self.wayland_drop_closed();
            if !ends && let Some(w) = &mut self.win { w.apply_cursor_core(self.app.rt(), false); w.apply_pointer_lock_core(self.app.rt_mut()); w.hide(); }
            crate::marks::mark("window.hidden");
            self.life.finish();
            return;
        }
        self.hide_window(event_loop, true);
    }

    /// Whether a close runs `on_close` (`Life::closes`).
    pub(in crate::window) fn closes(&self) -> bool { self.life.closes(self.keep_running, self.win.is_some()) }

    pub(in crate::window) fn close_hook(&mut self) {
        self.app.run_on_close();
        // Before the window hides or the app exits: a lock or clear must reach the worker.
        self.clipboard.flush_core(self.app.rt_mut());
    }

    pub(in crate::window) fn hide(&mut self, event_loop: &ActiveEventLoop) {
        self.hide_window(event_loop, false);
    }

    fn hide_window(&mut self, event_loop: &ActiveEventLoop, already_hidden: bool) {
        if !already_hidden {
            self.record_live_window_state();
        }
        self.rest_step(RestEvent::Unseen);
        self.stop_showing();
        self.wind_down();
        let launcher = self.launcher();
        if self.release_gpu && !launcher {
            self.release();
        } else if is_wayland(event_loop) {
            // Wayland has no hidden toplevel: unmap by destroying, recreate on open.
            self.win = None;
            self.native_handle.native.ready.store(false, Ordering::Release);
            self.native_handle.native.number.store(0, Ordering::Release);
            let rt = self.app.rt_mut();
            rt.set_window_backdrop(crate::WindowBackdrop::None);
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            {
                rt.set_window_backdrop_supported_mask(0);
                rt.set_wayland_blur_materials(false);
                rt.set_backdrop_opaque_fallback(false);
            }
        } else if let Some(w) = self.win.as_mut() {
            w.stop_pacing();
            #[cfg(target_os = "windows")]
            let _ = w.native.poll_pace.step(crate::window::poll_pace::PollPaceEvent::Reset);
            w.backoff_step(BackoffEvent::Lapsed);
            w.presented = None;
            // Its hidden frame delivers the focus loss.
            if launcher { w.request_redraw(); }
        }
    }

    /// What a window does just before it stops drawing: it leaves `Life` (a window that hid can
    /// be reopened), drops a pending minimize, lands a frame move where it was going, and (macOS)
    /// delivers the Fn key change a hide need not report. No event loop is needed, so a window
    /// whose surface can't be replaced does the same before `wind_down`
    /// (`Host::replace_lost_surface`).
    pub(in crate::window) fn stop_showing(&mut self) {
        self.life.hide(crate::platform::reopen::stamp);
        self.app.rt_mut().minimize_step(MinimizeEvent::Lost);
        self.land_frame();
        // Before the window can go: a hide need not report a blur. `about_to_wait` delivers it,
        // after any Fn change the monitor saw first.
        #[cfg(target_os = "macos")]
        {
            self.fn_key();
            self.media_keys();
        }
    }

    /// What a window stops doing when it hides: keys released, secure input and IME off, popups
    /// and file dialogs cancelled, the pointer cancelled, drops dropped, modifiers reset, the
    /// cursor back to normal, and the native window hidden. No event loop is needed, so a window
    /// that loses its surface does the same from `draw` (`Host::replace_lost_surface`).
    pub(in crate::window) fn wind_down(&mut self) {
        #[cfg(target_os = "windows")]
        self.on_resize_sync_event(crate::window::win32::resize_sync::Event::Hide);
        self.app.rt_mut().keys_lost();
        if let Some(w) = &mut self.win {
            w.first_frame.hide();
            #[cfg(target_os = "macos")]
            { w.native.prime.stop(); }
        }
        #[cfg(target_os = "windows")]
        self.look_stale();
        self.secure_input.set(false);
        self.ime.clear();
        if let Some(w) = &self.win { w.window.set_ime_allowed(false); }
        self.app.set_live_visible(false);
        self.app.rt_mut().cancel_timer_frame();
        self.app.popup_cancel(&mut self.popup);
        self.app.file_dialog_cancel(&mut self.file_dialog);
        self.app.rt_mut().cancel_pointer();
        self.files = FileDrop::default();
        if let Some(w) = &mut self.win { w.apply_cursor_core(self.app.rt(), false); w.apply_pointer_lock_core(self.app.rt_mut()); }
        self.mods = ModifiersState::empty();
        if let Some(w) = &self.win { w.hide(); }
    }

    /// Gives up the window and the device and surface it drew with, as `hide` does for an app
    /// that releases its GPU when closed. The next `open` makes a window, a device and a surface
    /// (an app that sees `AppHandle::window_error` and the window gone can ask for one).
    pub(in crate::window) fn release(&mut self) {
        #[cfg(target_os = "windows")]
        self.on_resize_sync_event(crate::window::win32::resize_sync::Event::Destroy);
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        if let Some(w) = self.win.as_mut() {
            let native = &mut w.native;
            native.materials.teardown(native.mode.handover_mut());
        }
        self.win = None;
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        self.app.rt_mut().paint.set_native_materials(false);
        #[cfg(target_os = "macos")]
        self.frame_presented.reset();
        self.native_handle.native.ready.store(false, Ordering::Release);
        self.native_handle.native.number.store(0, Ordering::Release);
        self.gpu = None;
        self.app.release_compositor_gpu();
        let rt = self.app.rt_mut();
        rt.set_window_backdrop(crate::WindowBackdrop::None);
        #[cfg(any(
            target_os = "windows",
            all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))
        ))]
        {
            rt.set_window_backdrop_supported_mask(0);
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            rt.set_wayland_blur_materials(false);
            rt.set_backdrop_opaque_fallback(false);
        }
        rt.text.atlas.invalidate();
        rt.images.invalidate();
    }

    /// Creates the window. When the first window can't be created and the app didn't ask to stay
    /// alive without one (`keep_running` / `windowless`), finishes this host with the error so
    /// `run` / `try_run` ends with it; otherwise logs it and leaves the app windowless so a later
    /// open can retry.
    pub(in crate::window) fn create(&mut self, event_loop: &ActiveEventLoop, token: Option<String>, focus: bool) {
        let result = self.try_create(event_loop, token, focus);
        self.finish_create(result);
    }

    pub(crate) fn finish_create(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => self.shared.set_window_error(None),
            Err(error) => {
                let key = self.native_handle.key();
                self.shared.set_window_error(Some(error.clone()));
                if self.life.fail_create(self.keep_running, self.shown) {
                    self.startup_error = Some(format!("could not create the window {key}: {error}"));
                } else {
                    eprintln!("rune: could not create the window {key}: {error}");
                }
            }
        }
    }

    /// `cover` starts or stops covering the window. Covered, it stops pacing and rests, and is
    /// no longer live; uncovered, its first frame paints the settled state of what changed
    /// meanwhile (guarantee 8).
    pub(in crate::window) fn set_cover(&mut self, cover: Cover, on: bool) {
        let Some(l) = self.win.as_mut() else { return };
        #[cfg(target_os = "macos")]
        { l.native.prime.stop(); }
        let was = self.life.cover(cover, on);
        l.backoff_step(BackoffEvent::Uncovered);
        l.presented = None;
        let live = self.life.uncovered() && !l.minimized();
        if self.life.covered() {
            self.secure_input.set(false);
            l.stop_pacing();
            #[cfg(target_os = "windows")]
            let _ = l.native.poll_pace.step(crate::window::poll_pace::PollPaceEvent::Reset);
            self.idle = true;
            self.app.set_live_visible(live);
            self.app.rt_mut().cancel_timer_frame();
            self.rest_step(RestEvent::Unseen);
            return;
        }
        // Its first frame uncovered draws what a covered one owed.
        #[cfg(target_os = "windows")]
        { l.covered_draw = l.covered_draw.step(CoveredDrawEvent::Drawn); }
        self.app.set_live_visible(live);
        let Some(l) = self.win.as_mut() else { return };
        sync_chrome_core(self.app.rt_mut(), l, self.chrome, self.titlebar);
        // A window opened while the displays slept has no display link yet.
        #[cfg(target_os = "macos")]
        if l.native.pacing.is_none() {
            l.native.pacing = crate::platform::display_link::DisplayLink::new(&l.window, self.frame_wake.clone())
                .map(|p| p.with_ambient_period(self.ambient_period));
        }
        if was {
            self.app.rt_mut().settle_all();
            l.request_redraw();
        } else if self.app.rt().stale() {
            l.request_redraw();
        }
    }

    pub(in crate::window) fn rest_step(&mut self, event: RestEvent) {
        for effect in rest::step(&mut self.rest, event) {
            match effect {
                RestEffect::GiveBackUnused => self.give_back(false),
                RestEffect::GiveBackAll => self.give_back(true),
            }
        }
    }

    fn give_back(&mut self, all: bool) {
        {
            let rt = self.app.rt_mut();
            rt.text.rest();
            rt.images.rest();
            if let Some(rewind) = rt.motion.rewind() { rewind.rest(); }
        }
        self.app.run_on_rest();
        if let Some((gpu, renderer)) = &mut self.gpu {
            // Poll once (non-blocking) at idle rest only if recyclable oversized chunks are in
            // flight so their completed buffers return before `give_back(false)` trims them;
            // ordinary windows skip polling to avoid the 4-9 ms Iris Xe post-poll penalty.
            if !all && renderer.has_oversized_uploads_in_flight() {
                rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
            }
            let rt = self.app.rt_mut();
            renderer.give_back(all, &mut rt.text.atlas.resident_caches, &rt.images);
            // Out of sight, what the last frame used goes too, and the staging chunks: an empty
            // submission and a wait land those frees now.
            if all {
                gpu.queue.submit(std::iter::empty());
                rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok();
                renderer.trim_uploads();
            }
        }
    }
}
