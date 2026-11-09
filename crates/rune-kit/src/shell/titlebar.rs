//! A window's own titlebar band, laid out clear of the window's controls (`ChromeInsets`).

use rune::{ChromeInsets, Color, Direction, El, row};

use crate::Kit;

/// The band's height where the window keeps its native frame (`ChromeInsets::height` is 0).
const HEIGHT: f32 = 44.0;

/// A titlebar the app draws: leading controls, a [`TabStrip`](super::TabStrip), and trailing
/// controls, in a `.window_drag()` band that keeps clear of the traffic lights on macOS and
/// the caption buttons on Windows.
/// `WindowTitlebar::from_insets(cx.chrome_insets(), zoom).leading(menu).tabs(strip).build(&kit)`.
///
/// Its buttons and tabs take their presses; the gaps between tabs, the bands above and below
/// them and the empty stretch after the strip move the window, double-click to zoom it and, on
/// Windows, right-click for the window menu.
pub struct WindowTitlebar {
    key: String,
    height: f32,
    top_pad: f32,
    leading_inset: f32,
    trailing_inset: f32,
    leading_pad: f32,
    trailing_pad: f32,
    gap: f32,
    leading: Vec<El>,
    tabs: Option<El>,
    trailing: Vec<El>,
    fill: Option<Color>,
    direction: Direction,
}

impl WindowTitlebar {
    /// Sized from the window's insets at UI zoom `zoom` (1 unscaled): the band is as tall as
    /// the window's titlebar and reserves `insets.leading / zoom` and `insets.trailing / zoom`.
    pub fn from_insets(insets: ChromeInsets, zoom: f32) -> Self {
        let z = if zoom > 0.0 { zoom } else { 1.0 };
        WindowTitlebar {
            key: "titlebar".into(),
            height: if insets.height > 0.0 { insets.height / z } else { HEIGHT },
            top_pad: 0.0,
            leading_inset: insets.leading / z,
            trailing_inset: insets.trailing / z,
            leading_pad: 14.0,
            trailing_pad: 12.0,
            gap: 6.0,
            leading: Vec::new(),
            tabs: None,
            trailing: Vec::new(),
            fill: None,
            direction: Direction::Ltr,
        }
    }
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }
    /// The band's height in points, in place of the insets'.
    pub fn height(mut self, logical_height: f32) -> Self {
        self.height = logical_height.max(0.0);
        self
    }
    /// Points above the controls (8 on macOS, whose 28 point row sits low in a 36 point band).
    pub fn top_padding(mut self, points: f32) -> Self {
        self.top_pad = points.max(0.0);
        self
    }
    /// Points past the leading inset (default 14; 10 on macOS, 9 beside side tabs on Windows).
    pub fn leading_padding(mut self, points: f32) -> Self {
        self.leading_pad = points.max(0.0);
        self
    }
    /// Points before the trailing inset (default 12).
    pub fn trailing_padding(mut self, points: f32) -> Self {
        self.trailing_pad = points.max(0.0);
        self
    }
    /// Points between items (default 6).
    pub fn gap(mut self, points: f32) -> Self {
        self.gap = points.max(0.0);
        self
    }
    /// The band's colour (default `bg_deep`).
    pub fn fill(mut self, color: Color) -> Self {
        self.fill = Some(color);
        self
    }
    pub fn leading(mut self, el: El) -> Self {
        self.leading.push(el);
        self
    }
    /// A horizontal strip, which takes the space between the sides. Its trailing action hugs the
    /// last tab, and the rest of its space stays bare, moving the window.
    pub fn tabs(mut self, strip: El) -> Self {
        self.tabs = Some(strip);
        self
    }
    pub fn trailing(mut self, el: El) -> Self {
        self.trailing.push(el);
        self
    }
    /// The direction the band sits in. Inside [`El::dir`](rune::El::dir)`(Direction::Rtl)` pass
    /// `Rtl`: its controls and tabs run from the right, while the room kept for the window's
    /// controls stays on their side (the caption buttons' on the right, the traffic lights' on
    /// the left), as Desktop's titlebar does under `dir="rtl"`. `Ltr` (the default) and `Auto`
    /// lay it out left to right.
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let slot = row().key(format!("{}-tabs", self.key)).grow(1.0).shrink().min_w(0.0).h_full().center_y().children(self.tabs);
        // Right to left the row's left padding is its start side: the insets keep their sides.
        let (left_inset, right_inset) = match self.direction {
            Direction::Rtl => (self.trailing_inset, self.leading_inset),
            Direction::Ltr | Direction::Auto => (self.leading_inset, self.trailing_inset),
        };
        row()
            .key(self.key)
            .role("toolbar")
            .w_full()
            .h(self.height)
            .pt(self.top_pad)
            .pl(left_inset + self.leading_pad)
            .pr(right_inset + self.trailing_pad)
            .gap(self.gap)
            .center_y()
            .fill(self.fill.unwrap_or(k.c.bg_deep))
            .window_drag()
            .children(self.leading)
            .child(slot)
            .children(self.trailing)
    }
}
