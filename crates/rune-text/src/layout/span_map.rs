//! Byte-range map for rich-text spans: sorted, non-overlapping intervals with later-entry-wins
//! precedence and a cursor for O(1) amortized monotonic lookups.

use std::cell::Cell;
use std::collections::BinaryHeap;
use std::ops::Range;

#[cfg(test)]
thread_local! {
    static SPAN_SCAN_STEPS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn take_span_scan_steps() -> usize {
    SPAN_SCAN_STEPS.with(|c| c.replace(0))
}

#[cfg(test)]
#[inline]
pub(crate) fn count_span_step() {
    SPAN_SCAN_STEPS.with(|c| c.set(c.get() + 1));
}

#[cfg(not(test))]
#[inline]
pub(crate) fn count_span_step() {}

/// Sorted, non-overlapping `(start, end, value)` byte intervals where later input entries win on
/// overlap. Queries advance a cursor in O(1) amortized time when offsets are monotonic and fall
/// back to binary search when an offset moves backwards (such as bidi runs).
pub(crate) struct SpanMap<T> {
    entries: Vec<(usize, usize, T)>,
    cursor: Cell<usize>,
}

impl<T> SpanMap<T> {
    pub(crate) fn empty() -> Self {
        Self { entries: Vec::new(), cursor: Cell::new(0) }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<T: Copy> SpanMap<T> {
    /// Builds a [`SpanMap`] from `(range, value)` entries in precedence order (later wins).
    /// Already-sorted, non-overlapping input takes a single linear pass with no sorting.
    pub(crate) fn new(iter: impl IntoIterator<Item = (Range<usize>, T)>) -> Self {
        let mut raw = Vec::new();
        let mut disjoint = true;
        let mut last_end = 0usize;
        for (r, v) in iter {
            count_span_step();
            if r.start >= r.end {
                continue;
            }
            if r.start < last_end {
                disjoint = false;
            }
            last_end = last_end.max(r.end);
            raw.push((r.start, r.end, v));
        }
        if disjoint || raw.len() <= 1 {
            return Self { entries: raw, cursor: Cell::new(0) };
        }
        let mut entries = Vec::new();
        flatten_into(&raw, &mut entries);
        Self { entries, cursor: Cell::new(0) }
    }

    /// Smallest index `i` in `0..=entries.len()` with `entries[i].1 > at`.
    fn seek(&self, at: usize) -> usize {
        let mut i = self.cursor.get();
        if i == 0 || self.entries[i - 1].1 <= at {
            while i < self.entries.len() && self.entries[i].1 <= at {
                count_span_step();
                i += 1;
            }
        } else {
            i = self.entries.partition_point(|e| {
                count_span_step();
                e.1 <= at
            });
        }
        self.cursor.set(i);
        i
    }

    /// The value covering byte offset `at`, if any.
    pub(crate) fn get(&self, at: usize) -> Option<T> {
        if self.entries.is_empty() {
            return None;
        }
        let i = self.seek(at);
        count_span_step();
        self.entries.get(i).filter(|e| e.0 <= at).map(|e| e.2)
    }

    /// Whether any entry overlaps the non-empty byte range `range`.
    pub(crate) fn overlaps(&self, range: Range<usize>) -> bool {
        if range.is_empty() || self.entries.is_empty() {
            return false;
        }
        let i = self.seek(range.start);
        count_span_step();
        self.entries.get(i).is_some_and(|e| e.0 < range.end)
    }

    /// Visits the maximal contiguous sub-ranges of `range` with constant coverage (`Some(v)`
    /// where an entry covers it, `None` for uncovered gaps), in ascending byte order.
    pub(crate) fn for_each_in(&self, range: Range<usize>, mut f: impl FnMut(Range<usize>, Option<T>)) {
        if range.is_empty() {
            return;
        }
        if self.entries.is_empty() {
            count_span_step();
            f(range, None);
            return;
        }
        let mut i = self.seek(range.start);
        let mut pos = range.start;
        while i < self.entries.len() {
            let (a, b, v) = self.entries[i];
            if a >= range.end {
                break;
            }
            count_span_step();
            if a > pos {
                f(pos..a, None);
            }
            let end = b.min(range.end);
            f(pos.max(a)..end, Some(v));
            pos = end;
            if b >= range.end {
                break;
            }
            i += 1;
        }
        self.cursor.set(i);
        if pos < range.end {
            f(pos..range.end, None);
        }
    }
}

/// Flattens unsorted or overlapping `(start, end, value)` entries into sorted, non-overlapping
/// intervals where higher input index wins, in O(n log n) time, appending into `out`.
pub(crate) fn flatten_into<K: Ord + Copy, T: Copy>(input: &[(K, K, T)], out: &mut Vec<(K, K, T)>) {
    let mut bounds = Vec::with_capacity(input.len() * 2);
    let mut order: Vec<usize> = Vec::with_capacity(input.len());
    for (i, &(a, b, _)) in input.iter().enumerate() {
        bounds.push(a);
        bounds.push(b);
        order.push(i);
    }
    bounds.sort_unstable();
    bounds.dedup();
    order.sort_unstable_by_key(|&i| input[i].0);

    // Max-heap of `(input_index, end)` for all entries with `start <= a`; stale entries whose
    // `end <= a` are discarded lazily when they reach the top.
    let mut active: BinaryHeap<(usize, K)> = BinaryHeap::new();
    let mut next = 0usize;
    out.reserve(bounds.len());
    let mut last_idx: Option<usize> = None;
    for w in bounds.windows(2) {
        count_span_step();
        let (a, b) = (w[0], w[1]);
        while next < order.len() && input[order[next]].0 <= a {
            count_span_step();
            let i = order[next];
            active.push((i, input[i].1));
            next += 1;
        }
        while active.peek().is_some_and(|&(_, end)| end <= a) {
            count_span_step();
            active.pop();
        }
        if let Some(&(i, _)) = active.peek() {
            if last_idx == Some(i) && let Some(last) = out.last_mut() && last.1 == a {
                last.1 = b;
            } else {
                out.push((a, b, input[i].2));
                last_idx = Some(i);
            }
        } else {
            last_idx = None;
        }
    }
}

#[cfg(test)]
#[path = "span_map_tests.rs"]
mod tests;
