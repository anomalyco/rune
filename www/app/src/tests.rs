use rune::{Env, Runtime};

use super::*;
use crate::code::{HELLO_TEST, HELLO_VIEW, RETARGET};

fn page(width: f32) -> Runtime<State> {
    let mut rt = Runtime::new(State::new(), view, (width, 900.0), Env::new(true, 2.0));
    for _ in 0..3 {
        rt.frame(0.0);
    }
    rt
}

fn paths(rt: &Runtime<State>) -> Vec<String> {
    rt.elements().into_iter().filter(|e| !e.exiting).map(|e| e.path).collect()
}

#[test]
fn every_snippet_still_cuts() {
    for s in [&HELLO_VIEW, &HELLO_TEST, &RETARGET] {
        let cut = s.cut().unwrap_or_else(|| panic!("{}: a marker stopped matching", s.file));
        assert!(cut.start <= cut.end && cut.code.lines().count() > 3, "{}: {cut:?}", s.file);
        assert_eq!(cut.code.matches('{').count(), cut.code.matches('}').count(), "{}: braces balance\n{}", s.file, cut.code);
    }
}

/// The static page keeps the narrowest width's tree and switches styles with `@media`, so every
/// width must build the same elements.
#[test]
fn every_breakpoint_builds_the_same_tree() {
    let narrow = paths(&page(360.0));
    assert!(narrow.len() > 100, "{} elements", narrow.len());
    for w in [400.0, 560.0, 760.0, 860.0, 1000.0, 1400.0] {
        let wide = paths(&page(w));
        let only: Vec<_> = wide.iter().filter(|p| !narrow.contains(p)).chain(narrow.iter().filter(|p| !wide.contains(p))).collect();
        assert!(only.is_empty(), "at {w}: {only:?}");
    }
}

#[test]
fn the_breakpoint_follows_the_window() {
    let mut rt = page(1200.0);
    let hero = |rt: &Runtime<State>| rt.presented("hero-text").unwrap().0;
    let wide = hero(&rt);
    rt.set_viewport((390.0, 900.0), 2.0);
    for _ in 0..3 {
        rt.frame(0.0);
    }
    let narrow = hero(&rt);
    assert!(wide[2] < 1200.0 - 520.0, "the card sits beside the text: {wide:?}");
    assert!((narrow[2] - (390.0 - 32.0)).abs() < 1.0, "and under it on a phone: {narrow:?}");
}

#[test]
fn the_cargo_line_names_the_repo_and_a_rev() {
    let line = sections::cargo_line();
    assert!(line.starts_with("rune = { git = \"https://github.com/anomalyco/rune\", rev = \""), "{line}");
}
