//! Scroll-translated reuse for `PaintKeep`: a clean subtree whose inherited state changed
//! only by a rigid scroll offset copies its last paint (shifting its instances, hits, drawn
//! rects, inherited scratch and sticky frames) instead of walking and painting its descendants.

use std::hash::Hasher;

use crate::render::{KIND_LAYER, KIND_LAYER_END};

use super::*;

pub(super) const ANY_SEEN: u8 = 1 << 0;
pub(super) const SHIFT_VIS: u8 = 1 << 1;
pub(super) const SHIFT_OFF: u8 = 1 << 2;
pub(super) const INNER_CLIP: u8 = 1 << 3;
pub(super) const INNER_HOVER: u8 = 1 << 4;
pub(super) const CLIP_HIT: u8 = 1 << 5;
pub(super) const HAS_HIT: u8 = 1 << 6;

const EMPTY_BOUNDS: [f32; 4] = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];

/// Per-node metadata recorded alongside `PaintRec` to decide and execute scroll-translated reuse.
#[derive(Clone, Copy)]
pub(super) struct ShiftRec {
    /// Hash of `Inh` without `dx`, `dy` and `snap.rigid`.
    pub(super) inh_base: u64,
    /// Inherited `[dx, dy, snap.rigid[0], snap.rigid[1]]` entering this node.
    pub(super) origin: [f32; 4],
    /// Conservative device-pixel bounds covering every node, shadow, outline, instance and text
    /// band in this subtree (used to keep a visible subtree strictly inside its clip/fade).
    pub(super) bounds: [f32; 4],
    /// Tighter device-pixel bounds of `screen_r ± m` across this subtree (matching `build`'s
    /// visibility and pixel-snap cull tests, used to keep an offscreen subtree wholly offscreen).
    pub(super) cull: [f32; 4],
    pub(super) flags: u8,
}

impl Default for ShiftRec {
    fn default() -> Self {
        Self {
            inh_base: 0,
            origin: [0.0; 4],
            bounds: EMPTY_BOUNDS,
            cull: EMPTY_BOUNDS,
            flags: 0,
        }
    }
}

