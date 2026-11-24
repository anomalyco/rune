//! Hash-indexed row-key lookup and single-pass duplicate key renaming for virtual lists.

use crate::runtime::HashMap;

/// Each row's place in `keys`, found by its key's hash: a table of indices, not a second copy
/// of every key, so a list of 100,000 rows takes new keys without 100,000 allocations. A key whose
/// hash another key took first is kept by name.
#[derive(Debug, Default)]
pub(super) struct Places {
    by_hash: HashMap<u64, usize>,
    collided: HashMap<String, usize>,
}

impl Places {
    pub(super) fn with_capacity(cap: usize) -> Self {
        Places {
            by_hash: HashMap::with_capacity_and_hasher(cap, Default::default()),
            collided: HashMap::default(),
        }
    }

    pub(super) fn hash(key: &str) -> u64 {
        use std::hash::{BuildHasher, Hash, Hasher};
        let mut h = rustc_hash::FxBuildHasher.build_hasher();
        key.hash(&mut h);
        h.finish()
    }

    /// Builds `Places` for `keys` in one pass, renaming any later duplicate `key#dup1`,
    /// `key#dup2`, ... in place (skipping names already taken). Returns `(places, renamed)`
    /// where each entry in `renamed` is `(index, original_key)`.
    pub(super) fn from_keys_deduping(keys: &mut [String]) -> (Places, Vec<(usize, String)>) {
        Self::from_keys_deduping_with(keys, Self::hash)
    }

    pub(super) fn from_keys_deduping_with(keys: &mut [String], hash_fn: impl Fn(&str) -> u64) -> (Places, Vec<(usize, String)>) {
        let mut places = Places::with_capacity(keys.len());
        let mut dups = Vec::new();
        for (i, key) in keys.iter().enumerate() {
            if !places.insert_or_dup(keys, key, hash_fn(key), i) {
                dups.push(i);
            }
        }
        let renamed = if dups.is_empty() {
            Vec::new()
        } else {
            places.finish_deduping(keys, &dups, hash_fn)
        };
        (places, renamed)
    }

    /// Renames each duplicate index in `dups` (ascending order) to the first free `key#dupN`
    /// name and indexes it in `self`. `self` must already hold every unique original key in
    /// `keys` (at its first occurrence, so no index in `dups` is referenced by `self`).
    pub(super) fn finish_deduping(
        &mut self,
        keys: &mut [String],
        dups: &[usize],
        hash_fn: impl Fn(&str) -> u64,
    ) -> Vec<(usize, String)> {
        let mut next_dup: HashMap<String, usize> = HashMap::default();
        let mut renamed = Vec::with_capacity(dups.len());
        for &i in dups {
            let mut n = next_dup.get(&keys[i]).copied().unwrap_or(1);
            let name = loop {
                let candidate = format!("{}#dup{n}", keys[i]);
                if self.get_hashed(keys, &candidate, hash_fn(&candidate)).is_none() {
                    break candidate;
                }
                n += 1;
            };
            let old = std::mem::replace(&mut keys[i], name);
            let hash = hash_fn(&keys[i]);
            self.insert_hashed(keys, &keys[i], hash, i);
            next_dup.insert(old.clone(), n + 1);
            renamed.push((i, old));
        }
        renamed
    }

    pub(super) fn reserve(&mut self, additional: usize) {
        self.by_hash.reserve(additional);
    }

    /// Inserts `key` at `i` if absent from `keys[..i]`; returns `false` when `key` duplicates
    /// an earlier key.
    pub(super) fn insert_or_dup(&mut self, keys: &[String], key: &str, hash: u64, i: usize) -> bool {
        match self.by_hash.entry(hash) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(i);
                true
            }
            std::collections::hash_map::Entry::Occupied(e) => {
                if keys[*e.get()] == key || self.collided.contains_key(key) {
                    return false;
                }
                self.collided.insert(key.to_owned(), i);
                true
            }
        }
    }

    /// Inserts `key` at index `i`, assuming `key` is not already present in `keys[..i]`.
    pub(super) fn insert(&mut self, keys: &[String], key: &str, i: usize) {
        self.insert_hashed(keys, key, Self::hash(key), i);
    }

    pub(super) fn insert_hashed(&mut self, keys: &[String], key: &str, hash: u64, i: usize) {
        match self.by_hash.entry(hash) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(i);
            }
            std::collections::hash_map::Entry::Occupied(e) => {
                debug_assert_ne!(keys[*e.get()], key, "virtual row keys must be unique");
                let prev = self.collided.insert(key.to_owned(), i);
                debug_assert!(prev.is_none(), "virtual row keys must be unique");
            }
        }
    }

    /// Where `key` is in `keys` (those `self` was made of).
    pub(super) fn get(&self, keys: &[String], key: &str) -> Option<usize> {
        self.get_hashed(keys, key, Self::hash(key))
    }

    pub(super) fn get_hashed(&self, keys: &[String], key: &str, hash: u64) -> Option<usize> {
        let &i = self.by_hash.get(&hash)?;
        if keys[i] == key {
            return Some(i);
        }
        if self.collided.is_empty() {
            return None;
        }
        self.collided.get(key).copied()
    }
}
