//! Remounting [`El::virtual_rows`](crate::El::virtual_rows) windows when a commit only shifted a
//! virtual scroll window or filled overscan (`CommitWhy::VIRTUAL`).
//!
//! With app state, scale, blend mode and layout reads unchanged, non-list nodes and rows already
//! mounted stay as they were: they move directly out of `self.nodes` into the new pass's node
//! buffer without running `view` or calling `RowSource::build` on staying rows. Only newly
//! entering rows (and unmounted focused or selection-pinned rows) build and flatten.

use std::ops::Range;
use std::rc::Rc;

use super::*;

pub(in crate::runtime) enum RemountOutcome {
    Measured(Measured),
    Retry,
    Fallback,
}

struct ListPlan {
    path: String,
    old_list_idx: usize,
    list_subtree_end: usize,
    range: Range<usize>,
    pad_top: f32,
    pad_bottom: f32,
    mounted_rows: HashMap<String, (usize, usize)>,
    focused: Option<usize>,
    pinned: Vec<usize>,
    rows: Rc<crate::el::virtual_rows::RowSource>,
    list_group: Option<String>,
    inert: bool,
    selection_off: bool,
    quiet: Vec<bool>,
    built_rows: Vec<(usize, El)>,
}

enum Plan {
    List(ListPlan),
    Items(virtual_items::ItemsPlan),
}

impl Plan {
    #[inline]
    fn old_idx(&self) -> usize {
        match self {
            Plan::List(p) => p.old_list_idx,
            Plan::Items(p) => p.old_idx,
        }
    }
}

impl CommitState {
    /// Whether a commit that resized nothing and changed nothing the view reads
    /// ([`Runtime::commit_kind`]) only shifts or fills `virtual_rows` / `virtual_items` windows,
    /// so `view` and already mounted subtrees need not rebuild.
    pub(in crate::runtime) fn virtual_only(&self, tree: &types::TreeState, hot: bool, checking_live_only: bool) -> bool {
        if hot || checking_live_only || (self.virtuals.is_empty() && self.virtual_items.is_empty()) || tree.nodes.is_empty() {
            return false;
        }
        if !tree.aspect_snap.is_empty() {
            return false;
        }
        let check_aspect = tree.has_pixel_aspect;
        if !tree.nodes.iter().all(|n| {
            relayout::reusable(n)
                && n.text.as_ref().is_none_or(|t| !t.redacted)
                && (!check_aspect
                    || n.cold.get().and_then(|c| c.pixel_aspect).is_none_or(|r| {
                        n.vis.pixels.as_ref().is_some_and(|p| flatten::pixel_ratio(p) == r)
                    }))
        }) {
            return false;
        }
        let has_nested_items = |path: &str| {
            self.virtual_items.keys().any(|other| {
                other != path && other.strip_prefix(path).is_some_and(|r| r.starts_with('/'))
            })
        };
        let has_nested = |path: &str| {
            has_nested_items(path)
                || self.virtuals.keys().any(|other| {
                    other != path && other.strip_prefix(path).is_some_and(|r| r.starts_with('/'))
                })
        };
        for (path, v) in &self.virtuals {
            if !v.indexed || !v.selection.pins.source().and_then(|s| s.rebuild()).is_some_and(|s| s.count == v.keys.len()) {
                return false;
            }
            let Some(&list_id) = tree.path_ids.get(path) else { return false };
            let Some(&list_idx) = tree.node_index.get(&list_id) else { return false };
            if !tree.nodes.get(list_idx).is_some_and(|n| n.id == list_id && n.path == *path && n.virtual_list && n.growth.is_some()) {
                return false;
            }
            let Some(&scroll_id) = tree.path_ids.get(&v.scroll_path) else { return false };
            let Some(&scroll_idx) = tree.node_index.get(&scroll_id) else { return false };
            if !tree.nodes.get(scroll_idx).is_some_and(|n| {
                n.id == scroll_id
                    && n.path == v.scroll_path
                    && n.scroll.is_some()
                    && n.scroll_axes.y()
                    && (!n.scroll_axes.x() || tree.nodes[list_idx].parent != Some(scroll_idx))
                    && n.growth.is_none()
            }) {
                return false;
            }
            // Nested `virtual_rows` inside `virtual_rows` remount recursively; `virtual_items`
            // nested inside `virtual_rows` still fall back to a full rebuild.
            if has_nested_items(path) {
                return false;
            }
        }
        for path in self.virtual_items.keys() {
            let Some(&container_id) = tree.path_ids.get(path) else { return false };
            let Some(&container_idx) = tree.node_index.get(&container_id) else { return false };
            if !tree.nodes.get(container_idx).is_some_and(|n| {
                n.id == container_id
                    && n.path == *path
                    && n.virtual_items
                    && !n.virtual_list
                    && n.growth.is_none()
                    && !(n.scroll.is_some() && n.scroll_axes.x())
            }) {
                return false;
            }
            if has_nested(path) {
                return false;
            }
        }
        true
    }
}

