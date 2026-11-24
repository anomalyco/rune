//! `El::scroll_anchor`: a viewport keeps the row at the top of its view where it is when a
//! commit changes the content above it, as CSS `overflow-anchor: auto` does. Each commit
//! holds that row with the same `Keep` a `Tx::keep_on_screen` makes, so it follows growth and
//! layout springs too; a commit that changes nothing above the row moves nothing.
use super::*;

impl requests::ScrollTables {
    /// Whether viewport `s` anchors now: scrolled off its top, not following its bottom, still
    /// (a hold would stop a fling or a glide where it is), and not already holding a row or
    /// revealing one.
    pub(super) fn anchor_holds(&self, nodes: &[Node], s: usize) -> bool {
        let path = &nodes[s].path;
        if self.requests.owns(path) { return false; }
        self.scrolls.get(&nodes[s].id).is_some_and(|st| {
            st.offset > 0.5 && !st.pinned() && !st.scrolling() && !st.direct() && st.coast() == 0.0 && st.glide().is_none()
        })
    }
}

/// Bounded fallback chain per `El::scroll_anchor` viewport so removing the primary candidate
/// promotes the next surviving visible sibling or descendant.
const MAX_ANCHOR_CANDIDATES: usize = 8;

/// Recursion depth cap for `collect_anchor_candidates` on deeply nested trees.
const MAX_ANCHOR_DEPTH: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum VirtualDepth {
    Outside,
    InList,
    InsideRow,
}

impl RuntimeCore {
    /// Before a commit: captures each anchored viewport's visible candidate chain so a surviving
    /// element stays put when content above it changes. Explicit `Tx::keep_on_screen` entries
    /// arrive right after in `capture_keeps` and replace these on the same viewport.
    pub(in crate::runtime) fn capture_anchor_keeps(&mut self) {
        if !self.tree.built || self.paint.drawn.len() != self.tree.nodes.len() { return; }
        let mut keeps = Vec::new();
        for s in 0..self.tree.nodes.len() {
            let n = &self.tree.nodes[s];
            if !n.scroll_edges.anchor || n.scroll_edges.no_anchor || !n.scroll_axes.y() { continue; }
            if !self.scroll.anchor_holds(&self.tree.nodes, s) { continue; }
            let candidates = self.anchor_candidates(s);
            let Some((&first, rest)) = candidates.split_first() else { continue };
            let fallbacks = rest.iter().map(|&i| self.build_keep_candidate(i, s)).collect();
            keeps.push(self.build_keep(first, s, fallbacks));
        }
        for keep in keeps {
            self.scroll.requests.step(requests::Event::Arrive(requests::Request::Capture(keep)));
        }
    }

    fn anchor_candidate_ok(&self, j: usize, top: f32, bottom: f32) -> bool {
        let [_, y, w, h] = self.paint.drawn[j];
        w > 0.0
            && h > 0.0
            && y + h > top + 0.5
            && y < bottom
            && self.tree.nodes[j].sticky.is_none()
            && !self.tree.nodes[j].scroll_edges.no_anchor
            && !self.subtree_exiting(j)
    }

    /// Ordered visible anchor candidates inside viewport `s` (CSS Scroll Anchoring §2.2).
    /// Descends into partially scrolled containers, including tall mounted `virtual_rows` rows
    /// whose top lies above the viewport top, while leaving whole-row virtual lists to
    /// `VirtualList`'s own anchor path.
    fn anchor_candidates(&self, s: usize) -> Vec<usize> {
        if self.commit.virtuals.contains_key(&self.tree.nodes[s].path) {
            return Vec::new();
        }
        let top = self.paint.drawn[s][1];
        let bottom = top + self.paint.drawn[s][3];
        let mut out = Vec::new();
        self.collect_anchor_candidates(s, top, bottom, VirtualDepth::Outside, MAX_ANCHOR_DEPTH, &mut out);
        out
    }

    /// Walks `parent`'s direct children in tree order. Returns `true` when the first visible
    /// content is a virtual list or direct virtual row whose whole-row anchor should own the
    /// viewport (so callers do not skip past it to later siblings).
    fn collect_anchor_candidates(
        &self,
        parent: usize,
        top: f32,
        bottom: f32,
        depth: VirtualDepth,
        depth_left: usize,
        out: &mut Vec<usize>,
    ) -> bool {
        let mut j = parent + 1;
        while j < self.tree.nodes.len() && self.tree.nodes[j].parent.is_some_and(|p| p >= parent) {
            let child = j;
            j += 1;
            while j < self.tree.nodes.len() && self.tree.nodes[j].parent.is_some_and(|p| p >= child) {
                j += 1;
            }
            if self.tree.nodes[child].parent != Some(parent) { continue; }
            if !self.anchor_candidate_ok(child, top, bottom) { continue; }

            let is_virtual = self.commit.virtuals.contains_key(&self.tree.nodes[child].path);
            let inner_scroller = self.tree.nodes[child].scroll.is_some() && self.tree.nodes[child].scroll_axes.y();
            let has_children = depth_left > 0 && !inner_scroller && child + 1 < j;
            let starts_above = self.paint.drawn[child][1] < top - 0.5;

            if is_virtual {
                if starts_above && has_children {
                    let before = out.len();
                    let owned_by_virtual = self.collect_anchor_candidates(
                        child, top, bottom, VirtualDepth::InList, depth_left - 1, out,
                    );
                    if out.len() > before {
                        return false;
                    }
                    if owned_by_virtual {
                        return true;
                    }
                } else {
                    return true;
                }
                continue;
            }

            if depth == VirtualDepth::InList {
                if starts_above && has_children {
                    let before = out.len();
                    self.collect_anchor_candidates(
                        child, top, bottom, VirtualDepth::InsideRow, depth_left - 1, out,
                    );
                    if out.len() > before {
                        return false;
                    }
                }
                return true;
            }

            if starts_above && has_children {
                let before = out.len();
                let owned_by_virtual = self.collect_anchor_candidates(
                    child, top, bottom, depth, depth_left - 1, out,
                );
                if owned_by_virtual && out.len() == before {
                    return true;
                }
                if out.len() == before {
                    out.push(child);
                }
            } else {
                out.push(child);
            }

            if out.len() >= MAX_ANCHOR_CANDIDATES {
                return false;
            }
        }
        false
    }
}
