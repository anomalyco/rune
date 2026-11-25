//! Shared coordinate transform and zero-glyph painting for [`rune_text::PositionedText`].
//!
//! Every consumer (hit-testing, highlight painting, caret painting, `Cx::selection_rects`,
//! `links`, `caret_browse`, `text_offset_at`, and accessibility) maps through
//! [`PositionedTransform`] so layout-size zoom (`.size(w * z, h * z)`) and live-transform zoom
//! (`.live_scale(&z)`) align to the sub-pixel with zero view rebuilds.

use super::*;
use crate::runtime::paint_content::LeafBox;
use crate::text::PositionedText;

/// Affine scale + translation mapping [`PositionedText`] source coordinates onto a target
/// coordinate space (window points or device pixels).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PositionedTransform {
    pub(crate) ox: f32,
    pub(crate) oy: f32,
    pub(crate) sx: f32,
    pub(crate) sy: f32,
}

impl PositionedTransform {
    /// Transform from `pt` source coordinates to window points for border box `rect`
    /// (`[x0, y0, x1, y1]`) drawn at scale `z` (pass `1.0` for layout-space queries).
    pub(crate) fn from_window_box(n: &Node, pt: &PositionedText, rect: [f32; 4], z: f32) -> Self {
        let ox = rect[0] + n.inset[0] * z;
        let oy = rect[1] + n.inset[1] * z;
        let cw = (rect[2] - rect[0] - (n.inset[0] + n.inset[2]) * z).max(0.0);
        let ch = (rect[3] - rect[1] - (n.inset[1] + n.inset[3]) * z).max(0.0);
        let [sx, sy] = match pt.view_box_size() {
            Some(_) => pt.scale_for(cw, ch),
            None => [z, z],
        };
        Self { ox, oy, sx, sy }
    }

    /// Transform from `pt` source coordinates to device pixels for device-pixel border box `zr`
    /// (`[x0, y0, x1, y1]`) at box scale `zk` and display scale `s`.
    pub(crate) fn from_device_box(n: &Node, pt: &PositionedText, zr: [f32; 4], zk: f32, s: f32) -> Self {
        let ox = zr[0] + n.inset[0] * zk * s;
        let oy = zr[1] + n.inset[1] * zk * s;
        let cw = (zr[2] - zr[0] - (n.inset[0] + n.inset[2]) * zk * s).max(0.0);
        let ch = (zr[3] - zr[1] - (n.inset[1] + n.inset[3]) * zk * s).max(0.0);
        let [sx, sy] = match pt.view_box_size() {
            Some(_) => pt.scale_for(cw, ch),
            None => [zk * s, zk * s],
        };
        Self { ox, oy, sx, sy }
    }

    /// Maps a point in target space back into `PositionedText` source coordinates.
    #[inline]
    pub(crate) fn to_source(self, x: f32, y: f32) -> Option<[f32; 2]> {
        (self.sx > 1e-6 && self.sy > 1e-6 && x.is_finite() && y.is_finite())
            .then(|| [(x - self.ox) / self.sx, (y - self.oy) / self.sy])
    }

    /// Maps a source `[x, y, w, h]` rect into target `[x, y, w, h]`.
    #[inline]
    pub(crate) fn rect_xywh(self, r: [f32; 4]) -> [f32; 4] {
        [self.ox + r[0] * self.sx, self.oy + r[1] * self.sy, r[2] * self.sx, r[3] * self.sy]
    }

    /// Maps a source `[x, y, w, h]` rect into target `[x0, y0, x1, y1]`.
    #[inline]
    pub(crate) fn rect_x0y0x1y1(self, r: [f32; 4]) -> [f32; 4] {
        [
            self.ox + r[0] * self.sx,
            self.oy + r[1] * self.sy,
            self.ox + (r[0] + r[2]) * self.sx,
            self.oy + (r[1] + r[3]) * self.sy,
        ]
    }

