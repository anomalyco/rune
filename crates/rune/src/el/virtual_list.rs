//! Virtual row bookkeeping. Only geometry outside the mounted window is estimated.

use std::cell::{Cell, Ref, RefCell};
use std::ops::Range;

use rune_taffy::CompactStyle;
use taffy::style::{Display, FlexDirection, FlexWrap, LengthPercentage, LengthPercentageAuto, Position};

use crate::El;
use crate::runtime::HashSet;

mod places;
use places::Places;

#[derive(Debug, Default)]
pub(crate) struct VirtualList {
    /// Row keys in order; replace them with [`VirtualList::set_keys`].
    pub keys: Vec<String>,
    places: Places,
    /// Each row's measured height, once it has been laid out, in `keys` order.
    measured: Vec<Option<f32>>,
    /// Rows excluded from scroll anchoring (`El::no_scroll_anchor`), in `keys` order; stays empty
    /// (zero allocations) until a row opts out.
    row_no_anchor: Vec<bool>,
    /// The commit each row in `keys` first appeared in.
    born: Vec<u64>,
    /// `keys` changed since the frame began ([`VirtualList::frame_begins`]).
    keys_fresh: bool,
    /// The last change to `keys` kept a row the reader could see.
    held: bool,
    /// Rows mounted with their insertion transition and not yet drawn; the runtime lands the
    /// ones that drew nothing (`runtime/unseen_presence.rs`).
    pub entering: HashSet<String>,
    /// Each row's height, measured or estimated, in `keys` order.
    row_heights: Vec<f32>,
    /// Running totals of `row_heights` (one longer), rebuilt when one changes. Every frame reads
    /// them, so a long list doesn't hash every key to find its window.
    sums: RefCell<Vec<f32>>,
    sums_stale: Cell<bool>,
    pub range: Range<usize>,
    pub before: f32,
    pub after: f32,
    estimate: f32,
    /// Rows are built by index (`El::virtual_rows`), so a commit costs only the rows it mounts.
    /// Such a list moves its window in small steps and spreads rows new to it over frames.
    pub indexed: bool,
    /// Opt-in `(overscan, edge)` in viewports (`El::virtual_overscan`), already clamped;
    /// `None` keeps `OVERSCAN` and `INDEXED_EDGE` (or `EAGER_EDGE` for `virtualize`).
    overscan_override: Option<(f32, f32)>,
    /// The rows the last applied commit mounted. Every measurement of a commit compares with
    /// these, not with the pass before it.
    pub shown: HashSet<String>,
    /// The rows drawn inside the viewport on the last frame (`Runtime::note_drawn_rows`).
    pub drawn: Vec<String>,
    /// The row holding the focused element, kept while that row is scrolled out.
    pub focused_row: Option<String>,
    /// Offscreen row state for text selection: the rows its anchor and focus pin, their texts
    /// and their row source, plus the kept rows a stored list builds from.
    pub selection: crate::runtime::selectable::virtual_selection::PinnedSelection,
    /// The selected row on the last commit of an `El::reveal_selected(true)` stack.
    pub selected_row: Option<String>,
    pub reveal_selected: Option<String>,
    pub scroll_path: String,
    pub viewport: f32,
    pub top: f32,
    /// The list lies in a row of another virtual list with the same scroll viewport (an open
    /// section of a long list, itself long): it windows against that viewport where the row
    /// places it ([`VirtualList::top`]), and only that viewport, not this list, is pinned to its
    /// end ([`VirtualList::locate`]).
    pub nested: bool,
    /// How far the viewport's content runs on under a nested list, as last laid out: where its
    /// window stands while the viewport is pinned to its end.
    pub tail: f32,
    /// A nested list wholly above the viewport's top holds no reader, so it anchors nothing:
    /// its rows changing height move the rows under it, which the list around it anchors.
    passed: bool,
    pub pad_top: f32,
    pub pad_bottom: f32,
    gap: f32,
    /// `El::no_scroll_anchor` on the virtual list or its scroll viewport: disables anchoring.
    pub no_anchor: bool,
    pub anchor: Option<(String, f32)>,
    anchor_offset: Option<f32>,
    /// The `El::virtual_rows_version` that `keys` came with.
    pub keys_version: Option<u64>,
    /// The visible row range last sent to `El::on_visible_rows`.
    pub reported_visible: Option<Range<usize>>,
    /// What the last applied commit mounted, for a memo holding this list to keep its nodes.
    pub mounted: Option<Mounted>,
    /// What the last pass that built the rows read building them.
    pub row_reads: crate::memo::Reads,
    /// Debug-only: this list took the `virtual_rows_append_only` fast path since its last full
    /// rekey, so newly mounted old rows check their key against `RowSource::key`.
    #[cfg(debug_assertions)]
    pub append_only_fast: bool,
    /// Debug-only: call site of `El::virtual_rows_append_only`.
    #[cfg(debug_assertions)]
    pub append_source: Option<&'static std::panic::Location<'static>>,
}

/// The rows a virtual list mounted and the padding standing in for the rest.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Mounted {
    range: Range<usize>,
    before: f32,
    after: f32,
    pad_top: f32,
    pad_bottom: f32,
}

impl Mounted {
    pub(crate) fn of(v: &VirtualList) -> Self {
        Mounted { range: v.range.clone(), before: v.before, after: v.after, pad_top: v.pad_top, pad_bottom: v.pad_bottom }
    }
}

/// The most an element can be tall before layout, so a list's first showing, which has no
/// measured viewport yet, mounts what its viewport can show rather than the window's height.
/// Its own height in points wins; otherwise `avail`, what its parent leaves it. An estimate
/// short of the laid-out height only costs a retry pass, which never presents a blank.
pub(crate) fn height_bound(style: &CompactStyle, avail: f32) -> f32 {
    let h = style.size().height.into_option().unwrap_or(avail);
    let h = length(style.max_size().height).map_or(h, |m| h.min(m));
    length(style.min_size().height).map_or(h, |m| h.max(m))
}

