//! Compact per-node inherited paint state (`Inh`) and its indexed cold side table (`InhCold`).
//!
//! `Inh` sits in `PaintState::inh_scratch` and `PaintKeep::inh` once per tree node (thousands
//! of entries on large documents), so keeping rarely-populated fields (soft-edge fades, rounded
//! clips, nested-flight outer clips, hover-group rects, press tint, scale anchors, and
//! pixel-snapping parent geometry) in `PaintState::inh_cold` behind a 1-based `u32` handle
//! (`0` = `InhCold::DEFAULT`) shrinks `size_of::<Inh>()` by >5x while descendants share their
//! parent's slot.

use super::*;

pub(in crate::runtime) const DEFAULT_GROUP_RECT: [f32; 4] =
    [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY];

const F_HELD_MOVING: u8 = 1 << 0;
const F_SCALE_TRACKED: u8 = 1 << 1;
const F_IN_CACHE: u8 = 1 << 2;
const F_SKIP_PAINT: u8 = 1 << 3;
const F_ANCHOR_HIDDEN: u8 = 1 << 4;
const F_SNAP_OFF: u8 = 1 << 5;
const F_SNAP_STEPS: u8 = 1 << 6;
const F_SNAP_SHIFTED: u8 = 1 << 7;

/// Hot per-node inherited paint state kept in `PaintState::inh_scratch` and `PaintKeep::inh`.
#[derive(Clone, Copy)]
pub(in crate::runtime) struct Inh {
    pub(in crate::runtime) opacity: f32,
    pub(in crate::runtime) paint_opacity: f32,
    /// `opacity` without active presence entrance fades and with each `El::group_opacity` at its
    /// hovered level: newly mounted controls on frame 0 and hover-revealed controls take hits.
    pub(in crate::runtime) hit_opacity: f32,
    /// Ancestors' scales, composed: a point `x` (points, with `dx`/`dy` applied) draws at
    /// `anchor_to + (x - anchor) * scale`.
    pub(in crate::runtime) scale: f32,
    /// Horizontal offset in points: horizontal scroll or a matched flight centring scaled children.
    pub(in crate::runtime) dx: f32,
    /// Vertical offset in points from scroll viewports and growth rows above.
    pub(in crate::runtime) dy: f32,
    pub(in crate::runtime) clip: [f32; 4],
    /// Paint clips are consumed by isolation; logical clips remain for hit tests/inspection.
    pub(in crate::runtime) paint_clip: [f32; 4],
    /// The nearest hover group's hover amount (0-1), for `group_*` styles.
    pub(in crate::runtime) group: f32,
    pub(in crate::runtime) mark: u32,
    /// Ancestors' `El::rotate` layers, composed, as an index into `turn_scratch` plus one
    /// (0: none): where this subtree's device pixels land.
    pub(in crate::runtime) turn: u32,
    /// Cold inherited fields as an index into `inh_cold` plus one (0: `InhCold::DEFAULT`).
    pub(in crate::runtime) cold: u32,
    /// Rigid translation in points from scrolls, sticky shifts and growth rows above, which
    /// pixel snapping rounds as a whole.
    pub(in crate::runtime) rigid: [f32; 2],
    /// How `scale` is changing, which decides how text rasterizes under it.
    pub(in crate::runtime) scale_motion: ScaleMotion,
    /// Packed inherited boolean flags.
    pub(in crate::runtime) flags: u8,
}

impl Default for Inh {
    #[inline]
    fn default() -> Self {
        Self {
            opacity: 1.0,
            paint_opacity: 1.0,
            hit_opacity: 1.0,
            scale: 1.0,
            dx: 0.0,
            dy: 0.0,
            clip: NO_CLIP,
            paint_clip: NO_CLIP,
            group: 0.0,
            mark: 0,
            turn: 0,
            cold: 0,
            rigid: [0.0; 2],
            scale_motion: ScaleMotion::Still,
            flags: 0,
        }
    }
}

