//! Virtual lists between commits: whether a window wants moving, and which rows show.

use super::*;
use super::fill::{FillEffect, FillEvent};

impl types::TreeState {
    /// Notes each list's rows drawn inside its viewport on the last frame, so the next window
    /// keeps them mounted while they're still on screen.
    pub(in crate::runtime) fn note_drawn_rows(
        &self,
        drawn: &[[f32; 4]],
        virtuals: &mut HashMap<String, crate::el::virtual_list::VirtualList>,
    ) {
        if virtuals.is_empty() { return; }
        // The last frame's rects describe these nodes only until a commit replaces them.
        let fresh = self.is_built() && drawn.len() == self.nodes.len();
        let nodes = &self.nodes;
        for (path, v) in virtuals.iter_mut() {
            v.drawn.clear();
            let (true, Some(list), Some(view)) = (fresh, self.index_of_path(path), self.index_of_path(&v.scroll_path)) else { continue };
            let (top, bottom) = (drawn[view][1], drawn[view][1] + drawn[view][3]);
            let rows = (list + 1..nodes.len()).take_while(|&j| nodes[j].parent >= Some(list)).filter(|&j| nodes[j].parent == Some(list));
            for j in rows.filter(|&j| drawn[j][3] > 0.0 && drawn[j][1] < bottom && drawn[j][1] + drawn[j][3] > top) {
                v.drawn.extend(nodes[j].key.clone());
            }
        }
    }

    /// A list's scroll offset within its window, and whether that viewport is pinned to its end.
    #[inline]
    pub(in crate::runtime) fn virtual_window_offset(
        &self,
        scrolls: &HashMap<NodeId, ScrollState>,
        v: &crate::el::virtual_list::VirtualList,
    ) -> (f32, bool) {
        self.id_of_path(&v.scroll_path).and_then(|id| scrolls.get(&id))
            .map_or((0.0, false), |s| v.at(s.offset as f32, s.pinned()))
    }
}

impl RuntimeCore {
    pub(in crate::runtime) fn virtual_window_changed(&self) -> bool {
        self.commit.virtuals.iter().any(|(path, v)| self.wants_window(path, v))
    }

    /// Whether any live virtual list's viewport reaches past its mounted rows, leaving part of it
    /// blank until another commit mounts rows there.
    pub(in crate::runtime) fn virtual_viewport_uncovered(&self) -> bool {
        self.commit.virtuals.iter().any(|(path, v)| self.viewport_uncovered(path, v))
    }

    fn viewport_uncovered(&self, path: &str, v: &crate::el::virtual_list::VirtualList) -> bool {
        let Some(i) = self.tree.id_of_path(path).and_then(|id| self.tree.index_of_id(id)) else { return false };
        if self.subtree_exiting(i) { return false; }
        let (offset, pinned) = self.tree.virtual_window_offset(&self.scroll.scrolls, v);
        v.viewport_uncovered(offset, pinned)
    }

    pub(in crate::runtime) fn report_visible_rows(&mut self) -> bool {
        if self.commit.virtuals.is_empty() { return false; }
        let mut queued = false;
        for i in 0..self.tree.nodes.len() {
            if self.tree.nodes[i].handlers.get().is_none_or(|h| h.on_visible_rows.is_none()) || self.subtree_exiting(i) { continue; }
            let path = &self.tree.nodes[i].path;
            let Some(v) = self.commit.virtuals.get_mut(path) else { continue };
            let (offset, pinned) = self.tree.virtual_window_offset(&self.scroll.scrolls, v);
            let range = v.visible_range(offset, pinned);
            if v.reported_visible.as_ref() != Some(&range) {
                v.reported_visible = Some(range.clone());
                let p = path.clone();
                self.frame.queue.retain(|a| !matches!(a, Action::VisibleRows(q, _) if *q == p));
                self.frame.queue.push(Action::VisibleRows(p, range));
                queued = true;
            }
        }
        queued
    }

    /// Notes each list's rows drawn inside its viewport on the last frame, so the next window
    /// keeps them mounted while they're still on screen.
    #[inline]
    pub(in crate::runtime) fn note_drawn_rows(&mut self) {
        self.tree.note_drawn_rows(&self.paint.drawn, &mut self.commit.virtuals);
    }

    pub(in crate::runtime) fn wants_window(&self, path: &str, v: &crate::el::virtual_list::VirtualList) -> bool {
        let Some(i) = self.tree.id_of_path(path).and_then(|id| self.tree.index_of_id(id)) else { return false };
        if self.subtree_exiting(i) { return false; }
        let (offset, pinned) = self.tree.virtual_window_offset(&self.scroll.scrolls, v);
        v.needs_window(offset, pinned)
    }

    /// Whether an index-built list wants its window moved, and whether one of those lists' own
    /// viewports moved this frame. Another viewport's motion invalidates nothing here.
    fn virtual_demand(&self) -> (bool, bool) {
        let (mut wanted, mut moved) = (false, false);
        for (path, v) in &self.commit.virtuals {
            if !v.indexed || !self.wants_window(path, v) { continue; }
            wanted = true;
            moved |= self.tree.id_of_path(&v.scroll_path).and_then(|id| self.scroll.scrolls.get(&id)).is_some_and(|s| s.moving());
        }
        (wanted, moved)
    }

    /// Rows an index-built list still has to mount across the lists that want a window moved;
    /// zero when every one of them is filled.
    fn virtual_unfilled(&self) -> usize {
        let mut unfilled = 0;
        for (path, v) in &self.commit.virtuals {
            if !v.indexed || !self.wants_window(path, v) { continue; }
            let (offset, pinned) = self.tree.virtual_window_offset(&self.scroll.scrolls, v);
            unfilled += v.unfilled(offset, pinned);
        }
        unfilled
    }

    /// Whether an index-built list still mounts its overscan a little per frame, so the host
    /// should draw another. Each such frame commits more rows; a few frames fill it. A list
    /// whose heights keep changing past those frames waits a few frames out and resumes, until
    /// the unfilled range stops shrinking.
    pub(in crate::runtime) fn virtual_filling(&mut self) -> bool {
        let now = self.clock;
        let (wanted, moved) = self.virtual_demand();
        // A wake must draw nothing for a list that finished, unmounted or stopped filling: it
        // retires the wait, and the deadline it left draws no frame of its own.
        if !wanted {
            self.commit.virtual_fill.step(FillEvent::Frame { now, wanted: false, missing: 0, moved: false });
            return false;
        }
        // The fill's own deadline, which it clears when it leaves the wait: a deadline a retired
        // run left behind is not this call's cause, so the frame is read as any other.
        if let Some(attempt) = self.commit.virtual_fill.wake(now) {
            return !self.commit.virtual_fill.step(FillEvent::Due { attempt, now }).is_empty();
        }
        // The count is only read at the bound, so a fill that finishes inside it costs none.
        let missing = if self.commit.virtual_fill.at_bound() { self.virtual_unfilled() } else { 0 };
        let mut draw = false;
        for effect in self.commit.virtual_fill.step(FillEvent::Frame { now, wanted, missing, moved }) {
            match effect {
                FillEffect::Draw => draw = true,
                FillEffect::WakeAt(due) => self.wake_at(due),
            }
        }
        draw
    }
}
