//! The shared look in Rune terms: colours, type and the tile frame every tile sits in.

use motion_bento_chaos::look::{self, Lch, color, grid};
use rune::{Color, El, TextStyle, col, oklcha, row, spacer, text};

pub fn c(l: Lch) -> Color {
    oklcha(l.l, l.c, l.h, l.a)
}

pub fn spring(s: look::Spring) -> rune::Spring {
    rune::Spring::new(s.duration, s.bounce)
}

pub fn font(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight).lh((size * 1.3).round())
}

/// Tabular figures, so changing numbers don't jitter sideways.
pub fn nums(size: f32, weight: f32) -> TextStyle {
    font(size, weight).features("'tnum' 1")
}

pub fn title(s: &str) -> El {
    text(s).font(font(13.0, 560.0)).color(c(color::TEXT_2))
}

pub fn caption(s: impl Into<String>) -> El {
    text(s.into()).font(font(12.0, 460.0)).color(c(color::TEXT_3))
}

/// A tile: a rounded card with a title row (title on the left, `aside` on the right) over `body`.
pub fn tile(key: &str, name: &str, aside: El, body: El) -> El {
    col()
        .key(key.to_owned())
        .radius(grid::RADIUS)
        .fill(c(color::TILE))
        .border(1.0, c(color::EDGE))
        .padding(grid::TILE_PAD)
        .gap(14.0)
        .child(row().key("head").h(20.0).center_y().gap(8.0).child(title(name)).child(spacer()).child(aside))
        .child(body)
}

/// Column width in the 4-column grid.
pub fn col_w() -> f32 {
    (grid::WIDTH - 2.0 * grid::PAD - 3.0 * grid::GAP) / 4.0
}

/// A tile's inside width for a span of `cols` columns.
pub fn inner_w(cols: f32) -> f32 {
    col_w() * cols + grid::GAP * (cols - 1.0) - 2.0 * grid::TILE_PAD
}

/// A tile's body height for a row of height `h`.
pub fn inner_h(h: f32) -> f32 {
    h - 2.0 * grid::TILE_PAD - 20.0 - 14.0
}

/// A rounded button in the well colour.
pub fn well_button(key: &str, label: &str, w: f32, h: f32) -> El {
    row()
        .key(key.to_owned())
        .size(w, h)
        .radius(h * 0.32)
        .center()
        .fill(c(color::WELL))
        .border(1.0, c(color::EDGE))
        .hover(c(color::WELL_HI))
        .press(c(color::WELL_HI))
        .press_scale(0.94)
        .child(text(label.to_owned()).font(font(15.0, 560.0)).color(c(color::TEXT)))
}
