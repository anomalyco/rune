//! What a window's `Tx::prepare_text` request may lay out, and when. The request is the window's
//! own (`TextSystem::prepare`). Its first idle turn measures the view in scratch
//! (`runtime/commit/prepare.rs`) and shapes for one slice from its start; past that, the full
//! layout of a text node (and only that: a cut for truncation, a run, ink or a measure from view
//! code always shapes for real) is queued instead, and the empty layout of its style stands in for
//! it in the scratch pass, not cached under its key. A text laid out by paragraph (a multi-line
//! field, a code block shaped per line) is left out once the slice is spent: it is not queued.
//! Each later idle turn shapes queued texts for one slice, stopping between texts, never inside
//! one, up to the request's bytes by the cache's own count (`Entry::new`'s cost); a text that
//! doesn't fit in what is left is skipped. What the first turn shapes is outside that count.

use std::any::Any;
use std::cell::Cell;
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;
use std::time::Instant;

use parley::{Alignment, BaseDirection};

use super::layout_cache::{Keep, entry_cost};
use super::{Ink, Layout, PillChip, Span, TextStyle, TextSystem};

/// One text the scratch pass asked for and didn't get, at each width it was asked for.
struct Pending {
    shape: u64,
    text: String,
    spans: Vec<Span>,
    blocks: Vec<crate::edit::Block>,
    pills: Vec<PillChip>,
    style: TextStyle,
    scale: f32,
    /// Each width asked for, its alignment and whether it wraps prettily there.
    widths: Vec<(Option<f32>, Alignment, bool)>,
    wrap_anywhere: bool,
    base: BaseDirection,
}

/// A request's texts still to shape.
struct Queue {
    items: VecDeque<Pending>,
    /// Bytes it may still lay out.
    left: usize,
    /// The most prepared layouts no view holds may take when it ends (`keep_prepared`).
    keep: usize,
    /// The layout keys held before the request began, so what it made is marked prepared.
    before: HashSet<u64>,
    /// The window's viewport and scale it was measured for; another drops it.
    at: ([f32; 2], f32),
}

/// A window's prepare: the request not yet begun, and what is left of one begun.
#[derive(Default)]
pub(crate) struct Prepare {
    request: Option<(Rc<dyn Any>, Option<(f32, f32)>)>,
    queue: Option<Queue>,
}

/// The open scratch pass, on this thread while the pass runs.
struct Pass {
    until: Instant,
    queued: Vec<Pending>,
}

thread_local! {
    static PASS: std::cell::RefCell<Option<Pass>> = const { std::cell::RefCell::new(None) };
    /// Set while a text node's own full layout is asked for (`NodeLayout`).
    static NODE: Cell<bool> = const { Cell::new(false) };
}

/// The open scratch pass. It closes on drop, so a panic in the view can't leave it open.
pub(crate) struct OpenPass(());

impl Drop for OpenPass {
    fn drop(&mut self) {
        PASS.set(None);
    }
}

/// Marks the text node layout in its scope: the only layout a late pass may stand in for.
pub(crate) struct NodeLayout(bool);

impl NodeLayout {
    pub(crate) fn enter() -> Self {
        NodeLayout(NODE.replace(true))
    }
}

impl Drop for NodeLayout {
    fn drop(&mut self) {
        NODE.set(self.0);
    }
}

/// Whether a scratch pass is open (probe only).
pub(crate) fn pass_open() -> bool {
    PASS.with_borrow(|p| p.is_some())
}

/// Whether the open pass's slice is spent and a text node's layout is asked for.
fn late_node() -> bool {
    NODE.get() && PASS.with_borrow(|p| p.as_ref().is_some_and(|p| Instant::now() >= p.until))
}

impl TextSystem {
    /// Holds `view` (a `fn(&S, &mut Cx<S>) -> El`, boxed) at `size` as this window's request,
    /// dropping any earlier one and what is left of it.
    pub(crate) fn prepare_request(&mut self, view: Rc<dyn Any>, size: Option<(f32, f32)>) {
        self.prepare_drop();
        self.prepare.request = Some((view, size));
    }

