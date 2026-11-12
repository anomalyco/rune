//! Virtual lists between commits: whether a window wants moving, and which rows show.

use super::*;
use super::virtual_fill::{FillEffect, FillEvent};

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
                .map_or((0.0, false), |s| ((s.offset as f32 - v.top).max(0.0), s.pinned()));
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

    /// Notes each list's rows drawn inside its viewport on the last frame, so the next window
    /// keeps them mounted while they're still on screen.
    pub(super) fn note_drawn_rows(&mut self) {
        if self.virtuals.is_empty() { return; }
        // The last frame's rects describe these nodes only until a commit replaces them.
        let fresh = self.built && self.drawn.len() == self.nodes.len();
        let (nodes, drawn) = (&self.nodes, &self.drawn);
        let at = |path: &str| self.path_ids.get(path).and_then(|id| self.node_index.get(id)).copied().filter(|&i| nodes.get(i).is_some_and(|n| n.path == path));
        for (path, v) in self.virtuals.iter_mut() {
            v.drawn.clear();
            let (true, Some(list), Some(view)) = (fresh, at(path), at(&v.scroll_path)) else { continue };
            let (top, bottom) = (drawn[view][1], drawn[view][1] + drawn[view][3]);
            let rows = (list + 1..nodes.len()).take_while(|&j| nodes[j].parent >= Some(list)).filter(|&j| nodes[j].parent == Some(list));
            for j in rows.filter(|&j| drawn[j][3] > 0.0 && drawn[j][1] < bottom && drawn[j][1] + drawn[j][3] > top) {
                v.drawn.extend(nodes[j].key.clone());
            }
        }
    }

    pub(super) fn wants_window(&self, path: &str, v: &crate::el::virtual_list::VirtualList) -> bool {
        let Some(&i) = self.path_ids.get(path).and_then(|id| self.node_index.get(id)) else { return false };
        if self.subtree_exiting(i) { return false; }
        let (offset, pinned) = self.window_offset(v);
        v.needs_window(offset, pinned)
    }

    /// A list's scroll offset within its window, and whether that viewport is pinned to its end.
    fn window_offset(&self, v: &crate::el::virtual_list::VirtualList) -> (f32, bool) {
        self.path_ids.get(&v.scroll_path).and_then(|id| self.scrolls.get(id))
            .map_or((0.0, false), |s| ((s.offset as f32 - v.top).max(0.0), s.pinned()))
    }

    /// Whether an index-built list wants its window moved, and whether one of those lists' own
    /// viewports moved this frame. Another viewport's motion invalidates nothing here.
    fn virtual_demand(&self) -> (bool, bool) {
        let (mut wanted, mut moved) = (false, false);
        for (path, v) in &self.virtuals {
            if !v.indexed || !self.wants_window(path, v) { continue; }
            wanted = true;
            moved |= self.path_ids.get(&v.scroll_path).and_then(|id| self.scrolls.get(id)).is_some_and(|s| s.moving());
        }
        (wanted, moved)
    }

    /// Rows an index-built list still has to mount across the lists that want a window moved;
    /// zero when every one of them is filled.
    fn virtual_unfilled(&self) -> usize {
        let mut unfilled = 0;
        for (path, v) in &self.virtuals {
            if !v.indexed || !self.wants_window(path, v) { continue; }
            let (offset, pinned) = self.window_offset(v);
            unfilled += v.unfilled(offset, pinned);
        }
        unfilled
    }

    /// Whether an index-built list still mounts its overscan a little per frame, so the host
    /// should draw another. Each such frame commits more rows; a few frames fill it. A list
    /// whose heights keep changing past those frames waits a few frames out and resumes, until
    /// the unfilled range stops shrinking.
    pub(super) fn virtual_filling(&mut self) -> bool {
        let now = self.clock;
        let (wanted, moved) = self.virtual_demand();
        // A wake must draw nothing for a list that finished, unmounted or stopped filling: it
        // retires the wait, and the deadline it left draws no frame of its own.
        if !wanted {
            self.virtual_fill.step(FillEvent::Frame { now, wanted: false, missing: 0, moved: false });
            return false;
        }
        // The fill's own deadline, which it clears when it leaves the wait: a deadline a retired
        // run left behind is not this call's cause, so the frame is read as any other.
        if let Some(attempt) = self.virtual_fill.wake(now) {
            return !self.virtual_fill.step(FillEvent::Due { attempt, now }).is_empty();
        }
        // The count is only read at the bound, so a fill that finishes inside it costs none.
        let missing = if self.virtual_fill.at_bound() { self.virtual_unfilled() } else { 0 };
        let mut draw = false;
        for effect in self.virtual_fill.step(FillEvent::Frame { now, wanted, missing, moved }) {
            match effect {
                FillEffect::Draw => draw = true,
                FillEffect::WakeAt(due) => self.wake_at(due),
            }
        }
        draw
    }
}
