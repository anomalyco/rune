//! Synthetic bold, as Chromium's Skia draws it for text asking for bold of a face that has none
//! (SimSun, MS Gothic): the glyph's outline filled and stroked at once, the stroke
//! `size × [1/24 at 9 px … 1/32 at 36 px]` wide (`SkScalerContext::MakeRecAndEffects`,
//! `kStdFakeBoldInterpKeys`), so it grows by half that on every side with mitred joins, and Blink
//! widens each advance by a pixel ([`ADVANCE`]). Which runs take it is
//! `system_weight::synthetic_bold`; their brush carries [`SYNTHETIC_BOLD`] to the rasteriser.

use std::ops::Range;

use swash::FontRef;
use swash::scale::ScaleContext;
use swash::zeno::{Command, Format, Join, Mask, Origin, PathData, Placement, Stroke};

use super::Span;

/// What Blink adds to each advance in synthetic bold, in points (`SimpleFontData`'s
/// `SyntheticBoldOffset`, a CSS pixel).
pub(super) const ADVANCE: f32 = 1.0;

/// The brush bit of a glyph run in synthetic bold; the rest is its `Ink`.
pub(crate) const SYNTHETIC_BOLD: u16 = 0x8000;

/// The brushes `ranges` take: each piece's own ink (the last span over it) with
/// [`SYNTHETIC_BOLD`], split where the spans over it change.
pub(super) fn inks(ranges: &[Range<usize>], spans: &[Span]) -> Vec<(Range<usize>, u16)> {
    let ink_at = |at: usize| spans.iter().rposition(|s| (s.start as usize) <= at && at < s.end as usize).map_or(0, |i| i as u16 + 1);
    let mut out = Vec::new();
    for r in ranges {
        let mut cuts: Vec<usize> = spans.iter().flat_map(|s| [s.start as usize, s.end as usize]).filter(|&c| r.start < c && c < r.end).collect();
        cuts.push(r.end);
        cuts.sort_unstable();
        cuts.dedup();
        let mut at = r.start;
        for c in cuts {
            out.push((at..c, ink_at(at) | SYNTHETIC_BOLD));
            at = c;
        }
    }
    out
}

/// The stroke Skia adds round a glyph `ppem` device px tall (`SkScalarInterpFunc` over
/// 9 → 1/24 and 36 → 1/32, held past either end).
pub(crate) fn stroke(ppem: f32) -> f32 {
    let t = ((ppem - 9.0) / (36.0 - 9.0)).clamp(0.0, 1.0);
    ppem * (1.0 / 24.0 + t * (1.0 / 32.0 - 1.0 / 24.0))
}

/// `glyph`'s coverage at `ppem` emboldened, its pen `phase_x` right and `shift_y` down of the
/// whole pixel, leaning `skew` degrees; swash's placement (`top` is rows above the baseline).
#[allow(clippy::too_many_arguments)]
pub(crate) fn mask(cx: &mut ScaleContext, font: FontRef<'_>, coords: &[i16], glyph: u16, ppem: f32, phase_x: f32, shift_y: f32, skew: u8) -> Option<(Vec<u8>, Placement)> {
    let mut scaler = cx.builder(font).size(ppem).hint(false).normalized_coords(coords.iter().copied()).build();
    let outline = scaler.scale_outline(glyph)?;
    let lean = if skew == 0 { 0.0 } else { (skew as f32).to_radians().tan() };
    let p = |v: swash::zeno::Point| swash::zeno::Point::new(v.x + v.y * lean + phase_x, v.y - shift_y);
    let path: Vec<Command> = outline.path().commands().map(|c| match c {
        Command::MoveTo(a) => Command::MoveTo(p(a)),
        Command::LineTo(a) => Command::LineTo(p(a)),
        Command::QuadTo(a, b) => Command::QuadTo(p(a), p(b)),
        Command::CurveTo(a, b, c) => Command::CurveTo(p(a), p(b), p(c)),
        Command::Close => Command::Close,
    }).collect();
    if path.is_empty() {
        return None;
    }
    let mut style = Stroke::new(stroke(ppem));
    style.join(Join::Miter).miter_limit(4.0);
    let (mut out, at) = Mask::new(&path[..]).style(style).format(Format::Alpha).origin(Origin::BottomLeft).inspect(|_, _, _| {}).render();
    // Stroke and fill: the fill's coverage joins the stroke's, inside its bounds.
    let (fill, f) = Mask::new(&path[..]).format(Format::Alpha).origin(Origin::BottomLeft).inspect(|_, _, _| {}).render();
    let (dx, dy) = (f.left - at.left, at.top - f.top);
    for row in 0..f.height as i32 {
        for col in 0..f.width as i32 {
            let (x, y) = (col + dx, row + dy);
            if x < 0 || y < 0 || x >= at.width as i32 || y >= at.height as i32 {
                continue;
            }
            let o = &mut out[(y * at.width as i32 + x) as usize];
            *o = (*o).max(fill[(row * f.width as i32 + col) as usize]);
        }
    }
    Some((out, at))
}
