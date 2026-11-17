//! Text laid out ahead (`Tx::prepare_text`): a view the app names, measured as the window would
//! measure it, into the window's layout cache, without mounting anything. Its nodes, layout tree,
//! ids and memo are the pass's own and go when it ends, so nothing draws, hits, animates or is
//! announced, and no handler or live value sees it. What it shaped stays in the cache until a view
//! shows it (then it is found, not shaped) or the cache lets it go: past [`PREPARE_BYTES`] of
//! prepared layouts the oldest go first, and all of them at the window's next rest. One request is
//! held at a time, the latest; it runs in the window's next idle turn after its first frame, and
//! none while input waits. Each idle turn works for one [`PREPARE_SLICE`]: the first measures the
//! view and shapes what it can in its slice, queuing the rest (`text/prepare_budget.rs`); each
//! later turn shapes queued texts for a slice, up to [`PREPARE_BYTES`] in all. The request and
//! its queue are the window's own; a press or a key in it drops them.

use super::*;

/// The most prepared layouts no view holds may take in the layout cache, by the cache's own count
/// (`LAYOUT_PER_BYTE`): about one transcript screen.
pub(crate) const PREPARE_BYTES: usize = 1 << 20;

/// How long one idle turn shapes for. A press that lands in a turn waits for the rest of it plus
/// the text it was shaping: about a quarter of a 240 Hz frame (4,167 µs), so the press's own
/// frame still fits. Shorter slices pay the idle turn's own overhead more often for nothing a
/// person can see. A request's first turn also measures the view, which no slice cuts.
pub(crate) const PREPARE_SLICE: std::time::Duration = std::time::Duration::from_micros(1_000);

impl<S: 'static> Runtime<S> {
    /// Whether an idle turn has a view to prepare for this window.
    pub(crate) fn text_idle_owed(&self) -> bool {
        self.text.prepare_owed()
    }

    /// One idle turn's text work: the view asked to be prepared, if any; true while more is owed.
    pub(crate) fn text_idle_turn(&mut self) -> bool {
        let at = ([self.viewport.0, self.viewport.1], self.env.scale);
        let t0 = std::time::Instant::now();
        if let Some((view, size)) = self.text.prepare_take::<fn(&S, &mut Cx<S>) -> El>() {
            self.prepare_text(view, size.unwrap_or(self.viewport), at);
            crate::text::probe_note(format_args!("prepare us={:.0} queued={}", t0.elapsed().as_secs_f64() * 1e6, self.text_idle_owed()));
        } else {
            let n = self.text.prepare_drain(std::time::Instant::now() + PREPARE_SLICE, at);
            crate::text::probe_note(format_args!("drain us={:.0} made={n} more={}", t0.elapsed().as_secs_f64() * 1e6, self.text_idle_owed()));
        }
        self.text_idle_owed()
    }

    /// Measures `view` at `size` (points) with this window's text system and scratch everything
    /// else, then drops it, shaping for one [`PREPARE_SLICE`] from the start and queuing the
    /// rest for a window at `at` (its viewport and scale).
    pub(crate) fn prepare_text(&mut self, view: fn(&S, &mut Cx<S>) -> El, size: (f32, f32), at: ([f32; 2], f32)) {
        let pass = self.text.prepare_open(std::time::Instant::now() + PREPARE_SLICE);
        let before = self.text.layout_keys();
        let (mut nodes, mut spare_nodes, mut layout) = (Vec::new(), Vec::new(), LayoutEngine::new());
        let (mut node_index, mut path_ids, mut memo) = (HashMap::default(), HashMap::default(), crate::memo::MemoCache::default());
        let (mut next_id, mut scrolls, mut virtuals) = (0u64, HashMap::default(), HashMap::default());
        let (mut virtual_culled, mut virtual_inputs, mut stats, mut key_warnings) = (HashSet::default(), HashMap::default(), FrameStats::default(), None);
        let m = Measurement {
            state: &self.state,
            view,
            playground: None,
            env: self.env,
            viewport: size,
            fit_height: false,
            system_reduced_motion: self.system_reduced_motion,
            titlebar_inset: self.titlebar_inset / self.zoom.k,
            chrome: self.titlebar.insets.zoomed(self.zoom.k),
            caption: self.titlebar.caption,
            system_accent: self.system_accent,
            system_contrast: self.system_contrast,
            system_increased_contrast: self.system_increased_contrast,
            system_scroll_bars: self.scroll_bars.state,
            reduced_motion: self.reduced_motion,
            time_scale: self.time_scale,
            zoom: self.zoom.k,
            defaults: self.defaults.clone(),
            titlebar: self.titlebar_set(),
            caret: None,
            caret_read: false,
            // The rects the window last laid out, as its next frame reads them, so a view that
            // measures against them (a header fitting its title) measures here too.
            layout_seen: self.layout_seen.rects.clone(),
            layout_reads: Vec::new(),
            commits: self.commits,
            nodes: &mut nodes,
            committing: false,
            spare_nodes: &mut spare_nodes,
            text: &mut self.text,
            layout: &mut layout,
            node_index: &mut node_index,
            path_ids: &mut path_ids,
            memo: &mut memo,
            next_id: &mut next_id,
            scrolls: &mut scrolls,
            virtuals: &mut virtuals,
            virtual_culled: &mut virtual_culled,
            virtual_inputs: &mut virtual_inputs,
            stats: &mut stats,
            key_warnings: &mut key_warnings,
            focused: None,
            state_unchanged: false,
            pass: Pass::First,
            hold_windows: false,
            windowed: HashMap::default(),
            kept_lists: Vec::new(),
        };
        drop(m.finish(&Tx::default()));
        self.text.prepare_close(pass, PREPARE_BYTES, before, at);
    }
}
