//! Collections and surfaces: list, table, card and diff view read their items, place in the
//! set, selection and headers.

use std::borrow::Cow;

use rune::accesskit::Action;
use rune::{Align, Cx, El, Key, col, text};
use rune_kit::diff::{DiffLabel, DiffMode, DiffOptions, DiffState, DiffView, FileDiff, FoldRow, LineKind, Side};
use rune_kit::*;

use crate::support::*;

fn lists(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let mut picker = List::new().active(None);
    for (i, name) in ["Inbox", "Drafts", "Sent"].into_iter().enumerate() {
        picker = picker.item(ListItem::new(name).group(if i < 2 { "Mail" } else { "Other" }).selected(s.picked == Some(i))
            .on_select(move |s: &mut App, _| s.picked = Some(i)));
    }
    col()
        .gap(8.0)
        .child(picker.build(&k).key("mail"))
        .child(List::new().item(ListItem::new("Read only")).item(ListItem::new("Rows")).build(&k).key("plain"))
}

#[test]
fn lists_read_their_rows_place_and_selection() {
    let mut rt = mount(App { picked: Some(1), ..App::default() }, lists);
    let tree = rt.accessibility_json();
    let mail = node(node(&tree, "mail"), "scroll");
    assert_eq!(said(mail).0, "list-box");
    assert_eq!(said(node(mail, "group0")), ("group", "Mail"));
    let inbox = node(mail, "item0");
    assert_eq!(said(inbox), ("list-box-option", "Inbox"));
    assert!(has(inbox, "unselected") && inbox["position"] == 1);
    assert!(has(node(mail, "item1"), "selected"));
    let plain = node(node(&tree, "plain"), "scroll");
    assert_eq!(said(plain).0, "list");
    assert_eq!(said(node(plain, "item1")), ("list-item", "Rows"));
    assert_eq!(node(plain, "item1")["position"], 2);
    no_echo(&tree);
    golden("list", &tree);

    assert!(act(&mut rt, "item0", Action::Click));
    assert_eq!(rt.state.picked, Some(0));
    act(&mut rt, "item0", Action::Focus);
    press_key(&mut rt, Key::Down);
    press_key(&mut rt, Key::Enter);
    assert_eq!(rt.state.picked, Some(1));
    assert!(has(node(&rt.accessibility_json(), "item1"), "selected"));
}

const NAMES: [&str; 4] = ["alpha.rs", "beta.rs", "gamma.rs", "delta.rs"];

fn table(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col().size(500.0, 300.0).child(
        Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |_| NAMES.len(), |_, i| Cow::Owned(format!("f{i}")))
            .name("Files")
            .column(TableColumn::new("name", "Name").flex(1.0).sortable())
            .column(TableColumn::new("size", "Size").width(90.0).align(Align::End).resizable(false))
            .cell(|_: &App, i, col| match col {
                "name" => Cell::text(NAMES[i]),
                _ => Cell::text(format!("{} KB", i + 1)).mono(),
            })
            .on_open(|s: &mut App, id, _| s.chosen = Some(id.to_owned()))
            .build(s, &k)
            .h(300.0),
    )
}

#[test]
fn a_table_reads_its_headers_rows_and_selection() {
    let mut rt = mount(App::default(), table);
    rt.update(|s, _| s.table.select("f1"));
    settle(&mut rt);
    let tree = rt.accessibility_json();
    let grid = node(&tree, "table");
    assert_eq!(said(grid), ("grid", "Files"));
    assert!(can(grid, "focus"));
    let name = node(&node(&tree, "header").clone(), "name").clone();
    assert_eq!(said(&name), ("column-header", "Name"));
    assert!(can(&name, "click"), "a sortable header sorts on click");
    let row = node(&tree, "f1");
    assert_eq!(said(row), ("row", "beta.rs 2 KB"));
    assert!(has(row, "selected") && row["position"] == 2);
    assert!(!has(node(&tree, "f0"), "selected"));
    assert_eq!(said(node(row, "size")), ("grid-cell", "2 KB"));
    assert_eq!(said(node(&tree, "resize")), ("splitter", "Resize Name"));
    golden("table", &tree);

    assert!(act(&mut rt, "name", Action::Click));
    let name = node(&node(&rt.accessibility_json(), "header").clone(), "name").clone();
    assert_eq!((name["name"].as_str(), name["sort"].as_str()), (Some("Name"), Some("ascending")));
    assert!(act(&mut rt, "f3", Action::Click));
    assert!(has(node(&rt.accessibility_json(), "f3"), "selected"), "an assistive click selects the row");
    act(&mut rt, "table", Action::Focus);
    press_key(&mut rt, Key::Up);
    let tree = rt.accessibility_json();
    assert!(has(node(&tree, "f2"), "selected"));
    assert_eq!(node(&tree, "table")["activeDescendant"], "f2", "focus stays on the table; a reader follows the cursor row");
    press_key(&mut rt, Key::Enter);
    assert_eq!(rt.state.chosen.as_deref(), Some("f2"));
}

