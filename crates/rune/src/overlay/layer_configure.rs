//! Non-blocking initial-configure lifecycle for Wayland `zwlr_layer_shell_v1` overlays.
//!
//! `LayerSurface::new` commits (bufferless) and flushes without waiting for the compositor's
//! first `configure`. While configuring, the overlay owns the `LayerSurface` here with no
//! `wgpu::Surface`, stays live for winit's `wake_on_wayland_read`, draws nothing, and ignores
//! no other host's frames or timers. When `configure` lands on a normal event-loop turn, the
//! overlay attaches its GPU surface at the configured size and scale and draws immediately;
//! `closed`, bound-output removal, a dead display connection, or hiding/recreating ends the
//! wait cleanly.

use super::*;
use crate::machine::Effects;

/// Where a Wayland layer-shell overlay sits between initial bufferless commit and its first
/// compositor `configure`.
#[cfg_attr(
    not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
    allow(dead_code)
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Phase {
    /// No Wayland layer surface is waiting for its initial `configure`.
    #[default]
    Idle,
    /// A `LayerSurface` has sent its initial bufferless commit and is waiting for its first
    /// `zwlr_layer_surface_v1.configure`. `scale` starts as the target monitor's integer scale
    /// and updates if `wl_surface.preferred_buffer_scale` arrives before `configure`.
    Configuring { scale: i32 },
}

#[cfg_attr(
    not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
    allow(dead_code)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    /// A new unconfigured `LayerSurface` was created and committed with initial monitor `scale`.
    Started { scale: i32 },
    /// `wl_surface.preferred_buffer_scale` arrived before the initial `configure`.
    Scale(i32),
    /// The initial `zwlr_layer_surface_v1.configure` arrived (with optional scale from the same
    /// dispatch).
    Configured { width: u32, height: u32, scale: Option<i32> },
    /// The compositor sent `closed` on a live output or the display connection failed fatally
    /// before the initial `configure`.
    Closed,
    /// The surface's `wl_output` went away (`wl_registry.global_remove`) before the initial
    /// `configure`: hide without failing the overlay.
    OutputGone,
    /// The overlay hid, changed target display, released its surface, closed, or entered terminal
    /// GPU loss before the initial `configure`.
    Cancel,
}

#[cfg_attr(
    not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))),
    allow(dead_code)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Effect {
    /// Create the GPU surface for the now-configured `LayerSurface` at `(width, height)`
    /// (0 on a side keeps the requested size) and backing `scale`, then finish ready/reveal.
    Attach { width: u32, height: u32, scale: i32 },
    /// Drop the unconfigured `LayerSurface` and report it closed/unavailable.
    FailClosed,
    /// Drop the unconfigured `LayerSurface` and hide without recording a failure.
    OutputGone,
    /// Drop the unconfigured `LayerSurface` quietly (hidden, recreated, or released).
    DropLayer,
}

/// Outcome of attempting to start a Wayland layer surface before its initial `configure`.
#[cfg_attr(
    not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))),
    allow(dead_code)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartLayer {
    /// Surface created and waiting for its initial `configure`.
    Started,
    /// Targeted display is already gone: make no fallback surface and report no failure.
    OutputGone,
    /// Compositor lacks layer-shell or display connection failed: fall back to XWayland/toplevel.
    Fallback,
}

pub(super) fn step(phase: &mut Phase, event: Event) -> Effects<Effect> {
    match (*phase, event) {
        (Phase::Idle, Event::Started { scale }) => {
            *phase = Phase::Configuring { scale: scale.max(1) };
            Effects::none()
        }
        (Phase::Configuring { .. }, Event::Started { scale }) => {
            *phase = Phase::Configuring { scale: scale.max(1) };
            Effects::one(Effect::DropLayer)
        }
        (Phase::Configuring { .. }, Event::Scale(scale)) => {
            *phase = Phase::Configuring { scale: scale.max(1) };
            Effects::none()
        }
        (Phase::Idle, Event::Scale(_)) => Effects::none(),
        (Phase::Configuring { scale: initial }, Event::Configured { width, height, scale }) => {
            let final_scale = scale.unwrap_or(initial).max(1);
            *phase = Phase::Idle;
            Effects::one(Effect::Attach { width, height, scale: final_scale })
        }
        (Phase::Idle, Event::Configured { .. }) => Effects::none(),
        (Phase::Configuring { .. }, Event::Closed) => {
            *phase = Phase::Idle;
            Effects::one(Effect::FailClosed)
        }
        (Phase::Idle, Event::Closed) => Effects::none(),
        (Phase::Configuring { .. }, Event::OutputGone) => {
            *phase = Phase::Idle;
            Effects::one(Effect::OutputGone)
        }
        (Phase::Idle, Event::OutputGone) => Effects::none(),
        (Phase::Configuring { .. }, Event::Cancel) => {
            *phase = Phase::Idle;
            Effects::one(Effect::DropLayer)
        }
        (Phase::Idle, Event::Cancel) => Effects::none(),
    }
}

