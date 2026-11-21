//! A loop-only step for a window that slides over content sliding back inside it, as a text
//! shimmer is made (`El::clip_fade` on a `LoopSpec::slide`, whose only child slides the other
//! way): the content stays where it is, so a step moves only the window's clip and its soft
//! edges in the instances under it. The ordinary paint records each such window (`note_slide`),
//! the glyphs under it it drew or left out, and whether each node under it showed. A step that
//! would move the content, show or hide anything, or change what the paint drew there any other
//! way takes an ordinary frame.

use std::ops::Range;

use super::*;

pub(in crate::runtime) struct Slide {
    pub(super) window: usize,
    pub(super) counter: usize,
    /// One past the window's subtree, in node order.
    pub(super) end: usize,
    pub(super) instances: Range<usize>,
    /// The window's anchor (`Anchor`: a rect in points, its ancestors' scale applied, at a slide
    /// offset), the offset this paint drew it at, and that offset with its child's, which cancels it.
    rect: [f32; 4],
    offset: [f32; 2],
    /// Its top and bottom (pixels), snapped as a still box's.
    y: [f32; 2],
    painted: [f32; 2],
    net: [f32; 2],
    /// The rect's points per offset point (an ancestor's scale), and the display scale.
    k: f32,
    s: f32,
    /// Its laid-out size on the grid, which paint lands a rect's size on.
    size: [f32; 2],
    /// The clip, fade widths and fade origins it inherits, and its own fade widths (pixels).
    outer: [[f32; 4]; 3],
    own: [f32; 4],
    vp: [f32; 4],
    /// Each node under it: where it showed (pixels) and whether it did.
    shown: Vec<([f32; 4], bool)>,
    /// Each glyph under it (x0, y0, x1, y1) and whether it drew.
    pub(super) glyphs: Vec<([f32; 4], bool)>,
    /// Its instances are what this record says they are: checked on the first step.
    checked: Option<bool>,
}

/// What a step draws the window's instances with: their clip and packed fade.
type Pose = ([f32; 4], [u32; 4]);

/// Where a sliding window is posed from, kept across paints while its laid-out place holds: its
/// rect (points) at a slide offset, and the rect's points per offset point, scale and grid size.
/// Paint and a loop-only step both pose the window from it at the offset they draw (`pose_rect`),
/// so the two reach one number for the same offset. Posed from each paint's own rect instead,
/// they would reach it by different float sums (the scene's layout origin plus offset, then the
/// ancestors' translation; against the last paint's rect plus the offset's change), which land a
/// grid step apart where the offset sits on a grid line's half.
#[derive(Clone, Copy, PartialEq)]
pub(in crate::runtime) struct Anchor {
    node: rune_motion::NodeId,
    rect: [f32; 4],
    offset: [f32; 2],
    k: f32,
    s: f32,
    size: [f32; 2],
    /// Its top and bottom as paint snapped them (pixels): across the slide it sits where a still
    /// box does, on whole pixels, as on every other platform.
    y: [f32; 2],
}

/// The window's rect in pixels at slide offset `ow` (points), posed from `a`.
fn pose_rect(a: &Anchor, ow: [f32; 2]) -> [f32; 4] { posed(a.rect, a.offset, a.k, a.s, a.size, a.y, ow) }

/// Along the slide, on paint's 1/64-pixel grid, moved by the offset's change from `at` (points);
/// across it, `y`.
fn posed(at: [f32; 4], offset: [f32; 2], k: f32, s: f32, size: [f32; 2], y: [f32; 2], ow: [f32; 2]) -> [f32; 4] {
    let d = [(ow[0] - offset[0]) * k, (ow[1] - offset[1]) * k];
    let mut rect = [at[0] + d[0], at[1] + d[1], at[2] + d[0], at[3] + d[1]];
    for axis in 0..2 {
        if ((rect[axis + 2] - rect[axis]) - size[axis]).abs() * s < 1.0 / 32.0 {
            rect[axis + 2] = rect[axis] + size[axis];
        }
    }
    let r = rect.map(|v| grid(v, s) * s);
    [r[0], y[0], r[2], y[1]]
}