impl Inh {
    #[inline(always)]
    pub(in crate::runtime) fn held_moving(self) -> bool { self.flags & F_HELD_MOVING != 0 }
    #[inline(always)]
    pub(in crate::runtime) fn set_held_moving(&mut self, on: bool) { self.set_flag(F_HELD_MOVING, on); }
    #[inline(always)]
    pub(in crate::runtime) fn scale_tracked(self) -> bool { self.flags & F_SCALE_TRACKED != 0 }
    #[inline(always)]
    pub(in crate::runtime) fn set_scale_tracked(&mut self, on: bool) { self.set_flag(F_SCALE_TRACKED, on); }
    #[inline(always)]
    pub(in crate::runtime) fn in_cache(self) -> bool { self.flags & F_IN_CACHE != 0 }
    #[inline(always)]
    pub(in crate::runtime) fn set_in_cache(&mut self, on: bool) { self.set_flag(F_IN_CACHE, on); }
    #[inline(always)]
    pub(in crate::runtime) fn skip_paint(self) -> bool { self.flags & F_SKIP_PAINT != 0 }
    #[inline(always)]
    pub(in crate::runtime) fn set_skip_paint(&mut self, on: bool) { self.set_flag(F_SKIP_PAINT, on); }
    #[inline(always)]
    pub(in crate::runtime) fn anchor_hidden(self) -> bool { self.flags & F_ANCHOR_HIDDEN != 0 }
    #[inline(always)]
    pub(in crate::runtime) fn set_anchor_hidden(&mut self, on: bool) { self.set_flag(F_ANCHOR_HIDDEN, on); }
    #[inline(always)]
    pub(in crate::runtime) fn snap_off(self) -> bool { self.flags & F_SNAP_OFF != 0 }
    #[inline(always)]
    pub(in crate::runtime) fn snap_steps(self) -> bool { self.flags & F_SNAP_STEPS != 0 }
    #[inline(always)]
    pub(in crate::runtime) fn snap_shifted(self) -> bool { self.flags & F_SNAP_SHIFTED != 0 }
    #[inline(always)]
    pub(in crate::runtime) fn set_snap_shifted(&mut self, on: bool) { self.set_flag(F_SNAP_SHIFTED, on); }

    #[inline(always)]
    fn set_flag(&mut self, bit: u8, on: bool) {
        if on { self.flags |= bit; } else { self.flags &= !bit; }
    }

    #[inline(always)]
    pub(in crate::runtime) fn pack_flags(
        held_moving: bool,
        scale_tracked: bool,
        in_cache: bool,
        skip_paint: bool,
        anchor_hidden: bool,
        snap_off: bool,
        snap_steps: bool,
        snap_shifted: bool,
    ) -> u8 {
        (u8::from(held_moving) * F_HELD_MOVING)
            | (u8::from(scale_tracked) * F_SCALE_TRACKED)
            | (u8::from(in_cache) * F_IN_CACHE)
            | (u8::from(skip_paint) * F_SKIP_PAINT)
            | (u8::from(anchor_hidden) * F_ANCHOR_HIDDEN)
            | (u8::from(snap_off) * F_SNAP_OFF)
            | (u8::from(snap_steps) * F_SNAP_STEPS)
            | (u8::from(snap_shifted) * F_SNAP_SHIFTED)
    }

    /// A translation that moves the subtree as a scroll does, `by` points on `axis`.
    #[inline(always)]
    pub(in crate::runtime) fn scroll_rigid(&mut self, axis: usize, by: f32) {
        self.rigid[axis] += by;
    }

    /// A growth offset `dy` added to the subtree's rigid vertical translation.
    #[inline(always)]
    pub(in crate::runtime) fn grow_rigid(&mut self, dy: f32) {
        self.rigid[1] += dy;
    }
}

