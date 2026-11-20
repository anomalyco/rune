//! The AAT `trak` table: tracking that depends on the point size. Core Text applies it at track 0
//! to fonts that also have a `STAT` table (the macOS system font among them), and so does
//! HarfRust, but Parley hands HarfRust the font size in device pixels as the point size, so on a
//! 2x display 14 pt text was tracked as 28 pt text. [`Trak::spacing`] corrects for that.

use std::collections::HashMap;
use std::rc::Rc;

use parley::fontique::Blob;
use swash::FontRef;

/// A font's horizontal track-0 tracking, in font units, at each point size its table lists.
pub(crate) struct Trak {
    sizes: Vec<(f32, f32)>,
    units_per_em: f32,
}

impl Trak {
    /// The table of a font HarfRust tracks: one with both `trak` and `STAT`.
    pub(crate) fn of(font: FontRef<'_>) -> Option<Trak> {
        font.table(swash::tag_from_bytes(b"STAT"))?;
        Trak::parse(font.table(swash::tag_from_bytes(b"trak"))?, font.metrics(&[]).units_per_em)
    }

    /// Track 0 of a `trak` table, or the interpolation between the tracks around it, read the way
    /// HarfRust reads it: a table it can't read tracks nothing, and a track whose values it can't
    /// read tracks by 0. Fonts can be untrusted, so every offset is checked.
    fn parse(data: &[u8], units_per_em: u16) -> Option<Trak> {
        if units_per_em == 0 {
            return None;
        }
        let bytes = |at: usize, len: usize| data.get(at..at.checked_add(len)?);
        let u16_at = |at: usize| Some(u16::from_be_bytes(bytes(at, 2)?.try_into().ok()?));
        let fixed = |b: &[u8]| i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32 / 65536.0;
        let horizontal = u16_at(6)? as usize;
        if horizontal == 0 {
            return None;
        }
        let (tracks, sizes) = (u16_at(horizontal)? as usize, u16_at(horizontal.checked_add(2)?)? as usize);
        let size_table = u32::from_be_bytes(bytes(horizontal.checked_add(4)?, 4)?.try_into().ok()?);
        let entries = bytes(horizontal.checked_add(8)?, tracks.checked_mul(8)?)?;
        let size_values = bytes(usize::try_from(size_table).ok()?, sizes.checked_mul(4)?)?;
        if tracks == 0 || sizes == 0 {
            return None;
        }
        let entry = |i: usize| &entries[i * 8..i * 8 + 8];
        let track = |i: usize| fixed(entry(i));
        let values = |i: usize| -> Vec<f32> {
            let at = u16::from_be_bytes([entry(i)[6], entry(i)[7]]) as usize;
            match bytes(at, sizes * 2) {
                Some(b) => b.chunks_exact(2).map(|v| i16::from_be_bytes([v[0], v[1]]) as f32).collect(),
                None => vec![0.0; sizes],
            }
        };
        let mut i = 0;
        while i + 1 < tracks && track(i + 1) <= 0.0 {
            i += 1;
        }
        let mut j = tracks - 1;
        while j > 0 && track(j - 1) >= 0.0 {
            j -= 1;
        }
        let (a, b) = (values(i), if i == j { Vec::new() } else { values(j) });
        let (t0, t1) = (track(i), track(j));
        let t = if i == j || (t1 - t0).abs() < f32::EPSILON { 0.0 } else { -t0 / (t1 - t0) };
        let sizes = size_values
            .chunks_exact(4)
            .zip(&a)
            .enumerate()
            .map(|(s, (size, &a))| (fixed(size), b.get(s).map_or(a, |&b| a + t * (b - a))))
            .collect();
        Some(Trak { sizes, units_per_em: units_per_em as f32 })
    }

    /// Tracking at `points`, in font units: interpolated between the listed sizes and held
    /// beyond them.
    fn units(&self, points: f32) -> f32 {
        let Some(i) = self.sizes.iter().position(|&(size, _)| size >= points) else {
            return self.sizes.last().map_or(0.0, |s| s.1);
        };
        if i == 0 {
            return self.sizes[0].1;
        }
        let ((s0, v0), (s1, v1)) = (self.sizes[i - 1], self.sizes[i]);
        if (s1 - s0).abs() < f32::EPSILON {
            return (v0 + v1) / 2.0;
        }
        v0 + (points - s0) / (s1 - s0) * (v1 - v0)
    }

    /// Letter spacing, in device px, that turns the tracking HarfRust applied to a run at `px`
    /// device pixels into Core Text's tracking for `points`-point text (its point size, or the
    /// size an explicit optical size stands for) scaled to the run, or removes it when `on` is
    /// false. HarfRust rounds tracking to whole font units; Core Text truncates.
    pub(crate) fn spacing(&self, px: f32, points: f32, on: bool) -> f32 {
        let applied = self.units(px).round();
        let wanted = if on { self.units(points).trunc() } else { 0.0 };
        (wanted - applied) * px / self.units_per_em
    }
}

/// Fonts' `trak` tables (None for an untracked font), by blob and index.
pub(crate) type Traks = HashMap<(u64, u32), Option<Rc<Trak>>>;

pub(crate) fn font_trak(traks: &mut Traks, data: &Blob<u8>, index: u32) -> Option<Rc<Trak>> {
    traks
        .entry((data.id(), index))
        .or_insert_with(|| FontRef::from_index(data.as_ref(), index as usize).and_then(Trak::of).map(Rc::new))
        .clone()
}

#[cfg(test)]
#[path = "trak_tests.rs"]
mod tests;
