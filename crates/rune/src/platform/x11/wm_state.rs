//! X11 window-manager state queries (`_NET_FRAME_EXTENTS`, `_GTK_EDGE_CONSTRAINTS`,
//! `_NET_WM_STATE`, `_NET_WM_ALLOWED_ACTIONS`, `_NET_WORKAREA`, and root `_NET_SUPPORTED`).
//!
//! Property decoding lives in pure cross-platform functions tested on every host; only the
//! pipelined `x11rb` round trip runs on Unix X11 targets. Follows Mutter's `src/x11/window-x11.c`
//! and `src/core/window.c`, GTK4's `gdk/x11/gdkdisplay-x11.c`, Chromium's
//! `ui/ozone/platform/x11/x11_window.cc`, and freedesktop EWMH.

use crate::window::wm_state::{DecorationMode, TiledEdges, WindowActions, WindowManagerState};

/// `_GTK_EDGE_CONSTRAINTS` bit for a tiled top edge (`META_GTK_EDGE_CONSTRAINT_TOP_TILED`, bit 0).
pub(crate) const GTK_EDGE_TOP_TILED: u32 = 1 << 0;
/// `_GTK_EDGE_CONSTRAINTS` bit for a tiled right edge (`META_GTK_EDGE_CONSTRAINT_RIGHT_TILED`, bit 2).
pub(crate) const GTK_EDGE_RIGHT_TILED: u32 = 1 << 2;
/// `_GTK_EDGE_CONSTRAINTS` bit for a tiled bottom edge (`META_GTK_EDGE_CONSTRAINT_BOTTOM_TILED`, bit 4).
pub(crate) const GTK_EDGE_BOTTOM_TILED: u32 = 1 << 4;
/// `_GTK_EDGE_CONSTRAINTS` bit for a tiled left edge (`META_GTK_EDGE_CONSTRAINT_LEFT_TILED`, bit 6).
pub(crate) const GTK_EDGE_LEFT_TILED: u32 = 1 << 6;

/// Pixel tolerance when matching a half-tiled window's outer rect against its monitor work area.
const WORK_AREA_EDGE_TOLERANCE: i32 = 4;

/// Axis-aligned rectangle in root X11 pixels (`x`, `y`, `width`, `height`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PixelRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl PixelRect {
    pub(crate) const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    pub(crate) fn right(self) -> i32 {
        self.x.saturating_add(i32::try_from(self.width).unwrap_or(i32::MAX))
    }

    pub(crate) fn bottom(self) -> i32 {
        self.y.saturating_add(i32::try_from(self.height).unwrap_or(i32::MAX))
    }

    /// Returns the non-empty intersection of two rectangles, or `None` if disjoint.
    pub(crate) fn intersect(self, other: Self) -> Option<Self> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Self {
            x: x0,
            y: y0,
            width: u32::try_from(x1 - x0).ok()?,
            height: u32::try_from(y1 - y0).ok()?,
        })
    }
}

/// Interned EWMH/GTK atoms used when decoding `_NET_WM_ALLOWED_ACTIONS`, `_NET_WM_STATE`, and
/// `_NET_SUPPORTED`. `0` represents an atom the X server has not interned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ActionAtoms {
    pub(crate) gtk_show_window_menu: u32,
    pub(crate) minimize: u32,
    pub(crate) maximize_horz: u32,
    pub(crate) maximize_vert: u32,
    pub(crate) fullscreen: u32,
    pub(crate) close: u32,
    pub(crate) state_maximized_horz: u32,
    pub(crate) state_maximized_vert: u32,
    pub(crate) state_fullscreen: u32,
}

