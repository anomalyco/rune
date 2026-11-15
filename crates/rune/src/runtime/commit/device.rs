//! CSS boxes whose authored dimensions resolve to whole device pixels at layout time.
use super::*;

pub(super) fn border(style: &mut CompactStyle, vis: &Visual, scale: f32) {
    let sides = vis.border_sides.unwrap_or([vis.border_width(); 4]);
    style.set_border(sides.map(|w| crate::runtime::pixel_snap::line_width(w * scale) / scale));
}

pub(super) fn disc(style: &mut CompactStyle, [ascent, descent, line]: [f32; 3], scale: f32) {
    let a = (ascent * scale).round();
    let two_thirds = (a * 2.0 / 3.0).floor();
    let diameter = ((two_thirds + 1.0) * 0.5).floor();
    let leading = ((line * scale - a - (descent * scale).round()) * 0.5).floor();
    let top = leading + (1.5 * (a - two_thirds)).floor();
    // Blink's outside margin_start = -offset - 8; the drawing starts 1 px into that box.
    let end = two_thirds + 7.0 - diameter;
    style.set_width(length(diameter / scale));
    style.set_height(length(diameter / scale));
    style.set_absolute_inset(Rect { left: auto(), right: length(end / scale), top: length(top / scale), bottom: auto() });
}
