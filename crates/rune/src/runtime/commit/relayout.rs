//! Laying a flattened tree out: Taffy, then each node's place and text.
//!
//! A commit that only resized the window lays the last commit's nodes out again at the new
//! size, without running the view or flattening what it returns: with the state, scale and
//! everything else a view can read unchanged, the view would build the same tree. A view that
//! reads layout or the caret still runs when what it read moved. A virtual list keeps its
//! mounted rows, which lay out again like the rest; the view runs only when the list's window
//! has to move to rows it hasn't built.

use super::*;

/// What tells a resize whether the last commit's tree still holds.
#[derive(Default)]
pub(in crate::runtime) struct Reuse {
    /// The scale the last commit built its view at.
    pub(in crate::runtime) scale: f32,
    /// A layout read moved this frame, so the view runs to see it.
    pub(in crate::runtime) read_moved: bool,
    /// The blend mode the last commit built its view in; theme colours resolve per mode.
    pub(in crate::runtime) srgb: Option<bool>,
    /// Authored unclamped styles of `anchored_fit` overlays clamped on the last commit, keyed by
    /// node index; empty when no overlay was clamped.
    pub(in crate::runtime) above_fit_styles: Vec<(usize, CompactStyle)>,
}

impl Reuse {
    /// Records what the commit just applied was built with.
    pub(in crate::runtime) fn built(&mut self, scale: f32, srgb: bool, above_fit_styles: Vec<(usize, CompactStyle)>) {
        self.scale = scale;
        self.srgb = Some(srgb);
        self.read_moved = false;
        self.above_fit_styles = above_fit_styles;
    }
}

/// Applies fixed viewport dimensions or content-sizing `auto()` + `(min, max)` bounds to the
/// root node's Taffy style. Fitted axes preserve explicit pixel lengths on the root while
/// turning `Auto` / `Percent` (e.g. `.w_full()`) into `auto()`, and apply `max_size` so
/// wrapping text wraps at `max_w` instead of laying out on one unbounded line.
pub(in crate::runtime) fn apply_root_sizing(
    style: &mut CompactStyle,
    viewport: (f32, f32),
    sizing: crate::ContentSizing,
    limits: crate::overlay::sizing::SizeLimits,
) {
    use taffy::util::MaybeResolve;
    let fit_dim = |d: Dimension| {
        if d.is_auto() || d.maybe_resolve(None, |_, _| 0.0).is_none() { auto() } else { d }
    };
    let width = if sizing.fits_width() { fit_dim(style.size().width) } else { length(viewport.0) };
    let height = if sizing.fits_height() { fit_dim(style.size().height) } else { length(viewport.1) };
    style.set_size(Size { width, height });
    if let Some((min_w, min_h)) = limits.0 {
        if sizing.fits_width() && min_w.is_finite() && min_w > 0.0 {
            let v = style.min_size().width.maybe_resolve(None, |_, _| 0.0).map_or(min_w, |c| c.max(min_w));
            style.set_min_width(LengthPercentageAuto::length(v));
        }
        if sizing.fits_height() && min_h.is_finite() && min_h > 0.0 {
            let v = style.min_size().height.maybe_resolve(None, |_, _| 0.0).map_or(min_h, |c| c.max(min_h));
            style.set_min_height(LengthPercentageAuto::length(v));
        }
    }
    if let Some((max_w, max_h)) = limits.1 {
        if sizing.fits_width() && max_w.is_finite() && max_w > 0.0 {
            let v = style.max_size().width.maybe_resolve(None, |_, _| 0.0).map_or(max_w, |c| c.min(max_w));
            style.set_max_width(LengthPercentageAuto::length(v));
        }
        if sizing.fits_height() && max_h.is_finite() && max_h > 0.0 {
            let v = style.max_size().height.maybe_resolve(None, |_, _| 0.0).map_or(max_h, |c| c.min(max_h));
            style.set_max_height(LengthPercentageAuto::length(v));
        }
    }
}

/// Whether a commit can lay `n` out again as it is. Layout rewrites the styles of
/// `aspect_fit` elements and fitted anchored overlays, and pictures can change their aspect.
/// Live bindings only own scene properties: moving the node retains their sources, sampled
/// values and exact fill, while `apply` leaves their scene tracks alone.
pub(in crate::runtime) fn reusable(n: &Node) -> bool {
    n.cold.get().is_none_or(|c| {
        c.aspect_fit.is_none() && c.above.as_ref().is_none_or(|a| !a.fit) && c.pixel_aspect.is_none()
    })
}

