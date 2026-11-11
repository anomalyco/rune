//! Requests outliving a commit. Keep and reveal are independent for fold-installed anchors;
//! ordinary captures replace both. Store them by kind so paint can skip an empty kind in O(1)
//! and preserve the order within each pass.
use super::sticky::{Keep, Reveal};
use crate::machine::Effects;

pub(super) type VirtualRow = (String, String, f32);

#[derive(Clone)]
enum Request { Mounted(Keep), Virtual { anchor: Keep, row: VirtualRow }, Reveal(Reveal) }

impl Request {
    fn viewport(&self) -> &str {
        match self { Self::Mounted(k) | Self::Virtual { anchor: k, .. } => &k.viewport, Self::Reveal(r) => &r.viewport }
    }
}

#[derive(Default, Clone)]
pub(super) struct Requests {
    entries: Vec<Request>,
    // Keep the two passes in their original order, and skip an empty pass in O(1).
    n_keeps: usize,
}

pub(super) enum Event<'a> {
    Capture(Keep, Option<VirtualRow>),
    FoldAnchor(Keep),
    Place { first: bool, scope: Option<&'a str>, key: &'a str, viewport: &'a str,
        center: bool, at_end: bool, from: f64, to: f64 },
    NewTail { viewport: &'a str, key: &'a str },
    RetainKeeps(&'a dyn Fn(&Keep) -> bool),
    Finish { gliding: &'a dyn Fn(&str) -> bool, animating: &'a dyn Fn(&str) -> bool,
        mounted: &'a dyn Fn(&Reveal) -> bool },
    Cancel,
    Destination,
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect { Rest, Move(f64) }

impl Requests {
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn has_keeps(&self) -> bool { self.n_keeps != 0 }
    pub fn owns(&self, path: &str) -> bool { self.entries.iter().any(|r| r.viewport() == path) }
    pub fn keeps(&self) -> impl Iterator<Item = &Keep> {
        self.entries[..self.n_keeps].iter().map(|r| match r { Request::Mounted(k) | Request::Virtual { anchor: k, .. } => k, _ => unreachable!() })
    }
    pub fn anchors(&self) -> impl Iterator<Item = (&Keep, Option<&VirtualRow>)> {
        self.entries[..self.n_keeps].iter().map(|r| match r {
            Request::Mounted(k) => (k, None), Request::Virtual { anchor, row } => (anchor, Some(row)),
            Request::Reveal(_) => unreachable!(),
        })
    }
    pub fn keeps_mut(&mut self) -> impl Iterator<Item = &mut Keep> {
        self.entries[..self.n_keeps].iter_mut().map(|r| match r { Request::Mounted(k) | Request::Virtual { anchor: k, .. } => k, _ => unreachable!() })
    }
    pub fn reveals(&self) -> impl Iterator<Item = &Reveal> {
        self.entries[self.n_keeps..].iter().map(|r| match r { Request::Reveal(r) => r, _ => unreachable!() })
    }
    fn reveals_mut(&mut self) -> impl Iterator<Item = &mut Reveal> {
        self.entries[self.n_keeps..].iter_mut().map(|r| match r { Request::Reveal(r) => r, _ => unreachable!() })
    }
    /// Only reveals are visible during the nested commit that remounts virtual rows.
    pub fn take_reveals(&mut self) -> Self {
        Self { entries: self.entries.split_off(self.n_keeps), n_keeps: 0 }
    }
    pub fn restore_reveals(&mut self, mut after_commit: Self) {
        self.entries.extend(after_commit.entries.drain(after_commit.n_keeps..));
    }
    fn retain(&mut self, keep: impl Fn(&Request) -> bool) {
        self.entries.retain(|r| {
            if keep(r) { return true; }
            if !matches!(r, Request::Reveal(_)) { self.n_keeps -= 1; }
            false
        });
    }
    pub fn step(&mut self, event: Event) -> Effects<Effect> {
        match event {
            Event::Capture(k, row) => {
                self.retain(|r| r.viewport() != k.viewport);
                let request = match row { Some(row) => Request::Virtual { anchor: k, row }, None => Request::Mounted(k) };
                self.entries.insert(self.n_keeps, request);
                self.n_keeps += 1;
            }
            Event::FoldAnchor(k) => {
                self.retain(|r| matches!(r, Request::Reveal(_)) || r.viewport() != k.viewport);
                self.entries.insert(self.n_keeps, Request::Mounted(k));
                self.n_keeps += 1;
            }
            Event::Place { first, scope, key, viewport, center, at_end, from, to } => {
                if first {
                    self.entries.retain(|r| !matches!(r, Request::Reveal(r) if r.viewport == viewport));
                    self.entries.push(Request::Reveal(Reveal { scope: scope.map(str::to_owned), key: key.to_owned(),
                        viewport: viewport.to_owned(), center, at_end }));
                } else if let Some(r) = self.reveals_mut().find(|r| r.viewport == viewport) {
                    r.at_end |= at_end;
                }
                if (to - from).abs() < 0.5 {
                    if first { return Effects::one(Effect::Rest); }
                } else { return Effects::one(Effect::Move(to)); }
            }
            Event::NewTail { viewport, key } => {
                if let Some(r) = self.reveals_mut().find(|r| r.viewport == viewport && r.at_end) {
                    r.scope = None;
                    r.key = key.to_owned();
                }
            }
            Event::RetainKeeps(keep) => {
                if self.has_keeps() { self.retain(|r| match r { Request::Mounted(k) | Request::Virtual { anchor: k, .. } => keep(k), Request::Reveal(_) => true }); }
            }
            Event::Finish { gliding, animating, mounted } => self.retain(|r| match r {
                Request::Mounted(k) | Request::Virtual { anchor: k, .. } => animating(&k.viewport),
                Request::Reveal(r) => gliding(&r.viewport) || (animating(&r.viewport) && mounted(r)),
            }),
            Event::Cancel => { self.entries.clear(); self.n_keeps = 0; }
            Event::Destination => self.entries.truncate(self.n_keeps),
        }
        Effects::none()
    }
}

#[cfg(test)]
#[path = "requests_tests.rs"]
mod tests;
