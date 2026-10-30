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
pub(super) struct Reuse {
    /// The scale the last commit built its view at.
    pub(super) scale: f32,
    /// A layout read moved this frame, so the view runs to see it.
    pub(super) read_moved: bool,
    /// The blend mode the last commit built its view in; theme colours resolve per mode.
    pub(super) srgb: Option<bool>,
}

impl Reuse {
    /// Records what the commit just applied was built with.
    pub(super) fn built(&mut self, scale: f32, srgb: bool) {
        self.scale = scale;
        self.srgb = Some(srgb);
    }
}

/// The root's size in a viewport; a window that fits its content takes its natural height.
pub(super) fn root_size(viewport: (f32, f32), fit_height: bool) -> Size<Dimension> {
    Size { width: length(viewport.0), height: if fit_height { auto() } else { length(viewport.1) } }
}

/// Whether a commit can lay `n` out again as it is. Layout rewrites the styles of
/// `aspect_fit` elements and fitted anchored overlays, and live bindings and pictures change
/// their nodes between commits.
fn reusable(n: &Node) -> bool {
    n.aspect_fit.is_none() && n.above.as_ref().is_none_or(|a| !a.fit) && n.live.is_empty() && n.pixel_aspect.is_none()
}

impl<S: 'static> Runtime<S> {
    /// Measures a commit: lays the last tree out again when only the viewport changed since it
    /// was built, else runs the view.
    pub(super) fn measure_commit(&mut self, tx: &Tx) -> Measured {
        if !self.resize_only(tx) {
            self.refresh_layout_seen();
            return self.measure(tx);
        }
        // The view keeps what it last read; `settle_layout_reads` and the caret check run it
        // when the new layout moves them.
        let reads = self.layout_seen.reads.clone();
        let caret_read = self.caret_watch;
        let mut m = self.measurement(true);
        m.layout_reads = reads;
        m.caret_read = caret_read;
        m.relayout().unwrap_or_else(|| {
            self.refresh_layout_seen();
            self.measure(tx)
        })
    }

    /// Whether this commit only resized the window: nothing the view reads changed since the
    /// last commit built it, and the nodes it built are all there, in the layout tree's order.
    pub(super) fn resize_only(&self, tx: &Tx) -> bool {
        self.relayout && self.state_unchanged && !tx.settle && !self.reuse.read_moved
            && self.reuse.scale == self.env.scale && self.reuse.srgb == Some(self.srgb_blend)
            && self.playground.is_none() && !self.patch_pending()
            // Exiting nodes left the layout tree.
            && self.layout_entries.len() == self.nodes.len()
            && self.nodes.iter().all(reusable)
    }
}

impl<S: 'static> Measurement<'_, S> {
    /// Lays the last commit's nodes out again at the viewport, in its layout tree. None when a
    /// virtual list's window has to move, which only the view can build.
    fn relayout(mut self) -> Option<Measured> {
        self.anchor_virtuals();
        let quiet = self.quiet_rows();
        let mut taffy = std::mem::replace(self.layout, TaffyTree::with_capacity(0));
        let nodes: Vec<Node> = self.nodes.iter_mut().zip(quiet).map(|(n, quiet)| {
            // The new nodes lay their text out again; the cache then holds the only reference.
            n.text_layout = None;
            Node { run_layout: None, elided: None, virtual_mount: quiet, ..n.clone() }
        }).collect();
        let root = nodes[0].taffy;
        let mut style = taffy.style(root).unwrap().clone();
        style.size = root_size(self.viewport, self.fit_height);
        taffy.set_style(root, style).unwrap();
        // As flattening does: secure leaves don't keep a measurement.
        for n in nodes.iter().filter(|n| n.text.as_ref().is_some_and(|t| t.redacted)) {
            taffy.mark_dirty(n.taffy).unwrap();
        }
        let (layout, nodes) = self.solve(taffy, root, nodes);
        if self.measure_virtuals(&nodes, false) || self.windows_move() {
            *self.layout = layout;
            return None;
        }
        // The mounted rows are the ones shown, so none was culled.
        self.virtual_culled.clear();
        Some(Measured { layout, nodes, reduced_motion: self.reduced_motion, time_scale: self.time_scale, caret_read: self.caret_read, layout_reads: self.layout_reads })
    }

    /// Anchors each virtual list's reader to the first row in view, as a commit's window pass
    /// does, so rows above it that rewrap leave it in place.
    fn anchor_virtuals(&mut self) {
        for v in self.virtuals.values_mut() {
            let scroll = self.path_ids.get(&v.scroll_path).and_then(|id| self.scrolls.get(id));
            let sticky = || self.nodes.iter().find(|n| n.path == v.scroll_path).and_then(|n| n.scroll).is_some_and(|s| s.stick_bottom);
            let pinned = scroll.map_or_else(sticky, |s| s.pinned);
            let offset = (scroll.map_or(0.0, |s| s.offset as f32) - v.top).max(0.0);
            v.anchor = if pinned { None } else { v.first_visible(offset.min(v.max_offset()), |_| true) };
        }
    }

