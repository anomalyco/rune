//! Headless tests for `El::aria_disabled`, `HasPopup`, `Orientation`, and the WAI-ARIA
//! setters `multiselectable`, `required`, `has_popup`, `orientation`, `controls`, and `owns`.

use accesskit::{Action as Ak, HasPopup as AkHasPopup, Orientation as AkOrientation};
use serde_json::{Value, json};

use crate::edit::{Key, KeyEvent, Mods};
use crate::runtime::web_a11y::A11yMirror;
use crate::{AccessibilityLink, Cx, El, Env, HasPopup, Orientation, Runtime, col, el, row, text};

fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    if v["key"] == key { return Some(v); }
    v["children"].as_array()?.iter().find_map(|c| find(c, key))
}

fn states(v: &Value) -> Vec<&str> {
    v["states"].as_array().map_or(Vec::new(), |s| s.iter().filter_map(Value::as_str).collect())
}

fn actions(v: &Value) -> Vec<&str> {
    v["actions"].as_array().map_or(Vec::new(), |s| s.iter().filter_map(Value::as_str).collect())
}

fn ak_node<'a>(tree: &'a accesskit::TreeUpdate, key: &str) -> (accesskit::NodeId, &'a accesskit::Node) {
    tree.nodes
        .iter()
        .find(|(_, n)| n.author_id() == Some(key))
        .map(|(id, n)| (*id, n))
        .unwrap_or_else(|| panic!("missing AccessKit node keyed {key:?}"))
}

fn focused_key<S>(rt: &Runtime<S>) -> Option<String> {
    let tree = rt.accessibility_tree();
    tree.nodes.iter().find(|(id, _)| *id == tree.focus).and_then(|(_, n)| n.author_id().map(str::to_owned))
}

fn press_key<S: 'static>(rt: &mut Runtime<S>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(0.0);
}

