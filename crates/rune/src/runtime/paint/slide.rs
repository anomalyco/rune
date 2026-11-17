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
    /// The window's rect (points, its ancestors' scale applied) and slide offset as painted,
    /// and its offset with its child's, which cancels it.
    rect: [f32; 4],
    offset: [f32; 2],
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

/// One past node `i`'s subtree: nodes go depth first, so the first node after it whose parent
/// comes before `i`.
fn subtree_end(nodes: &[Node], i: usize) -> usize {
    (i + 1..nodes.len()).find(|&j| nodes[j].parent.is_none_or(|p| p < i)).unwrap_or(nodes.len())
}

fn slides(spec: Option<&rune_motion::LoopSpec>) -> bool {
    matches!(spec.map(|l| l.kind), Some(rune_motion::LoopKind::Slide { .. } | rune_motion::LoopKind::SlideEased { .. }))
}

/// Paint's test for whether a box at `r` (x0, y0, x1, y1) under `clip` shows (`build`).
fn shows(r: [f32; 4], clip: [f32; 4], vp: [f32; 4]) -> bool {
    let m = 2.0;
    clip[2] > clip[0] && clip[3] > clip[1]
        && r[2] >= clip[0].max(vp[0]) - m && r[0] <= clip[2].min(vp[2]) + m
        && r[3] >= clip[1].max(vp[1]) - m && r[1] <= clip[3].min(vp[3]) + m
}

