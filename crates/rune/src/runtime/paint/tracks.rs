//! A node's paint: colours, shadow geometry, hover and press. Most of it never changes, so a
//! property gets a track only when it does, starting from what was last drawn.

use super::*;

/// Finds new nodes in the last build. Both trees are mostly in the same order, so it walks
/// them together, and once a node is out of step it walks together no more: it finds every
/// later one in a map built on that first miss.
#[derive(Debug)]
pub(in crate::runtime) enum Finder {
    /// Both trees agree on every node so far: the next one to compare sits at `next`.
    InStep { next: usize },
    /// The first node out of step built `map` of the last build, from `next` on.
    Mapped { next: usize, map: HashMap<NodeId, usize> },
}

impl Default for Finder {
    fn default() -> Self { Finder::InStep { next: 0 } }
}

impl Finder {
    pub(in crate::runtime) fn find(&mut self, old: &[Node], id: NodeId) -> Option<usize> {
        let at = match self {
            Finder::InStep { next } => match old.get(*next) {
                Some(n) if n.id == id => *next,
                _ => {
                    let (next, map): (usize, HashMap<NodeId, usize>) = (*next, old.iter().enumerate().map(|(i, n)| (n.id, i)).collect());
                    let at = map.get(&id).copied();
                    *self = Finder::Mapped { next, map };
                    at?
                }
            },
            Finder::Mapped { next, map } => match old.get(*next) {
                Some(n) if n.id == id => *next,
                _ => map.get(&id).copied()?,
            },
        };
        self.advance(at + 1);
        Some(at)
    }

    pub(in crate::runtime) fn advance(&mut self, next: usize) {
        match self {
            Finder::InStep { next: at } | Finder::Mapped { next: at, .. } => *at = next,
        }
    }
}

/// Resolves `old` against `old_nodes`, falling back to `nodes[i]` when `old` was moved out of
/// `old_nodes` by a `same_path` memo reuse (leaving its paint and text fields unchanged on `nodes[i]`).
#[inline]
pub(in crate::runtime) fn prev_node<'a>(old_nodes: &'a [Node], nodes: &'a [Node], old: Option<usize>, i: usize) -> Option<&'a Node> {
    old.map(|o| if old_nodes[o].is_moved_stub() { &nodes[i] } else { &old_nodes[o] })
}

/// The colour a node's text or icon draws in.
pub(in crate::runtime) fn text_color(n: &Node) -> Option<Color> {
    n.text.as_ref().map(|t| t.color).or(n.vis.icon.map(|(_, c)| c))
}

/// Text or icon colour `c` as it paints while its hover group is `group` hovered (0 to 1):
/// mixed toward its `El::group_color`.
pub(in crate::runtime) fn group_tint(n: &Node, c: Color, group: f32) -> Color {
    match n.vis.group_color {
        Some(g) if group > 0.0 => c.mix(g, group),
        _ => c,
    }
}

/// Text or icon colour `c` while a press on the nearest `El::press_color` element is held (`pressed`,
/// 0 to 1): mixed toward that colour.
pub(in crate::runtime) fn press_tint(c: Color, to: Option<Color>, pressed: f32) -> Color {
    match to {
        Some(p) if pressed > 0.0 => c.mix(p, pressed),
        _ => c,
    }
}

/// Premultiplied linear interpolation between `a` and `b`, matching CSS transitions so
/// `Color::TRANSPARENT` fades cleanly without dark fringing.
fn lerp_premul(a: Color, b: Color, t: f32) -> Color {
    if t <= 0.0 { return a; }
    if t >= 1.0 { return b; }
    let pa = a.premul(1.0);
    let pb = b.premul(1.0);
    let r = pa[0] + (pb[0] - pa[0]) * t;
    let g = pa[1] + (pb[1] - pa[1]) * t;
    let bl = pa[2] + (pb[2] - pa[2]) * t;
    let al = pa[3] + (pb[3] - pa[3]) * t;
    if al <= 1e-6 {
        Color::TRANSPARENT
    } else {
        Color { r: r / al, g: g / al, b: bl / al, a: al }
    }
}

/// Resolves a rest fill or border colour through optional declarative hover and press
/// replacements (hover first, then press so press wins). Returns `(color, replacement_weight)`
/// where `replacement_weight` is in `0.0..=1.0` for suppressing active fill gradients.
#[inline]
pub(in crate::runtime) fn replace_color(
    rest: Color,
    hover: Option<Color>,
    hov_v: f32,
    press: Option<Color>,
    prs_v: f32,
) -> (Color, f32) {
    let mut c = rest;
    let mut w = 0.0f32;
    if let Some(h) = hover && hov_v > 0.0 {
        c = lerp_premul(c, h, hov_v);
        w = hov_v;
    }
    if let Some(p) = press && prs_v > 0.0 {
        c = lerp_premul(c, p, prs_v);
        w = w + (1.0 - w) * prs_v;
    }
    (c, w)
}

