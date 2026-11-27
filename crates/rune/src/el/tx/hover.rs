//! `Tx`'s reads about the element under the pointer: [`Tx::hovered`] and [`Tx::is_hovered`].

use crate::Tx;

/// Snapshot of what the pointer hovers when a transaction runs, shared by `Rc` across handlers
/// in the same frame/state so `RuntimeCore::transaction` stays O(1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HoverSnapshot {
    /// Key of the innermost hovered element (hovered [`Link`](crate::Link) id, else innermost
    /// keyed element, else `#n` when only unkeyed hit elements are hovered).
    pub(crate) key: Option<String>,
    /// Full tree paths of the hovered leaf element(s), topmost first. Holds 1 entry in almost
    /// every case; more only when overlapping non-hit subtrees contain the pointer without an
    /// occluding hit target above them.
    pub(crate) paths: Vec<String>,
    /// Hovered inline link `(node_path, link_id)` when the pointer sits on a [`Link`](crate::Link).
    pub(crate) link: Option<(String, String)>,
}

/// Whether `path` or any of its ancestors matches `key` (a key or `/`-separated key path,
/// skipping unkeyed `#n` segments), as [`Tx::focus_within`] and [`Tx::is_hovered`] resolve it.
pub(crate) fn path_matches_within(path: &str, key: &str) -> bool {
    let mut cur = path;
    loop {
        let own = cur.rsplit_once('/').map_or(cur, |(_, s)| s);
        if let Some(decoded) = crate::runtime::element_path::decode_key_segment(own)
            && crate::flight::selects(key, cur, Some(decoded.as_ref()))
        {
            return true;
        }
        let Some((above, _)) = cur.rsplit_once('/') else { return false };
        cur = above;
    }
}

/// Innermost keyed segment along `path`, decoded back to its author key.
pub(crate) fn innermost_key(path: &str) -> Option<String> {
    let mut cur = path;
    loop {
        let own = cur.rsplit_once('/').map_or(cur, |(_, s)| s);
        if let Some(decoded) = crate::runtime::element_path::decode_key_segment(own) {
            return Some(decoded.into_owned());
        }
        let Some((above, _)) = cur.rsplit_once('/') else { return None };
        cur = above;
    }
}

impl Tx {
    /// The key of the innermost keyed element under the pointer as this event runs (or the
    /// hovered [`Link`](crate::Link)'s id; `#n` when the pointer is over an unkeyed hit element
    /// with no keyed ancestor), `None` when the pointer is outside the window, over empty
    /// unkeyed background, blocked by an active modal or `El::inert`, or driven by touch.
    ///
    /// Follows CSS `:hover` and [`El::on_hover_change`](crate::El::on_hover_change): overlying
    /// hit elements block what lies beneath them unless marked
    /// [`El::hover_passes_below`](crate::El::hover_passes_below), exiting and unmounted elements
    /// stop hovering at once, and moving layout holds hover until the pointer moves or layout
    /// settles.
    ///
    /// Use [`Tx::is_hovered`] to ask whether the pointer is anywhere inside a container (such as
    /// a card holding child buttons).
    ///
    /// ```
    /// # use rune::{Cx, El, Env, Runtime, col, el};
    /// # #[derive(Default)] struct S { seen: Option<String> }
    /// # fn view(_: &S, _: &mut Cx<S>) -> El {
    /// col().size(200.0, 80.0).child(
    ///     el().key("btn").size(80.0, 40.0).on_press(|_: &mut S| {}),
    /// )
    /// # }
    /// # let mut rt = Runtime::new(S::default(), view, (200.0, 80.0), Env::new(true, 1.0));
    /// # rt.frame(0.0);
    /// # rt.pointer_move(40.0, 20.0);
    /// # rt.update(|s, tx| s.seen = tx.hovered().map(str::to_owned));
    /// # assert_eq!(rt.state.seen.as_deref(), Some("btn"));
    /// ```
    pub fn hovered(&self) -> Option<&str> {
        self.hover.as_deref().and_then(|h| h.key.as_deref())
    }

    /// Whether the pointer is currently over an element matching `key` or anywhere inside it
    /// (CSS `:hover`), as this event runs.
    ///
    /// `key` takes the same key or `/`-separated key path that [`Tx::focus_within`] and
    /// [`Cx::layout`](crate::Cx::layout) resolve (skipping unkeyed `#n` ancestors): `"card"`
    /// matches whether the pointer sits directly on `"card"` or on any descendant inside it,
    /// while `"popover/close"` distinguishes a `"close"` button inside `"popover"` from one
    /// inside `"page"`. Also matches hovered [`Link`](crate::Link) ids (e.g. `"docs"` or
    /// `"body/docs"`). Returns `false` when the pointer is outside the window, driven by touch,
    /// occluded by another hit subtree, blocked by a modal or `El::inert`, exiting/unmounted, or
    /// `key` is empty.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, Runtime, col, el};
    /// # #[derive(Default)] struct S { lifted: bool }
    /// # fn view(_: &S, _: &mut Cx<S>) -> El {
    /// col().key("card").size(160.0, 60.0)
    ///     .on_focus_within_change(|s: &mut S, focused, tx| {
    ///         s.lifted = (focused && tx.focus_visible()) || tx.is_hovered("card");
    ///     })
    ///     .child(el().key("btn").size(80.0, 30.0).focusable(true).on_press(|_: &mut S| {}))
    /// # }
    /// # let mut rt = Runtime::new(S::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    /// # rt.frame(0.0);
    /// # rt.pointer_move(40.0, 15.0);
    /// # let mut over = false;
    /// # rt.update(|_, tx| over = tx.is_hovered("card"));
    /// # assert!(over);
    /// ```
    pub fn is_hovered(&self, key: &str) -> bool {
        if key.is_empty() { return false; }
        let Some(h) = self.hover.as_deref() else { return false };
        if let Some((path, link)) = h.link.as_ref()
            && crate::flight::selects_under(key, path, link)
        {
            return true;
        }
        h.paths.iter().any(|path| path_matches_within(path, key))
    }
}

#[cfg(test)]
#[path = "hover_tests.rs"]
mod tests;
