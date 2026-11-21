//! The shaped-layout caches' eviction: what stale text stays shaped between commits.

use std::rc::Rc;

use rune_core::HashMap;

use crate::{Ink, Layout, TextStyle, TextSystem, shape_key};

/// Shaped text by hash.
pub(crate) type Cache<T> = HashMap<u64, Entry<T>>;

/// Where a layout [`TextSystem::layout_keyed`] makes is kept.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// In the shared cache, by its text's key, for every text that shows it.
    Cached,
    /// Only by the caller: a by-paragraph field's paragraph, which its
    /// [`ParagraphLayout`](crate::ParagraphLayout) keeps across edits.
    Owned,
    /// Only by the caller, shaped in a context of its own that keeps none of its characters:
    /// secure text.
    Secret,
}

#[doc(hidden)]
#[derive(Clone)]
pub struct CachedElision {
    pub layout: Rc<Layout<Ink>>,
    pub head: usize,
    pub tail: usize,
}

pub(crate) struct Entry<T> {
    pub(crate) value: T,
    /// The commit generation it was last used in.
    pub(crate) used: u32,
    /// About how many heap bytes it holds.
    pub(crate) cost: u32,
    /// The [`shape_key`](super::shape_key) it lays out, or 0 for a run.
    shape: u64,
    /// The `Tx::prepare_text` pass that made it, numbered from 1; 0 when a view made it.
    prepared: u32,
}

impl<T> Entry<T> {
    pub(crate) fn new(value: T, used: u32, text_len: usize, shape: u64) -> Self {
        let cost = (text_len * LAYOUT_PER_BYTE + LAYOUT_BASE).min(u32::MAX as usize) as u32;
        Entry { value, used, cost, shape, prepared: 0 }
    }
}

/// Heap bytes a layout holds per byte of its text, and on top of that. A freshly shaped
/// layout holds 35 to 90 bytes per byte (its buffers keep the room they grew into; a re-broken
/// copy is tight): 605 bytes of prose take 21 to 55 KB, a ten-byte label 1 to 3 KB.
const LAYOUT_PER_BYTE: usize = 64;
const LAYOUT_BASE: usize = 2048;

/// Heap bytes of layouts kept after the commits that used them, so a view shown again after a
/// while (a list returned to from a grid someone browsed) doesn't reshape: about 1,100 short
/// labels or 48 KB of paragraphs.
pub(crate) const STALE_BYTES: usize = 3 << 20;

/// Drops what `cache` hasn't used in the last two commits once it outgrows [`STALE_BYTES`],
/// least recently used first. A view's layouts all share the commit it was last shown in, so
/// that commit's are kept while the budget lasts rather than all dropped together.
fn evict<T>(cache: &mut Cache<T>, g: u32) {
    evict_to(cache, g, STALE_BYTES);
}

/// [`evict`] to `budget` bytes of stale layouts.
fn evict_to<T>(cache: &mut Cache<T>, g: u32, budget: usize) {
    let stale = |used: u32| g.wrapping_sub(used) >= 2;
    let total: usize = cache.values().filter(|e| stale(e.used)).map(|e| e.cost as usize).sum();
    if total <= budget {
        return;
    }
    let mut ages: Vec<(u32, usize)> = cache.values().filter(|e| stale(e.used)).map(|e| (g.wrapping_sub(e.used), e.cost as usize)).collect();
    ages.sort_unstable_by_key(|a| a.0);
    let (mut younger, mut kept, mut cutoff) = (0, 0, u32::MAX);
    for (i, &(age, cost)) in ages.iter().enumerate() {
        if i > 0 && ages[i - 1].0 != age { younger = kept; }
        kept += cost;
        if kept > budget {
            cutoff = age;
            break;
        }
    }
    let mut room = budget - younger;
    cache.retain(|_, e| {
        let (age, cost) = (g.wrapping_sub(e.used), e.cost as usize);
        age < cutoff || age == cutoff && cost <= room && { room -= cost; true }
    });
}

impl TextSystem {
    /// Marks the layouts of `shape` as replaced by an edit of its text, as typing or streaming
    /// makes. Unless this commit used them, they go when it ends, rather than each edit's copy
    /// taking the stale budget from views that may be shown again.
    #[doc(hidden)]
    pub fn supersede(&mut self, shape: u64) {
        self.superseded.insert(shape);
    }

    /// Drops replaced layouts this commit didn't use, then keeps layouts used in the last two
    /// A layout pass begins: a text compared before it may have gone, and another taken its
    /// address.
    #[doc(hidden)]
    pub fn forget_diff(&mut self) {
        self.last_diff = None;
        self.line_scan = None;
    }

