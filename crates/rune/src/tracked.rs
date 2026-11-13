//! [`Tracked`]: state a view reads that tells [`region`](crate::region)s when it changed.
//!
//! A region is a [`memo`](crate::memo) with no version of its own: what it read is its version.
//! Each `Tracked` value has a cell id and a version. Reading it (`Deref`) while a region builds
//! records the cell and its version with the region; writing it (`DerefMut`) bumps the version.
//! The next build reuses the region, without calling its closure, while every cell it read keeps
//! its version (and its layout and caret reads hold, as any memo's do).
//!
//! ```ignore
//! struct App { tabs: Tracked<Vec<Tab>>, draft: Tracked<TextEdit> }
//! fn view(s: &App, cx: &mut Cx<App>) -> El {
//!     col().child(region("tabs", || tab_strip(&s.tabs))).child(region("composer", || composer(&s.draft)))
//! }
//! ```
//!
//! A region goes stale only by reading state that isn't `Tracked` (a plain field, a global, a
//! `Cell`): debug builds build every reused region again and print the first element that
//! differs (`region_warnings`). Versions belong to the UI thread that reads and writes them.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU32, Ordering};

/// The next cell id. Ids are never reused, so a read of a dropped value never matches another's.
static NEXT: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Each cell's version on this thread, by id; a cell not written here yet is at 0.
    static VERSIONS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    /// How many regions are building: reads are recorded only inside one.
    static RECORDING: Cell<u32> = const { Cell::new(0) };
    /// The cells read by each building region, innermost last.
    static READS: RefCell<Vec<Vec<u32>>> = const { RefCell::new(Vec::new()) };
}

/// A value whose reads and writes [`region`](crate::region)s see. Use it like the value:
/// it derefs to it. A `&mut` borrow counts as a write whether or not it changes anything, which
/// only costs the regions that read it a rebuild. Assign through it (`*field = value`).
pub struct Tracked<T> {
    id: u32,
    value: T,
}

impl<T> Tracked<T> {
    pub fn new(value: T) -> Self {
        Tracked { id: NEXT.fetch_add(1, Ordering::Relaxed), value }
    }

    // These are associated functions (`Tracked::touch(&field)`), as `Rc`'s are, so they never
    // shadow a method of the value: `field.touch()` still calls the value's own.

    /// The value, without recording a read: for code outside the view (handlers, polls) that
    /// wants to read without paying the check, and for comparing.
    pub fn untracked(this: &Self) -> &T {
        &this.value
    }

    /// Marks the value changed without borrowing it: for a value with interior mutability that
    /// changed behind a shared reference.
    pub fn touch(this: &Self) {
        bump(this.id);
    }

    /// The value's cell: what `RUNE_REGION_MISS` names a `Tracked` read by.
    pub fn id(this: &Self) -> u32 {
        this.id
    }

    /// How many writes the value has seen. A region that keeps rebuilding read a value whose
    /// version moves every frame: compare versions to find which.
    pub fn version(this: &Self) -> u64 {
        version(this.id)
    }

}

impl<T> std::ops::Deref for Tracked<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        if RECORDING.get() > 0 {
            READS.with_borrow_mut(|r| if let Some(frame) = r.last_mut() { frame.push(self.id) });
        }
        &self.value
    }
}

impl<T> std::ops::DerefMut for Tracked<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        bump(self.id);
        &mut self.value
    }
}

/// A value swapped out of its place (`mem::replace`, `mem::take`) counts as a write when it's
/// dropped, so a region that read it builds again; a swapped-out value kept alive doesn't, so
/// assign through it (`*field = value`) instead.
impl<T> Drop for Tracked<T> {
    fn drop(&mut self) {
        bump(self.id);
    }
}

impl<T: Default> Default for Tracked<T> {
    fn default() -> Self {
        Tracked::new(T::default())
    }
}

impl<T: Clone> Clone for Tracked<T> {
    /// A new cell holding a copy: the copy's writes don't touch the original's regions.
    fn clone(&self) -> Self {
        Tracked::new(self.value.clone())
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Tracked<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(f)
    }
}

