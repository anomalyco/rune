//! Completeness checks for the accessibility JSON dump's action table.

use std::collections::HashSet;

use accesskit::Action;
use serde_json::Value;

use super::dump::ACTIONS;
use crate::{Cx, El, Env, Link, Runtime, TextEdit, col, el, rich, row, text, text_input};

/// Every variant of `accesskit::Action`, kept exhaustive by the match below so an
/// upgrade that adds a new action variant cannot silently escape the check.
const ALL_ACCESSKIT_ACTIONS: [Action; 22] = [
    Action::Click,
    Action::Focus,
    Action::Blur,
    Action::Collapse,
    Action::Expand,
    Action::CustomAction,
    Action::Decrement,
    Action::Increment,
    Action::HideTooltip,
    Action::ShowTooltip,
    Action::ReplaceSelectedText,
    Action::ScrollDown,
    Action::ScrollLeft,
    Action::ScrollRight,
    Action::ScrollUp,
    Action::ScrollIntoView,
    Action::ScrollToPoint,
    Action::SetScrollOffset,
    Action::SetTextSelection,
    Action::SetSequentialFocusNavigationStartingPoint,
    Action::SetValue,
    Action::ShowContextMenu,
];

const _: () = {
    let mut i = 0;
    while i < ALL_ACCESSKIT_ACTIONS.len() {
        let _ = match ALL_ACCESSKIT_ACTIONS[i] {
            Action::Click
            | Action::Focus
            | Action::Blur
            | Action::Collapse
            | Action::Expand
            | Action::CustomAction
            | Action::Decrement
            | Action::Increment
            | Action::HideTooltip
            | Action::ShowTooltip
            | Action::ReplaceSelectedText
            | Action::ScrollDown
            | Action::ScrollLeft
            | Action::ScrollRight
            | Action::ScrollUp
            | Action::ScrollIntoView
            | Action::ScrollToPoint
            | Action::SetScrollOffset
            | Action::SetTextSelection
            | Action::SetSequentialFocusNavigationStartingPoint
            | Action::SetValue
            | Action::ShowContextMenu => (),
        };
        i += 1;
    }
};

#[derive(Default)]
struct AllActions {
    field: TextEdit,
}

fn scene(s: &AllActions, _: &mut Cx<AllActions>) -> El {
    col().w_full().gap(4.0)
        .child(
            row().key("btn").role("button").focusable(true)
                .on_press(|_: &mut AllActions| {})
                .on_secondary_press(|_: &mut AllActions, _, _| {})
                .child(text("Action")),
        )
        .child(
            row().key("closed").role("button").expanded(false)
                .on_press(|_: &mut AllActions| {})
                .child(text("Closed")),
        )
        .child(
            row().key("open").role("button").expanded(true)
                .on_press(|_: &mut AllActions| {})
                .child(text("Open")),
        )
        .child(
            el().key("slider").role("slider").label("Volume")
                .range_value(5.0, 0.0, 10.0).range_step(1.0)
                .size(100.0, 12.0)
                .on_step(|_: &mut AllActions, _, _| {}),
        )
        .child(
            text_input(&s.field, crate::tok::type_::BODY).key("field").label("Name")
                .single_line().w(160.0)
                .on_edit(|s: &mut AllActions| &mut s.field),
        )
        .child(
            rich("Read the spec now.", vec![]).key("para").w(240.0).focusable(true)
                .link(Link::new(9..13, "https://example.com/spec"))
                .on_link(|_: &mut AllActions, _, _| {})
                .on_link_secondary(|_: &mut AllActions, _, _, _| {}),
        )
        .child(
            col().key("scroller").size(120.0, 80.0).scroll_xy()
                .child(
                    col().size(400.0, 300.0)
                        .child(text("Inside scroll").key("inside")),
                ),
        )
}

fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    if v["key"] == key { return Some(v); }
    v["children"].as_array()?.iter().find_map(|c| find(c, key))
}

fn collect_json_actions<'a>(v: &'a Value, out: &mut HashSet<&'a str>) {
    if let Some(arr) = v["actions"].as_array() {
        for s in arr.iter().filter_map(Value::as_str) {
            out.insert(s);
        }
    }
    if let Some(children) = v["children"].as_array() {
        for c in children {
            collect_json_actions(c, out);
        }
    }
}

#[test]
fn dump_actions_cover_every_emitted_action_including_scroll_right() {
    let mut rt = Runtime::new(AllActions::default(), scene, (400.0, 500.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let table: Vec<Action> = ACTIONS.iter().map(|(a, _)| *a).collect();
    let mut seen_table_actions = HashSet::new();
    let mut seen_table_names = HashSet::new();
    for &(action, name) in &ACTIONS {
        assert!(seen_table_actions.insert(action as u8), "duplicate action in dump::ACTIONS: {action:?}");
        assert!(seen_table_names.insert(name), "duplicate name in dump::ACTIONS: {name:?}");
    }

    let tree = rt.accessibility_tree();
    let mut emitted = HashSet::new();
    for (_, node) in &tree.nodes {
        for action in ALL_ACCESSKIT_ACTIONS {
            if node.supports_action(action) {
                assert!(
                    table.contains(&action),
                    "node {:?} (key={:?}) supports {action:?}, which is missing from a11y::dump::ACTIONS",
                    node.role(),
                    node.author_id(),
                );
                emitted.insert(action as u8);
            }
        }
    }

    for &(action, name) in &ACTIONS {
        assert!(
            emitted.contains(&(action as u8)),
            "a11y::dump::ACTIONS entry ({action:?}, {name:?}) was not emitted by any node in the test scene",
        );
    }

    let json = rt.accessibility_json();
    let scroller = find(&json, "scroller").expect("two-way scroll view in accessibility JSON");
    let scroller_actions: Vec<&str> = scroller["actions"]
        .as_array()
        .expect("scroller actions array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for expected in ["scroll-up", "scroll-down", "scroll-left", "scroll-right", "set-scroll-offset"] {
        assert!(
            scroller_actions.contains(&expected),
            "scroller actions {scroller_actions:?} missing {expected:?}",
        );
    }

    let mut json_actions = HashSet::new();
    collect_json_actions(&json, &mut json_actions);
    for &(_, name) in &ACTIONS {
        assert!(json_actions.contains(name), "dumped JSON missing action {name:?}");
    }
}
