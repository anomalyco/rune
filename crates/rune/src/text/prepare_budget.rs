//! What one `Tx::prepare_text` pass may lay out: new layouts up to a byte budget by the cache's
//! own count (`Entry::new`'s cost), and none begun after a deadline. Past either, a text the pass
//! asks for gets the empty layout of its style instead of being shaped, and is not cached under its
//! own key, so a view that shows it later shapes it then. The first new layout of a pass is always
//! made, so a pass that resumes on the next idle turn gets further each time.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use parley::{Alignment, BaseDirection};

use super::layout_cache::{Keep, entry_cost};
use super::{Ink, Layout, TextStyle, TextSystem};

/// Why a pass stopped laying text out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cut {
    /// It laid out all it asked for.
    No,
    /// Its time ran out; the rest can be laid out on another turn.
    Time,
    /// Its bytes ran out.
    Bytes,
}

#[derive(Clone, Copy)]
struct Budget {
    left: usize,
    until: Instant,
    made: u32,
    cut: Cut,
}

thread_local! {
    /// The open pass's budget, if a pass is open on this thread.
    static BUDGET: Cell<Option<Budget>> = const { Cell::new(None) };
}

/// A pass's budget while it is open; [`Self::close`] says what it used.
pub(crate) struct PrepareBudget {
    bytes: usize,
}

impl PrepareBudget {
    /// Opens a pass that may add `bytes` of layouts, begun before `until`.
    pub(crate) fn open(bytes: usize, until: Instant) -> Self {
        BUDGET.set(Some(Budget { left: bytes, until, made: 0, cut: Cut::No }));
        PrepareBudget { bytes }
    }

    /// Closes the pass: the bytes it added, and whether and why it stopped short.
    pub(crate) fn close(self) -> (usize, Cut) {
        BUDGET.take().map_or((0, Cut::No), |b| (self.bytes.saturating_sub(b.left), b.cut))
    }
}

impl Drop for PrepareBudget {
    fn drop(&mut self) {
        BUDGET.set(None);
    }
}

impl TextSystem {
    /// Inside an open pass past its budget: the empty layout of `style` for a text of `len` bytes,
    /// which the caller returns instead of shaping it. None outside a pass or within its budget,
    /// which then pays for this layout.
    pub(super) fn past_prepare_budget(&mut self, len: usize, style: TextStyle, scale: f32, align: Alignment, base: BaseDirection) -> Option<Rc<Layout<Ink>>> {
        let mut b = BUDGET.get()?;
        let cost = entry_cost(len);
        let late = Instant::now() >= b.until;
        if b.made == 0 || (cost <= b.left && !late) {
            b.left = b.left.saturating_sub(cost);
            b.made += 1;
            BUDGET.set(Some(b));
            return None;
        }
        if b.cut == Cut::No { b.cut = if late { Cut::Time } else { Cut::Bytes }; }
        if len == 0 { BUDGET.set(Some(b)); return None; }
        BUDGET.set(None);
        let empty = self.layout_keyed(None, "", &[], &[], &[], style, scale, None, align, false, false, Keep::Cached, base);
        BUDGET.set(Some(b));
        Some(empty)
    }
}
