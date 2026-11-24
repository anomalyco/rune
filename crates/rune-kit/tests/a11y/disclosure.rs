//! Disclosures and outlines: accordion, collapsible, tree and file tree read expanded or
//! collapsed, their level and selection, and open by assistive action, pointer and key.

use rune::accesskit::Action;
use rune::{Cx, El, Key, col, text};
use rune_kit::*;

use crate::support::*;

fn disclosures(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col()
        .gap(8.0)
        .child(
            Accordion::new()
                .item(AccordionItem::new("General").expanded(s.open).on_toggle(|s: &mut App| s.open = !s.open).child(text("Name and icon")))
                .item(AccordionItem::new("Advanced").disabled(true))
                .build(&k)
                .key("accordion"),
        )
        .child(Collapsible::new("Details", s.on).on_toggle(|s: &mut App| s.on = !s.on).child(text("More words")).build(&k).key("details"))
}

#[test]
fn accordions_and_collapsibles_read_open_and_open_by_any_input() {
    let mut rt = mount(App::default(), disclosures);
    let tree = rt.accessibility_json();
    let general = node(node(&tree, "item0"), "trigger");
    assert_eq!(said(general), ("button", "General"));
    assert!(has(general, "collapsed") && can(general, "expand") && can(general, "focus"));
    let advanced = node(node(&tree, "item1"), "trigger");
    assert!(has(advanced, "disabled") && actions(advanced).is_empty());
    let details = node(node(&tree, "details"), "trigger");
    assert!(has(details, "collapsed") && can(details, "expand"));
    no_echo(&tree);
    golden("disclosures", &tree);

    // Both triggers are keyed "trigger"; the first in the tree is the accordion's.
    assert!(act(&mut rt, "trigger", Action::Expand));
    assert!(rt.state.open);
    let tree = rt.accessibility_json();
    let general = node(node(&tree, "item0"), "trigger");
    assert!(has(general, "expanded") && can(general, "collapse"));
    let content = node(node(&tree, "item0"), "content");
    assert_eq!(said(content), ("region", "General"));
    assert!(!named(content, "Name and icon").is_empty(), "the opened content reads");
    golden("disclosures-open", &tree);

    act(&mut rt, "trigger", Action::Focus);
    press_key(&mut rt, Key::Text(" ".into()));
    assert!(!rt.state.open, "Space on the focused trigger closes it");
    tap(&mut rt, "details");
    assert!(rt.state.on, "a press opens the collapsible");
    assert!(has(node(node(&rt.accessibility_json(), "details"), "trigger"), "expanded"));
}

fn files() -> rune_kit::TreeModel<FileNode<(u32, u32)>> {
    let mut m = FileTree::new().build(["src/main.rs", "src/lib.rs", "README.md"].map(|p| (p, (3, 1))));
    m.select("README.md");
    m
}

fn file_tree(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col().h(400.0).child(
        Tree::new(|s: &App| &s.files, |s: &mut App| &mut s.files, FileNode::label)
            .name("Files")
            .row(|n, r| {
                let item = TreeItem::file(n, r);
                match &n.data {
                    Some((add, del)) if r.expanded.is_none() => item.changes(*add, *del).badge("M"),
                    _ => item,
                }
            })
            .on_open(|s: &mut App, id, _| s.chosen = Some(id.to_owned()))
            .build(s, &k)
            .h(400.0),
    )
}

#[test]
fn a_file_tree_reads_level_open_state_and_selection() {
    let mut rt = mount(App { files: files(), ..App::default() }, file_tree);
    let tree = rt.accessibility_json();
    let outline = node(&tree, "files");
    assert_eq!(said(outline), ("tree", "Files"));
    assert!(can(outline, "focus"));
    let src = node(&tree, "src");
    assert_eq!(said(src), ("tree-item", "src"));
    assert!(has(src, "collapsed") && can(src, "expand") && src["level"] == 1);
    let readme = node(&tree, "README.md");
    assert!(has(readme, "selected"), "the selected row reads selected: {readme}");
    assert_eq!(readme["description"], "3 added, 1 removed, M");
    assert!(!has(src, "selected"));
    assert!(with_role(&tree, "image").is_empty(), "file icons and chevrons are decoration");
    no_echo(&tree);
    golden("file-tree", &tree);

    assert!(act(&mut rt, "src", Action::Expand));
    assert!(rt.state.files.is_expanded("src"));
    let tree = rt.accessibility_json();
    let src = node(&tree, "src");
    assert!(has(src, "expanded") && has(src, "selected"), "an assistive click selects and opens: {src}");
    let main = node(&tree, "src/main.rs");
    assert_eq!((said(main), main["level"].as_u64()), (("tree-item", "main.rs"), Some(2)));
    golden("file-tree-open", &tree);

    act(&mut rt, "files", Action::Focus);
    press_key(&mut rt, Key::Down);
    let tree = rt.accessibility_json();
    assert!(has(node(&tree, "src/lib.rs"), "selected"), "keys move the selection the tree reads");
    assert_eq!(node(&tree, "files")["activeDescendant"], "src/lib.rs", "focus stays on the tree; a reader follows the cursor row");
    press_key(&mut rt, Key::Left);
    press_key(&mut rt, Key::Left);
    assert!(!rt.state.files.is_expanded("src"));
    assert!(has(node(&rt.accessibility_json(), "src"), "collapsed"));
    tap(&mut rt, "README.md");
    press_key(&mut rt, Key::Enter);
    assert_eq!(rt.state.chosen.as_deref(), Some("README.md"));
}