/// How a commit measures, decided once before it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum CommitKind {
    /// Run the view and flatten what it returns.
    Rebuild,
    /// Only the window resized: lay the last commit's nodes out again at the new size.
    Relayout,
    /// Only virtual windows moved: keep mounted subtrees and build the rows that enter.
    Remount,
}

impl RuntimeCore {
    /// Which way this commit measures. Both shortcuts need everything the view reads to be as
    /// the last commit built it, checked here once for both.
    pub(in crate::runtime) fn commit_kind(&self, tx: &Tx) -> CommitKind {
        let unchanged = self.commit.state_unchanged && !tx.commit.lands() && !self.commit.reuse.read_moved
            && self.commit.reuse.scale == self.env.scale && self.commit.reuse.srgb == Some(self.paint.srgb_blend())
            && !self.tree.has_playground && !self.patch_pending()
            // Exiting nodes left the layout tree.
            && self.commit.layout.live_count() == self.tree.nodes.len();
        if !unchanged { return CommitKind::Rebuild; }
        if self.commit.relayout {
            // `aspect_fit` elements and clamped `anchored_fit` overlays restore their authored
            // styles before `solve`; only a picture whose decoded aspect changed needs a rebuild.
            return if pixel_aspects_unchanged(&self.tree) { CommitKind::Relayout } else { CommitKind::Rebuild };
        }
        if self.commit.virtual_only(&self.tree, self.frame.has_hot(), self.checking_live_only()) { CommitKind::Remount } else { CommitKind::Rebuild }
    }

    /// Measures a commit: lays the last tree out again when only the viewport changed since it
    /// was built, remounts virtual rows alone when only their window moved, else runs the view.
    pub(in crate::runtime) fn measure_commit(&mut self, app: &dyn ViewDriver, tx: &Tx) -> Measured {
        let kind = self.commit_kind(tx);
        if kind == CommitKind::Relayout {
            // The view keeps what it last read; `settle_layout_reads` and the caret check run it
            // when the new layout moves them.
            let reads = self.commit.layout_seen.reads.clone();
            let caret_read = self.edit.caret_watch;
            let selection = self.selection_by_path();
            let prev_above_fit = self.commit.reuse.above_fit_styles.clone();
            let (mut m, text) = self.measurement(app, true);
            m.layout_reads = reads;
            m.caret_read = caret_read;
            return match m.relayout(text, &prev_above_fit) {
                Some(measured) => Measured { selection, ..measured },
                None => {
                    self.refresh_layout_seen();
                    self.measure(app, tx)
                }
            };
        }
        if kind == CommitKind::Remount {
            let _blend = crate::theme::scope(self.paint.srgb_blend());
            let _bars = crate::platform::scroll_bars::scope(self.host.scroll_bars.state, self.host.scroll_bars.runtime_id);
            let reads = self.commit.layout_seen.reads.clone();
            let caret_read = self.edit.caret_watch;
            let selection = self.selection_by_path();
            let (mut m, text) = self.measurement(app, true);
            m.layout_reads = reads;
            m.caret_read = caret_read;
            return match m.remount_virtuals(text) {
                commit::remount::RemountOutcome::Measured(measured) => Measured { selection, ..measured },
                commit::remount::RemountOutcome::Retry => {
                    self.refresh_layout_seen();
                    self.measure_retry(app, tx)
                }
                commit::remount::RemountOutcome::Fallback => {
                    self.refresh_layout_seen();
                    self.measure(app, tx)
                }
            };
        }
        // A full view rebuild resets any stalled virtual_items fill so fresh app state can fill.
        self.commit.virtual_items.reset_fill();
        // A first commit has no layout to refresh from: its view is given the last launch's
        // rects when the app kept them (`layout_seed`).
        if !self.seed_layout_reads() { self.refresh_layout_seen(); }
        self.measure(app, tx)
    }
}