/// Which of a node's colours a live value owns. A commit leaves those tracks to the latch.
#[derive(Clone, Copy, Default)]
pub(in crate::runtime) struct LiveColors {
    pub(in crate::runtime) fill: bool,
    pub(in crate::runtime) text: bool,
    pub(in crate::runtime) border: bool,
    pub(in crate::runtime) shadow: bool,
    pub(in crate::runtime) gradient: [bool; 2],
}

pub(in crate::runtime) fn live_colors(n: &Node) -> LiveColors {
    let mut c = LiveColors::default();
    for b in n.live.iter() { c.add(b.property); }
    c
}

impl LiveColors {
    pub(in crate::runtime) fn add(&mut self, p: crate::live::Property) {
        use crate::live::Property;
        match p {
            Property::Fill => self.fill = true,
            Property::TextColor => self.text = true,
            Property::Border => self.border = true,
            Property::Shadow => self.shadow = true,
            Property::GradientTop => self.gradient[0] = true,
            Property::GradientBottom => self.gradient[1] = true,
            _ => {}
        }
    }

    /// Puts back on `new` the colours `old` last latched for the bindings it kept, since the
    /// view authored its own placeholder colour.
    pub(in crate::runtime) fn carry(self, old: &Node, new: &mut Node) {
        if self.fill { new.vis.fill = old.vis.fill; }
        if self.border { new.vis.set_border_color(old.vis.border()); }
        if self.text && let Some(c) = text_color(old) {
            if let Some(spec) = &mut new.text { if spec.color != c { spec.color = c; } }
            else if new.vis.icon.is_some_and(|(_, ic)| ic != c) && let Some((_, ic)) = &mut new.vis.cold_mut().icon { *ic = c; }
        }
        if self.shadow && let Some(c) = old.vis.shadows.first().map(|s| s.color)
            && new.vis.shadows.first().is_some_and(|s| s.color != c)
        {
            new.vis.cold_mut().shadows[0].color = c;
        }
        for k in 0..2 {
            if self.gradient[k] && let Some(c) = old.vis.gradient.as_deref().map(|g| g[k])
                && new.vis.gradient.as_deref().is_some_and(|g| g[k] != c)
                && let Some(g) = new.vis.cold_mut().gradient.as_deref_mut()
            {
                g[k] = c;
            }
        }
    }
}

/// Readies `p` (hover, press, press scale), which reads `rest` without a track, to move to
/// `to`: false when it has no track and `to` is `rest`, so there's nothing to do; otherwise it
/// has a track, made at `rest` if it had none.
pub(in crate::runtime) fn ready_from_rest(scene: &mut Scene, id: NodeId, p: Prop<f32>, rest: f32, to: f32) -> bool {
    if scene.store().has(id, p) {
        return true;
    }
    if to == rest {
        return false;
    }
    scene.set(id, p, rest);
    true
}

/// A shadow's geometry as its track holds it.
pub(in crate::runtime) fn shadow_geo(sh: &crate::el::Shadow) -> rune_motion::Rect {
    rune_motion::Rect { x: sh.x, y: sh.y, w: sh.blur, h: sh.spread }
}

