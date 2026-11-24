use super::*;

fn list_with(scroll_path: &str, keys: &[&str]) -> VirtualList {
    let mut v = VirtualList::default();
    v.scroll_path = scroll_path.to_owned();
    v.set_keys(keys.iter().map(|s| (*s).to_owned()).collect(), 1, None);
    v
}

#[test]
fn resolve_virtual_row_table() {
    let list_a = list_with(
        "#0/pane-a/scroll-a",
        &["r0", "r50", "list-b", "dir", "dir/file", "#0", "#!tag"],
    );
    let list_b = list_with(
        "#0/pane-b/scroll-b",
        &["r0", "r50", "only-b", "dir/file", "#0", "#!tag"],
    );
    // A list whose own key contains a slash, encoded with `#!` and `%2F` per path encoding.
    let list_slash = list_with(
        "#0/pane-c/scroll-c",
        &["r0", "r50", "src/main.rs"],
    );
    // Two distinct lists sharing the same leaf key `rows` under different panes.
    let shared_left = list_with("#0/workspace/left/scroll-l", &["shared", "only-left"]);
    let shared_right = list_with("#0/workspace/right/scroll-r", &["shared", "only-right"]);

    let slash_path = element_path::node_path("#0/pane-c/scroll-c", Some("sec/b"), 0);
    assert_eq!(slash_path, "#0/pane-c/scroll-c/#!sec%2Fb");

    let lists: [(&str, &VirtualList); 5] = [
        ("#0/pane-a/scroll-a/#0/list-a", &list_a),
        ("#0/pane-b/scroll-b/#0/list-b", &list_b),
        (slash_path.as_str(), &list_slash),
        ("#0/workspace/left/scroll-l/rows", &shared_left),
        ("#0/workspace/right/scroll-r/rows", &shared_right),
    ];

    let cases: &[(&str, Option<&str>, &str, Option<(usize, usize)>)] = &[
        // Plain keys: first matching list in tree order.
        ("plain key in multiple lists", None, "r50", Some((0, 1))),
        ("plain key unique to second list", None, "only-b", Some((1, 2))),
        ("plain key containing slash", None, "src/main.rs", Some((2, 2))),
        ("plain key matching #0", None, "#0", Some((0, 5))),
        ("plain key starting with #!", None, "#!tag", Some((0, 6))),

        // scope/row: leaf scope, multi-segment scope across unkeyed #0, full path.
        ("scope/row targets second list", None, "list-b/r50", Some((1, 1))),
        ("multi-segment scope/row skips #0", None, "pane-b/scroll-b/list-b/r50", Some((1, 1))),
        ("full path targets second list", None, "#0/pane-b/scroll-b/#0/list-b/r50", Some((1, 1))),
        // Direct scope/row in list-b beats earlier list-a that has a row literally keyed "list-b",
        // while a non-row suffix still falls back to row/mark on list-a's row "list-b".
        ("scope/row beats earlier row named after scope", None, "list-b/r0", Some((1, 0))),
        ("row/mark fallback when scope-named row has descendant mark", None, "list-b/mark", Some((0, 2))),

        // row/mark: unscoped descendant path (including multi-segment descendant).
        ("row/mark in first list", None, "r50/mark", Some((0, 1))),
        ("row/body/mark in first list", None, "r50/body/mark", Some((0, 1))),
        ("row/mark unique to second list", None, "only-b/mark", Some((1, 2))),
        ("row/mark prefers longer slash row key over prefix row", None, "dir/file/mark", Some((0, 4))),
        ("row/mark still finds prefix row when longer key absent", None, "dir/other/mark", Some((0, 3))),
        ("escaped row segment in row/mark", None, "#!dir%2Ffile/mark", Some((0, 4))),

        // scope/row/mark: scoped descendant path (single and multi-segment).
        ("scope/row/mark targets second list", None, "list-b/r50/mark", Some((1, 1))),
        ("multi-segment scope/row/body/mark", None, "pane-b/scroll-b/list-b/r50/body/mark", Some((1, 1))),
        ("full path scope/row/mark", None, "#0/pane-b/scroll-b/#0/list-b/r50/#0/mark", Some((1, 1))),

        // Escaped slash keys in row key, scope key, or both (path encoding from #1531).
        ("scope/row with slash in row key", None, "list-b/dir/file", Some((1, 3))),
        ("scope/row/mark with slash in row key", None, "list-b/dir/file/mark", Some((1, 3))),
        ("full encoded path with escaped slash row key", None, "#0/pane-b/scroll-b/#0/list-b/#!dir%2Ffile", Some((1, 3))),
        ("encoded path suffix with escaped slash row key", None, "list-b/#!dir%2Ffile", Some((1, 3))),
        ("full encoded path to mark inside escaped slash row", None, "#0/pane-b/scroll-b/#0/list-b/#!dir%2Ffile/#0/mark", Some((1, 3))),
        ("scope key containing slash + plain row", None, "sec/b/r50", Some((2, 1))),
        ("scope key containing slash + row containing slash", None, "sec/b/src/main.rs", Some((2, 2))),
        ("scope key containing slash + row containing slash + mark", None, "sec/b/src/main.rs/mark", Some((2, 2))),
        ("encoded scope + encoded row suffix", None, "#!sec%2Fb/#!src%2Fmain.rs", Some((2, 2))),
        ("scoped escaped #0 row key", None, "list-b/#!#0", Some((1, 4))),
        ("scoped escaped #! row key", None, "list-b/#!#!tag", Some((1, 5))),

        // A scope that matches two lists (both keyed "rows", or both under ancestor "workspace").
        ("shared scope picks first list when both have row", None, "rows/shared", Some((3, 0))),
        ("shared scope falls through to second list when first lacks row", None, "rows/only-right", Some((4, 1))),
        ("shared scope with mark falls through to second list", None, "rows/only-right/mark", Some((4, 1))),
        ("wider scope disambiguates second shared-key list", None, "right/scroll-r/rows/shared", Some((4, 0))),
        ("explicit ancestor scope matches first of two lists inside it", Some("workspace"), "shared", Some((3, 0))),
        ("explicit ancestor scope finds row in second list inside it", Some("workspace"), "only-right", Some((4, 1))),
        ("explicit ancestor scope + key path disambiguates second list", Some("workspace"), "right/scroll-r/rows/shared", Some((4, 0))),
        ("explicit ancestor scope excludes lists outside it", Some("workspace"), "only-b", None),

        // Keys that match nothing stay quiet no-ops.
        ("empty key", None, "", None),
        ("bare slash", None, "/", None),
        ("unknown plain key", None, "missing", None),
        ("unknown row under known scope", None, "list-a/missing", None),
        ("unknown row under escaped-slash scope", None, "sec/b/missing", None),
        ("unknown scope with known row", None, "missing/r50", None),
        ("unknown row/mark under known scope", None, "list-a/missing/mark", None),
        ("partial segment name must not alias scope", None, "st-b/r50", None),
        ("non-contiguous ancestor/row must not skip keyed list", None, "pane-b/r50", None),
        ("unknown explicit scope", Some("nope"), "r50", None),
    ];

    for &(name, scope, key, expected) in cases {
        assert_eq!(
            resolve_virtual_row_in(&lists, scope, key),
            expected,
            "case {name:?}: scope={scope:?} key={key:?}"
        );
    }
}
