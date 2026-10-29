//! The shaped-layout caches' eviction: what stale text stays shaped between commits.

use std::collections::HashMap;
use std::rc::Rc;

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
pub(super) fn evict<T>(cache: &mut Cache<T>, g: u32) {
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
