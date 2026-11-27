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

struct PrepareView<'a, S: 'static> {
    state: &'a S,
    view: fn(&S, &mut Cx<S>) -> El,
}

impl<S: 'static> ViewDriver for PrepareView<'_, S> {
    fn states_ref(&self) -> crate::states::StatesRef<'_> {
        crate::states::StatesRef::one(handler_state_ref(self.state))
    }

    fn view(&self, cx: cx::CxCore, _viewport: (f32, f32)) -> (El, cx::CxCore) {
        let mut cx = Cx { core: cx, _s: PhantomData };
        let root = crate::playground::call(self.view, self.state, &mut cx);
        (root, cx.core)
    }
}

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
        let (core, app) = self.split_mut();
        let prepare = PrepareView { state: &*app.state, view };
        core.prepare_text_with(&prepare, size)
    }
}

impl RuntimeCore {
    pub(super) fn prepare_text_with(&mut self, app: &dyn ViewDriver, size: (f32, f32)) -> usize {
        let _warm = crate::frame::stage::StageGuard::enter(crate::frame::stage::FrameStage::TextWarm);
        let before = self.text.layout_keys();
        let (mut nodes, mut spare_nodes, mut layout) = (Vec::new(), Vec::new(), LayoutEngine::new());
        let (mut node_index, mut path_ids, mut memo) = (NodeIndex::default(), HashMap::default(), crate::memo::MemoCache::default());
        let (mut next_id, mut scrolls, mut virtuals) = (0u64, HashMap::default(), HashMap::default());
        let mut virtual_items = virtual_items::VirtualItemsState::default();
        let scroll_requests = scroll::requests::Requests::default();
        let (mut virtual_culled, mut virtual_inputs, mut stats, mut key_warnings) = (HashSet::default(), HashMap::default(), FrameStats::default(), None);
        let m = Measurement {
            app,
            env: self.env,
            viewport: size,
            content_sizing: crate::ContentSizing::Fixed,
            content_limits: (None, None),
            system_reduced_motion: self.host.system_reduced_motion,
            pointer_locked: self.pointer_locked(),
            titlebar_inset: self.host.titlebar_inset / self.host.zoom.k,
            chrome: self.host.titlebar.insets.zoomed(self.host.zoom.k),
            safe_area: self.safe_area(),
            maximized: self.host.maximized,
            full_screen: self.host.full_screen,
            window_backdrop: self.host.window_backdrop,
            window_backdrop_supported: self.host.window_backdrop_supported,
            renderer_kind: self.host.renderer_kind,
            caption: self.host.titlebar.caption,
            system_accent: self.host.system_accent,
            system_contrast: self.host.system_contrast,
            system_increased_contrast: self.host.system_increased_contrast,
            system_text_scale: self.host.system_text_scale,
            system_reduced_transparency: self.host.system_reduced_transparency,
            system_scroll_bars: self.host.scroll_bars.state,
            system_languages: self.host.system_languages.clone(),
            languages_gen: self.host.languages_gen,
            reduced_motion: self.reduced_motion,
            time_scale: self.time_scale,
            zoom: self.host.zoom.k,
            defaults: self.commit.defaults.clone(),
            titlebar: self.titlebar_set(),
            caret: None,
            caret_read: false,
            wm_state: self.host.wm_state,
            wm_state_read: false,
            // The rects the window last laid out, as its next frame reads them, so a view that
            // measures against them (a header fitting its title) measures here too.
            layout_seen: self.commit.layout_seen.rects.clone(),
            layout_reads: Vec::new(),
            commits: self.commit.commits,
            nodes: &mut nodes,
            committing: false,
            spare_nodes: &mut spare_nodes,
            layout: &mut layout,
            node_index: &mut node_index,
            path_ids: &mut path_ids,
            memo: &mut memo,
            next_id: &mut next_id,
            scrolls: &mut scrolls,
            scroll_requests: &scroll_requests,
            virtuals: &mut virtuals,
            virtual_items: &mut virtual_items,
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
        drop(m.finish(&mut self.text, &Tx::default()));
        self.text.keep_prepared(&before, PREPARE_BYTES)
    }
}
