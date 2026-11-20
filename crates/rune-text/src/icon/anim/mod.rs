//! Monochrome SMIL SVG icon animation evaluator and double-buffered glyph-atlas tile cache.
//!
//! Only monochrome (`currentColor`) icons animate. Multi-colour icons stay static via
//! [`super::color`] (avoiding per-frame `usvg` parsing/rendering on the UI thread).
//!
//! Each live animated icon uses a pair of `(px + 2) × (px + 2)` slots in the glyph atlas
//! (`Atlas::put_bordered` / `Atlas::write_bordered`) and flips between them **only** when the
//! quantized coverage bytes actually change. That keeps colour tinting on the GPU (`KIND_GLYPH`),
//! avoids `ImageAtlas` / `Plan::All("image")` full-window redraws on Windows, and skips flat holds
//! via `AnimState::WaitingUntil`.

pub(crate) mod parse;

use std::collections::HashMap;
use std::hash::BuildHasherDefault;

use swash::zeno::{Cap, Fill, Join, Point, Transform};

use super::area;
use super::convex;
use super::{Facing, Icon, then, union};
use crate::atlas::{ATLAS_SIZE, Atlas, AtlasGlyph, GlyphQuad, InPlaceAtlas};
use parse::{
    AttrKind, BaseGeom, CalcMode, CompiledAnim, KeyVal, MAX_COORD, MAX_LAYERS, MAX_SCALE, MAX_SKEW_DEG, MAX_STROKE_WIDTH, Op, PaintProps, Track, TransformKind, rotate_transform,
};
pub(crate) use parse::{CompileOutcome, compile, may_be_animated};

/// Hard cap on distinct compiled animation plans retained per `TextSystem`.
const MAX_CACHED_PLANS: usize = 256;
/// CPU byte budget for cached last-frame coverage buffers (`last_cov`) before frame-boundary eviction.
const COVERAGE_BUDGET: usize = 2 * 1024 * 1024;
/// Maximum cadence for animated monochrome SVG icons (matches Rune's `Cycle` spinner at 30 Hz).
/// Combined with next-pixel-change probing so wakes fire only when quantized coverage bytes differ.
const ICON_ANIM_STEP: f64 = 1.0 / 30.0;
/// Maximum forward probes per frame when scanning for the next quantized pixel change (12 × 33.3 ms = 0.4 s).
const MAX_PIXEL_PROBES: usize = 12;

/// User-space composed transform safety bound (enforced inside `parse_transform_bounded` and
/// `resolve_transform` before multiplying by device scale `size / view`).
pub(super) fn local_ctm_is_safe(m: Transform) -> bool {
    let vals = [m.xx, m.xy, m.yx, m.yy, m.x, m.y];
    vals.iter().all(|v| v.is_finite())
        && m.xx.abs().max(m.xy.abs()).max(m.yx.abs()).max(m.yy.abs()) <= MAX_SCALE
        && m.x.abs().max(m.y.abs()) <= MAX_COORD
}

type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<rune_motion::Fx>>;

/// Motion state of an animated icon at the sampled time.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnimState {
    /// At least one track is actively interpolating right now (internal; `frame_quad` paces
    /// moving tracks into [`Self::WaitingUntil`] at the next quantized pixel change).
    Moving,
    /// Sleep and wake once at the returned runtime timestamp (seconds) — either for the next
    /// quantized pixel change during active motion (at <= 30 Hz) or after a flat hold / `begin`
    /// wait / discrete plateau / timed `<set>`.
    WaitingUntil(f64),
    /// All finite tracks have finished (or `<set>` without `dur` has already applied): stop
    /// scheduling frames and re-enable `El::cache` / `PaintKeep`.
    Finished,
}

/// Output of [`crate::TextSystem::animated_icon_quad`].
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct AnimatedQuad {
    pub quad: GlyphQuad,
    pub state: AnimState,
    /// True when this call wrote/flipped the atlas slot; false when quantized pixels matched the
    /// already-displayed slot (allowing `loop_step` / `is_unchanged` to skip presenting).
    pub changed: bool,
}

/// Result of rendering an icon's static `t = 0` resting frame when SMIL tags are present.
pub(crate) enum RestCoverage {
    /// Compiled animated SVG rendered at `t = 0.0`.
    Rendered(Vec<u8>),
    /// Well-formed SVG within all caps with no supported SMIL tracks (safe for static parser).
    StaticFallback,
    /// Hostile/cap-exceeding or malformed SVG (caller must return empty coverage).
    Rejected,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SlotKey {
    icon_ptr: usize,
    px_packed: u32,
    /// `0` for indefinite lockstep icons (shared across all instances); node id for finite icons.
    scope: u64,
}

struct SlotEntry {
    slots: [Option<(AtlasGlyph, u64)>; 2],
    cur: u8,
    /// `Atlas::upload_gen` at the time `slots[cur]` was last written, so missed drawables are
    /// detected per-slot without horizontal shelf aliasing.
    last_write_gen: u64,
    last_cov: Vec<u8>,
    last_t: f64,
    last_sample_t: f64,
    last_state: AnimState,
    used: bool,
}

impl SlotEntry {
    #[inline]
    fn pending_upload(&self, upload_gen: u64) -> bool {
        upload_gen > 0 && self.last_write_gen == upload_gen
    }

