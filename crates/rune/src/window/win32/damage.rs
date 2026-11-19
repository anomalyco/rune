//! Windows: what part of a window a frame redraws and presents. The renderer redraws only what
//! changed into the frame it keeps (`render::damaged`), and the window presents only that
//! (`Present1` dirty rectangles), which is all DWM composes again.
//!
//! What changed is the instances that differ from the last frame's. The lists' common start and
//! end are skipped; in what's left, an instance whose bytes are in both lists didn't change (its
//! place is in its bytes), so a typed glyph that pushes the rest of the list along changes only
//! itself and the caret. The bounds of the instances only one list has are the change, gathered
//! into at most `RECTS` rectangles. If the instances both lists have were drawn in another order,
//! everything changed.
//!
//! The whole window is drawn when anything else could have moved pixels (`Renderer::
//! redraws_in_place`), when the size or clear colour changed, after a new swap chain, and when
//! the change is most of the window anyway.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::render::{INSTANCE_WORDS, Instance};

/// Above this share of the window, a frame draws all of it.
const MOST: f32 = 0.6;
/// Past this many instances the new list has that the old doesn't, a frame draws all of it without
/// looking for where they are: a scroll moves every row, and checking the order of thousands of
/// instances and bounding them only to find they cover the window costs a list's frame hundreds of
/// microseconds. A typed glyph, a streamed line or a hover changes far fewer.
const MANY: usize = 256;
/// Rectangles a change is gathered into, at most.
pub(super) const RECTS: usize = crate::render::MAX_RECTS;

/// The last frames' changes.
#[derive(Default)]
pub(in crate::window) struct Damage {
    /// The last frame's instances, and its size, clear colour and swap chain (the surface's
    /// configuration, `configure::generation`).
    last: Vec<Instance>,
    shape: Option<((u32, u32), [f32; 4], u64)>,
    scratch: Scratch,
    /// The last change seen, however much was drawn (frame-profile).
    #[cfg_attr(not(feature = "frame-profile"), allow(dead_code))]
    pub(in crate::window) seen: Option<[u32; 4]>,
}

/// Rectangles `[x0, y0, x1, y1]` in the window's pixels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Area {
    /// Everything (or not known).
    #[default]
    All,
    /// More than `MANY` instances.
    Many,
    Rects(Vec<[u32; 4]>),
}

/// What a frame draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::window) enum Plan {
    /// Everything, and why.
    All(&'static str),
    /// Redraw `draw` (`[x, y, width, height]` each), present `present` (`[x0, y0, x1, y1]` each).
    Part { draw: Vec<[u32; 4]>, present: Vec<[i32; 4]> },
}

impl Damage {
    /// The frame about to draw `instances` into a `size` target cleared to `clear`, on swap chain
    /// `swap`; `in_place` when nothing but its instances draws there.
    pub(in crate::window) fn plan(&mut self, instances: &[Instance], size: (u32, u32), clear: [f32; 4], swap: u64, in_place: bool) -> Plan {
        let (now, why) = match (in_place && !whole_frames(), self.shape == Some((size, clear, swap))) {
            (false, _) => (Area::All, "not in place"),
            (_, false) => (Area::All, "new size, colour or swap chain"),
            _ => {
                let now = changed(&self.last, instances, size, &mut self.scratch);
                self.seen = match &now { Area::Rects(r) => r.iter().copied().reduce(join), Area::All | Area::Many => None };
                match now {
                    Area::Rects(r) if r.iter().map(|r| area(*r)).sum::<u32>() as f32 > MOST * (size.0 * size.1) as f32 => (Area::All, "most of it changed"),
                    Area::All => (Area::All, "reordered"),
                    Area::Many => (Area::All, "many changed"),
                    other => (other, ""),
                }
            }
        };
        self.last.clear();
        self.last.extend_from_slice(instances);
        self.shape = Some((size, clear, swap));
        match now {
            Area::All | Area::Many => Plan::All(why),
            // Nothing changed: one pixel keeps the present, which DWM composes again as is.
            Area::Rects(r) if r.is_empty() => Plan::Part { draw: vec![[0, 0, 1, 1]], present: vec![[0, 0, 1, 1]] },
            Area::Rects(r) => Plan::Part {
                draw: r.iter().map(|r| [r[0], r[1], r[2] - r[0], r[3] - r[1]]).collect(),
                present: r.iter().map(|r| r.map(|v| v as i32)).collect(),
            },
        }
    }

