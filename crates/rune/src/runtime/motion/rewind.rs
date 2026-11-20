//! Feeds an attached [`Rewind`](crate::rewind::Rewind) each frame: every keyed or trace-named
//! element on screen, its drawn rect and opacity (ancestors included).

use super::*;

impl tracks::MotionState {
    pub(crate) fn record_rewind(&self, clock: f64, viewport: (f32, f32), nodes: &[Node], paint: &paint_state::PaintState) {
        let Some(rewind) = &self.rewind else { return };
        let mut tape = rewind.lock();
        if tape.paused { return; }
        tape.begin(clock, viewport);
        if paint.drawn.len() == nodes.len() && paint.inh_scratch.len() == nodes.len() {
            for (i, n) in nodes.iter().enumerate() {
                if n.key.is_none() && n.trace_name.is_none() { continue; }
                if !paint.was_seen(i, true) { continue; }
                let q = crate::rewind::quantize(paint.drawn[i], paint.inh_scratch[i].opacity);
                tape.sample(n.id.0, q, || n.trace_name.map_or_else(|| n.key.clone().unwrap_or_else(|| n.path.clone()), str::to_owned));
            }
        }
        tape.end();
    }

    /// The recorder attached with `Tx::record_rewind`, if any.
    #[inline]
    pub(crate) fn rewind(&self) -> Option<&crate::rewind::Rewind> { self.rewind.as_ref() }
}
