//! Bounded per-locale cache and off-thread prewarm pool for Wayland XKB compose tables,
//! kept cross-platform so reuse, fallback, concurrency and eviction tests run on every host.

use std::sync::{Arc, Condvar, Mutex};

/// Maximum resolved locales retained in the UI thread's compose-table cache.
pub(crate) const MAX_LOCALES: usize = 8;

/// Outcome of compiling one locale's compose table (with optional `"C"` fallback).
#[derive(Debug)]
pub(crate) enum Compiled<T> {
    Primary(T),
    FallbackC(Option<T>),
    Miss,
}

impl<T> Compiled<T> {
    pub(crate) fn map<U>(self, f: impl FnOnce(T) -> U) -> Compiled<U> {
        match self {
            Self::Primary(t) => Compiled::Primary(f(t)),
            Self::FallbackC(t) => Compiled::FallbackC(t.map(f)),
            Self::Miss => Compiled::Miss,
        }
    }
}

pub(crate) fn compile_for_locale<T>(locale: &str, try_c_fallback: bool, mut compile: impl FnMut(&str) -> Option<T>) -> Compiled<T> {
    if let Some(table) = compile(locale) { Compiled::Primary(table) }
    else if try_c_fallback && locale != "C" { Compiled::FallbackC(compile("C")) }
    else { Compiled::Miss }
}

/// Bounded LRU cache of compiled compose tables keyed by resolved locale string.
pub(crate) struct TableCache<T> {
    cap: usize,
    /// Ordered least-recently used (front) to most-recently used (back).
    entries: Vec<(String, Option<T>)>,
}

impl<T> TableCache<T> {
    pub(crate) fn new(cap: usize) -> Self { Self { cap: cap.max(1), entries: Vec::new() } }
    pub(crate) fn cap(&self) -> usize { self.cap }
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize { self.entries.len() }
    pub(crate) fn contains(&self, locale: &str) -> bool { self.entries.iter().any(|(k, _)| k == locale) }
    #[cfg(test)]
    pub(crate) fn clear(&mut self) { self.entries.clear(); }

    pub(crate) fn insert(&mut self, locale: String, table: Option<T>) {
        if let Some(pos) = self.entries.iter().position(|(k, _)| k == &locale) { self.entries.remove(pos); }
        while self.entries.len() >= self.cap { self.entries.remove(0); }
        self.entries.push((locale, table));
    }
}

impl<T: Clone> TableCache<T> {
    pub(crate) fn get(&mut self, locale: &str) -> Option<Option<T>> {
        let pos = self.entries.iter().position(|(k, _)| k == locale)?;
        let entry = self.entries.remove(pos);
        let value = entry.1.clone();
        self.entries.push(entry);
        Some(value)
    }

    pub(crate) fn apply_compiled(&mut self, locale: &str, compiled: Compiled<T>, mut compile_c: impl FnMut() -> Option<T>) -> Option<T> {
        match compiled {
            Compiled::Primary(table) => { self.insert(locale.to_owned(), Some(table.clone())); Some(table) }
            Compiled::FallbackC(maybe_c) => {
                self.insert("C".to_owned(), maybe_c.clone());
                if locale != "C" { self.insert(locale.to_owned(), maybe_c.clone()); }
                maybe_c
            }
            Compiled::Miss if locale == "C" => { self.insert("C".to_owned(), None); None }
            Compiled::Miss => {
                let fallback = self.get("C").unwrap_or_else(|| {
                    let compiled = compile_c();
                    self.insert("C".to_owned(), compiled.clone());
                    compiled
                });
                self.insert(locale.to_owned(), fallback.clone());
                fallback
            }
        }
    }

    pub(crate) fn resolve(&mut self, locale: &str, mut compile: impl FnMut(&str) -> Option<T>) -> Option<T> {
        if let Some(cached) = self.get(locale) { return cached; }
        let try_c = locale != "C" && !self.contains("C");
        let compiled = compile_for_locale(locale, try_c, &mut compile);
        self.apply_compiled(locale, compiled, || compile("C"))
    }
}

/// Outcome of a non-blocking lookup on [`ComposePool`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Lookup<T> { Ready(Option<T>), Pending }

enum JobState<S> { Running, Done(Compiled<S>), Empty }

struct Job<S> { locale: String, shared: Arc<(Mutex<JobState<S>>, Condvar)> }

/// Wakes waiters if a worker thread exits unexpectedly before writing `JobState::Done`.
struct FinishGuard<S>(Option<Arc<(Mutex<JobState<S>>, Condvar)>>);
impl<S> Drop for FinishGuard<S> {
    fn drop(&mut self) {
        if let Some(shared) = self.0.take() {
            let mut state = shared.0.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*state, JobState::Running) { *state = JobState::Empty; shared.1.notify_all(); }
        }
    }
}

/// Coordinates a bounded UI-thread [`TableCache`] with off-thread prewarming.
pub(crate) struct ComposePool<T, S> { cache: TableCache<T>, in_flight: Vec<Job<S>> }