/// A column flex container's vertical gap in points; zero for non-column containers.
pub(crate) fn gap_y(style: &CompactStyle) -> f32 {
    let column = style.display() == Display::Flex && style.flex_wrap() == FlexWrap::NoWrap
        && matches!(style.flex_direction(), FlexDirection::Column | FlexDirection::ColumnReverse);
    if !column { return 0.0; }
    match style.gap().height.expand() {
        taffy::style::ExpandedLengthPercentage::Length(x) => x,
        _ => 0.0,
    }
}

/// The most each child of an element whose bound is `bound` can be tall: what padding leaves
/// and, in a column, what siblings of fixed height and gaps don't take. A scroll viewport's
/// content is unbounded, as is an out-of-flow child not pinned to both its parent's top and
/// bottom.
pub(crate) fn child_bounds(style: &CompactStyle, scrolls: bool, bound: f32, children: &[El]) -> ChildBounds {
    if scrolls { return ChildBounds { outer: f32::INFINITY, inner: f32::INFINITY, fixed: 0.0, column: false } }
    let points = |p: LengthPercentage| match p.expand() {
        taffy::style::ExpandedLengthPercentage::Length(x) => x,
        _ => 0.0,
    };
    let pad = style.padding();
    let inner = bound - points(pad.top) - points(pad.bottom);
    let column = style.display() == Display::Flex && style.flex_wrap() == FlexWrap::NoWrap
        && matches!(style.flex_direction(), FlexDirection::Column | FlexDirection::ColumnReverse);
    if !column || children.len() < 2 { return ChildBounds { outer: bound, inner, fixed: 0.0, column: false } }
    let gaps = points(style.gap().height) * children.iter().filter(|c| in_flow(&c.style)).count().saturating_sub(1) as f32;
    ChildBounds { outer: bound, inner: inner - gaps, fixed: children.iter().map(|c| fixed_height(&c.style)).sum(), column }
}

pub(crate) struct ChildBounds {
    /// The parent's bound: an absolute child's insets are from its padding box.
    outer: f32,
    inner: f32,
    fixed: f32,
    column: bool,
}

impl ChildBounds {
    pub fn of(&self, child: &El) -> f32 {
        let style = &child.style;
        let bound = if style.position() == Position::Absolute {
            let inset = style.inset();
            match (length(inset.top), length(inset.bottom)) {
                (Some(top), Some(bottom)) => self.outer - top - bottom,
                _ => f32::INFINITY,
            }
        } else if self.column && in_flow(style) {
            self.inner - (self.fixed - fixed_height(style))
        } else {
            self.inner
        };
        bound.max(0.0)
    }
}

fn length(d: LengthPercentageAuto) -> Option<f32> {
    match d.expand() {
        taffy::style::ExpandedLengthPercentageAuto::Length(x) => Some(x),
        _ => None,
    }
}

fn in_flow(style: &CompactStyle) -> bool {
    style.display() != Display::None && style.position() != Position::Absolute
}

/// The height a column's child takes whatever the others need: its own height when it can't
/// shrink, and its minimum.
fn fixed_height(style: &CompactStyle) -> f32 {
    if !in_flow(style) { return 0.0 }
    let min = length(style.min_size().height).unwrap_or(0.0);
    if style.flex_shrink() > 0.0 { return min }
    let h = style.size().height.into_option().unwrap_or(0.0);
    length(style.max_size().height).map_or(h, |m| h.min(m)).max(min)
}

/// How far ahead a virtual list keeps rows mounted by default, in viewports either side.
const OVERSCAN: f32 = 2.0;
/// Maximum opt-in overscan (`El::virtual_overscan`), in viewports either side.
pub(crate) const MAX_OVERSCAN: f32 = 8.0;
/// An index-built list moves its window once the viewport comes this close to its edge.
const INDEXED_EDGE: f32 = 1.75;
/// An eager `El::virtualize` list moves its window once the viewport comes this close to its edge.
const EAGER_EDGE: f32 = 0.5;
/// How much of an index-built list's overscan, in viewports per side, may be new in one commit.
/// Rows past it mount over the next frames, so a jump or a prepended page doesn't shape it all
/// in one frame.
const INDEXED_BUDGET: f32 = 0.5;
/// The fewest rows a list pinned to its end keeps mounted above the end, whatever its overscan.
const PINNED_ROWS: usize = 32;

/// Clamps `(overscan, edge)` for `El::virtual_overscan`: returns `None` when either argument is
/// non-finite, clamps `overscan` to `0.0..=MAX_OVERSCAN`, and keeps `edge` strictly below
/// `overscan` when `overscan > 0.0` (or `0.0` when `overscan == 0.0`) so `needs_window` cannot
/// oscillate on consecutive frames.
pub(crate) fn clamp_virtual_overscan(overscan: f32, edge: f32) -> Option<(f32, f32)> {
    if !overscan.is_finite() || !edge.is_finite() {
        return None;
    }
    let overscan = if overscan <= 0.0 { 0.0 } else { overscan.min(MAX_OVERSCAN) };
    let edge = if overscan == 0.0 {
        0.0
    } else {
        let max_edge = (overscan - 0.05).min(overscan * 0.95).max(0.0);
        if edge <= 0.0 { 0.0 } else { edge.min(max_edge) }
    };
    Some((overscan, edge))
}

