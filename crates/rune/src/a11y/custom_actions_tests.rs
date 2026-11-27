//! Tests for `El::a11y_action` custom accessibility actions: AccessKit node publication,
//! `accessibility_json()`, `Action::CustomAction` dispatch, incremental tree updates, disabled/
//! modal/inert guards, web DOM mirror (`a11y_json()`, `PatchState`, `web_a11y_action`), and
//! vendored macOS/AT-SPI adapter regression checks.

use accesskit::{Action as Ak, ActionData, Node, TreeUpdate};
use serde_json::{Value, json};

use crate::{Cx, El, Env, Runtime, col, el, text};

const MACOS_NODE_RS: &str = include_str!("../../../../vendor/accesskit_macos/src/node.rs");
const MACOS_CARGO: &str = include_str!("../../../../vendor/accesskit_macos/Cargo.toml");
const MACOS_PATCH: &str = include_str!("../../../../vendor/accesskit_macos.patch");
const ATSPI_NODE_RS: &str = include_str!("../../../../vendor/accesskit_atspi_common/src/node.rs");
const ATSPI_PATCH: &str = include_str!("../../../../vendor/accesskit_atspi_common.patch");

fn node_by_key<'a>(tree: &'a TreeUpdate, key: &str) -> &'a Node {
    tree.nodes
        .iter()
        .find(|(_, n)| n.author_id() == Some(key))
        .map(|(_, n)| n)
        .unwrap_or_else(|| panic!("missing a11y node {key}"))
}

fn json_by_key<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    if v.get("key").and_then(Value::as_str) == Some(key) {
        return Some(v);
    }
    v.get("children")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|c| json_by_key(c, key))
}

