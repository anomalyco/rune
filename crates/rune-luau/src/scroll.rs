//! Bounded per-viewport `onScroll` deduplication and coalescing.
//!
//! Layout reports scroll offsets mid-frame through `El::on_scroll_offset`, which also re-fires
//! whenever a re-render attaches a new closure. Keeping the last delivered `(offset, max)` per
//! viewport tree path suppresses unchanged re-renders so idle scripts stay idle, while coalescing
//! pending moves per path so each `Script::poll` delivers only the newest `ScrollPos`.

use std::rc::Rc;

use rune::ScrollPos;
use rustc_hash::FxHashMap;

use crate::ui::Callback;

/// Maximum number of distinct scroll viewport paths tracked per script.
pub(crate) const TRACK_CAP: usize = 1024;

/// What must change before a viewport's `onScroll` queues again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollSnapshot {
    pub(crate) offset: [f32; 2],
    pub(crate) max: [f32; 2],
}

impl From<ScrollPos> for ScrollSnapshot {
    fn from(pos: ScrollPos) -> Self {
        ScrollSnapshot { offset: pos.offset, max: pos.max }
    }
}

/// One coalesced scroll notification waiting for the next `Script::poll`.
pub(crate) struct PendingScroll {
    pub(crate) cb: Rc<Callback>,
    pub(crate) key: Option<Rc<str>>,
    pub(crate) pos: ScrollPos,
    pub(crate) snap: ScrollSnapshot,
}

/// Per-script bounded scroll deduplication and coalescing state.
#[derive(Default)]
pub(crate) struct ScrollState {
    /// Last `(offset, max)` delivered per viewport path, with a monotonic touch sequence for
    /// oldest-entry eviction past `TRACK_CAP`.
    last: FxHashMap<Box<str>, (ScrollSnapshot, u64)>,
    /// Coalesced pending scroll notifications keyed by viewport path (newest `ScrollPos` wins).
    pending: FxHashMap<Box<str>, PendingScroll>,
    /// Insertion order of `pending` keys for deterministic delivery.
    order: Vec<Box<str>>,
    seq: u64,
}

impl ScrollState {
    /// Clears all tracked and pending scroll state (on VM swap / hot reload).
    pub(crate) fn clear(&mut self) {
        self.last.clear();
        self.pending.clear();
        self.order.clear();
        self.seq = 0;
    }

    /// Records a scroll report for `path`. Returns `true` when a new or changed event is now
    /// pending and the host should wake the window for the next poll.
    pub(crate) fn queue(&mut self, path: &str, cb: &Rc<Callback>, key: Option<&Rc<str>>, pos: ScrollPos) -> bool {
        let snap = ScrollSnapshot::from(pos);
        if let Some((last_snap, seq)) = self.last.get_mut(path)
            && *last_snap == snap
        {
            self.seq = self.seq.wrapping_add(1);
            *seq = self.seq;
            if self.pending.remove(path).is_some() {
                self.order.retain(|p| p.as_ref() != path);
            }
            return false;
        }
        if let Some(entry) = self.pending.get_mut(path) {
            let unchanged = entry.snap == snap;
            entry.cb = cb.clone();
            entry.key = key.cloned();
            entry.pos = pos;
            entry.snap = snap;
            return !unchanged;
        }
        if self.pending.len() >= TRACK_CAP
            && let Some(oldest) = self.order.first().cloned()
        {
            self.order.remove(0);
            self.pending.remove(&oldest);
        }
        let owned: Box<str> = path.into();
        self.order.push(owned.clone());
        self.pending.insert(owned, PendingScroll { cb: cb.clone(), key: key.cloned(), pos, snap });
        true
    }

    /// Drains all pending scroll events in deterministic order.
    pub(crate) fn take_pending(&mut self) -> Vec<(Box<str>, PendingScroll)> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let order = std::mem::take(&mut self.order);
        let mut pending = std::mem::take(&mut self.pending);
        order.into_iter().filter_map(|path| pending.remove(&path).map(|item| (path, item))).collect()
    }

    /// Records `snap` as the last delivered scroll state for `path`, evicting the least-recently
    /// seen entry if the map is at `TRACK_CAP`.
    pub(crate) fn record_last(&mut self, path: Box<str>, snap: ScrollSnapshot) {
        self.seq = self.seq.wrapping_add(1);
        if let Some(entry) = self.last.get_mut(&path) {
            *entry = (snap, self.seq);
            return;
        }
        if self.last.len() >= TRACK_CAP
            && let Some(oldest) = self.last.iter().min_by_key(|(_, (_, s))| *s).map(|(k, _)| k.clone())
        {
            self.last.remove(&oldest);
        }
        self.last.insert(path, (snap, self.seq));
    }

    #[cfg(test)]
    pub(crate) fn last_len(&self) -> usize {
        self.last.len()
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }
}