fn move_slice(
    old_nodes: &mut [Node],
    range: Range<usize>,
    nodes: &mut Vec<Node>,
    old_to_new: &mut [u32],
    taffy: &mut LayoutEngine,
    reused: &mut Vec<memo_reuse::ReusedSpan>,
    root_virtual_mount: Option<bool>,
) {
    if range.is_empty() {
        return;
    }
    let start = range.start;
    let len = range.len();
    let new_start = nodes.len();
    for (k, old_idx) in range.enumerate() {
        let new_idx = new_start + k;
        old_to_new[old_idx] = new_idx as u32;
        let slot = &mut old_nodes[old_idx];
        taffy.reuse_slot(slot.taffy, new_idx as u32);
        let old_parent = slot.parent;
        let stub = Node::moved_stub(slot);
        let mut n = std::mem::replace(slot, stub);
        if !n.text.as_ref().is_some_and(|t| t.cold.live.is_some()) {
            n.text_layout = None;
            n.elided = None;
            n.clear_tabular_layout();
        }
        n.clear_run_layout();
        n.scale_track = false;
        n.group_fade = false;
        n.virtual_mount = if k == 0 { root_virtual_mount.unwrap_or(false) } else { false };
        n.parent = old_parent.map(|p| old_to_new[p] as usize);
        n.target = [0.0; 4];
        n.local = MRect::ZERO;
        n.inset = [0.0; 4];
        nodes.push(n);
    }
    reused.push(memo_reuse::ReusedSpan {
        new_start,
        old_start: start,
        len,
        same_path: true,
        moved: true,
    });
}

struct EmitCtx<'a> {
    old_nodes: &'a mut [Node],
    node_index: &'a NodeIndex,
    committing: bool,
    empty_committed: &'a HashMap<u64, memo_reuse::FlattenedMemo>,
    path_ids: &'a mut HashMap<String, NodeId>,
    next_id: &'a mut u64,
    foreground: Option<Color>,
    scale: f32,
    virtuals: &'a HashMap<String, crate::el::virtual_list::VirtualList>,
    on_spine: &'a [bool],
    plans: Vec<Option<Plan>>,
    taffy: LayoutEngine,
    nodes: Vec<Node>,
    pending: Vec<memo_reuse::PendingFlat>,
    reused: Vec<memo_reuse::ReusedSpan>,
    path_buf: String,
    old_to_new: Vec<u32>,
}