#[test]
fn custom_actions_publish_on_accesskit_node_json_and_dispatch_handlers() {
    #[derive(Default)]
    struct RowState {
        log: Vec<&'static str>,
        announced: bool,
    }

    fn view(_: &RowState, _: &mut Cx<RowState>) -> El {
        col().w(300.0).h(120.0).child(
            el().key("row-1")
                .role("article")
                .label("Row one")
                .focusable(true)
                .w_full()
                .h(40.0)
                .a11y_action("open", "Open", |s: &mut RowState, tx| {
                    s.log.push("open");
                    tx.announce("Opened", crate::LiveRegion::Polite);
                    s.announced = true;
                })
                .a11y_action("more", "More", |s: &mut RowState, _| {
                    s.log.push("more");
                })
                .child(text("Row one")),
        )
    }

    let mut rt = Runtime::new(RowState::default(), view, (320.0, 140.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let tree = rt.accessibility_tree();
    let row = node_by_key(&tree, "row-1");
    assert!(row.supports_action(Ak::Focus));
    assert!(row.supports_action(Ak::CustomAction));
    assert!(!row.supports_action(Ak::Click), "custom actions must not make a row pressable");

    let custom = row.custom_actions();
    assert_eq!(custom.len(), 2);
    assert_eq!(custom[0].description, "Open");
    assert_eq!(custom[1].description, "More");
    assert_ne!(custom[0].id, custom[1].id);
    let open_id = custom[0].id;
    let more_id = custom[1].id;

    // accessibility_json() lists both the action flag and the named custom actions.
    let dump = rt.accessibility_json();
    let row_json = json_by_key(&dump, "row-1").expect("row-1 in accessibility_json()");
    let actions: Vec<&str> = row_json["actions"]
        .as_array()
        .expect("actions array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(actions.contains(&"custom-action"), "actions must contain custom-action: {actions:?}");
    let custom_json = row_json["customActions"].as_array().expect("customActions array");
    assert_eq!(custom_json.len(), 2);
    assert_eq!(custom_json[0]["id"].as_i64(), Some(i64::from(open_id)));
    assert_eq!(custom_json[0]["description"].as_str(), Some("Open"));
    assert_eq!(custom_json[1]["id"].as_i64(), Some(i64::from(more_id)));
    assert_eq!(custom_json[1]["description"].as_str(), Some("More"));

    // Invoking "more" dispatches only "more".
    assert!(rt.accessibility_action("row-1", Ak::CustomAction, Some(ActionData::CustomAction(more_id))));
    rt.frame(0.016);
    assert_eq!(rt.state.log, vec!["more"]);
    assert!(!rt.state.announced);

    // Invoking "open" dispatches "open" with a working Tx.
    assert!(rt.accessibility_action("row-1", Ak::CustomAction, Some(ActionData::CustomAction(open_id))));
    rt.frame(0.032);
    assert_eq!(rt.state.log, vec!["more", "open"]);
    assert!(rt.state.announced);

    // Unknown action id or missing ActionData is rejected cleanly.
    let bogus = open_id.wrapping_add(12345);
    assert!(!rt.accessibility_action("row-1", Ak::CustomAction, Some(ActionData::CustomAction(bogus))));
    assert!(!rt.accessibility_action("row-1", Ak::CustomAction, None));
}

#[test]
fn duplicate_id_replaces_blank_ignored_and_ids_stay_stable_across_updates() {
    struct Dynamic {
        show_pin: bool,
        renamed: bool,
        hits: Vec<&'static str>,
    }

    fn view(s: &Dynamic, _: &mut Cx<Dynamic>) -> El {
        let mut item = el()
            .key("row")
            .role("article")
            .label("Item")
            .w(200.0)
            .h(40.0)
            .a11y_action("", "Ignored empty id", |st: &mut Dynamic, _| st.hits.push("bad"))
            .a11y_action("blank", "   ", |st: &mut Dynamic, _| st.hits.push("blank"));
        if s.show_pin {
            item = item.a11y_action("pin", "Pin", |st: &mut Dynamic, _| st.hits.push("pin"));
        }
        item = item
            .a11y_action("open", "Old Open", |st: &mut Dynamic, _| st.hits.push("old"))
            .a11y_action("open", if s.renamed { "Open details" } else { "Open" }, |st: &mut Dynamic, _| {
                st.hits.push("open")
            });
        col().w(220.0).h(80.0).child(item)
    }

    let mut rt = Runtime::new(
        Dynamic { show_pin: true, renamed: false, hits: Vec::new() },
        view,
        (240.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let _initial = rt.core_mut().a11y_activate("test");

    let tree1 = rt.accessibility_tree();
    let row1 = node_by_key(&tree1, "row");
    assert_eq!(row1.custom_actions().len(), 2);
    assert_eq!(row1.custom_actions()[0].description, "Pin");
    assert_eq!(row1.custom_actions()[1].description, "Open");
    let open_id_before = row1.custom_actions()[1].id;
    let pin_id = row1.custom_actions()[0].id;

    // Duplicate "open" replaced handler in place (runs "open", not "old").
    assert!(rt.accessibility_action("row", Ak::CustomAction, Some(ActionData::CustomAction(open_id_before))));
    rt.frame(0.016);
    assert_eq!(rt.state.hits, vec!["open"]);

    // Removing "pin" and renaming "open" triggers an incremental update while keeping "open"'s id stable.
    rt.state.show_pin = false;
    rt.state.renamed = true;
    rt.invalidate();
    rt.frame(0.032);
    let update = rt.core_mut().a11y_update().expect("changing custom actions must emit an incremental update");
    let updated_row = node_by_key(&update, "row");
    assert_eq!(updated_row.custom_actions().len(), 1);
    assert_eq!(updated_row.custom_actions()[0].description, "Open details");
    assert_eq!(updated_row.custom_actions()[0].id, open_id_before, "removing a sibling must not shift remaining action ids");

    // Removed "pin" id is now rejected.
    assert!(!rt.accessibility_action("row", Ak::CustomAction, Some(ActionData::CustomAction(pin_id))));
}

#[test]
fn custom_actions_blocked_when_disabled_inert_or_modal_occluded_and_do_not_delegate() {
    struct Guard {
        modal_open: bool,
        hits: Vec<&'static str>,
        delegated: usize,
    }

    fn view(s: &Guard, _: &mut Cx<Guard>) -> El {
        let mut root = col()
            .key("list")
            .role("listbox")
            .label("List")
            .w(260.0)
            .h(200.0)
            .on_a11y_action(|st: &mut Guard, _, _, _| {
                st.delegated += 1;
                true
            })
            .child(
                el().key("ok")
                    .role("option")
                    .label("Enabled")
                    .w_full()
                    .h(30.0)
                    .a11y_action("open", "Open", |st: &mut Guard, _| st.hits.push("ok")),
            )
            .child(
                el().key("disabled")
                    .role("option")
                    .label("Disabled")
                    .w_full()
                    .h(30.0)
                    .disabled(true)
                    .a11y_action("open", "Open", |st: &mut Guard, _| st.hits.push("disabled")),
            )
            .child(
                el().key("aria-disabled")
                    .role("option")
                    .label("Pending")
                    .w_full()
                    .h(30.0)
                    .focusable(true)
                    .aria_disabled(true)
                    .a11y_action("open", "Open", |st: &mut Guard, _| st.hits.push("aria-disabled")),
            )
            .child(
                el().key("inert")
                    .role("option")
                    .label("Inert")
                    .w_full()
                    .h(30.0)
                    .inert()
                    .a11y_action("open", "Open", |st: &mut Guard, _| st.hits.push("inert")),
            );
        if s.modal_open {
            root = root.child(
                el().key("dlg")
                    .role("dialog")
                    .label("Dialog")
                    .w(120.0)
                    .h(80.0)
                    .modal(true, |_: &mut Guard| {})
                    .child(text("Modal")),
            );
        }
        root
    }

    let mut rt = Runtime::new(
        Guard { modal_open: false, hits: Vec::new(), delegated: 0 },
        view,
        (280.0, 220.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let tree = rt.accessibility_tree();
    let ok = node_by_key(&tree, "ok");
    assert!(ok.supports_action(Ak::CustomAction));
    let open_id = ok.custom_actions()[0].id;

    for blocked in ["disabled", "aria-disabled", "inert"] {
        let n = node_by_key(&tree, blocked);
        assert!(!n.supports_action(Ak::CustomAction), "{blocked} must not advertise CustomAction");
        assert!(n.custom_actions().is_empty(), "{blocked} must not publish custom_actions");
        assert!(
            !rt.accessibility_action(blocked, Ak::CustomAction, Some(ActionData::CustomAction(open_id))),
            "{blocked} must refuse Action::CustomAction"
        );
    }
    // Unknown CustomAction on enabled node does not delegate to ancestor on_a11y_action.
    assert!(!rt.accessibility_action("ok", Ak::CustomAction, Some(ActionData::CustomAction(open_id.wrapping_add(99)))));
    rt.frame(0.016);
    assert!(rt.state.hits.is_empty());
    assert_eq!(rt.state.delegated, 0);

    // Modal occlusion hides and blocks background custom actions.
    rt.state.modal_open = true;
    rt.invalidate();
    rt.frame(0.032);
    assert!(!rt.accessibility_action("ok", Ak::CustomAction, Some(ActionData::CustomAction(open_id))));
    rt.frame(0.048);
    assert!(rt.state.hits.is_empty());
}

#[test]
fn web_mirror_exposes_custom_actions_patches_incrementally_and_dispatches() {
    struct WebState {
        show_more: bool,
        log: Vec<&'static str>,
    }

    fn view(s: &WebState, _: &mut Cx<WebState>) -> El {
        let mut row = el()
            .key("row-1")
            .role("article")
            .label("Row one")
            .focusable(true)
            .w(240.0)
            .h(40.0)
            .a11y_action("open", "Open", |st: &mut WebState, _| st.log.push("open"));
        if s.show_more {
            row = row.a11y_action("more", "More", |st: &mut WebState, _| st.log.push("more"));
        }
        let option = el()
            .key("opt-1")
            .role("option")
            .label("Option one")
            .focusable(true)
            .w(240.0)
            .h(30.0)
            .a11y_action("pin", "Pin", |st: &mut WebState, _| st.log.push("pin"));
        let disabled = el()
            .key("row-dis")
            .role("article")
            .label("Disabled row")
            .disabled(true)
            .w(240.0)
            .h(30.0)
            .a11y_action("open", "Open", |st: &mut WebState, _| st.log.push("disabled"));
        col().w(260.0).h(140.0).child(row.child(text("Row one"))).child(option).child(disabled)
    }

    let mut rt = Runtime::new(
        WebState { show_more: true, log: Vec::new() },
        view,
        (280.0, 160.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let items: Vec<Value> = serde_json::from_str(&rt.a11y_json()).expect("valid a11y_json()");
    let row_idx = items
        .iter()
        .position(|v| v["k"] == "group" && v["g"] == "article" && v["t"] == "Row one")
        .expect("article row in web mirror");
    let row_path = items[row_idx]["p"].as_str().unwrap().to_owned();

    // Custom action buttons sit inside the article container (parent == row_idx).
    let row_actions: Vec<&Value> = items
        .iter()
        .filter(|v| v["k"] == "button" && v["p"] == row_path && v.get("ca").is_some())
        .collect();
    assert_eq!(row_actions.len(), 2);
    assert_eq!((row_actions[0]["ca"].as_str(), row_actions[0]["t"].as_str(), row_actions[0]["a"].as_u64()), (Some("open"), Some("Open"), Some(row_idx as u64)));
    assert_eq!((row_actions[1]["ca"].as_str(), row_actions[1]["t"].as_str(), row_actions[1]["a"].as_u64()), (Some("more"), Some("More"), Some(row_idx as u64)));

    // Control element's custom action button sits as a sibling (not nested inside <button>).
    let opt_idx = items
        .iter()
        .position(|v| v["k"] == "button" && v["o"] == "option" && v["t"] == "Option one")
        .expect("option control in web mirror");
    let opt_path = items[opt_idx]["p"].as_str().unwrap().to_owned();
    let opt_action = items
        .iter()
        .find(|v| v["k"] == "button" && v["p"] == opt_path && v["ca"] == "pin")
        .expect("option custom action button");
    assert_eq!(opt_action.get("a"), items[opt_idx].get("a"), "control custom action must be sibling, not child of <button>");

    // Disabled row does not emit custom action buttons.
    let dis_idx = items
        .iter()
        .position(|v| v["k"] == "group" && v["t"] == "Disabled row")
        .expect("disabled row");
    let dis_path = items[dis_idx]["p"].as_str().unwrap().to_owned();
    assert!(!items.iter().any(|v| v["p"] == dis_path && v.get("ca").is_some()));

    // Dispatch via web_a11y_action.
    assert!(rt.web_a11y_action(&json!({ "kind": "a11y_custom", "path": row_path, "id": "more" })));
    assert!(rt.web_a11y_action(&json!({ "kind": "a11y_custom", "path": opt_path, "id": "pin" })));
    assert!(!rt.web_a11y_action(&json!({ "kind": "a11y_custom", "path": dis_path, "id": "open" })));
    assert!(!rt.web_a11y_action(&json!({ "kind": "a11y_custom", "path": row_path, "id": "missing" })));
    rt.frame(0.016);
    assert_eq!(rt.state.log, vec!["more", "pin"]);

    // Incremental PatchState removes only the removed custom action key.
    let mut mirror = crate::runtime::web_a11y::A11yMirror::default();
    let first = rt.core().web_a11y_patch(&mut mirror).expect("initial patch").to_owned();
    assert!(first.contains(r#""ca":"open""#) && first.contains(r#""ca":"more""#));

    rt.state.show_more = false;
    rt.invalidate();
    rt.frame(0.032);
    let second = rt.core().web_a11y_patch(&mut mirror).expect("removal patch");
    let patch: Value = serde_json::from_str(second).expect("valid patch json");
    let deleted = patch["d"].as_array().expect("deleted array");
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].as_str(), Some(format!("button:{row_path}\x1fmore").as_str()));
}

#[test]
fn vendored_macos_and_atspi_adapters_expose_custom_actions_and_patches_preserve_them() {
    assert!(MACOS_CARGO.contains("\"NSAccessibilityCustomAction\""));
    assert!(MACOS_NODE_RS.contains("#[method_id(accessibilityCustomActions)]"));
    assert!(MACOS_NODE_RS.contains("NSAccessibilityCustomAction::initWithName_target_selector"));
    assert!(MACOS_NODE_RS.contains("selector == sel!(accessibilityCustomActions)"));
    assert!(MACOS_NODE_RS.contains("Action::CustomAction"));
    assert!(MACOS_PATCH.contains("+    \"NSAccessibilityCustomAction\","));
    assert!(MACOS_PATCH.contains("+#[method_id(accessibilityCustomActions)]") || MACOS_PATCH.contains("+        #[method_id(accessibilityCustomActions)]"));

    assert!(ATSPI_NODE_RS.contains("fn custom_actions(&self)"));
    assert!(ATSPI_NODE_RS.contains("ActionData::CustomAction(ca.id)"));
    assert!(ATSPI_PATCH.contains("+    fn custom_actions(&self)"));
}
