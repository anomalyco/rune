//! Paint: a subtree that would paint what it painted last time copies it instead.
//!
//! Each paint records, for every node, what its subtree emitted (its instances and their marks,
//! its hits, the atlas pages its glyphs sample, its glyph count) and what that depended on: the
//! node's commit epoch (a node built again from its element gets a new one; a node a region kept
//! keeps its own), the per-node rows paint reads (`row_dy`, `row_h`, `row_mask`), and the hash of
//! the state it inherited (`Inh`). The next paint copies a subtree whose nodes are the same ones,
//! in the same shape, none of whose scene values were presented again since, under the same
//! inherited state, in a window whose shared paint state (scale, viewport, blending, the glyph
//! atlas's key, focus) is the same. What paint would read elsewhere, or as time passes, keeps a
//! node from being copied (`static_ok`, `dynamic_ok`, and what the paint of it found).
//!
//! Debug builds paint a subtree they would copy and say on stderr where the copy would differ.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

static EPOCH: AtomicU32 = AtomicU32::new(1);

/// A new commit epoch for a node built from its element.
pub(super) fn next_epoch() -> u32 {
    EPOCH.fetch_add(1, Ordering::Relaxed)
}

const NONE: u32 = u32::MAX;

#[derive(Clone, Copy)]
pub(super) struct PaintRec {
    id: NodeId,
    epoch: u32,
    /// Its index less its parent's (0 for a root).
    up: u32,
    own: u64,
    inh: u64,
    /// One past its subtree's last node.
    end: u32,
    inst: [u32; 2],
    hits: [u32; 2],
    overlay: [u32; 2],
    help: [u32; 2],
    drop: [u32; 2],
    pages: [u32; 2],
    glyphs: [u32; 2],
    /// What its own fields say (`static_ok`), kept while its epoch is.
    fits: bool,
    keep: bool,
}

impl Default for PaintRec {
    fn default() -> Self {
        PaintRec { id: NodeId(u64::MAX), epoch: 0, up: 0, own: 0, inh: 0, end: 0, inst: [0; 2], hits: [0; 2], overlay: [0; 2], help: [0; 2], drop: [0; 2], pages: [0; 2], glyphs: [0; 2], fits: false, keep: false }
    }
}

#[derive(Default)]
pub(super) struct PaintKeep {
    recs: Vec<PaintRec>,
    prev: Vec<PaintRec>,
    instances: Vec<Instance>,
    marks: Vec<u32>,
    hits: Vec<(NodeId, [f32; 4])>,
    overlay: Vec<[f32; 4]>,
    help: Vec<(NodeId, [f32; 4], usize, Option<usize>)>,
    drop: Vec<(NodeId, [f32; 4], usize)>,
    drawn: Vec<[f32; 4]>,
    drawn_scale: Vec<f32>,
    seen: Vec<bool>,
    inh: Vec<Inh>,
    sticky_frame: Vec<[f32; 8]>,
    pages: Vec<(u16, u32)>,
    prev_pages: Vec<(u16, u32)>,
    /// Nodes presented again since the last paint.
    pub(super) moved: Vec<NodeId>,
    moved_at: Vec<bool>,
    /// The window's shared paint state at the last paint, when it could be kept from.
    key: Option<u64>,
    /// This paint copies what it can, and records what it paints (in tree order only).
    on: bool,
    recording: bool,
    /// Frames stepped, and the one the last paint ran in: a second paint in a frame paints all.
    pub(super) frame: u64,
    painted: u64,
    /// This paint's nodes: last paint's index of each, one past each subtree, whether each
    /// subtree is as it was.
    old: Vec<u32>,
    end: Vec<u32>,
    clean: Vec<bool>,
    fits: Vec<bool>,
    open: Vec<usize>,
    /// Debug builds: the last paint's index of a subtree painted to check its copy.
    verify: Vec<u32>,
    verify_span: (usize, usize),
    /// Nodes copied this paint, and why the others weren't as they were (`RUNE_PAINT_KEEP_LOG`).
    kept: usize,
    why: [usize; 6],
    /// The next paint paints everything (the raster check's fresh paint).
    pub(super) off_once: bool,
}

