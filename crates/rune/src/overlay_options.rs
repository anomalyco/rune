//! Placement and input policy for native overlays. Coordinates inside a display are points.

use crate::{Align, Cx, El, OverlaySpec, ScreenEdge};

/// An opaque display identity, valid for this event-loop session. Obtain it from `Display::id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayId(pub(crate) winit::monitor::MonitorHandle);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DisplayTarget {
    #[default]
    Primary,
    UnderPointer,
    Id(DisplayId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayPlacement {
    Edge { edge: ScreenEdge, margin: f32, align: Align, inset: f32 },
    /// Points from the selected display's top-left, not desktop pixels.
    Rect { x: f32, y: f32, width: f32, height: f32 },
    FullScreen,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayLevel {
    #[default]
    Status,
    AboveStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyPolicy {
    #[default]
    None,
    /// Permit `OverlayHandle::request_keys`; showing alone never requests focus.
    OnDemand,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayInput {
    /// Preserve `OverlaySpec::interactive` (whole surface or complete pass-through).
    #[default]
    Legacy,
    /// Only elements marked `El::hit()` intercept the pointer.
    Regions,
}

#[derive(Clone, Debug, Default)]
pub struct OverlayOptions {
    /// None preserves the spec's edge, margin, alignment and inset.
    pub placement: Option<OverlayPlacement>,
    pub display: DisplayTarget,
    /// Re-resolve UnderPointer while visible. Unsupported on Wayland.
    pub follow_pointer: bool,
    pub level: OverlayLevel,
    /// Native best effort, not a security boundary. Use the window number in capture filters.
    pub exclude_from_capture: bool,
    pub keys: KeyPolicy,
    pub input: OverlayInput,
}

/// Keeps existing exhaustive `OverlaySpec` literals source compatible.
pub struct ConfiguredOverlay<T: 'static> {
    pub spec: OverlaySpec<T>,
    pub options: OverlayOptions,
    pub on_key: Option<fn(&mut T, &crate::KeyEvent, &mut crate::Tx) -> bool>,
}

impl<T: 'static> ConfiguredOverlay<T> {
    pub fn on_key(mut self, f: fn(&mut T, &crate::KeyEvent, &mut crate::Tx) -> bool) -> Self { self.on_key = Some(f); self }
}

impl<T: 'static> OverlaySpec<T> {
    pub fn new(name: &'static str, size: (f32, f32), view: fn(&T, &mut Cx<T>) -> El) -> Self {
        Self { name, size, edge: ScreenEdge::Top, margin: 0.0, align: Align::Center, inset: 0.0, interactive: false, view }
    }

    pub fn options(self, options: OverlayOptions) -> ConfiguredOverlay<T> {
        ConfiguredOverlay { spec: self, options, on_key: None }
    }
}

impl<T: 'static> From<OverlaySpec<T>> for ConfiguredOverlay<T> {
    fn from(spec: OverlaySpec<T>) -> Self { spec.options(OverlayOptions::default()) }
}

/// Native capability, distinct from whether an overlay could be created.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlaySupport {
    #[default]
    Unknown,
    Supported,
    /// The OS accepted a capture-exclusion request; capture clients may still need a filter.
    Requested,
    Unsupported,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayCapabilities {
    pub capture_exclusion: OverlaySupport,
    pub nonactivating_keys: OverlaySupport,
    pub global_pointer: OverlaySupport,
}

/// Bounds in logical points, local to a display. Also shared by native placement tests.
pub(crate) fn local_rect(at: crate::overlay::Placement, placement: Option<OverlayPlacement>, area: (f32, f32)) -> [f32; 4] {
    let (edge, margin, align, inset) = match placement {
        Some(OverlayPlacement::FullScreen) => return [0.0, 0.0, area.0, area.1],
        Some(OverlayPlacement::Rect { x, y, width, height }) => return [x, y, width.max(1.0), height.max(1.0)],
        Some(OverlayPlacement::Edge { edge, margin, align, inset }) => (edge, margin, align, inset),
        None => (at.edge, at.margin, at.align, at.inset),
    };
    let (w, h) = (at.size.0.max(1.0), at.size.1.max(1.0));
    let x = match align { Align::Start => inset, Align::Center => (area.0 - w) / 2.0, Align::End => area.0 - w - inset };
    let y = match edge { ScreenEdge::Top => margin, ScreenEdge::Bottom => area.1 - h - margin };
    [x, y, w, h]
}

pub(crate) fn contains(rect: [f32; 4], x: f32, y: f32) -> bool {
    x >= rect[0] && y >= rect[1] && x < rect[2] && y < rect[3]
}

/// Bounds and pointer share one native coordinate space. Never rescale desktop origins.
pub(crate) fn choose_display(bounds: &[[f64; 4]], primary: Option<usize>, requested: Option<usize>, pointer: Option<(f64, f64)>) -> Option<usize> {
    requested.filter(|i| *i < bounds.len()).or_else(|| pointer.and_then(|(x, y)| {
        bounds.iter().position(|r| x >= r[0] && y >= r[1] && x < r[2] && y < r[3])
    })).or(primary.filter(|i| *i < bounds.len())).or_else(|| (!bounds.is_empty()).then_some(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_selection_handles_negative_origins_shared_edges_and_disconnects() {
        // Native pixels, so a 2x display can have a larger rectangle than the primary.
        let bounds = [[-3840.0, -200.0, 0.0, 1960.0], [0.0, 0.0, 1920.0, 1080.0]];
        assert_eq!(choose_display(&bounds, Some(1), None, Some((-100.0, 0.0))), Some(0));
        assert_eq!(choose_display(&bounds, Some(1), None, Some((0.0, 0.0))), Some(1));
        assert_eq!(choose_display(&bounds, Some(1), Some(0), None), Some(0));
        assert_eq!(choose_display(&bounds, Some(1), Some(4), None), Some(1));
        assert_eq!(choose_display(&bounds, None, None, None), Some(0));
        assert_eq!(choose_display(&[], None, None, None), None);
    }

    #[test]
    fn edge_and_rect_use_display_local_points() {
        let at = crate::overlay::Placement { size: (300.0, 80.0), edge: ScreenEdge::Bottom, margin: 24.0, align: Align::End, inset: 12.0 };
        assert_eq!(local_rect(at, None, (960.0, 540.0)), [648.0, 436.0, 300.0, 80.0]);
        assert_eq!(local_rect(at, Some(OverlayPlacement::FullScreen), (960.0, 540.0)), [0.0, 0.0, 960.0, 540.0]);
        assert_eq!(local_rect(at, Some(OverlayPlacement::Rect { x: -10.0, y: 20.0, width: 100.0, height: 50.0 }), (960.0, 540.0)), [-10.0, 20.0, 100.0, 50.0]);
        // A 2x display left of primary: scale local offsets, never the desktop origin.
        let r = local_rect(at, None, (960.0, 540.0));
        assert_eq!((-1920 + (r[0] * 2.0) as i32, -100 + (r[1] * 2.0) as i32), (-624, 772));
    }
}
