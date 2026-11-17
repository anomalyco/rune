//! What a `Tx::prepare_text` request may lay out, and when. A request's first idle turn measures
//! the view in scratch (`runtime/commit/prepare.rs`); a new layout it asks for after the turn's
//! slice is spent is queued instead of shaped, and the empty layout of its style stands in for
//! it in the scratch pass (it is not cached under its own key, so a view that shows it shapes it
//! then). Each later idle turn shapes queued texts for one slice, stopping between texts, never
//! inside one, so a turn is a slice plus at most the one text it began. Up to the request's bytes
//! by the cache's own count (`Entry::new`'s cost); past them the rest is dropped. A press or a key
//! drops what is left: the frame it causes shapes what it needs, as it would have.

use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

use parley::{Alignment, BaseDirection};

use super::layout_cache::{Keep, entry_cost};
use super::{Ink, Layout, PillChip, Span, TextStyle, TextSystem};

/// One text the scratch pass asked for and didn't get: everything `layout_keyed` needs.
struct Pending {
    shape: Option<u64>,
    text: String,
    spans: Vec<Span>,
    blocks: Vec<crate::edit::Block>,
    pills: Vec<PillChip>,
    style: TextStyle,
    scale: f32,
    max_width: Option<f32>,
    align: Alignment,
    wrap_anywhere: bool,
    pretty: bool,
    base: BaseDirection,
}

/// The open scratch pass: until when it shapes, and what it queued after that.
struct Pass {
    until: Instant,
    queued: Vec<Pending>,
}

/// A request's texts still to shape, for windows of state `owner`.
struct Queue {
    owner: TypeId,
    items: std::collections::VecDeque<Pending>,
    /// Bytes it may still lay out.
    left: usize,
    /// The most prepared layouts no view holds may take when it ends (`keep_prepared`).
    keep: usize,
    /// The layout keys held before the request began, so what it made is marked prepared when
    /// it ends (`keep_prepared`).
    before: HashSet<u64>,
}

thread_local! {
    static PASS: RefCell<Option<Pass>> = const { RefCell::new(None) };
    static QUEUE: RefCell<Option<Queue>> = const { RefCell::new(None) };
}

impl TextSystem {
    /// Opens a request's scratch pass, shaping until `until`. Drops any earlier request's queue
    /// (marking what it made as prepared).
    pub(crate) fn prepare_open(&mut self, until: Instant) {
        self.prepare_stop();
        PASS.set(Some(Pass { until, queued: Vec::new() }));
    }

    /// Closes the scratch pass and queues what it didn't shape for windows of state `owner`, to
    /// lay out at most `bytes` of it, `before` being the keys held before the pass began. True
    /// when nothing is queued.
    pub(crate) fn prepare_close(&mut self, owner: TypeId, bytes: usize, before: HashSet<u64>) -> bool {
        let queued = PASS.take().map_or_else(Vec::new, |p| p.queued);
        if queued.is_empty() {
            self.keep_prepared(&before, bytes);
            return true;
        }
        QUEUE.set(Some(Queue { owner, items: queued.into(), left: bytes, keep: bytes, before }));
        false
    }

    /// Whether a request for windows of state `owner` has texts queued.
    pub(crate) fn prepare_queued(owner: TypeId) -> bool {
        QUEUE.with_borrow(|q| q.as_ref().is_some_and(|q| q.owner == owner))
    }

    /// Shapes queued texts until `until`, at least one, stopping between texts. How many it shaped.
    pub(crate) fn prepare_drain(&mut self, until: Instant) -> usize {
        let Some(mut q) = QUEUE.take() else { return 0 };
        let mut shaped = 0;
        while let Some(p) = q.items.front() {
            if shaped > 0 && Instant::now() >= until { break; }
            let cost = entry_cost(p.text.len());
            if cost > q.left { q.items.clear(); break; }
            q.left -= cost;
            let Some(p) = q.items.pop_front() else { break };
            let t0 = Instant::now();
            drop(self.layout_keyed(p.shape, &p.text, &p.spans, &p.blocks, &p.pills, p.style, p.scale, p.max_width, p.align, p.wrap_anywhere, p.pretty, Keep::Cached, p.base));
            super::probe_note(format_args!("one us={:.0} len={}", t0.elapsed().as_secs_f64() * 1e6, p.text.len()));
            shaped += 1;
        }
        if q.items.is_empty() { self.keep_prepared(&q.before, q.keep); } else { QUEUE.set(Some(q)); }
        shaped
    }

    /// Drops a request's queue, as a press or a key does, keeping what it made as prepared.
    pub(crate) fn prepare_stop(&mut self) {
        if let Some(q) = QUEUE.take() { self.keep_prepared(&q.before, q.keep); }
    }

    /// In the open scratch pass past its slice: queues this text and gives the empty layout of
    /// `style`, which the caller returns instead of shaping it. None outside a pass or within
    /// its slice.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_defer(
        &mut self,
        shape: Option<u64>,
        text: &str,
        spans: &[Span],
        blocks: &[crate::edit::Block],
        pills: &[PillChip],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
        wrap_anywhere: bool,
        pretty: bool,
        base: BaseDirection,
    ) -> Option<Rc<Layout<Ink>>> {
        let late = PASS.with_borrow(|p| p.as_ref().is_some_and(|p| Instant::now() >= p.until));
        if PASS.with_borrow(|p| p.is_some()) && !late && !text.is_empty() {
            let t0 = Instant::now();
            let pass = PASS.take();
            let l = self.layout_keyed(shape, text, spans, blocks, pills, style, scale, max_width, align, wrap_anywhere, pretty, Keep::Cached, base);
            PASS.set(pass);
            super::probe_note(format_args!("one us={:.0} len={} first", t0.elapsed().as_secs_f64() * 1e6, text.len()));
            return Some(l);
        }
        if !late || text.is_empty() { return None; }
        let pending = Pending {
            shape,
            text: text.to_owned(),
            spans: spans.to_vec(),
            blocks: blocks.to_vec(),
            pills: pills.to_vec(),
            style,
            scale,
            max_width,
            align,
            wrap_anywhere,
            pretty,
            base,
        };
        let pass = PASS.take();
        let empty = self.layout_keyed(None, "", &[], &[], &[], style, scale, None, align, false, false, Keep::Cached, base);
        PASS.set(pass.map(|mut p| { p.queued.push(pending); p }));
        Some(empty)
    }
}
