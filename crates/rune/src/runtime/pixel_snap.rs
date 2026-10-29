//! Paint-time pixel snapping (`El::pixel_snap`). Layout stays fractional; only where a box's
//! fill, border and clips land on the device grid changes, so a 1.1 pt gap at 2× still draws
//! hard edges.
//!
//! Edges snap, not origin plus size: each of a box's four edges rounds to the nearest device
//! pixel, so two boxes that share an edge round it the same way and meet with no seam or
//! overlap. A box at least half a pixel across keeps at least one pixel; a thinner one keeps
//! its exact edges on that axis rather than vanish.
//!
//! Motion: a moving box's edges are corrected toward the rounding of where it comes to rest
//! (every track at its target), not of where it is, so it glides at full sub-pixel precision
//! with no stepping. When the rest changes, each edge eases from the correction it drew with
//! at that moment to the new rest's, in proportion to how far it has left to travel, so a
//! motion starts where the box was and its last frame is the snapped resting rect: no jump at
//! either end. Boxes that move together share their progress, so a child keeps its place
//! against its parent's edges as both glide. Translations that are not animation tracks
//! (scroll offsets, sticky shifts, rows growing above) move a whole subtree together: they snap
//! as one rounded translation, so the rows of a scrolling list stay crisp and keep their
//! spacing every frame. A scaled subtree doesn't snap while the scale moves its edges by a
//! pixel or more; over the last pixel of a scale settling at 1 the snap fades in, so it lands
//! without a jump either. Turned subtrees don't snap.
//!
//! Scaled layers: a node with its own scale (a zoomed canvas, a pressed button) heads a layer,
//! and its subtree snaps against the grid where the scale puts it once the layer holds still.
//! While the layer's drawn rect moves (a zoom, a pinch, a spring), its content draws at its
//! exact sub-pixel edges. Once it holds still the correction to the grid eases in over
//! [`SETTLE`], so a zoom ends with no pop; when it moves again the correction it drew at rest
//! fades out over the first pixel the layer's edges move.

use std::collections::HashMap;

use rune_motion::{NodeId, Presented, Rect};

/// How far each edge of a moving node is from where it comes to rest (device px), and that
/// resting rect (points), which keys its glide.
#[derive(Clone, Copy)]
pub(super) struct Motion {
    by: [f32; 4],
    rest: [f32; 4],
    /// The scale `by` is drawn at: 1 for the unscaled rect, a layer's scale for the drawn one.
    k: f32,
}

/// `p`'s motion toward `rest`, the rect the scene gives while the node is moving. An anchored
/// element is placed from its anchor, not its frame, so its frame's motion doesn't move it; a
/// growth row draws its bottom edge at its top plus its current height.
pub(super) fn motion(p: &Presented, rest: Option<Rect>, s: f32, anchored: bool, growth_row: bool) -> Option<Motion> {
    let rest = rest.filter(|_| !anchored)?;
    let (a, b) = (p.rect.edges(), rest.edges());
    let mut by: [f32; 4] = core::array::from_fn(|k| (a[k] - b[k]) * s);
    if growth_row {
        by[3] = by[1];
    }
    Some(Motion { by, rest: b, k: 1.0 })
}

/// One moving node's glide: its edges' corrections go from `from` to `to` (the resting rect's
/// rounding) as their motion goes from `span` to 0.
#[derive(Clone, Copy)]
struct Glide {
    rest: [f32; 4],
    k: f32,
    /// Resting edges in device px, less the rigid translation.
    rest_px: [f32; 4],
    from: [f32; 4],
    to: [f32; 4],
    span: [f32; 4],
    seen: u32,
}

impl Glide {
    /// Edge `k`'s correction with `m` px left to travel. Past the start (a retarget that carries
    /// it further away) it holds the starting correction.
    fn at(&self, k: usize, to: f32, m: f32) -> f32 {
        let t = if self.span[k].abs() > 1.0 / 64.0 { (m / self.span[k]).clamp(-1.0, 1.0) } else { 0.0 };
        to + (self.from[k] - to) * t
    }
}

