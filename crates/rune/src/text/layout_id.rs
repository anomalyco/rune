//! Which layout an address belongs to, so a cache can key on a layout without holding it.
//!
//! A layout `Rc` can't be held by a cache of quads: a strong count stops `Rc::get_mut` from
//! re-breaking it in place, and a [`Weak`](std::rc::Weak) does too. Addresses alone are no
//! identity, since the allocator hands a freed layout's address to the next one. Each layout is
//! stamped with an id that only goes up: one is built, and one when one is re-broken in place,
//! so an id never describes a layout that came after it. The counter is the process's, so a
//! text system that replaces another never hands out an id the replaced one used.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::runtime::HashMap;

use super::{Ink, Layout, TextSystem};

/// A layout's identity, from [`LayoutIds`].
pub(crate) type LayoutId = u64;

/// Layouts stamped before the oldest lose theirs. A layout that outlives its stamp isn't copied
/// until it is built or re-broken again; ids are 16 bytes each.
const MAX_STAMPED: usize = 8 << 10;

/// Every id, from every text system: a replacing system's first layout is newer than the one it
/// replaced, so its id can't match a quad the old system drew.
static NEXT: AtomicU64 = AtomicU64::new(1);

fn address_of(layout: &Layout<Ink>) -> usize {
    layout as *const Layout<Ink> as usize
}

/// The ids of the layouts at the addresses they were built at.
#[derive(Default)]
pub(crate) struct LayoutIds {
    by_address: HashMap<usize, LayoutId>,
}

impl LayoutIds {
    /// A new id for `layout`, which is what the address holds from now on.
    pub(crate) fn build(&mut self, layout: &Layout<Ink>) -> LayoutId {
        self.stamp(address_of(layout))
    }

    /// The id stamped for the layout at `layout`'s address, if the table still keeps it.
    ///
    /// A lookup never stamps: a layout whose stamp went misses its copy until it is built or
    /// re-broken again, and no lookup can push another live layout's id out.
    pub(crate) fn of(&self, layout: &Layout<Ink>) -> Option<LayoutId> {
        self.of_at(address_of(layout))
    }

    fn of_at(&self, address: usize) -> Option<LayoutId> {
        self.by_address.get(&address).copied()
    }

    fn stamp(&mut self, address: usize) -> LayoutId {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        if self.by_address.len() >= MAX_STAMPED && !self.by_address.contains_key(&address) {
            // The ids that have gone the longest without a build go first.
            let mut ids: Vec<LayoutId> = self.by_address.values().copied().collect();
            ids.sort_unstable();
            let cutoff = ids[ids.len() / 2];
            self.by_address.retain(|_, id| *id >= cutoff);
        }
        self.by_address.insert(address, id);
        id
    }
}

/// The id of the layout a cache can key on without holding it.
impl TextSystem {
    pub(crate) fn layout_id(&self, layout: &Rc<Layout<Ink>>) -> Option<LayoutId> {
        self.layout_ids.of(layout)
    }

    /// Stamps `layout`'s address again, as a build there would, for a test that gives a freed
    /// layout's address to a new one.
    #[cfg(test)]
    pub(crate) fn restamp_for_test(&mut self, layout: &Rc<Layout<Ink>>) {
        self.layout_ids.stamp(address_of(layout));
    }

    /// Drops `layout`'s id, as the table's bound does to the oldest stamps.
    #[cfg(test)]
    pub(crate) fn forget_stamp_for_test(&mut self, layout: &Rc<Layout<Ink>>) {
        self.layout_ids.by_address.remove(&address_of(layout));
    }
}

#[cfg(test)]
#[path = "layout_id_tests.rs"]
mod tests;