fn file_tree_with_menu_and_bar(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col().h(400.0).child(
        Tree::new(|s: &App| &s.files, |s: &mut App| &mut s.files, FileNode::label)
            .name("Files")
            .row(|n, r| TreeItem::file(n, r))
            .on_menu(|s: &mut App, id, _, _| s.chosen = Some(id.to_owned()))
            .scroll_bar(true)
            .build(s, &k)
            .h(400.0),
    )
}

#[test]
fn a_tree_with_on_menu_and_scroll_bar_exposes_row_context_menu_and_hides_the_bar() {
    let mut rt = mount(App { files: files(), ..App::default() }, file_tree_with_menu_and_bar);
    let tree = rt.accessibility_json();
    let outline = node(&tree, "files");
    assert!(!can(outline, "context-menu"), "tree container itself does not advertise a row menu");
    let readme = node(&tree, "README.md");
    assert!(can(readme, "context-menu"), "rows with on_menu advertise ShowContextMenu: {readme}");
    assert!(find(&tree, "scroll-bar").is_none() && find(&tree, "thumb").is_none(), "scroll bar stays out of the accessibility tree");
    no_echo(&tree);

    assert!(act(&mut rt, "README.md", Action::ShowContextMenu));
    assert_eq!(rt.state.chosen.as_deref(), Some("README.md"));

    // Without on_menu, rows do not advertise ShowContextMenu.
    let plain = mount(App { files: files(), ..App::default() }, file_tree);
    let plain_tree = plain.accessibility_json();
    assert!(!can(node(&plain_tree, "README.md"), "context-menu"));
}

fn lazy_branch_files() -> rune_kit::TreeModel<FileNode<(u32, u32)>> {
    let mut m = TreeModel::new().key("files");
    m.insert_branch(None, "unloaded", FileNode { name: "unloaded".into(), path: "unloaded".into(), dir: true, data: None });
    m.insert_branch(None, "empty", FileNode { name: "empty".into(), path: "empty".into(), dir: true, data: None });
    m.insert(None, "README.md", FileNode { name: "README.md".into(), path: "README.md".into(), dir: false, data: Some((0, 0)) });
    m
}

fn lazy_branch_tree(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col().h(400.0).child(
        Tree::new(|s: &App| &s.files, |s: &mut App| &mut s.files, FileNode::label)
            .name("Files")
            .row(|n, r| TreeItem::file(n, r))
            .on_toggle(|s: &mut App, id, open, _| {
                s.presses += 1;
                s.chosen = Some(format!("{id}:{open}"));
                if open && id == "unloaded" && !s.files.has_children("unloaded") {
                    s.files.insert(
                        Some("unloaded"),
                        "unloaded/mod.rs",
                        FileNode { name: "mod.rs".into(), path: "unloaded/mod.rs".into(), dir: false, data: Some((0, 0)) },
                    );
                }
            })
            .build(s, &k)
            .h(400.0),
    )
}

#[test]
fn unloaded_and_empty_branches_expose_expanded_state_and_actions() {
    let mut rt = mount(App { files: lazy_branch_files(), ..App::default() }, lazy_branch_tree);
    let tree = rt.accessibility_json();
    let unloaded = node(&tree, "unloaded");
    assert_eq!(said(unloaded), ("tree-item", "unloaded"));
    assert!(has(unloaded, "collapsed") && can(unloaded, "expand") && !can(unloaded, "collapse"));
    let empty = node(&tree, "empty");
    assert_eq!(said(empty), ("tree-item", "empty"));
    assert!(has(empty, "collapsed") && can(empty, "expand") && !can(empty, "collapse"));
    let readme = node(&tree, "README.md");
    assert!(!has(readme, "collapsed") && !has(readme, "expanded") && !can(readme, "expand") && !can(readme, "collapse"));
    no_echo(&tree);

    // Assistive Expand on an unloaded branch fires on_toggle(..., true) and mounts lazy children.
    assert!(act(&mut rt, "unloaded", Action::Expand));
    assert_eq!(rt.state.chosen.as_deref(), Some("unloaded:true"));
    let tree = rt.accessibility_json();
    let unloaded = node(&tree, "unloaded");
    assert!(has(unloaded, "expanded") && can(unloaded, "collapse") && !can(unloaded, "expand"));
    let child = node(&tree, "unloaded/mod.rs");
    assert_eq!((said(child), child["level"].as_u64()), (("tree-item", "mod.rs"), Some(2)));

    // Assistive Expand and Collapse on a genuinely empty branch keep expanded/collapsed state and fire on_toggle.
    assert!(act(&mut rt, "empty", Action::Expand));
    assert_eq!(rt.state.chosen.as_deref(), Some("empty:true"));
    let tree = rt.accessibility_json();
    let empty = node(&tree, "empty");
    assert!(has(empty, "expanded") && can(empty, "collapse"));

    // Web accessibility mirror reflects expanded state on empty and loaded branches, and omits it on leaves.
    let mirror: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    let web_row = |name: &str| mirror.iter().find(|e| e["o"] == "treeitem" && e["t"] == name).unwrap_or_else(|| panic!("web mirror missing {name:?}: {mirror:?}"));
    assert_eq!(web_row("unloaded")["x"], serde_json::json!(true));
    assert_eq!(web_row("empty")["x"], serde_json::json!(true));
    assert!(web_row("README.md").get("x").is_none());

    assert!(act(&mut rt, "empty", Action::Collapse));
    assert_eq!(rt.state.chosen.as_deref(), Some("empty:false"));
    let tree = rt.accessibility_json();
    let empty = node(&tree, "empty");
    assert!(has(empty, "collapsed") && can(empty, "expand"));
    let mirror: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    let web_empty = mirror.iter().find(|e| e["o"] == "treeitem" && e["t"] == "empty").unwrap();
    assert_eq!(web_empty["x"], serde_json::json!(false));
}