/// What a node hands its subtree for snapping.
#[derive(Clone, Copy, Default)]
pub(super) struct Inherit {
    /// `El::pixel_snap(false)` above, or a turn: this subtree draws at its exact edges.
    pub off: bool,
    /// The translation in points from scrolls, sticky shifts and growth rows above, which
    /// snaps as a whole.
    pub rigid: [f32; 2],
    /// The nearest scaled layer above or here, as an index into this frame's layers plus one
    /// (0: none).
    pub layer: u32,
}

/// The glides of the nodes moving now and the scaled layers, owned by the runtime.
#[derive(Default)]
pub(super) struct Glides {
    glides: HashMap<NodeId, Glide>,
    layers: HashMap<NodeId, Layer>,
    /// This frame's layers, which `Inherit::layer` indexes from 1.
    now: Vec<LayerSnap>,
    frame: u32,
    t: f64,
    /// A layer's correction still has a visible way to go.
    easing: bool,
}

/// Frames a glide outlives its node's last paint (a cached subtree, a removed node).
const KEEP: u32 = 120;

/// Seconds a still scaled layer's correction to the grid takes to ease in.
pub(super) const SETTLE: f64 = 0.1;

/// A node with its own scale, which heads a scaled layer.
#[derive(Clone, Copy)]
struct Layer {
    /// Its drawn rect (device px, less its rigid translation) when it last moved or came to
    /// rest, and when.
    rect: [f32; 4],
    t: f64,
    seen: u32,
    moving: bool,
    /// When it came to rest.
    since: f64,
    /// The rest whose correction fades out, the weight it was drawn with when the layer began
    /// to move, and its weight when the layer stopped.
    rest: [f32; 4],
    rest_w: f32,
    held: f32,
}

/// A scaled layer's snapping this frame, handed down its subtree.
#[derive(Clone, Copy)]
struct LayerSnap {
    /// Weight of the snap where the layer is now: 0 while it moves, easing to 1 once it holds.
    to: f32,
    /// Weight of the correction it drew at its last rest, fading out.
    from: f32,
    /// Maps a device px coordinate now to where it was at that rest: `x * a + b`, per axis.
    map: [f32; 4],
    /// The layer's rigid translation (device px).
    rigid: [f32; 2],
}

const NO_LAYER: LayerSnap = LayerSnap { to: 0.0, from: 0.0, map: [1.0, 0.0, 1.0, 0.0], rigid: [0.0; 2] };

/// The largest distance between matching edges.
fn far(a: [f32; 4], b: [f32; 4]) -> f32 {
    (0..4).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max)
}

impl Glides {
    /// Called once before each paint pass, at animation time `t`.
    pub(super) fn begin_frame(&mut self, t: f64) {
        self.frame = self.frame.wrapping_add(1);
        self.t = t;
        self.now.clear();
        self.easing = false;
        let frame = self.frame;
        if !self.glides.is_empty() {
            self.glides.retain(|_, g| frame.wrapping_sub(g.seen) < KEEP);
        }
        if !self.layers.is_empty() {
            self.layers.retain(|_, l| frame.wrapping_sub(l.seen) < KEEP);
        }
    }

    /// Whether a scaled layer's correction is still easing in or out: the last paint asks for
    /// another frame.
    pub(super) fn easing(&self) -> bool {
        self.easing
    }

