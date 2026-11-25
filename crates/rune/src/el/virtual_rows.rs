//! Index-built virtual rows: [`El::virtual_rows`](crate::El::virtual_rows) gives a virtual
//! list a row count, a key per index and a builder per index. Layout builds only the rows in
//! the mounted window, so a commit costs a key check per row plus the rows it mounts, however
//! long the list is.

use std::borrow::Cow;
use std::rc::Rc;

use crate::El;
use crate::el::ElFlags;
use crate::states::downcast;

impl El {
    /// Window keyed rows of a growth stack inside a vertical scroll viewport.
    /// Unvisited rows use an estimated height; measurement preserves the visible anchor.
    /// Offscreen rows have no layout, text or scene nodes.
    ///
    /// Use [`El::virtual_rows`] unless the list is short: the view still builds every row
    /// handed to `virtualize` on every commit, mounted or not. Debug builds warn once per list
    /// that builds more than 200 rows and four times what it mounts.
    ///
    /// The same element must also be a growth stack: call `.growth(GrowthSpec)` on it as well
    /// as `.virtualize(..)`, and put it inside a `.scroll(..)` viewport: as its content, or
    /// under or among other content in it, at any depth (a page whose figures sit above a long
    /// list in one scroll). It windows against the nearest viewport around it. Layout panics
    /// with the element's path otherwise.
    ///
    /// ```ignore
    /// col().scroll(ScrollSpec { stick_bottom: false }).grow(1.0).child(
    ///     col().key("rows").growth(GrowthSpec::default()).virtualize(32.0).children(rows),
    /// )
    /// ```
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn virtualize(mut self, estimated_row_height: f32) -> Self {
        self.cold_mut().virtual_estimate = Some(estimated_row_height.max(1.0));
        self.flags.set(ElFlags::HAS_VIRTUAL, true);
        #[cfg(debug_assertions)]
        { self.cold_mut().virtual_source = Some(std::panic::Location::caller()); }
        self
    }

