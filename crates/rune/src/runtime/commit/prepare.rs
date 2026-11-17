//! Text laid out ahead (`Tx::prepare_text`): a view the app names, measured as the window would
//! measure it, into the window's layout cache, without mounting anything. Its nodes, layout tree,
//! ids and memo are the pass's own and go when it ends, so nothing draws, hits, animates or is
//! announced, and no handler or live value sees it. What it shaped stays in the cache until a view
//! shows it (then it is found, not shaped) or the cache lets it go: past [`PREPARE_BYTES`] of
//! prepared layouts the oldest go first, and all of them at the window's next rest. One request is
//! held at a time, the latest; it runs in the window's next idle turn after its first frame, and
//! none while input waits.

use super::*;

/// The most prepared layouts no view holds may take in the layout cache, by the cache's own count
/// (`LAYOUT_PER_BYTE`): about one transcript screen.
pub(crate) const PREPARE_BYTES: usize = 1 << 20;

impl<S: 'static> Runtime<S> {
    /// Whether an idle turn has a view to prepare for this window.
    pub(crate) fn text_idle_owed(&self) -> bool {
        Tx::prepare_owed::<S>()
    }

    /// One idle turn's text work: the view asked to be prepared, if any; true while more is owed.
    pub(crate) fn text_idle_turn(&mut self) -> bool {
        if let Some((view, size)) = Tx::take_prepared::<S>() {
            self.prepare_text(view, size.unwrap_or(self.viewport));
        }
        self.text_idle_owed()
    }

    /// Measures `view` at `size` (points) with this window's text system and scratch everything
    /// else, then drops it. Returns the layouts it added.
    pub(crate) fn prepare_text(&mut self, view: fn(&S, &mut Cx<S>) -> El, size: (f32, f32)) -> usize {
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
        self.text.keep_prepared(&before, PREPARE_BYTES)
    }
}