    /// Node `id` has its own scale and heads a layer: `r` is its drawn rect (device px edges)
    /// and `rigid` its rigid translation (device px). `cached` content snaps without easing,
    /// as its raster isn't redrawn while the correction eases. The returned index goes in
    /// `Inherit::layer`.
    pub(super) fn layer(&mut self, id: NodeId, r: [f32; 4], rigid: [f32; 2], cached: bool) -> u32 {
        let cur: [f32; 4] = core::array::from_fn(|k| r[k] - rigid[k % 2]);
        let (t, frame) = (self.t, self.frame);
        let l = self.layers.entry(id).or_insert(Layer {
            rect: cur, t, seen: frame, moving: false, since: f64::NEG_INFINITY, rest: cur, rest_w: 0.0, held: 0.0,
        });
        let eased = |since: f64| {
            let x = if cached { 1.0 } else { ((t - since) / SETTLE).clamp(0.0, 1.0) as f32 };
            x * x * (3.0 - 2.0 * x)
        };
        let (to, from);
        if far(cur, l.rect) > 1.0 / 256.0 {
            if !l.moving {
                // It drew `f` of this rest's correction and `last` of the one before: carry the
                // larger out.
                let f = eased(l.since);
                let last = l.held * (1.0 - f);
                if f >= last {
                    (l.rest, l.rest_w) = (l.rect, f);
                } else {
                    l.rest_w = last;
                }
                l.moving = true;
            }
            (l.rect, l.t) = (cur, t);
            to = 0.0;
            from = l.rest_w * (1.0 - far(cur, l.rest)).clamp(0.0, 1.0);
        } else {
            if l.moving {
                // Still since its last paint, which already drew it here.
                l.moving = false;
                l.since = l.t;
                l.held = l.rest_w * (1.0 - far(cur, l.rest)).clamp(0.0, 1.0);
            }
            to = eased(l.since);
            from = l.held * (1.0 - to);
        }
        l.seen = frame;
        let mut map = NO_LAYER.map;
        if from > 0.0 {
            for a in 0..2 {
                let (w, rest_w) = (cur[a + 2] - cur[a], l.rest[a + 2] - l.rest[a]);
                let k = if w.abs() > 1e-3 { rest_w / w } else { 1.0 };
                (map[a * 2], map[a * 2 + 1]) = (k, l.rest[a] - cur[a] * k);
            }
        }
        self.now.push(LayerSnap { to, from, map, rigid });
        self.now.len() as u32
    }

    /// Node `id` is at rest and unscaled: its paint rect `r` (device px edges) snapped, with
    /// `rigid` from `Inherit::rigid` in device px.
    #[inline]
    pub(super) fn at_rest(&mut self, id: NodeId, r: [f32; 4], rigid: [f32; 2]) -> [f32; 4] {
        if !self.glides.is_empty() {
            self.glides.remove(&id);
        }
        snap(r, [0.0; 4], rigid, |_, c| c)
    }

    /// Node `id`'s paint rect `r` (device px edges) snapped. `unscaled` is the same rect before
    /// the scales on it and above it, `motion` from [`motion`], and `rigid` as in
    /// [`Self::at_rest`]. `layer` is `Inherit::layer` and `sc` the node's scale.
    pub(super) fn snapped(&mut self, id: NodeId, motion: Option<Motion>, (r, unscaled): ([f32; 4], [f32; 4]), rigid: [f32; 2], (layer, sc): (u32, f32)) -> [f32; 4] {
        // How far scaling moves the edges. Snapping to the unscaled grid fades in over its last
        // pixel, so a scale settling at 1 lands on the grid without a jump.
        let moved = far(r, unscaled);
        let weight = ((1.0 - moved) / (1.0 - 1.0 / 16.0)).clamp(0.0, 1.0);
        let l = layer.checked_sub(1).map_or(NO_LAYER, |i| self.now[i as usize]);
        let mut out = r;
        if weight > 0.0 && l.to < 1.0 {
            let snapped = match motion {
                None => self.at_rest(id, unscaled, rigid),
                Some(m) if l.to == 0.0 => self.glide(id, m, unscaled, rigid),
                Some(m) => snap(unscaled, m.by, rigid, |_, c| c),
            };
            for k in 0..4 {
                out[k] += (snapped[k] - r[k]) * weight * (1.0 - l.to);
            }
        }
        if layer == 0 {
            return out;
        }
        // Translations inside the layer are scaled with it.
        let lr = l.rigid;
        let rigid: [f32; 2] = core::array::from_fn(|a| lr[a] + (rigid[a] - lr[a]) * sc);
        let mut left = 0.0f32;
        if l.from > 0.0 && weight < 1.0 {
            let m: [f32; 4] = core::array::from_fn(|k| {
                let a = k % 2;
                (r[k] - lr[a]) * l.map[a * 2] + l.map[a * 2 + 1] + lr[a]
            });
            let at = snap(m, [0.0; 4], rigid, |_, c| c);
            for k in 0..4 {
                let c = (at[k] - m[k]) * l.from * (1.0 - weight);
                out[k] += c;
                left = left.max(c.abs());
            }
        }
        let at = match motion {
            Some(m) if l.to > 0.0 => self.glide(id, Motion { by: m.by.map(|v| v * sc), k: sc, ..m }, r, rigid),
            _ if l.to > 0.0 => self.at_rest(id, r, rigid),
            // Moving: where holding still would take it, to know if it has anywhere to go.
            _ => snap(r, [0.0; 4], rigid, |_, c| c),
        };
        if l.to == 1.0 && l.from == 0.0 {
            return at;
        }
        for k in 0..4 {
            out[k] += (at[k] - r[k]) * l.to;
            left = left.max((at[k] - r[k]).abs() * (1.0 - l.to));
        }
        self.easing |= left > 1.0 / 128.0;
        out
    }