fn card(_: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    Card::new()
        .title("Build failed", Some(k.glyph.close))
        .description("3 tests failed on Linux")
        .action(Button::new("Retry").build(&k))
        .build(&k)
        .key("card")
}

#[test]
fn a_card_is_a_group_named_by_its_title() {
    let rt = mount(App::default(), card);
    let tree = rt.accessibility_json();
    let card = node(&tree, "card");
    assert_eq!(said(card), ("group", "Build failed"));
    assert!(find(card, "rule").is_none() && with_role(card, "image").is_empty(), "the accent rule and icon are decoration");
    assert_eq!(said(node(card, "description")), ("label", "3 tests failed on Linux"));
    assert_eq!(named(card, "Retry")[0]["role"], "button");
    golden("card", &tree);
}

fn diff(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col().w_full().h(400.0).child(DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff).build(s, &k)).child(text("end"))
}

#[test]
fn a_diff_reads_its_file_header_and_lines() {
    let old = "fn main() {\n    println!(\"hi\");\n}\n";
    let new = "fn main() {\n    println!(\"hello\");\n}\n";
    let state = DiffState::new(vec![FileDiff::from_texts("src/main.rs", old, new)]);
    let mut rt = mount(App { diff: state, ..App::default() }, diff);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    let tree = rt.accessibility_json();
    let lines = with_role(&tree, "list");
    let list = lines.iter().find(|l| l["name"] == "Diff").expect("the diff's rows are a list");
    let heading = &with_role(&tree, "heading")[0];
    assert_eq!(heading["name"], "src/main.rs, 1 added, 1 removed");
    let toggle = named(&tree, "Collapse file")[0];
    assert!(has(toggle, "expanded") && can(toggle, "collapse"));
    let items = with_role(list, "list-item");
    assert!(!items.is_empty() && items.iter().all(|i| i["name"].as_str().is_some_and(|n| !n.is_empty())));
    assert!(with_role(list, "button").iter().all(|b| b["name"].as_str().is_some_and(|n| !n.is_empty())), "no nameless buttons");
    golden("diff", &tree);
    assert!(rt.accessibility_action("toggle", Action::Collapse, None));
    settle(&mut rt);
    assert!(has(named(&rt.accessibility_json(), "Expand file")[0], "collapsed"));
}

fn localized_diff(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let mut view = DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff)
        .fold_row(if s.open { FoldRow::LineInfo } else { FoldRow::Summary })
        .on_comment(|s: &mut App, _, _| s.presses += 1)
        .labels(|label| match label {
            DiffLabel::List => Some("Différences".into()),
            DiffLabel::ExpandFile => Some("Déplier le fichier".into()),
            DiffLabel::CollapseFile => Some("Replier le fichier".into()),
            DiffLabel::CommentLine => Some("Commenter cette ligne".into()),
            DiffLabel::Line { kind, number, text } => {
                let k = match kind {
                    LineKind::Added => "ajout",
                    LineKind::Removed => "suppression",
                    LineKind::Context => "contexte",
                };
                Some(format!("{k} {number}: {text}"))
            }
            DiffLabel::FileSummary { path, additions, deletions, .. } => {
                Some(format!("{path} (+{additions}/-{deletions})"))
            }
            DiffLabel::UnchangedLines(n) => Some(format!("{n} lignes masquées")),
            DiffLabel::UnmodifiedLines(n) => Some(format!("{n} lignes identiques")),
            DiffLabel::UnchangedLinesBefore { count, header } => Some(format!("{count} masquées avant {header}")),
            DiffLabel::ShowMoreLinesAbove(n) => Some(format!("+{n} au-dessus")),
            DiffLabel::ShowMoreLinesBelow(n) => Some(format!("+{n} en dessous")),
            DiffLabel::ShowAllLines(n) => Some(format!("Tout afficher ({n})")),
            DiffLabel::ShowHiddenLines(n) => Some(format!("Afficher {n}")),
            _ => None,
        });
    if s.on {
        view = view.comment_filter(|_, at| at.side == Side::New);
    }
    col().w_full().h(600.0).child(view.build(s, &k))
}

