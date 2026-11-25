//! X11 window-manager state queries (`_NET_FRAME_EXTENTS`, `_GTK_EDGE_CONSTRAINTS`,
//! `_NET_WM_ALLOWED_ACTIONS`, and root `_NET_SUPPORTED`).
//!
//! Property decoding lives in pure cross-platform functions tested on every host; only the
//! pipelined `x11rb` round trip runs on Unix X11 targets. Follows Chromium's
//! `ui/ozone/platform/x11`, GTK4's `gdk/x11` (`_GTK_EDGE_CONSTRAINTS`), and freedesktop EWMH.

use crate::window::wm_state::{DecorationMode, TiledEdges, WindowActions, WindowManagerState};

/// `_GTK_EDGE_CONSTRAINTS` bit for a tiled top edge (`GDK_TOP_TILED`, bit 0).
pub(crate) const GTK_EDGE_TOP_TILED: u32 = 1 << 0;
/// `_GTK_EDGE_CONSTRAINTS` bit for a tiled right edge (`GDK_RIGHT_TILED`, bit 2).
pub(crate) const GTK_EDGE_RIGHT_TILED: u32 = 1 << 2;
/// `_GTK_EDGE_CONSTRAINTS` bit for a tiled bottom edge (`GDK_BOTTOM_TILED`, bit 4).
pub(crate) const GTK_EDGE_BOTTOM_TILED: u32 = 1 << 4;
/// `_GTK_EDGE_CONSTRAINTS` bit for a tiled left edge (`GDK_LEFT_TILED`, bit 6).
pub(crate) const GTK_EDGE_LEFT_TILED: u32 = 1 << 6;

/// Interned EWMH/GTK atoms used when decoding `_NET_WM_ALLOWED_ACTIONS` and `_NET_SUPPORTED`.
/// `0` represents an atom the X server has not interned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ActionAtoms {
    pub(crate) gtk_show_window_menu: u32,
    pub(crate) minimize: u32,
    pub(crate) maximize_horz: u32,
    pub(crate) maximize_vert: u32,
    pub(crate) fullscreen: u32,
    pub(crate) close: u32,
}

/// Decodes EWMH `_NET_FRAME_EXTENTS` (`CARDINAL[4]`: left, right, top, bottom).
///
/// Any non-zero extent means the window manager drew a frame ([`DecorationMode::Server`]);
/// four zeroes mean no server-side frame ([`DecorationMode::Client`]); an absent or
/// malformed property (length != 4) remains [`DecorationMode::Unknown`].
pub(crate) fn decode_frame_extents(extents: Option<&[u32]>) -> DecorationMode {
    match extents {
        Some([left, right, top, bottom]) => {
            if (*left | *right | *top | *bottom) != 0 {
                DecorationMode::Server
            } else {
                DecorationMode::Client
            }
        }
        _ => DecorationMode::Unknown,
    }
}

/// Decodes `_GTK_EDGE_CONSTRAINTS` (`CARDINAL[1]` bitmask) into `Option<TiledEdges>`.
///
/// - Present with one cardinal: decodes tiled bits 0 (top), 2 (right), 4 (bottom), 6 (left);
///   resizable bits (1, 3, 5, 7) and higher bits are ignored.
/// - Present with a malformed length (0 or more than 1 cardinal): returns `None` (unknown).
/// - Absent on the window while `_GTK_EDGE_CONSTRAINTS` is advertised in root `_NET_SUPPORTED`:
///   returns `Some(TiledEdges::NONE)` (known untiled, as Mutter clears the property when untiled).
/// - Absent and not advertised: returns `None` (unknown). Never guesses from
///   `_NET_WM_STATE_MAXIMIZED_VERT`.
pub(crate) fn decode_edge_constraints(values: Option<&[u32]>, advertised: bool) -> Option<TiledEdges> {
    match values {
        Some([mask]) => Some(TiledEdges::new(
            (mask & GTK_EDGE_TOP_TILED) != 0,
            (mask & GTK_EDGE_RIGHT_TILED) != 0,
            (mask & GTK_EDGE_BOTTOM_TILED) != 0,
            (mask & GTK_EDGE_LEFT_TILED) != 0,
        )),
        Some(_) => None,
        None => advertised.then_some(TiledEdges::NONE),
    }
}