/// Commits `to` on paint property `p` of node `id`, which last drew `from` (None if it's
/// new). Without a track a node draws its own value (`Runtime::color`, `shadow_at`), so it
/// needs none until the value changes, and a commit that leaves it changes nothing.
#[inline]
pub(in crate::runtime) fn commit_paint<T: rune_motion::Animatable + PartialEq>(
    scene: &mut Scene,
    id: NodeId,
    p: Prop<T>,
    from: Option<T>,
    to: T,
    s: Spring,
    instant: bool,
) {
    if !instant && from == Some(to) {
        // Its track, if any, already heads to `to`: nothing else moves this property.
        return;
    }
    let tracked = scene.store().has(id, p);
    if from == Some(to) && !tracked {
        return;
    }
    if !tracked {
        match from {
            Some(from) if !instant => scene.set(id, p, from),
            _ => return,
        }
    }
    if instant {
        scene.set(id, p, to);
    } else {
        scene.animate(id, p, to, s);
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::runtime) fn commit_cold_paint(
    scene: &mut Scene,
    old_nodes: &[Node],
    id: NodeId,
    vis: &Visual,
    old: Option<usize>,
    live: LiveColors,
    spring_c: Spring,
    spring_l: Spring,
    instant: bool,
) {
    fn prev_vis<'a>(old_nodes: &'a [Node], vis: &'a Visual, old: Option<usize>) -> Option<&'a Visual> {
        let o = old?;
        Some(if old_nodes[o].is_moved_stub() { vis } else { &old_nodes[o].vis })
    }
    for (k, sh) in vis.shadows.iter().take(P_SHADOWS.len()).enumerate() {
        let was = prev_vis(old_nodes, vis, old).and_then(|o| o.shadows.get(k).copied());
        // An inset ring where a drop shadow was (or the other way) is another shadow, not this
        // one changing: it lands rather than morphing between the two.
        let instant = instant || was.is_some_and(|w| w.inset != sh.inset);
        if !(k == 0 && live.shadow) {
            commit_paint(scene, id, P_SHADOWS[k], was.map(|w| w.color), sh.color, spring_c, instant);
        }
        commit_paint(scene, id, P_SHADOW_GEO[k], was.map(|w| shadow_geo(&w)), shadow_geo(sh), spring_l, instant);
    }
    let prev_shadows = prev_vis(old_nodes, vis, old).map_or(0, |o| o.shadows.len()).min(P_SHADOWS.len());
    for k in vis.shadows.len()..prev_shadows {
        scene.store_mut().remove(id, P_SHADOWS[k]);
        scene.store_mut().remove(id, P_SHADOW_GEO[k]);
    }
    if let Some(&g) = vis.gradient.as_deref() {
        let was = prev_vis(old_nodes, vis, old).and_then(|o| o.gradient.as_deref().copied());
        for k in 0..2 {
            if !live.gradient[k] {
                commit_paint(scene, id, P_GRADIENT[k], was.map(|w| w[k]), g[k], spring_c, instant);
            }
        }
    }
    if let Some(rg) = &vis.radial {
        for (k, &(_, c)) in rg.stops.iter().take(P_RADIAL.len()).enumerate() {
            let was = prev_vis(old_nodes, vis, old).and_then(|o| o.radial.as_ref()?.stops.get(k).map(|s| s.1));
            commit_paint(scene, id, P_RADIAL[k], was, c, spring_c, instant);
        }
    }
    if let Some(ol) = vis.outline {
        let was = prev_vis(old_nodes, vis, old).and_then(|o| o.outline.map(|w| w.color));
        commit_paint(scene, id, P_OUTLINE, was, ol.color, spring_c, instant);
    }
}

/// True when `nodes[i]` has the same parent, frame, visual, text colour, transition and
/// resting scene state as `old_nodes[old]`, so `apply`'s per-node diff has nothing to move.
#[allow(clippy::too_many_arguments)]
pub(in crate::runtime) fn unchanged_node_resting(
    tree: &types::TreeState,
    scene: &Scene,
    motion: &crate::runtime::tracks::MotionState,
    nodes: &[Node],
    i: usize,
    old: Option<usize>,
    scene_rect: MRect,
    parent_virtual: Option<&crate::el::virtual_list::VirtualList>,
    reorder_jump: impl FnOnce() -> f32,
) -> bool {
    let Some(o_idx) = old else { return false };
    let o = &tree.nodes[o_idx];
    if o.is_moved_stub() {
        return false;
    }
    let n = &nodes[i];
    let plain_cold = |nd: &Node| {
        nd.cold.get().is_none_or(|c| {
            c.matched.is_none()
                && c.matched_motion.is_none()
                && c.looping.is_none()
                && c.loop_times.is_none()
                && c.live.is_empty()
                && c.transition.is_none()
                && c.transition_motion.is_none()
                && c.frame_motion.is_none()
                && c.opacity_motion.is_none()
        })
    };
    n.local == o.local
        && !o.scale_track
        && n.static_scale == 1.0
        && !o.group_fade
        && n.parent.map(|p| nodes[p].id) == o.parent.map(|p| tree.nodes[p].id)
        && plain_cold(n)
        && plain_cold(o)
        && (motion.flying.is_empty() || !motion.flying.iter().any(|f| f.0 == n.id))
        && (tree.aspect_snap.is_empty() || !tree.aspect_snap.contains(&n.id))
        && !parent_virtual.is_some_and(crate::el::virtual_list::VirtualList::replaced_view)
        && memo_reuse::same_visual(&n.vis, &o.vis)
        && text_color(n) == text_color(o)
        && n.text.as_ref().is_none_or(|t| {
            t.reveal < 0.0 || motion.reveal_caught_up(n.id, t.content.len())
        })
        && scene.is_resting_at(n.id, scene_rect)
        && (motion.stacks.is_empty()
            || n.parent.is_none_or(|p| nodes[p].growth.is_none())
            || reorder_jump() == 0.0)
}

#[cfg(test)]
#[path = "../paint_tracks_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "../live_colors_tests.rs"]
mod live_colors_tests;