    #[inline]
    fn can_reuse(&self, t: f64, sample_t: f64) -> bool {
        let same_ts = (self.last_sample_t - sample_t).abs() <= 1e-9;
        let still_waiting = sample_t >= self.last_sample_t
            && t >= self.last_t
            && match self.last_state {
                AnimState::Finished => true,
                AnimState::WaitingUntil(wake) => t + 1e-6 < wake,
                AnimState::Moving => false,
            };
        same_ts || still_waiting
    }
}

#[derive(Default)]
pub(crate) struct AnimatedIcons {
    plans: FxMap<usize, CompiledAnim>,
    slots: FxMap<SlotKey, SlotEntry>,
    cov_bytes: usize,
    /// Reusable per-frame scratch buffers (zero heap allocation in steady state).
    scratch_cov: Vec<u8>,
    scratch_probe_cov: Vec<u8>,
    scratch_buf: Vec<u8>,
    scratch_layers: Vec<(Vec<u8>, f32)>,
    scratch_overrides: Vec<NodeOverrides>,
    scratch_scopes: Vec<RenderScope>,
    has_any: bool,
}

impl AnimatedIcons {
    #[inline]
    pub(crate) fn has_any(&self) -> bool {
        self.has_any
    }

    pub(crate) fn begin_frame(&mut self) {
        if !self.has_any {
            return;
        }
        if self.cov_bytes > COVERAGE_BUDGET {
            self.slots.retain(|_, e| e.used);
            self.cov_bytes = self.slots.values().map(|e| e.last_cov.len()).sum();
        }
        for e in self.slots.values_mut() {
            e.used = false;
        }
    }

    /// Registers a compiled monochrome animation for `icon_ptr`, returning `true` if accepted.
    pub(crate) fn insert_plan(&mut self, icon_ptr: usize, plan: CompiledAnim) -> bool {
        if self.plans.len() >= MAX_CACHED_PLANS && !self.plans.contains_key(&icon_ptr) {
            return false;
        }
        self.plans.insert(icon_ptr, plan);
        self.has_any = true;
        true
    }

    #[inline]
    pub(crate) fn is_finite(&self, icon_ptr: usize) -> bool {
        self.plans.get(&icon_ptr).is_some_and(|p| !p.has_indefinite)
    }

    /// Whether the animation for `icon_ptr` is still live (moving or waiting for a future segment)
    /// at runtime time `t` (with `mount_t` used for finite-only icons).
    pub(crate) fn is_live_at(&self, icon_ptr: usize, t: f64, mount_t: f64) -> bool {
        let Some(plan) = self.plans.get(&icon_ptr) else { return false };
        let sample_t = if plan.has_indefinite { t } else { (t - mount_t).max(0.0) };
        plan.state_at(sample_t, t) != AnimState::Finished
    }

    /// Renders `icon_ptr`'s `t = 0.0` resting frame (for reduced motion / static fallback),
    /// distinguishing hostile-cap rejection from benign SVGs with unsupported SMIL attributes.
    pub(crate) fn render_rest_coverage(icon: &Icon, px: u32, size: f32, origin: Point) -> RestCoverage {
        let plan = match compile(icon.body, icon.view) {
            CompileOutcome::Animated(plan) => plan,
            CompileOutcome::StaticOnly => return RestCoverage::StaticFallback,
            CompileOutcome::Rejected => return RestCoverage::Rejected,
        };
        let Some(len) = (px as usize).checked_mul(px as usize) else {
            return RestCoverage::Rejected;
        };
        let mut cov = vec![0u8; len];
        let mut buf = vec![0u8; len];
        let mut layers = Vec::new();
        let mut ov = Vec::new();
        let mut scopes = Vec::with_capacity(parse::MAX_DEPTH);
        plan.render_into(px, size, origin, 0.0, 0.0, &mut cov, &mut buf, &mut layers, &mut ov, &mut scopes);
        RestCoverage::Rendered(cov)
    }

    /// True iff `icon_ptr` already has both double-buffered atlas tiles resident on non-evicted
    /// pages, so `loop_step` can update it strictly via `write_bordered` without ever calling
    /// `Atlas::put_bordered` or evicting a glyph page.
    pub(crate) fn slots_ready(
        &self,
        atlas: &Atlas,
        icon_ptr: usize,
        (px, size): (u32, f32),
        facing: Facing,
        scope_id: u64,
    ) -> bool {
        if px == 0 || px + 4 > ATLAS_SIZE {
            return false;
        }
        let Some(plan) = self.plans.get(&icon_ptr) else { return false };
        let key = slot_key(icon_ptr, px, size, facing, plan.has_indefinite, scope_id);
        let Some(entry) = self.slots.get(&key) else { return false };
        entry.slots.iter().all(|slot| {
            slot.is_some_and(|(s, epoch)| s.w == px + 2 && s.h == px + 2 && atlas.page_gen(s.page) == epoch)
        })
    }

    /// Evaluates `icon_ptr`'s compiled AST at `(t, sample_t)` into `self.scratch_cov` and paces
    /// `Moving` into `WaitingUntil` at the next quantized pixel change (at <= 30 Hz). Pure CPU
    /// evaluation; never touches the glyph atlas.
    fn eval_into_scratch(
        &mut self,
        icon_ptr: usize,
        px: u32,
        snapped_size: f32,
        facing: Facing,
        t: f64,
        sample_t: f64,
    ) -> Option<AnimState> {
        let plan = self.plans.get(&icon_ptr)?;
        let needed = (px as usize).checked_mul(px as usize)?;
        self.scratch_cov.resize(needed, 0);
        self.scratch_buf.resize(needed, 0);
        let raw_state = plan.render_into(
            px,
            snapped_size,
            Point::new(0.0, 0.0),
            sample_t,
            t,
            &mut self.scratch_cov,
            &mut self.scratch_buf,
            &mut self.scratch_layers,
            &mut self.scratch_overrides,
            &mut self.scratch_scopes,
        );
        if facing == Facing::Mirrored {
            self.scratch_cov.chunks_exact_mut(px as usize).for_each(<[u8]>::reverse);
        }

        // Pace `Moving` through `WaitingUntil` at <= 30 Hz, probing ahead so the next wake lands
        // on a timestamp where quantized coverage bytes actually differ from `scratch_cov`.
        let state = match raw_state {
            AnimState::Moving => {
                self.scratch_probe_cov.resize(needed, 0);
                let mut chosen = AnimState::WaitingUntil(t + MAX_PIXEL_PROBES as f64 * ICON_ANIM_STEP);
                for k in 1..=MAX_PIXEL_PROBES {
                    let dt = k as f64 * ICON_ANIM_STEP;
                    let pt = sample_t + dt;
                    let rt = t + dt;
                    let ps = plan.render_into(
                        px,
                        snapped_size,
                        Point::new(0.0, 0.0),
                        pt,
                        rt,
                        &mut self.scratch_probe_cov,
                        &mut self.scratch_buf,
                        &mut self.scratch_layers,
                        &mut self.scratch_overrides,
                        &mut self.scratch_scopes,
                    );
                    if facing == Facing::Mirrored {
                        self.scratch_probe_cov.chunks_exact_mut(px as usize).for_each(<[u8]>::reverse);
                    }
                    if self.scratch_probe_cov.as_slice() != self.scratch_cov.as_slice() {
                        chosen = AnimState::WaitingUntil(rt);
                        break;
                    }
                    match ps {
                        AnimState::Finished => {
                            chosen = AnimState::Finished;
                            break;
                        }
                        AnimState::WaitingUntil(w) => {
                            chosen = AnimState::WaitingUntil(w);
                            break;
                        }
                        AnimState::Moving => {}
                    }
                }
                chosen
            }
            other => other,
        };
        Some(state)
    }