impl EmitCtx<'_> {
    fn take_plan(&mut self, old_idx: usize) -> Option<Plan> {
        self.plans.iter_mut().find(|p| p.as_ref().is_some_and(|p| p.old_idx() == old_idx))?.take()
    }

    fn has_plan_in(&self, range: Range<usize>) -> bool {
        self.plans.iter().any(|p| p.as_ref().is_some_and(|p| range.start <= p.old_idx() && p.old_idx() < range.end))
    }

    fn move_container(&mut self, old_idx: usize, virtual_mount: bool) -> (usize, NodeId, SlotId, Spring, RowLayout) {
        let new_idx = self.nodes.len();
        self.old_to_new[old_idx] = new_idx as u32;
        let slot = &mut self.old_nodes[old_idx];
        let old_parent = slot.parent;
        let stub = Node::moved_stub(slot);
        let mut n = std::mem::replace(slot, stub);
        if !n.text.as_ref().is_some_and(|t| t.cold.live.is_some()) {
            n.text_layout = None;
            n.elided = None;
            n.clear_tabular_layout();
        }
        n.clear_run_layout();
        n.scale_track = false;
        n.group_fade = false;
        n.virtual_mount = virtual_mount;
        n.parent = old_parent.map(|p| self.old_to_new[p] as usize);
        n.target = [0.0; 4];
        n.local = MRect::ZERO;
        n.inset = [0.0; 4];
        let info = (new_idx, n.id, n.taffy, n.motion, n.in_row);
        self.nodes.push(n);
        self.reused.push(memo_reuse::ReusedSpan {
            new_start: new_idx,
            old_start: old_idx,
            len: 1,
            same_path: true,
            moved: true,
        });
        info
    }

    fn flatten_child(
        &mut self,
        el: El,
        parent_idx: usize,
        pos: usize,
        motion: Spring,
        in_row: RowLayout,
        parent_row: bool,
    ) -> (SlotId, bool) {
        self.path_buf.clear();
        self.path_buf.push_str(&self.nodes[parent_idx].path);
        let mut memo_ctx = memo_reuse::MemoFlattenCtx {
            old_nodes: &mut *self.old_nodes,
            old_present_len: 0,
            old_node_index: self.node_index,
            old_cursor: 0,
            committing: self.committing,
            committed: self.empty_committed,
            pending: std::mem::take(&mut self.pending),
            reused: std::mem::take(&mut self.reused),
            taken_paths: Vec::new(),
            path_ids: &mut *self.path_ids,
            next_id: &mut *self.next_id,
            foreground: self.foreground,
            missed_virtual_memo: false,
        };
        let res = flatten_with_buf(el, Some(parent_idx), &mut self.path_buf, pos, motion, in_row, parent_row, &mut self.taffy, &mut self.nodes, self.scale, &mut memo_ctx);
        self.pending = memo_ctx.pending;
        self.reused = memo_ctx.reused;
        res
    }

    fn sync_container(
        &mut self,
        new_idx: usize,
        id: NodeId,
        expected_taffy: SlotId,
        style: CompactStyle,
        child_slots: Vec<SlotId>,
        children_dirty: bool,
    ) -> bool {
        let child_mark = self.taffy.begin_children();
        for s in child_slots {
            self.taffy.push_child(s);
        }
        let (slot, dirty) = self.taffy.sync_node(id.0, new_idx as u32, style, None, false, child_mark, children_dirty);
        debug_assert_eq!(slot, expected_taffy);
        if dirty {
            self.taffy.mark_dirty_and_ancestors(
                expected_taffy,
                std::iter::successors(self.nodes[new_idx].parent, |&p| self.nodes[p].parent).map(|p| self.nodes[p].taffy),
            );
        }
        dirty
    }

    /// Emits old nodes in `range`, splicing any `Plan`s at their old indices (including nested
    /// `virtual_rows` inside staying rows of an outer `virtual_rows`). Returns whether any plan
    /// inside `range` dirtied layout.
    fn emit_range(&mut self, range: Range<usize>, root_virtual_mount: Option<bool>) -> bool {
        let mut any_dirty = false;
        let mut old_idx = range.start;
        while old_idx < range.end {
            let root_vm = (old_idx == range.start).then_some(root_virtual_mount).flatten();
            if let Some(plan) = self.take_plan(old_idx) {
                match plan {
                    Plan::List(plan) => {
                        let (new_list_idx, list_id, list_taffy, list_motion, list_in_row) =
                            self.move_container(old_idx, root_vm.unwrap_or(false));
                        let mut list_style = self.taffy.style(list_taffy).clone();
                        list_style.set_pad_top(plan.pad_top);
                        list_style.set_pad_bottom(plan.pad_bottom);
                        let parent_row = matches!(list_style.flex_direction(), FlexDirection::Row | FlexDirection::RowReverse);

                        let mut row_slots = Vec::with_capacity(plan.range.len());
                        let mut any_row_dirty = false;
                        let mut built_iter = plan.built_rows.into_iter().peekable();
                        let keys = &self.virtuals[&plan.path].keys;
                        for (pos, row_i) in plan.range.clone().enumerate() {
                            if built_iter.peek().is_some_and(|(p, _)| *p == pos) {
                                let (_, row_el) = built_iter.next().unwrap();
                                let (kid_slot, kid_dirty) =
                                    self.flatten_child(row_el, new_list_idx, pos, list_motion, list_in_row, parent_row);
                                row_slots.push(kid_slot);
                                any_row_dirty |= kid_dirty;
                            } else {
                                let &(rs, re) = &plan.mounted_rows[&keys[row_i]];
                                let row_taffy = self.old_nodes[rs].taffy;
                                if self.has_plan_in(rs..re) {
                                    any_row_dirty |= self.emit_range(rs..re, Some(plan.quiet[pos]));
                                } else {
                                    move_slice(self.old_nodes, rs..re, &mut self.nodes, &mut self.old_to_new, &mut self.taffy, &mut self.reused, Some(plan.quiet[pos]));
                                }
                                row_slots.push(row_taffy);
                            }
                        }
                        any_dirty |= self.sync_container(new_list_idx, list_id, list_taffy, list_style, row_slots, any_row_dirty);
                        old_idx = plan.list_subtree_end;
                    }
                    Plan::Items(plan) => {
                        let (new_container_idx, container_id, container_taffy, container_motion, container_in_row) =
                            self.move_container(old_idx, root_vm.unwrap_or(false));
                        let container_style = self.taffy.style(container_taffy).clone();
                        let parent_row = matches!(container_style.flex_direction(), FlexDirection::Row | FlexDirection::RowReverse);

                        let mut item_slots = Vec::with_capacity(plan.next_mounted.len());
                        let mut any_item_dirty = false;
                        let mut built_iter = plan.built_items.into_iter().peekable();
                        for (pos, key) in plan.next_mounted.iter().enumerate() {
                            if built_iter.peek().is_some_and(|(p, _)| *p == pos) {
                                let (_, item_el) = built_iter.next().unwrap();
                                let (kid_slot, kid_dirty) =
                                    self.flatten_child(item_el, new_container_idx, pos, container_motion, container_in_row, parent_row);
                                item_slots.push(kid_slot);
                                any_item_dirty |= kid_dirty;
                            } else {
                                let &(rs, re) = &plan.mounted_items[key];
                                let item_taffy = self.old_nodes[rs].taffy;
                                move_slice(self.old_nodes, rs..re, &mut self.nodes, &mut self.old_to_new, &mut self.taffy, &mut self.reused, Some(false));
                                item_slots.push(item_taffy);
                            }
                        }
                        any_dirty |= self.sync_container(new_container_idx, container_id, container_taffy, container_style, item_slots, any_item_dirty);
                        old_idx = plan.subtree_end;
                    }
                }
            } else if self.on_spine[old_idx] {
                move_slice(self.old_nodes, old_idx..old_idx + 1, &mut self.nodes, &mut self.old_to_new, &mut self.taffy, &mut self.reused, root_vm);
                old_idx += 1;
            } else {
                let mut sub_end = old_idx + 1;
                while sub_end < range.end && self.old_nodes[sub_end].parent.is_some_and(|p| p >= old_idx) {
                    sub_end += 1;
                }
                move_slice(self.old_nodes, old_idx..sub_end, &mut self.nodes, &mut self.old_to_new, &mut self.taffy, &mut self.reused, root_vm);
                old_idx = sub_end;
            }
        }
        any_dirty
    }
}

