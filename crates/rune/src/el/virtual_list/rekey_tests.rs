use std::borrow::Cow;

use super::*;
use crate::input::key_diagnostics::rename_duplicates;
use crate::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el, text};

fn assert_places_matches_rename_duplicates(input: &[&str]) {
    let mut expected: Vec<String> = input.iter().map(|s| (*s).to_owned()).collect();
    let expected_renamed = rename_duplicates(expected.iter_mut());

    let mut actual: Vec<String> = input.iter().map(|s| (*s).to_owned()).collect();
    let (places, actual_renamed) = Places::from_keys_deduping(&mut actual);

    assert_eq!(actual, expected, "renamed keys differ for {input:?}");
    assert_eq!(actual_renamed, expected_renamed, "renamed list differs for {input:?}");
    for (i, key) in actual.iter().enumerate() {
        assert_eq!(places.get(&actual, key), Some(i), "places.get({key:?}) for {input:?}");
    }
    assert_eq!(places.get(&actual, "__absent__"), None);
}

#[test]
fn places_from_keys_deduping_matches_rename_duplicates_across_edge_cases() {
    assert_places_matches_rename_duplicates(&[]);
    assert_places_matches_rename_duplicates(&["only"]);
    assert_places_matches_rename_duplicates(&["a", "b", "c", "d"]);
    assert_places_matches_rename_duplicates(&["a", "b", "a", "a"]);
    assert_places_matches_rename_duplicates(&["a", "a#dup1", "a", "a"]);
    assert_places_matches_rename_duplicates(&["a", "a", "a#dup1"]);
    assert_places_matches_rename_duplicates(&["a", "a#dup1", "a", "a#dup1", "a"]);
    assert_places_matches_rename_duplicates(&["a#dup1", "a", "a", "a#dup1"]);
    assert_places_matches_rename_duplicates(&["a", "a", "a#dup2", "a"]);
    assert_places_matches_rename_duplicates(&["", "", ""]);
    assert_places_matches_rename_duplicates(&["row:R-94", "row:R-94#dup1", "row:R-94", "row:R-94"]);
}

#[test]
fn places_from_keys_deduping_handles_hash_collisions_and_duplicate_collided_keys() {
    let input = ["alpha", "beta", "alpha", "beta", "alpha#dup1", "gamma", "alpha"];
    let mut expected: Vec<String> = input.iter().map(|s| (*s).to_owned()).collect();
    let expected_renamed = rename_duplicates(expected.iter_mut());

    let mut actual: Vec<String> = input.iter().map(|s| (*s).to_owned()).collect();
    // Force every key (including generated #dupN names) onto the same hash bucket.
    let (places, actual_renamed) = Places::from_keys_deduping_with(&mut actual, |_| 0xDEAD_BEEF);

    assert_eq!(actual, expected);
    assert_eq!(actual_renamed, expected_renamed);
    for (i, key) in actual.iter().enumerate() {
        assert_eq!(places.get_hashed(&actual, key, 0xDEAD_BEEF), Some(i), "key {key:?}");
    }
    assert_eq!(places.get_hashed(&actual, "missing", 0xDEAD_BEEF), None);
    assert_eq!(places.get_hashed(&actual, "alpha", 0x1234), None);
}

