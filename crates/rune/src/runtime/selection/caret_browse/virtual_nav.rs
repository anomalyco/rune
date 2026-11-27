//! Virtual-list boundary, paging, document-end, and offscreen-collapse navigation for
//! `El::caret_browsing(true)`, driven by the explicit `pending::State` state machine.

use super::pending::{self, Effect, Event, Landing, Scope, Target};
use super::{horiz_dist, pill_ranges, snap_out_of_pill};
use crate::edit::CaretMove;
use crate::runtime::selectable::phase;
use crate::runtime::selectable::virtual_selection::EndRef;
use crate::runtime::{AppDriver, Commit, RuntimeCore, Tx, scroll_edge};

impl RuntimeCore {
    /// Cancels any in-flight virtual-row caret seek (on blur, clear, pointer press, or new move).
    #[inline]
    pub(in crate::runtime) fn cancel_pending_caret(&mut self) {
        if self.edit.selectable.pending_move.is_seeking() {
            let _ = pending::step(&mut self.edit.selectable.pending_move, Event::Cancel);
        }
    }

    /// Live, non-empty virtual lists belonging to caret-browsing host `host`, in tree order.
    pub(super) fn host_virtual_lists(&self, host: usize) -> Vec<(usize, String)> {
        if self.commit.virtuals.is_empty() {
            return Vec::new();
        }
        let host_path = self.tree.nodes[host].path.as_str();
        let host_end = self.tree.subtree_end(host);
        let mut out = Vec::new();
        for j in host..=host_end {
            if self.subtree_exiting(j) || !self.allowed(j) {
                continue;
            }
            let path = self.tree.nodes[j].path.as_str();
            let Some(v) = self.commit.virtuals.get(path) else {
                continue;
            };
            if v.keys.is_empty() || !self.virtual_list_allowed(path) {
                continue;
            }
            let belongs = j == host
                || v.selection.scope.group_path.as_deref() == Some(host_path)
                || self.group_of(j) == Some(host);
            if belongs {
                out.push((j, path.to_owned()));
            }
        }
        out
    }

    /// Mounted selectable text nodes belonging to `host` inside row `row_key` of `list_path`.
    fn mounted_row_texts(&self, host: usize, list_path: &str, row_key: &str) -> Vec<usize> {
        let Some(list_node) = self.live(list_path) else {
            return Vec::new();
        };
        let row_path = crate::runtime::flatten::node_path(list_path, Some(row_key), 0);
        let Some(row_node) = self.tree.direct_child_by_path(list_node, self.tree.nodes.len(), &row_path) else {
            return Vec::new();
        };
        let row_end = self.tree.subtree_end(row_node);
        (row_node..=row_end)
            .filter(|&j| self.caret_browsing_contains(host, j))
            .collect()
    }

    /// Whether mounted text nodes `a` and `b` live inside the same virtual row.
    pub(super) fn same_virtual_row(&self, a: usize, b: usize) -> bool {
        match (
            self.virtual_end_of(EndRef::Mounted(a), 0),
            self.virtual_end_of(EndRef::Mounted(b), 0),
        ) {
            (Some(va), Some(vb)) => va.list_path == vb.list_path && va.row_key == vb.row_key,
            _ => false,
        }
    }