fn build_indexed_row(
    rows: &crate::el::virtual_rows::RowSource,
    state: crate::states::StatesRef<'_>,
    path: &str,
    key: &str,
    i: usize,
    key_warnings: &mut Option<Vec<String>>,
) -> El {
    let tag = crate::memo::virtual_row_tag(path, key);
    let mut row = crate::memo::in_virtual_row(tag, || (rows.build)(state, i)).key(key.to_owned());
    if crate::input::key_diagnostics::clean(&row) {
        return row;
    }
    let duplicates = crate::input::key_diagnostics::repair_at(&mut row, &format!("{path}/{key}"));
    match key_warnings.as_mut() {
        Some(out) => out.extend(duplicates.into_iter().map(|d| d.message)),
        None => crate::input::key_diagnostics::warn_once(&duplicates),
    }
    row
}

impl Measurement<'_> {
    pub(in crate::runtime) fn remount_virtuals(mut self, text: &mut TextSystem) -> RemountOutcome {
        self.virtual_culled.clear();
        self.virtual_inputs.clear();

        let mut ordered_virtuals: Vec<(usize, String)> = self
            .virtuals
            .keys()
            .map(|path| (self.node_index[&self.path_ids[path]], path.clone()))
            .collect();
        // Plan outer lists before nested ones, matching `window_tree` pre-order.
        ordered_virtuals.sort_unstable_by_key(|(idx, _)| *idx);
        let mut plans: Vec<Plan> = Vec::with_capacity(ordered_virtuals.len() + self.virtual_items.keys().count());
        for (old_list_idx, path) in ordered_virtuals {
            let Some(v) = self.virtuals.get_mut(&path) else { continue };
            let scroll_id = self.path_ids[&v.scroll_path];
            let scroll_idx = self.node_index[&scroll_id];

            let mut list_subtree_end = old_list_idx + 1;
            while list_subtree_end < self.nodes.len()
                && self.nodes[list_subtree_end].parent.is_some_and(|p| p >= old_list_idx)
            {
                list_subtree_end += 1;
            }
            let mut mounted_rows = HashMap::default();
            let mut cur = old_list_idx + 1;
            while cur < list_subtree_end {
                debug_assert_eq!(self.nodes[cur].parent, Some(old_list_idx));
                let mut row_end = cur + 1;
                while row_end < list_subtree_end
                    && self.nodes[row_end].parent.is_some_and(|p| p >= cur)
                {
                    row_end += 1;
                }
                if let Some(k) = &self.nodes[cur].key {
                    mounted_rows.insert(k.clone(), (cur, row_end));
                }
                cur = row_end;
            }

            let sticky = self.nodes[scroll_idx].scroll.unwrap().stick_bottom;
            let old_scroll = self.scrolls.get(&scroll_id);
            let held = old_scroll.map_or(sticky, |s| s.pinned());
            let offset = old_scroll.map_or(0.0, |s| s.offset as f32);
            v.viewport = self.nodes[scroll_idx].local.h;
            let (mut offset, pinned) = v.locate(offset, held);
            v.anchor = if pinned || v.passed() || v.no_anchor { None } else { v.first_visible(offset, |_| true) };
            let owned = self.scroll_requests.owns(&v.scroll_path);
            let delta = if held || owned { 0.0 } else { v.anchor_delta() };
            offset = (offset + delta).max(0.0);
            if let Some(st) = self.scrolls.get_mut(&scroll_id) && !held && !owned {
                st.shift_anchor(delta);
            }
            let shown = std::mem::take(&mut v.shown);
            v.select(offset, pinned, Some(&shown));
            for key in shown.iter().filter(|k| v.index_of(k).is_some_and(|i| !v.range.contains(&i))) {
                self.virtual_culled.insert(node_path(&path, Some(key), 0));
            }
            v.shown = shown;

            let list_group = v.selection.scope.group_path.clone();
            let inert = v.selection.scope.inert;
            let selection_off = v.selection.scope.selection_off;
            // Cleared in place: a later plan that falls back keeps this list's buffers too.
            v.selection.pins.clear_texts();
            let rows = v.selection.pins.source().and_then(|s| s.rebuild())
                .expect("an index-built list keeps its row builder").clone();

            let under = |f: &str| f.strip_prefix(path.as_str()).is_some_and(|rest| rest.starts_with('/'));
            let focused = v
                .focused_row
                .as_deref()
                .filter(|_| self.focused.is_some_and(under))
                .and_then(|key| v.index_of(key))
                .filter(|i| !v.range.contains(i));
            let pinned_rows: Vec<usize> = v
                .selection
                .pins
                .rows()
                .iter()
                .filter_map(|key| v.index_of(key))
                .filter(|i| !v.range.contains(i))
                .collect();

            plans.push(Plan::List(ListPlan {
                path: path.clone(),
                old_list_idx,
                list_subtree_end,
                range: v.range.clone(),
                pad_top: v.pad_top + v.before,
                pad_bottom: v.pad_bottom + v.after,
                mounted_rows,
                focused,
                pinned: pinned_rows,
                rows,
                list_group,
                inert,
                selection_off,
                quiet: Vec::new(),
                built_rows: Vec::new(),
            }));
        }
        if !self.virtual_items.is_empty() {
            let item_plans = self.virtual_items.plan_remounts(
                self.nodes,
                self.path_ids,
                self.node_index,
                self.focused,
                self.key_warnings,
                self.virtual_culled,
            );
            plans.extend(item_plans.into_iter().map(Plan::Items));
        }
        plans.sort_unstable_by_key(Plan::old_idx);

        // If an outer list unmounts a row containing a nested virtual list, fall back to a full
        // rebuild so pruning and exits tear down the nested list cleanly.
        for (i, plan) in plans.iter().enumerate() {
            let Plan::List(outer) = plan else { continue };
            let outer_keys = &self.virtuals[&outer.path].keys;
            for child in &plans[i + 1..] {
                let child_idx = child.old_idx();
                if child_idx >= outer.list_subtree_end {
                    break;
                }
                let in_staying_row = outer.range.clone().any(|row_i| {
                    outer
                        .mounted_rows
                        .get(&outer_keys[row_i])
                        .is_some_and(|&(rs, re)| rs <= child_idx && child_idx < re)
                });
                if !in_staying_row {
                    self.virtual_items.clear_staged();
                    return RemountOutcome::Fallback;
                }
            }
        }

        let mut kept_rows = HashSet::default();
        for plan in &plans {
            match plan {
                Plan::List(plan) => {
                    let keys = &self.virtuals[&plan.path].keys;
                    for row_i in plan.range.clone() {
                        let key = &keys[row_i];
                        if plan.mounted_rows.contains_key(key) {
                            kept_rows.insert(crate::memo::virtual_row_tag(&plan.path, key));
                        }
                    }
                }
                Plan::Items(plan) => {
                    for key in &plan.next_mounted {
                        if plan.mounted_items.contains_key(key) {
                            kept_rows.insert(crate::memo::virtual_row_tag(&plan.path, key));
                        }
                    }
                }
            }
        }

        let memo = crate::memo::enter_partial(
            std::mem::take(self.memo),
            &kept_rows,
            self.env,
            self.system_accent,
            self.system_reduced_motion,
            self.system_reduced_transparency,
            self.languages_gen,
            crate::theme::blends_srgb(),
            self.memo_chrome_bits(),
            self.caret.clone(),
            self.layout_seen.clone(),
        );
        let view_time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::View);
        let state = self.app.states_ref();
        let mut nested_virtual = false;
        for plan in &mut plans {
            match plan {
                Plan::List(plan) => {
                    let v = self.virtuals.get_mut(&plan.path).unwrap();
                    if let Some(i) = plan.focused {
                        let row = build_indexed_row(&plan.rows, state, &plan.path, &v.keys[i], i, self.key_warnings);
                        measure::collect_virtual_inputs(&row, &plan.path, i, plan.inert, self.virtual_inputs);
                        if plan.pinned.contains(&i) {
                            selectable::virtual_selection::collect_row_into_pins(
                                &mut v.selection.pins,
                                &row,
                                &plan.path,
                                &v.keys[i],
                                i,
                                plan.list_group.as_deref(),
                                plan.inert,
                                plan.selection_off,
                            );
                        }
                    }
                    for &i in &plan.pinned {
                        if Some(i) == plan.focused {
                            continue;
                        }
                        let row = build_indexed_row(&plan.rows, state, &plan.path, &v.keys[i], i, self.key_warnings);
                        selectable::virtual_selection::collect_row_into_pins(
                            &mut v.selection.pins,
                            &row,
                            &plan.path,
                            &v.keys[i],
                            i,
                            plan.list_group.as_deref(),
                            plan.inert,
                            plan.selection_off,
                        );
                    }
                    for (pos, row_i) in plan.range.clone().enumerate() {
                        let key = &v.keys[row_i];
                        if !plan.mounted_rows.contains_key(key) {
                            #[cfg(debug_assertions)]
                            v.check_append_row_key(&plan.path, row_i, self.commits, |i| (plan.rows.key)(state, i));
                            let row = build_indexed_row(&plan.rows, state, &plan.path, key, row_i, self.key_warnings);
                            if row.flags.has(crate::el::ElFlags::HAS_VIRTUAL) {
                                nested_virtual = true;
                                break;
                            }
                            plan.built_rows.push((pos, row));
                        }
                    }
                }
                Plan::Items(plan) => {
                    for (pos, key) in plan.next_mounted.iter().enumerate() {
                        if !plan.mounted_items.contains_key(key) {
                            let item = virtual_items::build_item_el(&*plan.source, &plan.path, key, self.key_warnings);
                            if item.flags.has(crate::el::ElFlags::HAS_VIRTUAL) {
                                nested_virtual = true;
                                break;
                            }
                            plan.built_items.push((pos, item));
                        }
                    }
                }
            }
            if nested_virtual {
                break;
            }
        }
        let (memo, replayed) = memo.finish();
        drop(view_time);
        *self.memo = memo;
        if nested_virtual {
            self.virtual_items.clear_staged();
            return RemountOutcome::Fallback;
        }
        self.caret_read |= replayed.caret;
        for read in replayed.layout {
            layout_read::note_read_at(&mut self.layout_reads, &read.key, read.same, read.at);
        }

        for plan in &mut plans {
            if let Plan::List(plan) = plan {
                let v = self.virtuals.get_mut(&plan.path).unwrap();
                plan.quiet = plan.range.clone().map(|i| v.mounts_quietly(i, self.commits)).collect();
                for (pos, row) in &mut plan.built_rows {
                    v.set_row_no_anchor(plan.range.start + *pos, row.cold.scroll_edges.no_anchor);
                    row.flags.set(crate::el::ElFlags::VIRTUAL_MOUNT, plan.quiet[*pos]);
                }
                v.reanchor_if_excluded();
            }
        }

        let mut on_spine = vec![false; self.nodes.len()];
        for plan in &plans {
            let idx = plan.old_idx();
            on_spine[idx] = true;
            let mut cur = self.nodes[idx].parent;
            while let Some(p) = cur {
                if on_spine[p] {
                    break;
                }
                on_spine[p] = true;
                cur = self.nodes[p].parent;
            }
        }

        let scale = self.env.scale;
        let mut taffy = std::mem::replace(self.layout, LayoutEngine::with_capacity(0));
        taffy.begin_commit();
        let mut nodes = std::mem::take(self.spare_nodes);
        nodes.clear();
        nodes.reserve(self.nodes.len().max(32));
        let mut pending = std::mem::take(&mut self.memo.pending_scratch);
        pending.clear();
        let mut reused = std::mem::take(&mut self.memo.reused_scratch);
        reused.clear();
        let mut path_buf = std::mem::take(&mut self.memo.path_scratch);
        path_buf.clear();
        let empty_committed = HashMap::default();

        let old_len = self.nodes.len();
        let mut emit = EmitCtx {
            old_nodes: self.nodes,
            node_index: self.node_index,
            committing: self.committing,
            empty_committed: &empty_committed,
            path_ids: self.path_ids,
            next_id: self.next_id,
            foreground: self.defaults.foreground,
            scale,
            virtuals: self.virtuals,
            on_spine: &on_spine,
            plans: plans.into_iter().map(Some).collect(),
            taffy,
            nodes,
            pending,
            reused,
            path_buf,
            old_to_new: vec![u32::MAX; old_len],
        };
        emit.emit_range(0..old_len, None);
        let EmitCtx { mut taffy, nodes, mut pending, mut reused, mut path_buf, .. } = emit;

        path_buf.clear();
        self.memo.path_scratch = path_buf;
        taffy.finish_flatten();
        let root_id = nodes[0].taffy;
        let (layout, mut nodes, above_fit_styles) = self.solve(text, taffy, root_id, nodes, &reused);
        if self.measure_virtuals(&nodes, false) {
            memo_reuse::restore_moved_nodes(self.nodes, &mut nodes, &reused, &[]);
            nodes.clear();
            *self.spare_nodes = nodes;
            pending.clear();
            reused.clear();
            self.memo.pending_scratch = pending;
            self.memo.reused_scratch = reused;
            self.virtual_items.clear_staged();
            *self.layout = layout;
            return RemountOutcome::Retry;
        }
        RemountOutcome::Measured(Measured {
            layout,
            nodes,
            reduced_motion: self.reduced_motion,
            time_scale: self.time_scale,
            zoom: self.zoom,
            defaults: self.defaults.clone(),
            titlebar: self.titlebar,
            caret_read: self.caret_read,
            wm_state_read: None,
            primary_pointer_read: None,
            layout_reads: self.layout_reads,
            memo_flattened: Some(pending),
            memo_reused: reused,
            taken_paths: Vec::new(),
            selection: Vec::new(),
            above_fit_styles,
        })
    }
}

#[cfg(test)]
impl<S: 'static> Runtime<S> {
    /// Runs one remount pass on its own and says whether it fell back, so a test can read what
    /// the fallback left in the lists before the full rebuild that follows it fills them again.
    pub(in crate::runtime) fn remount_falls_back(&mut self) -> bool {
        let (core, app) = self.split_mut();
        let (m, text) = core.measurement(&app, true);
        matches!(m.remount_virtuals(text), RemountOutcome::Fallback)
    }
}