/// Decodes per-window `_NET_WM_ALLOWED_ACTIONS` and root `_NET_SUPPORTED` into [`WindowActions`].
///
/// - `window_menu`: `Some(true/false)` when root `_NET_SUPPORTED` is present (testing
///   `_GTK_SHOW_WINDOW_MENU`), `None` when `_NET_SUPPORTED` is absent.
/// - `minimize`, `maximize` (requiring both `_NET_WM_ACTION_MAXIMIZE_HORZ` and
///   `_NET_WM_ACTION_MAXIMIZE_VERT`), `fullscreen`, and `close`: `Some(true/false)` when
///   per-window `_NET_WM_ALLOWED_ACTIONS` is present, `None` when absent.
pub(crate) fn decode_allowed_actions(
    allowed: Option<&[u32]>,
    supported: Option<&[u32]>,
    atoms: ActionAtoms,
) -> WindowActions {
    let has = |list: &[u32], atom: u32| super::window_menu::advertised(list, atom);
    let window_menu = supported.map(|list| has(list, atoms.gtk_show_window_menu));
    let (maximize, minimize, fullscreen, close) = match allowed {
        Some(list) => (
            Some(has(list, atoms.maximize_horz) && has(list, atoms.maximize_vert)),
            Some(has(list, atoms.minimize)),
            Some(has(list, atoms.fullscreen)),
            Some(has(list, atoms.close)),
        ),
        None => (None, None, None, None),
    };
    WindowActions { window_menu, maximize, minimize, fullscreen, close }
}

