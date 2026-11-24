//! Cached subtree representation ([`Built`], [`Shared`]), placeholder expansion on [`El`],
//! and flatten-reusability and text-prekeying checks.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::El;
use crate::el::ElFlags;

/// The children a memo's placeholders share.
#[derive(Clone)]
pub(crate) struct Shared {
    pub(crate) children: Vec<El>,
    /// Whether a focused field is among them, so an offscreen virtual row that has none isn't
    /// searched for one.
    pub(crate) focused_input: bool,
    /// Whether two siblings anywhere among them share a key, so the per-commit key check can
    /// skip a memo without any.
    pub(crate) duplicate_keys: bool,
    /// Whether any descendant uses `virtualize` or `virtual_rows`.
    pub(crate) has_virtual: bool,
    /// Whether every descendant can reuse its flattened `Node` and Taffy subtree across commits.
    pub(crate) reusable: bool,
    /// Whether any descendant carries a path-bound hook (`El::bind_path`), so moving the memo
    /// to a different parent path must re-flatten rather than rebasing stale handlers.
    pub(crate) has_path_bind: bool,
}

impl El {
    /// Every child: those shared with a memo cache, then the element's own.
    pub(crate) fn all_children(&self) -> impl Iterator<Item = &El> {
        self.memo_children().into_iter().flat_map(|c| c.children.iter()).chain(&self.children)
    }

    /// The children shared with a memo cache, which come before `children`.
    #[inline]
    pub(crate) fn memo_children(&self) -> Option<&Rc<Shared>> {
        self.memo.as_ref().map(|b| &b.children)
    }

    /// The cached subtree this placeholder is, unless it was expanded or given extra children.
    #[inline]
    pub(crate) fn memo_built(&self) -> Option<&Rc<Built>> {
        self.memo.as_ref().filter(|_| !self.flags.has(ElFlags::MEMO_EXTENDED))
    }

    /// Copies in the children shared with a memo cache, so the element owns all of them.
    pub(crate) fn expand_memo(&mut self) {
        let whole = !self.flags.has(ElFlags::MEMO_EXTENDED);
        self.flags.set(ElFlags::MEMO_EXTENDED, false);
        if let Some(built) = self.memo.take() {
            if built.children.has_virtual {
                self.flags.set(ElFlags::HAS_VIRTUAL, true);
                // Its virtual lists mount their rows into the copy: flattening records the copy
                // as the memo's nodes, so a later commit whose windows match keeps them.
                if whole && built.reusable {
                    self.cold_mut().expanded_memo = Some(built.clone());
                }
            }
            let shared = Rc::unwrap_or_clone(built.children.clone()).children;
            let own = std::mem::replace(&mut self.children, shared);
            self.children.extend(own);
        }
    }
}

fn el_flatten_reusable(e: &El) -> bool {
    let c = &*e.cold;
    // A list whose rows `virtual_rows` builds is reused with them while it mounts the same rows
    // (`Measurement::keeps_windows`); one built from children isn't.
    let indexed = c.virtual_rows.is_some();
    (c.virtual_estimate.is_none() || indexed)
        && (c.growth.is_none() || indexed)
        && c.aspect_fit <= 0.0
        && c.above.is_none()
        && c.input.is_none()
        && e.handlers.get().is_none_or(|h| h.modal.is_none())
        && e.text.as_ref().is_none_or(|t| !t.redacted && t.cold.live.is_none())
        && e.memo_children().is_none_or(|m| m.reusable && !m.duplicate_keys)
        && e.children.iter().all(el_flatten_reusable)
}

/// Precomputes `shape` on plain, non-input, non-cased text specs at `scale` while the freshly
/// built subtree is still uniquely owned, so flattening never has to copy a shared `TextSpec`.
fn prekey_text_shapes(el: &mut El, scale: f32) {
    if el.cold.input.is_none()
        && let Some(t) = &el.text
        && !t.redacted
        && t.case.is_none()
        && t.cold.rings.is_empty()
        && t.shape.is_none_or(|s| s.0 != scale.to_bits())
        && let Some(t) = &mut el.text
    {
        t.key_shape(scale);
    }
    for child in &mut el.children {
        prekey_text_shapes(child, scale);
    }
}

fn has_path_bind(el: &El) -> bool {
    el.cold.get().is_some_and(|c| c.on_path.is_some())
        || el.memo_children().is_some_and(|m| m.has_path_bind)
        || el.children.iter().any(has_path_bind)
}

/// Whether `el` or anything under it is a focused field.
pub(crate) fn has_focused_input(el: &El) -> bool {
    el.cold.input.as_ref().is_some_and(|i| i.edit.focused)
        || el.memo_children().is_some_and(|m| m.focused_input)
        || el.children.iter().any(has_focused_input)
}

/// A built subtree: its root without children (or key, supplied per placeholder), and the
/// children placeholders share.
pub(crate) struct Built {
    pub(crate) id: u64,
    pub(crate) reusable: bool,
    /// Whether the root or any descendant carries `El::bind_path`.
    pub(crate) has_path_bind: bool,
    /// Built by a [`region`](super::region): what it shows is what it read, so its virtual lists'
    /// rows can be kept with it while their reads hold.
    pub(crate) region: bool,
    pub(crate) root: El,
    pub(crate) children: Rc<Shared>,
}

static NEXT_BUILT_ID: AtomicU64 = AtomicU64::new(1);

impl Built {
    pub(super) fn new(mut el: El, scale: f32, region: bool) -> (Rc<Built>, Option<String>) {
        let key = el.key.take();
        el.flags.set(ElFlags::MEMO_EXTENDED, false);
        let shared = if el.children.is_empty() && let Some(built) = el.memo.take() {
            prekey_text_shapes(&mut el, scale);
            built.children.clone()
        } else {
            el.expand_memo();
            prekey_text_shapes(&mut el, scale);
            let children = std::mem::take(&mut el.children);
            let focused_input = children.iter().any(has_focused_input);
            let duplicate_keys = crate::input::key_diagnostics::memo_children_have_duplicates(&el, &children);
            let has_virtual = children.iter().any(|c| c.flags.has(ElFlags::HAS_VIRTUAL));
            let reusable = !duplicate_keys && children.iter().all(el_flatten_reusable);
            let has_path_bind = children.iter().any(has_path_bind);
            Rc::new(Shared { children, focused_input, duplicate_keys, has_virtual, reusable, has_path_bind })
        };
        el.flags.set(ElFlags::HAS_VIRTUAL, el.cold.virtual_estimate.is_some());
        let reusable = shared.reusable && !shared.duplicate_keys && el_flatten_reusable(&el);
        let has_path_bind = shared.has_path_bind || el.cold.get().is_some_and(|c| c.on_path.is_some());
        let id = NEXT_BUILT_ID.fetch_add(1, Ordering::Relaxed);
        (Rc::new(Built { id, reusable, has_path_bind, region, root: el, children: shared }), key)
    }

    /// The root, with its children shared rather than copied.
    // Called on every memo hit from `memo.rs`; `#[inline]` keeps it visible across codegen
    // units so callers avoid copying the `key` argument into a temporary before the call.
    #[inline]
    pub(super) fn placeholder(self: &Rc<Self>, key: Option<String>) -> El {
        let mut el = self.root.clone();
        el.key = key;
        if self.children.has_virtual {
            el.flags.set(ElFlags::HAS_VIRTUAL, true);
        }
        el.memo = Some(self.clone());
        el
    }
}
