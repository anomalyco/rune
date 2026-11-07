//! A desktop app's shell: [`SplitView`] (sidebar, main pane, inspector, title bar band) with
//! its [`SidebarToggle`], [`PaneMotion`] and [`SidebarPeek`] settings, [`Toolbar`], [`Sidebar`]
//! with [`SidebarSection`], [`SidebarHeader`] and [`SidebarRow`], and the app-owned
//! [`PaneState`] behind each resizable pane and [`Reorder`] behind a list's drag to reorder;
//! a [`TabStrip`] with the app-owned [`TabDragState`] behind its gestures, and the
//! [`WindowTitlebar`] band it sits in.
//!
//! The module uses only rune's and rune-kit's public API.

mod pane;
mod reorder;
mod sidebar;
mod split;
mod tab_strip;
mod titlebar;
mod toolbar;

pub use pane::{PaneMotion, PaneState, SidebarPeek, is_sidebar_toggle};
pub use reorder::Reorder;
pub use sidebar::{INDENT, ROW_HEIGHT, RowStatus, Sidebar, SidebarHeader, SidebarRow, SidebarSection};
pub use split::{Page, SidebarToggle, SplitView};
pub use tab_strip::{TabDensity, TabDragMode, TabDragPhase, TabDragState, TabItem, TabItemStatus, TabOrientation, TabOverflow, TabReorder, TabStrip, TabTearMove, TabTearOut, TearOutSpec};
pub use titlebar::WindowTitlebar;
pub use toolbar::{TOOLBAR_HEIGHT, Toolbar};

use rune::Icon;

/// The sidebar toggle's glyph: a window with its leading pane drawn in.
pub static SIDEBAR: Icon = Icon {
    name: "kit-sidebar",
    view: 16.0,
    body: r#"<rect x="2" y="3" width="12" height="10" rx="2" stroke="currentColor" stroke-width="1" fill="none"/><path d="M6 3.5V12.5" stroke="currentColor" stroke-width="1"/>"#,
};
/// The inspector toggle's glyph: the same window with its trailing pane drawn in.
pub static INSPECTOR: Icon = Icon {
    name: "kit-inspector",
    view: 16.0,
    body: r#"<rect x="2" y="3" width="12" height="10" rx="2" stroke="currentColor" stroke-width="1" fill="none"/><path d="M10 3.5V12.5" stroke="currentColor" stroke-width="1"/>"#,
};
