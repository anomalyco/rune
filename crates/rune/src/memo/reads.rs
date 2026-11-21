//! Layout, caret, selection and [`Tracked`](crate::Tracked) reads recorded inside a memo build,
//! validated on reuse, and replayed toward the enclosing view.

use std::rc::Rc;

use crate::runtime::layout_read::{LayoutRects, SELECTION, Same, boxes_moved, moved, note_read, seen_rect, size, window_key};

use super::SCOPES;

/// A `Cx` read made while a memo built, with what it returned.
#[derive(Clone)]
pub(super) enum Read {
    Caret(String, Option<[f32; 4]>),
    Layout(String, Option<[f32; 4]>),
    /// A `Cx::layout_with` read: its function's check, and the rect it was given.
    LayoutWith(String, Same, Option<[f32; 4]>),
    /// A `Cx::selection_rects` read, and the boxes it returned.
    Selection(Rc<[[f32; 4]]>),
    /// A [`Tracked`](crate::Tracked) cell read, at its version then.
    State(u32, u64),
}

impl PartialEq for Read {
    fn eq(&self, other: &Read) -> bool {
        match (self, other) {
            (Read::Caret(a, x), Read::Caret(b, y)) | (Read::Layout(a, x), Read::Layout(b, y)) => a == b && x == y,
            (Read::LayoutWith(a, f, x), Read::LayoutWith(b, g, y)) => a == b && f.is(g) && x == y,
            (Read::Selection(a), Read::Selection(b)) => a == b,
            (Read::State(a, x), Read::State(b, y)) => a == b && x == y,
            _ => false,
        }
    }
}

/// What the `Cx` of the view being built would return now.
pub(super) struct Seen {
    pub(super) caret: Option<(String, [f32; 4])>,
    pub(super) layout: LayoutRects,
}

impl Seen {
    pub(super) fn holds(&self, read: &Read) -> bool {
        match read {
            Read::Caret(key, rect) => crate::runtime::caret_of(&self.caret, key) == *rect,
            Read::Layout(key, rect) => !moved(*rect, seen_rect(&self.layout, key)),
            Read::LayoutWith(key, same, rect) => !same.moved(*rect, seen_rect(&self.layout, key)),
            Read::Selection(boxes) => !boxes_moved(boxes, &self.layout.selection),
            Read::State(id, version) => crate::tracked::version(*id) == *version,
        }
    }
}

/// Each `Read::State` once, the latest version a frame recorded for its cell.
pub(super) fn dedup_states(reads: &mut Vec<Read>) {
    if reads.iter().filter(|r| matches!(r, Read::State(..))).count() < 2 { return; }
    let (mut states, rest): (Vec<Read>, Vec<Read>) = std::mem::take(reads).into_iter().partition(|r| matches!(r, Read::State(..)));
    states.sort_unstable_by_key(|r| match r { Read::State(id, v) => (*id, std::cmp::Reverse(*v)), _ => (0, std::cmp::Reverse(0)) });
    states.dedup_by_key(|r| match r { Read::State(id, _) => *id, _ => 0 });
    *reads = rest;
    reads.extend(states);
}

/// `Cx` reads made by memos this build reused, which the view's `Cx` didn't see.
#[derive(Default)]
pub(crate) struct Replayed {
    pub(crate) caret: bool,
    pub(crate) layout: crate::runtime::layout_read::LayoutReads,
}

/// Records a `Cx::caret` read for the memo building, if any.
pub(crate) fn read_caret(key: &str, rect: Option<[f32; 4]>) {
    record(|| Read::Caret(key.to_owned(), rect));
}

/// Records a `Cx::layout` read for the memo building, if any.
pub(crate) fn read_layout(key: &str, rect: Option<[f32; 4]>) {
    record(|| Read::Layout(key.to_owned(), rect));
}

/// Records a `Cx::selection_rects` read for the memo building, if any.
pub(crate) fn read_selection(boxes: &Rc<[[f32; 4]]>) {
    record(|| Read::Selection(boxes.clone()));
}

/// Records a `Cx::layout_with` read for the memo building, if any.
pub(crate) fn read_layout_with(key: &str, same: &Same, rect: Option<[f32; 4]>) {
    record(|| Read::LayoutWith(key.to_owned(), same.clone(), rect));
}

/// [`Cx::layout`](crate::Cx::layout) for a component that builds inside a view without taking
/// [`Cx`](crate::Cx); `None` outside a view build.
pub fn layout(key: &str) -> Option<[f32; 4]> {
    read_outside_cx(key, |rect| Read::Layout(key.to_owned(), rect)).flatten()
}

