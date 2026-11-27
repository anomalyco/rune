//! Visibility checks for selectable text across hit-testing, group drags, Select All, copy,
//! `Cx::selection_rects`, and selection highlight culling.
use super::*;

/// Whether pixel rect `[x, y, w, h]` has positive area and overlaps `clip` (`[x0, y0, x1, y1]`).
#[inline]
pub(in crate::runtime) fn rect_in_clip(rect: [f32; 4], clip: [f32; 4]) -> bool {
    rect[2] > 0.0
        && rect[3] > 0.0
        && rect[0] + rect[2] > clip[0]
        && rect[0] < clip[2]
        && rect[1] + rect[3] > clip[1]
        && rect[1] < clip[3]
}

impl RuntimeCore {
    /// Whether mounted text node `i` is hidden from the user and must not take selection hits,
    /// participate in Select All / group drags / copy, or emit selection rects:
    /// - `opacity(0.0)`, `live_opacity(0.0)`, `layer_opacity(0.0)`, or unhovered `group_opacity(0.0, _)`
    ///   on itself or any ancestor (preserving frame-0 active presence entrances);
    /// - `scale(0.0)` / `live_scale(0.0)` on itself or any ancestor;
    /// - collapsed zero-area clipping ancestor (`clip_soft`, `clip_rounded`, `collapsing`, `scroll`, `mask`)
    ///   or zero-area self-clipping text (`truncate`, `truncate_fade`, `clip_rounded`, `collapsing`);
    /// - elided inline child or unresolved anchored overlay.
    ///
    /// Texts scrolled outside a positive-area `El::scroll` viewport are NOT hidden: they remain
    /// part of Select All and cross-scroll drags while paint clips their highlights to the viewport.
    pub(in crate::runtime) fn is_text_hidden(&self, i: usize) -> bool {
        let mut cur = Some(i);
        let mut opacity = 1.0_f32;
        let mut layer_opacity = 1.0_f32;
        while let Some(j) = cur {
            if super::super::commit::inline::is_elided_inline_child(&self.tree.nodes, j) {
                return true;
            }
            let n = &self.tree.nodes[j];
            if n.vis.opacity <= 0.001 || n.vis.layer_opacity <= 0.001 {
                return true;
            }
            layer_opacity *= n.vis.layer_opacity;
            if layer_opacity <= 0.001 {
                return true;
            }
            if let Some(a) = &n.above
                && anchor_index_live(&self.tree.nodes, j, a, |idx| self.subtree_exiting(idx)).is_none()
                && !(self.subtree_exiting(j) && self.paint.last_anchor_targets.contains_key(&n.id))
            {
                return true;
            }
            if let Some(p) = self.scene.presented(n.id) {
                let mut own = p.opacity;
                if own <= 0.001 {
                    let entering = p.phase == PresencePhase::Entering
                        && self.scene.store().is_moving(n.id, prop::PRESENCE);
                    if !entering {
                        return true;
                    }
                    own = self.scene.get(n.id, prop::OPACITY).unwrap_or(n.vis.opacity).clamp(0.0, 1.0);
                }
                if n.vis.group_opacity.is_some() {
                    let parent_group = std::iter::successors(n.parent, |&k| self.tree.nodes[k].parent)
                        .find(|&k| {
                            let an = &self.tree.nodes[k];
                            an.vis.tracks_hover_paint() || an.vis_flags.hover_group()
                        })
                        .map_or(0.0, |k| self.scene.get(self.tree.nodes[k].id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0));
                    own *= group_factor(
                        &self.scene,
                        self.kbd.focus.keyboard(),
                        self.kbd.focus.path(),
                        n,
                        group_lit(&self.scene, n, parent_group),
                    );
                }
                opacity *= own;
                if opacity <= 0.001 {
                    return true;
                }
                let static_k = if n.scale_track {
                    self.scene.get(n.id, P_SCALE).unwrap_or(n.static_scale)
                } else {
                    n.static_scale
                };
                if (p.transform.scale.x * static_k).abs() <= 1e-6
                    || (p.transform.scale.y * static_k).abs() <= 1e-6
                {
                    return true;
                }
                if !p.mask.is_none() {
                    let mw = p.rect.w - p.mask.inset.0[0] - p.mask.inset.0[2];
                    let mh = p.rect.h - p.mask.inset.0[1] - p.mask.inset.0[3];
                    if mw <= 0.0 || mh <= 0.0 {
                        return true;
                    }
                }
                if j != i {
                    let clips = n.vis.clip.is_some()
                        || n.vis.clip_radius.is_some()
                        || n.collapsing
                        || n.scroll.is_some();
                    if clips && (p.rect.w <= 0.0 || p.rect.h <= 0.0) {
                        return true;
                    }
                } else {
                    let self_clips = n.vis.clip_radius.is_some()
                        || n.collapsing
                        || n.elided.is_some()
                        || n.text.as_ref().is_some_and(|t| t.truncate || t.truncate_fade);
                    if self_clips {
                        let cw = p.rect.w - n.inset[0] - n.inset[2];
                        let ch = p.rect.h - n.inset[1] - n.inset[3];
                        if cw <= 0.0 || ch <= 0.0 {
                            return true;
                        }
                    }
                }
            }
            cur = n.parent;
        }
        false
    }
}