impl<T: PartialEq> PartialEq for Tracked<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

/// `tracked == value`: compares the value, recording a read.
impl<T: PartialEq> PartialEq<T> for Tracked<T> {
    fn eq(&self, other: &T) -> bool {
        **self == *other
    }
}

impl<T: Eq> Eq for Tracked<T> {}

impl<T: std::hash::Hash> std::hash::Hash for Tracked<T> {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        (**self).hash(h)
    }
}

impl<A, T: FromIterator<A>> FromIterator<A> for Tracked<T> {
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        Tracked::new(iter.into_iter().collect())
    }
}

/// `for x in &tracked`: iterates the value, recording a read.
impl<'a, T> IntoIterator for &'a Tracked<T> where &'a T: IntoIterator {
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (**self).into_iter()
    }
}

/// `for x in &mut tracked`: iterates the value, counting as a write.
impl<'a, T> IntoIterator for &'a mut Tracked<T> where &'a mut T: IntoIterator {
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (**self).into_iter()
    }
}

impl<T> From<T> for Tracked<T> {
    fn from(value: T) -> Self {
        Tracked::new(value)
    }
}

impl<T: serde::Serialize> serde::Serialize for Tracked<T> {
    fn serialize<Z: serde::Serializer>(&self, s: Z) -> Result<Z::Ok, Z::Error> {
        self.value.serialize(s)
    }
}

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for Tracked<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Tracked::new)
    }
}

fn bump(id: u32) {
    VERSIONS.with_borrow_mut(|v| {
        let i = id as usize;
        if v.len() <= i {
            v.resize(i + 1, 0);
        }
        v[i] += 1;
    });
}

/// Cell `id`'s version now.
pub(crate) fn version(id: u32) -> u64 {
    VERSIONS.with_borrow(|v| v.get(id as usize).copied().unwrap_or(0))
}

/// A region starts building: its reads go to a frame of their own.
pub(crate) fn begin() {
    RECORDING.set(RECORDING.get() + 1);
    READS.with_borrow_mut(|r| r.push(Vec::new()));
}

/// A memo that isn't a region starts building: reads inside it record nothing (until a region
/// inside it begins). Returns what `resume` restores.
pub(crate) fn pause() -> u32 {
    RECORDING.replace(0)
}

pub(crate) fn resume(recording: u32) {
    RECORDING.set(recording);
}

/// The region that `begin` started finished: the cells it read, each once, with their versions.
pub(crate) fn end() -> Vec<(u32, u64)> {
    RECORDING.set(RECORDING.get().saturating_sub(1));
    let mut ids = READS.with_borrow_mut(|r| r.pop()).unwrap_or_default();
    ids.sort_unstable();
    ids.dedup();
    VERSIONS.with_borrow(|v| ids.into_iter().map(|id| (id, v.get(id as usize).copied().unwrap_or(0))).collect())
}

/// The [`Tracked`] values a closure read, at the versions it read them ([`tracked_reads`]): for
/// keeping a value derived from state outside a region (a poll's derived list, a cache in a
/// handler) while the state it came from is unchanged.
#[derive(Clone, Debug, Default)]
pub struct Reads(Vec<(u32, u64)>);

impl Reads {
    /// Whether any value read was written since.
    pub fn changed(&self) -> bool {
        VERSIONS.with_borrow(|v| self.0.iter().any(|&(id, version)| v.get(id as usize).copied().unwrap_or(0) != version))
    }

    /// Counts the values as read again, by a region building around a caller that reuses what
    /// it derived from them instead of reading them.
    pub fn read(&self) {
        if RECORDING.get() > 0 {
            READS.with_borrow_mut(|r| if let Some(frame) = r.last_mut() { frame.extend(self.0.iter().map(|&(id, _)| id)) });
        }
    }
}

/// Runs `f`, recording which [`Tracked`] values it reads. A region building around the call
/// reads them too.
pub fn tracked_reads<R>(f: impl FnOnce() -> R) -> (R, Reads) {
    begin();
    let out = f();
    let reads = Reads(end());
    reads.read();
    (out, reads)
}