/// Decodes a complete [`WindowManagerState`] from raw X11 property slices.
///
/// `decoration_mode` comes solely from `_NET_FRAME_EXTENTS` and is never inferred from missing
/// actions or `_NET_SUPPORTED`.
pub(crate) fn decode_wm_state(
    frame_extents: Option<&[u32]>,
    edge_constraints: Option<&[u32]>,
    allowed_actions: Option<&[u32]>,
    net_supported: Option<&[u32]>,
    gtk_edge_constraints_atom: u32,
    action_atoms: ActionAtoms,
) -> WindowManagerState {
    let edges_advertised = net_supported
        .is_some_and(|list| super::window_menu::advertised(list, gtk_edge_constraints_atom));
    WindowManagerState {
        decoration_mode: decode_frame_extents(frame_extents),
        tiled_edges: decode_edge_constraints(edge_constraints, edges_advertised),
        actions: decode_allowed_actions(allowed_actions, net_supported, action_atoms),
    }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod native {
    use super::{ActionAtoms, decode_wm_state};
    use crate::window::wm_state::WindowManagerState;
    use x11rb::errors::ReplyError;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, GetPropertyReply};

    /// The X11 window ID and screen behind `window`, or `None` off X11 (e.g. Wayland).
    fn handle(window: &winit::window::Window) -> Option<(u32, Option<usize>)> {
        use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
        let xwindow = match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(w) => u32::try_from(w.window).ok()?,
            RawWindowHandle::Xcb(w) => w.window.get(),
            _ => return None,
        };
        let screen = match window.display_handle().ok().map(|h| h.as_raw()) {
            Some(RawDisplayHandle::Xlib(d)) => usize::try_from(d.screen).ok(),
            Some(RawDisplayHandle::Xcb(d)) => usize::try_from(d.screen).ok(),
            _ => None,
        };
        Some((xwindow, screen))
    }

    fn property_u32s(reply: &GetPropertyReply, expected_type: AtomEnum, reject_truncated: bool) -> Option<Vec<u32>> {
        if reply.type_ != u32::from(expected_type) || reply.format != 32 {
            return None;
        }
        if reject_truncated && reply.bytes_after != 0 {
            return None;
        }
        Some(reply.value32()?.collect())
    }

    fn query_state(
        conn: &x11rb::rust_connection::RustConnection,
        root: u32,
        xwindow: u32,
    ) -> Result<Option<WindowManagerState>, ReplyError> {
        // Pipeline all atom lookups in one round trip; `only_if_exists = true` avoids interning
        // unused atoms on servers without EWMH/GTK support.
        let c_supported = conn.intern_atom(true, b"_NET_SUPPORTED")?;
        let c_extents = conn.intern_atom(true, b"_NET_FRAME_EXTENTS")?;
        let c_edges = conn.intern_atom(true, b"_GTK_EDGE_CONSTRAINTS")?;
        let c_menu = conn.intern_atom(true, b"_GTK_SHOW_WINDOW_MENU")?;
        let c_allowed = conn.intern_atom(true, b"_NET_WM_ALLOWED_ACTIONS")?;
        let c_minimize = conn.intern_atom(true, b"_NET_WM_ACTION_MINIMIZE")?;
        let c_max_h = conn.intern_atom(true, b"_NET_WM_ACTION_MAXIMIZE_HORZ")?;
        let c_max_v = conn.intern_atom(true, b"_NET_WM_ACTION_MAXIMIZE_VERT")?;
        let c_fullscreen = conn.intern_atom(true, b"_NET_WM_ACTION_FULLSCREEN")?;
        let c_close = conn.intern_atom(true, b"_NET_WM_ACTION_CLOSE")?;

        let a_supported = c_supported.reply()?.atom;
        let a_extents = c_extents.reply()?.atom;
        let a_edges = c_edges.reply()?.atom;
        let a_menu = c_menu.reply()?.atom;
        let a_allowed = c_allowed.reply()?.atom;
        let action_atoms = ActionAtoms {
            gtk_show_window_menu: a_menu,
            minimize: c_minimize.reply()?.atom,
            maximize_horz: c_max_h.reply()?.atom,
            maximize_vert: c_max_v.reply()?.atom,
            fullscreen: c_fullscreen.reply()?.atom,
            close: c_close.reply()?.atom,
        };

        // Pipeline all property reads in one round trip.
        let p_supported = (a_supported != 0)
            .then(|| conn.get_property(false, root, a_supported, AtomEnum::ATOM, 0, 4096))
            .transpose()?;
        let p_extents = (a_extents != 0)
            .then(|| conn.get_property(false, xwindow, a_extents, AtomEnum::CARDINAL, 0, 8))
            .transpose()?;
        let p_edges = (a_edges != 0)
            .then(|| conn.get_property(false, xwindow, a_edges, AtomEnum::CARDINAL, 0, 4))
            .transpose()?;
        let p_allowed = (a_allowed != 0)
            .then(|| conn.get_property(false, xwindow, a_allowed, AtomEnum::ATOM, 0, 256))
            .transpose()?;

        let net_supported = match p_supported {
            Some(c) => property_u32s(&c.reply()?, AtomEnum::ATOM, false),
            None => None,
        };
        let frame_extents = match p_extents {
            Some(c) => property_u32s(&c.reply()?, AtomEnum::CARDINAL, true),
            None => None,
        };
        let edge_constraints = match p_edges {
            Some(c) => property_u32s(&c.reply()?, AtomEnum::CARDINAL, true),
            None => None,
        };
        let allowed_actions = match p_allowed {
            Some(c) => property_u32s(&c.reply()?, AtomEnum::ATOM, false),
            None => None,
        };

        Ok(Some(decode_wm_state(
            frame_extents.as_deref(),
            edge_constraints.as_deref(),
            allowed_actions.as_deref(),
            net_supported.as_deref(),
            a_edges,
            action_atoms,
        )))
    }

    /// Reads X11 window-manager state for `window` on the shared X11 connection.
    /// Returns `WindowManagerState::default()` off X11 or when the X server cannot be queried.
    pub(crate) fn read(window: &winit::window::Window) -> WindowManagerState {
        let Some((xwindow, screen)) = handle(window) else {
            return WindowManagerState::default();
        };
        super::super::query::with_connection(|q| {
            let Some(root) = q.screen_info(screen).map(|s| s.root) else {
                return super::super::query::QueryOutcome::NoAnswer;
            };
            super::super::query::classify_reply(query_state(&q.connection, root, xwindow))
        })
        .unwrap_or_default()
    }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) use native::read;

#[cfg(test)]
#[path = "wm_state_tests.rs"]
mod tests;
