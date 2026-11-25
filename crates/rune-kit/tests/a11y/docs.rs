//! Documents: Markdown's headings, paragraphs, links, lists, tasks, code, quotes and tables,
//! structured notes, and diagrams, as a reader moves through them.

use rune::accesskit::Action;
use rune::{Cx, El, col};
use rune_kit::*;

use crate::support::*;

const DOC: &str = "# Release notes

Read [the guide](https://example.com/guide) first.

## Changes

* [x] tests pass
* [ ] docs

- Faster start
- Smaller downloads

1. Update
2. Restart

```rust
fn main() {}
```

> Quoted advice.

| Name | Size |
| --- | ---: |
| app | 4 MB |
";

fn markdown(_: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col().w(600.0).child(
        Markdown::new(DOC)
            .on_link(|s: &mut App, url| s.chosen = Some(url.to_owned()))
            .on_task(|s: &mut App, _, _, checked| s.on = checked)
            .build(&k)
            .key("doc"),
    )
}

#[test]
fn markdown_reads_as_a_document() {
    let mut rt = mount(App::default(), markdown);
    let tree = rt.accessibility_json();
    let headings = with_role(&tree, "heading");
    assert_eq!(headings.iter().map(|h| (h["name"].as_str().unwrap(), h["level"].as_u64().unwrap())).collect::<Vec<_>>(),
        [("Release notes", 1), ("Changes", 2)]);
    let link = node(&tree, "https://example.com/guide");
    assert_eq!((said(link), link["url"].as_str()), (("link", "the guide"), Some("https://example.com/guide")));
    assert!(can(link, "click"));
    let lists = with_role(&tree, "list");
    assert_eq!(lists.len(), 3, "bullets, numbers and tasks are lists");
    assert_eq!(with_role(lists[0], "check-box").len(), 2, "a task list holds its checkboxes");
    let items = with_role(lists[1], "list-item");
    assert_eq!((said(items[0]), items[0]["position"].as_u64()), (("list-item", "Faster start"), Some(1)));
    assert_eq!(lists[1]["setSize"], 2);
    assert!(named(&tree, "•").is_empty(), "unordered bullet markers are decoration");
    let ordered = with_role(lists[2], "list-item");
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0]["listStyle"], "numeric");
    assert_eq!(ordered[1]["listStyle"], "numeric");
    assert_eq!(with_role(ordered[0], "list-marker").iter().map(|m| m["name"].as_str()).collect::<Vec<_>>(), [Some("1.")]);
    assert_eq!(with_role(ordered[1], "list-marker").iter().map(|m| m["name"].as_str()).collect::<Vec<_>>(), [Some("2.")]);
    let tasks = with_role(&tree, "check-box");
    assert_eq!(tasks.iter().map(|t| (said(t).1, has(t, "checked"))).collect::<Vec<_>>(), [("tests pass", true), ("docs", false)]);
    assert!(can(tasks[1], "click") && can(tasks[1], "focus"));
    assert_eq!(with_role(&tree, "code").len(), 1);
    assert_eq!(with_role(&tree, "blockquote").len(), 1);
    let table = &with_role(&tree, "table")[0];
    let rows = with_role(table, "row");
    assert_eq!(rows.len(), 2);
    assert_eq!(with_role(rows[0], "column-header").iter().map(|c| said(c).1).collect::<Vec<_>>(), ["Name", "Size"]);
    assert_eq!(with_role(rows[1], "cell").iter().map(|c| said(c).1).collect::<Vec<_>>(), ["app", "4 MB"]);
    golden("markdown", &tree);

    assert!(act(&mut rt, "https://example.com/guide", Action::Click));
    assert_eq!(rt.state.chosen.as_deref(), Some("https://example.com/guide"));
    let docs = with_role(&rt.accessibility_json(), "check-box")[1]["key"].as_str().unwrap().to_owned();
    assert!(act(&mut rt, &docs, Action::Click));
    assert!(rt.state.on, "a reader's click checks the task");
}