    /// Maps a source `[x, y, w, h]` rect into a pixel-grid-snapped device-px `[x, y, w, h]`.
    #[inline]
    pub(crate) fn pixel_rect(self, r: [f32; 4]) -> [f32; 4] {
        let [x0, y0, x1, y1] = self.rect_x0y0x1y1(r).map(f32::round);
        [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
    }
}

impl RuntimeCore {
    /// Paints a glyph-less [`PositionedText`] leaf: zero glyph quads, only `El::text_highlight` /
    /// `El::highlight_texts`, `Cx::mark_texts`, active selection highlights (merged per line
    /// without `extend_run` column bridging), and collapsed `caret_browsing` carets.
    pub(in crate::runtime) fn paint_positioned_text(
        &mut self,
        i: usize,
        b: &LeafBox,
        text_color: Option<Color>,
        sel_ranges: &[(usize, usize, usize)],
        mark_ranges: &[(usize, usize, usize)],
    ) {
        let collapsed = sel_ranges.is_empty().then(|| self.caret_browse_collapsed_on(i)).flatten();
        let focused = self.host.window_focused != Some(false);
        let s = self.env.scale;
        let srgb = self.paint.srgb_blend();
        let n = &self.tree.nodes[i];
        let Some(spec) = n.text.as_deref() else { return };
        let Some(pt) = spec.positioned() else { return };
        let (zk, zr) = match b.cover {
            Some((k, c)) => (
                k,
                [
                    b.r[0] + c[0] * s,
                    b.r[1] + c[1] * s,
                    b.r[0] + (c[0] + n.local.w * k) * s,
                    b.r[1] + (c[1] + n.local.h * k) * s,
                ],
            ),
            None => (b.sc, b.r),
        };
        let tk = if b.cover.is_some() { 1.0 } else { b.sc };
        let tf = PositionedTransform::from_device_box(n, pt, zr, zk, s);
        if tf.sx <= 1e-6 || tf.sy <= 1e-6 {
            return;
        }

        for h in &spec.cold.highlights {
            if h.bg.a <= 0.0 { continue; }
            let fill = premul(h.bg, b.o, srgb);
            let radius = h.radius * s * tk;
            let pad = h.pad * s * tk;
            pt.for_each_range_box(h.start as usize, h.end as usize, |r| {
                let mut pr = tf.pixel_rect(r);
                if pad > 0.0 { pr = [pr[0] - pad, pr[1] - pad, pr[2] + 2.0 * pad, pr[3] + 2.0 * pad]; }
                self.instances.push(rect_inst(pr, fill, radius, b.clip, b.fade));
            });
        }

        if !spec.selectable || n.input.is_some() {
            return;
        }

        if !mark_ranges.is_empty() {
            let highlight = spec.cold.selection_style.map_or(spec.cold.selection, |st| st.pick(true).0);
            let fill = premul(highlight, b.o, srgb);
            for &(_, a, end) in mark_ranges.iter().filter(|m| m.0 == i) {
                pt.for_each_range_box(a, end, |r| {
                    self.instances.push(rect_inst(tf.pixel_rect(r), fill, 0.0, b.clip, b.fade));
                });
            }
        }

        if let Some((a, end)) = selectable::range_in(sel_ranges, i) {
            let (highlight, _) = spec.cold.selection_style.map_or((spec.cold.selection, None), |st| st.pick(focused));
            let fill = premul(highlight, b.o, srgb);
            let radius = 2.0 * s * tk;
            pt.for_each_range_box(a, end, |r| {
                self.instances.push(rect_inst(tf.pixel_rect(r), fill, radius, b.clip, b.fade));
            });
        } else if let Some((byte, _)) = collapsed
            && let Some([cx_s, cy_s, _, ch_s]) = pt.caret(byte)
        {
            let cx = tf.ox + cx_s * tf.sx;
            let cy = tf.oy + cy_s * tf.sy;
            let ch = (ch_s * tf.sy).max(1.0);
            let (caret_x, caret_w) = if (tk - 1.0).abs() <= 1e-5 {
                let gx = if b.gliding { crate::runtime::paint::pixel_snap::text_glide(b.r[0]) } else { 0.0 };
                ((cx - gx).round() + gx, (1.5 * s).round().max(1.0))
            } else {
                (cx, (1.5 * s * tk).max(1.0))
            };
            let caret_color = spec.cold.caret.unwrap_or_else(|| text_color.unwrap_or(spec.color));
            self.instances.push(rect_inst(
                [caret_x, cy + ch * 0.1, caret_w, ch * 0.8],
                premul(caret_color, b.o, srgb),
                0.0,
                b.clip,
                b.fade,
            ));
        }
    }
}
