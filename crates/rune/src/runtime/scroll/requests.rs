//! Requests that outlive a commit: `Tx::keep_on_screen` anchors, a fold's row held below its
//! header, and `Tx::scroll_into_view`/`Tx::scroll_center` reveals. One [`Request`] and one
//! [`Requests::step`] own their life, so which request a viewport keeps, and when it gives way
//! to another, is answered in one place instead of five scans across three files.
//!
//! They are stored in two lanes because they are consumed apart: the paint pass reads only
//! keeps, and the nested commit that remounts virtual rows lifts only the keeps out, leaving
//! the reveals' buffer where it is.
//!
//! Events that test every request on every frame are types of their own, generic over what
//! they measure, so a caller's measurements compile into the loop with no indirect call.
use super::sticky::{Hold, Keep, Reveal};
use super::{AppDriver, HashMap, NodeId, RuntimeCore, ScrollState, Tx, scroll_x};
use crate::machine::{Attempt, Effects};

/// Retained scroll viewports, wheel gesture locking, scroll requests and per-frame sticky state.
/// Ordered hot-first under `#[repr(C)]` so per-node sticky vectors and `scrolls` stay contiguous.
#[repr(C)]
pub(crate) struct ScrollTables {
    /// Each node's sticky shift this frame (points), while `has_sticky`.
    pub(in crate::runtime) stuck: Vec<f32>,
    /// Per node, what its children stick within: the nearest vertical viewport's top and
    /// bottom, then the node's own content box top and bottom (unscaled points), while
    /// `has_sticky`.
    pub(in crate::runtime) sticky_frame: Vec<[f32; 8]>,
    /// Sections whose rows fade under their stuck header this frame (`ScrollFade::below_sticky`),
    /// with the paint clip top the header keeps.
    pub(in crate::runtime) sticky_bands: Vec<(usize, f32)>,
    pub(crate) scrolls: HashMap<NodeId, ScrollState>,
    /// `Tx::keep_on_screen` across the commit underway, and `Tx::scroll_into_view` requests that
    /// follow their target while growth/layout springs run.
    pub(in crate::runtime) requests: Requests,
    /// The axis the current wheel or trackpad gesture scrolls (`runtime/scroll/x.rs`).
    pub(super) wheel_lock: scroll_x::WheelLock,
    /// Whole notches of a mouse wheel ease where the platform's browsers do (`Runtime::set_wheel_glide`).
    wheel_glide: bool,
}

impl ScrollTables {
    // Single-caller constructor for `RuntimeCore::new`; inlined so it stays in its caller's
    // codegen unit instead of shifting partitioning and dropping inlining in the paint walk.
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            stuck: Vec::new(),
            sticky_frame: Vec::new(),
            sticky_bands: Vec::new(),
            scrolls: HashMap::default(),
            requests: Default::default(),
            wheel_lock: Default::default(),
            wheel_glide: true,
        }
    }

    /// Whether whole notches of a mouse wheel ease rather than jump.
    #[inline]
    pub(in crate::runtime) fn wheel_glides(&self, reduced_motion: bool, settled: bool) -> bool {
        self.wheel_glide && !reduced_motion && !settled
    }

    #[inline]
    pub(in crate::runtime) fn set_wheel_glide(&mut self, on: bool) {
        self.wheel_glide = on;
    }

    #[inline]
    pub(in crate::runtime) fn wheel_gesture(&self) -> Attempt {
        self.wheel_lock.gesture
    }

    #[inline]
    pub(in crate::runtime) fn wheel_momentum(&self) -> bool {
        self.wheel_lock.momentum()
    }

    #[inline]
    pub(in crate::runtime) fn wheel_phased(&self) -> bool {
        self.wheel_lock.phased()
    }

    #[inline]
    pub(in crate::runtime) fn wheel_notched(&mut self, now: f64, notch: bool) -> bool {
        self.wheel_lock.notched(now, notch)
    }

    #[inline]
    pub(in crate::runtime) fn wheel_owner(&self) -> &scroll_x::WheelOwner {
        self.wheel_lock.owner()
    }

    #[inline]
    pub(in crate::runtime) fn wheel_owner_of(&self, gesture: Attempt) -> Option<&scroll_x::WheelOwner> {
        self.wheel_lock.owner_of(gesture)
    }

    #[inline]
    pub(in crate::runtime) fn sync_wheel_gesture(&mut self, gesture: Attempt) {
        self.wheel_lock.sync_gesture(gesture);
    }

    #[inline]
    pub(in crate::runtime) fn set_wheel_owner(&mut self, gesture: Attempt, owner: scroll_x::WheelOwner) {
        self.wheel_lock.set_owner(gesture, owner);
    }

    #[inline]
    pub(in crate::runtime) fn wheel_vertical_run(&mut self, now: f64, x: f32, y: f32, dy: f32) -> Option<NodeId> {
        self.wheel_lock.vertical_run(now, x, y, dy)
    }

    #[inline]
    pub(in crate::runtime) fn end_wheel_vertical(&mut self) {
        self.wheel_lock.end_vertical();
    }

    #[inline]
    pub(in crate::runtime) fn ran_wheel_vertical(&mut self, id: NodeId, now: f64, x: f32, y: f32, dy: f32, moved: bool, held: bool) {
        self.wheel_lock.ran_vertical(id, now, x, y, dy, moved, held);
    }

    /// A user or explicit scroll ends the holds and reveals that would pull the view back.
    #[inline]
    pub(in crate::runtime) fn drop_holds(&mut self) {
        self.requests.step(Event::Cancel);
    }

    /// Whether a `Tx::keep_on_screen` holds a stuck header (`Hold`).
    #[inline]
    pub(in crate::runtime) fn holding(&self) -> bool {
        self.requests.keeps().any(|k| k.hold.is_some())
    }

    #[inline]
    pub(in crate::runtime) fn hold_of(&self, path: &str) -> Option<&Hold> {
        if !self.requests.has_keeps() { return None; }
        self.requests.keeps().find_map(|k| k.hold.as_ref().filter(|h| h.path == path))
    }
}

