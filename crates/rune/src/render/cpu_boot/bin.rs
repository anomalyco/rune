//! Binning: which instances touch which tile, in painter order, from the quad the vertex shader
//! would rasterise for each, cut to its clip's reach. An opaque box that covers a whole tile
//! starts that tile over: nothing before it there can show.

use std::collections::HashMap;

use super::Scene;
use super::shade::{Clip, has_sides, sides_inner};
use crate::render::{Instance, KIND_BACKDROP_BLUR, KIND_DASHED, KIND_EFFECT, KIND_GLYPH, KIND_GRADIENT, KIND_LAYER, KIND_LAYER_END, KIND_RADIAL, KIND_RECT, KIND_SHADOW, NO_CLIP, corner_extent};

pub(super) const TILE_W: u32 = 64;
/// `Bins::within` for an instance that draws straight into the window.
pub(super) const TOP: u32 = u32::MAX;
pub(super) const TILE_H: u32 = 32;

/// Instances by tile: tile `t`'s are `index[start[t]..start[t + 1]]`.
pub(super) struct Bins {
    pub cols: u32,
    pub rows: u32,
    start: Vec<u32>,
    index: Vec<u32>,
    /// Each instance's pixel bounds `[x0, y0, x1, y1)`.
    pub bounds: Vec<[i32; 4]>,
    /// Per tile, whether it begins under an opaque instance (no clear needed).
    pub covered: Vec<bool>,
    /// The layer each instance draws into: the plan of the innermost layer with a target of its
    /// own around it, or `TOP` for the window's pixels.
    pub within: Vec<u32>,
    /// The frame's backdrop blurs (all outside layers, `Scene::needs_gpu`), in painter order:
    /// each must see everything before it painted in its region first.
    pub blurs: Vec<(u32, super::backdrop::Record)>,
    /// Tiles a blur reads: no later box stands in for what's under it there.
    pub quiet: Vec<bool>,
    pub culled: usize,
    /// Each layer scope as the renderer plans it (`layers.rs`), and which plan each scope's
    /// `KIND_LAYER` and `KIND_LAYER_END` belong to.
    pub layers: Vec<Layer>,
    pub scope: HashMap<u32, u32>,
}

/// How a layer scope draws, as `Renderer::prepare_layers` decides.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Show {
    /// Fully transparent, empty, or inside one that is: none of it draws.
    Hidden,
    /// Opaque and unclipped: its contents draw straight into the parent.
    Flat,
    /// Into its own `Rgba16Float` target, composited at its end.
    Render,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Layer {
    pub begin: u32,
    pub end: u32,
    pub show: Show,
    /// The target's pixels, `[x0, y0, x1, y1)`: the composite draws nothing outside them.
    pub bounds: [i32; 4],
    pub additive: bool,
    /// The round hole its composite cuts (`El::cutout_circle`).
    pub hole: [f32; 4],
}

