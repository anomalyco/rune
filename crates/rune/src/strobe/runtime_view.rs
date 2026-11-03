//! What the strobe bridge reads of a runtime: its elements as selectors see them, without
//! building `ElementInfo`s, and the counters a run reports.

use super::*;
use crate::strobe::select::Candidate;

impl<S: 'static> Runtime<S> {
    /// Calls `f` with each presented element in tree order until it returns true.
    pub(crate) fn strobe_each(&self, mut f: impl FnMut(&Candidate) -> bool) {
        let s = self.env.scale;
        for (i, n) in self.nodes.iter().enumerate() {
            if self.scene.presented(n.id).is_none() { continue; }
            let Some(&rect) = self.drawn.get(i) else { continue };
            let clip = self.screen_clip_of_parent(n.parent);
            let (x0, y0) = (rect[0].max(clip[0] / s), rect[1].max(clip[1] / s));
            let (x1, y1) = ((rect[0] + rect[2]).min(clip[2] / s), (rect[1] + rect[3]).min(clip[3] / s));
            let exiting = self.subtree_exiting(i);
            let secure = n.is_secure();
            let candidate = Candidate {
                key: n.key.as_deref(),
                path: &n.path,
                role: n.role,
                label: n.label.as_deref(),
                text: n.text.as_ref().filter(|_| !secure).map(|t| t.source_text()),
                rect,
                visible: [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)],
                opacity: self.inh_scratch.get(i).map_or(0.0, |h| h.opacity),
                exiting,
                focused: self.focused.as_deref() == Some(n.path.as_str()),
                enabled: self.allowed(i) && !exiting,
            };
            if f(&candidate) { return; }
        }
    }

    /// Commits so far, texts shaped so far and elements now.
    pub(crate) fn strobe_tally(&self) -> crate::strobe::Tally {
        crate::strobe::Tally { commits: self.stats.commits, shaped: u64::from(self.text.shaped), elements: self.nodes.len() }
    }
}