impl<S: 'static> Runtime<S> {
    /// A sliding node's loop glides: its edges land where the slide puts them, not on whole
    /// pixels, as a browser's compositor moves a transformed layer. What it holds still snaps.
    pub(in crate::runtime) fn slide_glides(&self, n: &Node) -> bool {
        !self.reduced_motion && slides(n.looping.as_deref()) && self.loops.contains_key(&n.id)
    }

    /// Paint reached node `i`: a window whose subtree it has passed is complete.
    pub(super) fn slide_node_opens(&mut self, i: usize) {
        let c = &mut self.paint_keep.cycles;
        c.opened = (i, self.instances.len());
        // A subtree paints in one run, in tree order or the layers' (`update_layer_paint_order`),
        // as long as nothing in it is lifted or raised out to paint later.
        if c.open.is_some_and(|k| i < c.slides[k].window || i >= c.slides[k].end) { self.close_slide(); }
    }

    /// Node `i` clipped what it holds to `r` under `outer` (clip, fade widths, fade origins),
    /// from `rect` (points) at `k` points of rect per point of offset and ancestors' scale `sc`:
    /// recorded if it is a sliding window a loop-only step can move.
    pub(in crate::runtime) fn note_slide(&mut self, i: usize, outer: [[f32; 4]; 3], rect: [f32; 4], r: [f32; 4], (k, sc): (f32, f32)) {
        if !self.slide_glides(&self.nodes[i]) { return; }
        let s = self.env.scale;
        let vp = [0.0, 0.0, self.viewport.0 * s, self.viewport.1 * s];
        let (counter, end, f) = match self.slide_fits(i, r, outer[0], vp) {
            Ok(fits) => fits,
            Err(why) => { self.paint_keep.cycles.missed = Some(why); return; }
        };
        let n = &self.nodes[i];
        let off = |j: usize| self.scene.get(self.nodes[j].id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
        let (ow, oc) = (off(i), off(counter));
        let slide = Slide {
            window: i,
            counter,
            end,
            instances: self.instances.len()..self.instances.len(),
            rect,
            offset: [ow.x, ow.y],
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
        let c = &mut self.paint_keep.cycles;
        c.open = Some(c.slides.len());
        c.slides.push(slide);
    }

    /// Whether sliding node `i`, drawn at `r` under `clip`, is a window a step can move: its only
    /// child slides back and holds everything under it, nothing under it takes input, clips,
    /// blurs, sticks or moves on its own, and it shows. Its child, one past its subtree, and its
    /// fade widths; or why not.
    fn slide_fits(&self, i: usize, r: [f32; 4], clip: [f32; 4], vp: [f32; 4]) -> Result<(usize, usize, [f32; 4]), &'static str> {
        let c = &self.paint_keep.cycles;
        if !self.flying.is_empty() { return Err("a slide while something flies"); }
        if c.open.is_some() { return Err("a slide inside a slide"); }
        if c.opened != (i, self.instances.len()) { return Err("a slide that draws itself"); }
        let n = &self.nodes[i];
        let (counter, end) = (i + 1, subtree_end(&self.nodes, i));
        let Some(f) = n.vis.clip else { return Err("a slide that doesn't clip") };
        if counter >= end || subtree_end(&self.nodes, counter) != end || !slides(self.nodes[counter].looping.as_deref())
            || !self.loops.contains_key(&self.nodes[counter].id) { return Err("a slide whose child doesn't slide back"); }
        if n.scroll.is_some() || self.row_h[i] >= 0.0 || self.row_mask[i] >= 0.0 { return Err("a slide that scrolls or folds"); }
        let parent = n.parent.map_or_else(|| self.root_inh(), |p| self.inh_scratch[p]);
        if parent.in_cache || parent.skip_paint || parent.turn != 0 || parent.rounded.is_some() || parent.blur != 0.0 { return Err("a slide in a layer"); }
        let plain = (i..end).all(|j| {
            let m = &self.nodes[j];
            m.handlers.get().is_none() && m.focus.get().is_none() && !m.vis_flags.hit() && m.input.is_none() && m.sticky.is_none()
                && m.vis.effect.is_none() && m.help.is_none() && m.above.is_none() && m.matched.is_none() && !m.vis_flags.raise()
        });
        if !plain { return Err("a slide over input"); }
        let still = (i..end).all(|j| {
            let m = &self.nodes[j];
            self.scene.presented(m.id).is_some_and(|p| p.blur == 0.0 && p.mask.is_none() && p.transform.rotate == 0.0)
                && (j == i || (m.vis.clip.is_none() && m.scroll.is_none() && (j == counter || m.looping.is_none())))
        });
        if !still { return Err("a slide over clips or motion"); }
        if !shows([r[0], r[1], r[2].max(r[0]), r[3].max(r[1])], clip, vp) { return Err("a slide out of sight"); }
        Ok((counter, end, f))
    }

    /// The open window's subtree is painted: its instances, and what each node under it showed.
    pub(super) fn close_slide(&mut self) {
        let Some(k) = self.paint_keep.cycles.open.take() else { return };
        let s = self.env.scale;
        let len = self.instances.len();
        let (drawn, seen) = (&self.drawn, &self.seen);
        let sl = &mut self.paint_keep.cycles.slides[k];
        sl.instances.end = len;
        sl.shown = (sl.window + 1..sl.end)
            .map(|j| (drawn.get(j).map_or([0.0; 4], |d| [d[0] * s, d[1] * s, (d[0] + d[2]) * s, (d[1] + d[3]) * s]), seen.get(j).copied().unwrap_or(false)))
            .collect();
    }

    /// Where window `sl` would clip and fade its content with its own offset at `ow` (points):
    /// None when the step can't move it alone (the content would move, or something under it
    /// would show or hide, or a glyph draw or not).
    fn slide_pose(sl: &Slide, ow: [f32; 2]) -> Option<Pose> {
        let d = [(ow[0] - sl.offset[0]) * sl.k, (ow[1] - sl.offset[1]) * sl.k];
        let mut rect = [sl.rect[0] + d[0], sl.rect[1] + d[1], sl.rect[2] + d[0], sl.rect[3] + d[1]];
        for axis in 0..2 {
            if ((rect[axis + 2] - rect[axis]) - sl.size[axis]).abs() * sl.s < 1.0 / 32.0 {
                rect[axis + 2] = rect[axis] + sl.size[axis];
            }
        }
        let r = rect.map(|v| grid(v, sl.s) * sl.s);
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
    pub(super) fn slide_poses(&mut self, t: f64) -> Result<Vec<Pose>, &'static str> {
        let at = |rt: &Self, j: usize| -> Option<[f32; 2]> {
            let n = &rt.nodes[j];
            let store = rt.scene.store();
            let time = store.node_time(n.id) + (t - rt.t) * f64::from(store.node_time_scale(n.id));
            let [x, y] = rt.loops.get(&n.id)?.offset(time, rt.reduced_motion)?;
            Some([x * n.local.w, y * n.local.h])
        };
        let mut poses = Vec::with_capacity(self.paint_keep.cycles.slides.len());
        for k in 0..self.paint_keep.cycles.slides.len() {
            if !self.slide_checked(k) { return Err("a slide drew otherwise"); }
            let sl = &self.paint_keep.cycles.slides[k];
            let (ow, oc) = at(self, sl.window).zip(at(self, sl.counter)).ok_or("a slide stopped")?;
            // The content holds still only while the two offsets cancel as they did.
            if (ow[0] + oc[0] - sl.net[0]).abs() > 1e-3 || (ow[1] + oc[1] - sl.net[1]).abs() > 1e-3 { return Err("a slide's content moves"); }
            poses.push(Self::slide_pose(sl, ow).ok_or("a slide shows or hides something")?);
        }
        Ok(poses)
    }

    /// Window `k`'s instances are what its record says: the pose it was painted at, the glyphs
    /// it drew, no layer. Asked once a paint.
    fn slide_checked(&mut self, k: usize) -> bool {
        let sl = &self.paint_keep.cycles.slides[k];
        if let Some(ok) = sl.checked { return ok; }
        let range = sl.instances.clone();
        let ok = range.end <= self.instances.len() && Self::slide_pose(sl, sl.offset).is_some_and(|(clip, fade)| {
            let here = &self.instances[range.clone()];
            here.iter().all(|q| q.clip == clip && q.fade == fade && q.params[1] != crate::render::KIND_LAYER && q.params[1] != crate::render::KIND_LAYER_END)
                && here.iter().filter(|q| q.params[1] == KIND_GLYPH).count() == sl.glyphs.iter().filter(|g| g.1).count()
        });
        self.paint_keep.cycles.slides[k].checked = Some(ok);
        ok
    }

    /// Draws each window's instances at its pose; returns the instances it wrote.
    pub(super) fn apply_slides(&mut self, poses: &[Pose], patched: &mut Vec<u32>) {
        for (sl, &(clip, fade)) in self.paint_keep.cycles.slides.iter().zip(poses) {
            for at in sl.instances.clone() {
                let q = &mut self.instances[at];
                q.clip = clip;
                q.fade = fade;
                patched.push(at as u32);
            }
        }
    }
}