impl Measurement<'_> {
    /// Lays the last commit's nodes out again at the viewport, in its layout tree. None when a
    /// virtual list's window has to move, which only the view can build.
    fn relayout(mut self, text: &mut TextSystem, prev_above_fit: &[(usize, CompactStyle)]) -> Option<Measured> {
        self.anchor_virtuals();
        let len = self.nodes.len();
        let mut taffy = std::mem::replace(self.layout, LayoutEngine::with_capacity(0));
        let mut nodes = std::mem::take(self.spare_nodes);
        nodes.clear();
        nodes.reserve(len);
        for slot in self.nodes.iter_mut() {
            if slot.input.is_some() || slot.is_secure() || slot.text.as_ref().is_some_and(|t| t.by_paragraph(false)) {
                // The new nodes lay their text out again; the cache then holds the only reference.
                // A by-paragraph field's goes with it, to re-break its paragraphs from.
                let kept = slot.text_layout.take().filter(|l| matches!(l, TextLayout::Paragraphs(_)));
                let mut copy = Node { elided: None, virtual_mount: false, text_layout: kept, epoch: crate::runtime::paint_keep::next_epoch(), ..slot.clone() };
                copy.clear_run_layout();
                copy.clear_tabular_layout();
                nodes.push(copy);
            } else {
                let stub = Node::moved_stub(slot);
                let mut n = std::mem::replace(slot, stub);
                if !n.text.as_ref().is_some_and(|t| t.cold.live.is_some()) {
                    n.text_layout = None;
                    n.elided = None;
                    n.clear_tabular_layout();
                }
                n.clear_run_layout();
                n.virtual_mount = false;
                nodes.push(n);
            }
        }
        self.mark_quiet_rows(&mut nodes);
        let mut reused = std::mem::take(&mut self.memo.reused_scratch);
        let mut split = std::mem::take(&mut self.memo.split_scratch);
        build_relayout_spans(self.nodes, &nodes, self.virtuals, self.path_ids, self.node_index, &mut split, &mut reused);
        self.memo.split_scratch = split;
        let root = nodes[0].taffy;
        let mut style = taffy.style(root).clone();
        apply_root_sizing(&mut style, self.viewport, self.content_sizing, self.content_limits);
        taffy.set_style_and_mark_ancestors(root, style, std::iter::empty());
        // Restore authored unclamped styles on fitted anchored overlays and room styles on
        // `aspect_fit` elements so `solve` re-fits both against the new viewport.
        for (i, orig) in prev_above_fit {
            if let Some(n) = nodes.get(*i) {
                taffy.set_style_and_mark_ancestors(
                    n.taffy,
                    orig.clone(),
                    std::iter::successors(n.parent, |&p| nodes[p].parent).map(|p| nodes[p].taffy),
                );
            }
        }
        for i in 0..nodes.len() {
            if let Some(fit) = nodes[i].cold.get().and_then(|c| c.aspect_fit.as_deref()) {
                let parent_row = nodes[i].parent.is_some_and(|p| {
                    matches!(taffy.style(nodes[p].taffy).flex_direction(), FlexDirection::Row | FlexDirection::RowReverse)
                });
                taffy.set_style_and_mark_ancestors(
                    nodes[i].taffy,
                    fit_room(&fit.1, parent_row),
                    std::iter::successors(nodes[i].parent, |&p| nodes[p].parent).map(|p| nodes[p].taffy),
                );
            }
        }
        // As flattening does: secure leaves don't keep a measurement.
        for n in nodes.iter().filter(|n| n.text.as_ref().is_some_and(|t| t.redacted)) {
            taffy.mark_dirty_and_ancestors(
                n.taffy,
                std::iter::successors(n.parent, |&p| nodes[p].parent).map(|p| nodes[p].taffy),
            );
        }
        let (layout, mut nodes, above_fit_styles) = self.solve(text, taffy, root, nodes, &reused);
        if self.measure_virtuals(&nodes, false) || self.windows_move() {
            memo_reuse::restore_moved_nodes(self.nodes, &mut nodes, &reused, &[]);
            reused.clear();
            self.memo.reused_scratch = reused;
            nodes.clear();
            *self.spare_nodes = nodes;
            *self.layout = layout;
            return None;
        }
        // The mounted rows are the ones shown, so none was culled.
        self.virtual_culled.clear();
        Some(Measured {
            layout,
            nodes,
            reduced_motion: self.reduced_motion,
            time_scale: self.time_scale,
            zoom: self.zoom,
            defaults: self.defaults.clone(),
            titlebar: self.titlebar,
            caret_read: self.caret_read,
            wm_state_read: None,
            layout_reads: self.layout_reads,
            memo_flattened: None,
            memo_reused: reused,
            taken_paths: Vec::new(),
            selection: Vec::new(),
            above_fit_styles,
        })
    }

    /// Anchors each virtual list's reader to the first row in view, as a commit's window pass
    /// does, so rows above it that rewrap leave it in place.
    fn anchor_virtuals(&mut self) {
        for v in self.virtuals.values_mut() {
            let scroll = self.path_ids.get(&v.scroll_path).and_then(|id| self.scrolls.get(id));
            let sticky = || self.nodes.iter().find(|n| n.path == v.scroll_path).and_then(|n| n.scroll).is_some_and(|s| s.stick_bottom);
            let held = scroll.map_or_else(sticky, |s| s.pinned());
            let (offset, pinned) = v.locate(scroll.map_or(0.0, |s| s.offset as f32), held);
            v.anchor = if pinned || v.passed() || v.no_anchor { None } else { v.first_visible(offset.min(v.max_offset()), |_| true) };
        }
    }

    /// Marks which nodes mount quietly, as `populate_virtual_children` marks a virtual list's rows.
    fn mark_quiet_rows(&mut self, nodes: &mut [Node]) {
        if self.virtuals.is_empty() {
            return;
        }
        for (path, v) in self.virtuals.iter_mut() {
            let Some(&list) = self.path_ids.get(path).and_then(|id| self.node_index.get(id)) else { continue };
            for n in nodes.iter_mut().skip(list + 1).filter(|n| n.parent == Some(list)) {
                n.virtual_mount = n.key.as_deref().and_then(|k| v.index_of(k)).is_some_and(|row| v.mounts_quietly(row, self.commits));
            }
        }
    }

    /// Whether a virtual list's viewport came near the edge of its mounted rows, or its rows grew
    /// a viewport past the overscan; the view then mounts the window again.
    fn windows_move(&self) -> bool {
        self.virtuals.values().any(|v| {
            self.path_ids.get(&v.scroll_path).and_then(|id| self.scrolls.get(id)).is_some_and(|s| {
                let (offset, pinned) = v.at(s.offset as f32, s.pinned());
                v.needs_window(offset, pinned) || v.spare(offset, pinned) > v.viewport
            })
        }) || self.virtual_items.iter().any(|(path, c)| c.wants_window(path, self.focused))
    }

    /// Lays `nodes` out from `root_id` at the viewport, places them, and lays out their text.
    pub(in crate::runtime) fn solve(&mut self, text: &mut TextSystem, mut taffy: LayoutEngine, root_id: SlotId, mut nodes: Vec<Node>, reused: &[memo_reuse::ReusedSpan]) -> (LayoutEngine, Vec<Node>, Vec<(usize, CompactStyle)>) {
        let scale = self.env.scale;
        let room = Size {
            width: if self.content_sizing.fits_width() { AvailableSpace::MaxContent } else { AvailableSpace::Definite(self.viewport.0) },
            height: if self.content_sizing.fits_height() { AvailableSpace::MaxContent } else { AvailableSpace::Definite(self.viewport.1) },
        };
        {
            let _layout_time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::Layout);
            compute_layout(&mut taffy, root_id, room, &nodes, text, scale);
            if self.content_sizing.fits_width()
                && let Some((max_w, _)) = self.content_limits.1
                && max_w.is_finite()
                && max_w > 0.0
                && taffy.box_layout(root_id).size().width >= max_w - 0.5
            {
                let mut style = taffy.style(root_id).clone();
                style.set_width(length(max_w));
                taffy.set_style_and_mark_ancestors(root_id, style, std::iter::empty());
                let wrapped_room = Size { width: AvailableSpace::Definite(max_w), ..room };
                compute_layout(&mut taffy, root_id, wrapped_room, &nodes, text, scale);
            }
            // `aspect_fit` elements laid out filling their room; fix each at the largest box
            // at its ratio inside it and lay out again, so siblings and alignment follow.
            let mut refit = false;
            for n in &nodes {
                if let Some(fit) = &n.aspect_fit {
                    let style = fitted(fit.0, &fit.1, taffy.box_layout(n.taffy).size());
                    taffy.set_style_and_mark_ancestors(
                        n.taffy,
                        style,
                        std::iter::successors(n.parent, |&p| nodes[p].parent).map(|p| nodes[p].taffy),
                    );
                    refit = true;
                }
            }
            if refit { compute_layout(&mut taffy, root_id, room, &nodes, text, scale); }
        }
        self.stats.layout_nodes = nodes.len();
        #[cfg(debug_assertions)]
        layout_diagnostics::collapsed_scrolls(&taffy, &nodes);

        self.place(&taffy, &mut nodes);
        let mut above_fit_styles = Vec::new();
        if self.fit_anchored(&mut taffy, &nodes, &mut above_fit_styles) {
            compute_layout(&mut taffy, root_id, room, &nodes, text, scale);
            self.place(&taffy, &mut nodes);
        }
        let mut span_idx = 0usize;
        for i in 0..nodes.len() {
            let Some(spec) = nodes[i].text.as_deref() else { continue };
            let w = (taffy.box_layout(nodes[i].taffy).w - nodes[i].inset[0] - nodes[i].inset[2]).max(0.0);
            if spec.cold.live.is_some() {
                while span_idx < reused.len() && reused[span_idx].new_start + reused[span_idx].len <= i {
                    span_idx += 1;
                }
                let reused_old = reused
                    .get(span_idx)
                    .filter(|s| i >= s.new_start && i < s.new_start + s.len)
                    .and_then(|s| self.nodes.get(s.old_start + (i - s.new_start)));
                let width_sensitive = spec.wrap || spec.truncate || spec.max_lines > 0;
                let kept_valid = nodes[i].text_layout.is_some()
                    && reused_old.is_some_and(|old| {
                        !width_sensitive || (old.local.w - old.inset[0] - old.inset[2]).max(0.0) == w
                    });
                if kept_valid {
                    if nodes[i].text.as_ref().is_some_and(|t| !t.cold.inline_boxes.is_empty()) {
                        inline::place_inline_children(&mut taffy, text, &mut nodes, i, scale);
                    }
                    continue;
                }
                if nodes[i].cold.get().is_some_and(|c| c.live_text_seen.is_some()) {
                    lay_out_text_inner(text, &mut nodes[i], w, scale, true);
                    nodes[i].epoch = crate::runtime::paint_keep::next_epoch();
                    if nodes[i].text.as_ref().is_some_and(|t| !t.cold.inline_boxes.is_empty()) {
                        inline::place_inline_children(&mut taffy, text, &mut nodes, i, scale);
                    }
                    continue;
                }
                if reused_old.is_some() {
                    nodes[i].epoch = crate::runtime::paint_keep::next_epoch();
                }
            }
            lay_out_text(text, &mut nodes[i], w, scale);
            if nodes[i].text.as_ref().is_some_and(|t| !t.cold.inline_boxes.is_empty()) {
                inline::place_inline_children(&mut taffy, text, &mut nodes, i, scale);
            }
        }

        (taffy, nodes, above_fit_styles)
    }
}

