//! Which nodes sit in an exiting subtree, found once a frame. Hit registration, reports and
//! sticky headers ask this of most nodes; without it each asks the scene for every ancestor.

use super::*;

/// One flag per node of the commit it was filled for: the node or an ancestor is exiting.
pub(super) enum Mask {
    /// The last frame's flags, kept only for their buffer: they answer nothing.
    Forgotten { exiting: Vec<bool> },
    /// The flags of commit `commit`, one per node: they answer only while that commit and the
    /// node count hold.
    Fresh { exiting: Vec<bool>, commit: u64 },
}

impl Default for Mask {
    fn default() -> Self { Mask::Forgotten { exiting: Vec::new() } }
}

impl Mask {
    /// The flags, wherever they came from: the buffer a fill builds into.
    fn into_flags(self) -> Vec<bool> {
        match self {
            Mask::Forgotten { exiting } | Mask::Fresh { exiting, .. } => exiting,
        }
    }

    pub(super) fn forget(&mut self) {
        *self = Mask::Forgotten { exiting: std::mem::take(self).into_flags() };
    }

    fn get(&self, i: usize, len: usize, commit: u64) -> Option<bool> {
        match self {
            Mask::Fresh { exiting, commit: c } if *c == commit && exiting.len() == len => Some(exiting[i]),
            Mask::Forgotten { .. } | Mask::Fresh { .. } => None,
        }
    }
}

impl<S: 'static> Runtime<S> {
    /// Fills the mask for this frame's paint and reports. Call [`Self::end_exit_mask`] once
    /// they're done, so nothing reads it after a phase changes.
    pub(super) fn begin_exit_mask(&mut self) {
        let mut exiting = std::mem::take(&mut self.exit_mask).into_flags();
        exiting.clear();
        exiting.reserve(self.nodes.len());
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            let up = n.parent.is_some_and(|p| if p < i { exiting[p] } else { self.subtree_exiting(p) });
            exiting.push(up || self.is_exiting(n.id));
        }
        self.exit_mask = Mask::Fresh { exiting, commit: self.commit_gen };
    }

    pub(super) fn end_exit_mask(&mut self) {
        self.exit_mask.forget();
    }

    pub(super) fn subtree_exiting(&self, mut i: usize) -> bool {
        if let Some(exiting) = self.exit_mask.get(i, self.nodes.len(), self.commit_gen) {
            return exiting;
        }
        loop {
            if self.is_exiting(self.nodes[i].id) { return true; }
            let Some(parent) = self.nodes[i].parent else { return false };
            i = parent;
        }
    }
}
