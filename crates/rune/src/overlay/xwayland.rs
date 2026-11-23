//! An overlay's XWayland backing, for a Wayland compositor without layer-shell.

use super::*;

impl OverlayCore {
    /// The compositor has no layer-shell (GNOME): an overlay that takes no input shows on
    /// XWayland when there is one, and so does one that takes the pointer when its host opted
    /// in (`OverlayOptions::xwayland_input`).
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn xwayland(&mut self, app: &mut dyn OverlayApp) -> Option<Live> {
        let input = self.options.input.resolved(self.interactive);
        if !xwayland_allowed(input, self.options.keys, self.options.xwayland_input) { return None; }
        let wake = self.shared.wake.lock().unwrap().clone();
        let surface = crate::platform::x11::xwayland::XwaylandSurface::new(self.name, self.at, &self.options, input, wake)?;
        let scale = surface.scale();
        self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
            capture_exclusion: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Unsupported, nonactivating_keys: OverlaySupport::Unsupported,
        };
        self.attach(app, Backing::Xwayland(surface), scale)
    }

    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    pub(super) fn xwayland(&mut self, _app: &mut dyn OverlayApp) -> Option<Live> {
        None
    }
}

/// Whether an overlay may show on XWayland: one that takes no input always may; one that takes
/// the pointer only by its host's opt-in; one that asks for keys never does.
#[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
pub(super) fn xwayland_allowed(input: OverlayInput, keys: KeyPolicy, opted_in: bool) -> bool {
    keys != KeyPolicy::OnDemand && (input == OverlayInput::None || opted_in)
}

/// Visibility and input state of an `XwaylandSurface`.
///
/// A surface starts `Hidden` (unmapped, `NO_EVENT`, empty input shape) so a prewarmed overlay
/// never captures clicks before its first `show()`. Showing transitions it to `Shown`, draining
/// any stray events, applying the pending input policy, mapping the window above siblings, and
/// starting the socket watch when input is enabled.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceState {
    Hidden { input: OverlayInput, can_watch: bool },
    Shown { input: OverlayInput, can_watch: bool, watching: bool },
}

#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceEvent {
    Show,
    SetInput(OverlayInput),
}

#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceEffect {
    Drain,
    ApplyInput(OverlayInput),
    MapAbove,
    StartWatch,
}

#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
impl SurfaceState {
    pub(crate) fn new(input: OverlayInput, has_wake: bool) -> Self {
        Self::Hidden { input, can_watch: input != OverlayInput::None && has_wake }
    }

    pub(crate) fn shown(self) -> bool {
        matches!(self, Self::Shown { .. })
    }

    /// The input policy currently active on the X11 window (`None` while hidden).
    pub(crate) fn active_input(self) -> OverlayInput {
        match self {
            Self::Hidden { .. } => OverlayInput::None,
            Self::Shown { input, .. } => input,
        }
    }

    /// Whether hit regions should be pushed to the X11 input shape now.
    pub(crate) fn accepts_regions(self) -> bool {
        matches!(self, Self::Shown { input: OverlayInput::Regions, .. })
    }
}

#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(crate) fn step(state: &mut SurfaceState, event: SurfaceEvent) -> crate::machine::Effects<SurfaceEffect> {
    use crate::machine::Effects;
    match (*state, event) {
        (SurfaceState::Hidden { input, can_watch }, SurfaceEvent::Show) => {
            let watching = can_watch && input != OverlayInput::None;
            *state = SurfaceState::Shown { input, can_watch, watching };
            let mut effects = Effects::one(SurfaceEffect::Drain);
            if input != OverlayInput::None { effects.push(SurfaceEffect::ApplyInput(input)); }
            effects.push(SurfaceEffect::MapAbove);
            if watching { effects.push(SurfaceEffect::StartWatch); }
            effects
        }
        (SurfaceState::Shown { .. }, SurfaceEvent::Show) => Effects::none(),
        (SurfaceState::Hidden { can_watch, .. }, SurfaceEvent::SetInput(next)) => {
            let next = if can_watch { next } else { OverlayInput::None };
            *state = SurfaceState::Hidden { input: next, can_watch };
            Effects::none()
        }
        (SurfaceState::Shown { input, can_watch, watching }, SurfaceEvent::SetInput(next)) => {
            let next = if can_watch { next } else { OverlayInput::None };
            if next == input { return Effects::none(); }
            let start_watch = !watching && can_watch && next != OverlayInput::None;
            *state = SurfaceState::Shown { input: next, can_watch, watching: watching || start_watch };
            if start_watch {
                Effects::two(SurfaceEffect::ApplyInput(next), SurfaceEffect::StartWatch)
            } else {
                Effects::one(SurfaceEffect::ApplyInput(next))
            }
        }
    }
}

#[cfg(test)]
#[path = "xwayland_tests.rs"]
mod tests;
