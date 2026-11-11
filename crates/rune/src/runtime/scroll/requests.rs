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
use super::sticky::{Keep, Reveal};
use crate::machine::Effects;

/// One request outliving the commit that made it.
pub(super) enum Request {
    /// A `Tx::keep_on_screen` element: it and any reveal on its viewport give way to it.
    Capture(Keep),
    /// The row below a fold's header, held where it shows: it takes the viewport's earlier
    /// keeps, while a reveal of the same viewport stays.
    FoldAnchor(Keep),
    /// A `Tx::scroll_into_view` or `Tx::scroll_center` target.
    Reveal(Reveal),
}

#[derive(Default, Clone)]
pub(super) struct Requests {
    keeps: Vec<Keep>,
    reveals: Vec<Reveal>,
}

pub(super) enum Event<'a> {
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
pub(super) enum Effect { Rest, Move(f64) }

/// A fold starts opening: the keeps on its viewport above its end give way. `index` is a path's
/// position in the commit's nodes.
pub(super) struct FoldOpens<'a, I> { pub viewport: &'a str, pub end: usize, pub index: I }

/// Folds nearly shut have left their viewports' ends to take over: a keep holding one of the
/// `giving` headers on a `pinned` viewport gives way.
pub(super) struct EndTakesOver<'a, P> { pub giving: &'a [&'a String], pub pinned: P }

/// Where a keep's element shows, for [`EndClamps`].
pub(super) enum Shown {
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
pub(super) struct EndClamps<F> { pub shown: F }

/// A frame ended: keeps last while their viewport's subtree moves, reveals while their viewport
/// glides or moves with their target mounted in it.
pub(super) struct Finish<G, A, M> { pub gliding: G, pub animating: A, pub mounted: M }

/// Anything [`Requests::step`] takes.
pub(super) trait Transition {
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

#[cfg(test)]
#[path = "requests_tests.rs"]
mod tests;
