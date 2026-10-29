use super::*;

pub(super) fn collect_virtual_inputs(el: &El, parent: &str, index: usize, inert: bool, inputs: &mut HashMap<String, Box<InputSpec>>) {
    // Most offscreen rows hold no focused field; find that out before building any paths.
    if !crate::memo::has_focused_input(el) {
        return;
    }
    let inert = inert || el.inert;
    let input = el.input.as_ref().filter(|i| i.edit.focused && i.access.is_some());
    // A memo's shared children are skipped whole when none is a focused field.
    let skip = el.memo_children.as_ref().filter(|m| !m.focused_input).map_or(0, |m| m.children.len());
    if input.is_none() && el.children.is_empty() && skip == el.memo_children.as_ref().map_or(0, |m| m.children.len()) {
        return;
    }
    let segment = el.key.clone().unwrap_or_else(|| format!("#{index}"));
    let path = format!("{parent}/{segment}");
    if let Some(input) = input {
        let mut input = input.clone();
        if inert { input.edit.focused = false; }
        inputs.insert(path.clone(), input);
    }
    for (i, child) in el.all_children().enumerate().skip(skip) {
        collect_virtual_inputs(child, &path, i, inert, inputs);
    }
}

impl<S: 'static> Runtime<S> {
    pub(super) fn measure(&mut self, tx: &Tx) -> Measured {
        self.measure_pass(tx, true)
    }

    /// `committing` when the result is applied, so the last commit's text layouts can go.
    pub(super) fn measure_pass(&mut self, tx: &Tx, committing: bool) -> Measured {
        let _blend = crate::theme::scope(self.srgb_blend);
        Measurement {
            state: &self.state,
            view: self.view,
            playground: self.playground.as_deref(),
            env: self.env,
            viewport: self.viewport,
            fit_height: self.fit_height,
            system_reduced_motion: self.system_reduced_motion,
            titlebar_inset: self.titlebar_inset,
            system_accent: self.system_accent,
            reduced_motion: self.reduced_motion,
            time_scale: self.time_scale,
            caret: self.caret.clone(),
            caret_read: false,
            layout_seen: self.layout_seen.rects.clone(),
            layout_reads: Vec::new(),
            commits: self.commits,
            nodes: &mut self.nodes,
            committing,
            spare_nodes: std::mem::take(&mut self.spare_nodes),
            text: &mut self.text,
            layout: &mut self.layout,
            layout_entries: &mut self.layout_entries,
            node_index: &mut self.node_index,
            path_ids: &mut self.path_ids,
            memo: &mut self.memo,
            next_id: &mut self.next_id,
            scrolls: &mut self.scrolls,
            virtuals: &mut self.virtuals,
            virtual_culled: &mut self.virtual_culled,
            virtual_inputs: &mut self.virtual_inputs,
            stats: &mut self.stats,
            key_warnings: &mut self.key_warnings,
            focused: self.focused.as_deref(),
            state_unchanged: std::mem::take(&mut self.state_unchanged),
            retry: false,
        }.finish(tx)
    }

    /// Forgets the ids of paths that left with nothing kept under their id, so a session that
    /// mounts many distinct keys (a long virtual list scrolled end to end) doesn't keep them all.
    /// A path whose id still holds scroll, growth or reveal state keeps it, so it comes back
    /// with that state as before.
    pub(super) fn prune_path_ids(&mut self) {
        if self.path_ids.len() <= self.node_index.len() { return; }
        let Self { path_ids, node_index, scrolls, stacks, grows, reveals, .. } = self;
        path_ids.retain(|_, id| {
            node_index.contains_key(id) || scrolls.contains_key(id) || stacks.contains_key(id) || grows.contains_key(id) || reveals.contains_key(id)
        });
    }
}

impl<S: 'static> Measurement<'_, S> {
    pub(super) fn node_id(&mut self, path: &str) -> NodeId {
        if let Some(&id) = self.path_ids.get(path) { return id; }
        let id = NodeId(*self.next_id);
        *self.next_id += 1;
        self.path_ids.insert(path.to_owned(), id);
        id
    }