impl VirtualList {
    pub fn height(&self, key: &str) -> f32 {
        self.index_of(key).and_then(|i| self.measured[i]).unwrap_or(self.estimate)
    }

    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.places.get(&self.keys, key)
    }

    /// Whether row `i`, mounting in `commit`, skips its insertion transition. A row scrolled into
    /// view was there all along, so it does. One new to the rows enters as an insertion, and so
    /// does one that a change to the rows this frame (a filter, a sort) mounts: the rows it
    /// replaces leave with their exits. Rows mounting with their transition are noted in
    /// `entering`, and the runtime lands the ones that drew out of view, so only what shows fades.
    pub fn mounts_quietly(&mut self, i: usize, commit: u64) -> bool {
        let quiet = self.born[i] < commit && !(self.keys_fresh && !self.shown.contains(&self.keys[i]));
        if !quiet { self.entering.insert(self.keys[i].clone()); }
        quiet
    }

    /// The rows changed this frame and none the reader could see stayed (a filter): the rows
    /// that leave hold no place, so what comes in shows where they stood.
    pub fn replaced_view(&self) -> bool {
        self.keys_fresh && !self.held
    }

    /// A new frame: what changed the keys before it has been drawn. Within a frame a scroll the
    /// change asks for (`Tx::scroll_into_view`) commits again, and the rows it brings in are
    /// still the change's.
    pub fn frame_begins(&mut self) {
        self.keys_fresh = false;
    }

    /// Whether the rows are exactly `keys`, in order.
    pub fn same_keys<'a>(&self, keys: impl ExactSizeIterator<Item = Option<&'a str>>) -> bool {
        keys.len() == self.keys.len() && keys.zip(&self.keys).all(|(a, b)| a == Some(b.as_str()))
    }

    /// Replaces the rows. Keys that stay keep their measured height and birth; new ones are born
    /// at `commit`. Returns the first row visible at `offset` (when given) that stays, with its
    /// top before the change, to anchor the reader to: none when no row in view stays.
    #[cfg(test)]
    pub fn set_keys(&mut self, keys: Vec<String>, commit: u64, offset: Option<f32>) -> Option<(String, f32)> {
        let (anchor, renamed) = self.set_keys_deduping(keys, commit, offset, false);
        assert!(renamed.is_empty(), "virtual row keys must be unique");
        anchor
    }

    /// Replaces the rows while deduplicating repeated keys in place (`key#dup1`, `key#dup2`, ...)
    /// and building `Places` in a single pass. Skips empty-list probes on initial load, tracks
    /// order equality and reorder membership inline, and bounds anchor lookup to the viewport.
    pub(crate) fn set_keys_deduping(
        &mut self,
        mut keys: Vec<String>,
        commit: u64,
        offset: Option<f32>,
        skip_anchor_on_reorder: bool,
    ) -> (Option<(String, f32)>, Vec<(usize, String)>) {
        let n = keys.len();
        if self.keys.is_empty() {
            let (places, renamed) = Places::from_keys_deduping(&mut keys);
            if n > 0 { self.keys_fresh = true; self.held = false; }
            self.finish_rekey(keys, places, vec![None; n], Vec::new(), vec![commit; n], vec![self.estimate; n]);
            return (None, renamed);
        }
        let mut places = Places::with_capacity(n);
        let track_no_anchor = !self.row_no_anchor.is_empty();
        let (mut born, mut heights, mut measured, mut row_no_anchor) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            if track_no_anchor { Vec::with_capacity(n) } else { Vec::new() },
        );
        let (mut all_existed, mut same_order, mut any_stayed) = (n == self.keys.len(), n == self.keys.len(), false);
        let mut dups = Vec::new();
        for (idx, key) in keys.iter().enumerate() {
            let hash = Places::hash(key);
            if !places.insert_or_dup(&keys, key, hash, idx) { dups.push(idx); continue; }
            if !dups.is_empty() { continue; }
            self.carry_row(idx, self.places.get_hashed(&self.keys, key, hash), commit, &mut born, &mut heights, &mut measured, track_no_anchor.then_some(&mut row_no_anchor), &mut all_existed, &mut same_order, &mut any_stayed);
        }
        let renamed = if dups.is_empty() {
            Vec::new()
        } else {
            let renamed = places.finish_deduping(&mut keys, &dups, Places::hash);
            born.clear(); heights.clear(); measured.clear(); row_no_anchor.clear();
            all_existed = n == self.keys.len(); same_order = all_existed; any_stayed = false;
            for (idx, key) in keys.iter().enumerate() {
                self.carry_row(idx, self.index_of(key), commit, &mut born, &mut heights, &mut measured, track_no_anchor.then_some(&mut row_no_anchor), &mut all_existed, &mut same_order, &mut any_stayed);
            }
            renamed
        };
        if !row_no_anchor.iter().any(|&v| v) {
            row_no_anchor = Vec::new();
        }
        let effective_offset = (!(skip_anchor_on_reorder && all_existed)).then_some(offset).flatten();
        // A change that keeps none of the rows in view (a filter) leaves nothing to hold.
        let anchor = if any_stayed && !self.no_anchor {
            effective_offset.and_then(|o| self.first_visible_in_viewport(o, |k| places.get(&keys, k).is_some()))
        } else { None };
        if !same_order {
            self.keys_fresh = true;
            self.held = if let Some(o) = effective_offset {
                anchor.is_some() || (any_stayed && self.any_visible_in_viewport(o, |k| places.get(&keys, k).is_some()))
            } else {
                any_stayed
            };
        }
        self.finish_rekey(keys, places, measured, row_no_anchor, born, heights);
        (anchor, renamed)
    }

    #[allow(clippy::too_many_arguments)]
    fn carry_row(
        &self, idx: usize, old: Option<usize>, commit: u64,
        born: &mut Vec<u64>, heights: &mut Vec<f32>, measured: &mut Vec<Option<f32>>, row_no_anchor: Option<&mut Vec<bool>>,
        all_existed: &mut bool, same_order: &mut bool, any_stayed: &mut bool,
    ) {
        match old {
            Some(i) => {
                *any_stayed = true; *same_order &= i == idx;
                let height = self.measured[i];
                born.push(self.born[i]); heights.push(height.unwrap_or(self.estimate)); measured.push(height);
                if let Some(flags) = row_no_anchor { flags.push(self.is_row_no_anchor(i)); }
            }
            None => {
                *all_existed = false; *same_order = false;
                born.push(commit); heights.push(self.estimate); measured.push(None);
                if let Some(flags) = row_no_anchor { flags.push(false); }
            }
        }
    }

    fn finish_rekey(&mut self, keys: Vec<String>, places: Places, measured: Vec<Option<f32>>, row_no_anchor: Vec<bool>, born: Vec<u64>, heights: Vec<f32>) {
        self.places = places; self.measured = measured; self.row_no_anchor = row_no_anchor; self.born = born;
        self.keys = keys; self.row_heights = heights;
        self.sums_stale.set(true); self.reported_visible = None;
        #[cfg(debug_assertions)]
        { self.append_only_fast = false; }
    }

    /// Attempts the `El::virtual_rows_append_only` fast path when `count > self.keys.len()` and
    /// `self.keys` is non-empty: checks the existing prefix with [`Self::mismatch_before_append`]
    /// (warning once in debug builds on a broken promise), reads only `self.keys.len()..count`,
    /// and delegates to [`Self::append_keys`]. Returns `None` to fall back to the full path.
    pub(crate) fn try_append_rows<'a>(
        &mut self,
        path: &str,
        count: usize,
        commit: u64,
        offset: Option<f32>,
        mut key_at: impl FnMut(usize) -> std::borrow::Cow<'a, str>,
    ) -> Option<Option<(String, f32)>> {
        if self.keys.is_empty() || count <= self.keys.len() {
            return None;
        }
        if let Some((i, got)) = self.mismatch_before_append(&mut key_at) {
            #[cfg(debug_assertions)]
            crate::tooling::perf_warn::virtual_rows_append_broken(path, self.append_source, i, &self.keys[i], &got);
            let _ = (path, i, got);
            return None;
        }
        let new = (self.keys.len()..count).map(|i| key_at(i).into_owned()).collect();
        self.append_keys(new, commit, offset)
    }

    /// Checks whether the existing rows still match `key_at` before taking the append-only fast
    /// path. Always probes the start, midpoint and end of `self.keys` (at most 3 calls, O(1));
    /// debug builds also check the currently mounted window (`self.range`). Returns the first
    /// mismatched `(index, actual_key)`, or `None` when every checked row matches.
    pub(crate) fn mismatch_before_append<'a>(
        &self,
        mut key_at: impl FnMut(usize) -> std::borrow::Cow<'a, str>,
    ) -> Option<(usize, String)> {
        let len = self.keys.len();
        if len == 0 {
            return None;
        }
        let mut prev = usize::MAX;
        for i in [0, len / 2, len - 1] {
            if i == prev { continue; }
            prev = i;
            let got = key_at(i);
            if got.as_ref() != self.keys[i] {
                return Some((i, got.into_owned()));
            }
        }
        #[cfg(debug_assertions)]
        {
            let start = self.range.start.min(len);
            let end = self.range.end.min(len);
            for i in start..end {
                if i == 0 || i == len / 2 || i == len - 1 { continue; }
                let got = key_at(i);
                if got.as_ref() != self.keys[i] {
                    return Some((i, got.into_owned()));
                }
            }
        }
        None
    }

    /// Appends `new` rows after the current ones in O(`new.len()`), keeping every existing row's
    /// index, measured height and birth commit. Returns `None`, changing nothing, when any new
    /// key duplicates an existing row or another new key (the caller then falls back to
    /// [`Self::set_keys`]). Otherwise returns `Some(anchor)`, matching [`Self::set_keys`].
    pub fn append_keys(
        &mut self,
        new: Vec<String>,
        commit: u64,
        offset: Option<f32>,
    ) -> Option<Option<(String, f32)>> {
        if new.is_empty() {
            return Some(
                offset
                    .and_then(|o| self.first_visible(o, |_| true))
                    .filter(|(_, top)| offset.is_some_and(|o| *top < o + self.viewport)),
            );
        }
        if new.len() == 1 {
            if self.index_of(&new[0]).is_some() {
                return None;
            }
        } else {
            let mut seen = HashSet::with_capacity_and_hasher(new.len(), Default::default());
            for k in &new {
                if self.index_of(k).is_some() || !seen.insert(k.as_str()) {
                    return None;
                }
            }
        }
        let anchor = offset
            .and_then(|o| self.first_visible(o, |_| true))
            .filter(|(_, top)| offset.is_some_and(|o| *top < o + self.viewport));
        let held = offset.is_none_or(|o| anchor.is_some() || self.any_visible_in_viewport(o, |_| true));
        let sums_fresh =
            !self.sums_stale.get() && self.sums.borrow().len() == self.row_heights.len() + 1;
        let added = new.len();
        let track_no_anchor = !self.row_no_anchor.is_empty();
        self.places.reserve(added);
        self.keys.reserve(added);
        self.measured.reserve(added);
        if track_no_anchor { self.row_no_anchor.reserve(added); }
        self.born.reserve(added);
        self.row_heights.reserve(added);
        if sums_fresh {
            let mut sums = self.sums.borrow_mut();
            sums.reserve(added);
            let mut y = sums.last().copied().unwrap_or(0.0);
            for _ in 0..added {
                y += self.estimate + self.gap;
                sums.push(y);
            }
        } else {
            self.sums_stale.set(true);
        }
        for key in new {
            let index = self.keys.len();
            self.places.insert(&self.keys, &key, index);
            self.keys.push(key);
            self.measured.push(None);
            if track_no_anchor { self.row_no_anchor.push(false); }
            self.born.push(commit);
            self.row_heights.push(self.estimate);
        }
        self.keys_fresh = true;
        self.held = held;
        self.reported_visible = None;
        #[cfg(debug_assertions)]
        { self.append_only_fast = true; }
        Some(anchor)
    }

    /// Debug-only: warns once if a newly mounted existing row's key no longer matches what the
    /// list recorded before an `El::virtual_rows_append_only` fast path.
    #[cfg(debug_assertions)]
    pub(crate) fn check_append_row_key<'a>(
        &self,
        path: &str,
        i: usize,
        commit: u64,
        key_at: impl FnOnce(usize) -> std::borrow::Cow<'a, str>,
    ) {
        if !self.append_only_fast || self.born[i] >= commit || self.shown.contains(&self.keys[i]) {
            return;
        }
        let actual = key_at(i);
        if actual.as_ref() != self.keys[i] {
            crate::tooling::perf_warn::virtual_rows_append_broken(
                path, self.append_source, i, &self.keys[i], actual.as_ref(),
            );
        }
    }

    /// The list's own scroll offset, and whether its window is pinned to its end, for its
    /// viewport scrolled to `scroll` (`pinned`: the viewport holds its end). A nested list
    /// is never pinned itself: it stands where the viewport's end leaves it.
    pub fn at(&self, scroll: f32, pinned: bool) -> (f32, bool) {
        if !self.nested {
            return ((scroll - self.top).max(0.0), pinned);
        }
        let offset = if pinned { self.extent() + self.tail - self.viewport } else { scroll - self.top };
        (offset.max(0.0), false)
    }

    /// [`VirtualList::at`], noting whether a nested list lies wholly above the viewport's top
    /// and so anchors nothing (`select`, `hold` and the commits that anchor the reader).
    pub fn locate(&mut self, scroll: f32, pinned: bool) -> (f32, bool) {
        let at = self.at(scroll, pinned);
        self.passed = self.nested && at.0 >= self.extent();
        at
    }

    /// Whether the list is nested and wholly above the viewport's top, so it anchors nothing
    /// ([`VirtualList::locate`]).
    pub fn passed(&self) -> bool {
        self.passed
    }

    /// The rows' extent, their padding included.
    fn extent(&self) -> f32 {
        self.pad_top + self.prefix(self.keys.len()) + self.pad_bottom
    }

    pub fn set_estimate(&mut self, estimate: f32) {
        if estimate != self.estimate {
            self.estimate = estimate;
            for (h, m) in self.row_heights.iter_mut().zip(&self.measured) {
                if m.is_none() { *h = estimate; }
            }
            self.sums_stale.set(true);
        }
    }

    pub fn set_gap(&mut self, gap: f32) {
        if gap != self.gap {
            self.gap = gap;
            self.sums_stale.set(true);
        }
    }

    pub(crate) fn set_overscan(&mut self, override_vp: Option<(f32, f32)>) -> bool {
        let changed = self.overscan_override != override_vp;
        self.overscan_override = override_vp;
        changed
    }

    #[inline]
    pub(crate) fn overscan_vp(&self) -> f32 {
        self.overscan_override.map_or(OVERSCAN, |(o, _)| o)
    }

    #[inline]
    pub(crate) fn edge_vp(&self) -> f32 {
        self.overscan_override.map_or(if self.indexed { INDEXED_EDGE } else { EAGER_EDGE }, |(_, e)| e)
    }

    pub fn set_height(&mut self, key: &str, height: f32) {
        let Some(i) = self.index_of(key) else { return };
        self.measured[i] = Some(height);
        if self.row_heights[i] != height {
            self.row_heights[i] = height;
            self.sums_stale.set(true);
        }
    }

    #[inline]
    fn is_row_no_anchor(&self, index: usize) -> bool {
        self.row_no_anchor.get(index).copied().unwrap_or(false)
    }

    pub fn set_row_no_anchor(&mut self, index: usize, excluded: bool) {
        if index >= self.keys.len() { return; }
        if excluded {
            if self.row_no_anchor.len() < self.keys.len() {
                self.row_no_anchor.resize(self.keys.len(), false);
            }
            self.row_no_anchor[index] = true;
        } else if let Some(slot) = self.row_no_anchor.get_mut(index) {
            *slot = false;
        }
    }

    pub fn set_no_anchor(&mut self, key: &str, excluded: bool) {
        if let Some(i) = self.index_of(key) { self.set_row_no_anchor(i, excluded); }
    }

    /// Re-picks `anchor` against pre-layout heights if the row chosen by `select`/`hold`
    /// turned out to carry `El::no_scroll_anchor` once built.
    pub fn reanchor_if_excluded(&mut self) {
        let Some((key, _)) = &self.anchor else { return };
        let excluded = self.no_anchor || self.index_of(key).is_some_and(|i| self.is_row_no_anchor(i));
        if excluded {
            self.anchor = if self.no_anchor { None } else { self.anchor_offset.and_then(|o| self.anchor_at(o)) };
        }
    }

    fn sums(&self) -> Ref<'_, Vec<f32>> {
        if self.sums_stale.replace(false) || self.sums.borrow().len() != self.row_heights.len() + 1 {
            let mut sums = self.sums.borrow_mut();
            sums.clear();
            sums.push(0.0);
            let mut y = 0.0;
            for h in &self.row_heights {
                y += h + self.gap;
                sums.push(y);
            }
        }
        self.sums.borrow()
    }

    pub fn prefix(&self, end: usize) -> f32 {
        let sums = self.sums();
        if end == 0 || end < self.keys.len() { sums[end] } else { sums[end] - self.gap }
    }

    /// The bottom of the first `end` rows (without the gap after row `end - 1`).
    pub fn row_end(&self, end: usize) -> f32 {
        if end == 0 { 0.0 } else { self.sums()[end] - self.gap }
    }

    /// The first anchor-eligible row still visible at `offset`, among those `keep` accepts, and its top.
    pub fn first_visible(&self, offset: f32, keep: impl Fn(&str) -> bool) -> Option<(String, f32)> {
        if self.no_anchor { return None; }
        let sums = self.sums();
        let first = sums[1..].partition_point(|&s| self.pad_top + s - self.gap <= offset);
        (first..self.keys.len())
            .find(|&i| !self.is_row_no_anchor(i) && keep(&self.keys[i]))
            .map(|i| (self.keys[i].clone(), self.pad_top + sums[i]))
    }

    /// Like `first_visible`, stopping once rows start at or past `offset + self.viewport`.
    fn first_visible_in_viewport(&self, offset: f32, keep: impl Fn(&str) -> bool) -> Option<(String, f32)> {
        if self.no_anchor { return None; }
        let sums = self.sums();
        let first = sums[1..].partition_point(|&s| self.pad_top + s - self.gap <= offset);
        let limit = offset + self.viewport;
        (first..self.keys.len())
            .take_while(|&i| self.pad_top + sums[i] < limit)
            .find(|&i| !self.is_row_no_anchor(i) && keep(&self.keys[i]))
            .map(|i| (self.keys[i].clone(), self.pad_top + sums[i]))
    }

    /// Whether any visible row in the viewport satisfies `keep`, regardless of `no_anchor`.
    fn any_visible_in_viewport(&self, offset: f32, keep: impl Fn(&str) -> bool) -> bool {
        let sums = self.sums();
        let first = sums[1..].partition_point(|&s| self.pad_top + s - self.gap <= offset);
        let limit = offset + self.viewport;
        (first..self.keys.len())
            .take_while(|&i| self.pad_top + sums[i] < limit)
            .any(|i| keep(&self.keys[i]))
    }

    /// The furthest the rows can scroll: their extent less the viewport.
    pub fn max_offset(&self) -> f32 {
        (self.pad_top + self.prefix(self.keys.len()) + self.pad_bottom - self.viewport).max(0.0)
    }

    /// The row index range intersecting the scroll viewport (`offset .. offset + viewport`).
    pub fn visible_range(&self, offset: f32, pinned: bool) -> Range<usize> {
        if self.keys.is_empty() {
            return 0..0;
        }
        let offset = if pinned { self.max_offset() } else { offset.clamp(0.0, self.max_offset()) };
        let sums = self.sums();
        let start = sums[1..].partition_point(|&s| self.pad_top + s - self.gap <= offset);
        let end = sums[..self.keys.len()].partition_point(|&s| self.pad_top + s < offset + self.viewport).max(start);
        start..end
    }

    /// Picks the rows to mount for `offset`, clamped to the rows' extent. With `budget` (the
    /// rows shown last commit), at most [`INDEXED_BUDGET`] of rows not among them mount per
    /// side beyond the viewport.
    pub fn select(&mut self, offset: f32, pinned: bool, budget: Option<&HashSet<String>>) {
        let offset = if pinned { self.max_offset() } else { offset.min(self.max_offset()) };
        let (start, end) = self.overscan(offset);
        let (mut start, mut end) = {
            let sums = self.sums();
            if let Some(shown) = budget.filter(|_| !pinned) {
                let mounted = |k: &str| shown.contains(k);
                // The viewport's own rows always mount; the overscan takes new rows up to a budget.
                let first = sums[1..].partition_point(|&s| self.pad_top + s - self.gap <= offset).max(start);
                let last = sums[..self.keys.len()].partition_point(|&s| self.pad_top + s < offset + self.viewport).max(first);
                let budget = self.viewport * INDEXED_BUDGET.min(self.overscan_vp());
                let mut spent = 0.0;
                let mut from = first;
                while from > start {
                    let i = from - 1;
                    if !mounted(&self.keys[i]) {
                        if spent >= budget { break; }
                        spent += self.row_heights[i] + self.gap;
                    }
                    from = i;
                }
                let mut spent = 0.0;
                let mut to = last.min(end);
                while to < end {
                    if !mounted(&self.keys[to]) {
                        if spent >= budget { break; }
                        spent += self.row_heights[to] + self.gap;
                    }
                    to += 1;
                }
                (from, to.max(from))
            } else {
                (start, end)
            }
        };
        // Rows drawn in the viewport stay while motion still shows them where layout has moved
        // them out (rows below a fold gliding open), within an overscan of the window.
        let (mut lo, mut hi) = (start, end);
        let overscan_px = self.viewport * self.overscan_vp();
        for i in self.drawn.iter().filter_map(|k| self.index_of(k)) {
            if i < start && self.prefix(start) - self.prefix(i) <= overscan_px { lo = lo.min(i); }
            if i >= end && self.row_end(i + 1) - self.row_end(end) <= overscan_px { hi = hi.max(i + 1); }
        }
        (start, end) = (lo, hi);
        // A row still at its estimate grows or shrinks once measured; anchoring to it would move
        // the measured rows below, such as the reader's row under a page that just landed.
        let disable = pinned || self.passed || self.no_anchor;
        self.anchor_offset = (!disable).then_some(offset);
        self.anchor = if disable { None } else { self.anchor_at(offset) };
        if pinned { start = start.min(end.saturating_sub(PINNED_ROWS)); }
        end = end.max(start);
        self.place(start..end);
    }

    /// Keeps the rows mounted now (clamped to the rows), sets the anchor for `offset` as
    /// [`select`](Self::select) does, and re-derives the space above and below them.
    pub fn hold(&mut self, offset: f32, pinned: bool) {
        let offset = if pinned { self.max_offset() } else { offset.min(self.max_offset()) };
        let disable = pinned || self.passed || self.no_anchor;
        self.anchor_offset = (!disable).then_some(offset);
        self.anchor = if disable { None } else { self.anchor_at(offset) };
        let len = self.keys.len();
        self.place(self.range.start.min(len)..self.range.end.min(len));
    }

    fn place(&mut self, range: Range<usize>) {
        let total = self.prefix(self.keys.len());
        let (start, end) = (range.start, range.end.max(range.start));
        self.range = start..end;
        self.before = self.prefix(start);
        self.after = if start < end { total - self.row_end(end) } else { total - self.before };
    }

    fn anchor_at(&self, offset: f32) -> Option<(String, f32)> {
        if self.no_anchor { return None; }
        let range = self.visible_range(offset, false);
        let sums = self.sums();
        range.into_iter().find(|&i| !self.is_row_no_anchor(i) && self.measured[i].is_some())
            .map(|i| (self.keys[i].clone(), self.pad_top + sums[i]))
            .or_else(|| self.first_visible(offset, |_| true))
    }

    /// The rows within the overscan around `offset`: rows ending at or above its top go, rows
    /// starting above its bottom stay.
    fn overscan(&self, offset: f32) -> (usize, usize) {
        let overscan = self.overscan_vp();
        let lo = offset - self.viewport * overscan;
        let hi = offset + self.viewport * (1.0 + overscan);
        let sums = self.sums();
        let start = sums[1..].partition_point(|&s| self.pad_top + s - self.gap <= lo);
        (start, sums[..self.keys.len()].partition_point(|&s| self.pad_top + s < hi))
    }

    /// How tall the mounted rows outside the overscan around `offset` are: rows that grew since
    /// the window was picked and lay out for nothing.
    pub(crate) fn spare(&self, offset: f32, pinned: bool) -> f32 {
        if pinned || self.keys.is_empty() { return 0.0; }
        let (start, end) = self.overscan(offset.min(self.max_offset()));
        let above = self.prefix(start.min(self.range.end)) - self.prefix(self.range.start.min(start));
        let below = self.row_end(self.range.end) - self.row_end(end.max(self.range.start).min(self.range.end));
        above.max(0.0) + below.max(0.0)
    }

    pub fn anchor_delta(&self) -> f32 {
        let Some((key, old)) = &self.anchor else { return 0.0 };
        let Some(i) = self.index_of(key) else { return 0.0 };
        self.pad_top + self.prefix(i) - old
    }

    pub fn needs_window(&self, offset: f32, pinned: bool) -> bool {
        if self.keys.is_empty() { return false; }
        if pinned { return self.range.end != self.keys.len(); }
        let edge = self.viewport * self.edge_vp();
        let top = self.pad_top + self.before - self.gap;
        (self.range.start > 0 && offset < top + edge)
            || (self.range.end < self.keys.len() && offset + self.viewport > self.pad_top + self.prefix(self.range.end) - edge)
    }

    /// Whether the viewport at `offset` (or the end when `pinned`) reaches past the mounted rows,
    /// leaving part of it blank.
    pub fn viewport_uncovered(&self, offset: f32, pinned: bool) -> bool {
        if self.keys.is_empty() { return false; }
        if pinned {
            let end = self.row_end(self.range.end);
            let mounted = (end - self.before).max(0.0);
            self.range.end < self.keys.len() || (self.range.start > 0 && mounted < self.viewport)
        } else {
            let offset = offset.min(self.max_offset());
            (self.range.start > 0 && offset < self.pad_top + self.before - self.gap)
                || (self.range.end < self.keys.len() && offset + self.viewport > self.pad_top + self.prefix(self.range.end))
        }
    }

    /// How many rows of the overscan around `offset` are still unmounted: zero when the window
    /// is filled. The fill measures this at its bound to tell a list that is still making
    /// progress from one whose heights keep changing under it. Only the overscan's own rows
    /// count: after a jump the mounted window may lie wholly above or below it, and the rows
    /// between the two are wanted by neither. A pinned list wants the overscan at its end, and
    /// at least [`PINNED_ROWS`] rows, as [`select`](Self::select) mounts them.
    pub(crate) fn unfilled(&self, offset: f32, pinned: bool) -> usize {
        if self.keys.is_empty() { return 0; }
        let (start, end) = if pinned {
            let (start, end) = self.overscan(self.max_offset());
            (start.min(end.saturating_sub(PINNED_ROWS)), end)
        } else {
            self.overscan(offset.min(self.max_offset()))
        };
        let mounted = self.range.end.min(end).saturating_sub(self.range.start.max(start));
        end.saturating_sub(start) - mounted
    }
}