#[test]
fn diff_labels_and_comment_filter_reach_the_accessibility_tree() {
    let middle: String = (0..50).map(|i| format!("same {i}\n")).collect();
    let old = format!("old\n{middle}old_end\n");
    let new = format!("new\n{middle}new_end\n");
    let state = DiffState::new(vec![FileDiff::from_texts("a.txt", &old, &new)]);
    let mut rt = mount(App { diff: state, on: true, ..App::default() }, localized_diff);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }

    let tree = rt.accessibility_json();
    let list = with_role(&tree, "list").into_iter().find(|l| l["name"] == "Différences").expect("localized list name");
    let heading = &with_role(&tree, "heading")[0];
    assert_eq!(heading["name"], "a.txt (+2/-2)");
    let toggle = named(&tree, "Replier le fichier")[0];
    assert!(has(toggle, "expanded") && can(toggle, "collapse"));

    // Localized line items and fold controls.
    assert!(with_role(list, "list-item").iter().any(|n| n["name"] == "suppression 1: old"));
    assert!(with_role(list, "list-item").iter().any(|n| n["name"] == "ajout 1: new"));
    assert!(with_role(list, "list-item").iter().any(|n| n["name"] == "contexte 2: same 0"));
    assert!(!named(list, "+20 au-dessus").is_empty());
    assert!(!named(list, "+20 en dessous").is_empty());
    assert!(!named(list, "Tout afficher (44)").is_empty());

    // comment_filter(|_, at| at.side == Side::New): removed row has no comment button, added row does.
    let removed = node(list, "df0.o1");
    assert!(with_role(removed, "button").is_empty(), "removed line has no comment button in a11y tree");
    let added = node(list, "df0.n1");
    let plus = named(added, "Commenter cette ligne");
    assert_eq!(plus.len(), 1);
    assert_eq!(plus[0]["role"], "button");

    // Collapsing and expanding switches the accessible toggle label.
    assert!(rt.accessibility_action("toggle", Action::Collapse, None));
    settle(&mut rt);
    let tree = rt.accessibility_json();
    let collapsed = named(&tree, "Déplier le fichier");
    assert_eq!(collapsed.len(), 1);
    assert!(has(collapsed[0], "collapsed") && can(collapsed[0], "expand"));
    assert!(rt.accessibility_action("toggle", Action::Expand, None));
    settle(&mut rt);
    assert_eq!(named(&rt.accessibility_json(), "Replier le fichier").len(), 1);

    // LineInfo fold row uses localized UnmodifiedLines and Split mode honors comment_filter.
    rt.update(|s, _| {
        s.open = true;
        s.diff.set_mode(DiffMode::Split);
    });
    settle(&mut rt);
    let tree = rt.accessibility_json();
    assert!(!named(&tree, "44 lignes identiques").is_empty());
    let pair = node(&tree, "df0.o1");
    assert!(with_role(node(pair, "old"), "button").is_empty(), "split old half omits filtered comment button");
    assert_eq!(named(node(pair, "new"), "Commenter cette ligne").len(), 1, "split new half keeps localized comment button");
}

fn rtl_and_blank_diff(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let view = DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff)
        .on_comment(|_, _, _| {})
        .labels(|label| match label {
            DiffLabel::List => Some("الفروقات".into()),
            DiffLabel::ExpandFile => Some("توسيع الملف".into()),
            DiffLabel::CollapseFile => Some("طي الملف".into()),
            // Blank strings must fall back to English so controls are never unnamed.
            DiffLabel::CommentLine => Some("   ".into()),
            DiffLabel::FileSummary { .. } => Some(String::new()),
            _ => None,
        });
    col().w_full().h(400.0).child(view.build(s, &k))
}

#[test]
fn diff_labels_support_rtl_and_fall_back_on_blank_strings() {
    let state = DiffState::new(vec![FileDiff::from_texts("rtl.txt", "مرحبا\n", "أهلا\n")]);
    let mut rt = mount(App { diff: state, ..App::default() }, rtl_and_blank_diff);
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let tree = rt.accessibility_json();
    assert!(with_role(&tree, "list").iter().any(|l| l["name"] == "الفروقات"));
    assert_eq!(named(&tree, "طي الملف").len(), 1);
    // Blank strings fell back to exact English defaults.
    assert_eq!(with_role(&tree, "heading")[0]["name"], "rtl.txt, 1 added, 1 removed");
    assert!(!named(&tree, "Comment on this line").is_empty());
    assert!(with_role(&tree, "button").iter().all(|b| b["name"].as_str().is_some_and(|n| !n.trim().is_empty())));
}

