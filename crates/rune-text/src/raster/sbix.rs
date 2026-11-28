//! Apple `sbix` records swash doesn't draw. swash decodes `png ` and follows one `dupe`; a
//! `flip` record is another glyph's picture mirrored left to right, which Apple Color Emoji uses
//! for its people facing right (🚶‍➡️ is 🚶 flipped), so without this they draw nothing.

use swash::FontRef;
use swash::scale::image::{Content, Image};
use swash::scale::{Render, Scaler};

/// Records a chain of `dupe` and `flip` references may pass through before its picture.
const MAX_CHAIN: usize = 4;

/// `glyph` drawn by `render` when its `sbix` record is a `flip`, or a `dupe` swash can't follow:
/// the picture the chain of references ends at, mirrored about `glyph`'s advance at `size` px
/// when an odd number of them were `flip`s. None when `glyph`'s record is a picture or a plain
/// `dupe` of one (swash draws those), or the chain ends at nothing.
pub(crate) fn render_flipped(render: &Render, scaler: &mut Scaler<'_>, font: FontRef<'_>, glyph: u16, size: f32) -> Option<Image> {
    let (source, flipped) = resolve(font, glyph)?;
    let mut image = render.render(scaler, source)?;
    if flipped {
        mirror(&mut image, font.glyph_metrics(&[]).scale(size).advance_width(glyph));
    }
    Some(image)
}

/// The glyph whose picture `glyph` draws through `font`'s `sbix`, and whether it's mirrored,
/// when that takes more than swash follows: any `flip`, or more than one `dupe`. Read from the
/// first strike with a record for `glyph` (every strike's references agree).
pub(crate) fn resolve(font: FontRef<'_>, glyph: u16) -> Option<(u16, bool)> {
    let sbix = font.table(swash::tag_from_bytes(b"sbix"))?;
    let glyphs = font.metrics(&[]).glyph_count;
    let u16_at = |at: usize| sbix.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let u32_at = |at: usize| sbix.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    // A record: origin x and y (i16 each), its type, then its data; empty when the strike has
    // no picture for the glyph. A strike: ppem, ppi, then numGlyphs + 1 record offsets.
    let record = |strike: usize, g: u16| -> Option<Option<(&[u8], usize)>> {
        let at = strike + 4 + 4 * g as usize;
        let (start, end) = (strike + u32_at(at)?, strike + u32_at(at + 4)?);
        Some((end > start + 8).then(|| (sbix.get(start + 4..start + 8).unwrap_or_default(), start + 8)))
    };
    let strikes = u32_at(4)?;
    let strike = (0..strikes).find_map(|s| {
        let strike = u32_at(8 + 4 * s)?;
        (glyph < glyphs && record(strike, glyph)?.is_some()).then_some(strike)
    })?;
    let (mut at, mut flips, mut dupes) = (glyph, 0, 0);
    for _ in 0..MAX_CHAIN {
        let (kind, data) = record(strike, at)??;
        match kind {
            b"flip" => flips += 1,
            b"dupe" => dupes += 1,
            _ => return (flips > 0 || dupes > 1).then_some((at, flips % 2 == 1)),
        }
        at = u16_at(data)?;
        if at >= glyphs { return None; }
    }
    None
}

/// Mirrors `image` left to right about a glyph advance of `advance` px.
fn mirror(image: &mut Image, advance: f32) {
    let stride = if image.content == Content::Color { 4 } else { 1 };
    let w = image.placement.width as usize;
    if w > 0 {
        for row in image.data.chunks_exact_mut(w * stride) {
            for x in 0..w / 2 {
                let (a, b) = (x * stride, (w - 1 - x) * stride);
                for c in 0..stride { row.swap(a + c, b + c); }
            }
        }
    }
    image.placement.left = advance.round() as i32 - image.placement.left - image.placement.width as i32;
}

#[cfg(test)]
#[path = "sbix_tests.rs"]
mod tests;