/// Rarely-populated inherited paint state stored in `PaintState::inh_cold` (and swapped with
/// `PaintKeep::inh_cold`) behind `Inh::cold`.
#[derive(Clone, Copy, PartialEq)]
pub(in crate::runtime) struct InhCold {
    /// `El::stuck_fade`: how much of a stuck header's first child (its title) shows.
    pub(in crate::runtime) first_fade: f32,
    pub(in crate::runtime) fade: [f32; 4],
    pub(in crate::runtime) fade_origin: [f32; 4],
    pub(in crate::runtime) paint_fade: [f32; 4],
    pub(in crate::runtime) paint_fade_origin: [f32; 4],
    pub(in crate::runtime) rounded: Option<([f32; 4], [f32; 4])>,
    /// The clip, fade and fade origin outside the nearest flight above, for a flight nested in it.
    pub(in crate::runtime) outer: Option<[[f32; 4]; 3]>,
    pub(in crate::runtime) anchor: [f32; 2],
    pub(in crate::runtime) anchor_to: [f32; 2],
    pub(in crate::runtime) blur: f32,
    /// The part of `dx`/`dy` from flights' covers above.
    pub(in crate::runtime) cover: [f32; 2],
    /// The nearest `El::press_color` above or here, and how far a press on it is held (0-1).
    pub(in crate::runtime) pressed: f32,
    pub(in crate::runtime) press_to: Option<Color>,
    /// The nearest hover group's rect in points (x0, y0, x1, y1): where a pointer hovers it.
    pub(in crate::runtime) group_rect: [f32; 4],
    /// True when an ancestor's `El::group_opacity` is below its hovered level, so hits while
    /// `effective_opacity <= 0.001` stay clipped to `group_rect`.
    pub(in crate::runtime) group_hidden: bool,
    /// `(scale, anchor, anchor_to)` of ancestors whose scales hold still from the root down.
    pub(in crate::runtime) still: (f32, [f32; 2], [f32; 2]),
    /// `(scale, anchor, anchor_to)` of ancestors whose scales the app holds or drives.
    pub(in crate::runtime) held: (f32, [f32; 2], [f32; 2]),
    /// Composed resting target scale when a `P_SCALE` track drives this subtree (no live scale).
    pub(in crate::runtime) scale_target: f32,
    /// Accumulated `(current - target)` residual of any animating `El::press_offset` here or above.
    pub(in crate::runtime) press_off_by: [f32; 2],
    /// Paint containments' origins alone (overlays start it again at zero).
    pub(in crate::runtime) snap_steady: [f32; 2],
    /// Origin in points of the paint containment above (`El::paint_contained`).
    pub(in crate::runtime) snap_contain: [f32; 2],
    /// Nearest scaled snap layer above or here, as an index plus one (0: none).
    pub(in crate::runtime) snap_layer: u32,
    /// The parent's edges, for centring.
    pub(in crate::runtime) snap_parent: Option<pixel_snap::Parent>,
}

impl InhCold {
    pub(in crate::runtime) const DEFAULT: Self = Self {
        first_fade: 1.0,
        fade: [0.0; 4],
        fade_origin: NO_CLIP,
        paint_fade: [0.0; 4],
        paint_fade_origin: NO_CLIP,
        rounded: None,
        outer: None,
        anchor: [0.0; 2],
        anchor_to: [0.0; 2],
        blur: 0.0,
        cover: [0.0; 2],
        pressed: 0.0,
        press_to: None,
        group_rect: DEFAULT_GROUP_RECT,
        group_hidden: false,
        still: (1.0, [0.0; 2], [0.0; 2]),
        held: (1.0, [0.0; 2], [0.0; 2]),
        scale_target: 1.0,
        press_off_by: [0.0; 2],
        snap_steady: [0.0; 2],
        snap_contain: [0.0; 2],
        snap_layer: 0,
        snap_parent: None,
    };

    /// `El::paint_contained` at `o` (points): children snap against it, rounded on its own.
    #[inline]
    pub(in crate::runtime) fn contain_at(&mut self, rigid: &mut [f32; 2], o: [f32; 2]) {
        let c = self.snap_contain;
        *rigid = [rigid[0] - c[0] + o[0], rigid[1] - c[1] + o[1]];
        self.snap_steady = [self.snap_steady[0] - c[0] + o[0], self.snap_steady[1] - c[1] + o[1]];
        self.snap_contain = o;
    }
}

impl Default for InhCold {
    #[inline]
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// How a subtree's scale is changing. Text under a still scale rasterizes at its scaled size.
/// Under a live zoom gesture it steps through logarithmic $2^{1/4}$ buckets and lands on the
/// exact resting scale once still; under a short spring it samples its rest-size glyphs.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::runtime) enum ScaleMotion {
    #[default]
    Still,
    /// Bound to a live scale that is actively changing: rasterize at $2^{1/4}$ octave buckets.
    Live,
    Moving,
}

