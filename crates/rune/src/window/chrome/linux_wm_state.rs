//! Mapping Wayland `xdg_toplevel` / `xdg-decoration` configure state to [`WindowManagerState`].
//!
//! Pure decoding functions live here so every host tests protocol-version gating, tiled edge
//! decoding, decoration mode mapping, and capability bitmasks without a live Wayland compositor.
//! Follows Chromium Linux (`ui/ozone/platform/wayland/host/wayland_window.h` `WindowTiledEdges`),
//! GTK4 (`GdkToplevelCapabilities` for `xdg_toplevel::wm_capabilities`), and `xdg-shell` v2/v5.

use crate::window::wm_state::{DecorationMode, TiledEdges, WindowActions, WindowManagerState};

/// `wayland_csd_frame::WindowManagerCapabilities::WINDOW_MENU` (bit 0).
pub(crate) const WAYLAND_CAP_WINDOW_MENU: u16 = 1 << 0;
/// `wayland_csd_frame::WindowManagerCapabilities::MAXIMIZE` (bit 1).
pub(crate) const WAYLAND_CAP_MAXIMIZE: u16 = 1 << 1;
/// `wayland_csd_frame::WindowManagerCapabilities::FULLSCREEN` (bit 2).
pub(crate) const WAYLAND_CAP_FULLSCREEN: u16 = 1 << 2;
/// `wayland_csd_frame::WindowManagerCapabilities::MINIMIZE` (bit 3).
pub(crate) const WAYLAND_CAP_MINIMIZE: u16 = 1 << 3;

/// Raw Wayland configure state reported by vendored winit's `WindowExtWayland::wayland_wm_state`.
///
/// - `server_decorations`: `true` when `xdg-decoration` negotiated `Server`, `false` for `Client`.
/// - `tiled_edges`: `Some([top, right, bottom, left])` when `xdg_toplevel.version() >= 2`, else `None`.
/// - `capabilities`: `Some(bits)` when `xdg_toplevel.version() >= 5`, else `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WaylandConfigureState {
    pub(crate) server_decorations: bool,
    pub(crate) tiled_edges: Option<[bool; 4]>,
    pub(crate) capabilities: Option<u16>,
}

/// Decodes raw Wayland configure state (or `None` before the initial configure) into
/// [`WindowManagerState`].
///
/// - Before initial configure (`None`): all fields remain unknown ([`WindowManagerState::default`]).
/// - `decoration_mode`: directly from `server_decorations` (`Server` vs `Client`), never inferred
///   from capabilities.
/// - `tiled_edges`: `Some(TiledEdges::new(t, r, b, l))` when reported (`xdg_toplevel` v2+),
///   `None` below v2.
/// - `actions`: `window_menu`, `maximize`, `fullscreen`, `minimize` decoded from `capabilities`
///   when reported (`xdg_toplevel` v5+), `None` below v5; `close` is always `None` on Wayland
///   because `xdg_toplevel::wm_capabilities` carries no close capability.
pub(crate) fn decode_wayland_wm_state(raw: Option<WaylandConfigureState>) -> WindowManagerState {
    let Some(raw) = raw else {
        return WindowManagerState::default();
    };
    let decoration_mode = if raw.server_decorations {
        DecorationMode::Server
    } else {
        DecorationMode::Client
    };
    let tiled_edges = raw
        .tiled_edges
        .map(|[top, right, bottom, left]| TiledEdges::new(top, right, bottom, left));
    let actions = match raw.capabilities {
        Some(bits) => WindowActions {
            window_menu: Some((bits & WAYLAND_CAP_WINDOW_MENU) != 0),
            maximize: Some((bits & WAYLAND_CAP_MAXIMIZE) != 0),
            minimize: Some((bits & WAYLAND_CAP_MINIMIZE) != 0),
            fullscreen: Some((bits & WAYLAND_CAP_FULLSCREEN) != 0),
            // `xdg_toplevel::wm_capabilities` does not carry a close capability.
            close: None,
        },
        None => WindowActions::default(),
    };
    WindowManagerState { decoration_mode, tiled_edges, actions }
}

/// Pure protocol-version gate matching vendored winit's `WindowState::wayland_wm_state`:
/// tiled edges require `xdg_toplevel` v2+ and `wm_capabilities` requires v5+ (preventing
/// `sctk`'s default `WindowManagerCapabilities::all()` from leaking as known on older compositors).
#[cfg(test)]
pub(crate) fn gate_wayland_configure(
    server_decorations: bool,
    tiled_edges: [bool; 4],
    capabilities_bits: u16,
    xdg_toplevel_version: u32,
) -> WaylandConfigureState {
    WaylandConfigureState {
        server_decorations,
        tiled_edges: (xdg_toplevel_version >= 2).then_some(tiled_edges),
        capabilities: (xdg_toplevel_version >= 5).then_some(capabilities_bits),
    }
}

/// Reads confirmed Wayland `xdg_toplevel` / `xdg-decoration` state from `window`.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) fn read_wayland(window: &winit::window::Window) -> WindowManagerState {
    use winit::platform::wayland::WindowExtWayland as _;
    let raw = window.wayland_wm_state().map(|(server_decorations, tiled_edges, capabilities)| {
        WaylandConfigureState { server_decorations, tiled_edges, capabilities }
    });
    decode_wayland_wm_state(raw)
}

#[cfg(test)]
#[path = "linux_wm_state_tests.rs"]
mod tests;