/// Local paint-loop facts for node `i` passed into `keep_painted`.
#[derive(Clone, Copy)]
pub(in crate::runtime) struct ShiftNode {
    pub(in crate::runtime) screen_r: [f32; 4],
    pub(in crate::runtime) r: [f32; 4],
    pub(in crate::runtime) rect_pt: [f32; 4],
    pub(in crate::runtime) m: f32,
    pub(in crate::runtime) visible: bool,
    pub(in crate::runtime) can_snap: bool,
    pub(in crate::runtime) gliding: bool,
    pub(in crate::runtime) isolate: bool,
    pub(in crate::runtime) unmasked: bool,
    pub(in crate::runtime) hover_owner: bool,
    pub(in crate::runtime) effective_opacity: f32,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum ShiftMode {
    Offscreen,
    Visible,
}

#[derive(Clone, Copy)]
pub(super) struct ShiftPlan {
    pub(super) mode: ShiftMode,
    pub(super) ddx: f32,
    pub(super) ddy: f32,
    pub(super) ddx_px: f32,
    pub(super) ddy_px: f32,
    pub(super) shift_x: f32,
    pub(super) shift_y: f32,
    pub(super) outer_clip: [f32; 4],
    pub(super) outer_paint_clip: [f32; 4],
    pub(super) outer_group_rect: [f32; 4],
    pub(super) inh_base: u64,
    pub(super) inh_full: u64,
    pub(super) flags: u8,
}

#[inline(always)]
pub(super) fn union_bounds(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

#[inline(always)]
pub(super) fn is_pow2_scale(s: f32) -> bool {
    s == 1.0 || s == 2.0 || s == 4.0
}

#[inline(always)]
pub(in crate::runtime) fn inh_key_from_base(base: u64, origin: [f32; 4]) -> u64 {
    let mut h = rune_motion::Fx::default();
    h.write_u64(base);
    bits(&mut h, &origin);
    h.finish()
}

/// Populates `ShiftRec` for node `i` after it painted itself.
pub(super) fn note_painted(rt: &mut RuntimeCore, i: usize, info: ShiftNode) {
    let s = rt.env.scale;
    let n = &rt.tree.nodes[i];
    let cull = [
        info.screen_r[0] - info.m,
        info.screen_r[1] - info.m,
        info.screen_r[2] + info.m,
        info.screen_r[3] + info.m,
    ];
    let mut bounds = union_bounds(
        cull,
        [info.r[0].min(info.r[2]), info.r[1].min(info.r[3]), info.r[0].max(info.r[2]), info.r[1].max(info.r[3])],
    );
    bounds = union_bounds(bounds, info.rect_pt.map(|v| v * s));

    let common_ok = !info.isolate
        && n.vis.cutouts.is_empty()
        && n.vis.clip_radius.is_none()
        && n.vis.clip_corners.is_none()
        && info.unmasked
        && !n.collapsing
        && n.vis.clip.is_none_or(|f| f == [0.0; 4])
        && !(info.hover_owner && info.effective_opacity <= 0.001)
        && n.vis.cold.get().is_none_or(|c| c.paint_contain.is_none());

    let mut shift_vis = common_ok
        && info.can_snap
        && !info.gliding
        && n.vis.grid.is_none()
        && !n.cold.get().is_some_and(|c| c.help_open)
        && !matches!(n.text_layout.as_ref(), Some(TextLayout::Paragraphs(p)) if p.waiting())
        && !n.vis_flags.hit();
    let shift_off = common_ok;

    if info.visible
        && let (Some(spec), Some(laid)) = (&n.text, &n.text_layout)
    {
        let em = spec.spans.iter().filter_map(|sp| sp.size).fold(spec.style.size.max(spec.style.line_px()), f32::max);
        let pad = 2.0 * em * s + 2.0;
        let flow = laid.view();
        let full_w = flow.full_width();
        let full_h = text_height(spec, flow, s);
        let tb = [
            info.r[0].min(info.r[2] - full_w) - pad,
            info.r[1].min(info.r[3] - full_h) - pad,
            info.r[2].max(info.r[0] + full_w) + pad,
            info.r[3].max(info.r[1] + full_h) + pad,
        ];
        bounds = union_bounds(bounds, tb);
    }

    let start = rt.paint.keep.recs[i].inst[0] as usize;
    for q in &rt.instances[start..] {
        let kind = q.params[1];
        if kind == KIND_LAYER || kind == KIND_LAYER_END {
            shift_vis = false;
        }
        let pad = if kind == KIND_SHADOW { q.params[2] * 3.0 + 2.0 } else { 2.0 };
        let qb = [q.rect[0] - pad, q.rect[1] - pad, q.rect[0] + q.rect[2] + pad, q.rect[1] + q.rect[3] + pad];
        bounds = union_bounds(bounds, qb);
    }

    let inner_clip = n.vis.clip.is_some()
        || n.elided.is_some()
        || n.text.as_ref().is_some_and(|t| t.truncate || t.truncate_fade || t.cold.live.is_some());
    let mut flags = 0u8;
    if info.visible { flags |= ANY_SEEN; }
    if shift_vis { flags |= SHIFT_VIS; }
    if shift_off { flags |= SHIFT_OFF; }
    if inner_clip { flags |= INNER_CLIP; }
    if info.hover_owner { flags |= INNER_HOVER; }

    let sr = &mut rt.paint.keep.shift[i];
    sr.bounds = bounds;
    sr.cull = cull;
    sr.flags = flags;
}

/// Folds closed child `r`'s shift metadata into its parent `p`.
pub(super) fn fold_into_parent(nodes: &[Node], shift: &mut [ShiftRec], rec: PaintRec, r: usize, p: usize) {
    let own_hit = rec.hits[1] > rec.hits[0]
        || rec.wheel[1] > rec.wheel[0]
        || rec.help[1] > rec.help[0]
        || rec.drop[1] > rec.drop[0];
    if own_hit {
        shift[r].flags |= HAS_HIT;
    }
    let child = shift[r];
    let parent = &mut shift[p];
    parent.bounds = union_bounds(parent.bounds, child.bounds);
    parent.cull = union_bounds(parent.cull, child.cull);
    parent.flags |= child.flags & (ANY_SEEN | INNER_CLIP | INNER_HOVER | CLIP_HIT | HAS_HIT);
    if nodes[p].vis.clip.is_some() && (child.flags & HAS_HIT) != 0 {
        parent.flags |= CLIP_HIT;
    }
    parent.flags &= child.flags | !(SHIFT_VIS | SHIFT_OFF);
}

/// Decides whether clean subtree `i` (previously at `o`) can be reused by shifting its last
/// paint by the rigid scroll delta in `pinh`.
pub(super) fn plan(
    rt: &RuntimeCore,
    o: usize,
    pinh: &Inh,
    pcold: &InhCold,
    inh_full: u64,
    inh_base: u64,
) -> Option<ShiftPlan> {
    let k = &rt.paint.keep;
    let r = k.prev[o];
    let sr = k.prev_shift[o];
    if sr.inh_base != inh_base || r.overlay[0] != r.overlay[1] {
        return None;
    }
    if pinh.scale != 1.0
        || pcold.anchor != pcold.anchor_to
        || pinh.turn != 0
        || pinh.in_cache()
        || pinh.skip_paint()
        || pinh.anchor_hidden()
        || pcold.snap_layer != 0
        || pcold.cover != [0.0; 2]
        || pcold.press_off_by != [0.0; 2]
        || pcold.snap_parent.is_some_and(|p| p.left_moving())
    {
        return None;
    }
    if rt.tree.has_sticky && rt.scroll.holding() {
        return None;
    }
    let ddx = pinh.dx - sr.origin[0];
    let ddy = pinh.dy - sr.origin[1];
    if (ddx == 0.0 && ddy == 0.0) || !ddx.is_finite() || !ddy.is_finite() {
        return None;
    }
    if (pinh.rigid[0] - sr.origin[2]) != ddx || (pinh.rigid[1] - sr.origin[3]) != ddy {
        return None;
    }
    let s = rt.env.scale;
    // Scroll offsets (`st.drawn`, `st.drawn_x`) and virtual-window origins (`row_dy[ci]`) sit on
    // the 1/64 px grid; continuous spring `row_dy` shifts do not and must paint fresh so
    // `grid(p.rect + dy)` stays bit-exact.
    if grid(pinh.dx, s) != pinh.dx
        || grid(pinh.dy, s) != pinh.dy
        || grid(sr.origin[0], s) != sr.origin[0]
        || grid(sr.origin[1], s) != sr.origin[1]
    {
        return None;
    }
    let max_mag = if is_pow2_scale(s) { 65536.0 } else { 8192.0 };
    if pinh.dx.abs() >= max_mag || pinh.dy.abs() >= max_mag
        || sr.origin[0].abs() >= max_mag || sr.origin[1].abs() >= max_mag
    {
        return None;
    }
    let (ddx_px, ddy_px) = (ddx * s, ddy * s);
    let shift_x = (pinh.rigid[0] * s + 0.5).floor() - (sr.origin[2] * s + 0.5).floor();
    let shift_y = (pinh.rigid[1] * s + 0.5).floor() - (sr.origin[3] * s + 0.5).floor();

    // Avoid crossing 0 on a moving axis so `f32::round` half-quantum sign flips cannot occur.
    if ddy != 0.0 && !((sr.cull[1] > 0.0 && sr.cull[1] + ddy_px > 0.0) || (sr.cull[3] < 0.0 && sr.cull[3] + ddy_px < 0.0)) {
        return None;
    }
    if ddx != 0.0 && !((sr.cull[0] > 0.0 && sr.cull[0] + ddx_px > 0.0) || (sr.cull[2] < 0.0 && sr.cull[2] + ddx_px < 0.0)) {
        return None;
    }

    let vp = k.vp;
    let vis_clip = pinh.paint_clip;
    let mode = if (sr.flags & ANY_SEEN) == 0 {
        if (sr.flags & SHIFT_OFF) == 0
            || r.inst[0] != r.inst[1]
            || r.hits[0] != r.hits[1]
            || r.wheel[0] != r.wheel[1]
            || r.help[0] != r.help[1]
            || r.drop[0] != r.drop[1]
        {
            return None;
        }
        let clip_has_area = vis_clip[2] > vis_clip[0] && vis_clip[3] > vis_clip[1];
        if clip_has_area {
            let v = [vis_clip[0].max(vp[0]), vis_clip[1].max(vp[1]), vis_clip[2].min(vp[2]), vis_clip[3].min(vp[3])];
            let pad = 3.0;
            let nx0 = sr.cull[0] + ddx_px;
            let ny0 = sr.cull[1] + ddy_px;
            let nx1 = sr.cull[2] + ddx_px;
            let ny1 = sr.cull[3] + ddy_px;
            let still_off = nx1 < v[0] - pad || nx0 > v[2] + pad || ny1 < v[1] - pad || ny0 > v[3] + pad;
            if !still_off {
                return None;
            }
        }
        ShiftMode::Offscreen
    } else {
        if (sr.flags & SHIFT_VIS) == 0 {
            return None;
        }
        if (sr.flags & INNER_CLIP) != 0 {
            if pcold.paint_fade != [0.0; 4] || pcold.fade != [0.0; 4] || pcold.rounded.is_some() {
                return None;
            }
            if (sr.flags & CLIP_HIT) != 0 && (shift_x != ddx_px || shift_y != ddy_px) {
                return None;
            }
        }
        let mut safe = intersect(intersect(pinh.clip, vis_clip), vp);
        // Keep clear of inherited soft-edge fades on both paint and hit channels.
        for (fade, origin) in [(pcold.paint_fade, pcold.paint_fade_origin), (pcold.fade, pcold.fade_origin)] {
            let w = fade.map(soft_edges::fade_extent);
            if w[0] > 0.0 { safe[0] = safe[0].max(origin[0] + w[0]); }
            if w[1] > 0.0 { safe[1] = safe[1].max(origin[1] + w[1]); }
            if w[2] > 0.0 { safe[2] = safe[2].min(origin[2] - w[2]); }
            if w[3] > 0.0 { safe[3] = safe[3].min(origin[3] - w[3]); }
        }
        if let Some((mask, radii)) = pcold.rounded {
            let e = radii.map(crate::render::corner_extent);
            if ddx != 0.0 {
                safe[0] = safe[0].max(mask[0] + e[0].max(e[3]));
                safe[2] = safe[2].min(mask[2] - e[1].max(e[2]));
            }
            if ddy != 0.0 {
                safe[1] = safe[1].max(mask[1] + e[0].max(e[1]));
                safe[3] = safe[3].min(mask[3] - e[2].max(e[3]));
            }
        }
        let pad = 3.0;
        if ddy != 0.0 {
            let lo = safe[1] + pad;
            let hi = safe[3] - pad;
            if sr.bounds[1] < lo || sr.bounds[3] > hi
                || sr.bounds[1] + ddy_px.min(shift_y) < lo
                || sr.bounds[3] + ddy_px.max(shift_y) > hi
            {
                return None;
            }
        }
        if ddx != 0.0 {
            let lo = safe[0] + pad;
            let hi = safe[2] - pad;
            if sr.bounds[0] < lo || sr.bounds[2] > hi
                || sr.bounds[0] + ddx_px.min(shift_x) < lo
                || sr.bounds[2] + ddx_px.max(shift_x) > hi
            {
                return None;
            }
        }
        ShiftMode::Visible
    };

    Some(ShiftPlan {
        mode,
        ddx,
        ddy,
        ddx_px,
        ddy_px,
        shift_x,
        shift_y,
        outer_clip: pinh.clip,
        outer_paint_clip: vis_clip,
        outer_group_rect: pcold.group_rect,
        inh_base,
        inh_full,
        flags: sr.flags,
    })
}

#[inline(always)]
fn shift_instance(q: &mut Instance, plan: &ShiftPlan) {
    q.rect[0] += plan.shift_x;
    q.rect[1] += plan.shift_y;
    if (plan.flags & INNER_CLIP) != 0 {
        if plan.shift_x != 0.0 {
            if q.clip[0] != plan.outer_paint_clip[0] { q.clip[0] += plan.shift_x; }
            if q.clip[2] != plan.outer_paint_clip[2] { q.clip[2] += plan.shift_x; }
        }
        if plan.shift_y != 0.0 {
            if q.clip[1] != plan.outer_paint_clip[1] { q.clip[1] += plan.shift_y; }
            if q.clip[3] != plan.outer_paint_clip[3] { q.clip[3] += plan.shift_y; }
        }
    }
    let kind = q.params[1];
    if kind == KIND_SEGMENT || kind == KIND_TRIANGLE {
        q.radii[0] += plan.shift_x;
        q.radii[1] += plan.shift_y;
        q.radii[2] += plan.shift_x;
        q.radii[3] += plan.shift_y;
        if kind == KIND_TRIANGLE {
            q.uv[0] += plan.shift_x;
            q.uv[1] += plan.shift_y;
        }
    }
}

#[inline(always)]
fn shift_drawn(old: [f32; 4], plan: &ShiftPlan, s: f32, pow2: bool) -> [f32; 4] {
    let nx = if plan.ddx == 0.0 { old[0] } else if pow2 { old[0] + plan.ddx } else { grid(old[0] + plan.ddx, s) };
    let ny = if plan.ddy == 0.0 { old[1] } else if pow2 { old[1] + plan.ddy } else { grid(old[1] + plan.ddy, s) };
    [nx, ny, old[2], old[3]]
}

#[inline(always)]
fn shift_hit(rect: [f32; 4], plan: &ShiftPlan) -> [f32; 4] {
    [rect[0] + plan.ddx, rect[1] + plan.ddy, rect[2] + plan.ddx, rect[3] + plan.ddy]
}

/// Executes a scroll-translated subtree copy from `o..o+len` into `i..end`.
pub(super) fn copy_shifted(rt: &mut RuntimeCore, i: usize, o: usize, end: usize, plan: ShiftPlan, glyph_count: &mut usize) {
    let s = rt.env.scale;
    let pow2 = is_pow2_scale(s);
    let len = end - i;
    let k = &mut rt.paint.keep;
    k.open.pop();
    let r = k.prev[o];
    let range = |a: [u32; 2]| a[0] as usize..a[1] as usize;
    let at = [rt.instances.len(), rt.hit.hits.len(), rt.hit.overlay_hits.len(), k.pages.len()];
    let at_wheel = rt.hit.wheel_hits.len();
    let (at_help, at_drop) = (rt.hit.help_hits.len(), rt.hit.drop_hits.len());

    if plan.mode == ShiftMode::Visible {
        rt.instances.extend_from_slice(&k.instances[range(r.inst)]);
        for q in &mut rt.instances[at[0]..] {
            shift_instance(q, &plan);
        }
        if rt.track_marks {
            rt.marks.resize(at[0], 0);
            rt.marks.extend_from_slice(&k.marks[range(r.inst)]);
        }
        rt.hit.hits.extend(k.hits[range(r.hits)].iter().map(|&(id, rect)| (id, shift_hit(rect, &plan))));
        rt.hit.wheel_hits.extend(k.wheel[range(r.wheel)].iter().map(|&(id, rect)| (id, shift_hit(rect, &plan))));
        let seq = |sq: usize| sq + at[1] - r.hits[0] as usize;
        rt.hit.help_hits.extend(k.help[range(r.help)].iter().map(|&(id, rect, sq, l)| (id, shift_hit(rect, &plan), seq(sq), l)));
        rt.hit.drop_hits.extend(k.drop[range(r.drop)].iter().map(|&(id, rect, sq)| (id, shift_hit(rect, &plan), seq(sq))));
        let pages = range(r.pages);
        k.pages.extend_from_slice(&k.prev_pages[pages.clone()]);
        for &(page, texels) in &k.prev_pages[pages] {
            rt.text.touch_quad(page, texels);
        }
        *glyph_count += (r.glyphs[1] - r.glyphs[0]) as usize;
    }

    rt.paint.drawn_scale[i..end].copy_from_slice(&k.drawn_scale[o..o + len]);
    rt.paint.seen[i..end].copy_from_slice(&k.seen[o..o + len]);

    let has_clip = (plan.flags & INNER_CLIP) != 0;
    let has_hover = (plan.flags & INNER_HOVER) != 0;
    let has_sticky = rt.tree.has_sticky;
    let (mut last_old_cold, mut last_new_cold) = (0u32, 0u32);
    for d in 0..len {
        let idx = i + d;
        let old_idx = o + d;
        let new_drawn = shift_drawn(k.drawn[old_idx], &plan, s, pow2);
        rt.paint.drawn[idx] = new_drawn;

        let mut h = k.inh[old_idx];
        h.dx += plan.ddx;
        h.dy += plan.ddy;
        h.rigid[0] += plan.ddx;
        h.rigid[1] += plan.ddy;
        let old_cold = h.cold;
        if has_hover {
            let mut cold = if old_cold == 0 { InhCold::DEFAULT } else { k.inh_cold[old_cold as usize - 1] };
            let n = &rt.tree.nodes[idx];
            if n.vis.tracks_hover_paint() || n.vis_flags.hover_group() {
                cold.group_rect = shift_hit(cold.group_rect, &plan);
            } else if d == 0 {
                cold.group_rect = plan.outer_group_rect;
            } else {
                let p_cold = rt.paint.inh_scratch[n.parent.unwrap()].cold;
                cold.group_rect = if p_cold == 0 { inh::DEFAULT_GROUP_RECT } else { rt.paint.inh_cold[p_cold as usize - 1].group_rect };
            }
            let parent_cold = if d == 0 { 0 } else { rt.paint.inh_scratch[n.parent.unwrap()].cold };
            h.cold = if cold == InhCold::DEFAULT {
                0
            } else if parent_cold != 0 && rt.paint.inh_cold[parent_cold as usize - 1] == cold {
                parent_cold
            } else {
                let next_idx = rt.paint.inh_cold.len() as u32 + 1;
                rt.paint.inh_cold.push(cold);
                next_idx
            };
        } else if old_cold != 0 {
            h.cold = if old_cold == last_old_cold {
                last_new_cold
            } else if let Some(p) = rt.tree.nodes[idx].parent
                && p >= i
                && k.inh[o + (p - i)].cold == old_cold
            {
                rt.paint.inh_scratch[p].cold
            } else {
                let val = k.inh_cold[old_cold as usize - 1];
                let next_idx = rt.paint.inh_cold.len() as u32 + 1;
                rt.paint.inh_cold.push(val);
                last_old_cold = old_cold;
                last_new_cold = next_idx;
                next_idx
            };
        }
        if has_clip {
            if plan.mode == ShiftMode::Visible {
                if plan.shift_x != 0.0 {
                    if h.clip[0] != plan.outer_clip[0] { h.clip[0] += plan.shift_x; }
                    if h.clip[2] != plan.outer_clip[2] { h.clip[2] += plan.shift_x; }
                    if h.paint_clip[0] != plan.outer_paint_clip[0] { h.paint_clip[0] += plan.shift_x; }
                    if h.paint_clip[2] != plan.outer_paint_clip[2] { h.paint_clip[2] += plan.shift_x; }
                }
                if plan.shift_y != 0.0 {
                    if h.clip[1] != plan.outer_clip[1] { h.clip[1] += plan.shift_y; }
                    if h.clip[3] != plan.outer_clip[3] { h.clip[3] += plan.shift_y; }
                    if h.paint_clip[1] != plan.outer_paint_clip[1] { h.paint_clip[1] += plan.shift_y; }
                    if h.paint_clip[3] != plan.outer_paint_clip[3] { h.paint_clip[3] += plan.shift_y; }
                }
            } else {
                let n = &rt.tree.nodes[idx];
                let mut pc = if d == 0 { plan.outer_paint_clip } else { rt.paint.inh_scratch[n.parent.unwrap()].paint_clip };
                let lc = if d == 0 { plan.outer_clip } else { rt.paint.inh_scratch[n.parent.unwrap()].clip };
                if n.vis.clip.is_some() {
                    let nr = [new_drawn[0] * s, new_drawn[1] * s, (new_drawn[0] + new_drawn[2]) * s, (new_drawn[1] + new_drawn[3]) * s];
                    pc = intersect(nr, pc);
                }
                h.paint_clip = pc;
                h.clip = intersect(lc, pc);
            }
        }
        rt.paint.inh_scratch[idx] = h;

        if has_sticky {
            let sf = k.sticky_frame[old_idx];
            rt.scroll.sticky_frame[idx] = [sf[0], sf[1], sf[2] + plan.ddy, sf[3] + plan.ddy, sf[4], sf[5], sf[6] + plan.ddx, sf[7] + plan.ddx];
        }
    }

    let shift_range = |a: [u32; 2], from: u32, to: usize| a.map(|v| v - from + to as u32);
    let glyphs = *glyph_count as u32 - (r.glyphs[1] - r.glyphs[0]);
    for d in 0..len {
        let idx = i + d;
        let p = k.prev[o + d];
        let mut ps = k.prev_shift[o + d];
        ps.origin = [ps.origin[0] + plan.ddx, ps.origin[1] + plan.ddy, ps.origin[2] + plan.ddx, ps.origin[3] + plan.ddy];
        ps.bounds = [ps.bounds[0] + plan.ddx_px, ps.bounds[1] + plan.ddy_px, ps.bounds[2] + plan.ddx_px, ps.bounds[3] + plan.ddy_px];
        ps.cull = [ps.cull[0] + plan.ddx_px, ps.cull[1] + plan.ddy_px, ps.cull[2] + plan.ddx_px, ps.cull[3] + plan.ddy_px];
        let new_inh = if d == 0 {
            ps.inh_base = plan.inh_base;
            plan.inh_full
        } else if !has_clip && !has_hover {
            inh_key_from_base(ps.inh_base, ps.origin)
        } else {
            let parent_idx = rt.tree.nodes[idx].parent.unwrap();
            let mut cpinh = rt.paint.inh_scratch[parent_idx];
            let mut cp_cold = if cpinh.cold == 0 { InhCold::DEFAULT } else { rt.paint.inh_cold[cpinh.cold as usize - 1] };
            cp_cold.first_fade = 1.0;
            let is_default_cold = cp_cold == InhCold::DEFAULT;
            let row_dy = rt.paint.row_dy[idx];
            cpinh.dy += row_dy;
            cpinh.grow_rigid(row_dy);
            let (full, base) = inh_keys_for(&rt.tree.nodes, has_sticky, &rt.scroll.sticky_frame, idx, &cpinh, &cp_cold, is_default_cold);
            ps.inh_base = base;
            full
        };
        k.shift[idx] = ps;
        k.recs[idx] = PaintRec {
            inh: new_inh,
            end: p.end - o as u32 + i as u32,
            inst: shift_range(p.inst, r.inst[0], at[0]),
            hits: shift_range(p.hits, r.hits[0], at[1]),
            wheel: shift_range(p.wheel, r.wheel[0], at_wheel),
            overlay: shift_range(p.overlay, r.overlay[0], at[2]),
            help: shift_range(p.help, r.help[0], at_help),
            drop: shift_range(p.drop, r.drop[0], at_drop),
            pages: shift_range(p.pages, r.pages[0], at[3]),
            glyphs: shift_range(p.glyphs, r.glyphs[0], glyphs as usize),
            ..p
        };
    }
    k.kept += len;
}

/// Debug builds: verifies that freshly painting subtree `r` matched what `copy_shifted` would
/// have produced from `o`.
pub(super) fn check_shifted(rt: &mut RuntimeCore, r: usize, o: usize, plan: ShiftPlan) {
    let s = rt.env.scale;
    let pow2 = is_pow2_scale(s);
    let k = &rt.paint.keep;
    let (now, was) = (k.recs[r], k.prev[o]);
    let range = |a: [u32; 2]| a[0] as usize..a[1] as usize;
    let bytes = bytemuck::cast_slice::<Instance, u8>;
    let mut differ = Vec::new();

    if plan.mode == ShiftMode::Offscreen {
        if now.inst[0] != now.inst[1] { differ.push("instances"); }
        if now.hits[0] != now.hits[1] { differ.push("hits"); }
        if now.wheel[0] != now.wheel[1] { differ.push("wheel hits"); }
        if now.help[0] != now.help[1] { differ.push("help hits"); }
        if now.drop[0] != now.drop[1] { differ.push("drop hits"); }
    } else {
        // Each instance against its shifted twin, one at a time: a list of them would be a heap
        // allocation in every scroll frame of a debug build (`tests/alloc.rs`).
        let (painted, kept) = (&rt.instances[range(now.inst)], &k.instances[range(was.inst)]);
        let same = painted.len() == kept.len() && painted.iter().zip(kept).all(|(p, q)| {
            let mut expected = *q;
            shift_instance(&mut expected, &plan);
            bytes(std::slice::from_ref(p)) == bytes(std::slice::from_ref(&expected))
        });
        if !same { differ.push("instances"); }
        let near_rect = |a: [f32; 4], b: [f32; 4]| {
            if pow2 { a == b } else { (0..4).all(|e| (a[e] - b[e]).abs() <= 1e-4) }
        };
        let (ah, bh) = (&rt.hit.hits[range(now.hits)], &k.hits[range(was.hits)]);
        if ah.len() != bh.len() || ah.iter().zip(bh).any(|(&(id1, r1), &(id2, r2))| id1 != id2 || !near_rect(r1, shift_hit(r2, &plan))) {
            differ.push("hits");
        }
        let (aw, bw) = (&rt.hit.wheel_hits[range(now.wheel)], &k.wheel[range(was.wheel)]);
        if aw.len() != bw.len() || aw.iter().zip(bw).any(|(&(id1, r1), &(id2, r2))| id1 != id2 || !near_rect(r1, shift_hit(r2, &plan))) {
            differ.push("wheel hits");
        }
        let (ahe, bhe) = (&rt.hit.help_hits[range(now.help)], &k.help[range(was.help)]);
        if ahe.len() != bhe.len() || ahe.iter().zip(bhe).any(|(&(id1, r1, s1, l1), &(id2, r2, s2, l2))| {
            id1 != id2 || !near_rect(r1, shift_hit(r2, &plan)) || s1 - now.hits[0] as usize != s2 - was.hits[0] as usize || l1 != l2
        }) {
            differ.push("help hits");
        }
        let (ad, bd) = (&rt.hit.drop_hits[range(now.drop)], &k.drop[range(was.drop)]);
        if ad.len() != bd.len() || ad.iter().zip(bd).any(|(&(id1, r1, s1), &(id2, r2, s2))| {
            id1 != id2 || !near_rect(r1, shift_hit(r2, &plan)) || s1 - now.hits[0] as usize != s2 - was.hits[0] as usize
        }) {
            differ.push("drop hits");
        }
    }
    if rt.track_marks && rt.marks.get(range(now.inst)) != k.marks.get(range(was.inst)) { differ.push("marks"); }
    let len = k.end[r] as usize - r;
    let drawn_differ = (0..len).any(|d| {
        let a = rt.paint.drawn[r + d];
        let b = shift_drawn(k.drawn[o + d], &plan, s, pow2);
        if pow2 { a != b } else { a[0] != b[0] || a[1] != b[1] || (a[2] - b[2]).abs() > 1e-4 || (a[3] - b[3]).abs() > 1e-4 }
    });
    if drawn_differ || rt.paint.drawn_scale[r..r + len] != k.drawn_scale[o..o + len] || rt.paint.seen[r..r + len] != k.seen[o..o + len] {
        differ.push("drawn rects");
    }
    if rt.tree.has_sticky {
        let sticky_differ = (0..len).any(|d| {
            let a = rt.scroll.sticky_frame[r + d];
            let sf = k.sticky_frame[o + d];
            let b = [sf[0], sf[1], sf[2] + plan.ddy, sf[3] + plan.ddy, sf[4], sf[5], sf[6] + plan.ddx, sf[7] + plan.ddx];
            if pow2 { a != b } else { (0..8).any(|e| a[e] != b[e] && (a[e] - b[e]).abs() > 1e-4) }
        });
        if sticky_differ { differ.push("sticky frames"); }
    }
    if rt.hit.overlay_hits[range(now.overlay)] != k.overlay[range(was.overlay)] { differ.push("overlay hits"); }
    if now.glyphs[1] - now.glyphs[0] != was.glyphs[1] - was.glyphs[0] { differ.push("glyph count"); }
    if k.pages[range(now.pages)] != k.prev_pages[range(was.pages)] { differ.push("atlas pages"); }
    if !differ.is_empty() {
        let path = rt.tree.nodes[r].path.as_str();
        rt.paint.keep.record_mismatch(true, path, &differ);
    }
}

#[cfg(test)]
#[path = "shift_tests.rs"]
mod tests;