    /// Evaluates `icon_ptr` at runtime time `t` (using `mount_t` when finite-only) during a full
    /// paint, allocating its double-buffered glyph-atlas tile pair when needed and flipping
    /// between them ONLY when the quantized coverage bytes changed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame_quad(
        &mut self,
        atlas: &mut Atlas,
        icon_ptr: usize,
        (px, size): (u32, f32),
        origin: [f32; 2],
        facing: Facing,
        t: f64,
        mount_t: f64,
        scope_id: u64,
    ) -> Option<AnimatedQuad> {
        if px == 0 || px + 4 > ATLAS_SIZE {
            return None;
        }
        let is_indefinite = self.plans.get(&icon_ptr)?.has_indefinite;
        let sample_t = if is_indefinite { t } else { (t - mount_t).max(0.0) };
        let short = ((px as f32 - size) * 64.0).round().clamp(0.0, 63.0) as u32;
        let snapped_size = px as f32 - short as f32 / 64.0;
        let key = slot_key(icon_ptr, px, size, facing, is_indefinite, scope_id);

        // Same timestamp, active hold/paced interval, or finished finite animation: reuse the
        // current atlas slot with zero re-rasterization or probing.
        if let Some(entry) = self.slots.get_mut(&key)
            && let Some((slot, epoch)) = entry.slots[entry.cur as usize]
            && atlas.page_gen(slot.page) == epoch
            && entry.can_reuse(t, sample_t)
        {
            entry.used = true;
            touch_entry_slots(atlas, &entry.slots);
            let pending = entry.pending_upload(atlas.upload_gen);
            return Some(AnimatedQuad { quad: make_quad(slot, origin, px), state: entry.last_state, changed: pending });
        }

        let state = self.eval_into_scratch(icon_ptr, px, snapped_size, facing, t, sample_t)?;

        if let Some(entry) = self.slots.get_mut(&key) {
            let cur_slot = entry.slots[entry.cur as usize];
            let valid_cur = cur_slot.filter(|&(s, g)| atlas.page_gen(s.page) == g);
            // If pixels are identical to what the active slot already holds, skip atlas write and
            // skip UV flip (zero upload, zero GPU/DWM damage).
            if let Some((slot, _)) = valid_cur
                && entry.last_cov.as_slice() == self.scratch_cov.as_slice()
            {
                entry.used = true;
                entry.last_t = t;
                entry.last_sample_t = sample_t;
                entry.last_state = state;
                touch_entry_slots(atlas, &entry.slots);
                let pending = entry.pending_upload(atlas.upload_gen);
                return Some(AnimatedQuad { quad: make_quad(slot, origin, px), state, changed: pending });
            }
            // Write into the OTHER slot of the double-buffered pair so UV flips and Windows
            // `Damage::plan` / `arm_loop` sees ONLY this icon's `KIND_GLYPH` instance change.
            // If `pending_upload`, the previous frame missed its drawable before uploading:
            // overwrite `entry.cur` in place so we never overwrite the on-screen slot or flip UV
            // back to match the last presented instance, with zero horizontal shelf aliasing.
            let next_idx = match valid_cur {
                Some(_) if entry.pending_upload(atlas.upload_gen) => entry.cur as usize,
                Some(_) => 1 - entry.cur as usize,
                None => 0,
            };
            let placed = match entry.slots[next_idx] {
                Some((slot, epoch)) if atlas.write_bordered(slot, epoch, px, &self.scratch_cov) => Some((slot, epoch)),
                _ => {
                    let fresh = atlas.put_bordered(px, &self.scratch_cov)?;
                    entry.slots[next_idx] = Some(fresh);
                    Some(fresh)
                }
            }?;
            if let Some((other, epoch)) = entry.slots[1 - next_idx]
                && atlas.page_gen(other.page) == epoch
            {
                atlas.touch(other.page, other.w * other.h);
            }
            entry.used = true;
            entry.last_t = t;
            entry.last_sample_t = sample_t;
            entry.last_state = state;
            entry.cur = next_idx as u8;
            entry.last_write_gen = atlas.upload_gen;
            if entry.last_cov.len() != self.scratch_cov.len() {
                self.cov_bytes = self.cov_bytes.saturating_sub(entry.last_cov.len()) + self.scratch_cov.len();
                entry.last_cov.resize(self.scratch_cov.len(), 0);
            }
            entry.last_cov.copy_from_slice(&self.scratch_cov);
            return Some(AnimatedQuad { quad: make_quad(placed.0, origin, px), state, changed: true });
        }

        // First allocation for this (icon, size, facing, scope): pre-allocate both double-buffer
        // slots during full paint so steady `loop_step` ticks never allocate or evict any live
        // glyph page. Only commit budget after placement succeeds.
        let first = atlas.put_bordered(px, &self.scratch_cov)?;
        let (slots, active) = match atlas.put_bordered(px, &self.scratch_cov) {
            Some(second) if atlas.page_gen(first.0.page) == first.1 => ([Some(first), Some(second)], first),
            Some(second) => ([Some(second), None], second),
            None => ([Some(first), None], first),
        };
        let last_cov = self.scratch_cov.clone();
        self.cov_bytes += last_cov.len();
        self.slots.insert(
            key,
            SlotEntry {
                slots,
                cur: 0,
                last_write_gen: atlas.upload_gen,
                last_cov,
                last_t: t,
                last_sample_t: sample_t,
                last_state: state,
                used: true,
            },
        );
        Some(AnimatedQuad { quad: make_quad(active.0, origin, px), state, changed: true })
    }

