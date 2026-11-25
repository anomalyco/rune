//! Cross-thread pointer injection (`OverlayHandle::send_pointer`), screen-to-local coordinate
//! conversion (`OverlayHandle::screen_to_local`), native-vs-injected pointer ownership state
//! machine, and the shared host-to-runtime pointer dispatch seam.

use super::*;
use crate::input::host_events;
use crate::machine::Effects;
use crate::{Mods, PointerButton, PointerInput, WheelDelta};

/// Maximum number of queued injected pointer events held inline per overlay (zero heap
/// allocation in steady state).
pub(super) const MAX_QUEUED: usize = 128;

/// Bounded fixed-capacity inline ring buffer for cross-thread `OverlayHandle::send_pointer`
/// events. Consecutive `Move` events coalesce in place at the tail; overflow drops the oldest
/// queued `Move` first (preserving `Down`/`Up` pairs) or the oldest entry if none are moves.
pub(super) struct PointerQueue {
    ring: [PointerInput; MAX_QUEUED],
    head: usize,
    len: usize,
}

impl Default for PointerQueue {
    fn default() -> Self {
        Self { ring: [PointerInput::Leave; MAX_QUEUED], head: 0, len: 0 }
    }
}

impl PointerQueue {
    /// Queues `event` if finite, coalescing consecutive `Move`s in place. Returns `true` when
    /// the queue transitioned from empty to non-empty so the caller wakes the event loop once.
    pub(super) fn push(&mut self, event: PointerInput) -> bool {
        if !event.is_finite() { return false; }
        let was_empty = self.len == 0;
        if matches!(event, PointerInput::Move { .. }) && self.len > 0 {
            let tail = (self.head + self.len - 1) % MAX_QUEUED;
            if matches!(self.ring[tail], PointerInput::Move { .. }) {
                self.ring[tail] = event;
                return false;
            }
        }
        if self.len == MAX_QUEUED {
            self.drop_one_for_overflow();
        }
        let slot = (self.head + self.len) % MAX_QUEUED;
        self.ring[slot] = event;
        self.len += 1;
        was_empty
    }

    fn drop_one_for_overflow(&mut self) {
        // Prefer dropping the oldest Move so Down/Up pairs stay balanced under bursts.
        for rel in 0..self.len {
            let idx = (self.head + rel) % MAX_QUEUED;
            if matches!(self.ring[idx], PointerInput::Move { .. }) {
                for shift in rel..(self.len - 1) {
                    let dst = (self.head + shift) % MAX_QUEUED;
                    let src = (self.head + shift + 1) % MAX_QUEUED;
                    self.ring[dst] = self.ring[src];
                }
                self.len -= 1;
                return;
            }
        }
        self.head = (self.head + 1) % MAX_QUEUED;
        self.len -= 1;
    }

    /// Drains all queued events into a stack array with zero heap allocation. Returns `None`
    /// immediately when empty.
    pub(super) fn drain_to_array(&mut self) -> Option<([PointerInput; MAX_QUEUED], usize)> {
        if self.len == 0 { return None; }
        let mut out = [PointerInput::Leave; MAX_QUEUED];
        let count = self.len;
        for slot in out.iter_mut().take(count) {
            *slot = self.ring[self.head];
            self.head = (self.head + 1) % MAX_QUEUED;
        }
        self.head = 0;
        self.len = 0;
        Some((out, count))
    }

    pub(super) fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize { self.len }
}

/// Which input path produced a pointer event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputSource {
    Native,
    Injected,
}

/// Bitmask bit for `PointerOwnership::Captured::buttons`.
pub(super) const fn button_mask(button: PointerButton) -> u8 {
    match button {
        PointerButton::Primary => 1 << 0,
        PointerButton::Secondary => 1 << 1,
        PointerButton::Middle => 1 << 2,
        PointerButton::Back => 1 << 3,
        PointerButton::Forward => 1 << 4,
        PointerButton::Other(_) => 1 << 5,
    }
}