fn disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("RUNE_PAINT_KEEP").is_some_and(|v| v == "0"))
}

fn logging() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUNE_PAINT_KEEP_LOG").is_some_and(|v| v != "0"))
}

#[inline(always)]
fn bits(h: &mut rune_motion::Fx, v: &[f32]) {
    // Two values a word: half the hasher's rounds.
    let mut pairs = v.chunks_exact(2);
    for p in &mut pairs {
        h.write_u64(u64::from(p[0].to_bits()) | u64::from(p[1].to_bits()) << 32);
    }
    if let [x] = pairs.remainder() {
        h.write_u32(x.to_bits());
    }
}

/// The bits of what a node inherits.
pub(super) fn inh_key(p: &Inh) -> u64 {
    let mut h = rune_motion::Fx::default();
    bits(&mut h, &[p.opacity, p.paint_opacity, p.first_fade, p.scale, p.blur, p.dy, p.dx, p.group, p.hit_opacity]);
    for a in [p.clip, p.fade, p.fade_origin, p.paint_clip, p.paint_fade, p.paint_fade_origin, p.group_rect] {
        bits(&mut h, &a);
    }
    bits(&mut h, &p.anchor);
    bits(&mut h, &p.anchor_to);
    bits(&mut h, &p.cover);
    match p.rounded {
        Some((a, b)) => { h.write_u8(1); bits(&mut h, &a); bits(&mut h, &b); }
        None => h.write_u8(0),
    }
    match p.outer {
        Some(o) => { h.write_u8(1); for a in o { bits(&mut h, &a); } }
        None => h.write_u8(0),
    }
    h.write_u32(p.mark);
    h.write_u8(p.scale_motion as u8);
    bits(&mut h, &[p.still.0]);
    bits(&mut h, &p.still.1);
    bits(&mut h, &p.still.2);
    (p.in_cache, p.skip_paint, p.turn).hash(&mut h);
    p.snap.hash_bits(&mut h);
    h.finish()
}