/// One past node `i`'s subtree: nodes go depth first, so the first node after it whose parent
/// comes before `i`.
fn subtree_end(nodes: &[Node], i: usize) -> usize {
    (i + 1..nodes.len()).find(|&j| nodes[j].parent.is_none_or(|p| p < i)).unwrap_or(nodes.len())
}

fn slides(spec: Option<&rune_motion::LoopSpec>) -> bool {
    matches!(spec.map(|l| l.kind), Some(rune_motion::LoopKind::Slide { .. } | rune_motion::LoopKind::SlideEased { .. }))
}

/// A slide that moves only sideways. Text paints only the lines in its clip's band, plus two ems
/// either side (`content.rs`), so a window moving up or down could uncover a line paint never
/// drew, which no step can draw.
fn sideways(spec: Option<&rune_motion::LoopSpec>) -> bool {
    matches!(spec.map(|l| l.kind), Some(rune_motion::LoopKind::Slide { from, to } | rune_motion::LoopKind::SlideEased { from, to, .. }) if from[1] == 0.0 && to[1] == 0.0)
}

/// Paint's test for whether a box at `r` (x0, y0, x1, y1) under `clip` shows (`build`).
fn shows(r: [f32; 4], clip: [f32; 4], vp: [f32; 4]) -> bool {
    let m = 2.0;
    clip[2] > clip[0] && clip[3] > clip[1]
        && r[2] >= clip[0].max(vp[0]) - m && r[0] <= clip[2].min(vp[2]) + m
        && r[3] >= clip[1].max(vp[1]) - m && r[1] <= clip[3].min(vp[3]) + m
}

impl paint_state::PaintState {
    /// Node `i`'s loop glides: its edges land where the slide puts them, not on whole pixels, as
    /// a browser's compositor moves a transformed layer, so that a loop-only step (which moves
    /// only the clip and soft edges) and an ordinary frame draw the same picture. Only for what a
    /// step can move, on the platform that steps it (Windows): a window sliding sideways over
    /// content that slides back inside it, and that content. What the content holds still snaps.
    /// Any other slide, and every slide elsewhere, snaps as before.
    #[inline]
    pub(in crate::runtime::paint) fn slide_glides(
        nodes: &[Node],
        motion: &crate::runtime::tracks::MotionState,
        reduced_motion: bool,
        i: usize,
    ) -> bool {
        if !cfg!(target_os = "windows") || reduced_motion { return false; }
        let n = &nodes[i];
        if !slides(n.looping.as_deref()) || !motion.has_loop(n.id) { return false; }
        Self::slide_pair(nodes, motion, i) || n.parent.is_some_and(|p| p + 1 == i && Self::slide_pair(nodes, motion, p))
    }

    /// Node `i` is a window a step could move, by its shape alone: it clips and slides sideways,
    /// and its only child holds everything under it and slides back, sideways. Shape only, decided
    /// before paint: a pair `slide_fits` then refuses (in a layer, over input, out of sight) still
    /// glides, drawn by ordinary frames, so its picture doesn't change with whether a step ran.
    #[inline]
    pub(in crate::runtime::paint) fn slide_pair(
        nodes: &[Node],
        motion: &crate::runtime::tracks::MotionState,
        i: usize,
    ) -> bool {
        let n = &nodes[i];
        let (counter, end) = (i + 1, subtree_end(nodes, i));
        n.vis.clip.is_some() && counter < end && subtree_end(nodes, counter) == end
            && sideways(n.looping.as_deref()) && sideways(nodes[counter].looping.as_deref())
            && motion.has_loop(n.id) && motion.has_loop(nodes[counter].id)
    }

