//! A full-window screen sharing [`SplitView`]'s title bar geometry and content card.

use rune::El;

use super::{SidebarToggle, SplitView};
use crate::Kit;
use crate::shell::toolbar::Toolbar;

/// A full-window screen without side panes — a settings page, lock screen or onboarding view
/// in the same window as a [`SplitView`]. Shares [`SplitView`]'s title bar height, traffic-light
/// clearance, window-drag band and framed content card so switching screens matches pixel for
/// pixel: `Page::new(content).title("Settings").leading(back).trailing(done).titlebar_inset(cx.titlebar_inset()).build(&kit)`.
pub struct Page {
    main: El,
    toolbar: Toolbar,
    inset: f32,
    toggle: SidebarToggle,
    frame: Option<f32>,
    flat: bool,
    in_panel: bool,
}

impl Page {
    pub fn new(main: El) -> Page {
        Page {
            main,
            toolbar: Toolbar::new(),
            inset: 0.0,
            toggle: SidebarToggle::new(),
            frame: None,
            flat: false,
            in_panel: false,
        }
    }
    /// A centred title in the toolbar band.
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.toolbar = self.toolbar.title(t);
        self
    }
    /// The toolbar's centre slot (replaces [`Self::title`]).
    pub fn center(mut self, e: El) -> Self {
        self.toolbar = self.toolbar.center(e);
        self
    }
    /// Adds a leading control to the toolbar (a Back button, say), placed past the traffic lights.
    pub fn leading(mut self, e: El) -> Self {
        self.toolbar = self.toolbar.leading(e);
        self
    }
    /// Adds a trailing control to the toolbar.
    pub fn trailing(mut self, e: El) -> Self {
        self.toolbar = self.toolbar.trailing(e);
        self
    }
    /// Replaces the toolbar with a pre-built [`Toolbar`].
    pub fn toolbar(mut self, t: Toolbar) -> Self {
        self.toolbar = t;
        self
    }
    /// Points the traffic lights take at the window's leading edge: `Cx::titlebar_inset`.
    pub fn titlebar_inset(mut self, points: f32) -> Self {
        self.inset = points.max(0.0);
        self
    }
    /// Points past the traffic lights (or the window's edge in full screen) where the toolbar's
    /// leading slot starts, matching [`SplitView::sidebar_toggle`].
    pub fn past_lights(mut self, past_lights: f32, edge: f32) -> Self {
        self.toggle = self.toggle.past_lights(past_lights).edge(edge);
        self
    }
    /// The window background showing around the content pane (default 8 points, or 0 points
    /// when [`Self::flat_main`] is enabled, matching [`SplitView::frame_inset`]).
    pub fn frame_inset(mut self, points: f32) -> Self {
        self.frame = Some(points.max(0.0));
        self
    }
    /// Draws the content pane flat instead of as a rounded card, matching
    /// [`SplitView::flat_main`].
    ///
    /// ```
    /// # use rune::el;
    /// # use rune_kit::{Kit, shell::Page};
    /// # let body = el();
    /// # let kit = Kit::neutral(true);
    /// # let _ =
    /// Page::new(body).title("Settings").flat_main(true).build(&kit)
    /// # ;
    /// ```
    pub fn flat_main(mut self, on: bool) -> Self {
        self.flat = on;
        self
    }
    /// Puts the toolbar inside the framed content pane at its top, matching
    /// [`SplitView::toolbar_in_panel`].
    pub fn toolbar_in_panel(mut self) -> Self {
        self.in_panel = true;
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let mut split = SplitView::new(self.main)
            .toolbar(self.toolbar)
            .titlebar_inset(self.inset)
            .sidebar_toggle(self.toggle)
            .flat_main(self.flat)
            .hide_sidebar_toggle();
        if let Some(frame) = self.frame {
            split = split.frame_inset(frame);
        }
        if self.in_panel {
            split = split.toolbar_in_panel();
        }
        split.build(k)
    }
}
