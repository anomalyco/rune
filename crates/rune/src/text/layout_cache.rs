//! The shaped-layout caches' eviction: what stale text stays shaped between commits.

use std::collections::HashMap;
use std::rc::Rc;

use super::{Ink, Layout, TextStyle, TextSystem, shape_key};

/// Shaped text by hash: the value, the commit generation it was last used in, and its text's
/// length in bytes.
pub(super) type Cache<T> = HashMap<u64, (Rc<T>, u32, u32)>;

/// Bytes of text, each layout counting [`STALE_ENTRY`] more, kept shaped after the commits
/// that used it, so a view shown again after a while (a list returned to from a grid someone
/// browsed) doesn't reshape. About 800 short labels or 64 KB of paragraphs, either about 3 MB.
pub(super) const STALE_TEXT: usize = 64 * 1024;
pub(super) const STALE_ENTRY: usize = 64;

/// Drops what `cache` hasn't used in the last two commits once it outgrows [`STALE_TEXT`],
/// least recently used first. A view's layouts all share the commit it was last shown in, so
/// that commit's are kept while the budget lasts rather than all dropped together.
fn evict<T>(cache: &mut Cache<T>, g: u32) {
    let stale = |used: u32| g.wrapping_sub(used) >= 2;
    let cost = |bytes: u32| bytes as usize + STALE_ENTRY;
    let total: usize = cache.values().filter(|e| stale(e.1)).map(|e| cost(e.2)).sum();
    if total <= STALE_TEXT {
        return;
    }
    let mut ages: Vec<(u32, usize)> = cache.values().filter(|e| stale(e.1)).map(|e| (g.wrapping_sub(e.1), cost(e.2))).collect();
    ages.sort_unstable_by_key(|a| a.0);
    let (mut younger, mut kept, mut cutoff) = (0, 0, u32::MAX);
    for (i, &(age, cost)) in ages.iter().enumerate() {
        if i > 0 && ages[i - 1].0 != age { younger = kept; }
        kept += cost;
        if kept > STALE_TEXT {
            cutoff = age;
            break;
        }
    }
    let mut room = STALE_TEXT - younger;
    cache.retain(|_, e| {
        let age = g.wrapping_sub(e.1);
        age < cutoff || age == cutoff && cost(e.2) <= room && { room -= cost(e.2); true }
    });
}

impl TextSystem {
    /// Keeps layouts used in the last two commits, and older ones, most recently used first,
    /// while they fit in [`STALE_TEXT`].
    pub fn end_commit(&mut self) {
        let g = self.generation;
        evict(&mut self.layouts, g);
        let layouts = &self.layouts;
        self.shapes.retain(|_, key| layouts.contains_key(key));
        evict(&mut self.runs, g);
        self.generation = g.wrapping_add(1);
    }

    /// A live run owns its latest layout; don't retain every timer tick in the commit cache.
    pub(crate) fn layout_live(&mut self, text: &str, style: TextStyle, scale: f32) -> Rc<Layout<Ink>> {
        let key = shape_key(text, &[], style, scale, false, &[]);
        let cached = self.layouts.contains_key(&key);
        let prev_shape = self.shapes.get(&key).copied();
        let layout = self.layout_rich(text, &[], style, scale, None);
        if !cached {
            self.layouts.remove(&key);
            match prev_shape {
                Some(prev) => { self.shapes.insert(key, prev); }
                None => { self.shapes.remove(&key); }
            }
        }
        layout
    }

    #[cfg(test)]
    pub(crate) fn cached_layout_count(&self) -> usize { self.layouts.len() }
}
