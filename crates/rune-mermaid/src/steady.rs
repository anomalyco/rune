//! Layouts that only grow while a diagram streams in. A build of a source still arriving holds
//! to the layout of the same source one line shorter: no node moves back a rank or changes
//! places with a node it was laid out beside, nodes only move on along their ranks (right in a
//! top-down diagram, down in a left-right one), never back toward where they were, and no gap
//! between ranks narrows. A new edge that would pull a node a rank further and leave the
//! diagram shorter for one line, only for the next line to stretch it again, leaves it as tall
//! as it was instead. The finished source lays out afresh, as a diagram that never streamed
//! does, and settles there in one glide.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// What one layout leaves for the next build of a longer source to hold to, in the layout's
/// own axes, which stay put while the diagram streams in (see [`crate::layered`]).
#[derive(Clone, Default)]
pub(crate) struct Memo {
    /// Each node's rank.
    pub rank: Vec<usize>,
    /// Each node's centre along its rank (`None` for one new since the last build). The next
    /// build places it there or further on.
    pub along: Vec<Option<f32>>,
    /// Each rank's centre across the ranks.
    pub depth: Vec<f32>,
    /// Each node's name (empty: known by index).
    pub ids: Vec<String>,
}

impl Memo {
    /// This memo for a layout of nodes named `ids`, each where the node of its name was.
    fn renamed(&self, ids: &[String]) -> Memo {
        if ids.is_empty() || self.ids.is_empty() {
            return self.clone();
        }
        let was: HashMap<&str, usize> = self.ids.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
        let old: Vec<Option<usize>> = ids.iter().map(|id| was.get(id.as_str()).copied()).collect();
        Memo {
            rank: old.iter().map(|o| o.and_then(|o| self.rank.get(o).copied()).unwrap_or(0)).collect(),
            along: old.iter().map(|o| o.and_then(|o| self.along.get(o).copied().flatten())).collect(),
            depth: self.depth.clone(),
            ids: ids.to_vec(),
        }
    }
}

/// What a streaming build laid out, one [`Memo`] per layout in the order it ran them, whether a
/// horizontal diagram turned upright, and how big it drew.
#[derive(Default)]
pub(crate) struct Memory {
    layouts: Vec<Memo>,
    turned: bool,
    /// The top left corner it was drawn from, in its layouts' points.
    origin: Option<[f32; 2]>,
    /// The size the scene was drawn at.
    pub size: [f32; 2],
}

struct Build {
    prev: Option<Rc<Memory>>,
    next: Memory,
}

thread_local! {
    static BUILD: RefCell<Option<Build>> = const { RefCell::new(None) };
}

/// Runs `f`, a scene build, holding its layouts to `prev`'s, and returns what they laid out.
pub(crate) fn build<R>(prev: Option<Rc<Memory>>, f: impl FnOnce() -> R) -> (R, Memory) {
    struct Restore(Option<Build>);
    impl Drop for Restore {
        fn drop(&mut self) {
            BUILD.set(self.0.take());
        }
    }
    let restore = Restore(BUILD.replace(Some(Build { prev, next: Memory::default() })));
    let out = f();
    let next = BUILD.with_borrow_mut(|b| b.take().map(|b| b.next)).unwrap_or_default();
    drop(restore);
    (out, next)
}

/// Runs one layout of nodes named `ids`: `place` gets the memo this layout's counterpart in the
/// last build left (`None` outside a streaming build, or for a first one) and returns what it
/// placed and the memo for the next build.
pub(crate) fn layout<T>(ids: &[String], place: impl FnOnce(Option<&Memo>) -> Option<(T, Memo)>) -> Option<T> {
    let Some((prev, at)) = BUILD.with_borrow(|b| b.as_ref().map(|b| (b.prev.clone(), b.next.layouts.len()))) else {
        return place(None).map(|p| p.0);
    };
    let held = prev.as_ref().and_then(|m| m.layouts.get(at)).map(|m| m.renamed(ids));
    let placed = place(held.as_ref());
    let (out, mut memo) = placed.map_or((None, Memo::default()), |(t, m)| (Some(t), m));
    memo.ids = ids.to_vec();
    BUILD.with_borrow_mut(|b| {
        if let Some(b) = b {
            b.next.layouts.push(memo);
        }
    });
    out
}

/// Whether the last build turned a horizontal diagram upright, so this one stays turned.
pub(crate) fn turned() -> bool {
    BUILD.with_borrow(|b| b.as_ref().and_then(|b| b.prev.as_ref()).is_some_and(|m| m.turned))
}

pub(crate) fn set_turned(turned: bool) {
    BUILD.with_borrow_mut(|b| {
        if let Some(b) = b {
            b.next.turned = turned;
        }
    });
}

/// The top left corner a drawing with bounds `b` is drawn from: never further in than the last
/// build's, so a diagram growing up or left moves its nodes on, and one drawing less far out
/// than before doesn't move them back. A turned diagram starts afresh.
pub(crate) fn origin(b: [f32; 4], turned: bool) -> [f32; 2] {
    BUILD.with_borrow_mut(|build| {
        let Some(build) = build else { return [b[0], b[1]] };
        let was = build.prev.as_ref().filter(|m| m.turned == turned).and_then(|m| m.origin);
        let o = was.map_or([b[0], b[1]], |o| [o[0].min(b[0]), o[1].min(b[1])]);
        build.next.origin = Some(o);
        o
    })
}

#[cfg(test)]
#[path = "steady_tests.rs"]
mod tests;