impl<S: 'static> Runtime<S> {
    /// Before a paint clears them: last paint's output and side tables become the record copies
    /// come from.
    pub(super) fn keep_swap(&mut self) {
        let k = &mut self.paint_keep;
        std::mem::swap(&mut self.instances, &mut k.instances);
        // Both sides of the swap retain the warmed paint budget, even if a reveal
        // temporarily emits fewer glyphs on the frames painted into one of them.
        self.instances.clear();
        self.instances.reserve_exact(k.instances.capacity());
        std::mem::swap(&mut self.marks, &mut k.marks);
        std::mem::swap(&mut self.hits, &mut k.hits);
        std::mem::swap(&mut self.overlay_hits, &mut k.overlay);
        std::mem::swap(&mut self.help.hits, &mut k.help);
        std::mem::swap(&mut self.file_drag.hits, &mut k.drop);
        std::mem::swap(&mut self.drawn, &mut k.drawn);
        std::mem::swap(&mut self.drawn_scale, &mut k.drawn_scale);
        std::mem::swap(&mut self.seen, &mut k.seen);
        std::mem::swap(&mut self.inh_scratch, &mut k.inh);
        std::mem::swap(&mut self.sticky_frame, &mut k.sticky_frame);
        std::mem::swap(&mut k.recs, &mut k.prev);
        std::mem::swap(&mut k.pages, &mut k.prev_pages);
        k.pages.clear();
        k.recs.clear();
        k.recs.resize(self.nodes.len(), PaintRec::default());
        k.open.clear();
        k.kept = 0;
    }

    /// Whether this paint copies what it can, from the window's shared state, and if so which
    /// of its subtrees are as last paint left them.
    pub(super) fn keep_begin(&mut self, tree_order: bool, vp: [f32; 4], default_ring: Option<usize>, sel: &[(usize, usize, usize)], marks: &[(usize, usize, usize)]) {
        let second = self.paint_keep.painted == self.paint_keep.frame;
        self.paint_keep.painted = self.paint_keep.frame;
        // Paint goes depth first, siblings in paint order (`update_layer_paint_order`), so a subtree
        // paints in one run unless it holds a node lifted out to paint last.
        let lifted = !self.flying.is_empty() || self.has_raise;
        let shareable = !lifted && self.flights.is_empty() && self.paint_rank.is_none()
            && self.modal_scope.is_none() && !self.defaults.force_focus_rings && !disabled();
        let key = shareable.then(|| {
            let mut h = rune_motion::Fx::default();
            bits(&mut h, &[self.env.scale]);
            bits(&mut h, &vp);
            (self.srgb_blend, self.track_marks, self.reduced_motion, self.env.system_dark, self.text.emit_key(), self.has_sticky, tree_order).hash(&mut h);
            (self.focus.path(), self.focus.keyboard(), self.focus.shown()).hash(&mut h);
            default_ring.map(|i| self.nodes[i].id).hash(&mut h);
            let ring = self.default_focus_ring();
            bits(&mut h, &[ring.width, ring.offset, ring.color.r, ring.color.g, ring.color.b, ring.color.a]);
            h.finish()
        });
        if logging() && !shareable {
            eprintln!("rune: paint keep off: raise {}, text flights {}, flying {}, rank {}, modal {}", self.has_raise, self.flights.len(), self.flying.len(), self.paint_rank.is_some(), self.modal_scope.is_some());
        }
        let k = &mut self.paint_keep;
        k.recording = !lifted;
        k.on = key.is_some() && key == k.key && !second && !std::mem::take(&mut k.off_once) && !k.prev.is_empty();
        k.key = key;
        let n = self.nodes.len();
        k.verify.clear();
        k.verify.resize(n, NONE);
        k.verify_span = (0, 0);
        if !k.recording {
            return;
        }
        // Last paint's index of each node: in order, as kept regions leave them.
        k.old.clear();
        k.old.resize(n, NONE);
        let mut by_id: Option<HashMap<NodeId, u32>> = None;
        let mut cursor = 0usize;
        for i in 0..n {
            let id = self.nodes[i].id;
            let o = if k.prev.get(cursor).is_some_and(|r| r.id == id) { Some(cursor) } else {
                by_id.get_or_insert_with(|| k.prev.iter().enumerate().map(|(o, r)| (r.id, o as u32)).collect()).get(&id).map(|&o| o as usize)
            };
            if let Some(o) = o {
                k.old[i] = o as u32;
                cursor = o + 1;
            }
        }
        // What each node's own fields say, read again only for a node built again.
        k.fits.clear();
        for i in 0..n {
            let o = k.old[i];
            let fits = match k.prev.get(o as usize).filter(|r| o != NONE && r.epoch == self.nodes[i].epoch) {
                Some(r) => r.fits,
                None => static_ok(&self.nodes[i]),
            };
            k.fits.push(fits);
        }
        if !k.on {
            return;
        }
        k.moved_at.clear();
        k.moved_at.resize(n, false);
        for id in &k.moved {
            if let Some(&i) = self.node_index.get(id) && i < n {
                k.moved_at[i] = true;
            }
        }
        // One past each subtree, and whether each node is as it was.
        k.end.clear();
        k.end.resize(n, n as u32);
        k.clean.clear();
        k.clean.resize(n, false);
        k.open.clear();
        for i in 0..n {
            let parent = self.nodes[i].parent;
            let (own, fit) = (own_key(self, i), self.paint_keep.fits[i] && dynamic_ok(self, i, sel, marks));
            let k = &mut self.paint_keep;
            while let Some(&r) = k.open.last() && parent.is_none_or(|p| r > p) {
                k.end[r] = i as u32;
                k.open.pop();
            }
            k.open.push(i);
            let o = k.old[i];
            if logging() {
                let why = if !fit { 0 } else if o == NONE { 1 } else if k.moved_at[i] { 2 } else if k.prev[o as usize].epoch != self.nodes[i].epoch { 3 } else if k.prev[o as usize].own != own { 4 } else { 5 };
                k.why[why] += 1;
                if (why == 2 || why == 3) && std::env::var_os("RUNE_PAINT_KEEP_LOG").is_some_and(|v| v == "2") {
                    eprintln!("rune: paint keep:   {} {}", if why == 2 { "moved" } else { "rebuilt" }, self.nodes[i].path);
                }
            }
            k.clean[i] = fit && o != NONE && !k.moved_at[i] && {
                let r = &k.prev[o as usize];
                let up = parent.map_or(0, |p| (i - p) as u32);
                r.epoch == self.nodes[i].epoch && r.up == up && r.own == own
                    && parent.is_none_or(|p| k.old[p] != NONE && k.old[p] + up == o)
            };
        }
        let k = &mut self.paint_keep;
        k.open.clear();
        for i in (1..n).rev() {
            if !k.clean[i] && let Some(p) = self.nodes[i].parent {
                k.clean[p] = false;
            }
        }
    }

    /// Opens node `i`'s record, before it paints anything.
    pub(super) fn keep_open(&mut self, i: usize, glyphs: usize, sel: &[(usize, usize, usize)], marks: &[(usize, usize, usize)]) {
        if !self.paint_keep.recording { return; }
        let rec = PaintRec {
            id: self.nodes[i].id,
            epoch: self.nodes[i].epoch,
            up: self.nodes[i].parent.map_or(0, |p| (i - p) as u32),
            own: own_key(self, i),
            inh: 0,
            end: 0,
            inst: [self.instances.len() as u32; 2],
            hits: [self.hits.len() as u32; 2],
            overlay: [self.overlay_hits.len() as u32; 2],
            help: [self.help.hits.len() as u32; 2],
            drop: [self.file_drag.hits.len() as u32; 2],
            pages: [self.paint_keep.pages.len() as u32; 2],
            glyphs: [glyphs as u32; 2],
            fits: self.paint_keep.fits[i],
            keep: self.paint_keep.fits[i] && dynamic_ok(self, i, sel, marks),
        };
        self.paint_keep.recs[i] = rec;
        self.paint_keep.open.push(i);
    }

    /// Node `i` inherits `pinh`: copies its subtree from the last paint when that's what it would
    /// paint, and returns how many nodes it holds.
    pub(super) fn keep_or_paint(&mut self, i: usize, pinh: &Inh, glyph_count: &mut usize) -> Option<usize> {
        // Only a node that can be copied needs what it inherits.
        if !self.paint_keep.recording || !self.paint_keep.recs[i].keep { return None; }
        let inh = inh_key(pinh);
        self.paint_keep.recs[i].inh = inh;
        let k = &self.paint_keep;
        if !k.on || !k.clean[i] {
            return None;
        }
        let o = k.old[i] as usize;
        let r = k.prev[o];
        let end = k.end[i] as usize;
        if !r.keep || r.inh != inh || r.end as usize - o != end - i {
            return None;
        }
        if cfg!(debug_assertions) {
            // Checked as one: what it holds paints as it does, inside it.
            let (a, b) = self.paint_keep.verify_span;
            if !(a..b).contains(&i) {
                self.paint_keep.verify[i] = o as u32;
                self.paint_keep.verify_span = (i, end);
                self.paint_keep.kept += end - i;
            }
            return None;
        }
        self.keep_copy(i, o, end, glyph_count);
        Some(end - i)
    }

    fn keep_copy(&mut self, i: usize, o: usize, end: usize, glyph_count: &mut usize) {
        let k = &mut self.paint_keep;
        k.open.pop();
        let r = k.prev[o];
        let range = |a: [u32; 2]| a[0] as usize..a[1] as usize;
        let at = [self.instances.len(), self.hits.len(), self.overlay_hits.len(), k.pages.len()];
        self.instances.extend_from_slice(&k.instances[range(r.inst)]);
        if self.track_marks {
            self.marks.resize(at[0], 0);
            self.marks.extend_from_slice(&k.marks[range(r.inst)]);
        }
        self.hits.extend_from_slice(&k.hits[range(r.hits)]);
        self.overlay_hits.extend_from_slice(&k.overlay[range(r.overlay)]);
        let seq = |s: usize| s + at[1] - r.hits[0] as usize;
        let help: Vec<_> = k.help[range(r.help)].iter().map(|&(id, rect, s, l)| (id, rect, seq(s), l)).collect();
        let drop: Vec<_> = k.drop[range(r.drop)].iter().map(|&(id, rect, s)| (id, rect, seq(s))).collect();
        let (at_help, at_drop) = (self.help.hits.len(), self.file_drag.hits.len());
        self.help.hits.extend(help);
        self.file_drag.hits.extend(drop);
        let k = &mut self.paint_keep;
        let pages = range(r.pages);
        k.pages.extend_from_slice(&k.prev_pages[pages.clone()]);
        for &(page, texels) in &k.prev_pages[pages] {
            self.text.touch_quad(page, texels);
        }
        *glyph_count += (r.glyphs[1] - r.glyphs[0]) as usize;
        let len = end - i;
        self.drawn[i..end].copy_from_slice(&k.drawn[o..o + len]);
        self.drawn_scale[i..end].copy_from_slice(&k.drawn_scale[o..o + len]);
        self.seen[i..end].copy_from_slice(&k.seen[o..o + len]);
        self.inh_scratch[i..end].copy_from_slice(&k.inh[o..o + len]);
        if self.has_sticky {
            self.sticky_frame[i..end].copy_from_slice(&k.sticky_frame[o..o + len]);
        }
        // Records for the next paint, as if it had painted them here.
        let shift = |a: [u32; 2], from: u32, to: usize| a.map(|v| v - from + to as u32);
        let glyphs = *glyph_count as u32 - (r.glyphs[1] - r.glyphs[0]);
        for d in 0..len {
            let p = k.prev[o + d];
            k.recs[i + d] = PaintRec {
                end: p.end - o as u32 + i as u32,
                inst: shift(p.inst, r.inst[0], at[0]),
                hits: shift(p.hits, r.hits[0], at[1]),
                overlay: shift(p.overlay, r.overlay[0], at[2]),
                help: shift(p.help, r.help[0], at_help),
                drop: shift(p.drop, r.drop[0], at_drop),
                pages: shift(p.pages, r.pages[0], at[3]),
                glyphs: shift(p.glyphs, r.glyphs[0], glyphs as usize),
                ..p
            };
        }
        k.kept += len;
    }

    /// Node `i` painted itself (not yet its children): what it emitted, and what painting it
    /// found, say whether it can be copied.
    pub(super) fn keep_painted(&mut self, i: usize, painted_ok: bool) {
        let k = &mut self.paint_keep;
        if !k.recording { return; }
        let start = k.recs[i].inst[0] as usize;
        let mut ok = painted_ok;
        for q in &self.instances[start..] {
            let kind = q.params[1];
            if kind == KIND_IMAGE || kind == KIND_EFFECT || kind == crate::render::KIND_BACKDROP_BLUR || (kind == KIND_GLYPH && q.radii[3] > 0.0) {
                ok = false;
            } else if kind == KIND_GLYPH {
                let (page, texels) = (q.params[2] as u16, (q.rect[2] * if q.radii[0] == 1.0 { 3.0 } else { 1.0 } * q.rect[3]) as u32);
                let from = k.recs[i].pages[0] as usize;
                match k.pages[from..].iter_mut().find(|p| p.0 == page) {
                    Some(p) => p.1 += texels,
                    None => k.pages.push((page, texels)),
                }
            }
        }
        k.recs[i].keep &= ok;
    }

    /// Closes the records of the open subtrees node `next`'s parent isn't inside (all of them
    /// for None), with the layers they opened.
    pub(super) fn keep_close(&mut self, parent: Option<Option<usize>>, layers: &mut Vec<(usize, usize, usize)>, active_cache: &mut Option<ActiveCache>, glyph_count: &mut usize) {
        if !self.paint_keep.recording { return; }
        while let Some(&r) = self.paint_keep.open.last() {
            if let Some(Some(p)) = parent && r <= p {
                break;
            }
            while let Some(&(owner, start, surface_start)) = layers.last() {
                if owner < r { break; }
                layers.pop();
                let before = self.instances.len();
                if active_cache.is_some_and(|ac| ac.instance_start == start) {
                    let ac = active_cache.take().unwrap();
                    self.close_cached_layer(ac, start, surface_start, glyph_count);
                } else {
                    crate::render::end_layer(&mut self.instances, &mut self.surfaces, &mut self.marks, start, surface_start);
                }
                // A scope folded into its one primitive took its marker out: what its subtree
                // recorded after the marker moved down one.
                if self.instances.len() < before {
                    for rec in &mut self.paint_keep.recs[owner + 1..] {
                        for v in rec.inst.iter_mut() {
                            if *v as usize > start { *v -= 1; }
                        }
                    }
                } else if self.instances.len() != before + 1 {
                    // A cached scope elided or replaced what it drew.
                    self.paint_keep.recs[owner].keep = false;
                }
            }
            self.paint_keep.open.pop();
            let (help, drop) = (self.help.hits.len() as u32, self.file_drag.hits.len() as u32);
            let k = &mut self.paint_keep;
            let rec = &mut k.recs[r];
            (rec.help[1], rec.drop[1]) = (help, drop);
            rec.inst[1] = self.instances.len() as u32;
            rec.hits[1] = self.hits.len() as u32;
            rec.overlay[1] = self.overlay_hits.len() as u32;
            rec.pages[1] = k.pages.len() as u32;
            rec.glyphs[1] = *glyph_count as u32;
            let keep = rec.keep;
            if let Some(p) = self.nodes[r].parent { k.recs[p].keep &= keep; }
            if cfg!(debug_assertions) && self.paint_keep.verify.get(r).is_some_and(|&o| o != NONE) {
                self.keep_check(r);
            }
        }
    }


    /// The end of a paint: what was moved is painted, and what wasn't kept is said.
    pub(super) fn keep_end(&mut self) {
        let k = &mut self.paint_keep;
        k.moved.clear();
        if !k.recording {
            k.recs.clear();
            return;
        }
        // Subtree ends, from the tree: a record holds one past its subtree.
        let n = self.nodes.len();
        let open = &mut k.open;
        debug_assert!(open.is_empty(), "paint closed every subtree");
        for i in 0..n {
            let parent = self.nodes[i].parent;
            while let Some(&r) = open.last() && parent.is_none_or(|p| r > p) {
                k.recs[r].end = i as u32;
                open.pop();
            }
            open.push(i);
        }
        for r in open.drain(..) { k.recs[r].end = n as u32; }
        k.moved.clear();
        if logging() {
            eprintln!("rune: paint keep: {} of {} nodes copied (on: {}; unfit {}, new {}, moved {}, rebuilt {}, rows {}, as they were {})", k.kept, n, k.on, k.why[0], k.why[1], k.why[2], k.why[3], k.why[4], k.why[5]);
            k.why = [0; 6];
        }
    }

    /// Debug builds: subtree `r` painted what its copy from the last paint would have.
    fn keep_check(&mut self, r: usize) {
        let k = &self.paint_keep;
        let o = k.verify[r] as usize;
        let (now, was) = (k.recs[r], k.prev[o]);
        let range = |a: [u32; 2]| a[0] as usize..a[1] as usize;
        let bytes = bytemuck::cast_slice::<Instance, u8>;
        let mut differ = Vec::new();
        if bytes(&self.instances[range(now.inst)]) != bytes(&k.instances[range(was.inst)]) { differ.push("instances"); }
        if self.hits[range(now.hits)] != k.hits[range(was.hits)] { differ.push("hits"); }
        let rebase = |v: &[(NodeId, [f32; 4], usize, Option<usize>)], from: u32| v.iter().map(|&(id, rect, s, l)| (id, rect, s - from as usize, l)).collect::<Vec<_>>();
        if rebase(&self.help.hits[range(now.help)], now.hits[0]) != rebase(&k.help[range(was.help)], was.hits[0]) { differ.push("help hits"); }
        let rebase = |v: &[(NodeId, [f32; 4], usize)], from: u32| v.iter().map(|&(id, rect, s)| (id, rect, s - from as usize)).collect::<Vec<_>>();
        if rebase(&self.file_drag.hits[range(now.drop)], now.hits[0]) != rebase(&k.drop[range(was.drop)], was.hits[0]) { differ.push("drop hits"); }
        if self.track_marks && self.marks.get(range(now.inst)) != k.marks.get(range(was.inst)) { differ.push("marks"); }
        let len = k.end[r] as usize - r;
        if self.drawn[r..r + len] != k.drawn[o..o + len] || self.drawn_scale[r..r + len] != k.drawn_scale[o..o + len] || self.seen[r..r + len] != k.seen[o..o + len] { differ.push("drawn rects"); }
        if (0..len).any(|d| inh_key(&self.inh_scratch[r + d]) != inh_key(&k.inh[o + d])) { differ.push("inherited state"); }
        if self.has_sticky && self.sticky_frame[r..r + len] != k.sticky_frame[o..o + len] { differ.push("sticky frames"); }
        if self.overlay_hits[range(now.overlay)] != k.overlay[range(was.overlay)] { differ.push("overlay hits"); }
        if now.glyphs[1] - now.glyphs[0] != was.glyphs[1] - was.glyphs[0] { differ.push("glyph count"); }
        if k.pages[range(now.pages)] != k.prev_pages[range(was.pages)] { differ.push("atlas pages"); }
        if !differ.is_empty() {
            eprintln!("rune: paint keep: {} would have copied what differs: {}", self.nodes[r].path, differ.join(", "));
        }
    }
}

