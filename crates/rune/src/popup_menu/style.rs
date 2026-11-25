//! How a page draws a [`PopupMenu`](super::PopupMenu) on the web, where the menu is the page's
//! own elements rather than the system's. Native menus (macOS, Windows) draw as the system does
//! and ignore it.

use serde_json::{Map, Value};

use crate::Color;

/// A web pop-up menu's look: colours, font, spacing and corner radii. Each one left unset keeps
/// the built-in light or dark default, so a style only names what an app's palette changes.
/// Set it with [`PopupMenu::style`](super::PopupMenu::style); keyboard, pointer and screen
/// reader use are the same with any style.
///
/// ```
/// use rune::{Color, PopupItem, PopupMenu, PopupMenuStyle};
///
/// let style = PopupMenuStyle::new()
///     .background(Color::BLACK)
///     .text(Color::WHITE)
///     .corner_radius(0.0)
///     .font_family("Inter, sans-serif");
/// let menu = PopupMenu::new().item(PopupItem::new("open", "Open")).style(style.clone());
/// assert_eq!(menu.menu_style(), Some(&style));
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PopupMenuStyle {
    background: Option<Color>,
    text: Option<Color>,
    muted: Option<Color>,
    separator: Option<Color>,
    highlight: Option<Color>,
    font_family: Option<String>,
    font_size: Option<f32>,
    line_height: Option<f32>,
    padding: Option<f32>,
    corner_radius: Option<f32>,
    item_corner_radius: Option<f32>,
}

impl PopupMenuStyle {
    pub fn new() -> Self { Self::default() }

    /// The menu's fill.
    pub fn background(mut self, color: Color) -> Self { self.background = Some(color); self }
    /// Item titles.
    pub fn text(mut self, color: Color) -> Self { self.text = Some(color); self }
    /// Disabled items, shortcuts and submenu arrows.
    pub fn muted(mut self, color: Color) -> Self { self.muted = Some(color); self }
    /// Separators and the menu's border.
    pub fn separator(mut self, color: Color) -> Self { self.separator = Some(color); self }
    /// The hovered or keyboard-focused item's fill.
    pub fn highlight(mut self, color: Color) -> Self { self.highlight = Some(color); self }
    /// A CSS font family list, such as `"Inter, sans-serif"`.
    pub fn font_family(mut self, family: impl Into<String>) -> Self { self.font_family = Some(family.into()); self }
    /// Item text size, in points.
    pub fn font_size(mut self, size: f32) -> Self { self.font_size = Some(size); self }
    /// Item line height, in points.
    pub fn line_height(mut self, height: f32) -> Self { self.line_height = Some(height); self }
    /// The inset between the menu's edge and its items, in points.
    pub fn padding(mut self, padding: f32) -> Self { self.padding = Some(padding); self }
    /// The menu's corner radius, in points.
    pub fn corner_radius(mut self, radius: f32) -> Self { self.corner_radius = Some(radius); self }
    /// A highlighted item's corner radius, in points.
    pub fn item_corner_radius(mut self, radius: f32) -> Self { self.item_corner_radius = Some(radius); self }

    /// The set values only, as the page reads them: colours as sRGB `#rrggbbaa`, the font size
    /// and line height when positive and finite, other lengths as non-negative finite points.
    /// The page sets the font family as its own style property, so a browser rejects a family it
    /// can't parse on its own and keeps the default.
    pub(crate) fn to_json(&self) -> Value {
        let mut v = Map::new();
        let colors = [("bg", self.background), ("fg", self.text), ("muted", self.muted), ("line", self.separator), ("hover", self.highlight)];
        for (name, color) in colors {
            if let Some(c) = color { v.insert(name.into(), c.hex().into()); }
        }
        if let Some(family) = self.font_family.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
            v.insert("fontFamily".into(), family.into());
        }
        // Zero-sized text would hide the items.
        for (name, length) in [("fontSize", self.font_size), ("lineHeight", self.line_height)] {
            if let Some(x) = length.filter(|x| x.is_finite() && *x > 0.0) { v.insert(name.into(), x.into()); }
        }
        let lengths = [("padding", self.padding), ("radius", self.corner_radius), ("itemRadius", self.item_corner_radius)];
        for (name, length) in lengths {
            if let Some(x) = length.filter(|x| x.is_finite()) { v.insert(name.into(), x.max(0.0).into()); }
        }
        Value::Object(v)
    }
}
