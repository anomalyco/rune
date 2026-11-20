//! The accessibility tree the runtime builds, and the actions it runs.

use accesskit::{Action as Ak, ActionData, ActionRequest, Role, TreeId};
use serde_json::Value;

use crate::{Cx, El, Env, GrowthSpec, LiveRegion, Runtime, ScrollSpec, TextEdit, col, row, text, text_input};

fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    if v["key"] == key { return Some(v); }
    v["children"].as_array()?.iter().find_map(|c| find(c, key))
}

fn count(v: &Value) -> usize {
    1 + v["children"].as_array().map_or(0, |c| c.iter().map(count).sum())
}

fn states(v: &Value) -> Vec<&str> {
    v["states"].as_array().map_or(Vec::new(), |s| s.iter().filter_map(Value::as_str).collect())
}

fn actions(v: &Value) -> Vec<&str> {
    v["actions"].as_array().map_or(Vec::new(), |s| s.iter().filter_map(Value::as_str).collect())
}

fn request<S>(rt: &Runtime<S>, key: &str, action: Ak, data: Option<ActionData>) -> ActionRequest {
    let tree = rt.accessibility_tree();
    let (id, _) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some(key)).expect("node keyed so");
    ActionRequest { action, target_tree: TreeId::ROOT, target_node: *id, data }
}

#[derive(Default)]
struct Form {
    presses: u32,
    on: bool,
    open: bool,
    level: i32,
    name: TextEdit,
    menu: u32,
}

fn form(s: &Form, _: &mut Cx<Form>) -> El {
    col().w_full().gap(8.0)
        .child(text("Settings").key("title").role("heading").level(1))
        .child(row().key("save").role("button").focusable(true).on_press(|s: &mut Form| s.presses += 1)
            .on_secondary_press(|s: &mut Form, _, _| s.menu += 1)
            .child(text("Save")))
        .child(row().key("close").role("button").label("Close window").focusable(true).on_press(|s: &mut Form| s.presses += 10)
            .child(crate::el().key("x").role("icon").size(12.0, 12.0)))
        .child(row().key("wifi").role("switch").checked(s.on).focusable(true).on_press(|s: &mut Form| s.on = !s.on)
            .child(text("Wi-Fi")))
        .child(row().key("more").role("button").expanded(s.open).focusable(true).on_press(|s: &mut Form| s.open = !s.open)
            .child(text("More")))
        .child(crate::el().key("volume").role("slider").range_value(f64::from(s.level), 0.0, 10.0).range_step(1.0)
            .label("Volume").size(100.0, 12.0)
            .on_step(|s: &mut Form, d, _| s.level = (s.level + d).clamp(0, 10)))
        .child(text_input(&s.name, crate::tok::type_::BODY).key("name").single_line().w(200.0)
            .on_edit(|s: &mut Form| &mut s.name))
        .child(row().key("off").role("button").disabled(true).child(text("Unavailable")))
        .child(text("Saved").key("status").role("status"))
        .child(text("Decoration").key("deco").a11y_hidden())
}

