//! Flat node positions by id. Sequential ids go straight to a slot without hashing. Size the
//! table from the tree being placed, not the largest id ever issued: a virtual list can mint
//! millions of ids with only a few dozen mounted. Old live ids sharing a slot go in `overflow`.
use super::{HashMap, NodeId};

#[derive(Default)]
pub(in crate::runtime) struct NodeIndex {
    slots: Vec<(NodeId, usize)>,
    overflow: HashMap<NodeId, usize>,
    len: usize,
}

impl NodeIndex {
    pub(in crate::runtime) fn rebuild(&mut self, nodes: usize) {
        self.slots.clear();
        self.slots.resize(nodes.next_power_of_two(), (NodeId(0), usize::MAX));
        self.overflow.clear();
        self.len = 0;
    }

    pub(in crate::runtime) fn insert(&mut self, id: NodeId, i: usize) {
        if self.slots.is_empty() {
            self.slots.resize(1, (NodeId(0), usize::MAX));
        }
        let at = id.0 as usize & (self.slots.len() - 1);
        let slot = &mut self.slots[at];
        if slot.1 == usize::MAX {
            *slot = (id, i);
            self.len += 1;
        } else if slot.0 == id {
            slot.1 = i;
        } else if self.overflow.insert(id, i).is_none() {
            self.len += 1;
        }
    }

    #[inline]
    pub(in crate::runtime) fn get(&self, id: &NodeId) -> Option<&usize> {
        let (stored, i) = self.slots.get(id.0 as usize & self.slots.len().wrapping_sub(1))?;
        if *stored == *id && *i != usize::MAX { Some(i) }
        else if self.overflow.is_empty() { None }
        else { self.overflow.get(id) }
    }

    pub(in crate::runtime) fn contains_key(&self, id: &NodeId) -> bool { self.get(id).is_some() }
    pub(in crate::runtime) fn len(&self) -> usize { self.len }

    pub(in crate::runtime) fn rest(&mut self, nodes: &[super::Node]) {
        let target_slots = nodes.len().max(1).next_power_of_two();
        let slot_floor = (target_slots.saturating_mul(2)).max(64);
        let overflow_floor = (self.overflow.len().saturating_mul(2)).max(32);
        if self.slots.capacity() > slot_floor || self.overflow.capacity() > overflow_floor {
            self.rebuild(nodes.len());
            for (i, n) in nodes.iter().enumerate() {
                self.insert(n.id, i);
            }
            self.slots.shrink_to(target_slots.max(64));
            let ov_cap = (self.overflow.len().saturating_mul(2)).max(16);
            if self.overflow.capacity() > ov_cap {
                self.overflow.shrink_to(ov_cap);
            }
        }
    }
}

impl std::ops::Index<&NodeId> for NodeIndex {
    type Output = usize;
    fn index(&self, id: &NodeId) -> &usize { self.get(id).expect("node not indexed") }
}
