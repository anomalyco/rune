//! Feeds an attached [`Rewind`](crate::rewind::Rewind) each frame: every keyed or trace-named
//! element on screen, its drawn rect and opacity (ancestors included).

use super::*;

impl RuntimeCore {
    pub(crate) fn record_rewind(&mut self) {
        let Some(rewind) = &self.motion.rewind else { return };
        let mut tape = rewind.lock();
        if tape.paused { return; }
        tape.begin(self.clock, self.viewport);
        if self.paint.drawn.len() == self.tree.nodes.len() && self.paint.inh_scratch.len() == self.tree.nodes.len() {
            for (i, n) in self.tree.nodes.iter().enumerate() {
                if n.key.is_none() && n.trace_name.is_none() { continue; }
                if !self.paint.seen.get(i).copied().unwrap_or(true) { continue; }
                let q = crate::rewind::quantize(self.paint.drawn[i], self.paint.inh_scratch[i].opacity);
                tape.sample(n.id.0, q, || n.trace_name.map_or_else(|| n.key.clone().unwrap_or_else(|| n.path.clone()), str::to_owned));
            }
        }
        tape.end();
    }

    /// The recorder attached with `Tx::record_rewind`, if any.
    pub fn rewind(&self) -> Option<&crate::rewind::Rewind> { self.motion.rewind.as_ref() }
}
