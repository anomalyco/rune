//! `RUNE_RASTER_CHECK=1` (debug builds): on every frame that comes to rest, checks that each
//! glyph and icon draws as one painted for the first time would, and that none draws its atlas
//! mask magnified or shrunk. A mask made for another scale (a cell rasterized before a zoom, a run
//! of text that never left its moving path, an icon rasterized at whole pixels and stretched to a
//! zoom's fractional size) shows soft or blocky beside its neighbours; this names it on stderr.

use super::*;

pub(super) fn on() -> bool {
    cfg!(debug_assertions) && std::env::var_os("RUNE_RASTER_CHECK").is_some_and(|v| v != "0")
}

impl<S: 'static> Runtime<S> {
    /// Paints the frame again with every text's raster path and cached glyph runs forgotten,
    /// and reports the glyphs that differ, then any resting glyph that magnifies its mask.
    pub(super) fn check_resting_raster(&mut self) {
        let settled = glyphs(&self.instances);
        self.text.forget_raster_state();
        self.build();
        let fresh = glyphs(&self.instances);
        let differ: Vec<_> = settled.iter().zip(&fresh).filter(|(a, b)| a != b).collect();
        if !differ.is_empty() || settled.len() != fresh.len() {
            eprintln!("rune: raster check at {:.3} s: {} of {} resting glyphs and icons draw unlike a fresh paint, first:", self.clock, differ.len(), settled.len());
            for (a, b) in differ.iter().take(3) {
                eprintln!("  resting {a:?}\n  fresh   {b:?}");
            }
        }
        let magnified: Vec<_> = self.instances.iter().filter(|i| magnifies(i)).map(|i| (i.rect, i.uv)).collect();
        if !magnified.is_empty() {
            eprintln!("rune: raster check at {:.3} s: {} resting glyphs or icons draw a mask stretched, first (rect, atlas uv) {:?}", self.clock, magnified.len(), &magnified[..magnified.len().min(3)]);
        }
    }
}

/// A glyph instance's rect, pen (radii), outline record (border), params and atlas footprint.
type Glyph = ([f32; 4], [f32; 4], [f32; 4], [f32; 4], [f32; 2]);

fn glyphs(instances: &[Instance]) -> Vec<Glyph> {
    instances.iter().filter(|i| i.params[1] == KIND_GLYPH).map(|i| (i.rect, i.radii, i.border, i.params, [i.uv[2], i.uv[3]])).collect()
}

/// An unturned glyph or icon drawn larger or smaller than its mask, by more than a 256th: at rest
/// both draw their masks 1:1, so even the 1.5% an icon at whole pixels is stretched to at 110%
/// zoom shows here.
fn magnifies(i: &Instance) -> bool {
    let texels = if i.radii[0] > 0.5 { 3.0 } else { 1.0 };
    i.params[1] == KIND_GLYPH && i.params[0] == 0.0 && i.uv[2] > 0.0 && (i.rect[2] * texels / i.uv[2] - 1.0).abs() > 1.0 / 256.0
}
