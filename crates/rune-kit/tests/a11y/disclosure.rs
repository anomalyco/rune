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
