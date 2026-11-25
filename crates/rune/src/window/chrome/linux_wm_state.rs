//! Mapping Wayland `xdg_toplevel` / `xdg-decoration` configure state to [`WindowManagerState`].
//!
//! Pure decoding functions live here so every host tests protocol-version gating, tiled edge
//! decoding, decoration mode mapping, and capability bitmasks without a live Wayland compositor.
//! Follows Chromium Linux (`ui/ozone/platform/wayland/host/wayland_window.h` `WindowTiledEdges`),
//! GTK4 (`gdk/wayland/gdktoplevel-wayland.c` and `gtk/gtkwindowcontrols.c`), and `xdg-shell` v2/v5.

use crate::window::wm_state::{DecorationMode, TiledEdges, WindowActions, WindowManagerState};

/// `wayland_csd_frame::WindowManagerCapabilities::WINDOW_MENU` (bit 0).
pub(crate) const WAYLAND_CAP_WINDOW_MENU: u16 = 1 << 0;
/// `wayland_csd_frame::WindowManagerCapabilities::MAXIMIZE` (bit 1).
pub(crate) const WAYLAND_CAP_MAXIMIZE: u16 = 1 << 1;
/// `wayland_csd_frame::WindowManagerCapabilities::FULLSCREEN` (bit 2).
pub(crate) const WAYLAND_CAP_FULLSCREEN: u16 = 1 << 2;
/// `wayland_csd_frame::WindowManagerCapabilities::MINIMIZE` (bit 3).
pub(crate) const WAYLAND_CAP_MINIMIZE: u16 = 1 << 3;

/// Raw Wayland configure state plus window policy reported for a configured `xdg_toplevel`.
///
/// - `server_decorations`: `true` when `xdg-decoration` negotiated `Server`, `false` for `Client`.
/// - `tiled_edges`: `Some([top, right, bottom, left])` when `xdg_toplevel.version() >= 2`, else `None`.
/// - `capabilities`: `Some(bits)` when `xdg_toplevel.version() >= 5`, else `None`.
/// - `resizable`: whether the window itself allows resizing/maximizing (`window.is_resizable()`
///   and `WindowButtons::MAXIMIZE`), matching GTK4's `gtkwindowcontrols.c` because
///   `xdg_toplevel.wm_capabilities` reports compositor support rather than per-window resizability.
/// - `closeable`: whether the window's close button is enabled (`WindowButtons::CLOSE`),
///   matching GTK4's `deletable` property because `xdg-shell` always lets a client close its own
///   toplevel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WaylandConfigureState {
    pub(crate) server_decorations: bool,
    pub(crate) tiled_edges: Option<[bool; 4]>,
    pub(crate) capabilities: Option<u16>,
    pub(crate) resizable: bool,
    pub(crate) closeable: bool,
}

/// Decodes raw Wayland configure state (or `None` before the initial configure) into
/// [`WindowManagerState`].
///
/// - Before initial configure (`None`): all fields remain unknown ([`WindowManagerState::default`]).
/// - `decoration_mode`: directly from `server_decorations` (`Server` vs `Client`), never inferred
///   from capabilities.
/// - `tiled_edges`: `Some(TiledEdges::new(t, r, b, l))` directly from the compositor's `TILED_*`
///   states (`xdg_toplevel` v2+), `None` below v2.
/// - `actions`:
///   - `window_menu`, `minimize`, `fullscreen`: decoded from `capabilities` on `xdg_toplevel` v5+,
///     `None` below v5.
///   - `maximize`: `Some(false)` when `!resizable` (matching GTK4 `gtkwindowcontrols.c` since
///     compositors like Mutter send `WM_CAPABILITIES_MAXIMIZE` unconditionally while refusing to
///     maximize non-resizable windows); otherwise decoded from `capabilities` on v5+ and `None`
///     below v5.
///   - `close`: `Some(closeable)` once configured (matching GTK4's `deletable` property since
///     Wayland always allows a client to close its own toplevel).
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
    let (window_menu, cap_maximize, minimize, fullscreen) = match raw.capabilities {
        Some(bits) => (
            Some((bits & WAYLAND_CAP_WINDOW_MENU) != 0),
            Some((bits & WAYLAND_CAP_MAXIMIZE) != 0),
            Some((bits & WAYLAND_CAP_MINIMIZE) != 0),
            Some((bits & WAYLAND_CAP_FULLSCREEN) != 0),
        ),
        None => (None, None, None, None),
    };
    let maximize = if !raw.resizable { Some(false) } else { cap_maximize };
    let actions = WindowActions {
        window_menu,
        maximize,
        minimize,
        fullscreen,
        close: Some(raw.closeable),
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
        resizable: true,
        closeable: true,
    }
}

/// Reads confirmed Wayland `xdg_toplevel` / `xdg-decoration` state from `window`.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) fn read_wayland(window: &winit::window::Window) -> WindowManagerState {
    use winit::platform::wayland::WindowExtWayland as _;
    use winit::window::WindowButtons;
    let buttons = window.enabled_buttons();
    let resizable = window.is_resizable() && buttons.contains(WindowButtons::MAXIMIZE);
    let closeable = buttons.contains(WindowButtons::CLOSE);
    let raw = window.wayland_wm_state().map(|(server_decorations, tiled_edges, capabilities)| {
        WaylandConfigureState {
            server_decorations,
            tiled_edges,
            capabilities,
            resizable,
            closeable,
        }
    });
    decode_wayland_wm_state(raw)
}

#[cfg(test)]
#[path = "linux_wm_state_tests.rs"]
mod tests;