    /// Whether crossing out of `focus_node` in direction `mov` enters an unmounted virtual row.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn virtual_boundary_target(
        &self,
        host: usize,
        texts: &[usize],
        focus_pos: usize,
        focus_node: usize,
        mov: CaretMove,
        shift: bool,
        goal_x: Option<f32>,
    ) -> Option<Target> {
        let forward = matches!(mov, CaretMove::Down | CaretMove::Right | CaretMove::WordRight);
        let landing = match mov {
            CaretMove::Down => Landing::TopAtX(goal_x.unwrap_or(0.0)),
            CaretMove::Up => Landing::BottomAtX(goal_x.unwrap_or(0.0)),
            CaretMove::Right => Landing::Start,
            CaretMove::Left => Landing::End,
            CaretMove::WordRight => Landing::WordForward,
            CaretMove::WordLeft => Landing::WordBackward,
            _ => return None,
        };
        let host_path = self.tree.nodes[host].path.clone();

        // Case 1: focus is already inside a virtual list row.
        if let Some(ve) = self.virtual_end_of(EndRef::Mounted(focus_node), 0) {
            let v = self.commit.virtuals.get(&ve.list_path)?;
            let next_row = if forward {
                let r = ve.row_index + 1;
                if r >= v.keys.len() {
                    return None;
                }
                r
            } else {
                ve.row_index.checked_sub(1)?
            };
            if v.range.contains(&next_row) {
                return None;
            }
            let row_key = v.keys.get(next_row)?.clone();
            return Some(Target {
                scope: Scope::default(),
                host_path,
                list_path: ve.list_path,
                row_index: next_row,
                row_key,
                rel_path: None,
                landing,
                shift,
                goal_x,
                scan_dir: if forward { 1 } else { -1 },
            });
        }

        // Case 2: focus is a mounted header before a virtual list (moving forward) or a mounted
        // footer after a virtual list (moving backward) whose edge row is unmounted.
        let lists = self.host_virtual_lists(host);
        if lists.is_empty() {
            return None;
        }
        if forward {
            let next_mounted = texts.get(focus_pos + 1).copied();
            for (list_node, list_path) in lists {
                if list_node <= focus_node {
                    continue;
                }
                if next_mounted.is_some_and(|nm| nm < list_node) {
                    return None;
                }
                let v = self.commit.virtuals.get(&list_path)?;
                if !v.keys.is_empty() && !v.range.contains(&0) {
                    return Some(Target {
                        scope: Scope::default(),
                        host_path,
                        list_path,
                        row_index: 0,
                        row_key: v.keys[0].clone(),
                        rel_path: None,
                        landing,
                        shift,
                        goal_x,
                        scan_dir: 1,
                    });
                }
                return None;
            }
        } else {
            let prev_mounted = focus_pos.checked_sub(1).and_then(|p| texts.get(p).copied());
            for (list_node, list_path) in lists.into_iter().rev() {
                let list_end = self.tree.subtree_end(list_node);
                if list_end >= focus_node {
                    continue;
                }
                if prev_mounted.is_some_and(|pm| pm > list_end) {
                    return None;
                }
                let v = self.commit.virtuals.get(&list_path)?;
                let last = v.keys.len().checked_sub(1)?;
                if !v.range.contains(&last) {
                    return Some(Target {
                        scope: Scope::default(),
                        host_path,
                        list_path,
                        row_index: last,
                        row_key: v.keys[last].clone(),
                        rel_path: None,
                        landing,
                        shift,
                        goal_x,
                        scan_dir: -1,
                    });
                }
                return None;
            }
        }
        None
    }

    /// Target for `DocStart` (`!end`) or `DocEnd` (`end`) when the document's first/last row lies
    /// unmounted inside a virtual list under `host`.
    pub(super) fn virtual_doc_edge_target(&self, host: usize, texts: &[usize], end: bool, shift: bool) -> Option<Target> {
        let lists = self.host_virtual_lists(host);
        if lists.is_empty() {
            return None;
        }
        let host_path = self.tree.nodes[host].path.clone();
        if !end {
            let (list_node, list_path) = lists.first()?;
            if texts.first().is_some_and(|&t| t < *list_node) {
                return None;
            }
            let v = self.commit.virtuals.get(list_path)?;
            if v.keys.is_empty() || v.range.contains(&0) {
                return None;
            }
            Some(Target {
                scope: Scope::default(),
                host_path,
                list_path: list_path.clone(),
                row_index: 0,
                row_key: v.keys[0].clone(),
                rel_path: None,
                landing: Landing::Start,
                shift,
                goal_x: None,
                scan_dir: 1,
            })
        } else {
            let (list_node, list_path) = lists.last()?;
            let list_end = self.tree.subtree_end(*list_node);
            if texts.last().is_some_and(|&t| t > list_end) {
                return None;
            }
            let v = self.commit.virtuals.get(list_path)?;
            let last = v.keys.len().checked_sub(1)?;
            if v.range.contains(&last) {
                return None;
            }
            Some(Target {
                scope: Scope::default(),
                host_path,
                list_path: list_path.clone(),
                row_index: last,
                row_key: v.keys[last].clone(),
                rel_path: None,
                landing: Landing::End,
                shift,
                goal_x: None,
                scan_dir: -1,
            })
        }
    }

    /// Target for `PageUp` / `PageDown` when the destination y lands in an unmounted virtual row.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn virtual_page_target(
        &self,
        host: usize,
        texts: &[usize],
        focus_node: usize,
        focus_byte: usize,
        affinity: parley::Affinity,
        down: bool,
        shift: bool,
        goal_x: f32,
    ) -> Option<Target> {
        let ve = self.virtual_end_of(EndRef::Mounted(focus_node), focus_byte);
        let (list_node, list_path, cur_row) = match ve {
            Some(ve) => {
                let ln = self.live(&ve.list_path)?;
                (ln, ve.list_path, Some(ve.row_index))
            }
            None => {
                let lists = self.host_virtual_lists(host);
                let chosen = if down {
                    lists.into_iter().find(|&(ln, _)| ln > focus_node)?
                } else {
                    lists.into_iter().rev().find(|&(ln, _)| self.tree.subtree_end(ln) < focus_node)?
                };
                (chosen.0, chosen.1, None)
            }
        };
        let v = self.commit.virtuals.get(&list_path)?;
        if v.keys.is_empty() {
            return None;
        }
        let vp_node = self.live(&v.scroll_path)?;
        let vp_h = (self.tree.nodes[vp_node].target[3] - self.tree.nodes[vp_node].target[1]).max(1.0);
        let cur_y = self
            .caret_unscrolled_pos(focus_node, focus_byte, affinity)
            .map_or_else(|| (self.tree.nodes[focus_node].target[1] + self.tree.nodes[focus_node].target[3]) * 0.5, |p| p[1]);
        let list_top = self.tree.nodes[list_node].target[1];
        let target_y = cur_y + if down { vp_h } else { -vp_h };
        let target_in_list = target_y - list_top;

        // If paging past the list's ends into a mounted header/footer, let mounted paging handle it.
        let total_h = v.pad_top + v.prefix(v.keys.len());
        if !down && target_in_list < v.pad_top && texts.first().is_some_and(|&t| t < list_node) {
            return None;
        }
        if down && target_in_list > total_h && texts.last().is_some_and(|&t| t > self.tree.subtree_end(list_node)) {
            return None;
        }

        let mut target_row = v.row_at(target_in_list)?;
        if let Some(cr) = cur_row && target_row == cr {
            if down && cr + 1 < v.keys.len() && self.tree.nodes[focus_node].target[3] < target_y {
                target_row = cr + 1;
            } else if !down && cr > 0 && self.tree.nodes[focus_node].target[1] > target_y {
                target_row = cr - 1;
            }
        }
        if v.range.contains(&target_row) {
            return None;
        }
        let row_key = v.keys.get(target_row)?.clone();
        Some(Target {
            scope: Scope::default(),
            host_path: self.tree.nodes[host].path.clone(),
            list_path,
            row_index: target_row,
            row_key,
            rel_path: None,
            landing: Landing::PageAt { goal_x, list_y: target_in_list, down },
            shift,
            goal_x: Some(goal_x),
            scan_dir: if down { 1 } else { -1 },
        })
    }

    /// Target for collapsing a selection back onto an offscreen pinned virtual-row anchor.
    pub(super) fn virtual_offscreen_collapse_target(&self, host: usize, vi: usize, ti: usize, offset: usize) -> Option<Target> {
        let t = self.offscreen_text(vi, ti);
        let v = self.commit.virtuals.get(&t.list_path)?;
        let row_index = v.index_of(&t.row_key)?;
        Some(Target {
            scope: Scope::default(),
            host_path: self.tree.nodes[host].path.clone(),
            list_path: t.list_path.clone(),
            row_index,
            row_key: t.row_key.clone(),
            rel_path: Some(t.rel_path.clone()),
            landing: Landing::ExactOffset { offset },
            shift: false,
            goal_x: None,
            scan_dir: 0,
        })
    }

    /// Target when the selection's focus itself scrolled offscreen (e.g. via mouse wheel) and the
    /// user presses a caret navigation key.
    pub(super) fn virtual_offscreen_focus_target(
        &self,
        host: usize,
        texts: &[usize],
        (vi, ti): (usize, usize),
        focus_byte: usize,
        mov: CaretMove,
        shift: bool,
    ) -> Option<Target> {
        if matches!(mov, CaretMove::DocStart | CaretMove::DocEnd) {
            return self.virtual_doc_edge_target(host, texts, mov == CaretMove::DocEnd, shift);
        }
        let t = self.offscreen_text(vi, ti);
        let list_path = t.list_path.clone();
        let rel_path = t.rel_path.clone();
        let v = self.commit.virtuals.get(&list_path)?;
        let cur_row = v.index_of(&t.row_key)?;
        let host_path = self.tree.nodes[host].path.clone();
        let (row_index, landing, scan_dir) = match mov {
            CaretMove::Down => {
                let r = (cur_row + 1).min(v.keys.len().saturating_sub(1));
                (r, Landing::Start, 1)
            }
            CaretMove::Up => (cur_row.saturating_sub(1), Landing::End, -1),
            CaretMove::Right | CaretMove::WordRight => {
                let r = (cur_row + 1).min(v.keys.len().saturating_sub(1));
                (r, if mov == CaretMove::WordRight { Landing::WordForward } else { Landing::Start }, 1)
            }
            CaretMove::Left | CaretMove::WordLeft => {
                (cur_row.saturating_sub(1), if mov == CaretMove::WordLeft { Landing::WordBackward } else { Landing::End }, -1)
            }
            CaretMove::PageDown | CaretMove::PageUp => {
                let down = mov == CaretMove::PageDown;
                let vp_h = self.live(&v.scroll_path)
                    .map_or(v.viewport.max(1.0), |vp| (self.tree.nodes[vp].target[3] - self.tree.nodes[vp].target[1]).max(1.0));
                let cur_y = v.pad_top + v.prefix(cur_row) + v.height(&t.row_key) * 0.5;
                let target_y = cur_y + if down { vp_h } else { -vp_h };
                let r = v.row_at(target_y).unwrap_or(cur_row);
                (r, Landing::PageAt { goal_x: 0.0, list_y: target_y, down }, if down { 1 } else { -1 })
            }
            CaretMove::LineStart => (cur_row, Landing::Start, 0),
            CaretMove::LineEnd => (cur_row, Landing::End, 0),
            CaretMove::DocStart | CaretMove::DocEnd => unreachable!(),
        };
        let row_key = v.keys.get(row_index)?.clone();
        Some(Target {
            scope: Scope::default(),
            host_path,
            list_path,
            row_index,
            row_key,
            rel_path: if row_index == cur_row && matches!(landing, Landing::ExactOffset { .. }) {
                Some(rel_path)
            } else {
                None
            },
            landing: match landing {
                Landing::ExactOffset { .. } => Landing::ExactOffset { offset: focus_byte },
                other => other,
            },
            shift,
            goal_x: None,
            scan_dir,
        })
    }

    /// Starts a pending virtual-row caret move and drives it to landing (or cancellation) within
    /// the bounded `MAX_SEEK_HOPS` scroll/remount rounds.
    pub(super) fn request_and_drive_pending_caret(&mut self, app: &mut dyn AppDriver, mut target: Target, scope: Scope) {
        target.scope = scope;
        let effects = pending::step(&mut self.edit.selectable.pending_move, Event::Request(target));
        for eff in effects {
            self.run_pending_effect(app, eff);
        }
        self.drive_pending_caret(app);
    }

    /// Resolves the live host node for a pending virtual-row target, respecting whether the seek
    /// belongs to a focused `El::caret_browsing(true)` host or an active plain selection.
    fn pending_target_host(&self, target: &Target) -> Option<usize> {
        match target.scope {
            Scope::FocusedCaretHost => self
                .caret_browsing_focused_host()
                .filter(|&h| self.tree.nodes[h].path == target.host_path),
            Scope::ActiveSelection => {
                self.selection_holder_end()?;
                self.live(&target.host_path)
                    .filter(|&h| self.tree.nodes[h].selection_group || self.is_selectable(h))
            }
        }
    }

    /// Checks an in-flight `PendingCaretMove` against the committed tree and executes its
    /// effects until it lands or returns to `Idle`. Never leaves idle wakeups behind.
    pub(in crate::runtime) fn drive_pending_caret(&mut self, app: &mut dyn AppDriver) {
        for _ in 0..=pending::MAX_SEEK_HOPS + 1 {
            let Some(target) = self.edit.selectable.pending_move.target().cloned() else {
                return;
            };
            let event = self.inspect_pending_target(&target);
            let effects = pending::step(&mut self.edit.selectable.pending_move, event);
            if effects.is_empty() {
                return;
            }
            for eff in effects {
                self.run_pending_effect(app, eff);
            }
        }
        self.cancel_pending_caret();
    }

    fn inspect_pending_target(&self, target: &Target) -> Event {
        let host = self.pending_target_host(target);
        let Some(host) = host else {
            return Event::Check {
                host_valid: false,
                current_index: None,
                mounted: false,
                has_selectable: false,
                next_in_scan: None,
            };
        };
        let Some(v) = self.commit.virtuals.get(&target.list_path).filter(|_| self.virtual_list_allowed(&target.list_path)) else {
            return Event::Check {
                host_valid: false,
                current_index: None,
                mounted: false,
                has_selectable: false,
                next_in_scan: None,
            };
        };
        let Some(idx) = v.index_of(&target.row_key) else {
            return Event::Check {
                host_valid: true,
                current_index: None,
                mounted: false,
                has_selectable: false,
                next_in_scan: None,
            };
        };
        let mounted = v.range.contains(&idx);
        let has_selectable = mounted && !self.mounted_row_texts(host, &target.list_path, &target.row_key).is_empty();
        let next_in_scan = if mounted && !has_selectable && target.scan_dir != 0 {
            let ni = if target.scan_dir > 0 {
                (idx + 1 < v.keys.len()).then_some(idx + 1)
            } else {
                idx.checked_sub(1)
            };
            ni.and_then(|r| v.keys.get(r).map(|k| (r, k.clone())))
        } else {
            None
        };
        Event::Check {
            host_valid: true,
            current_index: Some(idx),
            mounted,
            has_selectable,
            next_in_scan,
        }
    }

    fn run_pending_effect(&mut self, app: &mut dyn AppDriver, effect: Effect) {
        match effect {
            Effect::ScrollAndMount { list_path, row_index, row_key, page_y, down } => {
                self.scroll_and_mount_virtual_row(app, &list_path, row_index, &row_key, page_y, down);
            }
            Effect::Land(target) => {
                self.land_pending_caret(target);
            }
        }
    }

    fn scroll_and_mount_virtual_row(
        &mut self,
        app: &mut dyn AppDriver,
        list_path: &str,
        row_index: usize,
        row_key: &str,
        page_y: Option<f32>,
        down: bool,
    ) {
        let Some((scroll_path, row_top, row_h, viewport, already_mounted, list_top_in_scroll)) =
            self.commit.virtuals.get(list_path).map(|v| {
                (
                    v.scroll_path.clone(),
                    f64::from(v.top + v.pad_top + v.prefix(row_index)),
                    f64::from(v.height(row_key)).max(1.0),
                    f64::from(v.viewport).max(1.0),
                    v.range.contains(&row_index),
                    f64::from(v.top),
                )
            })
        else {
            return;
        };
        let Some(s_idx) = self.live(&scroll_path) else {
            return;
        };
        let id = self.tree.nodes[s_idx].id;
        let margin = f64::from(self.tree.nodes[s_idx].scroll_edges.margin);
        let stick = self.tree.nodes[s_idx].scroll.is_some_and(|s| s.stick_bottom);
        let actual_vp = f64::from(self.tree.nodes[s_idx].local.h).max(viewport);
        let max = self.scroll_overflow(s_idx).max(0.0);
        let moved = if let Some(scroll) = self.scroll.scrolls.get_mut(&id) {
            scroll.max = max;
            scroll.halt();
            let before = scroll.offset;
            let next = if let Some(ly) = page_y {
                let shifted = if down {
                    (scroll.offset + actual_vp).clamp(0.0, max)
                } else {
                    (scroll.offset - actual_vp).clamp(0.0, max)
                };
                let target_in_vp = list_top_in_scroll + f64::from(ly);
                let candidate = scroll_edge::reveal(shifted, row_top, row_h, actual_vp, margin).clamp(0.0, max);
                if (candidate - shifted).abs() > actual_vp {
                    (target_in_vp - actual_vp * 0.5).clamp(0.0, max)
                } else {
                    candidate
                }
            } else {
                let revealed = scroll_edge::reveal(scroll.offset, row_top, row_h, actual_vp, margin).clamp(0.0, max);
                if (revealed - scroll.offset).abs() < 1e-6 && !already_mounted {
                    if down {
                        (row_top + row_h - actual_vp + 1.0).clamp(0.0, max)
                    } else {
                        (row_top - 1.0).clamp(0.0, max)
                    }
                } else {
                    revealed
                }
            };
            scroll.offset = next;
            scroll.land(stick);
            scroll.moved();
            let m = (scroll.offset - before).abs() > 1e-6;
            if m {
                scroll.note_source(crate::ScrollSource::Programmatic);
            }
            m
        } else {
            false
        };
        if moved || !already_mounted || self.virtual_window_changed() {
            self.commit_virtual_scroll(app, &Tx { commit: Commit::Instant, ..Tx::default() });
            if let Some(s_idx) = self.live(&scroll_path) {
                let max = self.scroll_overflow(s_idx).max(0.0);
                let sid = self.tree.nodes[s_idx].id;
                if let Some(scroll) = self.scroll.scrolls.get_mut(&sid) {
                    scroll.max = max;
                    scroll.offset = scroll.offset.clamp(0.0, max);
                }
            }
            self.step_scroll(0.0);
        }
    }

    fn land_pending_caret(&mut self, target: Target) {
        let Some(host) = self.pending_target_host(&target) else {
            return;
        };
        let row_texts = self.mounted_row_texts(host, &target.list_path, &target.row_key);
        let Some(&first) = row_texts.first() else {
            return;
        };
        let last = row_texts.last().copied().unwrap_or(first);

        let nearest_band = |top_band: bool, gx: f32| -> usize {
            let edge = if top_band {
                row_texts.iter().map(|&j| self.tree.nodes[j].target[1]).fold(f32::INFINITY, f32::min)
            } else {
                row_texts.iter().map(|&j| self.tree.nodes[j].target[3]).fold(f32::NEG_INFINITY, f32::max)
            };
            row_texts
                .iter()
                .copied()
                .filter(|&j| {
                    let r = self.tree.nodes[j].target;
                    if top_band { r[1] < edge + 2.0 } else { r[3] > edge - 2.0 }
                })
                .min_by(|&a, &b| {
                    let (ra, rb) = (self.tree.nodes[a].target, self.tree.nodes[b].target);
                    horiz_dist(ra[0], ra[2], gx).total_cmp(&horiz_dist(rb[0], rb[2], gx))
                })
                .unwrap_or(if top_band { first } else { last })
        };

        let (dest_node, dest_byte, dest_aff) = match target.landing {
            Landing::Start => (first, 0, parley::Affinity::Downstream),
            Landing::End => (last, self.content_of(last).len(), parley::Affinity::Downstream),
            Landing::WordForward => {
                let byte = if cfg!(target_os = "macos")
                    && let Some(spec) = self.tree.nodes[first].text.as_ref()
                {
                    let pills = pill_ranges(spec);
                    crate::edit::next_word_in(spec.content_str(), 0, &pills)
                } else {
                    0
                };
                (first, byte, parley::Affinity::Downstream)
            }
            Landing::WordBackward => {
                let byte = self.tree.nodes[last].text.as_ref().map_or(0, |spec| {
                    let pills = pill_ranges(spec);
                    crate::edit::prev_word_in(spec.content_str(), spec.content_str().len(), &pills)
                });
                (last, byte, parley::Affinity::Downstream)
            }
            Landing::TopAtX(gx) => {
                let chosen = nearest_band(true, gx);
                let (b, aff) = self.edge_hit_unscrolled(chosen, gx, true);
                let pills = self.tree.nodes[chosen].text.as_deref().map(pill_ranges).unwrap_or_default();
                (chosen, snap_out_of_pill(&pills, b, true), aff)
            }
            Landing::BottomAtX(gx) => {
                let chosen = nearest_band(false, gx);
                let (b, aff) = self.edge_hit_unscrolled(chosen, gx, false);
                let pills = self.tree.nodes[chosen].text.as_deref().map(pill_ranges).unwrap_or_default();
                (chosen, snap_out_of_pill(&pills, b, false), aff)
            }
            Landing::PageAt { goal_x, list_y, down } => {
                let list_top = self.live(&target.list_path).map_or(0.0, |ln| self.tree.nodes[ln].target[1]);
                let target_y = list_top + list_y;
                let chosen = row_texts
                    .iter()
                    .copied()
                    .min_by(|&a, &b| {
                        let (ra, rb) = (self.tree.nodes[a].target, self.tree.nodes[b].target);
                        let dy = |r: [f32; 4]| (r[1] - target_y).max(target_y - r[3]).max(0.0);
                        dy(ra)
                            .total_cmp(&dy(rb))
                            .then_with(|| horiz_dist(ra[0], ra[2], goal_x).total_cmp(&horiz_dist(rb[0], rb[2], goal_x)))
                    })
                    .unwrap_or(if down { first } else { last });
                let r = self.tree.nodes[chosen].target;
                let (b, aff) = if target_y < r[1] {
                    self.edge_hit_unscrolled(chosen, goal_x, true)
                } else if target_y > r[3] {
                    self.edge_hit_unscrolled(chosen, goal_x, false)
                } else {
                    self.hit_unscrolled(chosen, goal_x, target_y)
                };
                let pills = self.tree.nodes[chosen].text.as_deref().map(pill_ranges).unwrap_or_default();
                (chosen, snap_out_of_pill(&pills, b, down), aff)
            }
            Landing::ExactOffset { offset } => {
                let row_path = crate::runtime::flatten::node_path(&target.list_path, Some(&target.row_key), 0);
                let chosen = target
                    .rel_path
                    .as_deref()
                    .and_then(|rel| {
                        let full = if rel.is_empty() { row_path.clone() } else { format!("{row_path}/{rel}") };
                        row_texts.iter().copied().find(|&j| self.tree.nodes[j].path == full)
                    })
                    .unwrap_or(first);
                let content = self.content_of(chosen);
                let clamped = content.floor_char_boundary(offset.min(content.len()));
                (chosen, clamped, parley::Affinity::Downstream)
            }
        };

        let dest_path = self.tree.nodes[dest_node].path.clone();
        let dest_content = self.content_of(dest_node).to_owned();
        let dest_byte = dest_content.floor_char_boundary(dest_byte.min(dest_content.len()));

        if target.shift && let Some(anchor_end) = self.selection_holder_end() {
            let far = (EndRef::Mounted(dest_node) != anchor_end).then(|| phase::Far::new(dest_path, dest_content));
            self.select_step(phase::Event::SetFocus { focus: dest_byte, far });
            #[cfg(any(target_os = "linux", test))]
            if self.has_nonempty_selection() {
                self.primary_selection_changed();
            }
        } else {
            self.select_step(phase::Event::Select(phase::Selection::new(
                dest_path,
                dest_content,
                (dest_byte, dest_byte),
                (dest_byte, dest_byte),
            )));
        }
        self.edit.selectable.goal_x = target.goal_x;
        self.edit.selectable.affinity = dest_aff;
        self.edit.caret_follow = true;
    }
}