/// Decodes EWMH `_NET_FRAME_EXTENTS` (`CARDINAL[4]`: left, right, top, bottom) into
/// [`DecorationMode`].
///
/// - While full screen, preserves `previous_mode` if already known (Mutter's `mutter-x11-frames`
///   collapses server frame borders to `[0, 0, 0, 0]` in full screen, and Wayland's
///   `xdg-decoration` mode does not change in full screen). A framed titled window that launches
///   straight into full screen with no prior mode stays `Unknown`.
/// - Present with 4 cardinals and any `> 0` -> [`DecorationMode::Server`].
/// - Present with 4 zeroes `[0, 0, 0, 0]`:
///   - If `requested_decorations && has_wm_frame && previous_mode == Server`, preserves
///     [`DecorationMode::Server`] across the asynchronous 1-frame window after leaving full screen
///     while `mutter-x11-frames` restores non-zero `_NET_FRAME_EXTENTS`.
///   - Otherwise -> [`DecorationMode::Client`].
/// - Present with malformed length (`!= 4`) -> [`DecorationMode::Unknown`].
/// - Absent -> [`DecorationMode::Client`] only when the app explicitly asked for no native frame
///   (`!requested_decorations`, e.g. `WindowOptions { borderless: true, .. }`), the window manager
///   is managing the window (`wm_managed`), and the window was not reparented into a WM frame
///   (`!has_wm_frame`). Never infers `Server` from `requested_decorations` alone or from missing
///   actions.
pub(crate) fn decode_frame_extents(
    extents: Option<&[u32]>,
    requested_decorations: bool,
    wm_managed: bool,
    has_wm_frame: bool,
    is_fullscreen: bool,
    previous_mode: DecorationMode,
) -> DecorationMode {
    if is_fullscreen {
        if previous_mode != DecorationMode::Unknown {
            return previous_mode;
        }
        if requested_decorations && has_wm_frame {
            return DecorationMode::Unknown;
        }
    }
    match extents {
        Some([left, right, top, bottom]) => {
            if (*left | *right | *top | *bottom) != 0 {
                DecorationMode::Server
            } else if requested_decorations && has_wm_frame && previous_mode == DecorationMode::Server {
                DecorationMode::Server
            } else {
                DecorationMode::Client
            }
        }
        Some(_) => DecorationMode::Unknown,
        None => {
            if !requested_decorations && wm_managed && !has_wm_frame {
                DecorationMode::Client
            } else {
                DecorationMode::Unknown
            }
        }
    }
}

/// Decodes `_GTK_EDGE_CONSTRAINTS` (`CARDINAL[1]` bitmask) into `Option<TiledEdges>`.
///
/// - Present with one cardinal: decodes tiled bits 0 (top), 2 (right), 4 (bottom), 6 (left);
///   resizable bits (1, 3, 5, 7) and higher bits are ignored.
/// - Present with a malformed length (0 or more than 1 cardinal): returns `None` (unknown).
/// - Absent on the window while `_GTK_EDGE_CONSTRAINTS` is advertised in root `_NET_SUPPORTED`:
///   returns `Some(TiledEdges::NONE)` (known untiled).
/// - Absent and not advertised: returns `None` (unknown).
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

/// Extracts the work area for `desktop` from root `_NET_WORKAREA` (`CARDINAL[4 * desktops]`),
/// intersecting with `monitor` when available so multi-monitor setups keep the active monitor's
/// work-area edges (matching GTK4's `gdk_x11_screen_get_work_area`).
pub(crate) fn resolve_work_area(
    net_workarea: Option<&[u32]>,
    current_desktop: Option<u32>,
    monitor: Option<PixelRect>,
) -> Option<PixelRect> {
    let from_prop = net_workarea.and_then(|vals| {
        if vals.len() < 4 || !vals.len().is_multiple_of(4) {
            return None;
        }
        let idx = usize::try_from(current_desktop.unwrap_or(0)).ok()?;
        let base = idx.checked_mul(4)?;
        let slice = vals.get(base..base + 4).or_else(|| vals.get(0..4))?;
        let x = i32::try_from(slice[0]).ok()?;
        let y = i32::try_from(slice[1]).ok()?;
        let w = slice[2];
        let h = slice[3];
        (w > 0 && h > 0).then_some(PixelRect::new(x, y, w, h))
    });
    match (monitor, from_prop) {
        (Some(m), Some(wa)) => m.intersect(wa).or(Some(m)),
        (Some(m), None) => Some(m),
        (None, Some(wa)) => Some(wa),
        (None, None) => None,
    }
}