/// `Renderer::prepare_layers`' planning walk, for the layers this painter draws (no blur, no
/// turn): each scope's bounds from what it holds, cut to its clip and its ancestors', and whether
/// it draws at all, straight into its parent or into a target of its own.
fn plan_layers(instances: &[Instance], viewport: (u32, u32)) -> (Vec<Layer>, HashMap<u32, u32>) {
    struct Open { at: usize, edges: [f32; 4], clip: [f32; 4], limit: [f32; 4], cache: bool }
    let mut plans: Vec<Layer> = Vec::new();
    let mut scope = HashMap::new();
    let mut open: Vec<Open> = Vec::new();
    let union = |e: &mut [f32; 4], r: [f32; 4]| *e = [e[0].min(r[0]), e[1].min(r[1]), e[2].max(r[2]), e[3].max(r[3])];
    for (index, i) in instances.iter().enumerate() {
        if i.params[1] == KIND_LAYER {
            let parent = open.last().map(|o| o.at);
            let limit = open.last().map_or(NO_CLIP, |p| [p.clip[0].max(p.limit[0]), p.clip[1].max(p.limit[1]), p.clip[2].min(p.limit[2]), p.clip[3].min(p.limit[3])]);
            let opacity = i.fill[3].clamp(0.0, 1.0);
            let hidden = opacity == 0.0 || parent.is_some_and(|p| plans[p].show == Show::Hidden);
            let cache = i.params[3] == 1.0;
            let additive = i.params[0] == 1.0;
            let additive_parent = parent.is_some_and(|p| plans[p].additive);
            let hole = if cache { [0.0; 4] } else { i.radii };
            let flat = !cache && opacity == 1.0 && !additive && !additive_parent && i.fade[3] == 0 && i.clip == NO_CLIP && hole[3] == 0.0;
            let show = if hidden { Show::Hidden } else if flat { Show::Flat } else { Show::Render };
            scope.insert(index as u32, plans.len() as u32);
            open.push(Open { at: plans.len(), edges: [i.rect[0], i.rect[1], i.rect[0] + i.rect[2], i.rect[1] + i.rect[3]], clip: i.clip, limit, cache });
            plans.push(Layer { begin: index as u32, end: 0, show, bounds: [0; 4], additive, hole });
        } else if i.params[1] == KIND_LAYER_END {
            let Some(o) = open.pop() else { continue };
            scope.insert(index as u32, o.at as u32);
            let e = o.edges;
            let vis = [(e[0]).max(o.clip[0].max(o.limit[0])).max(0.0).floor(), e[1].max(o.clip[1].max(o.limit[1])).max(0.0).floor(),
                e[2].min(o.clip[2].min(o.limit[2])).min(viewport.0 as f32).ceil(), e[3].min(o.clip[3].min(o.limit[3])).min(viewport.1 as f32).ceil()];
            let edges = if o.cache { [e[0].floor(), e[1].floor(), e[2].ceil(), e[3].ceil()] } else { vis };
            let l = &mut plans[o.at];
            l.end = index as u32;
            // A cached scope's target holds all of it; the window shows the part inside it.
            let shown = [edges[0].max(0.0), edges[1].max(0.0), edges[2].min(viewport.0 as f32), edges[3].min(viewport.1 as f32)];
            l.bounds = [shown[0] as i32, shown[1] as i32, shown[2] as i32, shown[3] as i32];
            if edges[2] <= edges[0] || edges[3] <= edges[1] || (o.cache && (vis[2] <= vis[0] || vis[3] <= vis[1])) { l.show = Show::Hidden; }
            if let Some(p) = open.last_mut().filter(|p| !p.cache) { union(&mut p.edges, edges); }
        } else if let Some(o) = open.last_mut() {
            if o.cache { continue; }
            let mut rect = i.rect;
            if i.params[1] == KIND_GLYPH && i.params[0] != 0.0 {
                let (sin, cos) = i.params[0].sin_cos();
                let (w, h) = (cos.abs() * rect[2] + sin.abs() * rect[3], sin.abs() * rect[2] + cos.abs() * rect[3]);
                rect = [rect[0] + (rect[2] - w) * 0.5, rect[1] + (rect[3] - h) * 0.5, w, h];
            }
            let k = i.params[1];
            let grow = if k == KIND_GLYPH { 0.0 } else if k == KIND_SHADOW { i.params[2] * 3.0 + 1.0 } else if k == KIND_EFFECT { i.params[2] + 1.0 }
                else if k == KIND_RECT || k == KIND_GRADIENT || k == KIND_DASHED || k == KIND_RADIAL { 1.0 + i.params[2] * 2.0 } else { 1.0 };
            union(&mut o.edges, [(rect[0] - grow).max(i.clip[0]), (rect[1] - grow).max(i.clip[1]), (rect[0] + rect[2] + grow).min(i.clip[2]), (rect[1] + rect[3] + grow).min(i.clip[3])]);
        }
    }
    // A layer that was never closed draws nothing.
    for o in open { plans[o.at].show = Show::Hidden; }
    (plans, scope)
}

/// The tiles holding any of `reg`'s pixels (`[x0, y0, x1, y1)`, device px).
pub(super) fn region_tiles(reg: [f32; 4], cols: u32, rows: u32) -> impl Iterator<Item = usize> {
    let (tx0, tx1) = (reg[0] as u32 / TILE_W, (reg[2].ceil() as u32).div_ceil(TILE_W).min(cols));
    ((reg[1] as u32 / TILE_H)..(reg[3].ceil() as u32).div_ceil(TILE_H).min(rows)).flat_map(move |ty| (tx0..tx1).map(move |tx| (ty * cols + tx) as usize))
}

