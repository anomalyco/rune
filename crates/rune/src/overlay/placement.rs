//! Where an overlay goes: its screen and its position on that screen's edge.

use winit::dpi::PhysicalPosition;
use winit::event_loop::ActiveEventLoop;

use super::{Placement, ScreenEdge};
use crate::el::Align;
use crate::{DisplayTarget, OverlayOptions, OverlayPlacement};

/// Places an overlay on its target display. Edge placements (`None` and
/// `OverlayPlacement::Edge`) anchor inside all four sides of the display's work area on Windows
/// (excluding taskbars and docked app bars); `Rect` and `FullScreen` stay in full display
/// coordinates.
pub(super) fn position(event_loop: &ActiveEventLoop, at: Placement, options: &OverlayOptions) -> Option<PhysicalPosition<i32>> {
    let monitor = monitor(event_loop, &options.display)?;
    let scale = monitor.scale_factor();
    let (origin, area) = (monitor.position(), monitor.size());
    let display = ((origin.x, origin.y), (area.width, area.height));
    // Inside the taskbar and app bars of whichever Windows display it's on.
    #[cfg(target_os = "windows")]
    let work = crate::platform::win32::work_area(&monitor).unwrap_or(display);
    #[cfg(not(target_os = "windows"))]
    let work = display;
    Some(position_in(display, work, scale, at, options.placement))
}

/// Computes an overlay's physical desktop origin from the display's full bounds and its usable
/// work area (both `(origin, size)` in physical desktop pixels).
///
/// Edge placements (`None` and `Some(OverlayPlacement::Edge { .. })`) anchor inside all four
/// sides of `work_area`. `OverlayPlacement::Rect` and `OverlayPlacement::FullScreen` anchor to
/// `display` so full-screen pickers and explicit desktop rects keep full-monitor coordinates.
fn position_in(
    display: ((i32, i32), (u32, u32)),
    work_area: ((i32, i32), (u32, u32)),
    scale: f64,
    at: Placement,
    placement: Option<OverlayPlacement>,
) -> PhysicalPosition<i32> {
    let ((dx, dy), (dw, dh)) = display;
    let (edge, margin, align, inset) = match placement {
        Some(OverlayPlacement::FullScreen | OverlayPlacement::Rect { .. }) => {
            let r = crate::overlay::options::local_rect(
                at,
                placement,
                (dw as f32 / scale as f32, dh as f32 / scale as f32),
            );
            return PhysicalPosition::new(
                dx + (f64::from(r[0]) * scale).round() as i32,
                dy + (f64::from(r[1]) * scale).round() as i32,
            );
        }
        Some(OverlayPlacement::Edge { edge, margin, align, inset }) => (edge, margin, align, inset),
        None => (at.edge, at.margin, at.align, at.inset),
    };
    let ((wx, wy), (ww, wh)) = work_area;
    let px = |v: f32| (f64::from(v) * scale).round() as i64;
    let (w, h) = (px(at.size.0.max(1.0)), px(at.size.1.max(1.0)));
    let left = i64::from(wx);
    let top = i64::from(wy);
    let right = left + i64::from(ww);
    let bottom = top + i64::from(wh);
    let x = match align {
        Align::Start => left + px(inset),
        Align::Center => left + (i64::from(ww) - w) / 2,
        Align::End => right - w - px(inset),
    };
    let y = match edge {
        ScreenEdge::Top => top + px(margin),
        ScreenEdge::Bottom => bottom - h - px(margin),
    };
    PhysicalPosition::new(
        x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
    )
}

pub(super) fn monitor(event_loop: &ActiveEventLoop, target: &DisplayTarget) -> Option<winit::monitor::MonitorHandle> {
    monitor_at(event_loop, target, None)
}

pub(crate) fn monitor_at(event_loop: &ActiveEventLoop, target: &DisplayTarget, sampled: Option<(i32, i32)>) -> Option<winit::monitor::MonitorHandle> {
    let monitors: Vec<_> = event_loop.available_monitors().collect();
    let primary = event_loop.primary_monitor().and_then(|p| monitors.iter().position(|m| *m == p));
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    let resolved = if let DisplayTarget::Native(crate::NativeDisplayId::X11Output(output)) = target {
        if !crate::window::is_wayland(event_loop) { crate::platform::x11::overlay::output_crtc(*output).map(|id| DisplayTarget::Native(crate::NativeDisplayId::X11Crtc(id))) } else { None }
    } else { None };
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    let target = resolved.as_ref().unwrap_or(target);
    let requested = match target {
        DisplayTarget::Id(id) => monitors.iter().position(|m| *m == id.0),
        DisplayTarget::Native(id) => monitors.iter().position(|m| crate::DisplayId::from_monitor(m.clone(), crate::window::is_wayland(event_loop)).native_id() == Some(*id)),
        _ => None,
    };
    let bounds: Vec<_> = monitors.iter().map(|m| {
        let (p, s) = (m.position(), m.size());
        [f64::from(p.x), f64::from(p.y), f64::from(p.x) + f64::from(s.width), f64::from(p.y) + f64::from(s.height)]
    }).collect();
    #[cfg(target_os = "windows")]
    let pointer = (*target == DisplayTarget::UnderPointer).then(crate::platform::win32::pointer_position).flatten();
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    let pointer = if *target == DisplayTarget::UnderPointer && !crate::window::is_wayland(event_loop) { crate::platform::x11::overlay::pointer_position() } else { None };
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android", target_arch = "wasm32"))]
    let pointer: Option<(i32, i32)> = None;
    let i = crate::overlay::options::choose_display(&bounds, primary, requested, sampled.or(pointer).map(|(x, y)| (f64::from(x), f64::from(y))))?;
    Some(monitors[i].clone())
}

#[cfg(test)]
#[path = "placement_tests.rs"]
mod tests;