    /// [`El::virtualize`] with rows built by index: `count` rows, row `i` keyed `key(s, i)` and
    /// built by `build(s, i)`. Layout calls `build` only for the rows in the mounted window (the
    /// viewport and up to two viewports either side), after it knows where that window is, so a
    /// commit costs one `key` call per row plus the rows it mounts, however long the list is.
    /// Rows new to the window beyond the viewport mount half a viewport per side per frame, so a
    /// jump or a prepended page doesn't lay out three viewports of text at once.
    /// Both closures get the window's state, so they capture nothing borrowed.
    ///
    /// Everything `virtualize` does holds except [`El::reveal_selected`]: rows keep their measured
    /// heights by key while the key stays, unvisited rows use the estimate, inserting or
    /// prepending rows keeps the reader's row in place, and
    /// [`Tx::scroll_into_view`](crate::Tx::scroll_into_view) reaches any row by key.
    /// [`El::reveal_selected`] only inspects eager child rows of [`El::virtualize`] and does
    /// nothing here (debug builds warn once per list); reveal rows from selection-moving handlers
    /// via [`Tx::scroll_into_view`](crate::Tx::scroll_into_view) or
    /// [`Tx::scroll_into_view_in`](crate::Tx::scroll_into_view_in) (or
    /// [`Tx::scroll_center`](crate::Tx::scroll_center) /
    /// [`Tx::scroll_key_to`](crate::Tx::scroll_key_to)). A row's key is `key(s, i)`, whatever
    /// key its element had. The same element needs `.growth(GrowthSpec)` and must be inside a
    /// `.scroll(..)` viewport (see [`El::virtualize`]), and holds no other children.
    ///
    /// A row outside the window isn't built, so it has no elements at all: the inspector,
    /// `Runtime::elements`, accessibility, Tab order, text selection and matched elements see the
    /// built rows only. A focused field in a row that scrolls out keeps its focus and typing (its
    /// row is built for that, not mounted); a field focused from state in a row that isn't built
    /// takes focus once the row is. A [`memo`](crate::memo()) inside `build` keeps its subtree while
    /// the row stays built; a row built again after scrolling out builds afresh.
    ///
    /// Handlers in a row must find their item by its key or id, never by `i`. An index is only
    /// where the item sat when the row was built: a row kept by `memo` keeps its handlers while
    /// rows are inserted or removed above it, so a handler holding `i` would act on whatever
    /// item moved into that slot.
    ///
    /// ```ignore
    /// col().scroll(ScrollSpec { stick_bottom: true }).grow(1.0).child(
    ///     col().key("rows").growth(GrowthSpec::default()).virtual_rows(s.messages.len(), 72.0,
    ///         |s: &App, i| s.messages[i].id.as_str().into(),
    ///         |s: &App, i| {
    ///             let m = &s.messages[i];
    ///             let id = m.id.clone();
    ///             memo(&m.id, m.revision, || message(m).on_press(move |s: &mut App| s.open(&id)))
    ///         }),
    /// )
    /// ```
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn virtual_rows<S: 'static>(
        mut self,
        count: usize,
        estimated_row_height: f32,
        key: impl for<'a> Fn(&'a S, usize) -> Cow<'a, str> + 'static,
        build: impl Fn(&S, usize) -> El + 'static,
    ) -> Self {
        self.cold_mut().virtual_rows = Some(Rc::new(RowSource::new(count, key, build)));
        self.virtualize(estimated_row_height)
    }

    /// Tells [`El::virtual_rows`] its keys are those of the last commit that gave the same
    /// `version`, so layout skips calling `key` for every row to find out. A list of tens of
    /// thousands of rows then commits in time independent of its length. Change `version`
    /// whenever rows are reordered, replaced or any row's key changes while the row count stays
    /// the same (a change in `count` is always detected without a bump; add
    /// [`El::virtual_rows_append_only`] when a growing count only appends at the end); with a
    /// stale version on an in-place change the rows keep their old keys.
    pub fn virtual_rows_version(mut self, version: impl std::hash::Hash) -> Self {
        self.cold_mut().virtual_rows_version = Some(crate::memo::hash_of(&version));
        self
    }

    /// Promises [`El::virtual_rows`] that when `count` grows (and [`El::virtual_rows_version`],
    /// if set, is unchanged), the existing rows keep their keys in order and the extra rows are
    /// appended at the end. Layout then reads only the new keys and updates row bookkeeping in
    /// time proportional to the appended rows rather than the full list.
    ///
    /// A shrinking list or a changed [`El::virtual_rows_version`] still takes the full rekey
    /// path, so a growing list can combine `virtual_rows_append_only()` with a version bump
    /// whenever it filters, sorts or resets.
    ///
    /// If the promise is broken (an existing row's key changes alongside an append without a
    /// version bump), start, midpoint and end checks (plus mounted rows in debug builds) detect
    /// the mismatch and fall back to the full rekey path; debug builds warn once per list when
    /// a broken prefix or stale mounted row is seen.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn virtual_rows_append_only(mut self) -> Self {
        self.cold_mut().virtual_rows_append_only = true;
        #[cfg(debug_assertions)]
        { self.cold_mut().virtual_append_source = Some(std::panic::Location::caller()); }
        self
    }

    /// Sets how far a virtual list ([`El::virtual_rows`] or [`El::virtualize`]) keeps rows
    /// mounted beyond its scroll viewport, in multiples of the viewport height.
    ///
    /// - `overscan`: viewports of rows mounted on each side of the viewport (default `2.0`, so
    ///   about 5 viewports total in the middle of a long list).
    /// - `edge`: how close the viewport may come to either end of the mounted window before a
    ///   commit moves the window (default `1.75` for [`El::virtual_rows`], `0.5` for
    ///   [`El::virtualize`]).
    ///
    /// Smaller values mount fewer offscreen rows and reduce per-frame scroll work, at the cost of
    /// more frequent window-move commits while scrolling (good for long fixed-height tables).
    /// Larger values commit less often while keeping more offscreen nodes mounted.
    ///
    /// `overscan` is clamped to `0.0..=8.0`. When `overscan == 0.0`, `edge` is clamped to `0.0`
    /// (viewport rows only); otherwise `edge` is clamped to `0.0..=(overscan - 0.05).min(overscan * 0.95).max(0.0)`
    /// so consecutive frames never oscillate. Non-finite inputs (`NaN`, `±∞`) are ignored.
    ///
    /// ```
    /// # use rune::{GrowthSpec, ScrollSpec, col, text};
    /// # struct App { rows: Vec<String> }
    /// # let s = App { rows: vec!["row-0".into()] };
    /// # let _ =
    /// col().scroll(ScrollSpec { stick_bottom: false }).grow(1.0).child(
    ///     col().key("rows").growth(GrowthSpec::default())
    ///         .virtual_rows(
    ///             s.rows.len(),
    ///             22.0,
    ///             |s: &App, i| s.rows[i].as_str().into(),
    ///             |s: &App, i| text(&s.rows[i]),
    ///         )
    ///         .virtual_overscan(0.5, 0.25),
    /// )
    /// # ;
    /// ```
    pub fn virtual_overscan(mut self, overscan: f32, edge: f32) -> Self {
        if let Some(pair) = crate::el::virtual_list::clamp_virtual_overscan(overscan, edge) {
            self.cold_mut().virtual_overscan = Some(pair);
        }
        self
    }

    /// On a virtual list ([`El::virtualize`] or [`El::virtual_rows`]), called with the index
    /// range `start..end` of rows currently visible in its scroll viewport: on its first layout,
    /// and whenever that range or the rows change. Changes from commits, rekeys, wheel scrolls
    /// and scroll requests are delivered in the same frame (bounded against feedback loops);
    /// range changes during scroll momentum or glides arrive on the next animation frame. Return
    /// `true` when `f` changed state the view reads (so the view runs again in that frame), or
    /// `false` when it only updated bookkeeping (so scrolling or rekeying when the view's output
    /// does not depend on the range skips an extra view run and frame).
    #[track_caller]
    pub fn on_visible_rows<S: 'static>(
        mut self,
        f: impl Fn(&mut S, std::ops::Range<usize>) -> bool + 'static,
    ) -> Self {
        self.handlers.on_visible_rows = Some(Rc::new(move |s, r| f(downcast::<S>(s), r)));
        #[cfg(debug_assertions)]
        { self.handlers.sites.visible_rows = Some(std::panic::Location::caller()); }
        self
    }
}