    fn glide(&mut self, id: NodeId, m: Motion, r: [f32; 4], rigid: [f32; 2]) -> [f32; 4] {
        let free: [f32; 4] = core::array::from_fn(|k| r[k] - rigid[k % 2]);
        let mut g = match self.glides.get(&id) {
            Some(g) if g.rest == m.rest && g.k == m.k => *g,
            prev => {
                // A new rest: start from the correction drawn with now, so nothing jumps.
                let from = match prev {
                    Some(g) => core::array::from_fn(|k| g.at(k, g.to[k], free[k] - g.rest_px[k])),
                    None => {
                        let mut now = [0.0; 4];
                        snap(r, [0.0; 4], rigid, |k, c| {
                            now[k] = c;
                            c
                        });
                        now
                    }
                };
                Glide { rest: m.rest, k: m.k, rest_px: core::array::from_fn(|k| free[k] - m.by[k]), from, to: from, span: m.by, seen: 0 }
            }
        };
        let out = snap(r, m.by, rigid, |k, c| {
            g.to[k] = c;
            g.at(k, c, m.by[k])
        });
        g.seen = self.frame;
        self.glides.insert(id, g);
        out
    }
}

/// Rounds half up on the device grid, the same way at every position.
fn round(v: f32) -> f32 {
    (v + 0.5).floor()
}

/// `r` (device px edges, `x0, y0, x1, y1`) snapped. `motion` is how far each edge is from where
/// it comes to rest and `rigid` the translation that snaps as a whole (both device px). Each
/// edge's correction toward its resting rounding passes through `ease(edge, correction)`.
#[inline]
pub(super) fn snap(r: [f32; 4], motion: [f32; 4], rigid: [f32; 2], mut ease: impl FnMut(usize, f32) -> f32) -> [f32; 4] {
    let mut out = r;
    for axis in 0..2 {
        let (a, b) = (axis, axis + 2);
        let (rest_a, rest_b) = (r[a] - motion[a] - rigid[axis], r[b] - motion[b] - rigid[axis]);
        if rest_b - rest_a < 0.5 {
            continue;
        }
        let shift = round(rigid[axis]);
        let lo = round(rest_a);
        let hi = round(rest_b).max(lo + 1.0);
        for (k, rest, at) in [(a, rest_a, lo), (b, rest_b, hi)] {
            // Built from the rounded edge, so a box at rest lands exactly on whole pixels.
            let (c, eased) = (at - rest, ease(k, at - rest));
            out[k] = if eased == c { at } else { rest + eased } + shift + motion[k];
        }
    }
    out
}