fn form_rt() -> Runtime<Form> {
    let mut rt = Runtime::new(Form::default(), form, (400.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

#[test]
fn controls_report_role_name_value_states_and_actions() {
    let rt = form_rt();
    let tree = rt.accessibility_json();
    assert_eq!(tree["role"], "window");
    let title = find(&tree, "title").unwrap();
    assert_eq!((title["role"].as_str(), title["name"].as_str(), title["level"].as_u64()), (Some("heading"), Some("Settings"), Some(1)));
    let save = find(&tree, "save").unwrap();
    assert_eq!((save["role"].as_str(), save["name"].as_str()), (Some("button"), Some("Save")));
    assert!(actions(save).contains(&"click") && actions(save).contains(&"focus") && actions(save).contains(&"context-menu"));
    let close = find(&tree, "close").unwrap();
    assert_eq!(close["name"], "Close window", "an icon-only button takes its label");
    let wifi = find(&tree, "wifi").unwrap();
    assert_eq!((wifi["role"].as_str(), wifi["name"].as_str()), (Some("switch"), Some("Wi-Fi")));
    assert!(states(wifi).contains(&"unchecked"));
    let more = find(&tree, "more").unwrap();
    assert!(states(more).contains(&"collapsed") && actions(more).contains(&"expand"));
    let volume = find(&tree, "volume").unwrap();
    assert_eq!(volume["role"], "slider");
    assert_eq!(volume["range"], serde_json::json!([0.0, 0.0, 10.0, 1.0]));
    assert!(actions(volume).contains(&"increment"));
    let name = find(&tree, "name").unwrap();
    assert_eq!(name["role"], "text-input");
    assert!(actions(name).contains(&"set-value"));
    let off = find(&tree, "off").unwrap();
    assert!(states(off).contains(&"disabled") && actions(off).is_empty());
    let status = find(&tree, "status").unwrap();
    assert!(states(status).contains(&"live-polite"));
    assert!(find(&tree, "deco").is_none(), "a11y_hidden leaves the tree");
    let b = save["bounds"].as_array().unwrap();
    assert!(b[2].as_f64().unwrap() > 0.0 && b[3].as_f64().unwrap() > 0.0, "bounds in points: {b:?}");
}

#[test]
fn actions_run_the_same_handlers_as_pointer_and_keys() {
    let mut rt = form_rt();
    let click = request(&rt, "save", Ak::Click, None);
    assert!(rt.a11y_action(&click));
    rt.frame(0.0);
    assert_eq!(rt.state.presses, 1);
    let menu = request(&rt, "save", Ak::ShowContextMenu, None);
    assert!(rt.a11y_action(&menu));
    rt.frame(0.0);
    assert_eq!(rt.state.menu, 1);
    let toggle = request(&rt, "wifi", Ak::Click, None);
    rt.a11y_action(&toggle);
    rt.frame(0.0);
    assert!(rt.state.on);
    assert!(states(find(&rt.accessibility_json(), "wifi").unwrap()).contains(&"checked"));
    let expand = request(&rt, "more", Ak::Expand, None);
    assert!(rt.a11y_action(&expand));
    rt.frame(0.0);
    assert!(rt.state.open);
    let expand_again = request(&rt, "more", Ak::Expand, None);
    assert!(!rt.a11y_action(&expand_again), "expanding an open disclosure does nothing");
    for _ in 0..3 {
        let up = request(&rt, "volume", Ak::Increment, None);
        rt.a11y_action(&up);
        rt.frame(0.0);
    }
    assert_eq!(rt.state.level, 3);
    let to_seven = request(&rt, "volume", Ak::SetValue, Some(ActionData::NumericValue(7.2)));
    assert!(rt.a11y_action(&to_seven), "a slider with a step takes a number");
    rt.frame(0.0);
    assert_eq!(rt.state.level, 7, "set as steps through on_step, rounded to a step");
    let set = request(&rt, "name", Ak::SetValue, Some(ActionData::Value("Ada".into())));
    assert!(rt.a11y_action(&set));
    rt.frame(0.0);
    assert_eq!(rt.state.name.text, "Ada");
    let tree = rt.accessibility_json();
    let name = find(&tree, "name").unwrap();
    assert_eq!(name["value"], "Ada");
    assert!(states(name).contains(&"focused"), "editing focuses the field: {name}");
    let off = request(&rt, "off", Ak::Click, None);
    assert!(!rt.a11y_action(&off), "a disabled control takes no action");
}

#[test]
fn focus_moves_through_the_tree() {
    let mut rt = form_rt();
    assert_eq!(rt.accessibility_tree().focus, super::tree::ROOT);
    let focus = request(&rt, "save", Ak::Focus, None);
    assert!(rt.a11y_action(&focus));
    rt.frame(0.0);
    let tree = rt.accessibility_tree();
    assert_eq!(tree.nodes.iter().find(|(id, _)| *id == tree.focus).and_then(|(_, n)| n.author_id()), Some("save"));
}

#[test]
fn updates_send_only_what_changed_and_nothing_while_inactive() {
    let mut rt = form_rt();
    assert!(rt.a11y_update().is_none(), "no client, no tree");
    rt.a11y_activate("Form");
    let full = rt.a11y_update().expect("first update after activation");
    assert!(full.tree.is_some() && full.nodes.len() > 10);
    assert!(rt.a11y_update().is_none(), "nothing changed");
    rt.frame(0.0);
    assert!(rt.a11y_update().is_none(), "an idle frame changes nothing");
    let toggle = request(&rt, "wifi", Ak::Click, None);
    rt.a11y_action(&toggle);
    rt.frame(0.0);
    let update = rt.a11y_update().expect("the switch changed");
    assert!(update.tree.is_none());
    assert!(update.nodes.len() <= 3, "only the switch and what moved: {:?}", update.nodes.iter().map(|(_, n)| n.role()).collect::<Vec<_>>());
    assert!(update.nodes.iter().any(|(_, n)| n.author_id() == Some("wifi")));
    rt.a11y_deactivate();
    rt.frame(0.0);
    assert!(rt.a11y_update().is_none());
}

/// A window closed and opened again keeps its runtime but gets a new adapter, which AccessKit
/// may already hold active (AT-SPI turned on): its first update must hold the whole tree, or
/// AccessKit panics building the tree from it.
#[test]
fn a_reopened_surface_gets_the_whole_tree() {
    let mut rt = form_rt();
    let mut first = crate::AccessibilityLink::new();
    first.activate();
    assert!(first.service(&mut rt, "Form").expect("activated").tree.is_some());
    let toggle = request(&rt, "wifi", Ak::Click, None);
    rt.a11y_action(&toggle);
    rt.frame(0.0);
    assert!(first.service(&mut rt, "Form").expect("the switch changed").tree.is_none());
    drop(first);
    let mut second = crate::AccessibilityLink::new();
    rt.frame(0.0);
    assert!(second.service(&mut rt, "Form").is_none(), "nothing before the new client asks");
    second.activate();
    assert!(second.service(&mut rt, "Form").expect("activated").tree.is_some());
}

#[derive(Default)]
struct List {
    rows: usize,
}

fn list(s: &List, _: &mut Cx<List>) -> El {
    col().key("scroller").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(
        col().key("rows").role("list").growth(GrowthSpec::default()).virtual_rows(s.rows, 20.0,
            |_: &List, i| format!("row-{i}").into(),
            |_: &List, i| row().role("listitem").h(20.0).child(text(format!("Entry {i}")))),
    )
}

#[test]
fn virtual_lists_report_their_size_and_rows_their_place() {
    let mut rt = Runtime::new(List { rows: 10_000 }, list, (300.0, 200.0), Env::new(true, 1.0));
    for _ in 0..4 { rt.frame(0.0); }
    let tree = rt.accessibility_json();
    let rows = find(&tree, "rows").unwrap();
    assert_eq!(rows["setSize"], 10_000);
    let first = find(&tree, "row-0").unwrap();
    assert_eq!((first["position"].as_u64(), first["name"].as_str()), (Some(1), Some("Entry 0")));
    assert!(count(&tree) < 400, "only mounted rows: {}", count(&tree));
    let scroller = find(&tree, "scroller").unwrap();
    assert_eq!(scroller["role"], "scroll-view");
    assert!(states(scroller).contains(&"clips") && actions(scroller).contains(&"scroll-down"));
    let down = request(&rt, "scroller", Ak::ScrollDown, None);
    assert!(rt.a11y_action(&down));
    for _ in 0..4 { rt.frame(0.0); }
    let tree = rt.accessibility_json();
    let y = find(&tree, "scroller").unwrap()["scroll"][1].as_f64().unwrap();
    assert!(y > 100.0, "a page down scrolled: {y}");
}

#[test]
fn scrolling_changes_only_the_viewport_node() {
    let mut rt = Runtime::new(List { rows: 200 }, list, (300.0, 200.0), Env::new(true, 1.0));
    for _ in 0..4 { rt.frame(0.0); }
    rt.a11y_activate("List");
    rt.a11y_update();
    let down = request(&rt, "scroller", Ak::SetScrollOffset, Some(ActionData::SetScrollOffset(accesskit::Point { x: 0.0, y: 40.0 })));
    rt.a11y_action(&down);
    rt.frame(0.0);
    let update = rt.a11y_update().expect("scrolled");
    let changed: Vec<_> = update.nodes.iter().map(|(_, n)| n.author_id().unwrap_or("")).collect();
    assert!(changed.contains(&"scroller"), "{changed:?}");
    assert!(update.nodes.iter().all(|(_, n)| n.role() != Role::Label), "rows keep their content-space bounds: {changed:?}");
}

#[derive(Default)]
struct Field {
    edit: TextEdit,
}

fn field(s: &Field, _: &mut Cx<Field>) -> El {
    col().child(text_input(&s.edit, crate::tok::type_::BODY).key("note").w(300.0).on_edit(|s: &mut Field| &mut s.edit))
}

#[test]
fn text_fields_expose_lines_caret_and_selection() {
    let mut edit = TextEdit::new("hello world");
    edit.focused = true;
    edit.set_selection(0, 5);
    let mut rt = Runtime::new(Field { edit }, field, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_tree();
    let (_, note) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("note")).unwrap();
    assert_eq!(note.role(), Role::MultilineTextInput);
    let runs: Vec<_> = tree.nodes.iter().filter(|(_, n)| n.role() == Role::TextRun).collect();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].1.value(), Some("hello world"));
    assert_eq!(runs[0].1.character_lengths().len(), 11);
    let sel = note.text_selection().expect("a selection");
    assert_eq!((sel.anchor.character_index, sel.focus.character_index), (0, 5));
    let positions = runs[0].1.character_positions().unwrap_or_default();
    assert!(positions.windows(2).all(|w| w[1] > w[0]), "characters advance: {positions:?}");
    // A reader moves the caret to the end of "world".
    let request = ActionRequest {
        action: Ak::SetTextSelection, target_tree: TreeId::ROOT, target_node: tree.nodes.iter().find(|(_, n)| n.author_id() == Some("note")).unwrap().0,
        data: Some(ActionData::SetTextSelection(accesskit::TextSelection {
            anchor: accesskit::TextPosition { node: runs[0].0, character_index: 11 },
            focus: accesskit::TextPosition { node: runs[0].0, character_index: 11 },
        })),
    };
    assert!(rt.a11y_action(&request));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.range(), (11, 11));
}