    /// A new swap chain: the next frame draws whole.
    pub(in crate::window) fn reset(&mut self) { self.shape = None; }

    /// The last frame drew into swap chain `swap` at this size and clear colour: a frame may draw
    /// and present only part of it. False after `reset` or a configure (a new swap chain), so its
    /// first frame, loop-only or not, goes through `plan` and draws whole.
    pub(in crate::window) fn current(&self, size: (u32, u32), clear: [f32; 4], swap: u64) -> bool { self.shape == Some((size, clear, swap)) }

    /// A loop-only frame changed only `patched` (ascending) and drew them where they were and
    /// are (`Renderer::arm_loop`): the next frame compares with them.
    pub(in crate::window) fn patched(&mut self, instances: &[Instance], patched: &[u32]) {
        if self.last.len() != instances.len() { self.shape = None; return; }
        for &i in patched { self.last[i as usize] = instances[i as usize]; }
    }
}
/// `RUNE_WHOLE_FRAMES=1`: every frame draws whole, to check partial frames against.
fn whole_frames() -> bool {
    static WHOLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WHOLE.get_or_init(|| std::env::var_os("RUNE_WHOLE_FRAMES").is_some_and(|v| v == "1"))
}

type Counts = HashMap<u64, (i32, usize), BuildHasherDefault<Hashed>>;

/// The map's keys are hashes already, mixed once more: FNV's multiply carries only upward, so its
/// low bits, which pick the bucket, come from the words' low bits alone, and most instances share
/// those (whole-pixel rects and clips). Unmixed, a scrolled list's thousands of instances fell in
/// a few long probe chains.
#[derive(Default)]
struct Hashed(u64);

impl Hasher for Hashed {
    fn finish(&self) -> u64 { self.0 }
    fn write(&mut self, _: &[u8]) { unreachable!("keys are u64") }
    fn write_u64(&mut self, v: u64) {
        // MurmurHash3's 64-bit finaliser.
        let mut h = v ^ (v >> 33);
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        self.0 = h ^ (h >> 33);
    }
}

/// FNV-1a over an instance's 32 words: a few ns, against SipHash's ~100.
fn hash(i: &Instance) -> u64 {
    bytemuck::cast_ref::<Instance, [u32; INSTANCE_WORDS]>(i).iter().fold(0xcbf2_9ce4_8422_2325u64, |h, w| (h ^ u64::from(*w)).wrapping_mul(0x0100_0000_01b3))
}

/// What differs between `old` and `new`.
/// What `changed` keeps between frames so it allocates nothing.
#[derive(Default)]
pub(super) struct Scratch {
    old: Vec<u64>,
    new: Vec<u64>,
    counts: Counts,
    each: Vec<[u32; 4]>,
}

