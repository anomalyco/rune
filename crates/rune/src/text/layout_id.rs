//! Which layout an address belongs to, so a cache can key on a layout without holding it.
//!
//! A layout `Rc` can't be held by a cache of quads: a strong count stops `Rc::get_mut` from
//! re-breaking it in place, and a [`Weak`](std::rc::Weak) does too. Addresses alone are no
//! identity, since the allocator hands a freed layout's address to the next one. Each layout is
//! stamped with an id that only goes up: one is built, and one when one is re-broken in place,
//! so an id never describes a layout that came after it.

use std::rc::Rc;

use crate::runtime::HashMap;

use super::{Ink, Layout, TextSystem};

/// A layout's identity, from [`LayoutIds`].
pub(crate) type LayoutId = u64;

/// Layouts stamped before the oldest lose theirs. A layout that outlives its stamp is stamped
/// again when it paints, at the cost of one frame's copy; ids are 16 bytes each.
const MAX_STAMPED: usize = 8 << 10;

/// The ids of the layouts at the addresses they were built at.
#[derive(Default)]
pub(crate) struct LayoutIds {
    next: LayoutId,
    by_address: HashMap<usize, LayoutId>,
}

impl LayoutIds {
    /// A new id for `layout`, which is what the address holds from now on.
    pub(crate) fn build(&mut self, layout: &Layout<Ink>) -> LayoutId {
        let address = layout as *const Layout<Ink> as usize;
        self.stamp(address)
    }

    /// The id last stamped for the layout at `layout`'s address, or a fresh one if its stamp is
    /// gone. A layout is only painted at an address its build stamped last: every build and every
    /// re-break stamps, so a freed layout's id is never read back off its successor.
    pub(crate) fn of(&mut self, layout: &Layout<Ink>) -> LayoutId {
        let address = layout as *const Layout<Ink> as usize;
        match self.by_address.get(&address) {
            Some(&id) => id,
            None => self.stamp(address),
        }
    }

    fn stamp(&mut self, address: usize) -> LayoutId {
        self.next += 1;
        if self.by_address.len() >= MAX_STAMPED && !self.by_address.contains_key(&address) {
            // The ids that have gone the longest without a build go first: a layout still shown
            // takes a new one the next time it paints.
            let mut ids: Vec<LayoutId> = self.by_address.values().copied().collect();
            ids.sort_unstable();
            let cutoff = ids[ids.len() / 2];
            self.by_address.retain(|_, id| *id >= cutoff);
        }
        self.by_address.insert(address, self.next);
        self.next
    }
}

/// The id of the layout a cache can key on without holding it.
impl TextSystem {
    pub(crate) fn layout_id(&mut self, layout: &Rc<Layout<Ink>>) -> LayoutId {
        self.layout_ids.of(layout)
    }
}

#[cfg(test)]
#[path = "layout_id_tests.rs"]
mod tests;
