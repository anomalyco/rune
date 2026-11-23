//! On-demand long-field accessibility: an intermediate container (`\x1frune-lines`) holding only
//! the drawn visual lines inside the clip band (plus an off-screen focused caret line), with IME
//! preedit folded back into model coordinates so scrolling updates only that container.

use accesskit::{Node as AkNode, NodeId as AkId, Rect as AkRect, Role, TextDirection};
use std::hash::{Hash, Hasher};

use super::RUN_MAX_BYTES;
use super::runs::{self, Line, build_visual_line, push_piece_lines_in, push_span_chunks};
use super::super::tree::{LINES_BOX_CLASS, LINE_CLASS, Walk, drawn_line_id, lines_box_id, round};
use crate::runtime::{RuntimeCore, caret_index};

/// Cap on drawn visual lines published for one long field, applied before building cluster arrays.
const MAX_DRAWN_LINES: usize = 512;

#[allow(clippy::too_many_arguments)]
fn fold_line_preedit(
    model_text: &str,
    disp_range: std::ops::Range<usize>,
    line_w: f32,
    rtl: bool,
    (sel_a, sel_b, p_len, caret_in_pre): (usize, usize, usize, usize),
    lengths: &[u8],
    positions: &[f32],
    widths: &[f32],
) -> (Vec<u8>, Vec<f32>, Vec<f32>) {
    let p_start = sel_a;
    let p_end = sel_a + p_len;
    let caret_disp = sel_a + caret_in_pre;
    let (mut out_l, mut out_p, mut out_w) = (
        Vec::with_capacity(lengths.len()),
        Vec::with_capacity(lengths.len()),
        Vec::with_capacity(lengths.len()),
    );
    let mut disp_cur = disp_range.start;
    let mut pre_min = f32::INFINITY;
    let mut pre_max = f32::NEG_INFINITY;
    let mut caret_x: Option<f32> = None;
    let mut folded = false;

    let flush_fold = |folded: &mut bool,
                      pre_min: f32,
                      pre_max: f32,
                      caret_x: Option<f32>,
                      out_l: &mut Vec<u8>,
                      out_p: &mut Vec<f32>,
                      out_w: &mut Vec<f32>| {
        if *folded { return; }
        *folded = true;
        let x0 = if pre_min.is_finite() { pre_min } else if rtl { line_w } else { 0.0 };
        let x1 = if pre_max.is_finite() { pre_max } else { x0 };
        if disp_range.start <= p_start && sel_b > sel_a {
            push_span_chunks(model_text, sel_a..sel_b, x0, (x1 - x0).max(0.0), out_l, out_p, out_w);
        } else if (disp_range.start <= caret_disp && caret_disp <= disp_range.end)
            || disp_range.start <= p_start
        {
            let cx = caret_x.unwrap_or(if rtl { x0 } else { x1 });
            out_l.push(0);
            out_p.push(cx);
            out_w.push(0.0);
        }
    };

    for k in 0..lengths.len() {
        let len = lengths[k];
        let pos = positions.get(k).copied().unwrap_or(0.0);
        let wid = widths.get(k).copied().unwrap_or(0.0);
        let next = disp_cur + usize::from(len);
        if next <= p_start {
            out_l.push(len);
            out_p.push(pos);
            out_w.push(wid);
        } else if disp_cur >= p_end {
            flush_fold(&mut folded, pre_min, pre_max, caret_x, &mut out_l, &mut out_p, &mut out_w);
            out_l.push(len);
            out_p.push(pos);
            out_w.push(wid);
        } else {
            pre_min = pre_min.min(pos);
            pre_max = pre_max.max(pos + wid.max(0.0));
            if caret_disp <= disp_cur && caret_x.is_none() {
                caret_x = Some(if rtl { pos + wid } else { pos });
            } else if caret_disp >= next {
                caret_x = Some(if rtl { pos } else { pos + wid });
            } else if caret_disp > disp_cur && caret_disp < next {
                caret_x = Some(pos + wid * 0.5);
            }
        }
        disp_cur = next;
    }
    flush_fold(&mut folded, pre_min, pre_max, caret_x, &mut out_l, &mut out_p, &mut out_w);
    (out_l, out_p, out_w)
}

impl RuntimeCore {
    /// Single O(1) shared predicate deciding whether node `i` is a non-secret text field answered
    /// through the on-demand long-field path: past `RUN_MAX_BYTES`, waiting lazy paragraphs,
    /// elided (`truncate` / `max_lines`), or laid-out lines whose end byte does not cover the
    /// drawn text.
    pub(in crate::a11y) fn a11y_uses_long_field(&self, i: usize) -> bool {
        let n = &self.tree.nodes[i];
        let (Some(spec), Some(tl), Some(input)) = (n.text.as_deref(), n.text_layout.as_ref(), n.input.as_deref()) else {
            return false;
        };
        if n.is_secure() { return false; }
        if n.elided.is_some() { return true; }
        let waiting = matches!(tl, crate::text::TextLayout::Paragraphs(p) if p.waiting());
        if waiting || input.edit.text.len() > RUN_MAX_BYTES || spec.content.len() > RUN_MAX_BYTES {
            return true;
        }
        let Some(drawn_len) = self.a11y_drawn_len(i) else { return false };
        if drawn_len > RUN_MAX_BYTES || spec.content.len() != drawn_len {
            return true;
        }
        runs::layout_end_byte(tl.view(), &spec.content) != Some(drawn_len)
    }