pub(crate) type KeyFn = dyn for<'a> Fn(crate::states::StatesRef<'a>, usize) -> Cow<'a, str>;
pub(crate) type BuildFn = dyn Fn(crate::states::StatesRef<'_>, usize) -> El;

/// The rows of an index-built list, built at layout time from the window's state.
pub(crate) struct RowSource {
    pub(crate) count: usize,
    pub(crate) key: Box<KeyFn>,
    pub(crate) build: Box<BuildFn>,
}

fn key_fn<F: for<'a> Fn(crate::states::StatesRef<'a>, usize) -> Cow<'a, str>>(f: F) -> F {
    f
}

fn state<S: 'static>(s: crate::states::StatesRef<'_>) -> &S {
    s.cast::<S>().expect("virtual rows registered for a different state type than the window's")
}

impl RowSource {
    pub(crate) fn new<S: 'static>(
        count: usize,
        key: impl for<'a> Fn(&'a S, usize) -> Cow<'a, str> + 'static,
        build: impl Fn(&S, usize) -> El + 'static,
    ) -> Self {
        RowSource {
            count,
            key: Box::new(key_fn(move |s, i| key(state::<S>(s), i))),
            build: Box::new(move |s, i| build(state::<S>(s), i)),
        }
    }
}

#[cfg(test)]
#[path = "virtual_rows_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "virtual_rows_append_tests.rs"]
mod append_tests;

#[cfg(test)]
#[path = "virtual_rows_overscan_tests.rs"]
mod overscan_tests;