fn custom_header_diff(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let view = DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff).file_header(|s: &App, f, collapsed| {
        let path = s.diff.files()[f].path().to_owned();
        rune::row()
            .key("custom-toggle")
            .w_full()
            .h_full()
            .role("button")
            .label(if collapsed { format!("Expand {path}") } else { format!("Collapse {path}") })
            .expanded(!collapsed)
            .on_press(move |s: &mut App| s.diff.set_collapsed(f, !collapsed))
            .child(text(path))
    });
    col().w_full().h(400.0).child(view.build(s, &k))
}

#[test]
fn custom_file_header_and_edge_whitespace_diff_reach_the_accessibility_tree() {
    let file = FileDiff::from_texts_with(
        "src/app.rs",
        "   keep\n  old\n",
        "\tkeep\n  new\n",
        DiffOptions::new().ignore_edge_whitespace(true),
    );
    let mut state = DiffState::new(vec![file]);
    state.set_mode(DiffMode::Split);
    let mut rt = mount(App { diff: state, ..App::default() }, custom_header_diff);
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let tree = rt.accessibility_json();
    assert_eq!(with_role(&tree, "heading")[0]["name"], "src/app.rs, 1 added, 1 removed");
    let toggle = named(&tree, "Collapse src/app.rs")[0];
    assert!(has(toggle, "expanded") && can(toggle, "collapse"));
    // Split context row exposes each side's original text in its accessible name.
    let items = with_role(&tree, "list-item");
    assert!(items.iter().any(|n| n["name"] == "unchanged line 1:    keep"));
    assert!(items.iter().any(|n| n["name"] == "unchanged line 1: \tkeep"));
    // Assistive click on the custom header collapses the file.
    assert!(rt.accessibility_action("custom-toggle", Action::Click, None));
    settle(&mut rt);
    let tree = rt.accessibility_json();
    let collapsed = named(&tree, "Expand src/app.rs")[0];
    assert!(has(collapsed, "collapsed"));
    assert!(with_role(&tree, "list-item").is_empty());
}

fn diff_with_scroll_bars(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col().w_full().h(300.0).child(
        DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff)
            .scroll_bar(true)
            .build(s, &k),
    )
}

fn xy_box_with_scroll_bars(_: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col().size(200.0, 120.0).child(
        col()
            .key("board")
            .w_full()
            .h(120.0)
            .scroll_xy()
            .child(col().size(600.0, 400.0).child(text("Board cell").key("cell")))
            .scroll_bars_xy(&k),
    )
}

#[test]
fn diff_view_and_scroll_bars_xy_hide_bar_chrome_from_a11y() {
    let long = "let x = some_long_function_call_that_overflows_horizontally_in_unified_mode(1, 2, 3, 4, 5, 6);";
    let old: String = (1..=30).map(|i| format!("fn f{i}() {{ {long} }}
")).collect();
    let new = old.replace("fn f2()", "fn f2_updated()");
    let state = DiffState::new(vec![FileDiff::from_texts("src/long.rs", &old, &new)]);
    let mut rt = mount(App { diff: state, ..App::default() }, diff_with_scroll_bars);
    rt.set_system_scroll_bars(rune::ScrollBars::Always);
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let tree = rt.accessibility_json();
    assert!(find(&tree, "scroll-bar").is_none(), "vertical track stays out of the accessibility tree");
    assert!(find(&tree, "scroll-bar-x").is_none(), "horizontal track stays out of the accessibility tree");
    assert!(find(&tree, "thumb").is_none(), "thumbs stay out of the accessibility tree");
    assert!(with_role(&tree, "list").iter().any(|l| l["name"] == "Diff"));
    assert_eq!(with_role(&tree, "heading")[0]["name"], "src/long.rs, 1 added, 1 removed");
    assert!(!with_role(&tree, "list-item").is_empty());
    no_echo(&tree);

    let mut xy = mount(App::default(), xy_box_with_scroll_bars);
    xy.set_system_scroll_bars(rune::ScrollBars::Always);
    for _ in 0..5 {
        xy.frame(1.0 / 60.0);
    }
    let xy_tree = xy.accessibility_json();
    assert!(find(&xy_tree, "scroll-bar").is_none());
    assert!(find(&xy_tree, "scroll-bar-x").is_none());
    assert!(find(&xy_tree, "thumb").is_none());
    assert_eq!(node(&xy_tree, "cell")["name"], "Board cell");
    no_echo(&xy_tree);
}