    fn a11y_long_content_key(&self, i: usize) -> u64 {
        let n = &self.tree.nodes[i];
        n.text.as_deref().and_then(|t| t.shape.map(|(_, k)| k))
            .or_else(|| n.text_layout.as_ref().and_then(crate::text::TextLayout::content_key))
            .or_else(|| n.text_layout.as_ref().map(|tl| tl.address() as u64))
            .unwrap_or(0)
    }

    /// Lightweight O(1) fingerprint for long field `i`'s `lines_box_id` container (zero text hashing).
    pub(in crate::a11y) fn a11y_lines_box_print(&self, i: usize, w: &Walk) -> u64 {
        let n = &self.tree.nodes[i];
        let mut h = rustc_hash::FxHasher::default();
        n.text_layout.as_ref().map(crate::text::TextLayout::address).hash(&mut h);
        self.a11y_long_content_key(i).hash(&mut h);
        n.text.as_deref().map_or(0, |t| t.content.len()).hash(&mut h);
        self.paint.drawn[i].map(f32::to_bits).hash(&mut h);
        w.shift.map(f32::to_bits).hash(&mut h);
        w.clip_y.map(f32::to_bits).hash(&mut h);
        self.env.scale.to_bits().hash(&mut h);
        self.paint.drawn_scale.get(i).map(|v| v.to_bits()).hash(&mut h);
        self.edit.input_scroll.get(&n.path).map(|v| v.to_bits()).hash(&mut h);
        if let Some(input) = n.input.as_deref() {
            let e = &input.edit;
            let upstream = e.focused && e.anchor == e.focus && e.affinity == parley::Affinity::Upstream;
            (e.text.len(), e.anchor, e.focus, e.focused, upstream, e.preedit().map(|p| (p.0.len(), p.1))).hash(&mut h);
        }
        h.finish()
    }

    /// Builds `(lines_box_id, box_node)` followed by its drawn `(drawn_line_id, line_node)`s.
    pub(in crate::a11y) fn a11y_build_lines_box(&self, i: usize, w: &Walk) -> Vec<(AkId, AkNode)> {
        let n = &self.tree.nodes[i];
        let box_id = lines_box_id(n.id);
        let mut box_node = AkNode::new(Role::GenericContainer);
        box_node.set_class_name(LINES_BOX_CLASS);
        let Some(input) = n.input.as_deref() else {
            return vec![(box_id, box_node)];
        };
        let content_key = self.a11y_long_content_key(i);
        let upstream = u8::from(
            input.edit.focused
                && input.edit.anchor == input.edit.focus
                && input.edit.affinity == parley::Affinity::Upstream,
        );
        box_node.set_author_id(format!("{content_key:x}:{upstream}"));

        let drawn = self.a11y_drawn_lines(i, w.clip_y);
        let preedit_info = input.edit.preedit().map(|(p, cur)| {
            let r = input.edit.range();
            let caret_in_pre = cur.map_or(p.len(), |(_, b)| b.min(p.len()));
            (r.0, r.1, p.len(), caret_in_pre)
        });
        let model_text = input.edit.text.as_str();
        let model_len = model_text.len();
        let map_disp = |d: usize, is_end: bool| -> usize {
            let Some((sel_a, sel_b, p_len, _)) = preedit_info else {
                return d.min(model_len);
            };
            if d < sel_a || (d == sel_a && !is_end) {
                d.min(model_len)
            } else if d < sel_a + p_len {
                sel_b.min(model_len)
            } else {
                (d - sel_a - p_len + sel_b).min(model_len)
            }
        };

        let mut out = Vec::with_capacity(1 + drawn.len());
        let mut child_ids = Vec::with_capacity(drawn.len());
        for (k, l) in drawn.into_iter().take(MAX_DRAWN_LINES).enumerate() {
            let disp_range = l.range.clone();
            let (a, b) = (map_disp(disp_range.start, false), map_disp(disp_range.end, true));
            let id = drawn_line_id(n.id, k);
            let mut line_node = AkNode::new(Role::GenericContainer);
            line_node.set_class_name(LINE_CLASS);
            line_node.set_author_id(format!("{a}:{b}"));
            line_node.set_text_direction(if l.rtl { TextDirection::RightToLeft } else { TextDirection::LeftToRight });
            line_node.set_bounds(AkRect {
                x0: round(l.rect[0] + w.shift[0]), y0: round(l.rect[1] + w.shift[1]),
                x1: round(l.rect[0] + l.rect[2] + w.shift[0]), y1: round(l.rect[1] + l.rect[3] + w.shift[1]),
            });
            let (lengths, positions, widths) = match preedit_info {
                Some((sel_a, sel_b, p_len, caret_in_pre))
                    if disp_range.start < sel_a + p_len && disp_range.end > sel_a =>
                {
                    fold_line_preedit(
                        model_text,
                        disp_range,
                        l.rect[2],
                        l.rtl,
                        (sel_a, sel_b, p_len, caret_in_pre),
                        &l.lengths,
                        &l.positions,
                        &l.widths,
                    )
                }
                _ => (l.lengths, l.positions, l.widths),
            };
            line_node.set_character_lengths(lengths);
            line_node.set_character_positions(positions);
            line_node.set_character_widths(widths);
            child_ids.push(id);
            out.push((id, line_node));
        }
        box_node.set_children(child_ids);
        out.insert(0, (box_id, box_node));
        out
    }