    /// Takes this window's request if it is for a view of type `V`.
    pub(crate) fn prepare_take<V: Copy + 'static>(&mut self) -> Option<(V, Option<(f32, f32)>)> {
        let (view, size) = self.prepare.request.take()?;
        Some((*view.downcast_ref::<V>()?, size))
    }

    /// Whether this window has a request to begin or texts queued.
    pub(crate) fn prepare_owed(&self) -> bool {
        self.prepare.request.is_some() || self.prepare.queue.is_some()
    }

    /// Opens a scratch pass that shapes until `until`. Drops what is left of an earlier request.
    pub(crate) fn prepare_open(&mut self, until: Instant) -> OpenPass {
        self.prepare_stop();
        PASS.set(Some(Pass { until, queued: Vec::new() }));
        OpenPass(())
    }

    /// Closes the scratch pass and queues what it didn't shape, to lay out at most `bytes` of it
    /// for a window at viewport `at.0` and scale `at.1`, `before` being the keys held before the
    /// pass began.
    pub(crate) fn prepare_close(&mut self, pass: OpenPass, bytes: usize, before: HashSet<u64>, at: ([f32; 2], f32)) {
        let queued = PASS.take().map_or_else(Vec::new, |p| p.queued);
        drop(pass);
        if queued.is_empty() {
            self.keep_prepared(&before, bytes);
        } else {
            self.prepare.queue = Some(Queue { items: queued.into(), left: bytes, keep: bytes, before, at });
        }
    }

    /// Shapes queued texts until `until`, at least one, stopping between texts, for a window now
    /// at viewport `at.0` and scale `at.1`; a queue measured for another is dropped. How many
    /// layouts it made.
    pub(crate) fn prepare_drain(&mut self, until: Instant, at: ([f32; 2], f32)) -> usize {
        let Some(mut q) = self.prepare.queue.take() else { return 0 };
        if q.at != at {
            self.keep_prepared(&q.before, q.keep);
            return 0;
        }
        let mut made = 0;
        while let Some(p) = q.items.pop_front() {
            let cost = entry_cost(p.text.len()) * p.widths.len();
            // One that doesn't fit in what is left is skipped; smaller ones behind it may.
            if cost > q.left { continue; }
            q.left -= cost;
            for &(max_width, align, pretty) in &p.widths {
                drop(self.layout_keyed(Some(p.shape), &p.text, &p.spans, &p.blocks, &p.pills, p.style, p.scale, max_width, align, p.wrap_anywhere, pretty, Keep::Cached, p.base));
                made += 1;
            }
            if Instant::now() >= until { break; }
        }
        if q.items.is_empty() { self.keep_prepared(&q.before, q.keep); } else { self.prepare.queue = Some(q); }
        made
    }

    /// Drops this window's request and what is left of one, keeping what it made as prepared.
    pub(crate) fn prepare_drop(&mut self) {
        self.prepare.request = None;
        self.prepare_stop();
    }

    /// Drops what is left of this window's begun request, keeping what it made as prepared.
    fn prepare_stop(&mut self) {
        if let Some(q) = self.prepare.queue.take() { self.keep_prepared(&q.before, q.keep); }
    }

    /// Drops what is left of a begun request without marking anything: the cache it was for is
    /// going (a rest, new font families).
    pub(super) fn prepare_forget(&mut self) {
        self.prepare.queue = None;
    }

    /// In a late scratch pass, a text node's full layout: queues it (once per text, with each
    /// width asked for) and gives the empty layout of `style`, which the caller returns instead
    /// of shaping it. None otherwise.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_defer(
        &mut self,
        shape: u64,
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
        if text.is_empty() || !late_node() { return None; }
        PASS.with_borrow_mut(|p| {
            let Some(p) = p.as_mut() else { return };
            match p.queued.iter_mut().find(|q| q.shape == shape && q.text == text) {
                Some(q) => if !q.widths.contains(&(max_width, align, pretty)) { q.widths.push((max_width, align, pretty)) },
                None => p.queued.push(Pending {
                    shape,
                    text: text.to_owned(),
                    spans: spans.to_vec(),
                    blocks: blocks.to_vec(),
                    pills: pills.to_vec(),
                    style,
                    scale,
                    widths: vec![(max_width, align, pretty)],
                    wrap_anywhere,
                    base,
                }),
            }
        });
        Some(self.prepare_stand_in(style, scale, align, base))
    }

    /// In a late scratch pass, a text node laid out by paragraph: the empty layout of `style`,
    /// left out of the request. None otherwise.
    pub(super) fn prepare_skip(&mut self, style: TextStyle, scale: f32, align: Alignment, base: BaseDirection) -> Option<Rc<Layout<Ink>>> {
        late_node().then(|| self.prepare_stand_in(style, scale, align, base))
    }

    /// The empty layout of `style`, shaped outside the pass.
    fn prepare_stand_in(&mut self, style: TextStyle, scale: f32, align: Alignment, base: BaseDirection) -> Rc<Layout<Ink>> {
        let node = NODE.replace(false);
        let empty = self.layout_keyed(None, "", &[], &[], &[], style, scale, None, align, false, false, Keep::Cached, base);
        NODE.set(node);
        empty
    }
}
