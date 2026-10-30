//! [`Kept`]: a build reused by a commit that changed no state, such as the commits a virtual
//! list's window shift causes while it scrolls.
//!
//! The view and the rows `El::virtual_rows` builds are functions of the app state and what their
//! `Cx` reads return. A commit the frame loop marks state-unchanged (no handler ran, nothing
//! invalidated) only needs them again when a read moved, so it takes them from the last build
//! and builds only the rows its window newly mounts. Unlike [`memo`](super::memo), a kept build
//! names no memo scope, so the memos inside keep the ids they have in an ordinary build.

use std::rc::Rc;

use super::{Read, SCOPES, Scope, keep_nested, replay};

/// One build kept for state-unchanged commits.
pub(crate) struct Kept<T> {
    built: Option<Build<T>>,
}

struct Build<T> {
    value: T,
    /// The `Cx` reads it made, its memos' included.
    reads: Vec<Read>,
    /// The memos it built or reused at its top level, kept while it is reused.
    nested: Vec<Rc<str>>,
    env: u64,
    /// [`super::MemoCache`]'s stamp when it built: a later stamp means state may have changed.
    stamp: u64,
}

impl<T> Default for Kept<T> {
    fn default() -> Self { Kept { built: None } }
}

impl<T> std::fmt::Debug for Kept<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.built.is_some() { "Kept(built)" } else { "Kept(empty)" })
    }
}

impl<T: Clone> Kept<T> {
    /// Whether nothing is kept.
    pub(crate) fn is_empty(&self) -> bool { self.built.is_none() }

    /// What `build` returned the last time, if this view build changes no state since and every
    /// `Cx` read it made still holds; otherwise `build`'s value, kept only when this view build
    /// changes no state. Outside a view build it just builds.
    pub(crate) fn get(&mut self, build: impl FnOnce() -> T) -> T {
        enum Step<T> { Plain, Reuse(T), Build }
        let step = SCOPES.with_borrow_mut(|scopes| {
            let Some(s) = scopes.last_mut() else { return Step::Plain };
            if !s.unchanged { return Step::Plain }
            if let Some(b) = &self.built
                && b.stamp == s.cache.stamp
                && b.env == s.env
                && b.reads.iter().all(|r| s.seen.holds(r))
            {
                let pass = s.cache.pass;
                keep_nested(&mut s.cache, b.nested.clone(), pass);
                if let Some(parent) = s.nested.last_mut() { parent.extend(b.nested.iter().cloned()); }
                s.note(&b.reads);
                replay(s, b.reads.iter().cloned());
                return Step::Reuse(b.value.clone());
            }
            s.nested.push(Vec::new());
            s.reads.push(Vec::new());
            Step::Build
        });
        match step {
            Step::Reuse(value) => value,
            Step::Plain => {
                self.built = None;
                build()
            }
            Step::Build => {
                let value = build();
                SCOPES.with_borrow_mut(|scopes| {
                    let s: &mut Scope = scopes.last_mut().expect("memo scope");
                    let nested = s.nested.pop().unwrap_or_default();
                    let reads = s.reads.pop().unwrap_or_default();
                    s.note(&reads);
                    if let Some(parent) = s.nested.last_mut() { parent.extend(nested.iter().cloned()); }
                    self.built = Some(Build { value: value.clone(), reads, nested, env: s.env, stamp: s.cache.stamp });
                });
                value
            }
        }
    }
}