fn cut(a: [i32; 4], b: [i32; 4]) -> [i32; 4] {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    if r[2] > r[0] && r[3] > r[1] { r } else { [0; 4] }
}

impl Bins {
    pub(super) fn of(scene: &Scene<'_>) -> Bins {
        let (w, h) = scene.size;
        let (cols, rows) = (w.div_ceil(TILE_W), h.div_ceil(TILE_H));
        let tiles = (cols * rows) as usize;
        let mut bounds: Vec<[i32; 4]> = scene.instances.iter().map(|i| pixel_bounds(i, w as i32, h as i32)).collect();
        let (layers, scope) = plan_layers(scene.instances, (w, h));
        // Within a layer drawn to its own target, nothing reaches past the layer's bounds, and
        // nothing there can stand in for what's under the layer.
        let mut within = vec![TOP; scene.instances.len()];
        let mut stack: Vec<([i32; 4], u32)> = Vec::new();
        let mut n = 0;
        while n < scene.instances.len() {
            let inside = stack.last().map(|s| s.0);
            within[n] = stack.last().map_or(TOP, |s| s.1);
            if let Some(&p) = scope.get(&(n as u32)) {
                let l = &layers[p as usize];
                if scene.instances[n].params[1] == KIND_LAYER {
                    match l.show {
                        Show::Hidden => {
                            for b in &mut bounds[n..=l.end as usize] { *b = [0; 4]; }
                            n = l.end as usize + 1;
                            continue;
                        }
                        Show::Flat => bounds[n] = [0; 4],
                        Show::Render => {
                            bounds[n] = inside.map_or(l.bounds, |o| cut(l.bounds, o));
                            stack.push((bounds[n], p));
                        }
                    }
                } else if l.show == Show::Render {
                    bounds[n] = stack.pop().map_or([0; 4], |s| s.0);
                    within[n] = stack.last().map_or(TOP, |s| s.1);
                } else {
                    bounds[n] = [0; 4];
                }
            } else if let Some(o) = inside {
                bounds[n] = cut(bounds[n], o);
            }
            n += 1;
        }
        let mut blurs = Vec::new();
        let mut quiet = vec![false; tiles];
        for (n, i) in scene.instances.iter().enumerate().filter(|(_, i)| i.params[1] == KIND_BACKDROP_BLUR) {
            bounds[n] = super::backdrop::bounds(i, (w, h));
            let Some(rec) = super::backdrop::plan(i, (w, h)) else { continue };
            for t in region_tiles(rec.region, cols, rows) { quiet[t] = true; }
            blurs.push((n as u32, rec));
        }
        // The last opaque instance covering each tile whole, plus one (0: none).
        let mut first = vec![0u32; tiles];
        for (n, i) in scene.instances.iter().enumerate() {
            let b = bounds[n];
            if b[2] <= b[0] || b[3] <= b[1] || within[n] != TOP { continue; }
            if let Some(solid) = opaque_box(i) {
                let clip = Clip::of(i);
                let (tx0, ty0) = ((b[0] as u32).div_ceil(TILE_W), (b[1] as u32).div_ceil(TILE_H));
                let (tx1, ty1) = ((b[2] as u32 / TILE_W).min(cols), (b[3] as u32 / TILE_H).min(rows));
                for ty in ty0..ty1 {
                    let (y0, y1) = ((ty * TILE_H) as f32 + 0.5, ((ty + 1) * TILE_H).min(h) as f32 - 0.5);
                    let span = |y: f32| intersect(solid.span(y), clip_span(&clip, y));
                    let (top, bottom) = (span(y0), span(y1));
                    for tx in tx0..tx1 {
                        let (x0, x1) = ((tx * TILE_W) as f32 + 0.5, ((tx + 1) * TILE_W).min(w) as f32 - 0.5);
                        if top.0 <= x0 && top.1 >= x1 && bottom.0 <= x0 && bottom.1 >= x1 && !quiet[(ty * cols + tx) as usize] {
                            first[(ty * cols + tx) as usize] = n as u32 + 1;
                        }
                    }
                }
            }
        }
        let mut count = vec![0u32; tiles + 1];
        let mut culled = 0;
        let each = |n: usize, f: &mut dyn FnMut(usize)| {
            let b = bounds[n];
            if b[2] <= b[0] || b[3] <= b[1] { return; }
            for ty in b[1] as u32 / TILE_H..(b[3] as u32).div_ceil(TILE_H) {
                for tx in b[0] as u32 / TILE_W..(b[2] as u32).div_ceil(TILE_W) {
                    f((ty * cols + tx) as usize);
                }
            }
        };
        for n in 0..scene.instances.len() {
            each(n, &mut |t| if n as u32 + 1 >= first[t] { count[t + 1] += 1 } else { culled += 1 });
        }
        for t in 0..tiles { count[t + 1] += count[t]; }
        let mut index = vec![0u32; count[tiles] as usize];
        let mut at = count.clone();
        for n in 0..scene.instances.len() {
            each(n, &mut |t| if n as u32 + 1 >= first[t] {
                index[at[t] as usize] = n as u32;
                at[t] += 1;
            });
        }
        Bins { cols, rows, start: count, index, bounds, covered: first.iter().map(|&f| f > 0).collect(), culled, layers, scope, within, blurs, quiet }
    }