    /// Loop-step variant of [`Self::frame_quad`] taking [`InPlaceAtlas`], which by construction
    /// cannot allocate in or evict from the glyph atlas or inflate `Shelf::live_texels`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame_quad_in_place(
        &mut self,
        mut atlas: InPlaceAtlas<'_>,
        icon_ptr: usize,
        (px, size): (u32, f32),
        origin: [f32; 2],
        facing: Facing,
        t: f64,
        mount_t: f64,
        scope_id: u64,
    ) -> Option<AnimatedQuad> {
        if px == 0 || px + 4 > ATLAS_SIZE {
            return None;
        }
        let is_indefinite = self.plans.get(&icon_ptr)?.has_indefinite;
        let sample_t = if is_indefinite { t } else { (t - mount_t).max(0.0) };
        let short = ((px as f32 - size) * 64.0).round().clamp(0.0, 63.0) as u32;
        let snapped_size = px as f32 - short as f32 / 64.0;
        let key = slot_key(icon_ptr, px, size, facing, is_indefinite, scope_id);

        if let Some(entry) = self.slots.get_mut(&key)
            && let Some((slot, epoch)) = entry.slots[entry.cur as usize]
            && atlas.page_gen(slot.page) == epoch
            && entry.can_reuse(t, sample_t)
        {
            entry.used = true;
            let pending = entry.pending_upload(atlas.upload_gen());
            return Some(AnimatedQuad { quad: make_quad(slot, origin, px), state: entry.last_state, changed: pending });
        }

        if !self.slots.contains_key(&key) {
            return None;
        }
        let state = self.eval_into_scratch(icon_ptr, px, snapped_size, facing, t, sample_t)?;
        let entry = self.slots.get_mut(&key)?;
        let (cur_slot, _) = entry.slots[entry.cur as usize].filter(|&(s, g)| atlas.page_gen(s.page) == g)?;
        let upload_gen = atlas.upload_gen();
        if entry.last_cov.as_slice() == self.scratch_cov.as_slice() {
            entry.used = true;
            entry.last_t = t;
            entry.last_sample_t = sample_t;
            entry.last_state = state;
            let pending = entry.pending_upload(upload_gen);
            return Some(AnimatedQuad { quad: make_quad(cur_slot, origin, px), state, changed: pending });
        }
        let next_idx = if entry.pending_upload(upload_gen) { entry.cur as usize } else { 1 - entry.cur as usize };
        let (slot, epoch) = entry.slots[next_idx]?;
        if !atlas.write_bordered(slot, epoch, px, &self.scratch_cov) {
            return None;
        }
        entry.used = true;
        entry.last_t = t;
        entry.last_sample_t = sample_t;
        entry.last_state = state;
        entry.cur = next_idx as u8;
        entry.last_write_gen = upload_gen;
        if entry.last_cov.len() != self.scratch_cov.len() {
            self.cov_bytes = self.cov_bytes.saturating_sub(entry.last_cov.len()) + self.scratch_cov.len();
            entry.last_cov.resize(self.scratch_cov.len(), 0);
        }
        entry.last_cov.copy_from_slice(&self.scratch_cov);
        Some(AnimatedQuad { quad: make_quad(slot, origin, px), state, changed: true })
    }
}

fn touch_entry_slots(atlas: &mut Atlas, slots: &[Option<(AtlasGlyph, u64)>; 2]) {
    for &(slot, epoch) in slots.iter().flatten() {
        if atlas.page_gen(slot.page) == epoch {
            atlas.touch(slot.page, slot.w * slot.h);
        }
    }
}

fn slot_key(icon_ptr: usize, px: u32, size: f32, facing: Facing, is_indefinite: bool, scope_id: u64) -> SlotKey {
    let short = ((px as f32 - size) * 64.0).round().clamp(0.0, 63.0) as u32;
    let px_packed = px | (short << 24) | (u32::from(facing == Facing::Mirrored) << 31);
    let scope = if is_indefinite { 0 } else { scope_id };
    SlotKey { icon_ptr, px_packed, scope }
}

fn make_quad(a: AtlasGlyph, origin: [f32; 2], px: u32) -> GlyphQuad {
    GlyphQuad {
        rect: [origin[0], origin[1], px as f32, px as f32],
        uv: [(a.x + 1) as f32, (a.page as u32 * ATLAS_SIZE + a.y + 1) as f32],
        ink: 0,
        byte: 0,
        color: 0,
        outline: None,
        lcd: false,
    }
}

/// Evaluated per-node animated overrides for one frame (reused in scratch vec, zero per-frame alloc).
#[derive(Default)]
struct NodeOverrides {
    /// Numeric attribute overrides: `(AttrKind, value, additive_sum)`.
    nums: [Option<(f32, bool)>; 17],
    /// Transform tracks applied in document order: `(Transform, additive_sum)`.
    transforms: Vec<(Transform, bool)>,
    has_any: bool,
}