impl ScaleMotion {
    /// The scale text under `sc` rasterizes at. It draws at `sc`, filtered, when they differ.
    pub(in crate::runtime) fn raster(self, sc: f32, rest: f32) -> f32 {
        let k = match self {
            ScaleMotion::Still => sc,
            ScaleMotion::Live => 2.0f32.powf((sc.max(1e-4).log2() * 4.0).round() * 0.25),
            ScaleMotion::Moving => rest,
        };
        if (1.0 / 16.0..=4.0).contains(&k) { k } else { 1.0 }
    }
}

#[inline(always)]
fn hash_f32s(h: &mut rune_motion::Fx, v: &[f32]) {
    use std::hash::Hasher;
    let mut pairs = v.chunks_exact(2);
    for p in &mut pairs {
        h.write_u64(u64::from(p[0].to_bits()) | u64::from(p[1].to_bits()) << 32);
    }
    if let [x] = pairs.remainder() {
        h.write_u32(x.to_bits());
    }
}

/// The bits of what a node inherits, returning `(full, without_scroll_translation)`.
/// Hashes `cold` by value (never by handle index) and skips cold hashing when `is_default_cold`.
pub(in crate::runtime) fn inh_keys(p: &Inh, c: &InhCold, is_default_cold: bool) -> (u64, u64) {
    use std::hash::Hasher;
    let mut h = rune_motion::Fx::default();
    hash_f32s(&mut h, &[p.opacity, p.paint_opacity, p.hit_opacity, p.scale, p.group]);
    hash_f32s(&mut h, &p.clip);
    hash_f32s(&mut h, &p.paint_clip);
    h.write_u32(p.mark);
    h.write_u32(p.turn);
    h.write_u8(p.scale_motion as u8);
    h.write_u8(p.flags);
    if is_default_cold {
        h.write_u8(0);
    } else {
        h.write_u8(1);
        hash_f32s(&mut h, &[c.first_fade, c.blur, c.pressed, c.scale_target]);
        for a in [c.fade, c.fade_origin, c.paint_fade, c.paint_fade_origin, c.group_rect] {
            hash_f32s(&mut h, &a);
        }
        h.write_u8(u8::from(c.group_hidden));
        hash_f32s(&mut h, &c.anchor);
        hash_f32s(&mut h, &c.anchor_to);
        hash_f32s(&mut h, &c.cover);
        hash_f32s(&mut h, &c.press_off_by);
        hash_f32s(&mut h, &c.snap_steady);
        hash_f32s(&mut h, &c.snap_contain);
        match c.rounded {
            Some((a, b)) => { h.write_u8(1); hash_f32s(&mut h, &a); hash_f32s(&mut h, &b); }
            None => h.write_u8(0),
        }
        match c.outer {
            Some(o) => { h.write_u8(1); for a in o { hash_f32s(&mut h, &a); } }
            None => h.write_u8(0),
        }
        match c.press_to {
            Some(col) => { h.write_u8(1); hash_f32s(&mut h, &[col.r, col.g, col.b, col.a]); }
            None => h.write_u8(0),
        }
        hash_f32s(&mut h, &[c.still.0]);
        hash_f32s(&mut h, &c.still.1);
        hash_f32s(&mut h, &c.still.2);
        hash_f32s(&mut h, &[c.held.0]);
        hash_f32s(&mut h, &c.held.1);
        hash_f32s(&mut h, &c.held.2);
        h.write_u32(c.snap_layer);
        match &c.snap_parent {
            Some(sp) => { h.write_u8(1); sp.hash_bits(&mut h); }
            None => h.write_u8(0),
        }
    }
    let base = h.finish();
    let full = keep::inh_key_from_base(base, [p.dx, p.dy, p.rigid[0], p.rigid[1]]);
    (full, base)
}

/// The bits of what a node inherits, resolving `p.cold` against `cold_table`.
pub(in crate::runtime) fn inh_key(p: &Inh, cold_table: &[InhCold]) -> u64 {
    match p.cold.checked_sub(1).and_then(|k| cold_table.get(k as usize)) {
        Some(c) => inh_keys(p, c, false).0,
        None => inh_keys(p, &InhCold::DEFAULT, true).0,
    }
}
