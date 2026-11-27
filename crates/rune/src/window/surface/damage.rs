//! What part of a window or canvas a frame redraws (and on Windows, presents). The renderer
//! redraws only what changed into the frame it keeps (`render::damaged`) and copies or blits it
//! into the drawable; on Windows, the swap chain also presents only those dirty rectangles
//! (`Present1`), which is all DWM composes again.
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

/// The glyph atlas state `Damage` needs to tell whether an unchanged instance may sample
/// different texels than in the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GlyphAtlas {
    pub(crate) resets: u64,
    pub(crate) rasterized: u64,
    pub(crate) dirty_rows: bool,
    pub(crate) pending_fills: bool,
}

impl GlyphAtlas {
    pub(crate) fn of(atlas: &crate::text::Atlas) -> Self {
        Self {
            resets: atlas.resets(),
            rasterized: atlas.rasterized(),
            dirty_rows: atlas.dirty.is_some() || atlas.pages.iter().any(|p| p.dirty.is_some()),
            pending_fills: atlas.has_pending_fills(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Shape {
    size: (u32, u32),
    clear: [f32; 4],
    swap: u64,
    atlas_resets: u64,
}

/// The last frames' changes.
#[derive(Default)]
pub(crate) struct Damage {
    /// The last frame's instances, and its size, clear colour, swap chain (the surface's
    /// configuration, `configure::generation`), and glyph atlas reset count.
    last: Vec<Instance>,
    shape: Option<Shape>,
    /// `Atlas::rasterized()` as of the last planned frame.
    atlas_rasterized: u64,
    scratch: Scratch,
    /// The last change seen, however much was drawn (frame-profile).
    #[cfg_attr(not(feature = "frame-profile"), allow(dead_code))]
    pub(crate) seen: Option<[u32; 4]>,
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
pub(crate) enum Plan {
    /// Everything, and why.
    All(&'static str),
    /// Redraw `draw` (`[x, y, width, height]` each), present `present` (`[x0, y0, x1, y1]` each).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Part { draw: Vec<[u32; 4]>, present: Vec<[i32; 4]> },
}

impl Damage {
    /// Whether a loop-only frame (`Renderer::arm_loop` / `Renderer::arm_cycle`) may patch only
    /// `patched` instead of going through `plan`: the swap chain, size and clear colour still
    /// match, the glyph atlas has had no eviction, new rasterization or pending GPU fill since
    /// the last frame, and if CPU glyph rows are dirty (an animated icon's in-place slot write),
    /// at least one patched instance's bytes differ from the last frame's.
    #[cfg_attr(not(any(target_os = "windows", target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) fn can_patch_loop(
        &self,
        instances: &[Instance],
        patched: &[u32],
        size: (u32, u32),
        clear: [f32; 4],
        swap: u64,
        atlas: GlyphAtlas,
    ) -> Result<(), &'static str> {
        if !self.current(size, clear, swap) {
            return Err("a new swap chain");
        }
        if !self.shape.is_some_and(|s| s.atlas_resets == atlas.resets)
            || self.atlas_rasterized != atlas.rasterized
            || atlas.pending_fills
        {
            return Err("glyph atlas");
        }
        if atlas.dirty_rows && !self.any_patched_differs(instances, patched) {
            return Err("glyph atlas");
        }
        Ok(())
    }

    #[cfg_attr(not(any(target_os = "windows", target_arch = "wasm32", test)), allow(dead_code))]
    fn any_patched_differs(&self, instances: &[Instance], patched: &[u32]) -> bool {
        self.last.len() == instances.len()
            && patched.iter().any(|&i| {
                let idx = i as usize;
                idx < instances.len()
                    && bytemuck::bytes_of(&self.last[idx]) != bytemuck::bytes_of(&instances[idx])
            })
    }

    /// The frame about to draw `instances` into a `size` target cleared to `clear`, on swap chain
    /// `swap`, with glyph atlas state `atlas`; `in_place` when nothing but its instances draws there.
    pub(crate) fn plan(
        &mut self,
        instances: &[Instance],
        size: (u32, u32),
        clear: [f32; 4],
        swap: u64,
        atlas: GlyphAtlas,
        in_place: bool,
    ) -> Plan {
        if size.0 == 0 || size.1 == 0 {
            self.shape = None;
            return Plan::All("empty");
        }
        let shape = Shape { size, clear, swap, atlas_resets: atlas.resets };
        let prev_shape = self.shape;
        let prev_rasterized = self.atlas_rasterized;
        // Pending GPU fills with no new rasterizations mean existing glyph slots were queued for
        // refill (e.g. `rows_uploaded` or `refill_gpu_masks`) while their instance bytes stay put.
        let glyph_refill = atlas.pending_fills && atlas.rasterized == prev_rasterized;
        let (now, why) = match (
            in_place && !whole_frames(),
            clear[3] == 1.0,
            prev_shape.is_some_and(|s| s.size == size && s.clear == clear && s.swap == swap),
            prev_shape.is_some_and(|s| s.atlas_resets == atlas.resets),
            !glyph_refill,
        ) {
            (false, _, _, _, _) => (Area::All, "not in place"),
            (_, false, _, _, _) => (Area::All, "translucent clear"),
            (_, _, false, _, _) => (Area::All, "new size, colour or swap chain"),
            (_, _, _, false, _) => (Area::All, "glyph atlas reset"),
            (_, _, _, _, false) => (Area::All, "glyph atlas fill"),
            _ => {
                let now = changed(&self.last, instances, size, &mut self.scratch);
                self.seen = match &now { Area::Rects(r) => r.iter().copied().reduce(join), Area::All | Area::Many => None };
                match now {
                    Area::Rects(r) if r.is_empty() && (atlas.dirty_rows || atlas.pending_fills || atlas.rasterized != prev_rasterized) => {
                        (Area::All, "glyph atlas")
                    }
                    Area::Rects(r) if r.iter().map(|r| area(*r)).sum::<u32>() as f32 > MOST * (size.0 * size.1) as f32 => {
                        (Area::All, "most of it changed")
                    }
                    Area::All => (Area::All, "reordered"),
                    Area::Many => (Area::All, "many changed"),
                    other => (other, ""),
                }
            }
        };
        self.last.clear();
        self.last.extend_from_slice(instances);
        self.shape = Some(shape);
        self.atlas_rasterized = atlas.rasterized;
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
    pub(crate) fn reset(&mut self) { self.shape = None; }

    /// The last frame drew into swap chain `swap` at this size and clear colour: a frame may draw
    /// and present only part of it. False after `reset` or a configure (a new swap chain), so its
    /// first frame, loop-only or not, goes through `plan` and draws whole.
    #[cfg_attr(not(any(target_os = "windows", target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) fn current(&self, size: (u32, u32), clear: [f32; 4], swap: u64) -> bool {
        self.shape.is_some_and(|s| s.size == size && s.clear == clear && s.swap == swap)
    }

    /// A loop-only frame changed only `patched` (ascending) and drew them where they were and
    /// are (`Renderer::arm_loop` / `Renderer::arm_cycle`): the next frame compares with them.
    #[cfg_attr(not(any(target_os = "windows", target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) fn patched(&mut self, instances: &[Instance], patched: &[u32]) {
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
#[path = "damage_tests.rs"]
mod tests;
