use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rune::{Cx, El, Env, Latest, Runtime, el, text};

/// A window's `poll` that takes the answer commits once per landed answer and stays idle
/// between them.
#[test]
fn poll_changes_state_once_per_answer() {
    struct Search { hits: String, results: Latest<String> }
    fn view(s: &Search, _: &mut Cx<Search>) -> El { el().child(text(s.hits.clone())) }
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = wakes.clone();
    let waker = Arc::new(move || { count.fetch_add(1, Ordering::SeqCst); });
    let state = Search { hits: String::new(), results: Latest::new(waker) };
    let mut rt = Runtime::new(state, view, (200.0, 100.0), Env::default());
    rt.poll = Some(|s, _, _| {
        let Some(hits) = s.results.take() else { return false };
        s.hits = hits;
        true
    });
    rt.frame(0.0);
    let commits = rt.stats.commits;
    rt.state.results.request("rune".to_owned(), |q| format!("{q}.md"));
    let deadline = rune::time::now() + Duration::from_secs(5);
    while wakes.load(Ordering::SeqCst) != 1 {
        assert!(rune::time::now() < deadline, "timed out waiting until the answer lands");
        std::thread::sleep(Duration::from_millis(1));
    }
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hits, "rune.md");
    assert_eq!(rt.stats.commits, commits + 1, "the landed answer commits");
    rt.frame(2.0 / 60.0);
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.stats.commits, commits + 1, "and nothing after it");
}
