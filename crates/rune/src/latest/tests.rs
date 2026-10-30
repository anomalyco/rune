use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use crate::{Cx, El, Env, Runtime, el, text};

/// A waker that counts its calls.
fn counter() -> (Waker, Arc<AtomicUsize>) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = wakes.clone();
    (Arc::new(move || { count.fetch_add(1, Ordering::SeqCst); }), wakes)
}

fn pool() -> Arc<Pool> {
    Pool::new(4, Duration::from_millis(50))
}

fn latest<T: Send + 'static>() -> (Latest<T>, Arc<AtomicUsize>) {
    let (waker, wakes) = counter();
    (Latest::on(pool(), Some(waker)), wakes)
}

fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Holds a request's work, once started, until the test opens the gate.
struct Gate { open: mpsc::Sender<()>, started: Arc<AtomicUsize> }

fn gate() -> (Gate, impl FnOnce() + Send + 'static) {
    let (open, wait) = mpsc::channel();
    let started = Arc::new(AtomicUsize::new(0));
    let flag = started.clone();
    (Gate { open, started }, move || { flag.store(1, Ordering::SeqCst); let _ = wait.recv(); })
}

impl Gate {
    fn wait_started(&self) {
        eventually("the work starts", || self.started.load(Ordering::SeqCst) == 1);
    }
    fn open(&self) {
        self.open.send(()).unwrap();
    }
}

#[test]
fn the_newest_answer_wins_when_an_older_one_finishes_last() {
    let (search, wakes) = latest::<String>();
    let (open_a, wait_a) = gate();
    let a_done = Arc::new(AtomicUsize::new(0));
    let done = a_done.clone();
    search.request("a", move |q| { wait_a(); done.fetch_add(1, Ordering::SeqCst); q.to_owned() });
    open_a.wait_started();
    search.request("ab", |q| q.to_owned());
    eventually("ab lands", || wakes.load(Ordering::SeqCst) == 1);
    assert_eq!(search.take().as_deref(), Some("ab"));

    open_a.open();
    eventually("a finishes", || a_done.load(Ordering::SeqCst) == 1);
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(search.take(), None, "the older answer is dropped");
    assert_eq!(wakes.load(Ordering::SeqCst), 1, "and wakes nothing");
}

#[test]
fn a_newer_request_drops_an_answer_not_yet_taken() {
    let (search, wakes) = latest::<&str>();
    search.request("a", |q| q);
    eventually("a lands", || wakes.load(Ordering::SeqCst) == 1);
    let (open, wait) = gate();
    search.request("ab", move |q| { wait(); q });
    assert_eq!(search.take(), None, "a is no longer the newest ask");
    assert!(search.is_pending());
    open.open();
    eventually("ab lands", || wakes.load(Ordering::SeqCst) == 2);
    assert_eq!(search.take(), Some("ab"));
    assert!(!search.is_pending());
}

#[test]
fn an_answer_is_taken_once_and_each_landing_wakes_once() {
    let (search, wakes) = latest::<usize>();
    for n in 1..=20 {
        search.request(n, |n| n * 10);
        eventually("the answer lands", || wakes.load(Ordering::SeqCst) == n);
        assert_eq!(search.take(), Some(n * 10));
        assert_eq!(search.take(), None, "taken once");
    }
    std::thread::sleep(Duration::from_millis(10));
    assert_eq!(wakes.load(Ordering::SeqCst), 20, "one wake per answer");
}

#[test]
fn debounce_runs_only_the_last_request_of_a_burst_after_a_quiet_wait() {
    let (waker, wakes) = counter();
    let search = Latest::on(pool(), Some(waker)).debounce(Duration::from_millis(40));
    let ran = Arc::new(Mutex::new(Vec::new()));
    let start = Instant::now();
    for q in ["a", "ab", "abc"] {
        let ran = ran.clone();
        search.request(q, move |q| { ran.lock().unwrap().push((q, start.elapsed())); q });
        std::thread::sleep(Duration::from_millis(10));
    }
    eventually("abc lands", || wakes.load(Ordering::SeqCst) == 1);
    assert_eq!(search.take(), Some("abc"));
    let ran = ran.lock().unwrap();
    assert_eq!(ran.iter().map(|(q, _)| *q).collect::<Vec<_>>(), ["abc"], "superseded requests never ran");
    // Asked at about 20 ms, so it waits until about 60 ms: the debounce restarted on each key.
    assert!(ran[0].1 >= Duration::from_millis(55), "started after the quiet wait: {:?}", ran[0].1);
}