    pub(super) fn tile(&self, t: usize) -> &[u32] {
        &self.index[self.start[t] as usize..self.start[t + 1] as usize]
    }

    pub(super) fn tile_count(&self) -> usize { (self.cols * self.rows) as usize }

    pub(super) fn len(&self) -> usize { self.index.len() }
}

/// The pixels whose centres lie in the quad the vertex shader draws for `i` (`base_vs`), within
/// the reach of its clip and the target. Empty for what rasterises nothing.
pub(super) fn pixel_bounds(i: &Instance, w: i32, h: i32) -> [i32; 4] {
    let kind = i.params[1];
    if kind > 11.5 || kind == crate::render::KIND_LAYER_END { return [0; 4]; }
    let grow = match kind as u32 {
        1 => i.params[2] * 3.0 + 1.0,
        2 => 0.0,
        3 | 5..=9 => 1.0,
        _ => 1.0 + i.params[2] * 2.0,
    };
    let mut rect = i.rect;
    // A turned icon's quad turns about its centre: its bounds are the turned quad's.
    if kind == KIND_GLYPH && i.params[0] != 0.0 {
        let (sin, cos) = i.params[0].sin_cos();
        let (w, h) = (cos.abs() * rect[2] + sin.abs() * rect[3], sin.abs() * rect[2] + cos.abs() * rect[3]);
        rect = [rect[0] + (rect[2] - w) * 0.5 - 1.0, rect[1] + (rect[3] - h) * 0.5 - 1.0, w + 2.0, h + 2.0];
    }
    let mut lo = [rect[0] - grow, rect[1] - grow];
    let mut hi = [lo[0] + rect[2] + 2.0 * grow, lo[1] + rect[3] + 2.0 * grow];
    // A band of a big shadow or border ring draws only its window (runtime/bands.rs).
    if kind < 1.5 && i.uv[2] < 0.0 {
        lo = [lo[0].max(i.rect[0] + i.uv[0]), lo[1].max(i.rect[1] + i.uv[1])];
        hi = [hi[0].min(i.rect[0] - i.uv[2]).max(lo[0]), hi[1].min(i.rect[1] + i.uv[3]).max(lo[1])];
    }
    // Centres in [lo, hi): the rasteriser's top-left rule.
    let first = |v: f32| (v - 0.5).ceil();
    let c = i.clip;
    let x0 = first(lo[0]).max((c[0] - 0.5).floor()).max(0.0);
    let y0 = first(lo[1]).max((c[1] - 0.5).floor()).max(0.0);
    let x1 = first(hi[0]).min((c[2] + 0.5).ceil()).min(w as f32);
    let y1 = first(hi[1]).min((c[3] + 0.5).ceil()).min(h as f32);
    if !(x1 > x0 && y1 > y0) { return [0; 4]; }
    [x0 as i32, y0 as i32, x1 as i32, y1 as i32]
}

/// A rounded box and how far inside its edge a pixel centre must be to be whole: the region of
/// pixels a box kind paints in one colour.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Solid {
    pub edges: [f32; 4],
    pub radii: [f32; 4],
    pub inset: f32,
}