    /// The shaped visual lines of long field `i` inside window clip band `clip_y` (plus its
    /// focused caret visual line when outside `clip_y`, kept intentionally so Windows Magnifier
    /// can locate an off-screen caret), in points in its content space.
    fn a11y_drawn_lines(&self, i: usize, clip_y: [f32; 2]) -> Vec<Line> {
        let n = &self.tree.nodes[i];
        let (Some(spec), Some(tl), Some(input)) = (n.text.as_deref(), n.text_layout.as_ref(), n.input.as_deref()) else {
            return Vec::new();
        };
        if n.elided.is_some() { return Vec::new(); }
        // `spec.content` is already the laid-out display string (including any IME preedit), so no
        // copy of a multi-megabyte buffer is ever made here.
        let content = spec.content.as_str();
        let layout = tl.view();
        let (origin, px) = self.a11y_text_origin(i, spec, layout);
        let layout_h = layout.spec_height(spec.style, self.env.scale, spec.exact_lines);
        let raw_top = (clip_y[0] - origin[1]) * px;
        let raw_bot = (clip_y[1] - origin[1]) * px;
        let mut out = Vec::new();
        if clip_y[1] > clip_y[0] && raw_bot >= 0.0 && raw_top <= layout_h {
            let top_px = raw_top.max(0.0);
            let bot_px = raw_bot.max(top_px);
            for p in layout.pieces_in(top_px, bot_px).filter(|p| p.shaped) {
                if out.len() >= MAX_DRAWN_LINES { break; }
                push_piece_lines_in(p, content, px, origin, Some((top_px, bot_px)), MAX_DRAWN_LINES, &mut out);
            }
        }
        if input.edit.focused && out.len() < MAX_DRAWN_LINES {
            let caret = if input.edit.preedit().is_some() {
                caret_index(&input.edit, content)
            } else {
                input.edit.display_index(input.edit.focus)
            }
            .min(content.len());
            let upstream = input.edit.anchor == input.edit.focus && input.edit.affinity == parley::Affinity::Upstream;
            let covered = out.iter().any(|l| {
                (l.range.start <= caret && caret < l.range.end)
                    || (l.range.start == caret && l.range.end == caret)
                    || (caret == l.range.end && caret == content.len() && !content.ends_with('\n'))
            });
            if !covered {
                let cp = layout.piece_at(caret);
                if cp.shaped {
                    let shaped_caret = cp.shaped(caret);
                    let total = cp.layout.len();
                    if (cp.is_blank() || total == 0) && !cp.is_whole() {
                        let mut single = Vec::new();
                        push_piece_lines_in(cp, content, px, origin, None, 1, &mut single);
                        if let Some(cl) = single.pop() {
                            let pos = out.partition_point(|l| l.range.start < cl.range.start);
                            out.insert(pos, cl);
                        }
                    } else if total > 0 {
                        let (mut lo, mut hi) = (0usize, total - 1);
                        while lo < hi {
                            let mid = lo + (hi - lo) / 2;
                            let end = cp.layout.get(mid).map_or(0, |ln| ln.text_range().end);
                            if end < shaped_caret || (end == shaped_caret && !upstream) {
                                lo = mid + 1;
                            } else {
                                hi = mid;
                            }
                        }
                        let li = lo;
                        if let Some(ln) = cp.layout.get(li)
                            && let Some(mut cl) = build_visual_line(&cp, ln, content, px, origin)
                        {
                            // A whole layout's piece ends at `usize::MAX`, past any break.
                            let br = if cp.is_whole() {
                                0
                            } else if content.as_bytes().get(cp.end..cp.end + 2) == Some(b"\r\n") {
                                2
                            } else if content.as_bytes().get(cp.end) == Some(&b'\n') {
                                1
                            } else {
                                0
                            };
                            if li + 1 == total && br > 0 {
                                cl.range.end += br;
                                cl.lengths.push(br as u8);
                                cl.positions.push(if cl.rtl { 0.0 } else { cl.rect[2] });
                                cl.widths.push(0.0);
                            }
                            let pos = out.partition_point(|l| l.range.start < cl.range.start);
                            out.insert(pos, cl);
                        }
                    }
                }
            }
        }
        out
    }
}
