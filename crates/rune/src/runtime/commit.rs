//! View-to-scene commit orchestration (`Runtime::commit`, `Measurement`, `Measured`).
use super::*;

#[cfg(not(target_arch = "wasm32"))]
mod prepare;
pub(super) mod apply;
pub(super) mod apply_scratch;
mod device;
pub(super) mod flatten;
pub(in crate::runtime) mod inline;
#[cfg(debug_assertions)]
pub(super) mod layout_diagnostics;
pub(super) mod measure;
pub(crate) mod memo_reuse;
pub(crate) mod path;
pub(super) mod prune;
#[cfg(test)]
mod prune_tests;
pub(super) mod relayout;
pub(super) mod remount;
pub(super) mod text_layout;
pub(super) mod watch;
pub(super) mod why;

/// View commit, Taffy layout, memoization, virtual list windows and layout/visibility callback
/// tracking owned by [`RuntimeCore`]. Ordered hot-first under `#[repr(C)]` so per-frame flags
/// (`dirty`, `relayout`, `settle`) sit directly adjacent to [`FrameState`]'s `queue`/`live`.
#[repr(C)]
pub(crate) struct CommitState {
    pub(crate) dirty: bool,
    pub(in crate::runtime) relayout: bool,
    /// Every viewport change since the last commit came from the window's own resize
    /// animation (`step_viewport`), not the user or the system.
    viewport_stepped: bool,
    settle: bool,
    /// True when the commit in flight is for scroll/layout only (`self.state` wasn't mutated).
    state_unchanged: bool,
    /// Why this frame committed, as [`commit_why::CommitWhy`] bits, until the frame log takes them.
    commit_why: u16,
    pub(crate) commits: u64,
    /// Commits whose cause was not purely `CommitWhy::VIRTUAL` (the initial commit counts).
    pub(crate) non_virtual_commits: u64,
    commit_gen: u64,
    /// The memo pass the current commit's first view build was. Memos none of its builds used
    /// go once it applies.
    memo_since: u64,
    /// The clock at the last commit.
    #[cfg(debug_assertions)]
    last_commit_clock: f64,
    /// The last view's `Cx::foreground` and `Cx::focus_ring`, for rows built after it and the
    /// rings painted until the next.
    pub(crate) defaults: cx::ViewDefaults,
    reuse: relayout::Reuse,
    pub(in crate::runtime) layout: LayoutEngine,
    memo: crate::memo::MemoCache,
    apply_scratch: apply_scratch::ApplyScratch,
    pub(in crate::runtime) watch: crate::tooling::perf_warn::Watch,
    /// Opt-in tooling checks. None keeps release commits free of key traversal/allocation.
    key_warnings: Option<Vec<String>>,
    pub(crate) virtuals: HashMap<String, crate::el::virtual_list::VirtualList>,
    pub(crate) virtual_items: virtual_items::VirtualItemsState,
    virtual_culled: HashSet<String>,
    /// Fresh view accessors for focused text fields omitted by virtualization.
    pub(in crate::runtime) virtual_inputs: HashMap<String, Box<InputSpec>>,
    /// Whether an index-built list still fills its overscan, and for how many frames.
    pub(in crate::runtime) virtual_fill: virtual_fill::Fill,
    /// Keys the last view read with `Cx::layout` or `Cx::layout_size`, and what each resolved to.
    pub(in crate::runtime) layout_seen: layout_read::LayoutSeen,
    /// What the first commit's view read (`layout_seed`): the next launch's first view starts from it.
    pub(in crate::runtime) layout_first: Option<crate::LayoutSeed>,
    /// (Rect, display scale bits) last sent to each element's `El::on_layout`.
    layout_fired: HashMap<NodeId, ([f32; 4], u32)>,
    /// (Rect, display scale bits) last sent to each element's `El::on_screen_layout`.
    screen_fired: HashMap<NodeId, ([f32; 4], u32)>,
    /// (Drawn rect, laid-out rect, display scale bits) last sent to each element's `El::on_drawn_layout`.
    drawn_fired: HashMap<NodeId, ([f32; 4], [f32; 4], u32)>,
    /// Whether each element with `El::on_visible` was last on screen.
    visible_fired: HashMap<NodeId, bool>,
    /// Live (non-exiting, non-unmounted) elements whose `El::on_mount` has already fired.
    mounted_seen: HashSet<NodeId>,
    /// Commit generation at which `mounted_seen` was last completely synchronized.
    mounted_gen: u64,
    /// Whether layout feedback may still cause a commit (see `layout_read`).
    feedback: layout_read::Feedback,
}