#[test]
fn aria_disabled_keeps_focusability_order_independently_and_emits_focus_action() {
    #[derive(Default)]
    struct Pending {
        presses: u32,
        hovered_a: bool,
        hovered_b: bool,
        pending: bool,
    }
    fn view(s: &Pending, _: &mut Cx<Pending>) -> El {
        col().w_full().gap(8.0)
            // Focusable then aria_disabled.
            .child(
                row().key("pending-a").role("button").size(120.0, 32.0)
                    .focusable(true)
                    .aria_disabled(s.pending)
                    .on_hover_change(|s: &mut Pending, on, _| s.hovered_a = on)
                    .child(text("Saving A")),
            )
            // aria_disabled then focusable (order-independent).
            .child(
                row().key("pending-b").role("button").size(120.0, 32.0)
                    .aria_disabled(s.pending)
                    .focusable(true)
                    .on_hover_change(|s: &mut Pending, on, _| s.hovered_b = on)
                    .child(text("Saving B")),
            )
            // Pending button that mistakenly left on_press attached: activation still suppressed in a11y.
            .child(
                row().key("pending-with-handler").role("button").size(120.0, 32.0)
                    .focusable(true)
                    .aria_disabled(s.pending)
                    .expanded(false)
                    .on_press(|s: &mut Pending| s.presses += 1)
                    .on_secondary_press(|s: &mut Pending, _, _| s.presses += 10)
                    .child(text("Handler")),
            )
            // Both disabled(true) and aria_disabled(true): stays out of Tab order (either order).
            .child(
                row().key("hard-disabled-first").role("button").size(120.0, 32.0)
                    .focusable(true)
                    .disabled(true)
                    .aria_disabled(true)
                    .child(text("Hard A")),
            )
            .child(
                row().key("hard-disabled-second").role("button").size(120.0, 32.0)
                    .focusable(true)
                    .aria_disabled(true)
                    .disabled(true)
                    .child(text("Hard B")),
            )
            // aria_disabled alone does not imply focusable.
            .child(
                row().key("unfocusable-aria").role("button").size(120.0, 32.0)
                    .aria_disabled(true)
                    .child(text("Not focusable")),
            )
            // Ordinary disabled(true) stays unfocusable and unchanged.
            .child(
                row().key("plain-disabled").role("button").size(120.0, 32.0)
                    .focusable(true)
                    .disabled(true)
                    .child(text("Plain disabled")),
            )
    }

    let mut rt = Runtime::new(Pending { pending: true, ..Default::default() }, view, (320.0, 360.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let tree = rt.accessibility_tree();
    let json = rt.accessibility_json();
    for key in ["pending-a", "pending-b", "pending-with-handler"] {
        let (_, ak) = ak_node(&tree, key);
        assert!(ak.is_disabled(), "{key} must report is_disabled()");
        assert!(ak.supports_action(Ak::Focus), "{key} must advertise Action::Focus");
        assert!(!ak.supports_action(Ak::Click), "{key} must not advertise Action::Click");
        assert!(!ak.supports_action(Ak::Expand), "{key} must not advertise Action::Expand");
        assert!(!ak.supports_action(Ak::ShowContextMenu), "{key} must not advertise ShowContextMenu");

        let j = find(&json, key).unwrap();
        assert!(states(j).contains(&"disabled"), "{key} json missing disabled state: {j}");
        assert!(actions(j).contains(&"focus"), "{key} json missing focus action: {j}");
        assert!(!actions(j).contains(&"click"), "{key} json should not advertise click: {j}");
    }

    for key in ["hard-disabled-first", "hard-disabled-second", "unfocusable-aria", "plain-disabled"] {
        let (_, ak) = ak_node(&tree, key);
        assert!(ak.is_disabled(), "{key} must report is_disabled()");
        assert!(!ak.supports_action(Ak::Focus), "{key} must not advertise Action::Focus");
        assert!(!ak.supports_action(Ak::Click), "{key} must not advertise Action::Click");
    }

    // Tab lands on pending-a, then pending-b, then pending-with-handler, skipping all four unfocusable buttons.
    press_key(&mut rt, Key::Tab);
    assert_eq!(focused_key(&rt).as_deref(), Some("pending-a"));
    let json = rt.accessibility_json();
    let pa = find(&json, "pending-a").unwrap();
    assert!(states(pa).contains(&"focused") && states(pa).contains(&"disabled"), "focused+disabled: {pa}");
    press_key(&mut rt, Key::Text(" ".into()));
    assert_eq!(rt.state.presses, 0, "Space on aria-disabled button does not activate");

    press_key(&mut rt, Key::Tab);
    assert_eq!(focused_key(&rt).as_deref(), Some("pending-b"));
    press_key(&mut rt, Key::Tab);
    assert_eq!(focused_key(&rt).as_deref(), Some("pending-with-handler"));
    press_key(&mut rt, Key::Tab);
    assert_eq!(focused_key(&rt).as_deref(), Some("pending-a"), "hard-disabled and unfocusable buttons stay out of Tab order");

    // Hover still works on aria_disabled buttons.
    let (x, y) = rt.find("pending-a").unwrap();
    rt.pointer_move(x, y);
    rt.frame(0.0);
    assert!(rt.state.hovered_a, "aria_disabled button keeps hover");
    let (x, y) = rt.find("pending-b").unwrap();
    rt.pointer_move(x, y);
    rt.frame(0.0);
    assert!(rt.state.hovered_b, "order-reversed aria_disabled button keeps hover");

    // AccessKit Action::Focus moves focus to an aria_disabled button, while Action::Click is refused.
    assert!(rt.accessibility_action("pending-with-handler", Ak::Focus, None));
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("pending-with-handler"));
    assert!(!rt.accessibility_action("pending-with-handler", Ak::Click, None));
    assert!(!rt.accessibility_action("pending-with-handler", Ak::Expand, None));
    assert!(!rt.accessibility_action("pending-with-handler", Ak::ShowContextMenu, None));
    rt.frame(0.0);
    assert_eq!(rt.state.presses, 0);

    // Clearing aria_disabled(false) restores actionable state and incremental updates pick it up.
    let mut link = AccessibilityLink::new();
    link.activate();
    assert!(link.service(&mut rt, "Pending").is_some());

    rt.update(|s, _| s.pending = false);
    rt.frame(0.0);
    let update = link.service(&mut rt, "Pending").expect("clearing aria_disabled updates tree");
    assert!(!update.nodes.is_empty());
    let tree = rt.accessibility_tree();
    let (_, ak) = ak_node(&tree, "pending-with-handler");
    assert!(!ak.is_disabled());
    assert!(ak.supports_action(Ak::Focus) && ak.supports_action(Ak::Click) && ak.supports_action(Ak::Expand));
}

#[test]
fn aria_disabled_false_on_fresh_el_allocates_no_cold_storage() {
    let bare = el().aria_disabled(false);
    assert!(bare.cold.is_empty(), "aria_disabled(false) on fresh El should not allocate cold storage");
    let cleared = el().aria_disabled(true).aria_disabled(false);
    assert!(!cleared.cold.a11y.disabled);
    let empty_rel = el().multiselectable(false).required(false).controls("").owns("");
    assert!(empty_rel.cold.is_empty(), "false/empty ARIA setters on fresh El should not allocate cold storage");
}

#[test]
fn wai_aria_setters_reach_accesskit_and_accessibility_json() {
    struct Scene {
        open: bool,
        multi: bool,
        req: bool,
        popup: HasPopup,
        orient: Orientation,
    }
    fn view(s: &Scene, _: &mut Cx<Scene>) -> El {
        let mut root = col().w_full().gap(8.0)
            .child(
                row().key("trigger").role("button").label("Fruit").size(120.0, 32.0)
                    .focusable(true)
                    .expanded(s.open)
                    .has_popup(s.popup)
                    .controls("popup-list")
                    .child(text("Fruit")),
            )
            .child(
                row().key("combo-owner").role("combobox").label("Picker").size(140.0, 32.0)
                    .focusable(true)
                    .required(s.req)
                    .owns("popup-list")
                    .child(text("Picker")),
            )
            .child(
                row().key("pop-menu").role("button").label("Menu").size(80.0, 28.0).has_popup(HasPopup::Menu),
            )
            .child(
                row().key("pop-tree").role("button").label("Tree").size(80.0, 28.0).has_popup(HasPopup::Tree),
            )
            .child(
                row().key("pop-grid").role("button").label("Grid").size(80.0, 28.0).has_popup(HasPopup::Grid),
            )
            .child(
                row().key("pop-dialog").role("button").label("Dialog").size(80.0, 28.0).has_popup(HasPopup::Dialog),
            )
            .child(
                row().key("tabs").role("tablist").label("Sections").orientation(s.orient).focus_group()
                    .child(row().key("tab-1").role("tab").size(60.0, 24.0).focusable(true).focus_selected(true).child(text("One"))),
            )
            // A focus_group without explicit .orientation(..) must leave orientation unset.
            .child(
                col().key("default-group").role("listbox").label("Default").focus_group()
                    .child(row().key("dg-1").role("option").size(60.0, 24.0).focusable(true).child(text("Item"))),
            )
            // Plain control without any of the six setters stays completely unchanged.
            .child(
                row().key("plain").role("button").label("Plain").size(80.0, 24.0).focusable(true),
            );
        // Root-attached overlay popup: not a descendant of trigger or combo-owner.
        if s.open {
            root = root.child(
                col().key("popup-list").role("listbox").label("Fruit options")
                    .multiselectable(s.multi)
                    .required(s.req)
                    .orientation(Orientation::Vertical)
                    .child(row().key("opt-apple").role("option").size(100.0, 24.0).focusable(true).focus_selected(true).child(text("Apple")))
                    .child(row().key("opt-banana").role("option").size(100.0, 24.0).focusable(true).focus_selected(true).child(text("Banana"))),
            );
        }
        root
    }

    let mut rt = Runtime::new(
        Scene { open: true, multi: true, req: true, popup: HasPopup::Listbox, orient: Orientation::Horizontal },
        view,
        (400.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let tree = rt.accessibility_tree();
    let json = rt.accessibility_json();

    let (popup_id, popup_ak) = ak_node(&tree, "popup-list");
    assert!(popup_ak.is_multiselectable());
    assert!(popup_ak.is_required());
    assert_eq!(popup_ak.orientation(), Some(AkOrientation::Vertical));

    let (_, trigger_ak) = ak_node(&tree, "trigger");
    assert_eq!(trigger_ak.has_popup(), Some(AkHasPopup::Listbox));
    assert_eq!(trigger_ak.controls(), &[popup_id]);

    let (_, owner_ak) = ak_node(&tree, "combo-owner");
    assert!(owner_ak.is_required());
    assert_eq!(owner_ak.owns(), &[popup_id]);

    assert_eq!(ak_node(&tree, "pop-menu").1.has_popup(), Some(AkHasPopup::Menu));
    assert_eq!(ak_node(&tree, "pop-tree").1.has_popup(), Some(AkHasPopup::Tree));
    assert_eq!(ak_node(&tree, "pop-grid").1.has_popup(), Some(AkHasPopup::Grid));
    assert_eq!(ak_node(&tree, "pop-dialog").1.has_popup(), Some(AkHasPopup::Dialog));

    assert_eq!(ak_node(&tree, "tabs").1.orientation(), Some(AkOrientation::Horizontal));
    assert_eq!(ak_node(&tree, "default-group").1.orientation(), None, "focus_group keeps default orientation unset");

    let (_, plain_ak) = ak_node(&tree, "plain");
    assert!(!plain_ak.is_multiselectable());
    assert!(!plain_ak.is_required());
    assert_eq!(plain_ak.has_popup(), None);
    assert_eq!(plain_ak.orientation(), None);
    assert!(plain_ak.controls().is_empty());
    assert!(plain_ak.owns().is_empty());

    // Check accessibility_json() serialization.
    let trigger_j = find(&json, "trigger").unwrap();
    assert_eq!(trigger_j["hasPopup"], "listbox");
    assert_eq!(trigger_j["controls"], json!(["popup-list"]));

    let owner_j = find(&json, "combo-owner").unwrap();
    assert_eq!(owner_j["owns"], json!(["popup-list"]));
    assert!(states(owner_j).contains(&"required"));

    let popup_j = find(&json, "popup-list").unwrap();
    assert_eq!(popup_j["orientation"], "vertical");
    assert!(states(popup_j).contains(&"multiselectable"));
    assert!(states(popup_j).contains(&"required"));

    assert_eq!(find(&json, "pop-menu").unwrap()["hasPopup"], "menu");
    assert_eq!(find(&json, "pop-tree").unwrap()["hasPopup"], "tree");
    assert_eq!(find(&json, "pop-grid").unwrap()["hasPopup"], "grid");
    assert_eq!(find(&json, "pop-dialog").unwrap()["hasPopup"], "dialog");
    assert_eq!(find(&json, "tabs").unwrap()["orientation"], "horizontal");

    let plain_j = find(&json, "plain").unwrap();
    for field in ["hasPopup", "orientation", "controls", "owns"] {
        assert!(plain_j.get(field).is_none(), "unset {field} must be omitted from JSON");
    }

    // Incremental AccessibilityLink updates detect changes to all six properties and target presence.
    let mut link = AccessibilityLink::new();
    link.activate();
    assert!(link.service(&mut rt, "Demo").is_some());
    assert!(link.service(&mut rt, "Demo").is_none());

    // Close the popup overlay: controls/owns must drop cleanly without dangling NodeIds.
    rt.update(|s, _| s.open = false);
    rt.frame(0.0);
    let upd = link.service(&mut rt, "Demo").expect("closing controlled/owned popup updates tree");
    assert!(!upd.nodes.is_empty());
    let tree = rt.accessibility_tree();
    assert!(ak_node(&tree, "trigger").1.controls().is_empty());
    assert!(ak_node(&tree, "combo-owner").1.owns().is_empty());

    // Mutate popup kind, orientation, required, and reopen with multiselectable=false.
    rt.update(|s, _| {
        s.open = true;
        s.multi = false;
        s.req = false;
        s.popup = HasPopup::Dialog;
        s.orient = Orientation::Vertical;
    });
    rt.frame(0.0);
    let upd = link.service(&mut rt, "Demo").expect("mutating ARIA properties updates tree");
    assert!(!upd.nodes.is_empty());
    let tree = rt.accessibility_tree();
    assert_eq!(ak_node(&tree, "trigger").1.has_popup(), Some(AkHasPopup::Dialog));
    assert_eq!(ak_node(&tree, "tabs").1.orientation(), Some(AkOrientation::Vertical));
    assert!(!ak_node(&tree, "combo-owner").1.is_required());
    assert!(!ak_node(&tree, "popup-list").1.is_multiselectable());
}

#[test]
fn controls_and_owns_scope_resolution_and_edge_cases() {
    struct Scopes {
        hide_target: bool,
    }
    fn view(s: &Scopes, _: &mut Cx<Scopes>) -> El {
        let mut target = col().key("opt-target").role("region").label("Hidden test").size(40.0, 20.0);
        if s.hide_target {
            target = target.a11y_hidden();
        }
        col().w_full().gap(4.0)
            // Component A: trigger controls its own sibling "panel", not component B's "panel".
            .child(
                col().key("comp-a")
                    .child(row().key("tab-a").role("tab").label("Tab A").size(60.0, 20.0).controls("panel"))
                    .child(col().key("panel").role("tabpanel").label("Panel A").size(60.0, 20.0)),
            )
            // Component B: its own "panel".
            .child(
                col().key("comp-b")
                    .child(row().key("tab-b").role("tab").label("Tab B").size(60.0, 20.0).controls("panel"))
                    .child(col().key("panel").role("tabpanel").label("Panel B").size(60.0, 20.0)),
            )
            // Self-reference must not resolve to itself.
            .child(
                row().key("self-ref").role("button").label("Self").size(60.0, 20.0)
                    .controls("self-ref")
                    .owns("self-ref"),
            )
            // Clearing a previously set key with "" produces no relation.
            .child(
                row().key("cleared").role("button").label("Cleared").size(60.0, 20.0)
                    .controls("opt-target").controls("")
                    .owns("opt-target").owns(""),
            )
            // Target that can be hidden (a11y_hidden), role("none"), or missing.
            .child(
                row().key("watch-hidden").role("button").label("Watch").size(60.0, 20.0)
                    .controls("opt-target")
                    .owns("omitted-role"),
            )
            // Slash-separated key path resolution.
            .child(
                row().key("slash-ref").role("button").label("Slash").size(60.0, 20.0)
                    .controls("overlay/list"),
            )
            .child(target)
            .child(col().key("omitted-role").role("none").size(40.0, 20.0))
            .child(
                col().key("overlay")
                    .child(col().key("list").role("listbox").label("Overlay list").size(60.0, 20.0)),
            )
    }

    let mut rt = Runtime::new(Scopes { hide_target: true }, view, (320.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_tree();

    // Nearest scope wins for duplicate "panel" keys.
    let panel_a_id = tree.nodes.iter()
        .find(|(_, n)| n.author_id() == Some("panel") && n.label() == Some("Panel A"))
        .map(|(id, _)| *id)
        .unwrap();
    let panel_b_id = tree.nodes.iter()
        .find(|(_, n)| n.author_id() == Some("panel") && n.label() == Some("Panel B"))
        .map(|(id, _)| *id)
        .unwrap();
    assert_ne!(panel_a_id, panel_b_id);
    assert_eq!(ak_node(&tree, "tab-a").1.controls(), &[panel_a_id]);
    assert_eq!(ak_node(&tree, "tab-b").1.controls(), &[panel_b_id]);

    // Self-ref and cleared emit nothing.
    assert!(ak_node(&tree, "self-ref").1.controls().is_empty());
    assert!(ak_node(&tree, "self-ref").1.owns().is_empty());
    assert!(ak_node(&tree, "cleared").1.controls().is_empty());
    assert!(ak_node(&tree, "cleared").1.owns().is_empty());

    // Hidden target and role("none") target emit no dangling NodeIds.
    assert!(ak_node(&tree, "watch-hidden").1.controls().is_empty());
    assert!(ak_node(&tree, "watch-hidden").1.owns().is_empty());

    // Slash-separated key path resolves to overlay/list.
    let (overlay_list_id, _) = ak_node(&tree, "list");
    assert_eq!(ak_node(&tree, "slash-ref").1.controls(), &[overlay_list_id]);

    // Revealing the hidden target connects controls on the next frame.
    let mut link = AccessibilityLink::new();
    link.activate();
    assert!(link.service(&mut rt, "Scopes").is_some());
    rt.update(|s, _| s.hide_target = false);
    rt.frame(0.0);
    assert!(link.service(&mut rt, "Scopes").is_some());
    let tree = rt.accessibility_tree();
    let (opt_target_id, _) = ak_node(&tree, "opt-target");
    assert_eq!(ak_node(&tree, "watch-hidden").1.controls(), &[opt_target_id]);
}

#[test]
fn web_mirror_and_focus_proxy_reflect_aria_disabled_and_wai_aria_setters() {
    struct WebDemo {
        pending: bool,
        open: bool,
        presses: u32,
    }
    fn view(s: &WebDemo, _: &mut Cx<WebDemo>) -> El {
        let mut root = col().w_full().gap(8.0)
            .child(
                row().key("pending-btn").role("button").size(120.0, 32.0)
                    .focusable(true)
                    .aria_disabled(s.pending)
                    .on_press(|s: &mut WebDemo| s.presses += 1)
                    .child(text("Save")),
            )
            .child(
                row().key("popup-btn").role("button").size(120.0, 32.0)
                    .focusable(true)
                    .expanded(s.open)
                    .has_popup(HasPopup::Listbox)
                    .controls("fruit-list")
                    .owns("fruit-list")
                    .on_press(|s: &mut WebDemo| s.open = !s.open)
                    .child(text("Fruit")),
            );
        if s.open {
            root = root.child(
                col().key("fruit-list").role("listbox").label("Fruit")
                    .multiselectable(true)
                    .orientation(Orientation::Vertical)
                    .required(true)
                    .child(
                        row().key("opt-a").role("option").size(100.0, 24.0)
                            .focusable(true)
                            .on_press(|_: &mut WebDemo| {})
                            .child(text("Apple")),
                    ),
            );
        }
        root
    }

    let mut rt = Runtime::new(WebDemo { pending: true, open: true, presses: 0 }, view, (320.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let mut mirror = A11yMirror::default();
    let full: Vec<Value> = serde_json::from_str(rt.web_a11y(&mut mirror).expect("initial mirror")).unwrap();

    let pending_item = full.iter().find(|v| v["t"] == "Save").expect("pending button in mirror");
    assert_eq!(pending_item["dis"], true, "web mirror carries aria-disabled: {pending_item}");
    let pending_path = pending_item["p"].as_str().unwrap().to_owned();

    let trigger_item = full.iter().find(|v| v["t"] == "Fruit").expect("trigger in mirror");
    assert_eq!(trigger_item["hp"], "listbox");
    assert_eq!(trigger_item["ct"], "fruit-list");
    assert_eq!(trigger_item["ow"], "fruit-list");
    assert_eq!(trigger_item["x"], true);

    let list_item = full.iter().find(|v| v["o"] == "listbox").expect("listbox in mirror");
    assert_eq!(list_item["i"], "fruit-list");
    assert_eq!(list_item["ms"], true);
    assert_eq!(list_item["rq"], true);
    assert_eq!(list_item["or"], "vertical");

    // Web mirror press on an aria-disabled button is refused.
    assert!(!rt.web_a11y_action(&json!({"kind": "a11y_press", "path": pending_path})));
    rt.frame(0.0);
    assert_eq!(rt.state.presses, 0);

    // Focus proxy reports aria-disabled on pending button and popup/controls/owns on trigger.
    press_key(&mut rt, Key::Tab);
    assert_eq!(focused_key(&rt).as_deref(), Some("pending-btn"));
    let focus_a: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut mirror).expect("focus proxy")).unwrap();
    assert_eq!(focus_a["o"], "button");
    assert_eq!(focus_a["dis"], true);

    press_key(&mut rt, Key::Tab);
    assert_eq!(focused_key(&rt).as_deref(), Some("popup-btn"));
    let focus_b: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut mirror).expect("focus proxy 2")).unwrap();
    assert_eq!(focus_b["hp"], "listbox");
    assert_eq!(focus_b["ct"], "fruit-list");
    assert_eq!(focus_b["ow"], "fruit-list");
    assert!(focus_b.get("dis").is_none());

    // Incremental web mirror patch detects clearing aria_disabled and closing popup.
    let mut patch_mirror = A11yMirror::default();
    assert!(rt.core().web_a11y_patch(&mut patch_mirror).is_some());
    assert!(rt.core().web_a11y_patch(&mut patch_mirror).is_none());

    rt.update(|s, _| {
        s.pending = false;
        s.open = false;
    });
    rt.frame(0.0);
    let patch: Value = serde_json::from_str(rt.core().web_a11y_patch(&mut patch_mirror).expect("incremental patch")).unwrap();
    assert!(patch.get("d").is_some(), "closed popup should be removed in patch: {patch}");
    let upserts = patch["u"].as_array().expect("updated buttons in patch");
    let save_u = upserts.iter().find(|v| v["t"] == "Save").expect("Save updated in patch");
    assert!(save_u.get("di").is_none(), "cleared aria_disabled omitted from patch: {save_u}");
    let fruit_u = upserts.iter().find(|v| v["t"] == "Fruit").expect("Fruit updated in patch");
    assert!(fruit_u.get("ct").is_none() && fruit_u.get("ow").is_none(), "closed popup drops ct/ow: {fruit_u}");
}
