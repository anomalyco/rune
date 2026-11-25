//! Reactive window-manager state exposed on [`Cx`](crate::Cx): negotiated decoration mode,
//! tiled edges, and allowed window actions.
//!
//! Every field preserves an explicit unknown state ([`DecorationMode::Unknown`], `None` for
//! tiled edges and each action) so views only adapt to facts the platform or window manager
//! actually reported and never guess from missing capabilities or `is_decorated()`.

/// How the window's outer frame (title bar and borders) is drawn, as confirmed by the window
/// manager or compositor.
///
/// Returned by [`Cx::decoration_mode`](crate::Cx::decoration_mode).
///
/// ```
/// # use rune::{Cx, DecorationMode, El, text};
/// fn header(_: &(), cx: &mut Cx<()>) -> El {
///     let label = match cx.decoration_mode() {
///         DecorationMode::Client => "Client frame",
///         DecorationMode::Server => "Server frame",
///         _ => "Unknown frame",
///     };
///     text(label)
/// }
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecorationMode {
    /// The platform or window manager did not report a decoration mode.
    #[default]
    Unknown,
    /// The window manager or compositor draws the window's frame (server-side decorations).
    Server,
    /// The application draws the window's frame (client-side decorations, or zero frame extents).
    Client,
}

impl DecorationMode {
    const fn bits(self) -> u32 {
        match self {
            Self::Unknown => 0,
            Self::Server => 1,
            Self::Client => 2,
        }
    }
}

/// Which edges of a window are tiled against a monitor edge or another window.
///
/// Returned inside `Option<TiledEdges>` by [`Cx::tiled_edges`](crate::Cx::tiled_edges), where
/// `None` means the platform or window manager did not report tiling state and
/// `Some(TiledEdges::NONE)` means it reported that no edge is tiled. Views drawing a client-side
/// frame typically square corners and drop outer shadows or resize borders along tiled edges
/// (matching Chromium's `WindowTiledEdges` and GTK4/libadwaita `.tiled-*` classes).
///
/// ```
/// # use rune::TiledEdges;
/// let left_half = TiledEdges::new(true, false, true, true);
/// assert!(left_half.any());
/// assert!(!left_half.is_empty());
/// assert!(TiledEdges::NONE.is_empty());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct TiledEdges {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl TiledEdges {
    /// No edges tiled.
    pub const NONE: Self = Self { top: false, right: false, bottom: false, left: false };
    /// All four edges tiled.
    pub const ALL: Self = Self { top: true, right: true, bottom: true, left: true };

    /// Creates a `TiledEdges` value in CSS edge order (top, right, bottom, left).
    #[inline]
    pub const fn new(top: bool, right: bool, bottom: bool, left: bool) -> Self {
        Self { top, right, bottom, left }
    }

    /// Whether at least one edge is tiled.
    #[inline]
    pub const fn any(self) -> bool {
        self.top || self.right || self.bottom || self.left
    }

    /// Whether no edge is tiled.
    #[inline]
    pub const fn is_empty(self) -> bool {
        !self.any()
    }

    const fn bits(self) -> u32 {
        (self.top as u32)
            | ((self.right as u32) << 1)
            | ((self.bottom as u32) << 2)
            | ((self.left as u32) << 3)
    }
}

/// Which standard window actions the window manager allows for this window.
///
/// Each field is `Some(true)` when explicitly allowed, `Some(false)` when explicitly disallowed
/// (or omitted from a reported capability/action list), and `None` when the platform or window
/// manager did not report availability.
///
/// Returned by [`Cx::window_actions`](crate::Cx::window_actions).
///
/// ```
/// # use rune::{Cx, El, text};
/// fn controls(_: &(), cx: &mut Cx<()>) -> El {
///     let show_maximize = cx.window_actions().maximize != Some(false);
///     text(if show_maximize { "Maximize" } else { "Fixed" })
/// }
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct WindowActions {
    /// Whether the window manager can open a window menu (`_GTK_SHOW_WINDOW_MENU` in root
    /// `_NET_SUPPORTED` on X11; `xdg_toplevel::wm_capabilities` on Wayland v5+). `None` when
    /// unknown (absent `_NET_SUPPORTED`, pre-configure or `< v5` on Wayland, non-Linux, overlays).
    pub window_menu: Option<bool>,
    /// Whether maximizing this window is allowed (`_NET_WM_ACTION_MAXIMIZE_HORZ` and
    /// `_NET_WM_ACTION_MAXIMIZE_VERT` on X11; `xdg_toplevel::wm_capabilities` combined with window
    /// resizability on Wayland, matching GTK4). `None` when unknown (absent
    /// `_NET_WM_ALLOWED_ACTIONS`, resizable window on Wayland `< v5` or pre-configure, non-Linux).
    pub maximize: Option<bool>,
    /// Whether minimizing this window is allowed (`_NET_WM_ACTION_MINIMIZE` on X11;
    /// `xdg_toplevel::wm_capabilities` on Wayland v5+). `None` when unknown.
    pub minimize: Option<bool>,
    /// Whether making this window full screen is allowed (`_NET_WM_ACTION_FULLSCREEN` on X11;
    /// `xdg_toplevel::wm_capabilities` on Wayland v5+). On non-resizable windows, X11/Mutter
    /// reports `Some(false)` while Wayland/Mutter allows full screen and reports `Some(true)`.
    /// `None` when unknown.
    pub fullscreen: Option<bool>,
    /// Whether closing this window is allowed (`_NET_WM_ACTION_CLOSE` on X11; window closeability
    /// on Wayland once configured, matching GTK4's `deletable` property since `xdg-shell` always
    /// lets a client close its own toplevel). `None` when unknown (absent `_NET_WM_ALLOWED_ACTIONS`
    /// on X11, pre-configure on Wayland, non-Linux, overlays).
    pub close: Option<bool>,
}