/// The per-node rows paint reads besides the node.
fn own_key<S: 'static>(rt: &Runtime<S>, i: usize) -> u64 {
    let mut h = rune_motion::Fx::default();
    bits(&mut h, &[rt.row_dy[i], rt.row_h[i], rt.row_mask[i]]);
    h.write_u8((rt.has_dissolve && rt.dissolve_scratch.get(i).copied().unwrap_or(false)) as u8);
    h.finish()
}

/// What a node's own fields say: paint reads nothing for it that changes without a new epoch
/// or a scene value.
fn static_ok(n: &Node) -> bool {
    n.input.is_none() && !n.ime_cursor && n.scroll.is_none() && n.looping.is_none() && n.live.is_empty()
        && n.vis.surface.is_none() && n.material.is_none() && !n.vis_flags.cache() && !n.vis_flags.raise()
        && n.vis.backdrop_blur <= 0.0 && n.vis.effect.is_none() && n.vis.pixels.is_none()
        && n.sticky.is_none() && n.above.is_none() && n.matched.is_none()
        && n.static_scale == 1.0 && !n.scale_track && n.vis.mark == 0
        && n.text.as_ref().is_none_or(|t| t.cold.links.is_empty() && t.cold.live.is_none() && t.cold.advance_in.is_none() && t.reveal < 0.0)
}

/// What the runtime holds for a node besides it: selection, marks, reveals, loops, scales
/// settling and folds.
fn dynamic_ok<S: 'static>(rt: &Runtime<S>, i: usize, sel: &[(usize, usize, usize)], marks: &[(usize, usize, usize)]) -> bool {
    let id = rt.nodes[i].id;
    !sel.iter().any(|r| r.0 == i) && !marks.iter().any(|r| r.0 == i)
        && (rt.reveals.is_empty() || !rt.reveals.contains_key(&id))
        && (rt.loops.is_empty() || !rt.loops.contains_key(&id))
        && (rt.static_scales.is_empty() || !rt.static_scales.contains_key(&id))
        && rt.row_mask[i] < 0.0
}