impl CommitState {
    // Single-caller constructor for `RuntimeCore::new`; inlined so it stays in its caller's
    // codegen unit instead of shifting partitioning and dropping inlining in the paint walk.
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            layout: LayoutEngine::new(),
            memo: Default::default(),
            memo_since: 0,
            apply_scratch: Default::default(),
            commits: 0,
            non_virtual_commits: 0,
            commit_gen: 0,
            dirty: true,
            relayout: false,
            viewport_stepped: false,
            settle: false,
            state_unchanged: false,
            reuse: Default::default(),
            commit_why: 0,
            watch: Default::default(),
            key_warnings: None,
            defaults: Default::default(),
            virtuals: HashMap::default(),
            virtual_items: virtual_items::VirtualItemsState::default(),
            virtual_culled: HashSet::default(),
            virtual_inputs: HashMap::default(),
            virtual_fill: virtual_fill::Fill::default(),
            layout_seen: Default::default(),
            layout_first: None,
            layout_fired: HashMap::default(),
            screen_fired: HashMap::default(),
            drawn_fired: HashMap::default(),
            visible_fired: HashMap::default(),
            mounted_seen: HashSet::default(),
            mounted_gen: 0,
            feedback: Default::default(),
            #[cfg(debug_assertions)]
            last_commit_clock: f64::NEG_INFINITY,
        }
    }

    #[inline]
    pub(in crate::runtime) fn is_viewport_stepped(&self) -> bool {
        self.viewport_stepped
    }

    #[inline]
    pub(in crate::runtime) fn set_viewport_stepped(&mut self, stepped: bool) {
        self.viewport_stepped = stepped;
    }

    #[inline]
    pub(in crate::runtime) fn is_settling(&self) -> bool {
        self.settle
    }

    #[inline]
    pub(in crate::runtime) fn request_settle(&mut self) {
        self.settle = true;
    }

    #[inline]
    pub(in crate::runtime) fn set_state_unchanged(&mut self, unchanged: bool) {
        self.state_unchanged = unchanged;
    }

    #[inline]
    pub(crate) fn take_commit_why(&mut self) -> u16 {
        std::mem::take(&mut self.commit_why)
    }

    #[inline]
    pub(crate) fn clear_commit_why(&mut self) {
        self.commit_why = 0;
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn commit_why(&self) -> u16 {
        self.commit_why
    }

    #[inline]
    pub(in crate::runtime) fn commit_gen(&self) -> u64 {
        self.commit_gen
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn set_commit_gen(&mut self, generation: u64) {
        self.commit_gen = generation;
    }

    pub(in crate::runtime) fn rest(&mut self) {
        fn shrink_map<K: Eq + std::hash::Hash, V, S: std::hash::BuildHasher>(m: &mut std::collections::HashMap<K, V, S>, floor: usize) {
            let cap = (m.len().saturating_mul(2)).max(floor);
            if m.capacity() > cap { m.shrink_to(cap); }
        }
        fn shrink_set<K: Eq + std::hash::Hash, S: std::hash::BuildHasher>(s: &mut std::collections::HashSet<K, S>, floor: usize) {
            let cap = (s.len().saturating_mul(2)).max(floor);
            if s.capacity() > cap { s.shrink_to(cap); }
        }
        self.memo.rest();
        self.apply_scratch.rest();
        shrink_map(&mut self.layout_seen.rects, 32);
        let reads_cap = (self.layout_seen.reads.len().saturating_mul(2)).max(32);
        if self.layout_seen.reads.capacity() > reads_cap { self.layout_seen.reads.shrink_to(reads_cap); }
        shrink_map(&mut self.layout_fired, 32);
        shrink_map(&mut self.screen_fired, 32);
        shrink_map(&mut self.drawn_fired, 32);
        shrink_map(&mut self.visible_fired, 32);
        shrink_set(&mut self.mounted_seen, 64);
        shrink_map(&mut self.virtuals, 16);
        self.virtual_items.rest();
        shrink_set(&mut self.virtual_culled, 16);
        shrink_map(&mut self.virtual_inputs, 16);
        let fit_cap = (self.reuse.above_fit_styles.len().saturating_mul(2)).max(16);
        if self.reuse.above_fit_styles.capacity() > fit_cap {
            self.reuse.above_fit_styles.shrink_to(fit_cap);
        }
    }

    #[cfg(debug_assertions)]
    #[inline]
    pub(in crate::runtime) fn last_commit_clock(&self) -> f64 {
        self.last_commit_clock
    }

    #[inline]
    pub(in crate::runtime) fn set_read_moved(&mut self, moved: bool) {
        self.reuse.read_moved = moved;
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn set_reuse_scale(&mut self, scale: f32) {
        self.reuse.scale = scale;
    }

    #[inline]
    pub(crate) fn clear_memo(&mut self) {
        self.memo.clear();
    }

    #[inline]
    pub(crate) fn evict_memo_prefix(&mut self, prefix: &str) {
        self.memo.evict_prefix(prefix);
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn memo_builds(&self) -> usize {
        self.memo.builds
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn memo_len(&self) -> usize {
        self.memo.len()
    }

    #[inline]
    pub(crate) fn enable_key_warnings(&mut self) {
        self.key_warnings = Some(Vec::new());
    }

    #[inline]
    pub(crate) fn key_warnings(&self) -> Option<&[String]> {
        self.key_warnings.as_deref()
    }

    #[inline]
    pub(in crate::runtime) fn push_key_warning(&mut self, msg: impl FnOnce() -> String) {
        if let Some(warnings) = self.key_warnings.as_mut() {
            warnings.push(msg());
        }
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn virtuals(&self) -> &HashMap<String, crate::el::virtual_list::VirtualList> {
        &self.virtuals
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn virtual_culled(&self) -> &HashSet<String> {
        &self.virtual_culled
    }

    #[inline]
    pub(in crate::runtime) fn take_layout_fired(&mut self, screen: bool) -> HashMap<NodeId, ([f32; 4], u32)> {
        std::mem::take(if screen { &mut self.screen_fired } else { &mut self.layout_fired })
    }

    #[inline]
    pub(in crate::runtime) fn set_layout_fired(&mut self, screen: bool, fired: HashMap<NodeId, ([f32; 4], u32)>) {
        *(if screen { &mut self.screen_fired } else { &mut self.layout_fired }) = fired;
    }

    #[inline]
    pub(in crate::runtime) fn drawn_fired_empty(&self) -> bool {
        self.drawn_fired.is_empty()
    }

    #[inline]
    pub(in crate::runtime) fn take_drawn_fired(&mut self) -> HashMap<NodeId, ([f32; 4], [f32; 4], u32)> {
        std::mem::take(&mut self.drawn_fired)
    }

    #[inline]
    pub(in crate::runtime) fn set_drawn_fired(&mut self, fired: HashMap<NodeId, ([f32; 4], [f32; 4], u32)>) {
        self.drawn_fired = fired;
    }

    #[inline]
    pub(in crate::runtime) fn visible_fired_empty(&self) -> bool {
        self.visible_fired.is_empty()
    }

    #[inline]
    pub(in crate::runtime) fn take_visible_fired(&mut self) -> HashMap<NodeId, bool> {
        std::mem::take(&mut self.visible_fired)
    }

    #[inline]
    pub(in crate::runtime) fn set_visible_fired(&mut self, seen: HashMap<NodeId, bool>) {
        self.visible_fired = seen;
    }

    #[inline]
    pub(in crate::runtime) fn mounted_seen_empty(&self) -> bool {
        self.mounted_seen.is_empty()
    }

    #[inline]
    pub(in crate::runtime) fn is_mount_seen(&self, id: NodeId) -> bool {
        self.mounted_seen.contains(&id)
    }

    #[inline]
    pub(in crate::runtime) fn take_mounted_seen(&mut self) -> HashSet<NodeId> {
        std::mem::take(&mut self.mounted_seen)
    }

    #[inline]
    pub(in crate::runtime) fn set_mounted_seen(&mut self, seen: HashSet<NodeId>) {
        self.mounted_seen = seen;
    }

    #[inline]
    pub(in crate::runtime) fn mounted_gen(&self) -> u64 {
        self.mounted_gen
    }

    #[inline]
    pub(in crate::runtime) fn set_mounted_gen(&mut self, generation: u64) {
        self.mounted_gen = generation;
    }

    #[inline]
    pub(in crate::runtime) fn feedback_open(&self) -> bool {
        self.feedback.open()
    }

    #[inline]
    pub(in crate::runtime) fn step_feedback(&mut self, event: layout_read::FeedbackEvent) {
        self.feedback = self.feedback.step(event);
    }

    /// Restores any nodes moved out of `old_nodes` and recycles `measured`'s buffers before a
    /// commit pass re-measures.
    pub(in crate::runtime) fn discard_measured(
        &mut self,
        old_nodes: &mut [Node],
        spare_nodes: &mut Vec<Node>,
        mut measured: Measured,
    ) {
        memo_reuse::restore_moved_nodes(old_nodes, &mut measured.nodes, &measured.memo_reused, &measured.taken_paths);
        measured.nodes.clear();
        *spare_nodes = measured.nodes;
        if let Some(mut pending) = measured.memo_flattened {
            pending.clear();
            self.memo.pending_scratch = pending;
        }
        measured.memo_reused.clear();
        self.memo.reused_scratch = measured.memo_reused;
        measured.taken_paths.clear();
        self.memo.taken_paths_scratch = measured.taken_paths;
        self.virtual_items.clear_staged();
        self.layout = measured.layout;
    }
}

/// Which measurement pass a commit is on. The first pass trusts the last commit's geometry; a
/// retry measures again because a virtual window fell short; past eight retries the pass is
/// forced, mounting every row so a degenerate transcript can't loop forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pass {
    First,
    Retry { n: u8 },
    Forced,
}

impl Pass {
    /// The pass number `finish_from` counts to.
    pub(super) fn of(passes: usize) -> Self {
        match passes {
            0 | 1 => Self::First,
            n if n > 8 => Self::Forced,
            n => Self::Retry { n: u8::try_from(n).unwrap_or(u8::MAX) },
        }
    }

    /// The last commit's measured viewport still stands; a retry keeps the one the last pass found.
    pub(super) fn trusts_estimates(self) -> bool {
        matches!(self, Self::First)
    }

    /// Newly mounted rows may spread over the next frames. A retry builds what the viewport
    /// needs now, and keeps the rows a live-only check already has mounted.
    pub(super) fn spreads_rows(self) -> bool {
        matches!(self, Self::First)
    }

    /// A newly selected row may be revealed. A retry would only fight the estimate it is fixing.
    pub(super) fn reveals(self) -> bool {
        matches!(self, Self::First)
    }

    /// Every row mounts, whatever the window says.
    pub(super) fn forced(self) -> bool {
        matches!(self, Self::Forced)
    }
}

/// Measurement capabilities: text/layout caches and virtual geometry only. In particular this
/// type has no Scene, growth tracks, reveal tracks, draw instances, or mutable app state.
pub(super) struct Measurement<'a> {
    pub(super) app: &'a dyn ViewDriver,
    pub(super) env: Env,
    pub(super) viewport: (f32, f32),
    pub(super) content_sizing: crate::ContentSizing,
    pub(super) content_limits: crate::overlay::sizing::SizeLimits,
    pub(super) system_reduced_motion: bool,
    pub(super) pointer_locked: bool,
    pub(super) titlebar_inset: f32,
    pub(super) chrome: crate::ChromeInsets,
    pub(super) safe_area: [f32; 4],
    pub(super) maximized: bool,
    pub(super) full_screen: bool,
    pub(super) window_backdrop: crate::WindowBackdrop,
    pub(super) window_backdrop_supported: u8,
    pub(super) renderer_kind: crate::RendererKind,
    /// Caption buttons drawn over the view's root.
    pub(super) caption: Option<crate::titlebar::Caption>,
    pub(super) system_accent: crate::SystemAccent,
    pub(super) system_contrast: Option<crate::SystemContrast>,
    pub(super) system_increased_contrast: bool,
    pub(super) system_text_scale: f32,
    pub(super) system_reduced_transparency: bool,
    pub(super) system_scroll_bars: crate::platform::scroll_bars::SystemScrollBars,
    pub(super) system_languages: Option<Rc<[String]>>,
    pub(super) languages_gen: u64,
    pub(super) reduced_motion: bool,
    pub(super) time_scale: f32,
    /// The window's zoom, then the one the view asked for (`Cx::zoom`) once it returns.
    pub(super) zoom: f32,
    /// `Cx::foreground` and `Cx::focus_ring`: the last view's, then this one's once it returns.
    pub(super) defaults: cx::ViewDefaults,
    /// `Cx::set_titlebar` and `Cx::titlebar_height`, as the view sets them.
    pub(super) titlebar: crate::titlebar::TitlebarSet,
    pub(super) commits: u64,
    /// The last commit's nodes. A pass that commits drops the text layouts of the ones it keeps.
    pub(super) nodes: &'a mut [Node],
    pub(super) committing: bool,
    pub(super) spare_nodes: &'a mut Vec<Node>,
    pub(super) layout: &'a mut LayoutEngine,
    pub(super) node_index: &'a mut NodeIndex,
    pub(super) path_ids: &'a mut HashMap<String, NodeId>,
    pub(super) memo: &'a mut crate::memo::MemoCache,
    pub(super) next_id: &'a mut u64,
    pub(super) scrolls: &'a mut HashMap<NodeId, ScrollState>,
    pub(super) scroll_requests: &'a scroll::requests::Requests,
    pub(super) virtuals: &'a mut HashMap<String, crate::el::virtual_list::VirtualList>,
    pub(super) virtual_items: &'a mut virtual_items::VirtualItemsState,
    pub(super) virtual_culled: &'a mut HashSet<String>,
    pub(super) virtual_inputs: &'a mut HashMap<String, Box<InputSpec>>,
    pub(super) stats: &'a mut FrameStats,
    pub(super) key_warnings: &'a mut Option<Vec<String>>,
    /// The focused element's path, so an index-built row holding it is still built.
    pub(super) focused: Option<&'a str>,
    /// True when `self.state` was not mutated since the previous commit.
    pub(super) state_unchanged: bool,
    /// Which pass this is: the first, a retry after estimates left part of a viewport blank, or
    /// the forced full measurement that ends a runaway retry.
    pub(super) pass: Pass,
    /// This commit is a live-only check's: virtual lists keep the rows they mount now.
    pub(super) hold_windows: bool,
    /// Windows picked for lists in a memo that `keeps_windows` then expanded, by list path.
    pub(super) windowed: HashMap<String, super::measure::Window>,
    /// Lists kept with their memo this pass, and which of their mounted rows mount quietly.
    pub(super) kept_lists: Vec<(String, Vec<bool>)>,
    pub(super) caret: Option<(String, [f32; 4])>,
    pub(super) caret_read: bool,
    pub(super) wm_state: crate::WindowManagerState,
    pub(super) wm_state_read: bool,
    pub(super) primary_pointer: cx::PrimaryPointer,
    pub(super) primary_pointer_read: bool,
    pub(super) layout_seen: layout_read::LayoutRects,
    pub(super) layout_reads: layout_read::LayoutReads,
}

/// Owned result of completed measurement. Scene mutation consumes this once; retries never
/// return it. Keeping this separate from El makes unmeasured views invalid commit inputs.
pub(super) struct Measured {
    pub(super) layout: LayoutEngine,
    pub(super) nodes: Vec<Node>,
    pub(super) reduced_motion: bool,
    pub(super) time_scale: f32,
    pub(super) zoom: f32,
    pub(super) defaults: cx::ViewDefaults,
    pub(super) titlebar: crate::titlebar::TitlebarSet,
    pub(super) caret_read: bool,
    pub(super) wm_state_read: Option<bool>,
    pub(super) primary_pointer_read: Option<bool>,
    pub(super) layout_reads: layout_read::LayoutReads,
    pub(super) memo_flattened: Option<Vec<memo_reuse::PendingFlat>>,
    pub(super) memo_reused: Vec<memo_reuse::ReusedSpan>,
    pub(super) taken_paths: Vec<(u32, u32)>,
    /// The selection as the measure began, before reused `memo` subtrees left the last
    /// commit's nodes.
    pub(super) selection: Vec<layout_read::SelectedRange>,
    /// Authored styles of `anchored_fit` overlays whose max-size `fit_anchored` clamped in this
    /// pass, so a later `Relayout` restores them before re-fitting at the new viewport.
    pub(super) above_fit_styles: Vec<(usize, CompactStyle)>,
}

impl RuntimeCore {
    pub(super) fn commit(&mut self, app: &mut dyn AppDriver, tx: &Tx) {
        let _commit_time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::Commit);
        let _first_view = crate::tooling::perf_warn::launch::FirstView::start(self.commit.commits == 0);
        let budget = crate::tooling::perf_warn::budget::Commit::start();
        self.apply_playground(app);
        let _history = crate::memo::snapshot::enter();
        COMMITS.set(COMMITS.get() + 1);
        #[cfg(debug_assertions)]
        { self.commit.last_commit_clock = self.clock; }
        self.commit.memo_since = self.commit.memo.next_pass();
        for r in self.scroll.requests.reveals() {
            if r.at_end && let Some(&id) = self.tree.path_ids.get(&r.viewport)
                && let Some(scroll) = self.scroll.scrolls.get_mut(&id)
            {
                scroll.pin();
            }
        }
        self.note_drawn_rows();
        let mut measured = self.measure_commit(app, tx);
        // A new zoom lays the view out again at the scale and size it gives, in this commit.
        if self.apply_zoom(measured.zoom) {
            self.discard_measured(measured);
            measured = self.measure_commit(app, tx);
        }
        self.kbd.focus_before_commit = self.kbd.focus.owned();
        // Settle layout reads first so a field that mounts once a rect is known is in `nodes`.
        measured = self.settle_layout_reads(app, measured, tx);
        for _ in 0..4 {
            if !self.sync_requested_focus(app, &mut measured.nodes) { break; }
            self.discard_measured(measured);
            measured = self.measure(app, tx);
            measured = self.settle_layout_reads(app, measured, tx);
            if self.kbd.focus_request.is_none() { break; }
        }
        if measured.caret_read && self.caret_moved(Some(&measured.nodes)) {
            self.discard_measured(measured);
            measured = self.measure(app, tx);
            measured = self.settle_layout_reads(app, measured, tx);
        }
        let mut focus_follow_up = self.apply(app, measured, tx);
        for _ in 0..2 {
            if !focus_follow_up { break; }
            let follow_tx = Tx { commit: Commit::Instant, ..Tx::default() };
            let mut next_measured = self.measure_commit(app, &follow_tx);
            next_measured = self.settle_layout_reads(app, next_measured, &follow_tx);
            for _ in 0..4 {
                if !self.sync_requested_focus(app, &mut next_measured.nodes) { break; }
                self.discard_measured(next_measured);
                next_measured = self.measure(app, &follow_tx);
                next_measured = self.settle_layout_reads(app, next_measured, &follow_tx);
                if self.kbd.focus_request.is_none() { break; }
            }
            focus_follow_up = self.apply(app, next_measured, &follow_tx);
        }
        self.resolve_caret_follow();
        self.scene.keep_flights(false);
        self.commit.memo.keep_since(self.commit.memo_since);
        self.track_focused_rows();
        #[cfg(debug_assertions)]
        if COMMITS.get() % 16 == 1 { self.a11y_warn_nameless(); }
        self.prune_text_selection();
        budget.end(self.commit.watch.cause);
    }

    /// A frame's commit for `why` (`CommitWhy` bits).
    pub(in crate::runtime) fn commit_for(&mut self, app: &mut dyn AppDriver, tx: &Tx, why: u16) {
        self.commit.begin_watch(why);
        self.commit(app, tx);
        self.commit.end_watch();
    }

    /// Remounts virtual windows after a scroll request moved a viewport, without re-running
    /// `view` when nothing the view reads has changed since the last commit.
    pub(in crate::runtime) fn commit_virtual_scroll(&mut self, app: &mut dyn AppDriver, tx: &Tx) {
        self.commit.set_state_unchanged(!self.commit.dirty && !self.commit.relayout && self.commit.commits > 0);
        self.commit_for(app, tx, CommitWhy::VIRTUAL);
    }

    /// Restores any nodes moved out of `self.nodes` and recycles `measured`'s buffers before a
    /// commit pass re-measures.
    #[inline]
    pub(super) fn discard_measured(&mut self, measured: Measured) {
        self.commit.discard_measured(&mut self.tree.nodes, &mut self.tree.spare_nodes, measured);
    }
}

/// A horizontal scroll viewport's content without a width of its own, or one filling the
/// viewport (`w_full`), takes its natural one, unwrapped, and at least the viewport's (CSS
/// `width: max-content; min-width: 100%`). Any other width stays as set, narrower than the
/// viewport too. The content never shrinks. The viewport lays it out as a row, where taffy
/// measures a max-content main size unwrapped (a column measures its height wrapped at the
/// viewport's width). With vertical scrolling too it keeps its natural height instead of
/// stretching to the viewport's.
pub(super) fn unwrapped_width(style: &mut CompactStyle, vertical: bool) {
    let w = style.size().width;
    if w.is_auto() || w == Dimension::percent(1.0) {
        style.set_width(Dimension::max_content());
        if style.min_size().width.is_auto() { style.set_min_width(percent(1.0)); }
    }
    style.set_flex_shrink(0.0);
    if vertical && style.align_self().is_none() { style.set_align_self(Some(AlignSelf::FLEX_START)); }
}