fn changed(old: &[Instance], new: &[Instance], size: (u32, u32), s: &mut Scratch) -> Area {
    fn bytes(i: &Instance) -> &[u8] { bytemuck::bytes_of(i) }
    let start = old.iter().zip(new).take_while(|(a, b)| bytes(a) == bytes(b)).count();
    let end = old[start..].iter().rev().zip(new[start..].iter().rev()).take_while(|(a, b)| bytes(a) == bytes(b)).count();
    let (old, new) = (&old[start..old.len() - end], &new[start..new.len() - end]);
    // Per distinct instance: how many more the old list has than the new, and one of them.
    s.counts.clear();
    s.old.clear();
    s.old.extend(old.iter().map(hash));
    s.new.clear();
    s.new.extend(new.iter().map(hash));
    for (i, h) in s.old.iter().enumerate() { s.counts.entry(*h).or_insert((0, i)).0 += 1; }
    let mut added = 0;
    for (i, h) in s.new.iter().enumerate() {
        let count = &mut s.counts.entry(*h).or_insert((0, old.len() + i)).0;
        *count -= 1;
        if *count < 0 { added += 1; }
        if added > MANY { return Area::Many; }
    }
    // The instances both lists have, in order: drawn in another order, anything may have changed.
    let counts = &s.counts;
    let mut j = 0;
    for &h in &s.old {
        if counts[&h].0 != 0 { continue; }
        if j < s.new.len() && s.new[j] == h { j += 1; continue; }
        while j < s.new.len() && counts[&s.new[j]].0 != 0 { j += 1; }
        if j >= s.new.len() || s.new[j] != h { return Area::All; }
        j += 1;
    }
    if s.new[j..].iter().any(|h| counts[h].0 == 0) { return Area::All; }
    // In reading order, so neighbours gather before rectangles run out.
    s.each.clear();
    s.each.extend(counts.values().filter(|(n, _)| *n != 0)
        .filter_map(|(_, at)| bounds(if *at < old.len() { &old[*at] } else { &new[*at - old.len()] }, size)));
    // A scrolled list changes thousands of instances over most of the window: their union says
    // so (the plan draws whole above `MOST`) without gathering them into rectangles.
    if s.each.len() > 256 && let Some(all) = s.each.iter().copied().reduce(join) && area(all) as f32 > MOST * (size.0 * size.1) as f32 {
        return Area::Rects(vec![all]);
    }
    s.each.sort_unstable_by_key(|r| (r[1], r[0]));
    let mut rects = Vec::new();
    for r in s.each.iter().copied() { add(&mut rects, r); }
    Area::Rects(rects)
}
/// Adds `r` to `rects`: into one it touches, or as its own until there are `RECTS`, then into
/// the one it grows least.
fn add(rects: &mut Vec<[u32; 4]>, r: [u32; 4]) {
    if let Some(i) = rects.iter().position(|q| near(*q, r)) {
        let merged = join(rects.swap_remove(i), r);
        return add(rects, merged);
    }
    if rects.len() < RECTS { rects.push(r); return; }
    let grow = |q: [u32; 4]| area(join(q, r)) - area(q);
    let i = (0..rects.len()).min_by_key(|&i| grow(rects[i])).unwrap_or(0);
    let merged = join(rects.swap_remove(i), r);
    add(rects, merged);
}

/// Whether two rectangles overlap or lie within 16 pixels of each other.
fn near(a: [u32; 4], b: [u32; 4]) -> bool {
    a[0] <= b[2] + 16 && b[0] <= a[2] + 16 && a[1] <= b[3] + 16 && b[1] <= a[3] + 16
}

fn join(a: [u32; 4], b: [u32; 4]) -> [u32; 4] { [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])] }

/// The pixels an instance can touch: its quad as the shader grows it, inside its clip.
fn bounds(i: &Instance, size: (u32, u32)) -> Option<[u32; 4]> {
    let (x0, y0, x1, y1) = i.damage_quad_bounds()?;
    if x1 <= x0 || y1 <= y0 { return None; }
    let c = i.clip;
    let (x0, y0) = ((x0.max(c[0]) - 1.0).floor().max(0.0), (y0.max(c[1]) - 1.0).floor().max(0.0));
    let (x1, y1) = ((x1.min(c[2]) + 1.0).ceil().min(size.0 as f32), (y1.min(c[3]) + 1.0).ceil().min(size.1 as f32));
    (x1 > x0 && y1 > y0).then(|| [x0 as u32, y0 as u32, x1 as u32, y1 as u32])
}

fn area(r: [u32; 4]) -> u32 { (r[2] - r[0]) * (r[3] - r[1]) }

#[cfg(test)]
mod tests {
    use bytemuck::Zeroable;

    use super::*;
    use crate::render::{KIND_BACKDROP_BLUR, KIND_DASHED, KIND_EFFECT, KIND_GLYPH, KIND_GRADIENT, KIND_IMAGE, KIND_INSET, KIND_RADIAL, KIND_RECT, KIND_SEGMENT, KIND_SHADOW, KIND_TRIANGLE, Kind, LayerSpec, NO_CLIP, cpu_boot};

