use super::Painter;
use super::super::bin::{EMPTY, Solid, TOP, clip_span, intersect, opaque_box};
use super::super::shade::Clip;

#[derive(Clone, Copy)]
pub(super) struct Occluder {
    n: u32,
    /// The layer it draws into (`Bins::within`): it stands in only for what's under it there.
    within: u32,
    solid: Solid,
    clip: Clip,
    r: [u32; 4],
}

/// The parts of a row left to draw: at most `RANGES` runs of pixels `[a, b)`.
#[derive(Clone, Copy)]
pub(super) struct Ranges {
    runs: [(u32, u32); RANGES],
    len: usize,
}

const RANGES: usize = 8;

impl Ranges {
    pub(super) fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ { self.runs[..self.len].iter().copied() }

    /// Takes pixels `[a, b)` out. Past `RANGES` runs it leaves them in: drawing under the box
    /// too is only wasted work.
    fn cut(&mut self, a: u32, b: u32) {
        let mut next = Ranges { runs: [(0, 0); RANGES], len: 0 };
        for (x0, x1) in self.iter() {
            for piece in [(x0, x1.min(a)), (x0.max(b), x1)] {
                if piece.1 <= piece.0 { continue; }
                if next.len == RANGES { return; }
                next.runs[next.len] = piece;
                next.len += 1;
            }
        }
        *self = next;
    }
}

/// A run of rows ending before `y1` of one instance in one tile, and what holds on each of them: the
/// pixels its interior paints in one colour (`[a, b)`), where its clip is whole, and the runs no
/// later opaque box covers.
#[derive(Clone, Copy)]
pub(super) struct Segment {
    pub(super) y1: u32,
    pub(super) solid: (u32, u32),
    pub(super) whole: (f32, f32),
    pub(super) vis: Ranges,
}

/// The first row (by index) whose centre is at least `v`, and the first past those at most `v`.
fn row_from(v: f32) -> u32 { (v - 0.5).ceil().max(0.0) as u32 }
fn row_past(v: f32) -> u32 { ((v - 0.5).floor() + 1.0).max(0.0) as u32 }

/// Where a `Solid`'s span changes: its top and bottom `inset`, and its corner bands.
fn solid_breaks(s: &Solid, out: &mut Vec<u32>) {
    let [_, y0, _, y1] = s.edges;
    let (top, bottom) = s.bands();
    out.extend([row_from(y0 + s.inset), row_from(y0 + top), row_past(y1 - bottom), row_past(y1 - s.inset)]);
}

/// Where a clip's whole span changes.
fn clip_breaks(c: &Clip, out: &mut Vec<u32>) {
    let inset = c.inset();
    out.extend([row_from(c.rect[1] + inset[1]), row_past(c.rect[3] - inset[3])]);
    if let Some(radii) = c.radii {
        let (top, bottom) = Solid { edges: c.rect, radii, inset: 0.5 }.bands();
        out.extend([row_from(c.rect[1] + top), row_past(c.rect[3] - bottom)]);
    }
}

/// Pixel indices `[ia, ib)` whose centres lie in `[a, b]`, within `x0..x1`.
pub(super) fn centres(span: (f32, f32), x0: u32, x1: u32) -> (u32, u32) {
    if span.0 > span.1 { return (x1, x1); }
    let a = (span.0 - 0.5).ceil().clamp(x0 as f32, x1 as f32) as u32;
    let b = ((span.1 - 0.5).floor() + 1.0).clamp(a as f32, x1 as f32) as u32;
    (a, b)
}

impl Painter<'_, '_> {
    /// The occluders among `list` (a tile's instances in one phase), in painter order. A box
    /// past a backdrop blur's record never stands in for what that blur reads: phases end at
    /// each record, and occluders come from this phase only.
    pub(super) fn occluders(&mut self, list: &[u32]) {
        self.occ.clear();
        for &n in list {
            let within = self.bins.within[n as usize];
            // Inside a PlusLighter layer every primitive adds to what came before it; an opaque
            // box never hides an earlier sibling.
            if within != TOP && self.bins.layers[within as usize].additive { continue; }
            let i = &self.scene.instances[n as usize];
            let Some(solid) = opaque_box(i) else { continue };
            let (b, tile) = (self.bins.bounds[n as usize], self.tile);
            let r = [(b[0] as u32).max(tile[0]), (b[1] as u32).max(tile[1]), (b[2] as u32).min(tile[2]), (b[3] as u32).min(tile[3])];
            if r[2] > r[0] && r[3] > r[1] { self.occ.push(Occluder { n, within, solid, clip: Clip::of_stops(i, self.scene.fade_stops), r }); }
        }
    }

    /// Splits rows `r[1]..r[3]` of `r` where anything this instance's drawing depends on changes
    /// (its interior's corner bands, its clip's, a later opaque box's), and for each run of rows
    /// takes what holds on its first and last row: those regions are convex, so it holds on every
    /// row between. A pixel outside these (smaller than exact) regions is shaded one by one,
    /// which gives it the same bytes.
    pub(super) fn segments(&mut self, r: [u32; 4], solid: Option<Solid>, clip: &Clip) -> Vec<Segment> {
        let mut breaks = std::mem::take(&mut self.breaks);
        breaks.clear();
        breaks.extend([r[1], r[3]]);
        if let Some(s) = &solid { solid_breaks(s, &mut breaks); }
        clip_breaks(clip, &mut breaks);
        // Occluders are in painter order: only those after this instance count.
        let later = self.occ.partition_point(|o| o.n <= self.n);
        let within = self.bins.within[self.n as usize];
        let occ: Vec<&Occluder> = self.occ[later..].iter().filter(|o| o.within == within && o.r[1] < r[3] && o.r[3] > r[1] && o.r[0] < r[2] && o.r[2] > r[0]).collect();
        for o in &occ {
            breaks.extend([o.r[1], o.r[3]]);
            solid_breaks(&o.solid, &mut breaks);
            clip_breaks(&o.clip, &mut breaks);
        }
        breaks.retain(|&b| b >= r[1] && b <= r[3]);
        breaks.sort_unstable();
        breaks.dedup();
        let mut out = std::mem::take(&mut self.segs);
        out.clear();
        for w in breaks.windows(2) {
            let (ya, yb) = (w[0], w[1]);
            let (pa, pb) = (ya as f32 + 0.5, (yb - 1) as f32 + 0.5);
            let whole = intersect(clip_span(clip, pa), clip_span(clip, pb));
            let inside = solid.map_or(EMPTY, |s| intersect(intersect(s.span(pa), s.span(pb)), whole));
            let mut vis = Ranges { runs: [(0, 0); RANGES], len: 1 };
            vis.runs[0] = (r[0], r[2]);
            for o in occ.iter().filter(|o| o.r[1] <= ya && o.r[3] >= yb) {
                let covered = intersect(intersect(o.solid.span(pa), o.solid.span(pb)), intersect(clip_span(&o.clip, pa), clip_span(&o.clip, pb)));
                let (ca, cb) = centres(covered, o.r[0], o.r[2]);
                if cb > ca { vis.cut(ca, cb); }
                if vis.len == 0 { break; }
            }
            out.push(Segment { y1: yb, solid: centres(inside, r[0], r[2]), whole, vis });
        }
        self.breaks = breaks;
        out
    }
}