/// Refines decoded `_GTK_EDGE_CONSTRAINTS` with `_NET_WM_STATE` maximization and work-area edge
/// contact.
///
/// - When Mutter reparents a server-decorated window into `priv->frame->xwindow`, Mutter writes
///   `_GTK_EDGE_CONSTRAINTS` exclusively on the frame window (`src/x11/window-x11.c`), so when
///   `has_wm_frame` is true the frame window's property takes precedence over any stale pre-frame
///   property left on the client window.
/// - When `_NET_WM_STATE` has both `MAXIMIZED_HORZ` and `MAXIMIZED_VERT`, reports
///   `Some(TiledEdges::ALL)`, matching GTK4, Mutter's `update_edge_constraints`, Chromium's
///   `X11Window::GetTiledState`, borderless X11, and Wayland.
/// - When `_NET_WM_STATE` has `MAXIMIZED_VERT` without `MAXIMIZED_HORZ` (and the horizontal
///   twin), checks flush contact against `work_area` so Ubuntu's `ubuntu-tiling-assistant`
///   half-tiles report the side edge without affecting floating vertically-maximized windows.
pub(crate) fn decode_tiled_edges(raw: RawX11WmState<'_>, edges_advertised: bool) -> Option<TiledEdges> {
    let raw_constraints = if raw.has_wm_frame {
        raw.frame_edge_constraints.or(raw.client_edge_constraints)
    } else {
        raw.client_edge_constraints.or(raw.frame_edge_constraints)
    };
    let mut edges = decode_edge_constraints(raw_constraints, edges_advertised);

    let has_vert = raw.wm_state.is_some_and(|list| {
        super::window_menu::advertised(list, raw.action_atoms.state_maximized_vert)
    });
    let has_horz = raw.wm_state.is_some_and(|list| {
        super::window_menu::advertised(list, raw.action_atoms.state_maximized_horz)
    });

    if has_vert && has_horz {
        return Some(TiledEdges::ALL);
    }

    if let (Some(outer), Some(wa)) = (raw.outer_rect, raw.work_area) {
        let tol = WORK_AREA_EDGE_TOLERANCE;
        let tol_u = u32::try_from(tol).unwrap_or(0);
        if has_vert && !has_horz && outer.width < wa.width.saturating_sub(tol_u) {
            let touches_left = (outer.x - wa.x).abs() <= tol;
            let touches_right = (outer.right() - wa.right()).abs() <= tol;
            if touches_left || touches_right {
                let mut e = edges.unwrap_or(TiledEdges::NONE);
                e.top = true;
                e.bottom = true;
                if touches_left {
                    e.left = true;
                }
                if touches_right {
                    e.right = true;
                }
                edges = Some(e);
            }
        } else if has_horz && !has_vert && outer.height < wa.height.saturating_sub(tol_u) {
            let touches_top = (outer.y - wa.y).abs() <= tol;
            let touches_bottom = (outer.bottom() - wa.bottom()).abs() <= tol;
            if touches_top || touches_bottom {
                let mut e = edges.unwrap_or(TiledEdges::NONE);
                e.left = true;
                e.right = true;
                if touches_top {
                    e.top = true;
                }
                if touches_bottom {
                    e.bottom = true;
                }
                edges = Some(e);
            }
        }
    }

    edges
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

/// Raw snapshot of X11 window properties and geometry passed into [`decode_wm_state`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RawX11WmState<'a> {
    pub(crate) frame_extents: Option<&'a [u32]>,
    pub(crate) client_edge_constraints: Option<&'a [u32]>,
    pub(crate) frame_edge_constraints: Option<&'a [u32]>,
    pub(crate) wm_state: Option<&'a [u32]>,
    pub(crate) allowed_actions: Option<&'a [u32]>,
    pub(crate) net_supported: Option<&'a [u32]>,
    pub(crate) gtk_edge_constraints_atom: u32,
    pub(crate) action_atoms: ActionAtoms,
    pub(crate) requested_decorations: bool,
    pub(crate) has_wm_frame: bool,
    pub(crate) is_fullscreen: bool,
    pub(crate) previous_decoration_mode: DecorationMode,
    pub(crate) outer_rect: Option<PixelRect>,
    pub(crate) work_area: Option<PixelRect>,
}