    fn old_bounds(i: &Instance, size: (u32, u32)) -> Option<[u32; 4]> {
        let kind = i.params[1];
        if (8.5..9.5).contains(&kind) { return None; }
        let grow = match kind {
            k if (0.5..1.5).contains(&k) => i.params[2] * 3.0 + 1.0,
            k if (1.5..2.5).contains(&k) => 0.0,
            k if (2.5..3.5).contains(&k) || (4.5..9.5).contains(&k) => 1.0,
            _ => 1.0 + i.params[2] * 2.0,
        };
        let [x, y, w, h] = i.rect;
        let (mut x0, mut y0, mut x1, mut y1) = (x - grow, y - grow, x + w + grow, y + h + grow);
        if (1.5..2.5).contains(&kind) && i.params[0] != 0.0 {
            let (cx, cy, r) = (x + w * 0.5, y + h * 0.5, (w * w + h * h).sqrt() * 0.5);
            (x0, y0, x1, y1) = (cx - r, cy - r, cx + r, cy + r);
        }
        let c = i.clip;
        let (x0, y0) = ((x0.max(c[0]) - 1.0).floor().max(0.0), (y0.max(c[1]) - 1.0).floor().max(0.0));
        let (x1, y1) = ((x1.min(c[2]) + 1.0).ceil().min(size.0 as f32), (y1.min(c[3]) + 1.0).ceil().min(size.1 as f32));
        (x1 > x0 && y1 > y0).then(|| [x0 as u32, y0 as u32, x1 as u32, y1 as u32])
    }

    fn contains(outer: [i32; 4], inner: [i32; 4]) -> bool {
        if inner[2] <= inner[0] || inner[3] <= inner[1] { return true; }
        outer[2] > outer[0] && outer[3] > outer[1]
            && outer[0] <= inner[0] && outer[1] <= inner[1]
            && outer[2] >= inner[2] && outer[3] >= inner[3]
    }