impl NodeOverrides {
    fn clear(&mut self) {
        if self.has_any {
            self.nums = [None; 17];
            self.transforms.clear();
            self.has_any = false;
        }
    }

    fn apply_num(&mut self, attr: AttrKind, val: f32, sum: bool) {
        let Some(slot) = attr_slot(attr) else { return };
        self.has_any = true;
        self.nums[slot] = match (self.nums[slot], sum) {
            (Some((prev, prev_sum)), true) => Some((prev + val, prev_sum)),
            _ => Some((val, sum)),
        };
    }

    fn resolve(&self, attr: AttrKind, base: f32) -> f32 {
        let Some(slot) = attr_slot(attr) else { return base };
        match self.nums[slot] {
            Some((v, true)) => base + v,
            Some((v, false)) => v,
            None => base,
        }
    }

    fn resolve_opt(&self, attr: AttrKind, base: Option<f32>) -> Option<f32> {
        let Some(slot) = attr_slot(attr) else { return base };
        match self.nums[slot] {
            Some((v, true)) => Some(base.unwrap_or(0.0) + v),
            Some((v, false)) => Some(v),
            None => base,
        }
    }

    fn resolve_transform(&self, base: Transform) -> Option<Transform> {
        if self.transforms.is_empty() {
            return Some(base);
        }
        let mut cur = base;
        for &(tr, sum) in &self.transforms {
            cur = if sum { then(tr, cur) } else { tr };
            if !local_ctm_is_safe(cur) {
                return None;
            }
        }
        Some(cur)
    }
}

fn attr_slot(attr: AttrKind) -> Option<usize> {
    Some(match attr {
        AttrKind::R => 0,
        AttrKind::Cx => 1,
        AttrKind::Cy => 2,
        AttrKind::Rx => 3,
        AttrKind::Ry => 4,
        AttrKind::X => 5,
        AttrKind::Y => 6,
        AttrKind::Width => 7,
        AttrKind::Height => 8,
        AttrKind::X1 => 9,
        AttrKind::Y1 => 10,
        AttrKind::X2 => 11,
        AttrKind::Y2 => 12,
        AttrKind::Opacity => 13,
        AttrKind::FillOpacity => 14,
        AttrKind::StrokeOpacity => 15,
        AttrKind::StrokeWidth => 16,
        AttrKind::Transform(_) => return None,
    })
}

impl CompiledAnim {
    fn state_at(&self, sample_t: f64, runtime_t: f64) -> AnimState {
        let mut state = AnimState::Finished;
        for tr in &self.tracks {
            let s = tr.sample(sample_t, runtime_t).1;
            state = merge_state(state, s);
        }
        state
    }

    #[allow(clippy::too_many_arguments)]
    fn render_into(
        &self,
        px: u32,
        size: f32,
        origin: Point,
        sample_t: f64,
        runtime_t: f64,
        acc: &mut [u8],
        buf: &mut [u8],
        layers: &mut Vec<(Vec<u8>, f32)>,
        overrides: &mut Vec<NodeOverrides>,
        scope_stack: &mut Vec<RenderScope>,
    ) -> AnimState {
        acc.fill(0);
        layers.clear();
        scope_stack.clear();
        if overrides.len() < self.node_count {
            overrides.resize_with(self.node_count, NodeOverrides::default);
        }
        for ov in overrides.iter_mut().take(self.node_count) {
            ov.clear();
        }

        let mut combined_state = AnimState::Finished;
        for tr in &self.tracks {
            let (val, st) = tr.sample(sample_t, runtime_t);
            combined_state = merge_state(combined_state, st);
            if let Some(kv) = val
                && let Some(ov) = overrides.get_mut(tr.target as usize)
            {
                match tr.attr {
                    AttrKind::Transform(tk) => {
                        ov.has_any = true;
                        ov.transforms.push((key_val_to_transform(tk, kv), tr.additive_sum));
                    }
                    other => ov.apply_num(other, kv.v[0], tr.additive_sum),
                }
            }
        }

        let s = size / self.view;
        let root_paint = ResolvedPaint::default();
        let root_ctm = Transform::scale(s, s);

        for op in &self.ops {
            match op {
                Op::PushGroup { node_id, transform, opacity, display_none, paint, .. } => {
                    let up_ctm = scope_stack.last().map_or(root_ctm, |sc| sc.ctm);
                    let up_paints = scope_stack.last().is_none_or(|sc| sc.paints);
                    let up_paint = scope_stack.last().map_or(root_paint, |sc| sc.paint);
                    let ov = &overrides[*node_id as usize];
                    let (ctm, ctm_ok) = match ov.resolve_transform(*transform) {
                        Some(local_ctm) => {
                            let composed = then(local_ctm, up_ctm);
                            (composed, super::ctm_is_safe(composed))
                        }
                        None => (up_ctm, false),
                    };
                    let alpha = ov.resolve(AttrKind::Opacity, *opacity).clamp(0.0, 1.0);
                    let paints = up_paints && ctm_ok && !*display_none && alpha > 0.0;
                    let next_paint = up_paint.inherit(*paint, ov);
                    let layer = paints && alpha < 1.0 && layers.len() < MAX_LAYERS;
                    if layer {
                        layers.push((vec![0u8; acc.len()], alpha));
                    }
                    scope_stack.push(RenderScope { ctm, paints, paint: next_paint, layer });
                }
                Op::PopGroup => {
                    if scope_stack.pop().is_some_and(|sc| sc.layer)
                        && let Some((top, alpha)) = layers.pop()
                    {
                        union(layers.last_mut().map_or(acc, |l| &mut l.0), &top, alpha);
                    }
                }
                Op::DrawShape { node_id, geom, transform, opacity, display_none, paint } => {
                    let up_ctm = scope_stack.last().map_or(root_ctm, |sc| sc.ctm);
                    let up_paints = scope_stack.last().is_none_or(|sc| sc.paints);
                    if !up_paints || *display_none {
                        continue;
                    }
                    let ov = &overrides[*node_id as usize];
                    let whole = ov.resolve(AttrKind::Opacity, *opacity).clamp(0.0, 1.0);
                    if whole <= 0.0 {
                        continue;
                    }
                    let Some(local_ctm) = ov.resolve_transform(*transform) else { continue };
                    let ctm = then(local_ctm, up_ctm);
                    if !super::ctm_is_safe(ctm) {
                        continue;
                    }
                    let up_paint = scope_stack.last().map_or(root_paint, |sc| sc.paint);
                    let resolved = up_paint.inherit(*paint, ov);
                    let into = layers.last_mut().map_or(&mut *acc, |l| &mut l.0);
                    draw_compiled_shape(geom, ov, ctm, origin, px, whole, resolved, buf, into);
                }
            }
        }

        while let Some((top, alpha)) = layers.pop() {
            union(layers.last_mut().map_or(acc, |l| &mut l.0), &top, alpha);
        }
        combined_state
    }
}

