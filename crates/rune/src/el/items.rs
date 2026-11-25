//! App-windowed keyed virtual items: [`El::virtual_items`](crate::El::virtual_items) mounts a
//! keyed set of positioned items whose visible window the app decides (such as a 2D canvas,
//! node graph, or LOD scene under [`El::live_offset`](crate::El::live_offset) /
//! [`El::live_scale`](crate::El::live_scale)). Changing the key set remounts only entering and
//! leaving items without re-running `view` or rebuilding staying subtrees.

use std::rc::Rc;

use crate::El;
use crate::el::ElFlags;

/// Source of keyed items for [`El::virtual_items`](crate::El::virtual_items).
///
/// The runtime polls [`Self::version`] once per frame (O(1) per container). When the version
/// changes, the runtime takes the remount fast path (`view` does not run), compares
/// [`Self::keys`] against currently mounted items, unmounts leaving items immediately (keeping
/// any focused item pinned until focus leaves it), keeps staying items intact without calling
/// [`Self::build`], and mounts entering items up to [`Self::mount_budget`] per frame ordered by
/// [`Self::mount_priority`].
///
/// To update an existing item's content without running `view`, replace its key (for example
/// `"node-42@2"`) and bump [`Self::version`]; for high-frequency property updates on a staying
/// item, bind [`crate::Live`] or [`crate::LiveText`] inside [`Self::build`].
pub trait VirtualItems: 'static {
    /// Monotonic or changing version counter checked once per frame.
    ///
    /// Keep this cheap (an integer read): when the returned value matches the last seen version
    /// and no budgeted items remain pending, the runtime does no work and stays idle.
    fn version(&self) -> u64;

    /// Desired item keys for the current window, in back-to-front paint/layout order.
    ///
    /// Keys should be unique within a container. Duplicate keys are deduplicated (keeping the
    /// first occurrence) and reported through Rune's duplicate-key diagnostics.
    fn keys(&self) -> Vec<String>;

    /// Builds the element for an entering item.
    ///
    /// Called only when `key` enters the mounted set (not for staying items on window shifts).
    /// The returned element's key is set to `key`.
    fn build(&self, key: &str) -> El;

    /// Optional per-frame cap on how many newly entering items mount in a single commit.
    ///
    /// `None` (default) mounts every entering item in one commit. `Some(n)` mounts at most
    /// `n.max(1)` entering items per frame, chosen by [`Self::mount_priority`], and requests
    /// follow-up frames until all pending items are mounted.
    fn mount_budget(&self) -> Option<usize> {
        None
    }

    /// Priority for budgeted mounting: smaller values mount first (for example, squared distance
    /// from the viewport center).
    ///
    /// Finite values sort ascending via `f32::total_cmp`; non-finite values (`NaN`, `±∞`) sort
    /// last; ties preserve [`Self::keys`] order. Mounted children always render in [`Self::keys`]
    /// back-to-front order regardless of mount order.
    fn mount_priority(&self, _key: &str) -> f32 {
        0.0
    }
}

impl El {
    /// Populates this element with an app-windowed keyed set of items from `source`.
    ///
    /// Designed for 2D canvases, node graphs, and other containers positioned under a camera
    /// ([`El::live_offset`], [`El::live_scale`]) where there is no 1D scroll viewport or growth
    /// stack. Unlike [`El::virtual_rows`], the app owns spatial culling and exposes the visible
    /// key set via [`VirtualItems`]:
    ///
    /// - When only `source.version()` changes (or budgeted items continue filling), Rune commits
    ///   via the remount path without re-running `view`.
    /// - Staying items keep their retained subtrees, `NodeId`, paint-keep epochs, `memo` entries,
    ///   and [`Live`](crate::Live) / [`LiveText`](crate::LiveText) subscriptions without calling
    ///   `VirtualItems::build` again.
    /// - Leaving items unmount immediately without exit transitions, except an item holding
    ///   keyboard focus (or a focused descendant), which stays pinned and mounted until focus
    ///   leaves it.
    /// - Entering items mount settled (no entrance transition while panning) up to
    ///   [`VirtualItems::mount_budget`] per frame, nearest-first by
    ///   [`VirtualItems::mount_priority`].
    ///
    /// The element must have no direct children and cannot be combined with [`El::virtualize`] or
    /// [`El::virtual_rows`] on the same element.
    ///
    /// ```
    /// # use std::cell::{Cell, RefCell};
    /// # use std::rc::Rc;
    /// # use rune::{El, Live, VirtualItems, el, text};
    /// struct CanvasItems {
    ///     ver: Cell<u64>,
    ///     visible: RefCell<Vec<String>>,
    /// }
    /// impl VirtualItems for CanvasItems {
    ///     fn version(&self) -> u64 { self.ver.get() }
    ///     fn keys(&self) -> Vec<String> { self.visible.borrow().clone() }
    ///     fn build(&self, key: &str) -> El { text(key).absolute(0.0, 0.0) }
    ///     fn mount_budget(&self) -> Option<usize> { Some(16) }
    /// }
    /// let cam = Live::new([0.0f32, 0.0]);
    /// let items: Rc<dyn VirtualItems> = Rc::new(CanvasItems {
    ///     ver: Cell::new(1),
    ///     visible: RefCell::new(vec!["n0".into(), "n1".into()]),
    /// });
    /// let _canvas = el().key("canvas").w_full().h_full().clip_soft(0.0)
    ///     .live_offset(&cam)
    ///     .virtual_items(items);
    /// ```
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn virtual_items(mut self, source: Rc<dyn VirtualItems>) -> Self {
        assert!(
            self.children.is_empty(),
            "`.virtual_items(..)` builds its own items; it takes no children"
        );
        self.cold_mut().virtual_items = Some(source);
        self.flags.set(ElFlags::HAS_VIRTUAL, true);
        #[cfg(debug_assertions)]
        {
            self.cold_mut().virtual_items_source = Some(std::panic::Location::caller());
        }
        self
    }
}