/// Owned subsystem state inside [`OverlayCore`] for a Wayland layer surface awaiting its first
/// compositor `configure`.
#[derive(Default)]
pub(in crate::overlay) struct PendingLayer {
    pub(super) phase: Phase,
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) layer: Option<crate::platform::layer_shell::LayerSurface>,
}

impl PendingLayer {
    pub(super) fn is_pending(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn layer(&self) -> Option<&crate::platform::layer_shell::LayerSurface> {
        self.layer.as_ref()
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn layer_mut(&mut self) -> Option<&mut crate::platform::layer_shell::LayerSurface> {
        self.layer.as_mut()
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn is_live(&self) -> bool {
        self.layer.as_ref().is_some_and(crate::platform::layer_shell::LayerSurface::is_live)
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn has_pending_events(&self) -> bool {
        self.layer.as_ref().is_some_and(crate::platform::layer_shell::LayerSurface::has_pending_events)
    }
}

impl OverlayCore {
    pub(super) fn has_pending_layer(&self) -> bool {
        self.pending_layer.is_pending()
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn has_live_pending_layer(&self) -> bool {
        self.pending_layer.is_live()
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn has_pending_layer_events(&self) -> bool {
        self.pending_layer.has_pending_events()
    }

    /// Cancels and drops any Wayland layer surface waiting for its initial `configure`. Returns
    /// `true` when a pending layer surface was destroyed.
    pub(super) fn cancel_pending_layer(&mut self) -> bool {
        let effects = step(&mut self.pending_layer.phase, Event::Cancel);
        let dropped = effects.into_iter().any(|effect| effect == Effect::DropLayer);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let had_layer = self.pending_layer.layer.take().is_some();
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        let had_layer = false;
        dropped || had_layer
    }

    /// Updates placement and commits on a Wayland layer surface that is still awaiting its first
    /// `configure`.
    pub(super) fn resize_pending_layer(&mut self) {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(layer) = self.pending_layer.layer_mut() {
            layer.place(self.at);
            layer.commit();
        }
    }

    /// Creates and commits a Wayland layer surface without blocking for its initial `configure`.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn start_layer(&mut self, event_loop: &ActiveEventLoop, showing: bool) -> StartLayer {
        use winit::platform::wayland::MonitorHandleExtWayland;
        use winit::raw_window_handle::HasDisplayHandle;

        let Ok(display) = event_loop.display_handle() else { return StartLayer::Fallback };
        let display = display.as_raw();
        // With no target, the compositor chooses an output; global pointer querying is forbidden.
        // If a specific display was targeted and is already gone, make no surface on another output.
        let output = match super::lifetime::resolve_wayland_output(
            &self.options.display,
            |id| event_loop.available_monitors().any(|m| m == id.0).then(|| id.0.native_id()),
            |id| event_loop.available_monitors().any(|m| m.native_id() == id),
        ) {
            super::lifetime::WaylandOutputTarget::Any => None,
            super::lifetime::WaylandOutputTarget::Output(id) => Some(id),
            super::lifetime::WaylandOutputTarget::Gone => return StartLayer::OutputGone,
        };
        let monitor = monitor(event_loop, &self.options.display);
        let scale = monitor.as_ref().map_or(1.0, |m| m.scale_factor()).ceil() as i32;
        let hold_keys = showing && self.key_hold.is_held();
        let layer = match crate::platform::layer_shell::LayerSurface::new(
            display,
            self.name,
            self.at,
            self.interactive,
            &self.options,
            output,
            hold_keys,
        ) {
            Ok(layer) => layer,
            Err(crate::platform::layer_shell::NewError::OutputGone) => return StartLayer::OutputGone,
            Err(crate::platform::layer_shell::NewError::Unavailable) => return StartLayer::Fallback,
        };
        self.wayland_globals = Some(layer.globals_handle());
        self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities::placed(
            OverlaySupport::Unsupported,
            if layer.supports_keys() { OverlaySupport::Supported } else { OverlaySupport::Unsupported },
            OverlaySupport::Unsupported,
        );
        let effects = step(&mut self.pending_layer.phase, Event::Started { scale });
        if effects.into_iter().any(|effect| effect == Effect::DropLayer) {
            self.pending_layer.layer = None;
        }
        self.pending_layer.layer = Some(layer);
        StartLayer::Started
    }

    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    pub(super) fn start_layer(&mut self, _event_loop: &ActiveEventLoop, _showing: bool) -> StartLayer {
        StartLayer::Fallback
    }

    /// Dispatches queued Wayland events for a pending layer surface without reading the socket.
    /// Attaches the GPU surface once the initial `configure` arrives, or tears down cleanly on
    /// compositor `closed`, bound-output removal, or fatal display error.
    pub(super) fn poll_pending_layer(&mut self, app: &mut dyn OverlayApp) {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            let Some(layer) = self.pending_layer.layer_mut() else { return };
            let changes = layer.dispatch();
            if let Some(kind) = changes.closed {
                let reason = layer.close_reason().map(str::to_owned);
                let event = match kind {
                    crate::platform::layer_shell_machine::readiness::CloseKind::Closed => Event::Closed,
                    crate::platform::layer_shell_machine::readiness::CloseKind::OutputGone => Event::OutputGone,
                };
                let effects = step(&mut self.pending_layer.phase, event);
                self.pending_layer.layer = None;
                for effect in effects {
                    match effect {
                        Effect::FailClosed => self.layer_closed(app, reason.as_deref()),
                        Effect::OutputGone => self.layer_output_gone(app),
                        Effect::Attach { .. } | Effect::DropLayer => {}
                    }
                }
                return;
            }
            if !layer.is_configured() {
                if let Some(scale) = changes.scale {
                    let _ = step(&mut self.pending_layer.phase, Event::Scale(scale));
                }
                return;
            }
            let (width, height) = changes.size.unwrap_or((0, 0));
            let effects = step(
                &mut self.pending_layer.phase,
                Event::Configured { width, height, scale: changes.scale },
            );
            let Some(Effect::Attach { width, height, scale }) = effects.into_iter().next() else {
                self.pending_layer.layer = None;
                return;
            };
            let Some(mut layer) = self.pending_layer.layer.take() else { return };
            if width > 0 { self.at.size.0 = width as f32; }
            if height > 0 { self.at.size.1 = height as f32; }
            layer.set_buffer_scale(scale);
            match self.attach(app, Backing::Layer(layer), scale as f32) {
                Some(live) => {
                    self.surface.made(live);
                    self.shared.failure.lock().unwrap().take();
                    self.sync_window_backdrop(app.rt_mut(), false);
                    let showing = self.presence.shown();
                    self.ready(app, showing);
                    if self.presence.shown() && self.surface.live().is_some() {
                        self.resume_live(app);
                        self.settle_before_reveal(app.rt_mut());
                        for input in changes.input {
                            if self.surface.live().is_none() { break; }
                            self.surface_input(app, input);
                        }
                        self.dirty = true;
                        self.idle &= !(app.rt().animating || app.compositor_animating());
                    }
                }
                None => {
                    let has_failure = self.shared.failure.lock().unwrap().is_some();
                    self.hide(app);
                    if !has_failure {
                        self.fail(app, "the platform could not create its window or surface".into());
                    } else {
                        self.set_availability_notify(app, Availability::Unavailable);
                    }
                }
            }
        }
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        let _ = app;
    }
}

#[cfg(test)]
#[path = "layer_configure_tests.rs"]
mod tests;
