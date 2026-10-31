//! Virtual lists between commits: whether a window wants moving, and which rows show.

use super::*;

/// The most frames in a row an index-built list keeps the host drawing to fill its overscan.
/// Filling takes four at most (half a viewport per side each); this bounds a list whose
/// heights keep changing.
const VIRTUAL_FILL_FRAMES: u32 = 12;

impl<S: 'static> Runtime<S> {
    pub(super) fn virtual_window_changed(&self) -> bool {
        self.virtuals.iter().any(|(path, v)| self.wants_window(path, v))
    }

    pub(super) fn report_visible_rows(&mut self) -> bool {
        if self.virtuals.is_empty() { return false; }
        let mut queued = false;
        for i in 0..self.nodes.len() {
            if self.nodes[i].handlers.get().is_none_or(|h| h.on_visible_rows.is_none()) || self.subtree_exiting(i) { continue; }
            let path = &self.nodes[i].path;
            let Some(v) = self.virtuals.get_mut(path) else { continue };
            let (offset, pinned) = self.path_ids.get(&v.scroll_path)
                .and_then(|id| self.scrolls.get(id))
                .map_or((0.0, false), |s| ((s.offset as f32 - v.top).max(0.0), s.pinned));
            let range = v.visible_range(offset, pinned);
            if v.reported_visible.as_ref() != Some(&range) {
                v.reported_visible = Some(range.clone());
                let p = path.clone();
                self.queue.retain(|a| !matches!(a, Action::VisibleRows(q, _) if *q == p));
                self.queue.push(Action::VisibleRows(p, range));
                queued = true;
            }
        }
        queued
    }

    pub(super) fn wants_window(&self, path: &str, v: &crate::el::virtual_list::VirtualList) -> bool {
        let Some(&i) = self.path_ids.get(path).and_then(|id| self.node_index.get(id)) else { return false };
        if self.subtree_exiting(i) { return false; }
        self.path_ids.get(&v.scroll_path).and_then(|id| self.scrolls.get(id))
            .is_some_and(|s| v.needs_window((s.offset as f32 - v.top).max(0.0), s.pinned))
    }

    /// Whether an index-built list still mounts its overscan a little per frame, so the host
    /// should draw another. Each such frame commits more rows; a few frames fill it.
    pub(super) fn virtual_filling(&mut self) -> bool {
        let filling = self.virtuals.iter().any(|(path, v)| v.indexed && self.wants_window(path, v));
        self.virtual_fill = if filling { self.virtual_fill + 1 } else { 0 };
        filling && self.virtual_fill <= VIRTUAL_FILL_FRAMES
    }
}
