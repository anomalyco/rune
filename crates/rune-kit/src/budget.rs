//! A cache bounded by bytes, not entries. What the last two commits used always stays, so a
//! screen showing many documents never thrashes; past the budget the least recently used of the
//! rest go first.
#![cfg_attr(not(feature = "markdown"), allow(dead_code))]

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;

struct Slot<V> {
    value: V,
    bytes: usize,
    /// When it was last used, in lookups.
    used: u64,
    /// The [`rune::commit_generation`] it was last used in.
    commit: u64,
}

pub(crate) struct Budget<K, V> {
    map: HashMap<K, Slot<V>>,
    bytes: usize,
    limit: usize,
    clock: u64,
}

impl<K: Hash + Eq, V> Budget<K, V> {
    pub(crate) fn new(limit: usize) -> Self {
        Budget { map: HashMap::new(), bytes: 0, limit, clock: 0 }
    }

    pub(crate) fn get<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        let slot = self.map.get_mut(key)?;
        self.clock += 1;
        slot.used = self.clock;
        slot.commit = rune::commit_generation();
        Some(&slot.value)
    }

    /// Looks `key` up without counting it as used.
    pub(crate) fn peek<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.map.get(key).map(|s| &s.value)
    }

    /// Drops `key`, returning whether it was there.
    pub(crate) fn remove<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        let Some(slot) = self.map.remove(key) else { return false };
        self.bytes -= slot.bytes;
        true
    }

    /// Whether `key` was used in the commit running now (never outside one).
    pub(crate) fn used_now<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        let now = rune::commit_generation();
        now > 0 && self.map.get(key).is_some_and(|s| s.commit == now)
    }

    /// Estimated bytes held.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    /// Drops entries not used in the last commit and shrinks sparse bucket capacity at rest.
    pub(crate) fn rest(&mut self) {
        let now = rune::commit_generation();
        self.map.retain(|_, s| now > 0 && s.commit == now);
        self.bytes = self.map.values().map(|s| s.bytes).sum();
        let cap = (self.map.len().saturating_mul(2)).max(8);
        if self.map.capacity() > cap {
            self.map.shrink_to(cap);
        }
    }

    pub(crate) fn insert(&mut self, key: K, value: V, bytes: usize) {
        self.clock += 1;
        let slot = Slot { value, bytes, used: self.clock, commit: rune::commit_generation() };
        if let Some(old) = self.map.insert(key, slot) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        if self.bytes > self.limit {
            self.evict();
        }
    }

    /// Drops the least recently used entries the last two commits didn't use, down to three
    /// quarters of the budget so the next few inserts don't evict again.
    fn evict(&mut self) {
        let now = rune::commit_generation();
        // Outside any commit (a test, a tool) nothing counts as on screen.
        let kept = |s: &Slot<V>| now > 0 && s.commit + 1 >= now;
        let mut old: Vec<(u64, usize)> = self.map.values().filter(|s| !kept(s)).map(|s| (s.used, s.bytes)).collect();
        old.sort_unstable();
        let target = self.limit / 4 * 3;
        let mut bytes = self.bytes;
        // Every lookup gets its own stamp, so evicting up to a stamp evicts exactly these.
        let mut last = None;
        for (used, b) in old {
            if bytes <= target {
                break;
            }
            bytes -= b;
            last = Some(used);
        }
        let Some(last) = last else { return };
        self.map.retain(|_, s| kept(s) || s.used > last);
        self.bytes = bytes;
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_last_two_commits_used_outlives_the_budget() {
        let mut rt = rune::Runtime::new((), |_: &(), _: &mut rune::Cx<()>| rune::el(), (10.0, 10.0), rune::Env::default());
        let mut commit = || {
            rt.invalidate();
            rt.frame(0.01);
        };
        commit();
        let mut b = Budget::new(100);
        for k in ["a", "b", "c"] {
            b.insert(k, (), 60);
        }
        assert_eq!(b.map.len(), 3, "all three are on screen");
        commit();
        assert!(b.get("c").is_some());
        commit();
        b.insert("d", (), 60);
        // a and b are two commits old; c was used last commit.
        let mut left: Vec<_> = b.map.keys().copied().collect();
        left.sort();
        assert_eq!(left, ["c", "d"]);
        assert_eq!(b.bytes, 120);
        commit();
        commit();
        b.insert("e", (), 10);
        let mut left: Vec<_> = b.map.keys().copied().collect();
        left.sort();
        assert_eq!(left, ["d", "e"], "the least recently used go until under three quarters");
        assert_eq!(b.bytes, 70);
    }
}
