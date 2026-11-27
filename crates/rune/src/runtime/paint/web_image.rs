//! Per-frame collection of `El::web_image` / `El::web_image_fit` draws in CSS pixels.
use std::sync::Arc;

use super::*;
use crate::web_image::{WebImageClip, WebImageDraw};

impl RuntimeCore {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_web_image(
        &mut self,
        i: usize,
        r: [f32; 4],
        radii: [f32; 4],
        border_inset: [f32; 4],
        effective_opacity: f32,
        pinh_clip: [f32; 4],
        content_clip: [f32; 4],
        turn_index: u32,
    ) {
        let Some(web_img) = self.tree.nodes[i].vis.cold.get().and_then(|c| c.web_image.as_ref()) else {
            return;
        };
        let url = Arc::clone(&web_img.url);
        let fit = web_img.fit;
        let n = &self.tree.nodes[i];
        let id = n.id;
        let place = n.vis.surface_place;
        let nearest = n.vis.image_filter == crate::ImageFilter::Nearest;
        let opacity = (effective_opacity * self.tree.effective_layer_opacity(i) * n.vis.paint_alpha).clamp(0.0, 1.0);
        if opacity <= 0.001 {
            return;
        }
        let (w, h) = ((r[2] - r[0]).max(0.0), (r[3] - r[1]).max(0.0));
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let radii_abs = radii.map(f32::abs);
        let (inner_px, inner_radii_px) = border::inset_box([r[0], r[1], w, h], radii_abs, border_inset);
        if inner_px[2] <= 0.0 || inner_px[3] <= 0.0 {
            return;
        }
        let half_px = inner_px[2].min(inner_px[3]) * 0.5;
        let inner_radii_px = inner_radii_px.map(|v| v.clamp(0.0, half_px));
        let ws = self.window_scale().max(1e-3);
        let z = self.zoom();
        let doc_y = self.page_offset() * z;
        let rect = [inner_px[0] / ws, inner_px[1] / ws + doc_y, inner_px[2] / ws, inner_px[3] / ws];
        let css_radii = inner_radii_px.map(|v| v / ws);
        let inner_edges_px = [inner_px[0], inner_px[1], inner_px[0] + inner_px[2], inner_px[1] + inner_px[3]];

        let (transform, bbox_px, hard_clip_px) = if let Some(&turn) = turn_index
            .checked_sub(1)
            .and_then(|k| self.paint.turn_scratch.get(k as usize))
        {
            let [a, b, c, d, tx, ty] = turn.mat;
            let e = (a * inner_px[0] + b * inner_px[1] + tx) / ws;
            let f = (c * inner_px[0] + d * inner_px[1] + ty) / ws + doc_y;
            let bbox = turn.bbox(inner_edges_px);
            (Some([a, c, b, d, e, f]), bbox, turn.outer_clip)
        } else {
            let hard = intersect(pinh_clip, content_clip);
            (None, inner_edges_px, hard)
        };

        let visible_px = intersect(bbox_px, hard_clip_px);
        if visible_px[2] <= visible_px[0] || visible_px[3] <= visible_px[1] {
            return;
        }
        let clip_cuts = hard_clip_px[0] > bbox_px[0] + 0.01
            || hard_clip_px[1] > bbox_px[1] + 0.01
            || hard_clip_px[2] < bbox_px[2] - 0.01
            || hard_clip_px[3] < bbox_px[3] - 0.01;
        let clip = clip_cuts.then_some([
            visible_px[0] / ws,
            visible_px[1] / ws + doc_y,
            (visible_px[2] - visible_px[0]) / ws,
            (visible_px[3] - visible_px[1]) / ws,
        ]);

        let bbox_css = [
            visible_px[0] / ws,
            visible_px[1] / ws + doc_y,
            visible_px[2] / ws,
            visible_px[3] / ws + doc_y,
        ];
        let mut rounded_clips = Vec::new();
        let mut parent = self.tree.nodes[i].parent;
        while let Some(p) = parent {
            let anc = &self.tree.nodes[p];
            parent = anc.parent;
            if self.paint.inh_scratch[p].turn != 0 {
                continue;
            }
            if anc.vis.clip_radius.is_none() && anc.vis.clip_corners.is_none() {
                continue;
            }
            let d = self.paint.drawn[p];
            let clip_rect = [d[0] * z, d[1] * z + doc_y, d[2] * z, d[3] * z];
            if clip_rect[2] <= 0.0 || clip_rect[3] <= 0.0 {
                return;
            }
            if bbox_css[2] <= clip_rect[0] || bbox_css[0] >= clip_rect[0] + clip_rect[2]
                || bbox_css[3] <= clip_rect[1] || bbox_css[1] >= clip_rect[1] + clip_rect[3]
            {
                return;
            }
            let raw_radii = anc.vis.clip_corners.unwrap_or_else(|| {
                if anc.vis.radii() != [0.0; 4] {
                    self.scene.presented(anc.id).map_or(anc.vis.radii(), |pr| pr.radius.0)
                } else {
                    [anc.vis.clip_radius.unwrap_or(0.0); 4]
                }
            });
            let half = clip_rect[2].min(clip_rect[3]) * 0.5;
            let sc_p = self.paint.inh_scratch[p].scale;
            let clip_radii = raw_radii.map(|v| (v.max(0.0) * sc_p * z).min(half));
            if clip_radii.iter().any(|&v| v > 0.05) {
                let max_l = clip_radii[0].max(clip_radii[3]);
                let max_r = clip_radii[1].max(clip_radii[2]);
                let max_t = clip_radii[0].max(clip_radii[1]);
                let max_b = clip_radii[2].max(clip_radii[3]);
                let inside_h = bbox_css[0] >= clip_rect[0] + max_l && bbox_css[2] <= clip_rect[0] + clip_rect[2] - max_r;
                let inside_v = bbox_css[1] >= clip_rect[1] + max_t && bbox_css[3] <= clip_rect[1] + clip_rect[3] - max_b;
                if !inside_h && !inside_v {
                    rounded_clips.push(WebImageClip { rect: clip_rect, radii: clip_radii });
                }
            }
        }
        rounded_clips.reverse();

        self.paint.push_web_image(WebImageDraw {
            id,
            url,
            rect,
            radii: css_radii,
            clip,
            rounded_clips,
            fit,
            anchor: place.anchor,
            unscaled: place.unscaled,
            nearest,
            opacity,
            transform,
        });
    }
}
