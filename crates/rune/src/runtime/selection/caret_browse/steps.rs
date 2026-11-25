//! Mounted horizontal, vertical, line-edge, page, and selection-collapse step helpers for
//! `El::caret_browsing(true)`, delegating to `virtual_nav` when a move crosses into an
//! unmounted virtual row.

use super::{StepOut, horiz_dist, pill_ranges, snap_out_of_pill};
use crate::edit::{CaretArrow, CaretLanding, CaretMove};
use crate::runtime::RuntimeCore;
use crate::runtime::selectable::virtual_selection::EndRef;
use crate::text::TextLayout;

impl RuntimeCore {

    /// Plain Left/Right collapse of a non-empty selection to its start or end edge.
    pub(super) fn collapse_caret_selection(
        &self,
        host: usize,
        anchor_end: EndRef,
        focus_node: usize,
        focus_byte: usize,
        right: bool,
    ) -> StepOut {
        let Some(sel) = self.edit.selectable.phase.selection() else {
            return StepOut::Mounted(focus_node, focus_byte, parley::Affinity::Downstream);
        };
        if anchor_end == EndRef::Mounted(focus_node) {
            let content = self.content_of(focus_node);
            let a = content.floor_char_boundary(sel.anchor.min(focus_byte).min(content.len()));
            let b = content.floor_char_boundary(sel.anchor.max(focus_byte).min(content.len()));
            let n = &self.tree.nodes[focus_node];
            let flow = n.text_layout.as_ref().map(TextLayout::view);
            let to = match (flow, n.elided) {
                (Some(l), Some(e)) => {
                    let da = e.range(a, a).0;
                    let db = e.range(b, b).0;
                    let df = e.range(focus_byte, focus_byte).0;
                    e.content(crate::edit::collapse_edge(Some(l), da, db, df, right)).min(content.len())
                }
                (l, None) => crate::edit::collapse_edge(l, a, b, focus_byte, right),
                (None, Some(_)) => if right { b } else { a },
            };
            return StepOut::Mounted(focus_node, to, parley::Affinity::Downstream);
        }
        let anchor_first = self.tree_order(anchor_end) < self.tree_order(EndRef::Mounted(focus_node));
        let forward = right == (self.tree.nodes[host].flow == crate::el::Flow::Ltr);
        let pick_focus = forward == anchor_first;
        if pick_focus {
            StepOut::Mounted(focus_node, focus_byte, parley::Affinity::Downstream)
        } else {
            match anchor_end {
                EndRef::Mounted(ai) => {
                    let content = self.content_of(ai);
                    StepOut::Mounted(ai, content.floor_char_boundary(sel.anchor.min(content.len())), parley::Affinity::Downstream)
                }
                EndRef::Offscreen(vi, ti) => {
                    if let Some(target) = self.virtual_offscreen_collapse_target(host, vi, ti, sel.anchor) {
                        StepOut::Seek(target)
                    } else {
                        StepOut::Mounted(focus_node, if forward { self.content_of(focus_node).len() } else { 0 }, parley::Affinity::Downstream)
                    }
                }
            }
        }
    }