/// One request outliving the commit that made it.
pub(in crate::runtime) enum Request {
    /// A `Tx::keep_on_screen` element: it and any reveal on its viewport give way to it.
    Capture(Keep),
    /// The row below a fold's header, held where it shows: it takes the viewport's earlier
    /// keeps, while a reveal of the same viewport stays.
    FoldAnchor(Keep),
    /// A `Tx::scroll_into_view` or `Tx::scroll_center` target.
    Reveal(Reveal),
}

#[derive(Default, Clone)]
pub(in crate::runtime) struct Requests {
    keeps: Vec<Keep>,
    reveals: Vec<Reveal>,
}

pub(in crate::runtime) enum Event<'a> {
    Arrive(Request),
    /// A reveal placed its target: `from` and `to` are its viewport's offset before and after,
    /// and `first` says whether this is the placement that created it.
    Place { first: bool, scope: Option<&'a str>, key: &'a str, viewport: &'a str,
        center: bool, at_end: bool, from: f64, to: f64 },
    /// A virtual list's last row left the view: an end-pinned reveal follows the row that took
    /// its place.
    NewTail { viewport: &'a str, key: &'a str },
    /// A user or explicit scroll ends every request that would pull the view back.
    Cancel,
    /// An explicit destination wins over a reveal for its whole lifetime; holds stay.
    Destination,
}

/// What placing a reveal asks of its viewport.
#[derive(Debug, PartialEq)]
pub(in crate::runtime) enum Effect { Rest, Move(f64) }

/// A fold starts opening: the keeps on its viewport above its end give way. `index` is a path's
/// position in the commit's nodes.
pub(in crate::runtime) struct FoldOpens<'a, I> { pub viewport: &'a str, pub end: usize, pub index: I }

/// Folds nearly shut have left their viewports' ends to take over: a keep holding one of the
/// `giving` headers on a `pinned` viewport gives way.
pub(in crate::runtime) struct EndTakesOver<'a, P> { pub giving: &'a [&'a String], pub pinned: P }

/// Where a keep's element shows, for [`EndClamps`].
pub(in crate::runtime) enum Shown {
    /// Not in the tree: the keep waits for it.
    Unmounted,
    /// In the tree with no vertical viewport around it.
    Loose,
    /// In a viewport that isn't pinned to its end.
    Free,
    /// This far below the top of a viewport pinned to its end.
    Pinned(f32),
}

/// A frame followed its keeps: one the content's end pushed down, or that lost its viewport,
/// gives way to stick-bottom.
pub(in crate::runtime) struct EndClamps<F> { pub shown: F }

/// A frame ended: keeps last while their viewport's subtree moves, reveals while their viewport
/// glides or moves with their target mounted in it.
pub(in crate::runtime) struct Finish<G, A, M> { pub gliding: G, pub animating: A, pub mounted: M }

/// Anything [`Requests::step`] takes.
pub(in crate::runtime) trait Transition {
    fn apply(self, requests: &mut Requests) -> Effects<Effect>;
}

impl Requests {
    pub fn is_empty(&self) -> bool { self.keeps.is_empty() && self.reveals.is_empty() }
    pub fn has_keeps(&self) -> bool { !self.keeps.is_empty() }
    pub fn keeps(&self) -> impl Iterator<Item = &Keep> { self.keeps.iter() }
    pub fn keeps_mut(&mut self) -> impl Iterator<Item = &mut Keep> { self.keeps.iter_mut() }
    pub fn reveals(&self) -> impl Iterator<Item = &Reveal> { self.reveals.iter() }

    /// Keeps some request, of either kind, holds `path` back.
    pub fn owns(&self, path: &str) -> bool {
        self.keeps.iter().any(|k| k.viewport == path) || self.reveals.iter().any(|r| r.viewport == path)
    }

    /// Lifts the keeps out for the nested commit that remounts virtual rows, which sees no
    /// holds and leaves the reveals in place. No allocation, as no buffer moves.
    pub fn take_keeps(&mut self) -> Vec<Keep> {
        std::mem::take(&mut self.keeps)
    }

    pub fn restore_keeps(&mut self, keeps: Vec<Keep>) {
        self.keeps = keeps;
    }

    fn arrive(&mut self, request: Request) {
        match request {
            Request::Capture(k) => {
                self.keeps.retain(|old| old.viewport != k.viewport);
                self.reveals.retain(|old| old.viewport != k.viewport);
                self.keeps.push(k);
            }
            Request::FoldAnchor(k) => {
                self.keeps.retain(|old| old.viewport != k.viewport);
                self.keeps.push(k);
            }
            Request::Reveal(r) => {
                self.reveals.retain(|old| old.viewport != r.viewport);
                self.reveals.push(r);
            }
        }
    }

    pub fn step(&mut self, event: impl Transition) -> Effects<Effect> {
        event.apply(self)
    }
}

impl Transition for Event<'_> {
    #[inline(always)]
    fn apply(self, requests: &mut Requests) -> Effects<Effect> {
        match self {
            Event::Arrive(request) => requests.arrive(request),
            Event::Place { first, scope, key, viewport, center, at_end, from, to } => {
                if first {
                    requests.arrive(Request::Reveal(Reveal { scope: scope.map(str::to_owned),
                        key: key.to_owned(), viewport: viewport.to_owned(), center, at_end }));
                } else if let Some(r) = requests.reveals.iter_mut().find(|r| r.viewport == viewport) {
                    r.at_end |= at_end;
                }
                if (to - from).abs() < 0.5 {
                    if first { return Effects::one(Effect::Rest); }
                } else { return Effects::one(Effect::Move(to)); }
            }
            Event::NewTail { viewport, key } => {
                if let Some(r) = requests.reveals.iter_mut().find(|r| r.viewport == viewport && r.at_end) {
                    r.scope = None;
                    r.key = key.to_owned();
                }
            }
            Event::Cancel => { requests.keeps.clear(); requests.reveals.clear(); }
            Event::Destination => requests.reveals.clear(),
        }
        Effects::none()
    }
}

