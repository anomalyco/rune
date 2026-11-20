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
    /// Everything `virtualize` does holds: rows keep their measured heights by key while the key
    /// stays, unvisited rows use the estimate, inserting or prepending rows keeps the reader's
    /// row in place, and `Tx::scroll_into_view` reaches any row by key. A row's key is
    /// `key(s, i)`, whatever key its element had. The same element needs `.growth(GrowthSpec)`
    /// and must be inside a `.scroll(..)` viewport (see [`El::virtualize`]), and holds no other
    /// children.
    ///
    /// A row outside the window isn't built, so it has no elements at all: the inspector,
    /// `Runtime::elements`, accessibility, Tab order, text selection and matched elements see the
    /// built rows only. A focused field in a row that scrolls out keeps its focus and typing (its
    /// row is built for that, not mounted); a field focused from state in a row that isn't built
    /// takes focus once the row is. A [`memo`](crate::memo) inside `build` keeps its subtree while
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
    /// the same (a change in `count` is always detected without a bump); with a stale version on
    /// an in-place change the rows keep their old keys.
    pub fn virtual_rows_version(mut self, version: impl std::hash::Hash) -> Self {
        self.cold_mut().virtual_rows_version = Some(crate::memo::hash_of(&version));
        self
    }

    /// On a virtual list ([`El::virtualize`] or [`El::virtual_rows`]), called with the index
    /// range `start..end` of rows currently visible in its scroll viewport: once after its
    /// first layout, then whenever that range or the rows change. Return `true` when `f`
    /// changed state the view reads (so the view runs again), or `false` when it only updated
    /// bookkeeping (so scrolling through rows that don't change the view's output doesn't
    /// re-run the view).
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