#[test]
fn unique_places_and_set_keys_avoid_duplicate_set_allocations() {
    let mut keys: Vec<String> = (0..10_000).map(|i| format!("row-{i}")).collect();
    let before = crate::tooling::test_alloc::allocs();
    let (places, renamed) = Places::from_keys_deduping(&mut keys);
    let places_allocs = crate::tooling::test_alloc::allocs() - before;
    assert!(renamed.is_empty());
    // Single HashMap allocation for by_hash: no Vec<&mut String>, no HashSet<&str>, no collided map.
    assert_eq!(places_allocs, 1, "unique Places::from_keys_deduping must make 1 allocation");
    assert_eq!(places.get(&keys, "row-0"), Some(0));
    assert_eq!(places.get(&keys, "row-9999"), Some(9_999));

    // Empty -> N initial load: by_hash + born + heights + measured = 4 allocations (no sums build).
    let mut v = VirtualList::default();
    v.viewport = 600.0;
    v.set_estimate(24.0);
    let init_keys: Vec<String> = (0..10_000).map(|i| format!("a-{i}")).collect();
    let before = crate::tooling::test_alloc::allocs();
    let anchor = v.set_keys(init_keys, 1, Some(0.0));
    let init_allocs = crate::tooling::test_alloc::allocs() - before;
    assert_eq!(anchor, None);
    assert_eq!(init_allocs, 4, "initial load must allocate only by_hash, born, heights and measured");
    assert!(v.replaced_view());

    // Empty -> empty does zero allocations and leaves keys_fresh false.
    let mut empty = VirtualList::default();
    let before = crate::tooling::test_alloc::allocs();
    let (anchor, renamed) = empty.set_keys_deduping(Vec::new(), 1, Some(0.0), false);
    assert_eq!(crate::tooling::test_alloc::allocs() - before, 0);
    assert_eq!(anchor, None);
    assert!(renamed.is_empty());
    assert!(!empty.replaced_view());

    // Disjoint rekey (column-count change): 4 allocations (no duplicate set, no sums rebuild).
    let next_keys: Vec<String> = (0..10_000).map(|i| format!("b-{i}")).collect();
    let before = crate::tooling::test_alloc::allocs();
    let (anchor, renamed) = v.set_keys_deduping(next_keys, 2, Some(0.0), true);
    let rekey_allocs = crate::tooling::test_alloc::allocs() - before;
    assert_eq!(anchor, None);
    assert!(renamed.is_empty());
    assert_eq!(rekey_allocs, 4, "disjoint rekey must allocate only by_hash, born, heights and measured");
    assert!(v.replaced_view());
}

#[test]
fn set_keys_deduping_preserves_heights_birth_anchor_and_reorder_semantics() {
    let mut v = VirtualList::default();
    v.viewport = 100.0;
    v.set_estimate(20.0);
    let (anchor, renamed) = v.set_keys_deduping((0..50).map(|i| format!("r{i}")).collect(), 1, Some(0.0), false);
    assert_eq!(anchor, None);
    assert!(renamed.is_empty());
    v.set_height("r10", 42.0);
    v.set_height("r11", 31.0);
    v.frame_begins();

    // Identical keys leave keys_fresh false.
    let (anchor, renamed) = v.set_keys_deduping((0..50).map(|i| format!("r{i}")).collect(), 2, Some(200.0), false);
    assert_eq!(anchor, Some(("r10".into(), 200.0)));
    assert!(renamed.is_empty());
    assert!(!v.replaced_view());
    assert_eq!(v.height("r10"), 42.0);
    assert!(v.mounts_quietly(10, 2), "surviving row keeps birth commit 1");

    // Prepending 3 rows preserves measured heights and anchors to the first visible surviving row.
    let mut prepended = vec!["p0".into(), "p1".into(), "p2".into()];
    prepended.extend((0..50).map(|i| format!("r{i}")));
    let (anchor, _) = v.set_keys_deduping(prepended, 3, Some(200.0), true);
    assert_eq!(anchor, Some(("r10".into(), 200.0)));
    assert!(!v.replaced_view(), "visible row survived so view was not replaced");
    assert_eq!(v.height("r10"), 42.0);
    assert_eq!(v.height("r11"), 31.0);
    assert!(!v.mounts_quietly(0, 3), "newly inserted row born at commit 3");

    // Animated reorder (skip_anchor_on_reorder = true) suppresses anchor but keeps held = true.
    let mut reordered = v.keys.clone();
    reordered.swap(0, 1);
    let (anchor, _) = v.set_keys_deduping(reordered.clone(), 4, Some(200.0), true);
    assert_eq!(anchor, None);
    assert!(!v.replaced_view(), "reorder keeps rows held even without an anchor");

    // Non-animated reorder (skip_anchor_on_reorder = false) keeps the visible anchor.
    reordered.swap(0, 1);
    let (anchor, _) = v.set_keys_deduping(reordered, 5, Some(260.0), false);
    assert!(anchor.is_some());

    // Partial filter where only rows far below the viewport survive yields anchor = None and replaced_view = true.
    let (anchor, _) = v.set_keys_deduping(vec!["r49".into()], 6, Some(0.0), false);
    assert_eq!(anchor, None);
    assert!(v.replaced_view());

    // Clearing a non-empty list marks replaced_view and returns anchor = None.
    let (anchor, _) = v.set_keys_deduping(Vec::new(), 7, Some(0.0), false);
    assert_eq!(anchor, None);
    assert!(v.replaced_view());
}

