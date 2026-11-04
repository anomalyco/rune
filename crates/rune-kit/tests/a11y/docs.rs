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
    assert!(named(&tree, "•").is_empty() && named(&tree, "1.").is_empty(), "list markers are decoration");
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