    #[test]
    fn damage_bounds_contain_exact_drawn_pixels_and_tighten_overestimates() {
        let size = (256u32, 256u32);
        let (w, h) = (size.0 as i32, size.1 as i32);
        let mut atlas = crate::text::Atlas::new();
        atlas.data.resize((crate::text::ATLAS_SIZE * crate::text::ATLAS_SIZE) as usize, 0);
        for y in 0..16usize {
            for x in 0..16usize {
                atlas.data[y * crate::text::ATLAS_SIZE as usize + x] = 255;
            }
        }
        let mut images = crate::pixels::ImageAtlas::default();
        let px = crate::Pixels::new();
        px.set(16, 16, vec![255u8; 16 * 16 * 4]);
        let placed = images.place(&px, [16.0, 16.0]).expect("place test image");
        let base = |kind: Kind, rect: [f32; 4]| Instance {
            rect,
            fill: [1.0, 0.4, 0.2, 1.0],
            border: [0.2, 0.8, 1.0, 1.0],
            params: [0.0, kind.as_f32(), 0.0, 1.0],
            clip: NO_CLIP,
            ..Instance::zeroed()
        };
        let cases: Vec<(&str, Instance, bool)> = vec![
            ("rect_plain", base(Kind::Rect, [30.3, 40.7, 50.0, 30.0]), true),
            ("rect_border_soft", Instance { params: [3.0, KIND_RECT, 2.0, 1.0], radii: [8.0; 4], ..base(Kind::Rect, [40.0, 40.0, 60.0, 40.0]) }, true),
            ("rect_clipped", Instance { clip: [45.2, 48.6, 82.4, 70.1], ..base(Kind::Rect, [30.0, 40.0, 80.0, 50.0]) }, true),
            ("rect_banded", Instance { params: [2.0, KIND_RECT, 0.0, 1.0], uv: [-1.0, -1.0, -101.0, 6.0], ..base(Kind::Rect, [50.0, 60.0, 100.0, 80.0]) }, true),
            ("shadow_plain", Instance { params: [0.0, KIND_SHADOW, 6.0, 1.0], radii: [8.0; 4], ..base(Kind::Shadow, [60.0, 60.0, 80.0, 50.0]) }, true),
            ("shadow_banded", Instance { params: [0.0, KIND_SHADOW, 6.0, 1.0], uv: [90.0, -19.0, -119.0, 99.0], ..base(Kind::Shadow, [40.0, 50.0, 100.0, 80.0]) }, true),
            ("glyph_upright", base(Kind::Glyph, [70.0, 70.0, 16.0, 16.0]), true),
            ("glyph_rot_15deg", Instance { params: [0.26, KIND_GLYPH, 0.0, 1.0], uv: [0.0, 0.0, 16.0, 16.0], ..base(Kind::Glyph, [80.0, 80.0, 40.0, 12.0]) }, true),
            ("glyph_rot_45deg", Instance { params: [std::f32::consts::FRAC_PI_4, KIND_GLYPH, 0.0, 1.0], uv: [0.0, 0.0, 16.0, 16.0], ..base(Kind::Glyph, [80.0, 80.0, 40.0, 12.0]) }, true),
            ("glyph_rot_90deg", Instance { params: [std::f32::consts::FRAC_PI_2, KIND_GLYPH, 0.0, 1.0], uv: [0.0, 0.0, 16.0, 16.0], ..base(Kind::Glyph, [80.0, 80.0, 40.0, 12.0]) }, true),
            ("inset", Instance { params: [2.0, KIND_INSET, 4.0, 1.0], uv: [2.0, 2.0, 0.0, 0.0], ..base(Kind::Inset, [50.0, 50.0, 60.0, 40.0]) }, true),
            ("gradient", Instance { uv: [0.1, 0.2, 0.9, 1.0], params: [0.0, KIND_GRADIENT, 1.5, 1.0], ..base(Kind::Gradient, [30.0, 30.0, 50.0, 50.0]) }, true),
            ("segment", Instance { radii: [40.0, 40.0, 110.0, 85.0], params: [3.0, KIND_SEGMENT, 1.0, 1.0], ..base(Kind::Segment, [36.0, 36.0, 78.0, 53.0]) }, true),
            ("triangle", Instance { radii: [50.0, 40.0, 120.0, 60.0], uv: [70.0, 110.0, 0.0, 0.0], params: [7.0, KIND_TRIANGLE, 0.0, 1.0], ..base(Kind::Triangle, [50.0, 40.0, 70.0, 70.0]) }, true),
            ("image", Instance { params: [placed.page as f32, KIND_IMAGE, 0.0, 1.0], uv: [placed.rect[0] as f32, placed.rect[1] as f32, placed.rect[2] as f32, placed.rect[3] as f32], ..base(Kind::Image, [80.0, 80.0, 24.0, 24.0]) }, true),
            ("layer_plain", Instance::begin_layer([40.0, 50.0, 80.0, 60.0], NO_CLIP, LayerSpec::default()), false),
            ("layer_turned", Instance::begin_layer([60.0, 60.0, 80.0, 40.0], NO_CLIP, LayerSpec::default()).turned(0.6, [100.0, 80.0]), false),
            ("layer_end", Instance::end_layer(), false),
            ("dashed", Instance { params: [3.0, KIND_DASHED, 0.5, 1.0], uv: [6.0, 4.0, 0.0, 0.0], ..base(Kind::Dashed, [40.0, 40.0, 80.0, 60.0]) }, true),
            ("radial", Instance { params: [0.0, KIND_RADIAL, 1.0, 1.0], uv: [40.0, 30.0, 40.0, 30.0], ..base(Kind::Radial, [40.0, 40.0, 80.0, 60.0]) }, true),
            ("effect_bleed16", Instance { params: [0.0, KIND_EFFECT, 16.0, 1.0], ..base(Kind::Effect, [60.0, 60.0, 40.0, 30.0]) }, false),
            ("backdrop_blur", Instance { params: [12.0, KIND_BACKDROP_BLUR, 0.0, 1.0], ..base(Kind::BackdropBlur, [50.0, 50.0, 70.0, 50.0]) }, false),
        ];
        let mut tighter = Vec::new();
        for (name, inst, paintable) in &cases {
            let new_r = bounds(inst, size).map_or([0; 4], |r| r.map(|v| v as i32));
            let old_r = old_bounds(inst, size).map_or([0; 4], |r| r.map(|v| v as i32));
            let pb = inst.pixel_bounds(w, h);
            assert!(contains(new_r, pb), "{name}: damage {new_r:?} missed pixel_bounds {pb:?}");
            if *paintable {
                let mut buf = vec![0u8; (w * h * 4) as usize];
                let scene = cpu_boot::Scene::new(std::slice::from_ref(inst), &atlas, &images, size, [0.0; 4]);
                cpu_boot::paint(&scene, &mut cpu_boot::Target::new(&mut buf, size.0, size.1), 1).expect("paint");
                let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
                for y in 0..h {
                    for x in 0..w {
                        let at = ((y * w + x) * 4) as usize;
                        if buf[at..at + 4] != [0, 0, 0, 0] {
                            x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x + 1); y1 = y1.max(y + 1);
                        }
                    }
                }
                let painted = if x1 > x0 && y1 > y0 { [x0, y0, x1, y1] } else { [0; 4] };
                assert!(contains(new_r, painted), "{name}: damage {new_r:?} missed CPU-painted {painted:?}");
            }
            if matches!(inst.kind(), Some(Kind::Effect | Kind::BackdropBlur)) {
                let lb = inst.layer_bounds();
                let lb_px = [lb[0].max(0.0).floor() as i32, lb[1].max(0.0).floor() as i32, lb[2].min(w as f32).ceil() as i32, lb[3].min(h as f32).ceil() as i32];
                assert!(contains(new_r, lb_px), "{name}: damage {new_r:?} missed layer_bounds {lb_px:?}");
            }
            let new_a = i64::from((new_r[2] - new_r[0]).max(0)) * i64::from((new_r[3] - new_r[1]).max(0));
            let old_a = i64::from((old_r[2] - old_r[0]).max(0)) * i64::from((old_r[3] - old_r[1]).max(0));
            if new_a < old_a {
                assert!(contains(old_r, new_r), "{name}: tighter damage must be subset of old");
                tighter.push(*name);
            }
        }
        assert!(tighter.contains(&"rect_banded"), "tighter: {tighter:?}");
        assert!(tighter.contains(&"shadow_banded"), "tighter: {tighter:?}");
        assert!(tighter.contains(&"glyph_rot_15deg"), "tighter: {tighter:?}");
        assert!(tighter.contains(&"effect_bleed16"), "tighter: {tighter:?}");
    }

    #[test]
    fn changed_order_check_handles_inserts_deletes_duplicates_and_reorders() {
        let r = |x: f32| Instance {
            rect: [x, 10.0, 20.0, 20.0],
            fill: [1.0; 4],
            params: [0.0, KIND_RECT, 0.0, 1.0],
            clip: NO_CLIP,
            ..Instance::zeroed()
        };
        let mut s = Scratch::default();
        let size = (256, 256);

        // Partial in-place update keeps order.
        let old = [r(0.0), r(30.0), r(60.0), r(90.0)];
        let mut new = old;
        new[2] = r(65.0);
        assert!(matches!(changed(&old, &new, size, &mut s), Area::Rects(ref rs) if !rs.is_empty()));

        // Insert and delete with duplicates preserve order of remaining items.
        let old_dup = [r(0.0), r(30.0), r(30.0), r(60.0), r(90.0)];
        let new_dup = [r(0.0), r(15.0), r(30.0), r(30.0), r(90.0)];
        assert!(matches!(changed(&old_dup, &new_dup, size, &mut s), Area::Rects(ref rs) if !rs.is_empty()));

        // Swapping two unchanged instances triggers full redraw (`Area::All`).
        let swapped = [r(0.0), r(60.0), r(30.0), r(90.0)];
        assert_eq!(changed(&old, &swapped, size, &mut s), Area::All);
    }
}
