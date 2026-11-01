//! A fold shutting under its own header, held stuck to the top of its viewport by
//! `Tx::keep_on_screen`. Most of what it held may sit above that header, out of sight: that part
//! goes at once, with the viewport's offset dropping by as much so nothing on screen moves, and
//! the contents left ride the fold's bottom edge (`GrowTrack::hide_above`). What shows then
//! slides up under the header on the fold's spring, the rows below rising with it, and once
//! the viewport's end holds them there the header comes down over the rest instead. Opened
//! again before it lands, the contents ride back down, and what went above comes back there
//! as the glide lands (`restore_above`), again with the offset taking it up; at a pinned end,
//! it grows from its bottom instead, the header rising back to the top (`ride_from_end`).
use super::*;

impl<S: 'static> Runtime<S> {
    /// Row `j` of the commit's `nodes`, a fold shutting to `to` points: how many of its points
    /// sit above the header held stuck in it, and that header's path, when it keeps no more
    /// than that header shows.
    pub(super) fn fold_hidden_above(&self, nodes: &[Node], j: usize, to: f32) -> Option<(f32, String)> {
        if self.keeps.is_empty() || !self.built || self.stuck.len() != self.nodes.len() { return None; }
        let row = &nodes[j].path;
        let within = |path: &str| path.starts_with(row.as_str()) && path.as_bytes().get(row.len()) == Some(&b'/');
        let hold = self.keeps.iter().filter_map(|k| k.hold.as_ref()).find(|h| within(&h.path))?;
        let mut end = j + 1;
        while end < nodes.len() && nodes[end].parent.is_some_and(|p| p >= j) { end += 1; }
        let header = (j + 1..end).find(|&i| nodes[i].path == hold.path)?;
        if to > nodes[header].target[3] - nodes[j].target[1] + 0.5 { return None; }
        let was = self.nodes.iter().position(|n| n.path == hold.path)?;
        Some((self.stuck[was], hold.path.clone())).filter(|&(s, _)| s > 0.5)
    }

    /// The scroll viewport around growth stack `c` of the commit's `nodes` steps `cut` points
    /// back, as the folds in it dropped that much above what shows.
    pub(super) fn scroll_by_cut(&mut self, nodes: &[Node], c: usize, cut: f32) {
        let Some(s) = std::iter::successors(nodes[c].parent, |&p| nodes[p].parent)
            .find(|&p| nodes[p].scroll.is_some() && nodes[p].scroll_axes.y()) else { return };
        let Some(st) = self.scrolls.get_mut(&nodes[s].id) else { return };
        let cut = f64::from(cut);
        st.offset = (st.offset - cut).max(0.0);
        st.max = (st.max - cut).max(0.0);
        if let Some(to) = st.glide_to.as_mut() { *to = (*to - cut).max(0.0); }
    }

    /// Row `j` of the commit's `nodes`, a fold opening again while its contents draw lifted,
    /// in growth stack `c`: with its viewport pinned to its end, the holds on it, in it or above
    /// it let go, so it grows from its bottom with the end held and its contents still, rather
    /// than pushing the end down and then snapping its header up as it lands.
    pub(super) fn ride_from_end(&mut self, nodes: &[Node], c: usize, j: usize) {
        let Some(s) = std::iter::successors(nodes[c].parent, |&p| nodes[p].parent)
            .find(|&p| nodes[p].scroll.is_some() && nodes[p].scroll_axes.y()) else { return };
        if !self.scrolls.get(&nodes[s].id).is_some_and(|st| st.pinned) { return; }
        let mut end = j + 1;
        while end < nodes.len() && nodes[end].parent.is_some_and(|p| p >= j) { end += 1; }
        let viewport = &nodes[s].path;
        self.keeps.retain(|k| k.viewport != *viewport || nodes.iter().position(|n| n.path == k.path).is_none_or(|i| i >= end));
        // How far below the viewport's top the fold's top showed: its header rises that far.
        let shown = (self.drawn.len() == self.nodes.len()).then(|| {
            let row = self.nodes.iter().position(|n| n.id == nodes[j].id)?;
            let view = self.nodes.iter().position(|n| n.id == nodes[s].id)?;
            let scale = self.drawn_scale.get(view).copied().filter(|&k| k > 1e-4)?;
            Some(((self.drawn[row][1] - self.drawn[view][1]) / scale).max(0.0))
        }).flatten().unwrap_or(0.0);
        let Some(g) = self.grows.get_mut(&nodes[j].id) else { return };
        // And what its contents hid below its bottom edge shows.
        let below = (g.target - g.h - g.lift()).max(0.0);
        g.ride_up(shown + below);
    }

    /// Fold `row` in growth stack `ci` landed whole with `r` points of its contents still drawn
    /// lifted: they come back above it, out of sight, so its viewport's offset steps `r` on and
    /// the holds on the fold or above it expect it that much higher.
    pub(super) fn restore_above(&mut self, ci: usize, row: NodeId, r: f32) {
        let Some(s) = self.viewport_of(ci) else { return };
        let (path, id, scale) = (self.nodes[s].path.clone(), self.nodes[s].id, self.drawn_scale.get(s).copied().unwrap_or(1.0));
        let Some(st) = self.scrolls.get_mut(&id) else { return };
        if st.pinned { return; }
        st.offset += f64::from(r);
        st.max = st.max.max(st.offset);
        let Some(&ri) = self.node_index.get(&row) else { return };
        for k in self.keeps.iter_mut().filter(|k| k.viewport == path) {
            if self.nodes.iter().position(|n| n.path == k.path).is_some_and(|i| i <= ri) { k.was -= r * scale; }
        }
    }
}