    /// One horizontal character or word step, crossing mounted or unmounted sibling texts at boundaries.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn step_horizontal(
        &self,
        host: usize,
        texts: &[usize],
        focus_pos: usize,
        focus_byte: usize,
        affinity: parley::Affinity,
        right: bool,
        word: bool,
        shift: bool,
    ) -> StepOut {
        let i = texts[focus_pos];
        let n = &self.tree.nodes[i];
        let Some(spec) = n.text.as_ref() else {
            return StepOut::Mounted(i, focus_byte, affinity);
        };
        let content = spec.content_str();
        let pills = pill_ranges(spec);

        if let Some(laid) = n.text_layout.as_ref() {
            let flow = laid.view();
            let arrow = match (right, word) {
                (false, false) => CaretArrow::Left,
                (true, false) => CaretArrow::Right,
                (false, true) => CaretArrow::WordLeft,
                (true, true) => CaretArrow::WordRight,
            };
            let drawn = n.elided.map_or(focus_byte, |e| e.range(focus_byte, focus_byte).0);
            if let Some((landing, dir)) = crate::edit::visual_in(flow, drawn, affinity, arrow, false) {
                match landing {
                    CaretLanding::At(mut j, mut side) => {
                        if word && !cfg!(target_os = "macos") {
                            let step = if right { CaretArrow::Right } else { CaretArrow::Left };
                            while crate::edit::space_ahead(flow, j, side, right)
                                && let Some((CaretLanding::At(k, s), _)) = crate::edit::visual_in(flow, j, side, step, false)
                            {
                                (j, side) = (k, s);
                            }
                        }
                        let byte = n.elided.map_or(j, |e| e.content(j)).min(content.len());
                        let forward = byte > focus_byte;
                        let byte = snap_out_of_pill(&pills, byte, forward);
                        return StepOut::Mounted(i, byte, side);
                    }
                    CaretLanding::Edge => {
                        let forward = right == (dir == crate::el::Flow::Ltr);
                        return self.step_logical(host, texts, focus_pos, focus_byte, forward, word, shift, &pills);
                    }
                }
            }
        }

        self.step_logical(host, texts, focus_pos, focus_byte, right, word, shift, &pills)
    }

    #[allow(clippy::too_many_arguments)]
    fn step_logical(
        &self,
        host: usize,
        texts: &[usize],
        focus_pos: usize,
        focus_byte: usize,
        forward: bool,
        word: bool,
        shift: bool,
        pills: &[(usize, usize)],
    ) -> StepOut {
        let i = texts[focus_pos];
        let content = self.content_of(i);
        if forward {
            if focus_byte < content.len() {
                let next = if word {
                    crate::edit::next_word_in(content, focus_byte, pills)
                } else {
                    crate::edit::next_char(content, focus_byte)
                };
                let next = snap_out_of_pill(pills, next, true);
                return StepOut::Mounted(i, next, parley::Affinity::Downstream);
            }
            let next_same_row = texts.get(focus_pos + 1).copied().is_some_and(|n| self.same_virtual_row(i, n));
            if !next_same_row {
                let mov = if word { CaretMove::WordRight } else { CaretMove::Right };
                if let Some(target) = self.virtual_boundary_target(host, texts, focus_pos, i, mov, shift, None) {
                    return StepOut::Seek(target);
                }
            }
            if focus_pos + 1 < texts.len() {
                let next_node = texts[focus_pos + 1];
                let next_byte = if word && cfg!(target_os = "macos")
                    && let Some(next_spec) = self.tree.nodes[next_node].text.as_ref()
                {
                    let next_pills = pill_ranges(next_spec);
                    crate::edit::next_word_in(next_spec.content_str(), 0, &next_pills)
                } else {
                    0
                };
                return StepOut::Mounted(next_node, next_byte, parley::Affinity::Downstream);
            }
            return StepOut::Mounted(i, content.len(), parley::Affinity::Downstream);
        }
        if focus_byte > 0 {
            let prev = if word {
                crate::edit::prev_word_in(content, focus_byte, pills)
            } else {
                crate::edit::prev_char(content, focus_byte)
            };
            let prev = snap_out_of_pill(pills, prev, false);
            return StepOut::Mounted(i, prev, parley::Affinity::Downstream);
        }
        let prev_same_row = focus_pos.checked_sub(1).and_then(|p| texts.get(p).copied()).is_some_and(|p| self.same_virtual_row(i, p));
        if !prev_same_row {
            let mov = if word { CaretMove::WordLeft } else { CaretMove::Left };
            if let Some(target) = self.virtual_boundary_target(host, texts, focus_pos, i, mov, shift, None) {
                return StepOut::Seek(target);
            }
        }
        if focus_pos > 0 {
            let prev_node = texts[focus_pos - 1];
            let Some(prev_spec) = self.tree.nodes[prev_node].text.as_ref() else {
                return StepOut::Mounted(prev_node, 0, parley::Affinity::Downstream);
            };
            let prev_len = prev_spec.content_str().len();
            let prev_byte = if word {
                let prev_pills = pill_ranges(prev_spec);
                crate::edit::prev_word_in(prev_spec.content_str(), prev_len, &prev_pills)
            } else {
                prev_len
            };
            return StepOut::Mounted(prev_node, prev_byte, parley::Affinity::Downstream);
        }
        StepOut::Mounted(i, 0, parley::Affinity::Downstream)
    }

    /// Visual line start or end inside `i`, preserving soft-wrapped end affinity.
    pub(super) fn step_line_edge(&self, i: usize, byte: usize, affinity: parley::Affinity, end: bool) -> (usize, parley::Affinity) {
        let n = &self.tree.nodes[i];
        let content = self.content_of(i);
        if let Some(pt) = n.text.as_deref().and_then(crate::el::TextSpec::positioned) {
            return (pt.line_edge(byte, end), parley::Affinity::Downstream);
        }
        if let Some(laid) = n.text_layout.as_ref() {
            let drawn = n.elided.map_or(byte, |e| e.range(byte, byte).0);
            let (edge, aff) = laid.view().line_edge_with_affinity(drawn, affinity, end);
            let mapped = n.elided.map_or(edge, |e| e.content(edge)).min(content.len());
            return (content.floor_char_boundary(mapped), aff);
        }
        let edge = if end {
            content[byte..].find('\n').map_or(content.len(), |k| byte + k)
        } else {
            content[..byte].rfind('\n').map_or(0, |k| k + 1)
        };
        (edge, parley::Affinity::Downstream)
    }

    /// Groups `texts` into vertical bands (so side-by-side table cells share a band) and finds the
    /// nearest text in the band strictly below (`down`) or above (`!down`) `cur_node`.
    fn adjacent_band_text(&self, texts: &[usize], cur_pos: usize, down: bool, goal_x: f32) -> Option<usize> {
        let cur = texts[cur_pos];
        let cur_r = self.tree.nodes[cur].target;
        let mut candidates: Vec<(usize, [f32; 4])> = texts
            .iter()
            .copied()
            .filter(|&j| j != cur)
            .filter_map(|j| {
                let r = self.tree.nodes[j].target;
                let ok = if down { r[1] >= cur_r[3] - 1.0 } else { r[3] <= cur_r[1] + 1.0 };
                ok.then_some((j, r))
            })
            .collect();
        if candidates.is_empty() {
            return if down {
                texts.get(cur_pos + 1).copied()
            } else {
                cur_pos.checked_sub(1).and_then(|p| texts.get(p).copied())
            };
        }
        let band_edge = if down {
            candidates.iter().map(|(_, r)| r[1]).fold(f32::INFINITY, f32::min)
        } else {
            candidates.iter().map(|(_, r)| r[3]).fold(f32::NEG_INFINITY, f32::max)
        };
        candidates.retain(|(_, r)| if down { r[1] < band_edge + 2.0 } else { r[3] > band_edge - 2.0 });
        candidates
            .into_iter()
            .min_by(|a, b| horiz_dist(a.1[0], a.1[2], goal_x).total_cmp(&horiz_dist(b.1[0], b.1[2], goal_x)))
            .map(|(j, _)| j)
    }

    /// One visual line Up or Down with sticky `goal_x`, crossing mounted or unmounted sibling texts.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn step_vertical(
        &self,
        host: usize,
        texts: &[usize],
        focus_pos: usize,
        focus_byte: usize,
        affinity: parley::Affinity,
        down: bool,
        shift: bool,
        goal_x: f32,
    ) -> StepOut {
        let i = texts[focus_pos];
        let n = &self.tree.nodes[i];
        let content = self.content_of(i);
        if let Some(pt) = n.text.as_deref().and_then(crate::el::TextSpec::positioned) {
            let tf = super::super::positioned::PositionedTransform::from_window_box(n, pt, n.target, 1.0);
            let gx = (tf.sx > 1e-6).then(|| (goal_x - tf.ox) / tf.sx);
            if let Some((next_byte, _)) = pt.move_line_strict(focus_byte, down, gx) {
                return StepOut::Mounted(i, next_byte, parley::Affinity::Downstream);
            }
        }
        if let (Some(laid), Some([ox, _])) = (n.text_layout.as_ref(), self.text_unscrolled_origin(i)) {
            let s = self.env.scale;
            let drawn = n.elided.map_or(focus_byte, |e| e.range(focus_byte, focus_byte).0);
            if let Some((next_drawn, next_aff, _)) = laid.view().move_line_strict(drawn, affinity, down, Some((goal_x - ox) * s)) {
                let mapped = n.elided.map_or(next_drawn, |e| e.content(next_drawn)).min(content.len());
                let byte = content.floor_char_boundary(mapped);
                let pills = n.text.as_deref().map(pill_ranges).unwrap_or_default();
                return StepOut::Mounted(i, snap_out_of_pill(&pills, byte, down), next_aff);
            }
        }
        let adj = self.adjacent_band_text(texts, focus_pos, down, goal_x);
        let adj_same_row = adj.is_some_and(|a| self.same_virtual_row(i, a));
        if !adj_same_row {
            let mov = if down { CaretMove::Down } else { CaretMove::Up };
            if let Some(target) = self.virtual_boundary_target(host, texts, focus_pos, i, mov, shift, Some(goal_x)) {
                return StepOut::Seek(target);
            }
        }
        if let Some(adj) = adj {
            let (byte, aff) = self.edge_hit_unscrolled(adj, goal_x, down);
            let pills = self.tree.nodes[adj].text.as_deref().map(pill_ranges).unwrap_or_default();
            return StepOut::Mounted(adj, snap_out_of_pill(&pills, byte, down), aff);
        }
        StepOut::Mounted(i, if down { content.len() } else { 0 }, parley::Affinity::Downstream)
    }

    /// Viewport-height PageUp or PageDown step with sticky `goal_x` across mounted texts.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn step_page(
        &self,
        host: usize,
        texts: &[usize],
        focus_pos: usize,
        focus_byte: usize,
        affinity: parley::Affinity,
        down: bool,
        shift: bool,
        goal_x: f32,
    ) -> StepOut {
        let i = texts[focus_pos];
        let page_h = std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent)
            .find(|&p| self.tree.nodes[p].scroll.is_some() && self.tree.nodes[p].scroll_axes.has(false))
            .map(|v| (self.tree.nodes[v].target[3] - self.tree.nodes[v].target[1]).max(1.0));
        let last_text = texts.last().copied().unwrap_or(texts[0]);
        let Some(page_h) = page_h else {
            return if down {
                StepOut::Mounted(last_text, self.content_of(last_text).len(), parley::Affinity::Downstream)
            } else {
                StepOut::Mounted(texts[0], 0, parley::Affinity::Downstream)
            };
        };
        let cur_y = self
            .caret_unscrolled_pos(i, focus_byte, affinity)
            .map_or_else(|| (self.tree.nodes[i].target[1] + self.tree.nodes[i].target[3]) * 0.5, |p| p[1]);
        let target_y = cur_y + if down { page_h } else { -page_h };

        let covering: Vec<usize> = texts
            .iter()
            .copied()
            .filter(|&j| {
                let r = self.tree.nodes[j].target;
                target_y >= r[1] && target_y <= r[3]
            })
            .collect();
        let nearest_x = |iter: Vec<usize>, fallback: usize| {
            iter.into_iter()
                .min_by(|&a, &b| {
                    let (ra, rb) = (self.tree.nodes[a].target, self.tree.nodes[b].target);
                    horiz_dist(ra[0], ra[2], goal_x).total_cmp(&horiz_dist(rb[0], rb[2], goal_x))
                })
                .unwrap_or(fallback)
        };
        let chosen = if !covering.is_empty() {
            nearest_x(covering, i)
        } else if down {
            let below: Vec<usize> = texts.iter().copied().filter(|&j| self.tree.nodes[j].target[1] > target_y).collect();
            if let Some(&first_below) = below.first() {
                let top = self.tree.nodes[first_below].target[1];
                let band: Vec<usize> = below.into_iter().filter(|&j| self.tree.nodes[j].target[1] < top + 2.0).collect();
                nearest_x(band, first_below)
            } else {
                last_text
            }
        } else {
            let above: Vec<usize> = texts.iter().copied().filter(|&j| self.tree.nodes[j].target[3] < target_y).collect();
            if let Some(&last_above) = above.last() {
                let bottom = self.tree.nodes[last_above].target[3];
                let band: Vec<usize> = above.into_iter().filter(|&j| self.tree.nodes[j].target[3] > bottom - 2.0).collect();
                nearest_x(band, last_above)
            } else {
                texts[0]
            }
        };

        let r = self.tree.nodes[chosen].target;
        let (byte, aff) = if target_y < r[1] {
            self.edge_hit_unscrolled(chosen, goal_x, true)
        } else if target_y > r[3] {
            self.edge_hit_unscrolled(chosen, goal_x, false)
        } else {
            self.hit_unscrolled(chosen, goal_x, target_y)
        };
        let pills = self.tree.nodes[chosen].text.as_deref().map(pill_ranges).unwrap_or_default();
        let byte = snap_out_of_pill(&pills, byte, down);
        if chosen == i && byte == focus_byte {
            return self.step_vertical(host, texts, focus_pos, focus_byte, affinity, down, shift, goal_x);
        }
        StepOut::Mounted(chosen, byte, aff)
    }
}