#[test]
fn markdown_ordered_lists_expose_markers_list_style_nesting_and_web_ol_tags() {
    const LISTS_DOC: &str = "1. First
2. Second

Between

3. Third
4. Fourth

- Outer bullet

  5. Nested ordered

1. Outer ordered
   - Nested bullet

* [x] Done task
* [ ] Open task
";

    fn view(_: &App, _: &mut Cx<App>) -> El {
        let k = kit();
        col().w(600.0).child(Markdown::new(LISTS_DOC).build(&k).key("md"))
    }

    let rt = mount(App::default(), view);
    let tree = rt.accessibility_json();
    let lists = with_role(&tree, "list");
    // Top-level: ord1, ord3, outer-ul, outer-ol, task-ul; nested: inner-ol, inner-ul => 7 lists.
    assert_eq!(lists.len(), 7, "expected 7 lists (5 top-level + 2 nested): {tree}");

    // 1. Ordered list starting at 1: numeric listStyle + list-marker "1.", "2.".
    let ord1_items = with_role(lists[0], "list-item");
    assert_eq!(ord1_items.len(), 2);
    assert_eq!((ord1_items[0]["listStyle"].as_str(), ord1_items[0]["name"].as_str()), (Some("numeric"), Some("First")));
    assert_eq!(with_role(ord1_items[0], "list-marker")[0]["name"], "1.");
    assert_eq!(with_role(ord1_items[1], "list-marker")[0]["name"], "2.");

    // 2. Ordered list starting at 3: numeric listStyle + list-marker "3.", "4.", set position 1, 2.
    let ord3_items = with_role(lists[1], "list-item");
    assert_eq!(ord3_items.len(), 2);
    assert_eq!((ord3_items[0]["listStyle"].as_str(), ord3_items[0]["position"].as_u64()), (Some("numeric"), Some(1)));
    assert_eq!(with_role(ord3_items[0], "list-marker")[0]["name"], "3.");
    assert_eq!(with_role(ord3_items[1], "list-marker")[0]["name"], "4.");

    // 3. Nested ordered list (start=5) inside unordered list: outer has no listStyle/marker, inner has "5.".
    let outer_ul_item = with_role(lists[2], "list-item")[0];
    assert!(outer_ul_item["listStyle"].is_null());
    let inner_ol = with_role(outer_ul_item, "list")[0];
    let inner_ol_item = with_role(inner_ol, "list-item")[0];
    assert_eq!(inner_ol_item["listStyle"], "numeric");
    assert_eq!(with_role(inner_ol_item, "list-marker")[0]["name"], "5.");

    // 4. Nested unordered list inside ordered list: outer has "1.", inner has no listStyle/marker.
    let outer_ol_item = with_role(lists[4], "list-item")[0];
    assert_eq!(outer_ol_item["listStyle"], "numeric");
    assert_eq!(with_role(outer_ol_item, "list-marker")[0]["name"], "1.");
    let inner_ul = with_role(outer_ol_item, "list")[0];
    let inner_ul_item = with_role(inner_ul, "list-item")[0];
    assert!(inner_ul_item["listStyle"].is_null());
    assert!(with_role(inner_ul_item, "list-marker").is_empty());

    // 5. Task list stays a list of checkboxes.
    assert_eq!(with_role(lists[6], "check-box").len(), 2);

    // 6. Web accessibility mirror JSON: ordered lists use g="ol" (with st when start != 1),
    //    unordered & task lists use g="ul", list items use g="li", and no stray marker text appears.
    let mirror: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).expect("valid a11y_json");
    let list_groups: Vec<_> = mirror
        .iter()
        .filter(|v| v["k"] == "group" && matches!(v["g"].as_str(), Some("ol" | "ul")))
        .collect();
    let tags_and_starts: Vec<_> = list_groups
        .iter()
        .map(|v| (v["g"].as_str().unwrap(), v.get("st").and_then(serde_json::Value::as_u64)))
        .collect();
    assert_eq!(
        tags_and_starts,
        vec![
            ("ol", None),
            ("ol", Some(3)),
            ("ul", None),
            ("ol", Some(5)),
            ("ol", None),
            ("ul", None),
            ("ul", None),
        ],
        "web mirror list tags and starts must match Markdown list semantics: {mirror:?}"
    );
    assert!(
        mirror.iter().all(|v| !matches!(v.get("t").and_then(serde_json::Value::as_str), Some("1." | "2." | "3." | "4." | "5."))),
        "web mirror must not duplicate list-marker strings beside native <ol>"
    );

    // 7. Ordered task list uses <ol> with checkbox children.
    fn ordered_task_view(_: &App, _: &mut Cx<App>) -> El {
        let k = kit();
        col().w(600.0).child(Markdown::new("1. [x] First step
2. [ ] Second step
").build(&k).key("md"))
    }
    let rt_ot = mount(App::default(), ordered_task_view);
    let ot_mirror: Vec<serde_json::Value> = serde_json::from_str(&rt_ot.a11y_json()).expect("valid a11y_json");
    let ot_list = ot_mirror.iter().find(|v| v["k"] == "group" && v["g"] == "ol").expect("ordered task list mirrors as <ol>");
    assert_eq!(ot_list.get("st"), None);
    assert_eq!(with_role(&rt_ot.accessibility_json(), "check-box").len(), 2);
}

const NOTE: &str = "```facts
Status: {done} shipped
Owner: Ada
```

```steps
{built} Parser · fast path
{todo} Docs
```

> [!warning]
> Back up first.
";

fn note(_: &App, _: &mut Cx<App>) -> El {
    col().w(600.0).child(Note::new(NOTE).build(&kit()))
}

#[test]
fn notes_read_facts_steps_and_callouts_in_words() {
    let rt = mount(App::default(), note);
    let tree = rt.accessibility_json();
    let facts = node(&tree, "note/p0");
    assert_eq!(said(facts).0, "list");
    assert_eq!(said(node(facts, "fact-0")), ("list-item", "Status: done shipped"));
    assert_eq!(node(facts, "fact-1")["position"], 2);
    let steps = node(&tree, "note/p1");
    assert_eq!(said(steps), ("list", "1 of 2 built"));
    assert_eq!(said(node(steps, "step-0")), ("list-item", "Parser, built, fast path"));
    let callout = node(&tree, "note/p2");
    assert_eq!(said(callout), ("note", "warning"));
    assert!(!named(callout, "Back up first.").is_empty());
    no_echo(&tree);
    golden("notes", &tree);
}

#[cfg(feature = "mermaid")]
fn diagram(_: &App, _: &mut Cx<App>) -> El {
    col().w(600.0).child(Markdown::new("```mermaid\ngraph TD\n  A[Start] --> B[Stop]\n```\n").build(&kit()))
}

#[cfg(feature = "mermaid")]
#[test]
fn a_diagram_is_a_figure_described_by_its_source() {
    let rt = mount(App::default(), diagram);
    let tree = rt.accessibility_json();
    let figure = &with_role(&tree, "figure")[0];
    assert_eq!(said(figure), ("figure", "Flowchart"));
    assert!(figure["description"].as_str().is_some_and(|d| d.contains("A[Start] --> B[Stop]")));
    golden("mermaid", &tree);
}
