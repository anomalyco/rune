use super::*;

fn find<S: 'static>(rt: &Runtime<S>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

/// A pill (default transition: a fade) over a row that its mount pushes down.
fn view(shown: &bool, _: &mut Cx<bool>) -> El {
    col().size(200.0, 200.0)
        .children(shown.then(|| el().key("pill").size(100.0, 40.0)))
        .child(el().key("row").size(100.0, 40.0))
}

fn app(needs_cause: bool) -> Runtime<bool> {
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.motion_needs_cause = needs_cause;
    rt.frame(0.0);
    rt
}

#[test]
fn what_no_input_caused_appears_and_goes_in_its_final_state() {
    let mut rt = app(true);
    rt.update(|s, _| *s = true);
    rt.frame(0.0);
    assert_eq!(find(&rt, "pill").unwrap().opacity, 1.0, "whole in the frame it mounts");
    let row = find(&rt, "row").unwrap();
    assert!(row.rect[1] < row.target[1] - 1.0, "what was already there keeps its spring: {:?}", row.rect);
    rt.update(|s, _| *s = false);
    rt.frame(0.0);
    assert!(find(&rt, "pill").is_none_or(|p| p.opacity == 0.0), "gone in the frame it's removed");
}

#[test]
fn a_handler_that_asks_for_motion_keeps_the_entrance() {
    let mut rt = app(true);
    rt.update(|s, tx| {
        *s = true;
        tx.animated();
    });
    rt.frame(0.0);
    assert!(find(&rt, "pill").unwrap().opacity < 1.0, "fades in");
}

#[test]
fn a_bare_runtime_keeps_every_commits_motion() {
    let mut rt = app(false);
    rt.update(|s, _| *s = true);
    rt.frame(0.0);
    assert!(find(&rt, "pill").unwrap().opacity < 1.0, "fades in");
}