#[derive(Clone, Copy)]
struct RenderScope {
    ctm: Transform,
    paints: bool,
    paint: ResolvedPaint,
    layer: bool,
}

#[derive(Clone, Copy)]
struct ResolvedPaint {
    fill: Option<bool>,
    stroke: bool,
    stroke_width: f32,
    fill_rule: Fill,
    cap: Cap,
    join: Join,
    miter_limit: f32,
    fill_opacity: f32,
    stroke_opacity: f32,
}

impl Default for ResolvedPaint {
    fn default() -> Self {
        Self {
            fill: None,
            stroke: false,
            stroke_width: 1.0,
            fill_rule: Fill::NonZero,
            cap: Cap::Butt,
            join: Join::Miter,
            miter_limit: 4.0,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
        }
    }
}

impl ResolvedPaint {
    fn inherit(self, props: PaintProps, ov: &NodeOverrides) -> Self {
        let stroke_width = ov
            .resolve(AttrKind::StrokeWidth, props.stroke_width.unwrap_or(self.stroke_width))
            .clamp(0.0, MAX_STROKE_WIDTH);
        let fill_opacity = ov
            .resolve(AttrKind::FillOpacity, props.fill_opacity.unwrap_or(self.fill_opacity))
            .clamp(0.0, 1.0);
        let stroke_opacity = ov
            .resolve(AttrKind::StrokeOpacity, props.stroke_opacity.unwrap_or(self.stroke_opacity))
            .clamp(0.0, 1.0);
        Self {
            fill: props.fill.or(self.fill),
            stroke: props.stroke.unwrap_or(self.stroke),
            stroke_width,
            fill_rule: props.fill_rule.unwrap_or(self.fill_rule),
            cap: props.cap.unwrap_or(self.cap),
            join: props.join.unwrap_or(self.join),
            miter_limit: props.miter_limit.unwrap_or(self.miter_limit),
            fill_opacity,
            stroke_opacity,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_compiled_shape(
    geom: &BaseGeom,
    ov: &NodeOverrides,
    ctm: Transform,
    origin: Point,
    px: u32,
    whole: f32,
    p: ResolvedPaint,
    buf: &mut [u8],
    into: &mut [u8],
) {
    let mut stack_path = [convex::El::Close; 10];
    let shape = match geom {
        BaseGeom::Circle { cx, cy, r } => {
            let r = ov.resolve(AttrKind::R, *r).clamp(0.0, MAX_COORD);
            if r <= 0.0 {
                return;
            }
            let cx = ov.resolve(AttrKind::Cx, *cx).clamp(-MAX_COORD, MAX_COORD);
            let cy = ov.resolve(AttrKind::Cy, *cy).clamp(-MAX_COORD, MAX_COORD);
            area::Shape::Oval { center: Point::new(cx, cy), radii: Point::new(r, r) }
        }
        BaseGeom::Ellipse { cx, cy, rx, ry } => {
            let rx = ov.resolve(AttrKind::Rx, *rx).clamp(0.0, MAX_COORD);
            let ry = ov.resolve(AttrKind::Ry, *ry).clamp(0.0, MAX_COORD);
            if rx <= 0.0 || ry <= 0.0 {
                return;
            }
            let cx = ov.resolve(AttrKind::Cx, *cx).clamp(-MAX_COORD, MAX_COORD);
            let cy = ov.resolve(AttrKind::Cy, *cy).clamp(-MAX_COORD, MAX_COORD);
            area::Shape::Oval { center: Point::new(cx, cy), radii: Point::new(rx, ry) }
        }
        BaseGeom::Rect { x, y, w, h, rx, ry } => {
            let w = ov.resolve(AttrKind::Width, *w).clamp(0.0, MAX_COORD);
            let h = ov.resolve(AttrKind::Height, *h).clamp(0.0, MAX_COORD);
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let x = ov.resolve(AttrKind::X, *x).clamp(-MAX_COORD, MAX_COORD);
            let y = ov.resolve(AttrKind::Y, *y).clamp(-MAX_COORD, MAX_COORD);
            let rx_v = ov.resolve_opt(AttrKind::Rx, *rx);
            let ry_v = ov.resolve_opt(AttrKind::Ry, *ry);
            let rx_c = rx_v.or(ry_v).unwrap_or(0.0).clamp(0.0, w * 0.5);
            let ry_c = ry_v.or(rx_v).unwrap_or(0.0).clamp(0.0, h * 0.5);
            if rx_c == 0.0 || (rx_c - ry_c).abs() <= 1e-4 {
                area::Shape::Rounded { lo: Point::new(x, y), hi: Point::new(x + w, y + h), r: rx_c }
            } else {
                // Non-uniform rounded rectangle (`rx != ry`) built directly onto stack with exact
                // 90-degree elliptical conics (`w = sqrt(2)/2`).
                let k = std::f32::consts::FRAC_1_SQRT_2;
                let v = convex::V::new;
                stack_path = [
                    convex::El::Move(v(x + rx_c, y)),
                    convex::El::Line(v(x + w - rx_c, y)),
                    convex::El::Conic(v(x + w, y), v(x + w, y + ry_c), k),
                    convex::El::Line(v(x + w, y + h - ry_c)),
                    convex::El::Conic(v(x + w, y + h), v(x + w - rx_c, y + h), k),
                    convex::El::Line(v(x + rx_c, y + h)),
                    convex::El::Conic(v(x, y + h), v(x, y + h - ry_c), k),
                    convex::El::Line(v(x, y + ry_c)),
                    convex::El::Conic(v(x, y), v(x + rx_c, y), k),
                    convex::El::Close,
                ];
                area::Shape::Path(&stack_path)
            }
        }
        BaseGeom::Line { x1, y1, x2, y2 } => {
            let x1 = ov.resolve(AttrKind::X1, *x1).clamp(-MAX_COORD, MAX_COORD);
            let y1 = ov.resolve(AttrKind::Y1, *y1).clamp(-MAX_COORD, MAX_COORD);
            let x2 = ov.resolve(AttrKind::X2, *x2).clamp(-MAX_COORD, MAX_COORD);
            let y2 = ov.resolve(AttrKind::Y2, *y2).clamp(-MAX_COORD, MAX_COORD);
            stack_path[0] = convex::El::Move(convex::V::new(x1, y1));
            stack_path[1] = convex::El::Line(convex::V::new(x2, y2));
            area::Shape::Path(&stack_path[..2])
        }
        BaseGeom::Path(els) => area::Shape::Path(els.as_slice()),
    };

    let do_fill = p.fill.unwrap_or(!p.stroke);
    if do_fill {
        super::paint_shape(&shape, ctm, origin, px, None, p.fill_rule, whole * p.fill_opacity, buf, into);
    }
    if p.stroke && p.stroke_width > 0.0 {
        let pen = area::Pen { width: p.stroke_width, cap: p.cap, join: p.join, miter_limit: p.miter_limit };
        super::paint_shape(&shape, ctm, origin, px, Some(&pen), Fill::NonZero, whole * p.stroke_opacity, buf, into);
    }
}

fn merge_state(a: AnimState, b: AnimState) -> AnimState {
    match (a, b) {
        (AnimState::Moving, _) | (_, AnimState::Moving) => AnimState::Moving,
        (AnimState::WaitingUntil(x), AnimState::WaitingUntil(y)) => AnimState::WaitingUntil(x.min(y)),
        (AnimState::WaitingUntil(x), AnimState::Finished) | (AnimState::Finished, AnimState::WaitingUntil(x)) => {
            AnimState::WaitingUntil(x)
        }
        (AnimState::Finished, AnimState::Finished) => AnimState::Finished,
    }
}

impl Track {
    /// Evaluates the track at `sample_t` (with `runtime_t` used to express `WaitingUntil` in the
    /// runtime clock domain).
    fn sample(&self, sample_t: f64, runtime_t: f64) -> (Option<KeyVal>, AnimState) {
        let begin = self.begin as f64;
        if sample_t < begin {
            let delta = (begin - sample_t).max(0.0);
            return (None, AnimState::WaitingUntil(runtime_t + delta));
        }
        let elapsed = sample_t - begin;

        if self.is_set {
            return match self.dur {
                None => (self.values.first().copied(), AnimState::Finished),
                Some(d) => {
                    let dur = d as f64;
                    if elapsed < dur {
                        (self.values.first().copied(), AnimState::WaitingUntil(runtime_t + (dur - elapsed)))
                    } else if self.freeze {
                        (self.values.first().copied(), AnimState::Finished)
                    } else {
                        (None, AnimState::Finished)
                    }
                }
            };
        }

        let dur = self.dur.unwrap_or(1.0) as f64;
        if let Some(rep) = self.repeat {
            let total = dur * rep as f64;
            if elapsed >= total {
                if !self.freeze {
                    return (None, AnimState::Finished);
                }
                // SMIL 3.0: fractional `repeatCount` with `fill="freeze"` freezes at `fract(rep)`.
                let frac = rep.fract();
                let end_u = if frac.abs() <= 1e-6 { 1.0 } else { frac };
                return (Some(self.eval_u(end_u)), AnimState::Finished);
            }
        }

        let cycle = (elapsed / dur).floor();
        let cycle_start = begin + cycle * dur;
        let local = (elapsed - cycle * dur).clamp(0.0, dur);
        let u = (local / dur) as f32;
        let (val, seg_state) = self.eval_u_with_state(u, cycle_start, dur, sample_t, runtime_t);
        (Some(val), seg_state)
    }

    fn eval_u(&self, u: f32) -> KeyVal {
        let n = self.values.len();
        if n <= 1 {
            return self.values[0];
        }
        let seg = self.segment_for(u);
        if self.calc_mode == CalcMode::Discrete {
            return self.values[seg];
        }
        let t0 = self.key_times[seg];
        let t1 = self.key_times[seg + 1];
        let span = t1 - t0;
        let raw = if span <= 1e-6 { 1.0 } else { ((u - t0) / span).clamp(0.0, 1.0) };
        let frac = match self.calc_mode {
            CalcMode::Spline => {
                let s = self.key_splines[seg];
                cubic_bezier(s[0], s[1], s[2], s[3], raw)
            }
            CalcMode::Linear | CalcMode::Paced => raw,
            CalcMode::Discrete => 0.0,
        };
        lerp_key_val(self.values[seg], self.values[seg + 1], frac)
    }

    fn eval_u_with_state(&self, u: f32, cycle_start: f64, dur: f64, sample_t: f64, runtime_t: f64) -> (KeyVal, AnimState) {
        let n = self.values.len();
        if n <= 1 {
            return (self.values[0], AnimState::Finished);
        }
        let seg = self.segment_for(u);
        if self.calc_mode == CalcMode::Discrete {
            let next_u = if seg + 1 < n { self.key_times[seg + 1] as f64 } else { 1.0 };
            let next_sample = cycle_start + next_u * dur;
            let wake = self.clamp_wake(next_sample, sample_t, runtime_t);
            return (self.values[seg], wake);
        }
        let a = self.values[seg];
        let b = self.values[seg + 1];
        let t0 = self.key_times[seg];
        let t1 = self.key_times[seg + 1];
        let span = t1 - t0;
        let flat = key_val_eq(a, b) || span <= 1e-6;
        if flat {
            let next_sample = cycle_start + (t1 as f64) * dur;
            let wake = self.clamp_wake(next_sample, sample_t, runtime_t);
            return (a, wake);
        }
        let raw = ((u - t0) / span).clamp(0.0, 1.0);
        let frac = match self.calc_mode {
            CalcMode::Spline => {
                let s = self.key_splines[seg];
                cubic_bezier(s[0], s[1], s[2], s[3], raw)
            }
            CalcMode::Linear | CalcMode::Paced => raw,
            CalcMode::Discrete => 0.0,
        };
        (lerp_key_val(a, b, frac), AnimState::Moving)
    }

    fn clamp_wake(&self, next_sample: f64, sample_t: f64, runtime_t: f64) -> AnimState {
        let mut target = next_sample;
        if let Some(rep) = self.repeat {
            let end = self.begin as f64 + self.dur.unwrap_or(1.0) as f64 * rep as f64;
            if target > end {
                target = end;
            }
        }
        let delta = target - sample_t;
        if delta <= 1e-6 {
            // At a segment boundary right now: step one frame to enter the next segment.
            AnimState::Moving
        } else {
            AnimState::WaitingUntil(runtime_t + delta)
        }
    }

    fn segment_for(&self, u: f32) -> usize {
        let n = self.values.len();
        if self.calc_mode == CalcMode::Discrete {
            let mut seg = 0;
            for i in 1..n {
                if u >= self.key_times[i] {
                    seg = i;
                } else {
                    break;
                }
            }
            return seg;
        }
        let mut seg = 0;
        for i in 0..n - 1 {
            if u >= self.key_times[i] {
                seg = i;
            } else {
                break;
            }
        }
        seg
    }
}

fn key_val_eq(a: KeyVal, b: KeyVal) -> bool {
    let len = a.len.max(b.len) as usize;
    (0..len).all(|i| (a.v[i] - b.v[i]).abs() <= 1e-6)
}

fn lerp_key_val(a: KeyVal, b: KeyVal, frac: f32) -> KeyVal {
    let len = a.len.max(b.len);
    let mut v = [0.0f32; 3];
    for (i, slot) in v.iter_mut().enumerate().take(len as usize) {
        let out = a.v[i] + (b.v[i] - a.v[i]) * frac;
        *slot = if out.is_finite() { out } else { b.v[i] };
    }
    KeyVal { v, len }
}

fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    if (x1 - y1).abs() < 1e-5 && (x2 - y2).abs() < 1e-5 {
        return x;
    }
    let (x1, x2) = (x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0));
    let sample = |a: f32, b: f32, t: f32| -> f32 {
        let mt = 1.0 - t;
        3.0 * mt * mt * t * a + 3.0 * mt * t * t * b + t * t * t
    };
    let slope = |a: f32, b: f32, t: f32| -> f32 {
        let mt = 1.0 - t;
        3.0 * mt * mt * a + 6.0 * mt * t * (b - a) + 3.0 * t * t * (1.0 - b)
    };
    let mut s = x;
    for _ in 0..8 {
        let err = sample(x1, x2, s) - x;
        if err.abs() < 1e-6 {
            return sample(y1, y2, s);
        }
        let d = slope(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - err / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    s = x;
    for _ in 0..20 {
        let v = sample(x1, x2, s);
        if (v - x).abs() < 1e-6 {
            break;
        }
        if v < x { lo = s; } else { hi = s; }
        s = 0.5 * (lo + hi);
    }
    sample(y1, y2, s)
}

fn key_val_to_transform(kind: TransformKind, kv: KeyVal) -> Transform {
    match kind {
        TransformKind::Translate => {
            Transform::translation(kv.v[0].clamp(-MAX_COORD, MAX_COORD), kv.v[1].clamp(-MAX_COORD, MAX_COORD))
        }
        TransformKind::Scale => {
            let sx = kv.v[0].clamp(-MAX_SCALE, MAX_SCALE);
            let sy = (if kv.len >= 2 { kv.v[1] } else { kv.v[0] }).clamp(-MAX_SCALE, MAX_SCALE);
            Transform::scale(sx, sy)
        }
        TransformKind::Rotate => rotate_transform(
            kv.v[0].clamp(-36_000.0, 36_000.0),
            kv.v[1].clamp(-MAX_COORD, MAX_COORD),
            kv.v[2].clamp(-MAX_COORD, MAX_COORD),
        ),
        TransformKind::SkewX => {
            Transform::new(1.0, 0.0, kv.v[0].clamp(-MAX_SKEW_DEG, MAX_SKEW_DEG).to_radians().tan(), 1.0, 0.0, 0.0)
        }
        TransformKind::SkewY => {
            Transform::new(1.0, kv.v[0].clamp(-MAX_SKEW_DEG, MAX_SKEW_DEG).to_radians().tan(), 0.0, 1.0, 0.0, 0.0)
        }
    }
}