impl Solid {
    /// How far in from its top and bottom edges the span stops narrowing for the corners.
    pub(super) fn bands(&self) -> (f32, f32) {
        let [x0, y0, x1, y1] = self.edges;
        let m = ((x1 - x0) * 0.5).min((y1 - y0) * 0.5);
        let reach = |r: f32| if r < 0.0 { corner_extent(-(-r).min(m)) } else { r.min(m) };
        let r = self.radii.map(reach);
        (r[0].max(r[1]).max(self.inset), r[3].max(r[2]).max(self.inset))
    }

    /// The pixel centres `[a, b]` on the row with centre `y` that lie at least `inset` inside the
    /// box (its signed distance at most `-inset`), conservatively near rounded corners.
    pub(super) fn span(&self, y: f32) -> (f32, f32) {
        let [x0, y0, x1, y1] = self.edges;
        let (top, bottom) = (y - y0, y1 - y);
        let t = self.inset;
        if top < t || bottom < t { return EMPTY; }
        let m = ((x1 - x0) * 0.5).min((y1 - y0) * 0.5);
        let reach = |r: f32| if r < 0.0 { corner_extent(-(-r).min(m)) } else { r.min(m) };
        let upper = y < (y0 + y1) * 0.5;
        let (rl, rr) = if upper { (reach(self.radii[0]), reach(self.radii[1])) } else { (reach(self.radii[3]), reach(self.radii[2])) };
        let near = top.min(bottom);
        let a = x0 + if near < rl { rl.max(t) } else { t };
        let b = x1 - if near < rr { rr.max(t) } else { t };
        (a, b)
    }
}

pub(super) const EMPTY: (f32, f32) = (f32::INFINITY, f32::NEG_INFINITY);

pub(super) fn intersect(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (a.0.max(b.0), a.1.min(b.1))
}

/// Where on row `y` the clip is whole (`clip_alpha` is 1).
pub(super) fn clip_span(clip: &Clip, y: f32) -> (f32, f32) {
    let c = clip.rect;
    let inset = clip.inset();
    if y < c[1] + inset[1] || y > c[3] - inset[3] { return EMPTY; }
    let edges = (c[0] + inset[0], c[2] - inset[2]);
    match clip.radii {
        None => edges,
        Some(radii) => intersect(edges, Solid { edges: c, radii, inset: 0.5 }.span(y)),
    }
}

/// The region a box kind paints in one colour, if it's opaque there: the interior of a
/// `KIND_RECT` (inside its border, when it has one) whose colour there has alpha 1.
pub(super) fn opaque_box(i: &Instance) -> Option<Solid> {
    if i.params[1] != crate::render::KIND_RECT || i.fill[3] < 1.0 { return None; }
    interior(i)
}

/// Where a `KIND_RECT` or `KIND_GRADIENT` paints its fill whole, over nothing of its border.
pub(super) fn interior(i: &Instance) -> Option<Solid> {
    let edges = [i.rect[0], i.rect[1], i.rect[0] + i.rect[2], i.rect[1] + i.rect[3]];
    let soft = i.params[2].max(0.5);
    let bw = i.params[0];
    if bw <= 0.0 { return Some(Solid { edges, radii: i.radii, inset: soft }); }
    if i.params[1] == crate::render::KIND_RECT && has_sides(i) {
        // The per-side inner box is inside the outer one, which is whole wherever it is.
        let half = [i.rect[2] * 0.5, i.rect[3] * 0.5];
        let (c, ih, radii) = sides_inner(i, [i.rect[0] + half[0], i.rect[1] + half[1]], half);
        return Some(Solid { edges: [c[0] - ih[0], c[1] - ih[1], c[0] + ih[0], c[1] + ih[1]], radii, inset: soft });
    }
    Some(Solid { edges, radii: i.radii, inset: soft + bw })
}

/// Whether every pixel centre of `r` is inside `solid` (if given) where `clip` is whole: true at
/// its first and last rows means true between (both regions are convex).
pub(super) fn covers(solid: Option<&Solid>, clip: &Clip, r: [f32; 4]) -> bool {
    let (x0, x1) = (r[0] + 0.5, r[2] - 0.5);
    [r[1] + 0.5, r[3] - 0.5].iter().all(|&y| {
        let span = solid.map_or(clip_span(clip, y), |s| intersect(s.span(y), clip_span(clip, y)));
        span.0 <= x0 && span.1 >= x1
    })
}
