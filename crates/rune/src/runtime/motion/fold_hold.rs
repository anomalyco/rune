//! A fold shutting under its own header, held stuck to the top of its viewport by
//! `Tx::keep_on_screen`. Most of what it held may sit above that header, out of sight: that part
//! goes at once, with the viewport's offset dropping by as much so nothing on screen moves, and
//! the contents left ride the fold's bottom edge (`GrowTrack::hide_above`). What shows then
//! slides up under the header on the fold's spring, the rows below rising with it, and once
//! the viewport's end holds them there the header comes down over the rest instead.
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
}