    /// Which nodes mount quietly, as `populate_virtual_children` marks a virtual list's rows.
    fn quiet_rows(&mut self) -> Vec<bool> {
        let mut quiet = vec![false; self.nodes.len()];
        for (path, v) in self.virtuals.iter_mut() {
            let Some(&list) = self.path_ids.get(path).and_then(|id| self.node_index.get(id)) else { continue };
            for (i, n) in self.nodes.iter().enumerate().skip(list + 1).filter(|(_, n)| n.parent == Some(list)) {
                quiet[i] = n.key.as_deref().and_then(|k| v.index_of(k)).is_some_and(|row| v.mounts_quietly(row, self.commits));
            }
        }
        quiet
    }

    /// Whether a virtual list's viewport came near the edge of its mounted rows, or its rows grew
    /// a viewport past the overscan; the view then mounts the window again.
    fn windows_move(&self) -> bool {
        self.virtuals.values().any(|v| {
            self.path_ids.get(&v.scroll_path).and_then(|id| self.scrolls.get(id)).is_some_and(|s| {
                let offset = (s.offset as f32 - v.top).max(0.0);
                v.needs_window(offset, s.pinned) || v.spare(offset, s.pinned) > v.viewport
            })
        })
    }

    /// Lays `nodes` out from `root_id` at the viewport, places them, and lays out their text.
    pub(super) fn solve(&mut self, mut taffy: TaffyTree<usize>, root_id: taffy::NodeId, mut nodes: Vec<Node>) -> (TaffyTree<usize>, Vec<Node>) {
        let scale = self.env.scale;
        let room = Size {
            width: AvailableSpace::Definite(self.viewport.0),
            height: if self.fit_height { AvailableSpace::MaxContent } else { AvailableSpace::Definite(self.viewport.1) },
        };
        {
            let _layout_time = crate::frame_profile::Scope::new(crate::frame_profile::Phase::Layout);
            compute_layout(&mut taffy, root_id, room, &nodes, &mut self.text, scale);
            // `aspect_fit` elements laid out filling their room; fix each at the largest box
            // at its ratio inside it and lay out again, so siblings and alignment follow.
            let mut refit = false;
            for n in &nodes {
                if let Some(fit) = &n.aspect_fit {
                    let style = fitted(fit.0, &fit.1, taffy.layout(n.taffy).unwrap().size);
                    taffy.set_style(n.taffy, style).unwrap();
                    refit = true;
                }
            }
            if refit { compute_layout(&mut taffy, root_id, room, &nodes, &mut self.text, scale); }
        }
        self.stats.layout_nodes = nodes.len();
        #[cfg(debug_assertions)]
        layout_diagnostics::collapsed_scrolls(&taffy, &nodes);

        self.place(&taffy, &mut nodes);
        if self.fit_anchored(&mut taffy, &nodes) {
            compute_layout(&mut taffy, root_id, room, &nodes, &mut self.text, scale);
            self.place(&taffy, &mut nodes);
        }
        for n in nodes.iter_mut().filter(|n| n.text.is_some()) {
            let w = (taffy.layout(n.taffy).unwrap().size.width - n.inset[0] - n.inset[2]).max(0.0);
            lay_out_text(&mut self.text, n, w, scale);
        }

        (taffy, nodes)
    }
}

/// Lays `n`'s text out again at the width it was laid out at.
pub(super) fn lay_out_text_in_place(text: &mut TextSystem, n: &mut Node, scale: f32) {
    let w = (n.local.w - n.inset[0] - n.inset[2]).max(0.0);
    lay_out_text(text, n, w, scale);
}

/// Lays `n`'s text out at its content width `w`.
pub(super) fn lay_out_text(text: &mut TextSystem, n: &mut Node, w: f32, scale: f32) {
    let Some(spec) = &n.text else { return };
    // A cell of a run draws from the run's layout (and keeps it cached) instead of its own.
    let run = spec.advance_in.as_ref().map(|(run, at)| (text.run_layout(run, spec.style, scale), *at));
    if let Some((r, _)) = run.filter(|(r, at)| r.x(*at).is_some()) {
        n.text_layout = Some(r.layout.clone());
        n.run_layout = Some(r);
        return;
    }
    let wrap = spec.wrap.then_some(w * scale);
    let mut layout = text.layout_for_spec(spec, scale, wrap, line_alignment(spec));
    let mut elided = None;
    if spec.truncate && layout.full_width() > w * scale + 0.5 {
        let (cut, e) = truncated(text, spec, scale, w * scale);
        (layout, elided) = (cut, Some(e));
    } else if let Some((cut, e)) = clamped(text, spec, scale, w * scale, &layout) {
        (layout, elided) = (cut, Some(e));
    }
    n.text_layout = Some(layout);
    n.elided = elided;
}