/// Explicit interleaving ownership state between native OS pointer input and app-injected
/// `OverlayHandle::send_pointer` input. In `Idle` or `Hovering`, the latest source takes
/// ownership; while `Captured { owner, .. }`, every event from the other source (including
/// native global pointer polls and `CursorLeft`) is ignored until all held buttons release,
/// `Cancel` arrives, or the overlay hides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PointerOwnership {
    #[default]
    Idle,
    Hovering(InputSource),
    Captured { owner: InputSource, buttons: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OwnershipEvent {
    Move(InputSource),
    Down { source: InputSource, button: PointerButton },
    Up { source: InputSource, button: PointerButton },
    Wheel(InputSource),
    Leave(InputSource),
    Cancel(InputSource),
    /// Overlay hid, started fading out, or released its window.
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OwnershipEffect {
    /// Forward this event into the shared host-to-runtime dispatch seam.
    Dispatch,
    /// Cancel any active pointer capture in the runtime.
    CancelCapture,
}

impl PointerOwnership {
    pub(super) fn step(&mut self, event: OwnershipEvent) -> Effects<OwnershipEffect> {
        match (*self, event) {
            (_, OwnershipEvent::Reset) => {
                *self = Self::Idle;
                Effects::none()
            }
            (Self::Idle | Self::Hovering(_), OwnershipEvent::Move(source) | OwnershipEvent::Wheel(source)) => {
                *self = Self::Hovering(source);
                Effects::one(OwnershipEffect::Dispatch)
            }
            (Self::Captured { owner, .. }, OwnershipEvent::Move(source) | OwnershipEvent::Wheel(source)) if owner == source => {
                Effects::one(OwnershipEffect::Dispatch)
            }
            (Self::Captured { .. }, OwnershipEvent::Move(_) | OwnershipEvent::Wheel(_)) => Effects::none(),
            (Self::Idle | Self::Hovering(_), OwnershipEvent::Down { source, button }) => {
                *self = Self::Captured { owner: source, buttons: button_mask(button) };
                Effects::one(OwnershipEffect::Dispatch)
            }
            (Self::Captured { owner, buttons }, OwnershipEvent::Down { source, button }) if owner == source => {
                *self = Self::Captured { owner, buttons: buttons | button_mask(button) };
                Effects::one(OwnershipEffect::Dispatch)
            }
            (Self::Captured { .. }, OwnershipEvent::Down { .. }) => Effects::none(),
            (Self::Captured { owner, buttons }, OwnershipEvent::Up { source, button }) if owner == source => {
                let remaining = buttons & !button_mask(button);
                *self = if remaining == 0 { Self::Hovering(source) } else { Self::Captured { owner, buttons: remaining } };
                Effects::one(OwnershipEffect::Dispatch)
            }
            (Self::Captured { .. }, OwnershipEvent::Up { .. }) => Effects::none(),
            (Self::Idle | Self::Hovering(_), OwnershipEvent::Up { source, .. }) => {
                *self = Self::Hovering(source);
                Effects::one(OwnershipEffect::Dispatch)
            }
            (Self::Idle, OwnershipEvent::Leave(_)) => Effects::one(OwnershipEffect::Dispatch),
            (Self::Hovering(owner), OwnershipEvent::Leave(source)) if owner == source => {
                *self = Self::Idle;
                Effects::one(OwnershipEffect::Dispatch)
            }
            (Self::Hovering(_), OwnershipEvent::Leave(_)) => Effects::none(),
            (Self::Captured { owner, .. }, OwnershipEvent::Leave(source)) if owner == source => {
                Effects::one(OwnershipEffect::Dispatch)
            }
            (Self::Captured { .. }, OwnershipEvent::Leave(_)) => Effects::none(),
            (Self::Idle, OwnershipEvent::Cancel(_)) => Effects::one(OwnershipEffect::CancelCapture),
            (Self::Hovering(owner) | Self::Captured { owner, .. }, OwnershipEvent::Cancel(source)) if owner == source => {
                *self = Self::Idle;
                Effects::one(OwnershipEffect::CancelCapture)
            }
            (Self::Hovering(_) | Self::Captured { .. }, OwnershipEvent::Cancel(_)) => Effects::none(),
        }
    }

    /// True while injected input owns the pointer (hovering or captured), so native OS cursor
    /// updates are suppressed.
    pub(super) fn is_injected(self) -> bool {
        matches!(self, Self::Hovering(InputSource::Injected) | Self::Captured { owner: InputSource::Injected, .. })
    }

    /// True while an injected press holds capture.
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
    pub(super) fn captured_by_injected(self) -> bool {
        matches!(self, Self::Captured { owner: InputSource::Injected, .. })
    }
}

/// Converts physical desktop screen pixels into overlay-local logical points using the last
/// published `(origin_px, backing_scale)` snapshot. Pure math, zero OS calls.
pub(super) fn screen_to_local(transform: Option<((i32, i32), f32)>, screen_px: (i32, i32)) -> Option<(f32, f32)> {
    let (origin, scale) = transform?;
    if !scale.is_finite() || scale <= 0.0 { return None; }
    let dx = (f64::from(screen_px.0) - f64::from(origin.0)) / f64::from(scale);
    let dy = (f64::from(screen_px.1) - f64::from(origin.1)) / f64::from(scale);
    let (x, y) = (dx as f32, dy as f32);
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

impl OverlayCore {
    /// Publishes the overlay's current physical origin and backing scale into `SharedCore` for
    /// `OverlayHandle::screen_to_local`. Stores `None` while hidden, fading out, or on hosts
    /// without global window positions (Wayland).
    pub(super) fn publish_screen_transform(&self, _rt: &RuntimeCore) {
        let transform = if self.presence.shown() {
            self.live_window().and_then(|w| {
                let scale = self.live_scale().unwrap_or_else(|| crate::window::backing_scale(w));
                if !scale.is_finite() || scale <= 0.0 { return None; }
                #[cfg(target_os = "macos")]
                let origin = crate::platform::macos::content_origin(w).map(|(x, y)| {
                    ((f64::from(x) * f64::from(scale)).round() as i32, (f64::from(y) * f64::from(scale)).round() as i32)
                });
                #[cfg(not(target_os = "macos"))]
                let origin = w.inner_position().ok().map(|p| (p.x, p.y));
                origin.map(|o| (o, scale))
            })
        } else {
            None
        };
        *self.shared.screen_transform.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = transform;
    }

    #[cfg(target_os = "windows")]
    pub(super) fn publish_screen_origin(&self, origin: (i32, i32), scale: f32) {
        let transform = (self.presence.shown() && scale.is_finite() && scale > 0.0).then_some((origin, scale));
        *self.shared.screen_transform.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = transform;
    }

    pub(super) fn clear_screen_transform(&self) {
        *self.shared.screen_transform.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Drops all queued injected pointer events without dispatching them (used when hidden,
    /// fading out, released, or dropped so stale events never replay on a later show).
    pub(super) fn drop_injected_pointers(&self) {
        self.shared.pointers.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    }

    /// Drains queued injected pointer events on `sync()` after visibility/placement/resize.
    /// Drops all queued events whenever the overlay is not shown.
    pub(super) fn drain_injected_pointers(&mut self, app: &mut dyn OverlayApp) {
        if !self.presence.shown() {
            self.drop_injected_pointers();
            return;
        }
        let Some((batch, count)) = self.shared.pointers.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drain_to_array() else {
            return;
        };
        let _input = crate::input::time::arrived_now();
        for &event in &batch[..count] {
            self.dispatch_pointer(app, InputSource::Injected, event);
        }
    }

    /// Shared host-to-runtime pointer dispatch seam used by both native overlay input and
    /// app-injected `OverlayHandle::send_pointer` events. Coordinates in `event` are
    /// overlay-local logical points (divided by `rt.zoom()` here).
    pub(super) fn dispatch_pointer(&mut self, app: &mut dyn OverlayApp, source: InputSource, event: PointerInput) {
        if !event.is_finite() { return; }
        let ow_event = match event {
            PointerInput::Move { .. } => OwnershipEvent::Move(source),
            PointerInput::Down { button, .. } => OwnershipEvent::Down { source, button },
            PointerInput::Up { button, .. } => OwnershipEvent::Up { source, button },
            PointerInput::Wheel { .. } => OwnershipEvent::Wheel(source),
            PointerInput::Leave => OwnershipEvent::Leave(source),
            PointerInput::Cancel => OwnershipEvent::Cancel(source),
        };
        let effects = self.pointer_owner.step(ow_event);
        if effects.is_empty() { return; }
        for effect in effects {
            match effect {
                OwnershipEffect::CancelCapture => {
                    app.rt_mut().cancel_pointer();
                    self.dirty = true;
                }
                OwnershipEffect::Dispatch => self.dispatch_pointer_inner(app, source, event),
            }
        }
    }

    fn dispatch_pointer_inner(&mut self, app: &mut dyn OverlayApp, source: InputSource, event: PointerInput) {
        let zoom = app.rt().zoom();
        let to_view = |x: f32, y: f32| (x / zoom, y / zoom);
        let in_bounds = |x: f32, y: f32| x >= 0.0 && y >= 0.0 && x < self.at.size.0 && y < self.at.size.1;
        match event {
            PointerInput::Move { x, y } => {
                if app.rt().pointer_captured() || in_bounds(x, y) {
                    let (vx, vy) = to_view(x, y);
                    self.dirty |= app.observe_pointer(vx, vy);
                    if source == InputSource::Native {
                        self.take_titlebar_action(app);
                        self.begin_drag_out(app.rt_mut());
                    } else {
                        self.refuse_injected_os_drags(app.rt_mut());
                    }
                } else if app.rt().pointer().is_some() {
                    app.rt_mut().pointer_leave();
                    self.dirty = true;
                }
            }
            PointerInput::Down { x, y, button, mods } => {
                let (vx, vy) = to_view(x, y);
                self.dispatch_button(app, source, button, true, Some((vx, vy)), mods);
            }
            PointerInput::Up { x, y, button, mods } => {
                let (vx, vy) = to_view(x, y);
                self.dispatch_button(app, source, button, false, Some((vx, vy)), mods);
            }
            PointerInput::Wheel { x, y, delta, mods, phase } => {
                let (vx, vy) = to_view(x, y);
                if app.rt().pointer() != Some((vx, vy)) && (app.rt().pointer_captured() || in_bounds(x, y)) {
                    self.dirty |= app.observe_pointer(vx, vy);
                }
                let winit_delta = match delta {
                    WheelDelta::Lines { dx, dy } => winit::event::MouseScrollDelta::LineDelta(dx, dy),
                    WheelDelta::Points { dx, dy } => winit::event::MouseScrollDelta::PixelDelta(
                        winit::dpi::PhysicalPosition::new(f64::from(dx), f64::from(dy)),
                    ),
                };
                if host_events::wheel_from_delta(app.rt_mut(), winit_delta, phase, 1.0, mods)
                    != host_events::WheelOutcome::Ignored
                {
                    self.dirty = true;
                }
            }
            PointerInput::Leave => {
                app.rt_mut().pointer_leave();
                self.dirty = true;
            }
            PointerInput::Cancel => {
                app.rt_mut().cancel_pointer();
                self.dirty = true;
            }
        }
    }

    /// Shared button dispatch for both native and injected sources (coordinates already in view
    /// points). Injected presses never start native window drags or OS drag-and-drop.
    pub(super) fn dispatch_button(
        &mut self,
        app: &mut dyn OverlayApp,
        source: InputSource,
        button: PointerButton,
        pressed: bool,
        point: Option<(f32, f32)>,
        mods: Mods,
    ) {
        match button {
            PointerButton::Primary => {
                if pressed {
                    if let Some((vx, vy)) = point && app.rt().pointer() != Some((vx, vy)) {
                        app.pointer_move(vx, vy);
                    }
                    if let Some((vx, vy)) = app.rt().pointer() {
                        if source == InputSource::Injected && mods != Mods::default() {
                            app.rt_mut().physical_input(crate::PhysicalInput::Modifiers(mods));
                        }
                        app.pointer_down(vx, vy);
                        if source == InputSource::Native || mods == Mods::default() {
                            app.rt_mut().physical_input(crate::PhysicalInput::Modifiers(mods));
                        }
                        if source == InputSource::Native {
                            if !self.take_titlebar_action(app) {
                                self.position_owner.step(input::PositionEvent::LeftReleased);
                            }
                        } else {
                            self.refuse_injected_os_drags(app.rt_mut());
                        }
                    }
                } else {
                    if let Some((vx, vy)) = point.or(app.rt().pointer()) {
                        app.pointer_up(vx, vy);
                    }
                    if source == InputSource::Native {
                        self.position_owner.step(input::PositionEvent::LeftReleased);
                    }
                }
            }
            PointerButton::Secondary => {
                if let Some((vx, vy)) = point && app.rt().pointer() != Some((vx, vy)) {
                    app.pointer_move(vx, vy);
                }
                if let Some((vx, vy)) = app.rt().pointer() {
                    if pressed { app.rt_mut().secondary_down(vx, vy); } else { app.secondary_up(vx, vy); }
                }
            }
            other => {
                if let Some((vx, vy)) = point && app.rt().pointer() != Some((vx, vy)) {
                    app.pointer_move(vx, vy);
                }
                if let Some((vx, vy)) = app.rt().pointer() {
                    if pressed { app.button_down(vx, vy, other); } else { app.button_up(vx, vy, other); }
                }
            }
        }
        self.dirty = true;
    }

    /// Drops any native window drag, resize, or OS drag-out request triggered by an injected
    /// pointer event so injected input never steals the OS pointer or enters a modal move loop.
    fn refuse_injected_os_drags(&self, rt: &mut RuntimeCore) {
        let _ = rt.take_resize_action();
        let _ = rt.take_titlebar_action();
        rt.disarm_drag_out();
        let _ = rt.take_drag_out();
    }

    /// Native `CursorLeft` / Wayland `Input::Leave` routed through `PointerOwnership`.
    pub(super) fn native_pointer_leave(&mut self, app: &mut dyn OverlayApp) {
        if self.pointer_owner.step(OwnershipEvent::Leave(InputSource::Native)).is_empty() { return; }
        app.rt_mut().pointer_leave();
        self.dirty = true;
    }

    /// Native non-primary button routed through `PointerOwnership` and the shared button seam.
    pub(super) fn native_button(&mut self, app: &mut dyn OverlayApp, button: PointerButton, pressed: bool) {
        let ev = if pressed {
            OwnershipEvent::Down { source: InputSource::Native, button }
        } else {
            OwnershipEvent::Up { source: InputSource::Native, button }
        };
        if self.pointer_owner.step(ev).is_empty() { return; }
        self.dispatch_button(app, InputSource::Native, button, pressed, None, crate::window::map_mods(self.mods));
    }

    /// Native winit `MouseWheel` routed through `PointerOwnership` and `host_events::wheel`.
    pub(super) fn native_mouse_wheel(
        &mut self,
        app: &mut dyn OverlayApp,
        delta: winit::event::MouseScrollDelta,
        phase: winit::event::TouchPhase,
    ) {
        if self.pointer_owner.step(OwnershipEvent::Wheel(InputSource::Native)).is_empty() { return; }
        let scale = self.live_scale().unwrap_or(1.0);
        host_events::wheel(app.rt_mut(), delta, host_events::gesture_phase(phase), scale, self.mods);
        self.dirty = true;
    }

    /// Wayland/XWayland `Input::Wheel` routed through `PointerOwnership` and `wheel_core`.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn native_surface_wheel(&mut self, app: &mut dyn OverlayApp, wheel: host_events::WheelInput) -> bool {
        if self.pointer_owner.step(OwnershipEvent::Wheel(InputSource::Native)).is_empty() { return false; }
        host_events::wheel_core(app.rt_mut(), wheel) != host_events::WheelOutcome::Ignored
    }

    /// Wayland/XWayland `Input::Move` routed through `PointerOwnership`.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn native_surface_move(&mut self, app: &mut dyn OverlayApp, x: f32, y: f32) {
        if self.pointer_owner.step(OwnershipEvent::Move(InputSource::Native)).is_empty() { return; }
        let zoom = app.rt().zoom();
        self.dirty |= app.observe_pointer(x / zoom, y / zoom);
    }

    /// Test helper: publishes an explicit `(origin_px, scale)` snapshot when `presence.shown()`,
    /// or `None` while hidden/fading, matching `publish_screen_transform` without a real window.
    #[cfg(test)]
    pub(super) fn set_test_screen_transform(&self, origin: Option<(i32, i32)>, scale: f32) {
        let transform = if self.presence.shown() && scale.is_finite() && scale > 0.0 {
            origin.map(|o| (o, scale))
        } else {
            None
        };
        *self.shared.screen_transform.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = transform;
    }
}

#[cfg(test)]
#[path = "pointer_inject_tests.rs"]
mod tests;