    /// Paint reached node `i`: a window whose subtree it has passed is complete.
    pub(super) fn slide_node_opens(&mut self, i: usize, instances_len: usize, scale: f32) {
        let c = &mut self.keep.cycles;
        c.opened = (i, instances_len);
        // A subtree paints in one run, in tree order or the layers' (`update_layer_paint_order`),
        // as long as nothing in it is lifted or raised out to paint later.
        if c.open.is_some_and(|k| i < c.slides[k].window || i >= c.slides[k].end) {
            self.close_slide(instances_len, scale);
        }
    }

    /// The rect, in pixels, that sliding window `i` clips and fades what it holds with: drawn at
    /// `exact` (`rect` in points, `k` points per offset point) and snapped to `r` as a still box
    /// is. Across the slide, `r`'s top and bottom; along it, posed from its anchor, kept from an
    /// earlier paint while its top and bottom hold and this paint's exact edges land within a grid
    /// step of the anchor's pose (its laid-out place held), else anchored here. Any other node
    /// keeps `r`.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::runtime::paint) fn slide_anchor(
        &mut self,
        nodes: &[Node],
        motion: &crate::runtime::tracks::MotionState,
        scene: &Scene,
        s: f32,
        reduced_motion: bool,
        i: usize,
        rect: [f32; 4],
        exact: [f32; 4],
        r: [f32; 4],
        k: f32,
    ) -> [f32; 4] {
        if !Self::slide_glides(nodes, motion, reduced_motion, i) || !Self::slide_pair(nodes, motion, i) { return r; }
        let n = &nodes[i];
        let ow = scene.get(n.id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
        let ow = [ow.x, ow.y];
        let here = Anchor { node: n.id, rect, offset: ow, k, s, size: [grid(n.local.w, s), grid(n.local.h, s)], y: [r[1], r[3]] };
        let c = &mut self.keep.cycles;
        let kept = c.anchors.iter().copied().find(|a| a.node == here.node && (a.k, a.s, a.size, a.y) == (here.k, here.s, here.size, here.y)
            && { let p = pose_rect(a, ow); [0, 2].iter().all(|&e| (p[e] - exact[e]).abs() <= 1.0 / 64.0 + 1e-4) });
        let a = kept.unwrap_or(here);
        c.next_anchors.push(a);
        c.anchor = Some((i, a));
        pose_rect(&a, ow)
    }

    /// Node `i` clipped what it holds to `r` under `outer` (clip, fade widths, fade origins),
    /// from `rect` (points) at `k` points of rect per point of offset and ancestors' scale `sc`:
    /// recorded if it is a sliding window a loop-only step can move.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::runtime::paint) fn note_slide(
        &mut self,
        nodes: &[Node],
        motion: &crate::runtime::tracks::MotionState,
        scene: &Scene,
        instances_len: usize,
        s: f32,
        viewport: (f32, f32),
        reduced_motion: bool,
        root_inh: Inh,
        i: usize,
        outer: [[f32; 4]; 3],
        rect: [f32; 4],
        r: [f32; 4],
        (k, sc): (f32, f32),
    ) {
        if !Self::slide_glides(nodes, motion, reduced_motion, i) { return; }
        let vp = [0.0, 0.0, viewport.0 * s, viewport.1 * s];
        let (counter, end, f) = match self.slide_fits(nodes, motion, scene, instances_len, root_inh, i, r, outer[0], vp) {
            Ok(fits) => fits,
            Err(why) => { self.keep.cycles.missed = Some(why); return; }
        };
        let n = &nodes[i];
        let off = |j: usize| scene.get(nodes[j].id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
        let (ow, oc) = (off(i), off(counter));
        // Posed from its anchor (`slide_anchor`, this paint's), else from this paint's own rect.
        let (rect, offset, y) = match self.keep.cycles.anchor { Some((j, a)) if j == i => (a.rect, a.offset, a.y), _ => (rect, [ow.x, ow.y], [r[1], r[3]]) };
        let slide = Slide {
            window: i,
            counter,
            end,
            instances: instances_len..instances_len,
            rect,
            offset,
            y,
            painted: [ow.x, ow.y],
            net: [ow.x + oc.x, ow.y + oc.y],
            k,
            s,
            size: [grid(n.local.w, s), grid(n.local.h, s)],
            outer,
            own: f.map(|v| v * sc * s),
            vp,
            shown: Vec::new(),
            glyphs: Vec::new(),
            checked: None,
        };
        let c = &mut self.keep.cycles;
        c.open = Some(c.slides.len());
        c.slides.push(slide);
    }

    /// Whether sliding node `i`, drawn at `r` under `clip`, is a window a step can move: its only
    /// child slides back and holds everything under it, nothing under it takes input, clips,
    /// blurs, sticks or moves on its own, and it shows. Its child, one past its subtree, and its
    /// fade widths; or why not.
    #[allow(clippy::too_many_arguments)]
    fn slide_fits(
        &self,
        nodes: &[Node],
        motion: &crate::runtime::tracks::MotionState,
        scene: &Scene,
        instances_len: usize,
        root_inh: Inh,
        i: usize,
        r: [f32; 4],
        clip: [f32; 4],
        vp: [f32; 4],
    ) -> Result<(usize, usize, [f32; 4]), &'static str> {
        let c = &self.keep.cycles;
        if !motion.flying.is_empty() { return Err("a slide while something flies"); }
        if c.open.is_some() { return Err("a slide inside a slide"); }
        if c.opened != (i, instances_len) { return Err("a slide that draws itself"); }
        let n = &nodes[i];
        let (counter, end) = (i + 1, subtree_end(nodes, i));
        let Some(f) = n.vis.clip else { return Err("a slide that doesn't clip") };
        if counter >= end || subtree_end(nodes, counter) != end || !slides(nodes[counter].looping.as_deref())
            || !motion.has_loop(nodes[counter].id) { return Err("a slide whose child doesn't slide back"); }
        if !sideways(n.looping.as_deref()) || !sideways(nodes[counter].looping.as_deref()) { return Err("a slide up or down"); }
        if n.scroll.is_some() || self.row_h[i] >= 0.0 || self.row_mask[i] >= 0.0 { return Err("a slide that scrolls or folds"); }
        let parent = n.parent.map_or(root_inh, |p| self.inh_scratch[p]);
        if parent.in_cache || parent.skip_paint || parent.turn != 0 || parent.rounded.is_some() || parent.blur != 0.0 { return Err("a slide in a layer"); }
        let plain = (i..end).all(|j| {
            let m = &nodes[j];
            m.handlers.get().is_none() && m.focus.get().is_none() && !m.vis_flags.hit() && m.input.is_none() && m.sticky.is_none()
                && m.vis.effect.is_none() && m.help.is_none() && m.above.is_none() && m.matched.is_none() && !m.vis_flags.raise()
        });
        if !plain { return Err("a slide over input"); }
        let still = (i..end).all(|j| {
            let m = &nodes[j];
            scene.presented(m.id).is_some_and(|p| p.blur == 0.0 && p.mask.is_none() && p.transform.rotate == 0.0)
                && (j == i || (m.vis.clip.is_none() && m.scroll.is_none() && (j == counter || m.looping.is_none())))
        });
        if !still { return Err("a slide over clips or motion"); }
        if !shows([r[0], r[1], r[2].max(r[0]), r[3].max(r[1])], clip, vp) { return Err("a slide out of sight"); }
        Ok((counter, end, f))
    }

    /// The open window's subtree is painted: its instances, and what each node under it showed.
    pub(super) fn close_slide(&mut self, len: usize, s: f32) {
        let Some(k) = self.keep.cycles.open.take() else { return };
        let (drawn, seen) = (&self.drawn, &self.seen);
        let sl = &mut self.keep.cycles.slides[k];
        sl.instances.end = len;
        sl.shown = (sl.window + 1..sl.end)
            .map(|j| (drawn.get(j).map_or([0.0; 4], |d| [d[0] * s, d[1] * s, (d[0] + d[2]) * s, (d[1] + d[3]) * s]), seen.get(j).copied().unwrap_or(false)))
            .collect();
    }

    /// Where window `sl` would clip and fade its content with its own offset at `ow` (points):
    /// None when the step can't move it alone (the content would move, or something under it
    /// would show or hide, or a glyph draw or not).
    fn slide_pose(sl: &Slide, ow: [f32; 2]) -> Option<Pose> {
        let r = posed(sl.rect, sl.offset, sl.k, sl.s, sl.size, sl.y, ow);
        if !shows([r[0], r[1], r[2].max(r[0]), r[3].max(r[1])], sl.outer[0], sl.vp) { return None; }
        let clip = intersect(r, sl.outer[0]);
        let (mut fade, mut origin) = (sl.outer[1], sl.outer[2]);
        inherit_fade(&mut fade, &mut origin, r, sl.own);
        if sl.shown.iter().any(|&(at, was)| shows(at, clip, sl.vp) != was) { return None; }
        if sl.glyphs.iter().any(|&(q, drew)| (q[1] > clip[3] || q[3] < clip[1] || q[0] > clip[2] || q[2] < clip[0]) == drew) { return None; }
        Some((clip, crate::render::pack_fade(fade, origin, clip)))
    }

    /// The pose each window's instances take at the loops' offsets at animation time `t`, or
    /// why a step can't draw one of them.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn slide_poses(
        &mut self,
        nodes: &[Node],
        motion: &crate::runtime::tracks::MotionState,
        scene: &Scene,
        instances: &[Instance],
        now_t: f64,
        reduced_motion: bool,
        t: f64,
    ) -> Result<Vec<Pose>, &'static str> {
        let at = |j: usize| -> Option<[f32; 2]> {
            let n = &nodes[j];
            let store = scene.store();
            let time = store.node_time(n.id) + (t - now_t) * f64::from(store.node_time_scale(n.id));
            let [x, y] = motion.loop_offset(n.id, time, reduced_motion)?;
            Some([x * n.local.w, y * n.local.h])
        };
        let mut poses = Vec::with_capacity(self.keep.cycles.slides.len());
        for k in 0..self.keep.cycles.slides.len() {
            if !self.slide_checked(k, instances) { return Err("a slide drew otherwise"); }
            let sl = &self.keep.cycles.slides[k];
            let (ow, oc) = at(sl.window).zip(at(sl.counter)).ok_or("a slide stopped")?;
            // The content holds still only while the two offsets cancel as they did.
            if (ow[0] + oc[0] - sl.net[0]).abs() > 1e-3 || (ow[1] + oc[1] - sl.net[1]).abs() > 1e-3 { return Err("a slide's content moves"); }
            poses.push(Self::slide_pose(sl, ow).ok_or("a slide shows or hides something")?);
        }
        Ok(poses)
    }

    /// Window `k`'s instances are what its record says: the pose it was painted at, the glyphs
    /// it drew, no layer. Asked once a paint.
    fn slide_checked(&mut self, k: usize, instances: &[Instance]) -> bool {
        let sl = &self.keep.cycles.slides[k];
        if let Some(ok) = sl.checked { return ok; }
        let range = sl.instances.clone();
        let ok = range.end <= instances.len() && Self::slide_pose(sl, sl.painted).is_some_and(|(clip, fade)| {
            let here = &instances[range.clone()];
            here.iter().all(|q| q.clip == clip && q.fade == fade && q.params[1] != crate::render::KIND_LAYER && q.params[1] != crate::render::KIND_LAYER_END)
                && here.iter().filter(|q| q.params[1] == KIND_GLYPH).count() == sl.glyphs.iter().filter(|g| g.1).count()
        });
        self.keep.cycles.slides[k].checked = Some(ok);
        ok
    }

    /// Draws each window's instances at its pose; returns the instances it wrote.
    pub(super) fn apply_slides(&self, poses: &[Pose], instances: &mut [Instance], patched: &mut Vec<u32>) {
        for (sl, &(clip, fade)) in self.keep.cycles.slides.iter().zip(poses) {
            for at in sl.instances.clone() {
                let q = &mut instances[at];
                q.clip = clip;
                q.fade = fade;
                patched.push(at as u32);
            }
        }
    }
}