#[test]
fn set_keys_deduping_preserves_measured_height_for_persisting_dup_key() {
    let mut v = VirtualList::default();
    v.viewport = 200.0;
    v.set_estimate(20.0);
    let (_, renamed) = v.set_keys_deduping(vec!["a".into(), "a".into(), "b".into()], 1, None, false);
    assert_eq!(renamed, vec![(1, "a".into())]);
    assert_eq!(v.keys, ["a", "a#dup1", "b"]);
    v.set_height("a#dup1", 55.0);

    // Re-submitting the same duplicate keys renames index 1 to "a#dup1" and retains its measured height.
    let (_, renamed) = v.set_keys_deduping(vec!["a".into(), "a".into(), "c".into()], 2, None, false);
    assert_eq!(renamed, vec![(1, "a".into())]);
    assert_eq!(v.keys, ["a", "a#dup1", "c"]);
    assert_eq!(v.height("a#dup1"), 55.0);
}

#[test]
fn runtime_virtual_rows_and_virtualize_report_identical_duplicate_warnings() {
    struct S {
        indexed: bool,
        keys: Vec<String>,
        version: u32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let stack = col().key("rows").w_full().growth(GrowthSpec::default());
        let stack = if s.indexed {
            stack
                .virtual_rows(
                    s.keys.len(),
                    20.0,
                    |s: &S, i| Cow::Borrowed(s.keys[i].as_str()),
                    |s: &S, i| text(s.keys[i].clone()).h(20.0),
                )
                .virtual_rows_version(s.version)
        } else {
            stack.virtualize(20.0).children(
                s.keys.iter().map(|k| text(k.clone()).key(k.clone()).h(20.0)),
            )
        };
        col().key("scroll").w_full().h_full().scroll(ScrollSpec::default()).child(stack)
    }

    for indexed in [true, false] {
        let keys = vec!["a".into(), "a".into(), "a#dup1".into(), "b".into()];
        let mut rt = Runtime::new(S { indexed, keys, version: 0 }, view, (300.0, 200.0), Env::default());
        rt.commit.enable_key_warnings();
        rt.frame(0.0);
        let v = &rt.commit.virtuals()["scroll/rows"];
        assert_eq!(v.keys, ["a", "a#dup2", "a#dup1", "b"], "indexed={indexed}");
        assert!(rt.find("a#dup2").is_some(), "indexed={indexed}");
        assert!(rt.find("a#dup1").is_some(), "indexed={indexed}");
        let warnings = rt.commit.key_warnings().unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("duplicate virtual row key \"a\" under \"scroll/rows\"")),
            "indexed={indexed}: {warnings:?}"
        );
    }
}

#[test]
fn runtime_virtual_rows_rekey_preserves_anchor_and_measured_heights_on_version_bump() {
    struct S {
        keys: Vec<String>,
        version: u32,
        height: f32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("scroll").w_full().h_full().scroll(ScrollSpec::default()).child(
            col().key("rows").w_full().growth(GrowthSpec::default())
                .virtual_rows(
                    s.keys.len(),
                    20.0,
                    |s: &S, i| Cow::Borrowed(s.keys[i].as_str()),
                    |s: &S, _| el().w_full().h(s.height),
                )
                .virtual_rows_version(s.version),
        )
    }
    let keys = (0..500).map(|i| format!("r{i}")).collect();
    let mut rt = Runtime::new(S { keys, version: 0, height: 32.0 }, view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 2_000.0));
    for _ in 0..8 { rt.frame(1.0 / 60.0); }

    let visible = rt.elements().into_iter()
        .find(|e| e.path.starts_with("scroll/rows/r") && e.visible[3] > 0.0)
        .expect("visible row");
    let visible_key = visible.path.rsplit('/').next().unwrap().to_owned();
    let visible_top = visible.rect[1];

    // Prepend 10 rows with a version bump: anchor stays put and measured height is retained.
    rt.update(|s, _| {
        let mut next: Vec<String> = (0..10).map(|i| format!("pre{i}")).collect();
        next.extend(s.keys.iter().cloned());
        s.keys = next;
        s.version = 1;
    });
    rt.frame(1.0 / 60.0);
    let after = rt.elements().into_iter()
        .find(|e| e.path == visible.path)
        .expect("anchor row stays mounted");
    assert!((after.rect[1] - visible_top).abs() < 0.5, "anchor shifted from {visible_top} to {}", after.rect[1]);
    assert_eq!(rt.commit.virtuals()["scroll/rows"].height(&visible_key), 32.0);
}
