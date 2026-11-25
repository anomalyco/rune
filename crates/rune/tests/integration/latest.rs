use std::sync::{Arc, mpsc};

use rune::{Cx, El, Env, Latest, Runtime, el, text};

/// A window's `poll` that takes the answer commits once per landed answer and stays idle
/// between them.
#[test]
fn poll_changes_state_once_per_answer() {
    struct Search { hits: String, results: Latest<String> }
    fn view(s: &Search, _: &mut Cx<Search>) -> El { el().child(text(s.hits.clone())) }
    let (woke_tx, woke_rx) = mpsc::channel();
    let waker = Arc::new(move || { woke_tx.send(()).ok(); });
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
    woke_rx.recv().expect("answer lands and wakes");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hits, "rune.md");
    assert_eq!(rt.stats.commits, commits + 1, "the landed answer commits");
    rt.frame(2.0 / 60.0);
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.stats.commits, commits + 1, "and nothing after it");
}
