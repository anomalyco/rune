//! What each node paints this frame, for `rune::motion_gate`: its presented colours, corners,
//! blurs, shadow, effect payload, image and place in the paint order.
use super::*;
use crate::motion_gate::check::paint::{C_BORDER, C_FILL, C_GRADIENT, C_OUTLINE, C_SHADOW, C_TEXT, Paint, S_BACKDROP, S_BLUR, S_BORDER, S_RADIUS, S_SHADOW};

impl RuntimeCore {
    /// Every presented node's paint, keyed by its path and whether it's leaving. Paint ranks come
    /// from the last frame built once [`Runtime::paint_rank`] is on.
    pub(crate) fn paints(&self) -> HashMap<(String, bool), Paint> {
        let mut out = HashMap::default();
        for (i, n) in self.tree.nodes.iter().enumerate() {
            let Some(p) = self.scene.presented(n.id) else { continue };
            let v = &n.vis;
            let mut overlay = Color::TRANSPARENT;
            let mut pointer = [0.0; 2];
            let mut over_c = |c: Color, k: f32| {
                if k > 0.0 && c.a > 0.0 { overlay = unpremul(over(c.premul(k), overlay.premul(1.0))); }
            };
            let focus_flat = n.focus.flat_within && self.focus_within(i);
            let hov_v = if !focus_flat && (v.tracks_hover_paint() || self.scene.store().is_animating(n.id, P_HOVER_C)) {
                self.scene.get(n.id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0)
            } else { 0.0 };
            let prs_v = if v.tracks_press() || self.scene.store().is_animating(n.id, P_PRESS_C) {
                self.scene.get(n.id, P_PRESS).unwrap_or(0.0).clamp(0.0, 1.0)
            } else { 0.0 };
            if v.tracks_hover_paint() || self.scene.store().is_animating(n.id, P_HOVER_C) {
                pointer[0] = hov_v;
                if v.hover().a > 0.0 || self.scene.store().is_animating(n.id, P_HOVER_C) {
                    over_c(self.color(n.id, P_HOVER_C, v.hover()), hov_v);
                }
            }
            if v.tracks_press() || self.scene.store().is_animating(n.id, P_PRESS_C) {
                pointer[1] = prs_v;
                if v.press().a > 0.0 || self.scene.store().is_animating(n.id, P_PRESS_C) {
                    over_c(self.color(n.id, P_PRESS_C, v.press()), prs_v);
                }
            }
            let (base, fill_replace_w) = paint_tracks::replace_color(
                self.color(n.id, P_FILL, v.fill),
                if focus_flat { None } else { v.hover_fill },
                hov_v,
                v.press_fill,
                prs_v,
            );
            let fill = unpremul(over(overlay.premul(1.0), base.premul(1.0)));
            // Static fades, as they show: its own `dim` and the layer opacity of it and every
            // element it sits in.
            let k = v.paint_alpha * self.tree.effective_layer_opacity(i);
            let mut colors = [Color::TRANSPARENT; 6];
            colors[C_FILL] = fill.alpha(fill.a * k);
            if v.border_width() > 0.0 {
                let bc = n.focus.border.filter(|_| self.focus_ring_visible(i)).unwrap_or_else(|| {
                    paint_tracks::replace_color(
                        self.color(n.id, P_BORDER, v.border()),
                        if focus_flat { None } else { v.hover_border },
                        hov_v,
                        v.press_border,
                        prs_v,
                    ).0
                });
                colors[C_BORDER] = bc.alpha_times(k);
            }
            // As it paints: a hovered group's `El::group_color` mixes in.
            let group = group_lit(&self.scene, n, self.paint.inh_scratch.get(i).map_or(0.0, |h| h.group));
            if let Some(c) = paint_tracks::text_color(n) { colors[C_TEXT] = paint_tracks::group_tint(n, self.color(n.id, P_TEXT, c), group).alpha_times(k); }
            // A group's hover lands its colour as the pointer moves, as a hover fill does.
            if v.group_color.is_some() { pointer[0] = pointer[0].max(group); }
            let mut sizes = [0.0; 5];
            sizes[S_RADIUS] = p.radius.0[0];
            sizes[S_BORDER] = v.border_width();
            sizes[S_BLUR] = self.paint.inh_scratch.get(i).map_or(p.blur, |h| self.paint.inh_cold_of(h).blur);
            sizes[S_BACKDROP] = v.backdrop_blur;
            if let Some(sh) = v.shadows.iter().find(|s| !s.inset) {
                let at = self.shadow_at(n.id, 0, sh);
                colors[C_SHADOW] = self.color(n.id, P_SHADOWS[0], sh.color).alpha_times(k);
                sizes[S_SHADOW] = at.blur + at.spread.abs() + at.x.abs().max(at.y.abs());
            }
            let gk = k * (1.0 - fill_replace_w).clamp(0.0, 1.0);
            let mut gradient_end = Color::TRANSPARENT;
            if let Some(g) = &v.gradient {
                colors[C_GRADIENT] = self.color(n.id, P_GRADIENT[0], g[0]).alpha_times(gk);
                gradient_end = self.color(n.id, P_GRADIENT[1], g[1]).alpha_times(gk);
            } else if let Some(g) = &v.radial && let (Some(first), Some(last)) = (g.stops.first(), g.stops.last()) {
                colors[C_GRADIENT] = self.color(n.id, P_RADIAL[0], first.1).alpha_times(gk);
                let end = (g.stops.len() - 1).min(P_RADIAL.len() - 1);
                gradient_end = if g.stops.len() - 1 < P_RADIAL.len() { self.color(n.id, P_RADIAL[end], last.1) } else { last.1 }.alpha_times(gk);
            }
            if let Some(o) = v.outline.filter(|o| o.width > 0.0) { colors[C_OUTLINE] = self.color(n.id, P_OUTLINE, o.color); }
            let intent = std::iter::successors(Some(i), |&j| self.tree.nodes[j].parent).fold(0u16, |bits, j| bits | self.tree.nodes[j].vis.motion_intent.bits());
            let paint = Paint {
                colors,
                sizes,
                gradient_end,
                effect: v.effect.as_ref().map(|e| e.data),
                image: v.pixels.as_ref().map(crate::pixels::Pixels::signature),
                rank: self.paint.paint_rank(i),
                intent,
                instant: n.instant_changes,
                at_once: crate::motion_gate::check::paint::at_once(v.instant_motion, v.blur_motion.as_deref().map(|a| a.curve == rune_motion::Curve::Instant)),
                pointer,
            };
            out.entry((n.path.clone(), self.subtree_exiting(i))).or_insert(paint);
        }
        out
    }
}

fn unpremul([r, g, b, a]: [f32; 4]) -> Color {
    if a <= 1e-6 { Color::TRANSPARENT } else { Color { r: r / a, g: g / a, b: b / a, a } }
}

trait AlphaTimes {
    fn alpha_times(self, k: f32) -> Self;
}

impl AlphaTimes for Color {
    fn alpha_times(self, k: f32) -> Color {
        self.alpha(self.a * k)
    }
}
