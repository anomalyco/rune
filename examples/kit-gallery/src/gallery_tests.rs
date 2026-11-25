//! Regression tests for kit-gallery virtualization, resize relayout, and unique sibling keys.

use std::cell::Cell;

use super::*;

fn duplicate_paths(rt: &rune::Runtime<G>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| e.key.as_deref().is_some_and(|k| k.contains("#dup")) || e.path.contains("#dup"))
        .map(|e| e.path)
        .collect()
}

/// The gallery index mounts only rows in and near the viewport (not all 183 stories), and
/// resizing with or without an open combobox takes Rune's relayout fast path.
#[test]
fn index_virtualizes_stories_and_relayouts_on_resize() {
    thread_local! { static VIEWS: Cell<usize> = const { Cell::new(0) }; }
    fn counted(s: &G, cx: &mut Cx<G>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        view(s, cx)
    }

    let mut rt = rune::Runtime::new(init(), counted, (800.0, 560.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    // Let virtual_rows finish its initial spread overscan fill before measuring resizes.
    for _ in 0..8 {
        rt.frame(1.0 / 60.0);
    }
    assert!(
        rt.stats.layout_nodes < 1_000,
        "index should mount only visible stories + overscan, got {} nodes",
        rt.stats.layout_nodes,
    );

    // Horizontal drag resizes do not move the virtual window and should run zero views.
    let v0 = VIEWS.get();
    for w in [840.0, 900.0, 760.0, 820.0] {
        rt.set_viewport((w, 560.0), 1.0);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(VIEWS.get() - v0, 0, "horizontal index resizes should take Relayout without rebuilding view");

    // Opening the top combobox mounts its scrim and content once with no duplicates, and still relayouts on resize.
    rt.update(|s, _| {
        let _ = s.combo.step(rune_kit::ComboboxEvent::Toggle { current: None });
    });
    rt.frame(1.0 / 60.0);
    assert!(duplicate_paths(&rt).is_empty(), "open combobox in index must have no duplicate keys: {:?}", duplicate_paths(&rt));
    assert!(rt.find("model-content").is_some(), "model-content should mount when s.combo is open");

    let v1 = VIEWS.get();
    for w in [860.0, 920.0, 780.0] {
        rt.set_viewport((w, 560.0), 1.0);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(VIEWS.get() - v1, 0, "index resizes with open combobox should take Relayout without rebuilding view");
}

/// Opening the comboboxes and every shared overlay in index produces zero duplicate sibling keys
/// (previously `rune-combobox--searchable` and `rune-combobox--animated-filter` both mounted
/// `model-scrim` and `model-content` under `kit-root`).
#[test]
fn index_open_overlays_have_no_duplicate_sibling_keys() {
    let mut rt = rune::Runtime::new(init(), view, (800.0, 560.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(duplicate_paths(&rt).is_empty(), "closed index duplicates: {:?}", duplicate_paths(&rt));

    // Open all three comboboxes simultaneously: each must use distinct scrim/content keys.
    rt.update(|s, _| {
        let _ = s.combo.step(rune_kit::ComboboxEvent::Toggle { current: None });
        let _ = s.combo_animated.step(rune_kit::ComboboxEvent::Toggle { current: None });
        let _ = s.combo_localized.step(rune_kit::ComboboxEvent::Toggle { current: None });
    });
    rt.frame(1.0 / 60.0);
    let dups = duplicate_paths(&rt);
    assert!(dups.is_empty(), "all three comboboxes open in index had duplicates: {dups:?}");
    assert!(rt.find("model-content").is_some());
    assert!(rt.find("model-animated-content").is_some());
    assert!(rt.find("modele-content").is_some());

    // Close comboboxes and open every overlay key listed in OPENED in index mode.
    rt.update(|s, _| {
        s.combo = Default::default();
        s.combo_animated = Default::default();
        s.combo_localized = Default::default();
    });
    for &(_, key) in stories::OPENED {
        rt.update(|s, _| s.open = Some(key.to_owned()));
        rt.frame(1.0 / 60.0);
        let dups = duplicate_paths(&rt);
        assert!(dups.is_empty(), "index with open={key:?} had duplicate sibling keys: {dups:?}");
    }
}

/// Standing check: no single story scene produces a duplicate sibling key.
#[test]
fn no_story_produces_duplicate_sibling_keys() {
    for &(id, setup) in stories::SCENES {
        let mut g = init();
        setup(&mut g);
        let mut rt = rune::Runtime::new(g, view, (720.0, 480.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        let dups = duplicate_paths(&rt);
        assert!(dups.is_empty(), "story {id:?} produced duplicate sibling keys: {dups:?}");
    }
}
