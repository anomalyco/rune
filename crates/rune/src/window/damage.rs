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

use crate::render::Instance;

/// Above this share of the window, a frame draws all of it.
const MOST: f32 = 0.6;
/// Rectangles a change is gathered into, at most.
pub(super) const RECTS: usize = crate::render::MAX_RECTS;

/// The last frames' changes.
#[derive(Default)]
pub(super) struct Damage {
    /// The last frame's instances, size and clear colour.
    last: Vec<Instance>,
    shape: Option<((u32, u32), [f32; 4])>,
    scratch: Scratch,
    /// The last change seen, however much was drawn (frame-profile).
    pub(super) seen: Option<[u32; 4]>,
}

/// Rectangles `[x0, y0, x1, y1]` in the window's pixels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Area {
    /// Everything (or not known).
    #[default]
    All,
    Rects(Vec<[u32; 4]>),
}

/// What a frame draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Plan {
    /// Everything, and why.
    All(&'static str),
    /// Redraw `draw` (`[x, y, width, height]` each), present `present` (`[x0, y0, x1, y1]` each).
    Part { draw: Vec<[u32; 4]>, present: Vec<[i32; 4]> },
}

impl Damage {
    /// The frame about to draw `instances` into a `size` target cleared to `clear`; `in_place`
    /// when nothing but its instances draws there.
    pub(super) fn plan(&mut self, instances: &[Instance], size: (u32, u32), clear: [f32; 4], in_place: bool) -> Plan {
        let (now, why) = match (in_place && !whole_frames(), self.shape == Some((size, clear))) {
            (false, _) => (Area::All, "not in place"),
            (_, false) => (Area::All, "new size or colour"),
            _ => {
                let now = changed(&self.last, instances, size, &mut self.scratch);
                self.seen = match &now { Area::Rects(r) => r.iter().copied().reduce(join), Area::All => None };
                match now {
                    Area::Rects(r) if r.iter().map(|r| area(*r)).sum::<u32>() as f32 > MOST * (size.0 * size.1) as f32 => (Area::All, "most of it changed"),
                    Area::All => (Area::All, "reordered"),
                    other => (other, ""),
                }
            }
        };
        self.last.clear();
        self.last.extend_from_slice(instances);
        self.shape = Some((size, clear));
        match now {
            Area::All => Plan::All(why),
            // Nothing changed: one pixel keeps the present, which DWM composes again as is.
            Area::Rects(r) if r.is_empty() => Plan::Part { draw: vec![[0, 0, 1, 1]], present: vec![[0, 0, 1, 1]] },
            Area::Rects(r) => Plan::Part {
                draw: r.iter().map(|r| [r[0], r[1], r[2] - r[0], r[3] - r[1]]).collect(),
                present: r.iter().map(|r| r.map(|v| v as i32)).collect(),
            },
        }
    }

    /// A new swap chain: the next frame draws whole.
    pub(super) fn reset(&mut self) { self.shape = None; }
}
/// `RUNE_WHOLE_FRAMES=1`: every frame draws whole, to check partial frames against.
fn whole_frames() -> bool {
    static WHOLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WHOLE.get_or_init(|| std::env::var_os("RUNE_WHOLE_FRAMES").is_some_and(|v| v == "1"))
}

type Counts = HashMap<u64, (i32, usize), BuildHasherDefault<Hashed>>;

/// The map's keys are hashes already.
#[derive(Default)]
struct Hashed(u64);

impl Hasher for Hashed {
    fn finish(&self) -> u64 { self.0 }
    fn write(&mut self, _: &[u8]) { unreachable!("keys are u64") }
    fn write_u64(&mut self, v: u64) { self.0 = v; }
}

/// FNV-1a over an instance's 32 words: a few ns, against SipHash's ~100.
fn hash(i: &Instance) -> u64 {
    bytemuck::cast_ref::<Instance, [u32; 32]>(i).iter().fold(0xcbf2_9ce4_8422_2325u64, |h, w| (h ^ u64::from(*w)).wrapping_mul(0x0100_0000_01b3))
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
    for (i, h) in s.new.iter().enumerate() { s.counts.entry(*h).or_insert((0, old.len() + i)).0 -= 1; }
    // The instances both lists have, in order: drawn in another order, anything may have changed.
    let counts = &s.counts;
    if !s.old.iter().filter(|h| counts[h].0 == 0).eq(s.new.iter().filter(|h| counts[h].0 == 0)) { return Area::All; }
    // In reading order, so neighbours gather before rectangles run out.
    s.each.clear();
    s.each.extend(counts.values().filter(|(n, _)| *n != 0)
        .filter_map(|(_, at)| bounds(if *at < old.len() { &old[*at] } else { &new[*at - old.len()] }, size)));
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
    let kind = i.params[1];
    // A scope's end draws nothing.
    if (8.5..9.5).contains(&kind) { return None; }
    let grow = match kind {
        k if (0.5..1.5).contains(&k) => i.params[2] * 3.0 + 1.0,
        k if (1.5..2.5).contains(&k) => 0.0,
        k if (2.5..3.5).contains(&k) || (4.5..9.5).contains(&k) => 1.0,
        _ => 1.0 + i.params[2] * 2.0,
    };
    let [x, y, w, h] = i.rect;
    let (mut x0, mut y0, mut x1, mut y1) = (x - grow, y - grow, x + w + grow, y + h + grow);
    // A turned glyph (an icon's loop) reaches its quad's circle.
    if (1.5..2.5).contains(&kind) && i.params[0] != 0.0 {
        let (cx, cy, r) = (x + w * 0.5, y + h * 0.5, (w * w + h * h).sqrt() * 0.5);
        (x0, y0, x1, y1) = (cx - r, cy - r, cx + r, cy + r);
    }
    let c = i.clip;
    let (x0, y0) = ((x0.max(c[0]) - 1.0).floor().max(0.0), (y0.max(c[1]) - 1.0).floor().max(0.0));
    let (x1, y1) = ((x1.min(c[2]) + 1.0).ceil().min(size.0 as f32), (y1.min(c[3]) + 1.0).ceil().min(size.1 as f32));
    (x1 > x0 && y1 > y0).then(|| [x0 as u32, y0 as u32, x1 as u32, y1 as u32])
}

fn area(r: [u32; 4]) -> u32 { (r[2] - r[0]) * (r[3] - r[1]) }
