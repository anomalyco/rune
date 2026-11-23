use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use super::{ComposePool, Lookup, MAX_LOCALES, TableCache, compile_for_locale};

#[derive(Debug, PartialEq, Eq)]
struct FakeTable { locale: String, compile_id: usize }

#[test]
fn table_cache_compiles_once_per_locale_shares_c_fallback_and_evicts_lru_at_cap() {
    let mut cache: TableCache<Arc<FakeTable>> = TableCache::new(3);
    let (compiles, c_compiles) = (Cell::new(0usize), Cell::new(0usize));
    let mut compile = |locale: &str| -> Option<Arc<FakeTable>> {
        let id = compiles.get() + 1;
        compiles.set(id);
        if locale == "C" { c_compiles.set(c_compiles.get() + 1); }
        matches!(locale, "en_US.UTF-8" | "fr_FR.UTF-8" | "C")
            .then(|| Arc::new(FakeTable { locale: locale.to_owned(), compile_id: id }))
    };

    let first = cache.resolve("en_US.UTF-8", &mut compile).expect("en_US table");
    let second = cache.resolve("en_US.UTF-8", &mut compile).expect("cached en_US table");
    assert_eq!(compiles.get(), 1, "second show for the same locale must not recompile");
    assert!(Arc::ptr_eq(&first, &second), "surfaces must share one compiled table");

    let missing = cache.resolve("xx_XX.UTF-8", &mut compile).expect("C fallback");
    assert_eq!(missing.locale, "C");
    let after_missing = compiles.get();
    assert!(Arc::ptr_eq(&missing, &cache.resolve("xx_XX.UTF-8", &mut compile).unwrap()));
    assert!(Arc::ptr_eq(&missing, &cache.resolve("C", &mut compile).unwrap()));
    assert_eq!((compiles.get(), c_compiles.get()), (after_missing, 1));

    // Promote en_US.UTF-8 so xx_XX.UTF-8 is LRU and gets evicted by yy_YY.UTF-8 (reusing C).
    assert!(Arc::ptr_eq(&first, &cache.resolve("en_US.UTF-8", &mut compile).unwrap()));
    assert!(Arc::ptr_eq(&missing, &cache.resolve("yy_YY.UTF-8", &mut compile).unwrap()));
    assert_eq!((cache.len(), c_compiles.get()), (3, 1));
    assert!(cache.contains("en_US.UTF-8") && cache.contains("C") && !cache.contains("xx_XX.UTF-8"));

    // Negative misses are cached too so missing system tables do not re-probe disk.
    let mut fail_cache: TableCache<Arc<FakeTable>> = TableCache::new(MAX_LOCALES);
    let mut calls = Vec::new();
    let mut fail = |loc: &str| { calls.push(loc.to_owned()); None };
    assert!(fail_cache.resolve("en_US.UTF-8", &mut fail).is_none());
    assert!(fail_cache.resolve("en_US.UTF-8", &mut fail).is_none());
    assert!(fail_cache.resolve("C", &mut fail).is_none());
    assert_eq!(calls, ["en_US.UTF-8", "C"]);
}

#[test]
fn compose_pool_warms_off_thread_deduplicates_and_bounds_in_flight_jobs() {
    let mut pool: ComposePool<Arc<FakeTable>, Arc<FakeTable>> = ComposePool::new(2);
    let ui_thread = thread::current().id();
    let worker_thread = Arc::new(Mutex::new(None));
    let compiles = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));

    let (wt, wc, wg) = (Arc::clone(&worker_thread), Arc::clone(&compiles), Arc::clone(&gate));
    assert!(pool.warm("en_US.UTF-8", |t| t, move |loc, try_c| {
        *wt.lock().unwrap() = Some(thread::current().id());
        let mut open = wg.0.lock().unwrap();
        while !*open { open = wg.1.wait(open).unwrap(); }
        compile_for_locale(loc, try_c, |l| {
            let id = wc.fetch_add(1, Ordering::SeqCst) + 1;
            Some(Arc::new(FakeTable { locale: l.to_owned(), compile_id: id }))
        })
    }));
    assert!(!pool.warm("en_US.UTF-8", |t| t, |_, _| panic!("duplicate warm")));
    assert_eq!(pool.try_get("en_US.UTF-8", |t| t, |_, _| panic!("dup"), |_| panic!("ui sync")), Lookup::Pending);

    // Fill second slot and verify in-flight cap rejects a third concurrent job.
    let wg2 = Arc::clone(&gate);
    assert!(pool.warm("fr_FR.UTF-8", |t| t, move |loc, try_c| {
        let mut open = wg2.0.lock().unwrap();
        while !*open { open = wg2.1.wait(open).unwrap(); }
        compile_for_locale(loc, try_c, |l| Some(Arc::new(FakeTable { locale: l.to_owned(), compile_id: 2 })))
    }));
    assert_eq!(pool.in_flight_len(), 2);
    assert!(!pool.warm("de_DE.UTF-8", |t| t, |_, _| panic!("exceeded cap")));

    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let first = pool.wait("en_US.UTF-8", |t| t, |_| panic!("unexpected sync")).expect("en_US");
    assert_ne!(*worker_thread.lock().unwrap(), Some(ui_thread), "must compile off UI thread");
    assert_eq!((first.compile_id, compiles.load(Ordering::SeqCst)), (1, 1));
    assert!(pool.wait("fr_FR.UTF-8", |t| t, |_| None).is_some());
    assert_eq!((pool.cached_len(), pool.in_flight_len()), (2, 0));

    // Subsequent shows hit cache immediately with zero compilations.
    assert!(!pool.warm("en_US.UTF-8", |t| t, |_, _| panic!("cached warm")));
    let Lookup::Ready(Some(second)) = pool.try_get("en_US.UTF-8", |t| t, |_, _| panic!("cached"), |_| panic!("sync"))
        else { panic!("expected immediate cache hit") };
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(compiles.load(Ordering::SeqCst), 1);

    // A panicking worker wakes waiters cleanly and falls back instead of deadlocking.
    assert!(pool.warm("panic_loc", |t| t, |_, _| panic!("worker boom")));
    let recovered = pool.wait("panic_loc", |t| t, |l| Some(Arc::new(FakeTable { locale: l.to_owned(), compile_id: 9 })));
    assert_eq!((recovered.expect("fallback").compile_id, pool.cached_len()), (9, 2));
    pool.clear();
    assert_eq!((pool.cached_len(), pool.in_flight_len()), (0, 0));
}