/// [`Cx::layout_with`](crate::Cx::layout_with) for a component that builds inside a view
/// without taking [`Cx`](crate::Cx); `None` outside a view build.
pub fn layout_with<T: PartialEq + 'static>(key: &str, f: impl Fn([f32; 4]) -> T + 'static) -> Option<T> {
    let mut value = None;
    let found = read_outside_cx(key, |rect| {
        value = rect.map(&f);
        let same = Same::new(f);
        Read::LayoutWith(key.to_owned(), same, rect)
    });
    found.and(value)
}

/// [`Cx::layout_size`](crate::Cx::layout_size) for a component that builds inside a view
/// without taking [`Cx`](crate::Cx); `None` outside a view build.
pub fn layout_size(key: &str) -> Option<[f32; 2]> {
    layout_with(key, size)
}

/// [`Cx::window_layout`](crate::Cx::window_layout) for a component that builds inside a view
/// without taking [`Cx`](crate::Cx); `None` outside a view build.
pub fn window_layout(key: &str) -> Option<[f32; 4]> {
    layout(&window_key(key))
}

/// [`Cx::window_layout_with`](crate::Cx::window_layout_with) for a component that builds
/// inside a view without taking [`Cx`](crate::Cx); `None` outside a view build.
pub fn window_layout_with<T: PartialEq + 'static>(key: &str, f: impl Fn([f32; 4]) -> T + 'static) -> Option<T> {
    layout_with(&window_key(key), f)
}

/// A layout read of `key` made without a `Cx`, counted toward the view and the memo building,
/// if any: `read` turns the rect the view sees into its record. The rect, or `None` outside a
/// view build.
fn read_outside_cx(key: &str, read: impl FnOnce(Option<[f32; 4]>) -> Read) -> Option<Option<[f32; 4]>> {
    SCOPES.with_borrow_mut(|scopes| {
        let s = scopes.last_mut()?;
        let rect = seen_rect(&s.seen.layout, key);
        let made = read(rect);
        let same = match &made { Read::LayoutWith(_, same, _) => Some(same.clone()), _ => None };
        note_read(&mut s.replayed.layout, key, same);
        if !s.reads.is_empty() {
            s.note(&[made]);
        }
        Some(rect)
    })
}

fn record(read: impl FnOnce() -> Read) {
    SCOPES.with_borrow_mut(|scopes| {
        if let Some(s) = scopes.last_mut()
            && !s.reads.is_empty()
        {
            s.note(&[read()]);
        }
    });
}

/// What a build read, as a region's reads: its [`Tracked`](crate::Tracked) values, layout rects
/// and caret, and those of the memos it built or reused.
#[derive(Default, Clone)]
pub(crate) struct Reads(Vec<Read>);

impl std::fmt::Debug for Reads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reads({})", self.0.len())
    }
}

impl Reads {
    /// Whether everything read still reads the same.
    pub(crate) fn hold(&self) -> bool {
        SCOPES.with_borrow(|scopes| scopes.last().is_none_or(|s| self.0.iter().all(|r| s.seen.holds(r))))
    }

    /// Counts the reads as made again, as a reused memo's are: what was built from them is
    /// used again without being built.
    pub(crate) fn replay(&self) {
        SCOPES.with_borrow_mut(|scopes| {
            let Some(s) = scopes.last_mut() else { return };
            for read in &self.0 {
                match read {
                    Read::Caret(..) => s.replayed.caret = true,
                    Read::Layout(k, _) => note_read(&mut s.replayed.layout, k, None),
                    Read::LayoutWith(k, same, _) => note_read(&mut s.replayed.layout, k, Some(same.clone())),
                    Read::Selection(_) => note_read(&mut s.replayed.layout, SELECTION, None),
                    Read::State(..) => {}
                }
            }
            s.note(&self.0);
        });
    }
}

fn begin_recording() -> bool {
    let framed = SCOPES.with_borrow_mut(|scopes| scopes.last_mut().map(|s| s.reads.push(Vec::new())).is_some());
    crate::tracked::begin();
    framed
}

fn end_recording(framed: bool) -> Reads {
    let states = crate::tracked::end();
    let mut reads = SCOPES.with_borrow_mut(|scopes| scopes.last_mut().filter(|_| framed).and_then(|s| s.reads.pop())).unwrap_or_default();
    reads.extend(states.into_iter().map(|(id, version)| Read::State(id, version)));
    dedup_states(&mut reads);
    SCOPES.with_borrow_mut(|scopes| if let Some(s) = scopes.last_mut() { s.note(&reads) });
    Reads(reads)
}

/// Runs `f` recording what it reads, for a caller that reuses what `f` built while the reads hold.
#[inline]
pub(crate) fn recording<R>(f: impl FnOnce() -> R) -> (R, Reads) {
    let framed = begin_recording();
    let out = f();
    (out, end_recording(framed))
}