impl<T: Clone, S: Send + 'static> ComposePool<T, S> {
    pub(crate) fn new(cap: usize) -> Self { Self { cache: TableCache::new(cap), in_flight: Vec::new() } }
    #[cfg(test)]
    pub(crate) fn cached_len(&self) -> usize { self.cache.len() }
    #[cfg(test)]
    pub(crate) fn in_flight_len(&self) -> usize { self.in_flight.len() }
    #[cfg(test)]
    pub(crate) fn clear(&mut self) { self.cache.clear(); self.in_flight.clear(); }

    fn sweep_ready(&mut self, into_table: &impl Fn(S) -> T) {
        let mut idx = 0;
        while idx < self.in_flight.len() {
            let Ok(mut state) = self.in_flight[idx].shared.0.try_lock() else { idx += 1; continue; };
            match &*state {
                JobState::Running => { drop(state); idx += 1; }
                JobState::Empty => { drop(state); self.in_flight.remove(idx); }
                JobState::Done(compiled) => {
                    if matches!(compiled, Compiled::Miss) && self.in_flight[idx].locale != "C" && !self.cache.contains("C") {
                        drop(state); idx += 1; continue;
                    }
                    let taken = std::mem::replace(&mut *state, JobState::Empty);
                    drop(state);
                    let job = self.in_flight.remove(idx);
                    if let JobState::Done(compiled) = taken {
                        let _ = self.cache.apply_compiled(&job.locale, compiled.map(into_table), || None);
                    }
                }
            }
        }
    }

    pub(crate) fn warm<F>(&mut self, locale: &str, into_table: impl Fn(S) -> T, compile_worker: F) -> bool
    where F: FnOnce(&str, bool) -> Compiled<S> + Send + 'static {
        self.sweep_ready(&into_table);
        if self.cache.contains(locale) || self.in_flight.iter().any(|j| j.locale == locale) || self.in_flight.len() >= self.cache.cap() {
            return false;
        }
        let try_c = locale != "C" && !self.cache.contains("C");
        let shared = Arc::new((Mutex::new(JobState::Running), Condvar::new()));
        let worker_shared = Arc::clone(&shared);
        let worker_locale = locale.to_owned();
        let spawned = std::thread::Builder::new().name("rune-xkb-compose".into()).spawn(move || {
            let mut guard = FinishGuard(Some(Arc::clone(&worker_shared)));
            let compiled = compile_worker(&worker_locale, try_c);
            let mut state = worker_shared.0.lock().unwrap_or_else(|e| e.into_inner());
            *state = JobState::Done(compiled);
            guard.0 = None;
            worker_shared.1.notify_all();
        });
        if spawned.is_ok() { self.in_flight.push(Job { locale: locale.to_owned(), shared }); true } else { false }
    }

    pub(crate) fn try_get<F>(&mut self, locale: &str, into_table: impl Fn(S) -> T, compile_worker: F, mut sync_compile: impl FnMut(&str) -> Option<T>) -> Lookup<T>
    where F: FnOnce(&str, bool) -> Compiled<S> + Send + 'static {
        self.sweep_ready(&into_table);
        if let Some(cached) = self.cache.get(locale) { return Lookup::Ready(cached); }
        if let Some(pos) = self.in_flight.iter().position(|j| j.locale == locale) {
            let mut state = self.in_flight[pos].shared.0.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                JobState::Running => return Lookup::Pending,
                JobState::Done(_) => {
                    let taken = std::mem::replace(&mut *state, JobState::Empty);
                    drop(state);
                    let job = self.in_flight.remove(pos);
                    if let JobState::Done(compiled) = taken {
                        return Lookup::Ready(self.cache.apply_compiled(&job.locale, compiled.map(&into_table), || sync_compile("C")));
                    }
                }
                JobState::Empty => { drop(state); self.in_flight.remove(pos); }
            }
        }
        if self.warm(locale, &into_table, compile_worker) { Lookup::Pending }
        else { Lookup::Ready(self.cache.resolve(locale, sync_compile)) }
    }

    pub(crate) fn wait(&mut self, locale: &str, into_table: impl Fn(S) -> T, mut sync_compile: impl FnMut(&str) -> Option<T>) -> Option<T> {
        self.sweep_ready(&into_table);
        if let Some(cached) = self.cache.get(locale) { return cached; }
        if let Some(pos) = self.in_flight.iter().position(|j| j.locale == locale) {
            let job = self.in_flight.remove(pos);
            let mut state = job.shared.0.lock().unwrap_or_else(|e| e.into_inner());
            while matches!(*state, JobState::Running) {
                state = job.shared.1.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            let taken = std::mem::replace(&mut *state, JobState::Empty);
            drop(state);
            if let JobState::Done(compiled) = taken {
                return self.cache.apply_compiled(&job.locale, compiled.map(&into_table), || sync_compile("C"));
            }
        }
        self.cache.resolve(locale, sync_compile)
    }
}

#[cfg(test)]
#[path = "compose_cache_tests.rs"]
mod tests;