/// Whether every picture sizing a side from its pixels still has the ratio its Taffy style or
/// measure key was built with, so a viewport resize can relayout without flattening again.
fn pixel_aspects_unchanged(tree: &types::TreeState) -> bool {
    if !tree.aspect_snap.is_empty() {
        return false;
    }
    if !tree.has_pixel_aspect {
        return true;
    }
    tree.nodes.iter().all(|n| {
        n.cold.get().and_then(|c| c.pixel_aspect).is_none_or(|r| {
            n.vis.pixels.as_ref().is_some_and(|p| flatten::pixel_ratio(p) == r)
        })
    })
}

/// Partitions the moved nodes of a `relayout` pass into disjoint subtree [`ReusedSpan`]s so
/// `clear_old_nodes` skips dropping every `moved_stub` and `clean_apply_spans` skips unchanged
/// subtrees (such as fixed-width sidebars or blocks that did not rewrap).
fn build_relayout_spans(
    old_nodes: &[Node],
    nodes: &[Node],
    virtuals: &HashMap<String, crate::el::virtual_list::VirtualList>,
    path_ids: &HashMap<String, NodeId>,
    node_index: &NodeIndex,
    split: &mut Vec<bool>,
    out: &mut Vec<memo_reuse::ReusedSpan>,
) {
    let len = nodes.len();
    out.clear();
    split.clear();
    split.resize(len, false);
    let mark_up = |mut cur: Option<usize>, split: &mut [bool]| {
        while let Some(p) = cur {
            if split[p] {
                break;
            }
            split[p] = true;
            cur = nodes[p].parent;
        }
    };
    for path in virtuals.keys() {
        if let Some(&list) = path_ids.get(path).and_then(|id| node_index.get(id))
            && list < len
        {
            mark_up(Some(list), split);
        }
    }
    for i in 0..len {
        if !old_nodes[i].is_moved_stub() {
            mark_up(Some(i), split);
            continue;
        }
        let has_children = nodes.get(i + 1).is_some_and(|c| c.parent == Some(i));
        let shallow = i == 0 || nodes[i].parent.is_some_and(|p| p == 0 || nodes[p].parent == Some(0));
        if has_children && shallow {
            split[i] = true;
        }
    }
    let mut i = 0usize;
    while i < len {
        if !old_nodes[i].is_moved_stub() {
            i += 1;
            continue;
        }
        let span_len = if split[i] {
            1
        } else {
            let mut sub_end = i + 1;
            while sub_end < len && nodes[sub_end].parent.is_some_and(|p| p >= i) {
                sub_end += 1;
            }
            sub_end - i
        };
        out.push(memo_reuse::ReusedSpan {
            new_start: i,
            old_start: i,
            len: span_len,
            same_path: true,
            moved: true,
        });
        i += span_len;
    }
}