#[cfg(test)]
mod overscan_tests;

#[cfg(test)]
mod rekey_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{col, el};

    #[test]
    fn gaps_surround_a_virtual_window_without_a_trailing_gap() {
        let mut v = VirtualList::default();
        v.set_estimate(20.0);
        v.set_gap(7.0);
        v.set_keys((0..100).map(|i| format!("r{i}")).collect(), 0, None);
        v.viewport = 40.0;
        assert_eq!(v.prefix(100), 2693.0);
        assert_eq!(v.prefix(50), 1350.0);
        assert_eq!(v.row_end(50), 1343.0);
        // In a gap the next row is the first visible row, not the one above it.
        assert_eq!(v.first_visible(21.0, |_| true), Some(("r1".into(), 27.0)));
        v.select(1350.0, false, None);
        let n = v.range.len();
        let mounted = n as f32 * 20.0 + n.saturating_sub(1) as f32 * 7.0;
        assert_eq!(v.before + mounted + v.after, 2693.0);
        assert_eq!(v.before, v.range.start as f32 * 27.0);
        v.set_height("r0", 35.0);
        assert_eq!(v.prefix(50), 1365.0, "measured heights replace only the estimate, not the gap");
        v.set_gap(3.0);
        assert_eq!(v.prefix(100), 2312.0, "a changed gap invalidates prefix sums");
        v.set_keys(Vec::new(), 1, None);
        assert_eq!(v.prefix(0), 0.0);
    }

    #[test]
    fn growth_gap_uses_the_vertical_axis_only() {
        assert_eq!(gap_y(&col().gaps(7.0, 19.0).style), 7.0);
        assert_eq!(gap_y(&crate::row().gaps(7.0, 19.0).style), 0.0);
    }

    #[test]
    fn an_absolute_child_is_bounded_only_when_pinned_top_and_bottom() {
        let children = [
            el().pin(Some(0.0), Some(10.0), Some(0.0), Some(20.0)),
            el().absolute(0.0, 10.0),
            el().h(40.0),
            el(),
        ];
        let parent = col().h(100.0).pt(5.0);
        let bounds = child_bounds(&parent.style, false, 100.0, &children);
        assert_eq!(bounds.of(&children[0]), 70.0, "between its insets in the padding box");
        assert_eq!(bounds.of(&children[1]), f32::INFINITY, "free to grow past its parent");
        assert_eq!(bounds.of(&children[3]), 55.0, "in flow, what padding and the fixed sibling leave");
    }

    /// The unfilled count is the overscan's rows the window doesn't hold, wherever the window
    /// sits: a window wholly above or below the overscan after a jump leaves exactly the
    /// overscan to fill, not the rows between them, and no row counts twice.
    #[test]
    fn unfilled_counts_only_the_overscan_rows_the_window_lacks() {
        let mut v = VirtualList::default();
        v.set_estimate(20.0);
        v.set_keys((0..100).map(|i| format!("r{i}")).collect(), 0, None);
        v.viewport = 40.0;
        // At 1080 the overscan runs from 1000 to 1200: rows 50 to 59.
        let (offset, wanted) = (1080.0, 50..60);
        for a in 0..=100 {
            for b in a..=100 {
                v.range = a..b;
                let missing = wanted.clone().filter(|i| !(a..b).contains(i)).count();
                assert_eq!(v.unfilled(offset, false), missing, "window {a}..{b}");
            }
        }
        v.range = 0..10;
        assert_eq!(v.unfilled(offset, false), 10, "a window above the overscan after a jump");
        v.range = 90..100;
        assert_eq!(v.unfilled(offset, false), 10, "a window below the overscan after a jump");
    }

    /// A list pinned to its end counts the rows of its end window the mounted ones don't hold:
    /// the overscan at the end, or the last 32 rows when that is fewer. A window left far above
    /// it after many rows land counts that end window alone, and rows above the window count too.
    #[test]
    fn pinned_unfilled_counts_only_the_end_window_rows_the_window_lacks() {
        let mut v = VirtualList::default();
        v.set_estimate(20.0);
        v.set_keys((0..100).map(|i| format!("r{i}")).collect(), 0, None);
        // A 40 tall viewport ends at 2000, its overscan from 1880: six rows, fewer than 32.
        // A 300 tall one's overscan runs from 1100: rows 55 on.
        for (viewport, wanted) in [(40.0, 68..100), (300.0, 55..100)] {
            v.viewport = viewport;
            for a in 0..=100 {
                for b in a..=100 {
                    v.range = a..b;
                    let missing = wanted.clone().filter(|i| !(a..b).contains(i)).count();
                    // The offset passed is ignored: a pinned list sits at its end.
                    assert_eq!(v.unfilled(0.0, true), missing, "viewport {viewport}, window {a}..{b}");
                }
            }
        }
        v.viewport = 40.0;
        v.range = 0..10;
        assert_eq!(v.unfilled(0.0, true), 32, "a window left at the top after rows landed below it");
    }

    #[test]
    fn set_estimate_after_anchor_compensates_only_unmeasured_rows_above() {
        let mut v = VirtualList::default();
        v.viewport = 200.0;
        v.set_keys((0..50).map(|i| format!("r{i}")).collect(), 0, None);
        v.set_estimate(100.0);
        // Measure rows 20..30 at 100 pt; rows 0..20 stay unmeasured.
        for i in 20..30 { v.set_height(&format!("r{i}"), 100.0); }
        // Anchor at r25 (offset 2500) before applying a smaller estimate.
        v.anchor = v.first_visible(2500.0, |_| true);
        v.set_estimate(95.0);
        // 20 unmeasured rows above r25 shrank by 5 pt each; the 5 measured rows kept 100 pt.
        assert_eq!(v.anchor_delta(), -100.0);
        // Removing r25 while changing estimate keeps surviving r26 anchored against old heights.
        let keys: Vec<_> = (0..50).filter(|&i| i != 25).map(|i| format!("r{i}")).collect();
        v.anchor = v.set_keys(keys, 1, Some(2400.0));
        v.set_estimate(105.0);
        // r26 was at 2500 (20*95 + 6*100); now at index 25 with 20*105 + 5*100 = 2600 -> delta +100.
        assert_eq!(v.anchor_delta(), 100.0);
    }

    #[test]
    fn append_keys_matches_set_keys_prefix_sums_and_heights() {
        let mut fast = VirtualList::default();
        let mut full = VirtualList::default();
        for v in [&mut fast, &mut full] {
            v.viewport = 80.0;
            v.set_estimate(20.0);
            v.set_gap(5.0);
            v.set_keys((0..10).map(|i| format!("r{i}")).collect(), 0, None);
            v.set_height("r2", 35.0);
            v.set_height("r7", 12.0);
            let _ = v.prefix(10); // warm sums before appending
        }
        // Duplicate against existing or within new returns None and mutates nothing.
        assert_eq!(fast.append_keys(vec!["r2".into()], 1, Some(50.0)), None);
        assert_eq!(fast.append_keys(vec!["r10".into(), "r10".into()], 1, Some(50.0)), None);
        assert_eq!(fast.keys.len(), 10);

        let fast_anchor = fast.append_keys((10..15).map(|i| format!("r{i}")).collect(), 1, Some(50.0)).expect("unique");
        let full_anchor = full.set_keys((0..15).map(|i| format!("r{i}")).collect(), 1, Some(50.0));
        assert_eq!(fast_anchor, full_anchor);
        assert_eq!(fast.keys, full.keys);
        assert_eq!(fast.max_offset(), full.max_offset());
        for i in 0..=15 {
            assert_eq!(fast.prefix(i), full.prefix(i), "prefix({i})");
            assert_eq!(fast.row_end(i), full.row_end(i), "row_end({i})");
        }
        for i in 0..15 {
            let k = format!("r{i}");
            assert_eq!(fast.index_of(&k), Some(i));
            assert_eq!(fast.height(&k), full.height(&k));
        }
    }

    #[test]
    fn no_anchor_rows_are_preserved_across_rekey_and_skipped_for_anchoring() {
        let mut v = VirtualList { viewport: 100.0, ..Default::default() };
        v.set_estimate(20.0);
        v.set_keys(vec!["loading".into(), "m0".into(), "m1".into(), "m2".into()], 0, None);
        v.set_no_anchor("loading", true);
        v.set_height("loading", 20.0);
        v.set_height("m0", 20.0);
        // Skipping "loading" anchors to "m0" at offset 0.
        assert_eq!(v.first_visible(0.0, |_| true), Some(("m0".into(), 20.0)));
        v.select(0.0, false, None);
        assert_eq!(v.anchor, Some(("m0".into(), 20.0)));
        // Prepending rows beneath "loading" anchors to "m0" and preserves "loading"'s opt-out.
        let anchor = v.set_keys(
            vec!["loading".into(), "old0".into(), "old1".into(), "m0".into(), "m1".into(), "m2".into()],
            1,
            Some(0.0),
        );
        assert_eq!(anchor, Some(("m0".into(), 20.0)));
        v.anchor = anchor;
        assert_eq!(v.anchor_delta(), 40.0);
        assert_eq!(v.first_visible(0.0, |_| true), Some(("old0".into(), 20.0)));
        // List-level no_anchor disables anchoring without falsely marking replaced_view.
        v.no_anchor = true;
        assert_eq!(v.first_visible(0.0, |_| true), None);
        let anchor = v.set_keys(
            vec!["new0".into(), "loading".into(), "old0".into(), "old1".into(), "m0".into(), "m1".into(), "m2".into()],
            2,
            Some(0.0),
        );
        assert_eq!(anchor, None);
        assert!(!v.replaced_view(), "visible rows survived even with list-level no_anchor");
    }
}
