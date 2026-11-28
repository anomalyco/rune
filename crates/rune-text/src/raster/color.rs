//! Colour glyph images: COLR v0 layers and embedded bitmaps, through swash.

use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, Scaler, Source, StrikeWith};

/// The colour image of `glyph` at `size` px (COLR layers, or an embedded bitmap scaled to size,
/// mirrored when its `sbix` record is a `flip`), or None when the font draws it as an outline.
pub(crate) fn rasterize_color(scaler: &mut Scaler<'_>, font: FontRef<'_>, glyph: u16, size: f32) -> Option<(rune_image::pixels::Pixels, swash::zeno::Placement)> {
    if !scaler.has_color_bitmaps() && !scaler.has_color_outlines() {
        return None;
    }
    let render = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit)]);
    let flipped = if scaler.has_color_bitmaps() { super::sbix::render_flipped(&render, scaler, font, glyph, size) } else { None };
    let image = flipped.or_else(|| render.render(scaler, glyph))?;
    let (w, h) = (image.placement.width, image.placement.height);
    if image.content != Content::Color || w == 0 || h == 0 || image.data.len() != (w * h * 4) as usize {
        return None;
    }
    let pixels = rune_image::pixels::Pixels::new();
    pixels.set(w, h, image.data);
    Some((pixels, image.placement))
}