/// Lays `n`'s text out again at the width it was laid out at.
pub(in crate::runtime) fn lay_out_text_in_place(text: &mut TextSystem, n: &mut Node, scale: f32) {
    let w = (n.local.w - n.inset[0] - n.inset[2]).max(0.0);
    lay_out_text_inner(text, n, w, scale, false);
}

/// Lays a wrapped, truncated, or line-clamped `live_text` node out at its existing content
/// width without inserting per-tick layouts or elisions into the shared commit caches.
pub(in crate::runtime) fn lay_out_live_text_in_place(text: &mut TextSystem, n: &mut Node, scale: f32) {
    let w = (n.local.w - n.inset[0] - n.inset[2]).max(0.0);
    lay_out_text_inner(text, n, w, scale, true);
}

/// Lays `n`'s text out at its content width `w`.
pub(in crate::runtime) fn lay_out_text(text: &mut TextSystem, n: &mut Node, w: f32, scale: f32) {
    lay_out_text_inner(text, n, w, scale, false);
}

fn lay_out_text_inner(text: &mut TextSystem, n: &mut Node, w: f32, scale: f32, live: bool) {
    n.clear_tabular_layout();
    let Some(spec) = &n.text else { return };
    // A cell of a run draws from the run's layout (and keeps it cached) instead of its own.
    let run = spec.cold.advance_in.as_ref().map(|(run, at)| (text.run_layout(run, spec.style, scale), *at));
    if let Some((r, _)) = run.filter(|(r, at)| r.x(*at).is_some()) {
        n.text_layout = Some(TextLayout::Single(r.layout.clone()));
        n.cold_mut().run_layout = Some(r);
        return;
    }
    let wrap = spec.wrap.then_some(w * scale + super::text_layout::FIT_SLACK);
    // A by-paragraph field keeps what its last layout (still on it) shares with this one.
    let is_input = n.input.is_some();
    let mut layout = if live {
        text.layout_live_spec(&spec.as_text_input(scale, false), scale, wrap, line_alignment(spec))
    } else {
        match text.layout_text(&spec.as_text_input(scale, is_input), is_input, scale, wrap, line_alignment(spec), n.text_layout.as_ref()) {
            TextLayout::Single(l) => l,
            TextLayout::Paragraphs(mut p) => {
                text.prepare_highlights(&mut p, &spec.cold.highlights);
                n.text_layout = Some(TextLayout::Paragraphs(p));
                n.elided = None;
                return;
            }
        }
    };
    let mut elided = None;
    let trunc_w = layout.get(0).map_or(layout.full_width(), |l| l.metrics().indent + l.metrics().advance);
    if spec.truncate && trunc_w > w * scale + 0.5 {
        if let Some((cut, e)) = truncated(text, spec, scale, w * scale) {
            (layout, elided) = (cut, Some(e));
        }
    } else if let Some((cut, e)) = clamped(text, spec, scale, w * scale, &layout) {
        (layout, elided) = (cut, Some(e));
    }
    n.text_layout = Some(TextLayout::Single(layout));
    n.elided = elided;
}