#[test]
fn debounce_waits_before_a_lone_request() {
    let (waker, wakes) = counter();
    let search = Latest::on(pool(), Some(waker)).debounce(Duration::from_millis(30));
    let start = Instant::now();
    search.request((), move |_| start.elapsed());
    assert!(search.is_pending());
    eventually("it lands", || wakes.load(Ordering::SeqCst) == 1);
    assert!(search.take().unwrap() >= Duration::from_millis(30));
}

#[test]
fn cancel_drops_pending_and_running_requests() {
    let (waker, wakes) = counter();
    let search = Latest::on(pool(), Some(waker)).debounce(Duration::from_millis(20));
    let (open, wait) = gate();
    let finished = Arc::new(AtomicUsize::new(0));
    let done = finished.clone();
    search.request((), move |_| { wait(); done.fetch_add(1, Ordering::SeqCst); 1 });
    open.wait_started();
    search.cancel();
    assert!(!search.is_pending());
    open.open();
    eventually("the running work finishes", || finished.load(Ordering::SeqCst) == 1);

    let ran = Arc::new(AtomicUsize::new(0));
    let counted = ran.clone();
    search.request((), move |_| { counted.fetch_add(1, Ordering::SeqCst); 2 });
    search.cancel();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(ran.load(Ordering::SeqCst), 0, "a debounced request cancelled before it started never runs");
    assert_eq!((search.take(), wakes.load(Ordering::SeqCst)), (None, 0), "nothing lands or wakes");
}

#[test]
fn dropping_it_cancels_and_nothing_wakes_after() {
    let (search, wakes) = latest::<u32>();
    let (open, wait) = gate();
    let finished = Arc::new(AtomicUsize::new(0));
    let done = finished.clone();
    search.request((), move |_| { wait(); done.fetch_add(1, Ordering::SeqCst); 1 });
    open.wait_started();
    drop(search);
    open.open();
    eventually("the running work finishes", || finished.load(Ordering::SeqCst) == 1);
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(wakes.load(Ordering::SeqCst), 0);
}

#[test]
fn a_panicking_request_settles_and_the_worker_carries_on() {
    let pool = Pool::new(1, Duration::from_millis(50));
    let (waker, wakes) = counter();
    let search = Latest::on(pool.clone(), Some(waker));
    search.request((), |_| -> u32 { panic!("search failed") });
    eventually("the panic settles it", || !search.is_pending());
    search.request((), |_| 7);
    eventually("the next lands", || wakes.load(Ordering::SeqCst) == 1);
    assert_eq!(search.take(), Some(7));
    assert_eq!(pool.threads().1, 1, "on the same worker");
}

/// One request per keystroke across several lists, some slow: the pool never grows past its
/// cap, and its threads all exit once idle.
#[test]
fn requests_reuse_a_few_threads_that_exit_when_idle() {
    let pool = Pool::new(2, Duration::from_millis(500));
    let lists: Vec<_> = (0..3).map(|_| Latest::on(pool.clone(), None)).collect();
    for key in 0..200u64 {
        let list = &lists[key as usize % 3];
        list.request(key, move |k| {
            if k % 50 == 0 { std::thread::sleep(Duration::from_millis(5)); }
            k
        });
        assert!(pool.threads().0 <= 2);
    }
    eventually("every list settles", || lists.iter().all(|l| !l.is_pending()));
    assert_eq!(lists[197 % 3].take(), Some(197));
    assert!(pool.threads().1 <= 2, "started {} threads", pool.threads().1);
    eventually("the workers exit", || pool.threads().0 == 0);
}

/// A window's `poll` that takes the answer commits once per landed answer and stays idle
/// between them.
#[test]
fn poll_changes_state_once_per_answer() {
    struct Search { hits: String, results: Latest<String> }
    fn view(s: &Search, _: &mut Cx<Search>) -> El { el().child(text(s.hits.clone())) }
    let (waker, wakes) = counter();
    let state = Search { hits: String::new(), results: Latest::on(pool(), Some(waker)) };
    let mut rt = Runtime::new(state, view, (200.0, 100.0), Env::default());
    rt.poll = Some(|s, _, _| {
        let Some(hits) = s.results.take() else { return false };
        s.hits = hits;
        true
    });
    rt.frame(0.0);
    let commits = rt.stats.commits;
    rt.state.results.request("rune".to_owned(), |q| format!("{q}.md"));
    eventually("the answer lands", || wakes.load(Ordering::SeqCst) == 1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hits, "rune.md");
    assert_eq!(rt.stats.commits, commits + 1, "the landed answer commits");
    rt.frame(2.0 / 60.0);
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.stats.commits, commits + 1, "and nothing after it");
}
