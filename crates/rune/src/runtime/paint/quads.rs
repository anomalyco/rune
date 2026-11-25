//! Per-node copied-quads cache for still text (`paint_content`), bounded in entries and bytes.
use super::*;
use crate::el::grid::DrawnRow;
use crate::text::LayoutId;

/// Entries kept at most across all text nodes and paragraphs.
pub(in crate::runtime) const MAX_TEXT_QUAD_ENTRIES: usize = 4096;
/// Heap bytes all entries may hold before the least recently used go.
pub(in crate::runtime) const MAX_TEXT_QUAD_BYTES: usize = 8 << 20;
/// Frames an unused entry may stay before the periodic sweep drops it.
const PRUNE_FRAMES: u64 = 64;

type Slot = (NodeId, usize);

struct Entry {
    layout: LayoutId,
    drawn: DrawnRow,
    used: u64,
    stamp: u64,
}

fn entry_bytes(drawn: &DrawnRow) -> usize {
    size_of::<(Slot, Entry)>()
        + drawn.instances.capacity() * size_of::<Instance>()
        + drawn.pages.capacity() * size_of::<(u16, u32)>()
}

/// Each still text piece's quads as it last drew them, keyed by `(NodeId, piece_index)` so a
/// changing text drops its superseded quads immediately instead of holding every recent value
/// until the 64-frame sweep. Bounded in entries and heap bytes with least-recently-used eviction.
pub(in crate::runtime) struct TextQuads {
    entries: HashMap<Slot, Entry>,
    bytes: usize,
    frame: u64,
    stamp: u64,
    max_entries: usize,
    max_bytes: usize,
}

impl TextQuads {
    #[inline]
    pub(super) fn new() -> Self {
        Self {
            entries: HashMap::default(),
            bytes: 0,
            frame: 0,
            stamp: 0,
            max_entries: MAX_TEXT_QUAD_ENTRIES,
            max_bytes: MAX_TEXT_QUAD_BYTES,
        }
    }

    #[inline]
    pub(super) fn begin_frame(&mut self) {
        self.frame += 1;
        if self.frame.is_multiple_of(PRUNE_FRAMES) && !self.entries.is_empty() {
            let frame = self.frame;
            let bytes = &mut self.bytes;
            self.entries.retain(|_, e| {
                let keep = frame - e.used < PRUNE_FRAMES;
                if !keep {
                    *bytes -= entry_bytes(&e.drawn);
                }
                keep
            });
        }
    }

    /// Returns cached quads when `(node, piece)` still holds `layout` drawn under `key`.
    /// When the slot holds a superseded layout or stale draw key, drops it immediately.
    #[inline]
    pub(super) fn get(&mut self, node: NodeId, piece: usize, layout: LayoutId, key: u64) -> Option<&DrawnRow> {
        if self.entries.is_empty() {
            return None;
        }
        match self.entries.entry((node, piece)) {
            std::collections::hash_map::Entry::Occupied(o) => {
                let e = o.get();
                if e.layout == layout && e.drawn.key == key {
                    let e = o.into_mut();
                    self.stamp += 1;
                    e.used = self.frame;
                    e.stamp = self.stamp;
                    Some(&e.drawn)
                } else {
                    let old = o.remove();
                    self.bytes -= entry_bytes(&old.drawn);
                    None
                }
            }
            std::collections::hash_map::Entry::Vacant(_) => None,
        }
    }

    #[inline]
    pub(super) fn remove(&mut self, node: NodeId, piece: usize) {
        if !self.entries.is_empty()
            && let Some(old) = self.entries.remove(&(node, piece))
        {
            self.bytes -= entry_bytes(&old.drawn);
        }
    }

    pub(super) fn insert(&mut self, node: NodeId, piece: usize, layout: LayoutId, drawn: DrawnRow) {
        let slot = (node, piece);
        if let Some(old) = self.entries.remove(&slot) {
            self.bytes -= entry_bytes(&old.drawn);
        }
        let cost = entry_bytes(&drawn);
        if self.max_entries == 0 || cost > self.max_bytes {
            return;
        }
        self.stamp += 1;
        let newest = self.stamp;
        self.bytes += cost;
        self.entries.insert(slot, Entry { layout, drawn, used: self.frame, stamp: newest });
        if self.entries.len() > self.max_entries || self.bytes > self.max_bytes {
            self.evict(newest);
        }
    }

    fn evict(&mut self, keep_stamp: u64) {
        if self.max_entries == 0 || self.max_bytes == 0 {
            self.entries.clear();
            self.bytes = 0;
            return;
        }
        let target_entries = (self.max_entries - self.max_entries / 4).max(1);
        let target_bytes = self.max_bytes - self.max_bytes / 4;
        let mut order: Vec<(u64, Slot)> = self.entries.iter().map(|(&slot, e)| (e.stamp, slot)).collect();
        order.sort_unstable_by_key(|&(stamp, _)| stamp);
        for (stamp, slot) in order {
            if self.entries.len() <= target_entries && self.bytes <= target_bytes {
                break;
            }
            if stamp == keep_stamp {
                continue;
            }
            if let Some(old) = self.entries.remove(&slot) {
                self.bytes -= entry_bytes(&old.drawn);
            }
        }
    }
}

#[cfg(test)]
impl TextQuads {
    pub(in crate::runtime) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(in crate::runtime) fn bytes(&self) -> usize {
        self.bytes
    }

    pub(in crate::runtime) fn set_limits(&mut self, max_entries: usize, max_bytes: usize) {
        self.max_entries = max_entries;
        self.max_bytes = max_bytes;
        if self.entries.len() > self.max_entries || self.bytes > self.max_bytes {
            self.evict(u64::MAX);
        }
    }
}