    /// Finishes all row measurements before scene/growth mutation. Newly measured estimates
    /// above the viewport adjust the scroll offset by exactly the same amount.
    pub(super) fn measure_virtuals(&mut self, nodes: &[Node], settle: bool) -> bool {
        let mut retry = false;
        for (path, v) in self.virtuals.iter_mut() {
            let Some((ci, _)) = nodes.iter().enumerate().find(|(_, n)| n.path == *path) else { continue };
            for n in nodes.iter().filter(|n| n.parent == Some(ci)) {
                v.set_height(n.key.as_deref().unwrap(), n.local.h);
            }
            let Some(scroll) = nodes.iter().find(|n| n.path == v.scroll_path) else { continue };
            v.viewport = scroll.local.h;
            v.top = (nodes[ci].target[1] - scroll.target[1]).max(0.0);
            let st = self.scrolls.get_mut(&scroll.id);
            let sticky = scroll.scroll.unwrap().stick_bottom;
            let pinned = (settle && sticky) || st.as_ref().map_or(sticky, |s| s.pinned);
            let offset = if let Some(st) = st {
                if !pinned { st.offset = (st.offset + f64::from(v.anchor_delta())).max(0.0); }
                st.offset as f32
            } else { 0.0 };
            // Rows that shrank or went leave the offset past the end until scrolling clamps it;
            // what shows then is the end.
            let offset = (offset - v.top).clamp(0.0, v.max_offset());
            let end = v.row_end(v.range.end);
            let mounted_height = (end - v.before).max(0.0);
            // Never present a blank caused by an estimate. Expand and measure again first.
            retry |= if pinned {
                v.range.start > 0 && mounted_height < v.viewport
            } else {
                (v.range.start > 0 && offset < v.pad_top + v.before)
                    || (v.range.end < v.keys.len() && offset + v.viewport > v.pad_top + end)
            };
        }
        retry
    }

    /// Checks the view's keys and builds the virtual windows, index-built rows included.
    pub(super) fn build_tree(&mut self, mut root: El, tx: &Tx, force_all: bool) -> El {
        // Two siblings with one key would share a layout node, which Taffy panics on. Rename the
        // later ones so every build, debug or release, lays out; --tree still lists them.
        let duplicates = crate::key_diagnostics::repair(&mut root);
        match self.key_warnings.as_mut() {
            Some(warnings) => *warnings = duplicates.into_iter().map(|d| d.message).collect(),
            None => crate::key_diagnostics::warn_once(&duplicates),
        }
        // Index-built rows build here, so memos inside them share the view's cache. A window that
        // fits its content lays its root out at its natural height, which nothing bounds.
        let bound = if self.fit_height { f32::INFINITY } else { self.viewport.1 };
        self.window_tree(&mut root, "", 0, None, tx.settle, force_all, bound, false, None);
        root
    }