#[test]
fn secure_fields_never_expose_their_text() {
    let edit = TextEdit::new("hunter2").secure();
    let mut rt = Runtime::new(Field { edit }, |s: &Field, _: &mut Cx<Field>| {
        col().child(text_input(&s.edit, crate::tok::type_::BODY).key("pw").single_line().label("Password").w(200.0).on_edit(|s: &mut Field| &mut s.edit))
    }, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let json = rt.accessibility_json().to_string();
    assert!(!json.contains("hunter2"), "{json}");
    let tree = rt.accessibility_json();
    let pw = find(&tree, "pw").unwrap();
    assert_eq!((pw["role"].as_str(), pw["name"].as_str()), (Some("password-input"), Some("Password")));
}

/// Every selection end in `tree` that AccessKit's AT-SPI adapter would read names a `TextRun`
/// in it: the adapter unwraps that and takes the app down otherwise.
#[cfg(target_os = "linux")]
fn assert_selections_on_runs(tree: &accesskit::TreeUpdate) {
    let role = |id| tree.nodes.iter().find(|(n, _)| *n == id).map(|(_, n)| n.role());
    for (_, n) in &tree.nodes {
        if let Some(sel) = n.text_selection() {
            assert_eq!((role(sel.anchor.node), role(sel.focus.node)), (Some(Role::TextRun), Some(Role::TextRun)), "{sel:?}");
        }
    }
}

#[test]
fn an_empty_field_answers_as_a_short_one() {
    let mut edit = TextEdit::new("");
    edit.focused = true;
    let mut rt = Runtime::new(Field { edit }, field, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_tree();
    // Parley shapes an empty text as one space; the field still has no visual lines box.
    assert!(!tree.nodes.iter().any(|(_, n)| n.class_name() == Some(super::tree::LINES_BOX_CLASS)));
    #[cfg(target_os = "linux")]
    assert_selections_on_runs(&tree);
    // The first character typed moves the caret from the empty field into its run.
    rt.update(|s, _| {
        s.edit = TextEdit::new("h");
        s.edit.focused = true;
    });
    rt.frame(0.0);
    let tree = rt.accessibility_tree();
    let (_, note) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("note")).unwrap();
    let sel = note.text_selection().expect("a caret");
    assert_eq!(sel.focus.character_index, 1);
    #[cfg(target_os = "linux")]
    assert_selections_on_runs(&tree);
}

#[cfg(target_os = "linux")]
#[test]
fn a_long_field_marks_no_selection_on_itself_on_linux() {
    let mut edit = TextEdit::new(&"word ".repeat(14_000));
    edit.focused = true;
    let mut rt = Runtime::new(Field { edit }, field, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_selections_on_runs(&rt.accessibility_tree());
}

#[derive(Default)]
struct Linked {
    opened: Vec<String>,
}

#[test]
fn inline_links_are_their_own_nodes() {
    let mut rt = Runtime::new(Linked::default(), |_: &Linked, _: &mut Cx<Linked>| {
        col().child(crate::rich("Read the spec now.", vec![]).key("para").w(300.0)
            .link(crate::Link::new(9..13, "https://example.com/spec"))
            .on_link(|s: &mut Linked, id, _| s.opened.push(id.to_owned())))
    }, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_json();
    let para = find(&tree, "para").unwrap();
    assert_eq!(para["name"], "Read the spec now.");
    let link = find(&tree, "https://example.com/spec").unwrap();
    assert_eq!((link["role"].as_str(), link["name"].as_str(), link["url"].as_str()), (Some("link"), Some("spec"), Some("https://example.com/spec")));
    let click = request(&rt, "https://example.com/spec", Ak::Click, None);
    assert!(rt.a11y_action(&click));
    rt.frame(0.0);
    assert_eq!(rt.state.opened, ["https://example.com/spec"]);
}

#[test]
fn modal_hides_the_background_and_live_regions_announce() {
    #[derive(Default)]
    struct M;
    let mut rt = Runtime::new(M, |_: &M, _: &mut Cx<M>| {
        col()
            .child(row().key("behind").role("button").on_press(|_: &mut M| {}).child(text("Behind")))
            .child(col().key("sheet").label("Confirm").modal(true, |_: &mut M| {})
                .child(text("Are you sure?").key("q").live_region(LiveRegion::Assertive)))
    }, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.0);
    let tree = rt.accessibility_json();
    assert!(find(&tree, "behind").is_none(), "behind a modal: {tree}");
    let sheet = find(&tree, "sheet").unwrap();
    assert_eq!((sheet["role"].as_str(), sheet["name"].as_str()), (Some("dialog"), Some("Confirm")));
    assert!(states(sheet).contains(&"modal"));
    assert!(states(find(&tree, "q").unwrap_or_else(|| panic!("{tree:#}"))).contains(&"live-assertive"));
}

struct Big {
    on: Vec<bool>,
}

fn big(s: &Big, _: &mut Cx<Big>) -> El {
    col().key("big").w_full().h(600.0).scroll(ScrollSpec { stick_bottom: false }).children(s.on.iter().enumerate().map(|(i, on)| {
        row().key(format!("r{i}")).role("listitem").h(24.0).gap(8.0)
            .child(row().key("check").role("checkbox").checked(*on).focusable(true).size(16.0, 16.0)
                .on_press(move |s: &mut Big| s.on[i] = !s.on[i]))
            .child(text(format!("Row {i}: the quick brown fox")))
    }))
}

/// What the tree costs: `cargo test -p rune --profile perf a11y_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn a11y_cost() {
    super::tree::CHECK_SENT.set(false);
    let median = |mut v: Vec<f64>| { v.sort_by(f64::total_cmp); v[v.len() / 2] };
    let mut rt = Runtime::new(Big { on: vec![false; 2000] }, big, (800.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let toggle = |rt: &mut Runtime<Big>, i: usize| { rt.update(|s, _| s.on[i] = !s.on[i]); rt.frame(0.0); };
    let time = |f: &mut dyn FnMut()| { let t = crate::time::now(); f(); t.elapsed().as_secs_f64() * 1e6 };
    let off: Vec<f64> = (0..30).map(|i| time(&mut || toggle(&mut rt, i))).collect();
    rt.a11y_activate("Big");
    let full: Vec<f64> = (0..10).map(|_| { rt.a11y_activate("Big"); time(&mut || { rt.a11y_update(); }) }).collect();
    let on: Vec<f64> = (0..30).map(|i| time(&mut || toggle(&mut rt, i))).collect();
    let incremental: Vec<f64> = (0..30).map(|i| { toggle(&mut rt, i); time(&mut || { rt.a11y_update(); }) }).collect();
    let idle: Vec<f64> = (0..30).map(|_| time(&mut || { rt.a11y_update(); })).collect();
    rt.pointer_move(100.0, 100.0);
    let scroll: Vec<f64> = (1..=30).map(|k| {
        rt.wheel_event(crate::WheelEvent::new(0.0, -24.0).precise(true).at(100.0, 100.0));
        rt.frame(10.0 + k as f64 / 60.0);
        let commits = rt.commit.commits;
        let mut sent = None;
        let t = time(&mut || { sent = rt.a11y_update().map(|u| u.nodes.len()); });
        assert_eq!(rt.commit.commits, commits);
        assert!(sent.is_some_and(|n| n > 0), "a scroll sends its viewports");
        t
    }).collect();
    let nodes = rt.accessibility_tree().nodes.len();
    println!("a11y_cost elements={} nodes={nodes} commit_us={:.0} commit_active_us={:.0} full_us={:.0} incremental_us={:.0} scroll_us={:.1} idle_turn_us={:.2}",
        rt.tree.nodes.len(), median(off), median(on), median(full), median(incremental), median(scroll), median(idle));
}

#[test]
fn help_describes_unless_it_repeats_the_name_and_rows_point_at_their_active_descendant() {
    #[derive(Default)]
    struct H;
    let mut rt = Runtime::new(H, |_: &H, _: &mut Cx<H>| {
        col()
            .child(row().key("save").role("button").label("Save").help("Save the file").on_press(|_: &mut H| {}))
            .child(row().key("trash").role("button").label("Delete").help("Moves it to the Trash").on_press(|_: &mut H| {}))
            .child(row().key("quiet").role("button").label("Quiet").help("Hush").description("").on_press(|_: &mut H| {}))
            .child(col().key("tree").role("tree").focusable(true).active_descendant("b")
                .child(row().key("a").role("treeitem").child(text("Alpha")))
                .child(row().key("b").role("treeitem").focus_selected(true).child(text("Beta"))))
            .child(col().role("textbox").child(text_input(&TextEdit::new(""), crate::tok::type_::BODY).key("field").label("Find")))
            .child(text("Bad").key("bad").invalid(true))
    }, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_json();
    assert!(find(&tree, "save").unwrap()["description"].is_null(), "help that repeats the name: {tree}");
    assert_eq!(find(&tree, "trash").unwrap()["description"], "Moves it to the Trash");
    assert!(find(&tree, "quiet").unwrap()["description"].is_null());
    assert!(states(find(&tree, "b").unwrap()).contains(&"selected"), "a selected row outside a focus group: {tree}");
    let whole = rt.accessibility_tree();
    let (_, t) = whole.nodes.iter().find(|(_, n)| n.author_id() == Some("tree")).unwrap();
    let (b, _) = whole.nodes.iter().find(|(_, n)| n.author_id() == Some("b")).unwrap();
    assert_eq!(t.active_descendant(), Some(*b));
    // The box around the field joins its parent; the field is the text box.
    let field_parent_is_root = tree["children"].as_array().unwrap().iter().any(|c| c["key"] == "field");
    assert!(field_parent_is_root, "{tree}");
    let (_, bad) = whole.nodes.iter().find(|(_, n)| n.author_id() == Some("bad")).unwrap();
    assert_eq!(bad.invalid(), Some(accesskit::Invalid::True));
}

#[test]
fn updates_report_structure_and_motion_beyond_accesskit_events() {
    let mut rt = Runtime::new(List { rows: 200 }, list, (300.0, 200.0), Env::new(true, 1.0));
    for _ in 0..4 { rt.frame(0.0); }
    rt.a11y_activate("List");
    rt.a11y_update();
    rt.update(|s, _| s.rows = 3);
    for _ in 0..4 { rt.frame(0.0); }
    rt.a11y_update();
    assert!(rt.a11y_changes().structure, "rows went");
    rt.pointer_move(50.0, 50.0);
    rt.update(|s, _| s.rows = 200);
    for _ in 0..4 { rt.frame(0.0); }
    rt.a11y_update();
    rt.wheel_event(crate::WheelEvent::new(0.0, -30.0).precise(true).at(50.0, 50.0));
    rt.frame(1.0 / 60.0);
    assert!(rt.a11y_update().is_some(), "a scroll sends its viewport");
    assert!(rt.a11y_changes().moved, "a scroll moves boxes");
}

#[test]
fn scaled_text_and_long_field_lines_use_drawn_scale_and_invalidate() {
    // 1. Selectable text inside El::scale(2.0) scales inset, line bounds and glyph widths.
    fn scaled_text_view(scale: &f32, _: &mut Cx<f32>) -> El {
        col().size(500.0, 200.0).child(
            col().size(200.0, 60.0).scale(*scale).origin(0.0, 0.0)
                .child(text("Hello world").key("t").selectable(true).padding(10.0).align(crate::Align::Center)),
        )
    }
    let make_text = |scale: f32| {
        let mut rt = Runtime::new(scale, scaled_text_view, (500.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("t")).unwrap();
        rt.core().a11y_lines(i).unwrap().1
    };
    let l1 = make_text(1.0);
    let l2 = make_text(2.0);
    assert!((l2[0].rect[0] - l1[0].rect[0] * 2.0).abs() < 0.5, "scaled x origin: {:?} vs {:?}", l1[0].rect, l2[0].rect);
    assert!((l2[0].rect[1] - l1[0].rect[1] * 2.0).abs() < 0.5, "scaled y inset: {:?} vs {:?}", l1[0].rect, l2[0].rect);
    assert!((l2[0].rect[2] - l1[0].rect[2] * 2.0).abs() < 0.5, "scaled width: {:?} vs {:?}", l1[0].rect, l2[0].rect);
    assert!((l2[0].rect[3] - l1[0].rect[3] * 2.0).abs() < 0.5, "scaled height: {:?} vs {:?}", l1[0].rect, l2[0].rect);
    assert!((l2[0].widths[0] - l1[0].widths[0] * 2.0).abs() < 0.25, "scaled char width: {} vs {}", l1[0].widths[0], l2[0].widths[0]);

    // 2. Long field (> 64KB) inside El::scale invalidates lines_box_print and scales drawn lines.
    struct LongScaled { edit: TextEdit, scale: f32 }
    fn long_view(s: &LongScaled, _: &mut Cx<LongScaled>) -> El {
        col().size(600.0, 400.0).child(
            col().size(240.0, 150.0).scale(s.scale).origin(0.0, 0.0).child(
                text_input(&s.edit, crate::tok::type_::BODY).key("big").size(240.0, 150.0).padding(10.0)
                    .on_edit(|s: &mut LongScaled| &mut s.edit),
            ),
        )
    }
    let big_text = "long line for a11y scaling test\n".repeat(2500);
    let mut rt = Runtime::new(LongScaled { edit: TextEdit::new(big_text), scale: 1.0 }, long_view, (600.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("big")).unwrap();
    assert!(rt.core().a11y_uses_long_field(i), "fixture must use the long-field path");
    rt.a11y_activate("Long");
    let first = rt.a11y_update().expect("initial tree");
    let first_line_h = first.nodes.iter()
        .find(|(_, n)| n.class_name() == Some("\x1frune-line"))
        .and_then(|(_, n)| n.bounds())
        .map(|b| (b.y1 - b.y0) as f32)
        .expect("drawn long-field line at 1x");
    let mut walk = super::tree::Walk::default();
    walk.clip_y = [0.0, 400.0];
    let print1 = rt.core().a11y_lines_box_print(i, &walk);

    rt.update(|s, tx| { s.scale = 2.0; tx.instant(); });
    rt.frame(1.0 / 60.0);
    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("big")).unwrap();
    let print2 = rt.core().a11y_lines_box_print(i, &walk);
    assert_ne!(print1, print2, "a11y_lines_box_print must change when drawn_scale changes");
    let update = rt.a11y_update().expect("scale change emits updated long-field lines");
    let scaled_line_h = update.nodes.iter()
        .find(|(_, n)| n.class_name() == Some("\x1frune-line"))
        .and_then(|(_, n)| n.bounds())
        .map(|b| (b.y1 - b.y0) as f32)
        .expect("drawn long-field line at 2x");
    assert!((scaled_line_h - first_line_h * 2.0).abs() < 1.0, "long-field line height scales 2x: {first_line_h} -> {scaled_line_h}");
}

#[test]
fn rtl_a11y_scroll_left_and_right_step_in_visual_direction() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(200.0, 60.0).child(
            row().key("rtl_x").dir(crate::Direction::Rtl).size(200.0, 60.0).scroll_x()
                .child(crate::el().w(600.0).h(60.0)),
        )
    }
    let mut rt = Runtime::new((), view, (200.0, 60.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("rtl_x")).unwrap().id;
    assert_eq!(rt.scroll.scrolls[&id].x.offset, 0.0);

    // In RTL, content overflows to the left: ScrollLeft reveals it (offset increases).
    assert!(rt.accessibility_action("rtl_x", accesskit::Action::ScrollLeft, Some(accesskit::ActionData::ScrollUnit(accesskit::ScrollUnit::Item))));
    rt.frame(0.0);
    let after_left = rt.scroll.scrolls[&id].x.offset;
    assert!(after_left > 20.0, "RTL ScrollLeft must increase offset toward left content, got {after_left}");

    // ScrollRight returns toward the right start edge (offset decreases).
    assert!(rt.accessibility_action("rtl_x", accesskit::Action::ScrollRight, Some(accesskit::ActionData::ScrollUnit(accesskit::ScrollUnit::Item))));
    rt.frame(0.0);
    assert!(rt.scroll.scrolls[&id].x.offset < after_left - 20.0, "RTL ScrollRight must decrease offset: {after_left} -> {}", rt.scroll.scrolls[&id].x.offset);
}

#[test]
fn a11y_active_descendant_and_labelled_by_skip_exiting_shadow() {
    // 1. active_descendant: exiting child inside the container must not shadow the live replacement elsewhere.
    fn ad_view(inside: &bool, _: &mut Cx<bool>) -> El {
        let mut root = col().size(200.0, 120.0);
        if !*inside {
            root = root.child(row().key("item").role("option").size(80.0, 20.0).child(text("Live item")));
        }
        let mut list = col().key("list").role("listbox").focusable(true).active_descendant("item").size(100.0, 40.0);
        if *inside {
            list = list.child(row().key("item").role("option").size(80.0, 20.0)
                .transition(crate::Transition::opacity()).child(text("Old item")));
        }
        root.child(list)
    }
    let mut rt = Runtime::new(true, ad_view, (200.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = false);
    rt.frame(1.0 / 60.0);
    let list_idx = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("list")).unwrap();
    let live_item = rt.tree.nodes.iter().enumerate().position(|(i, n)| n.key.as_deref() == Some("item") && !rt.core().subtree_exiting(i)).unwrap();
    assert_eq!(rt.core().a11y_active_descendant(list_idx), Some(live_item), "exiting child must not shadow live active_descendant");

    // 2. a11y_keyed (labelled_by): exiting local key must not shadow live outer key.
    fn lbl_view(local: &bool, _: &mut Cx<bool>) -> El {
        let mut inner = col().key("inner").size(100.0, 60.0)
            .child(row().key("btn").role("button").focusable(true).labelled_by("title").size(80.0, 20.0));
        if *local {
            inner = inner.child(text("Inner title").key("title").transition(crate::Transition::opacity()));
        }
        col().key("outer").size(200.0, 120.0)
            .child(text("Outer title").key("title"))
            .child(inner)
    }
    let mut rt = Runtime::new(true, lbl_view, (200.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = false);
    rt.frame(1.0 / 60.0);
    let btn_idx = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("btn")).unwrap();
    let outer_title = rt.tree.nodes.iter().enumerate().position(|(i, n)| n.key.as_deref() == Some("title") && !rt.core().subtree_exiting(i)).unwrap();
    assert_eq!(rt.core().a11y_keyed(btn_idx, "title"), Some(outer_title), "exiting inner key must not shadow live outer key");
}

#[test]
fn nested_selectable_control_inside_focusable_group_item_is_not_in_outer_group() {
    let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| {
        col().key("tabs").role("tablist").focus_group().size(200.0, 60.0)
            .child(
                row().key("tab1").role("tab").focusable(true).focus_selected(true).size(160.0, 40.0)
                    .child(crate::el().key("inner-opt").role("option").focusable(true).size(20.0, 20.0)),
            )
    }, (200.0, 60.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_tree();
    let (_, inner) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("inner-opt")).unwrap();
    assert_eq!(inner.is_selected(), None, "control nested inside a focusable item is not a member of the outer focus group");
}