impl WindowActions {
    /// Sets [`Self::window_menu`].
    #[inline]
    pub const fn with_window_menu(mut self, window_menu: Option<bool>) -> Self {
        self.window_menu = window_menu;
        self
    }

    /// Sets [`Self::maximize`].
    #[inline]
    pub const fn with_maximize(mut self, maximize: Option<bool>) -> Self {
        self.maximize = maximize;
        self
    }

    /// Sets [`Self::minimize`].
    #[inline]
    pub const fn with_minimize(mut self, minimize: Option<bool>) -> Self {
        self.minimize = minimize;
        self
    }

    /// Sets [`Self::fullscreen`].
    #[inline]
    pub const fn with_fullscreen(mut self, fullscreen: Option<bool>) -> Self {
        self.fullscreen = fullscreen;
        self
    }

    /// Sets [`Self::close`].
    #[inline]
    pub const fn with_close(mut self, close: Option<bool>) -> Self {
        self.close = close;
        self
    }

    const fn tri_bits(value: Option<bool>) -> u32 {
        match value {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        }
    }

    /// 10-bit encoding of the five tri-state action fields (2 bits each).
    const fn bits(self) -> u32 {
        Self::tri_bits(self.window_menu)
            | (Self::tri_bits(self.maximize) << 2)
            | (Self::tri_bits(self.minimize) << 4)
            | (Self::tri_bits(self.fullscreen) << 6)
            | (Self::tri_bits(self.close) << 8)
    }
}

/// Snapshot of the window manager's confirmed decoration mode, tiled edges, and allowed actions.
///
/// Exposed reactively through [`Cx::decoration_mode`](crate::Cx::decoration_mode),
/// [`Cx::tiled_edges`](crate::Cx::tiled_edges), and
/// [`Cx::window_actions`](crate::Cx::window_actions), and settable in headless tests with
/// [`Runtime::set_window_manager_state`](crate::Runtime::set_window_manager_state).
///
/// ```
/// # use rune::{DecorationMode, TiledEdges, WindowActions, WindowManagerState};
/// let state = WindowManagerState::default()
///     .with_decoration_mode(DecorationMode::Client)
///     .with_tiled_edges(Some(TiledEdges::new(true, false, true, true)))
///     .with_actions(WindowActions::default().with_maximize(Some(false)));
/// assert_eq!(state.decoration_mode, DecorationMode::Client);
/// assert_eq!(state.tiled_edges, Some(TiledEdges::new(true, false, true, true)));
/// assert_eq!(state.actions.maximize, Some(false));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct WindowManagerState {
    pub decoration_mode: DecorationMode,
    /// `None` when unknown; `Some(TiledEdges::NONE)` when known untiled.
    pub tiled_edges: Option<TiledEdges>,
    pub actions: WindowActions,
}

impl WindowManagerState {
    /// Sets [`Self::decoration_mode`].
    #[inline]
    pub const fn with_decoration_mode(mut self, decoration_mode: DecorationMode) -> Self {
        self.decoration_mode = decoration_mode;
        self
    }

    /// Sets [`Self::tiled_edges`].
    #[inline]
    pub const fn with_tiled_edges(mut self, tiled_edges: Option<TiledEdges>) -> Self {
        self.tiled_edges = tiled_edges;
        self
    }

    /// Sets [`Self::actions`].
    #[inline]
    pub const fn with_actions(mut self, actions: WindowActions) -> Self {
        self.actions = actions;
        self
    }

    /// Packs the full state into 17 bits (bits `0..=16`) for `Measurement::chrome_bits`:
    /// - bits `0..=1`: `decoration_mode` (3 states)
    /// - bit `2`: `tiled_edges.is_some()`
    /// - bits `3..=6`: `tiled_edges` top/right/bottom/left when `Some`
    /// - bits `7..=16`: five `WindowActions` tri-state fields (2 bits each)
    pub(crate) const fn bits(self) -> u32 {
        let mode = self.decoration_mode.bits();
        let edges = match self.tiled_edges {
            None => 0,
            Some(edges) => 1 | (edges.bits() << 1),
        };
        let actions = self.actions.bits();
        mode | (edges << 2) | (actions << 7)
    }
}

#[cfg(test)]
#[path = "wm_state_tests.rs"]
mod tests;