    pub(super) fn populate_virtual_children(
        &mut self,
        el: &mut El,
        path: &str,
        source: Option<Rc<crate::virtual_rows::RowSource>>,
        inert: bool,
        group: Option<&str>,
    ) {
        let state = playground_host::handler_states_ref(self.state, self.playground);
        let v = self.virtuals.get_mut(path).unwrap();
        let list_group = if el.selection_group { Some(path.to_owned()) } else { group.map(str::to_owned) };
        v.selection.group_path = list_group.clone();
        v.selection.inert = inert;
        v.selection.texts.clear();
        v.selection.copy_providers.clear();
        v.selection.unmounted.clear();
        if let Some(rows) = source {
            let quiet: Vec<bool> = v.range.clone().map(|i| v.mounts_quietly(i, self.commits)).collect();
            let key_warnings = &mut *self.key_warnings;
            let keys = &v.keys;
            let mut build = |i: usize| {
                let key = &keys[i];
                let mut row = (rows.build)(state, i).key(key.clone());
                // The view's key repair ran before these rows existed.
                if crate::key_diagnostics::clean(&row) { return row; }
                let duplicates = crate::key_diagnostics::repair_at(&mut row, &format!("{path}/{key}"));
                match key_warnings.as_mut() {
                    Some(out) => out.extend(duplicates.into_iter().map(|d| d.message)),
                    None => crate::key_diagnostics::warn_once(&duplicates),
                }
                row
            };
            // A focused field's row and a selection's pinned end rows are built unmounted.
            let under = |f: &str| f.strip_prefix(path).is_some_and(|rest| rest.starts_with('/'));
            let focused = v.focused_row.as_deref().filter(|_| self.focused.is_some_and(under))
                .and_then(|key| v.index_of(key)).filter(|i| !v.range.contains(i));
            let pinned: Vec<usize> = v.pinned_rows.iter()
                .filter_map(|key| v.index_of(key))
                .filter(|i| !v.range.contains(i))
                .collect();
            if let Some(i) = focused {
                let row = build(i);
                collect_virtual_inputs(&row, path, i, inert, self.virtual_inputs);
                if pinned.contains(&i) {
                    selectable::virtual_selection::collect_virtual_row_selection(
                        &row, path, &v.keys[i], i, list_group.as_deref(), inert,
                        &mut v.selection.texts, &mut v.selection.copy_providers,
                    );
                }
            }
            for i in pinned {
                if Some(i) == focused { continue; }
                let row = build(i);
                selectable::virtual_selection::collect_virtual_row_selection(
                    &row, path, &v.keys[i], i, list_group.as_deref(), inert,
                    &mut v.selection.texts, &mut v.selection.copy_providers,
                );
            }
            el.children = v.range.clone().zip(quiet).map(|(i, quiet)| {
                let mut row = build(i);
                row.virtual_mount = quiet;
                row
            }).collect();
            v.selection.source = Some(rows);
        } else {
            v.selection.source = None;
            if !self.retry && el.focus.reveal_selected {
                let selected = el.children.iter().find(|c| c.focus.selected).and_then(|c| c.key.clone());
                if selected != v.selected_row {
                    v.reveal_selected = selected.clone();
                    v.selected_row = selected;
                }
            }
            #[cfg(debug_assertions)]
            crate::perf_warn::overbuilt(path, el.virtual_source, el.children.len(), v.range.len());
            let pinned: Vec<usize> = v.pinned_rows.iter()
                .filter_map(|key| v.index_of(key))
                .filter(|i| !v.range.contains(i))
                .collect();
            el.children = std::mem::take(&mut el.children).into_iter().enumerate().filter_map(|(i, mut child)| {
                if !v.range.contains(&i) {
                    collect_virtual_inputs(&child, path, i, inert, self.virtual_inputs);
                    if pinned.contains(&i) {
                        selectable::virtual_selection::collect_virtual_row_selection(
                            &child, path, &v.keys[i], i, list_group.as_deref(), inert,
                            &mut v.selection.texts, &mut v.selection.copy_providers,
                        );
                    }
                    v.selection.unmounted.insert(i, child);
                    return None;
                }
                child.virtual_mount = v.mounts_quietly(i, self.commits);
                Some(child)
            }).collect();
        }
    }
}

impl<S: 'static> Runtime<S> {
    /// Records which row of each virtual list holds focus, by key, so the row is still built
    /// after it scrolls out. Walks the node tree, since a key may itself contain `/`.
    pub(super) fn track_focused_rows(&mut self) {
        if self.virtuals.is_empty() { return; }
        let focused = self.focused.as_deref().and_then(|f| self.nodes.iter().position(|n| n.path == f));
        for (path, v) in self.virtuals.iter_mut() {
            let Some(rest) = self.focused.as_deref().and_then(|f| f.strip_prefix(path.as_str())).and_then(|r| r.strip_prefix('/')) else {
                v.focused_row = None;
                continue;
            };
            if let Some(mut i) = focused {
                while let Some(p) = self.nodes[i].parent {
                    if self.nodes[p].path == *path {
                        v.focused_row = self.nodes[i].key.clone();
                        break;
                    }
                    i = p;
                }
            } else {
                let matches = |k: &str| rest == k || rest.strip_prefix(k).is_some_and(|t| t.starts_with('/'));
                if !v.focused_row.as_deref().is_some_and(matches) {
                    v.focused_row = v.keys.iter().find(|k| matches(k)).cloned();
                }
            }
        }
    }
}