    /// commits, and older ones, most recently used first, while they fit in [`STALE_BYTES`].
    pub fn end_commit(&mut self) {
        let g = self.generation;
        if !self.superseded.is_empty() {
            let gone = &self.superseded;
            self.layouts.retain(|_, e| e.used == g || !gone.contains(&e.shape));
            self.elisions.retain(|_, e| e.used == g || !gone.contains(&e.shape));
            self.paragraphs.retain(|_, e| e.used == g || !gone.contains(&e.shape));
            self.superseded.clear();
        }
        evict(&mut self.layouts, g);
        evict(&mut self.paragraphs, g);
        self.last_paragraphs = None;
        self.last_diff = None;
        self.line_scan = None;
        let layouts = &self.layouts;
        self.shapes.retain(|_, key| layouts.contains_key(key));
        evict(&mut self.elisions, g);
        evict(&mut self.runs, g);
        self.generation = g.wrapping_add(1);
    }

    /// A window at rest (`window::rest`): drops every layout the last two commits didn't use,
    /// and the glyph runs drawn from them. A view shown again after it shapes afresh, as one
    /// shown for the first time does.
    #[doc(hidden)]
    pub fn rest(&mut self) {
        // `end_commit` moved the generation on after the last commit.
        let g = self.generation.wrapping_sub(1);
        evict_to(&mut self.layouts, g, 0);
        evict_to(&mut self.paragraphs, g, 0);
        evict_to(&mut self.elisions, g, 0);
        evict_to(&mut self.runs, g, 0);
        let layouts = &self.layouts;
        self.shapes.retain(|_, key| layouts.contains_key(key));
        self.glyph_cache.drop_orphans();
    }

    /// The keys of the layouts held now, for [`Self::keep_prepared`].
    #[doc(hidden)]
    pub fn layout_keys(&self) -> std::collections::HashSet<u64> {
        self.layouts.keys().copied().collect()
    }

    /// Marks the layouts made since `before` was read as prepared, then keeps at most `budget`
    /// bytes of prepared layouts no view holds, by the cache's own count: past it the oldest pass's
    /// go first, and never one a mounted text holds. How many this pass added.
    #[doc(hidden)]
    pub fn keep_prepared(&mut self, before: &std::collections::HashSet<u64>, budget: usize) -> usize {
        thread_local! { static PASS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
        let pass = PASS.with(|p| { p.set(p.get().wrapping_add(1).max(1)); p.get() });
        let mut added = 0;
        for (k, e) in self.layouts.iter_mut() {
            if !before.contains(k) { e.prepared = pass; added += 1; }
        }
        let held = |e: &Entry<Rc<Layout<Ink>>>| Rc::strong_count(&e.value) > 1;
        let mut loose: Vec<(u32, u64, usize)> = self.layouts.iter().filter(|(_, e)| e.prepared != 0 && !held(e)).map(|(k, e)| (pass.wrapping_sub(e.prepared), *k, e.cost as usize)).collect();
        let mut total: usize = loose.iter().map(|l| l.2).sum();
        // Oldest pass first: the largest age.
        loose.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(b.2.cmp(&a.2)));
        for (_, k, cost) in loose {
            if total <= budget { break; }
            self.layouts.remove(&k);
            total -= cost;
        }
        let layouts = &self.layouts;
        self.shapes.retain(|_, key| layouts.contains_key(key));
        added
    }

    #[inline]
    #[doc(hidden)]
    pub fn get_elision(&mut self, key: u64) -> Option<CachedElision> {
        let g = self.generation;
        let e = self.elisions.get_mut(&key)?;
        e.used = g;
        Some(e.value.clone())
    }

    #[inline]
    #[doc(hidden)]
    pub fn insert_elision(&mut self, key: u64, shape: u64, drawn_len: usize, value: CachedElision) {
        let g = self.generation;
        self.elisions.insert(key, Entry::new(value, g, drawn_len, shape));
    }

    /// A live run owns its latest layout; don't retain every timer tick in the commit cache.
    #[doc(hidden)]
    pub fn layout_live(&mut self, text: &str, style: TextStyle, scale: f32) -> Rc<Layout<Ink>> {
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

    #[doc(hidden)]
    pub fn cached_layout_count(&self) -> usize { self.layouts.len() }

    #[doc(hidden)]
    pub fn cached_elision_count(&self) -> usize { self.elisions.len() }
}