impl<I: Fn(&str) -> Option<usize>> Transition for FoldOpens<'_, I> {
    fn apply(self, requests: &mut Requests) -> Effects<Effect> {
        let Self { viewport, end, index } = self;
        requests.keeps.retain(|k| k.viewport != viewport || index(&k.path).is_none_or(|i| i >= end));
        Effects::none()
    }
}

impl<P: Fn(&str) -> bool> Transition for EndTakesOver<'_, P> {
    fn apply(self, requests: &mut Requests) -> Effects<Effect> {
        let Self { giving, pinned } = self;
        requests.keeps.retain(|k| !(k.hold.as_ref().is_some_and(|h| giving.contains(&&h.path)) && pinned(&k.viewport)));
        Effects::none()
    }
}

impl<F: Fn(&Keep) -> Shown> Transition for EndClamps<F> {
    fn apply(self, requests: &mut Requests) -> Effects<Effect> {
        requests.keeps.retain(|k| match (self.shown)(k) {
            Shown::Loose => false,
            Shown::Pinned(top) if top > k.was + 0.5 => false,
            _ => true,
        });
        Effects::none()
    }
}

impl<G, A, M> Transition for Finish<G, A, M>
where G: Fn(&str) -> bool, A: Fn(&str) -> bool, M: Fn(&Reveal) -> bool {
    fn apply(self, requests: &mut Requests) -> Effects<Effect> {
        let Self { gliding, animating, mounted } = self;
        requests.keeps.retain(|k| animating(&k.viewport));
        requests.reveals.retain(|r| gliding(&r.viewport) || (animating(&r.viewport) && mounted(r)));
        Effects::none()
    }
}

impl RuntimeCore {
    /// Resolve reveals and destinations against the tree from the update that requested them.
    pub(in crate::runtime) fn finish_scroll_requests(&mut self, app: &mut dyn AppDriver, tx: &mut Tx, follow: bool) {
        let destination = !tx.scroll_bottom.is_empty() || !tx.scroll_pages.is_empty() || !tx.scroll_offsets.is_empty();
        if tx.scroll_to.is_empty() && !destination && !follow { return; }
        self.drop_scroll_holds();
        for r in tx.scroll_to.drain(..) { self.reveal_row(app, &r, tx.commit.snaps()); }
        if !destination && !follow { return; }
        self.step_scroll(0.0);
        if follow { self.follow_caret(app); }
        if tx.commit.snaps() {
            for (_, animated) in &mut tx.scroll_bottom { *animated = false; }
            for r in &mut tx.scroll_offsets { r.glide = None; }
        }
        if !tx.scroll_bottom.is_empty() { self.scroll_to_bottom(app, std::mem::take(&mut tx.scroll_bottom)); }
        if !tx.scroll_pages.is_empty() { self.scroll_pages(app, std::mem::take(&mut tx.scroll_pages)); }
        if !tx.scroll_offsets.is_empty() { self.scroll_to_offsets(app, std::mem::take(&mut tx.scroll_offsets)); }
        if destination { self.scroll.requests.step(Event::Destination); }
    }
}

#[cfg(test)]
#[path = "requests_tests.rs"]
mod tests;