/// Decodes a complete [`WindowManagerState`] from raw X11 property slices and geometry.
pub(crate) fn decode_wm_state(raw: RawX11WmState<'_>) -> WindowManagerState {
    let edges_advertised = raw
        .net_supported
        .is_some_and(|list| super::window_menu::advertised(list, raw.gtk_edge_constraints_atom));
    let wm_managed = raw.wm_state.is_some() || raw.allowed_actions.is_some();
    let is_fullscreen = raw.is_fullscreen
        || raw.wm_state.is_some_and(|list| {
            super::window_menu::advertised(list, raw.action_atoms.state_fullscreen)
        });
    WindowManagerState {
        decoration_mode: decode_frame_extents(
            raw.frame_extents,
            raw.requested_decorations,
            wm_managed,
            raw.has_wm_frame,
            is_fullscreen,
            raw.previous_decoration_mode,
        ),
        tiled_edges: decode_tiled_edges(raw, edges_advertised),
        actions: decode_allowed_actions(raw.allowed_actions, raw.net_supported, raw.action_atoms),
    }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod native {
    use super::{ActionAtoms, PixelRect, RawX11WmState, decode_wm_state, resolve_work_area};
    use crate::window::wm_state::{DecorationMode, WindowManagerState};
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
        requested_decorations: bool,
        is_fullscreen: bool,
        previous_decoration_mode: DecorationMode,
        monitor: Option<PixelRect>,
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
        let c_wm_state = conn.intern_atom(true, b"_NET_WM_STATE")?;
        let c_state_max_h = conn.intern_atom(true, b"_NET_WM_STATE_MAXIMIZED_HORZ")?;
        let c_state_max_v = conn.intern_atom(true, b"_NET_WM_STATE_MAXIMIZED_VERT")?;
        let c_state_fullscreen = conn.intern_atom(true, b"_NET_WM_STATE_FULLSCREEN")?;
        let c_workarea = conn.intern_atom(true, b"_NET_WORKAREA")?;
        let c_desktop = conn.intern_atom(true, b"_NET_CURRENT_DESKTOP")?;

        let a_supported = c_supported.reply()?.atom;
        let a_extents = c_extents.reply()?.atom;
        let a_edges = c_edges.reply()?.atom;
        let a_menu = c_menu.reply()?.atom;
        let a_allowed = c_allowed.reply()?.atom;
        let a_wm_state = c_wm_state.reply()?.atom;
        let a_workarea = c_workarea.reply()?.atom;
        let a_desktop = c_desktop.reply()?.atom;
        let action_atoms = ActionAtoms {
            gtk_show_window_menu: a_menu,
            minimize: c_minimize.reply()?.atom,
            maximize_horz: c_max_h.reply()?.atom,
            maximize_vert: c_max_v.reply()?.atom,
            fullscreen: c_fullscreen.reply()?.atom,
            close: c_close.reply()?.atom,
            state_maximized_horz: c_state_max_h.reply()?.atom,
            state_maximized_vert: c_state_max_v.reply()?.atom,
            state_fullscreen: c_state_fullscreen.reply()?.atom,
        };

        // Pipeline all property reads, parent tree query, and geometry lookup in one round trip.
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
        let p_wm_state = (a_wm_state != 0)
            .then(|| conn.get_property(false, xwindow, a_wm_state, AtomEnum::ATOM, 0, 64))
            .transpose()?;
        let p_workarea = (a_workarea != 0)
            .then(|| conn.get_property(false, root, a_workarea, AtomEnum::CARDINAL, 0, 256))
            .transpose()?;
        let p_desktop = (a_desktop != 0)
            .then(|| conn.get_property(false, root, a_desktop, AtomEnum::CARDINAL, 0, 4))
            .transpose()?;
        let q_tree = conn.query_tree(xwindow)?;
        let q_geom = conn.get_geometry(xwindow)?;
        let q_pos = conn.translate_coordinates(xwindow, root, 0, 0)?;

        let net_supported = match p_supported {
            Some(c) => property_u32s(&c.reply()?, AtomEnum::ATOM, false),
            None => None,
        };
        let frame_extents = match p_extents {
            Some(c) => property_u32s(&c.reply()?, AtomEnum::CARDINAL, true),
            None => None,
        };
        let client_edge_constraints = match p_edges {
            Some(c) => property_u32s(&c.reply()?, AtomEnum::CARDINAL, true),
            None => None,
        };
        let allowed_actions = match p_allowed {
            Some(c) => property_u32s(&c.reply()?, AtomEnum::ATOM, false),
            None => None,
        };
        let wm_state = match p_wm_state {
            Some(c) => property_u32s(&c.reply()?, AtomEnum::ATOM, false),
            None => None,
        };
        let net_workarea = match p_workarea {
            Some(c) => property_u32s(&c.reply()?, AtomEnum::CARDINAL, false),
            None => None,
        };
        let current_desktop = match p_desktop {
            Some(c) => property_u32s(&c.reply()?, AtomEnum::CARDINAL, true)
                .and_then(|v| (v.len() == 1).then_some(v[0])),
            None => None,
        };
        let parent = q_tree.reply()?.parent;
        let has_wm_frame = parent != 0 && parent != root;
        let geom = q_geom.reply()?;
        let pos = q_pos.reply()?;

        // Mutter writes `_GTK_EDGE_CONSTRAINTS` exclusively on `priv->frame->xwindow` once
        // reparented, so always read the parent frame's property when `has_wm_frame` is true
        // rather than letting a stale pre-frame property on `xwindow` shadow it.
        let frame_edge_constraints = if has_wm_frame && a_edges != 0 {
            let reply = conn.get_property(false, parent, a_edges, AtomEnum::CARDINAL, 0, 4)?.reply()?;
            property_u32s(&reply, AtomEnum::CARDINAL, true)
        } else {
            None
        };

        let (el, er, et, eb) = match frame_extents.as_deref() {
            Some([l, r, t, b]) => (*l, *r, *t, *b),
            _ => (0, 0, 0, 0),
        };
        let outer_rect = Some(PixelRect::new(
            i32::from(pos.dst_x).saturating_sub(i32::try_from(el).unwrap_or(0)),
            i32::from(pos.dst_y).saturating_sub(i32::try_from(et).unwrap_or(0)),
            u32::from(geom.width).saturating_add(el).saturating_add(er),
            u32::from(geom.height).saturating_add(et).saturating_add(eb),
        ));
        let work_area = resolve_work_area(net_workarea.as_deref(), current_desktop, monitor);

        Ok(Some(decode_wm_state(RawX11WmState {
            frame_extents: frame_extents.as_deref(),
            client_edge_constraints: client_edge_constraints.as_deref(),
            frame_edge_constraints: frame_edge_constraints.as_deref(),
            wm_state: wm_state.as_deref(),
            allowed_actions: allowed_actions.as_deref(),
            net_supported: net_supported.as_deref(),
            gtk_edge_constraints_atom: a_edges,
            action_atoms,
            requested_decorations,
            has_wm_frame,
            is_fullscreen,
            previous_decoration_mode,
            outer_rect,
            work_area,
        })))
    }

    /// Reads X11 window-manager state for `window` on the shared X11 connection.
    /// Returns `WindowManagerState::default()` off X11 or when the X server cannot be queried.
    pub(crate) fn read(window: &winit::window::Window, previous_decoration_mode: DecorationMode) -> WindowManagerState {
        let Some((xwindow, screen)) = handle(window) else {
            return WindowManagerState::default();
        };
        let requested_decorations = window.is_decorated();
        let is_fullscreen = window.fullscreen().is_some();
        let monitor = window.current_monitor().map(|m| {
            let p = m.position();
            let s = m.size();
            PixelRect::new(p.x, p.y, s.width, s.height)
        });
        super::super::query::with_connection(|q| {
            let Some(root) = q.screen_info(screen).map(|s| s.root) else {
                return super::super::query::QueryOutcome::NoAnswer;
            };
            super::super::query::classify_reply(query_state(
                &q.connection,
                root,
                xwindow,
                requested_decorations,
                is_fullscreen,
                previous_decoration_mode,
                monitor,
            ))
        })
        .unwrap_or_default()
    }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) use native::read;

#[cfg(test)]
#[path = "wm_state_tests.rs"]
mod tests;
