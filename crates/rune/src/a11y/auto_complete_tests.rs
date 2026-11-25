//! Headless tests for `AutoComplete` and `El::auto_complete` across AccessKit,
//! `accessibility_json()`, incremental updates, anchored popup modals, and the web DOM mirror.

use accesskit::{AutoComplete as AkAutoComplete, HasPopup as AkHasPopup, Role};
use serde_json::{Value, json};

use crate::edit::{Key, KeyEvent, Mods, TextEdit};
use crate::runtime::web_a11y::A11yMirror;
use crate::{AccessibilityLink, Align, AutoComplete, Cx, El, Env, HasPopup, Runtime, Side, TextStyle, Transition, col, el, text, text_input};

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

fn press_key<S: 'static>(rt: &mut Runtime<S>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(0.0);
}

#[test]
fn auto_complete_modes_reach_accesskit_json_and_incremental_updates() {
    struct Scene {
        combo: TextEdit,
        plain: TextEdit,
        search: TextEdit,
        mode: Option<AutoComplete>,
    }

    fn view(s: &Scene, _: &mut Cx<Scene>) -> El {
        let mut combo = text_input(&s.combo, TextStyle::new(14.0, 400.0))
            .single_line()
            .key("fruit")
            .role("combobox")
            .label("Fruit")
            .size(200.0, 24.0)
            .expanded(true)
            .has_popup(HasPopup::Listbox)
            .controls("list")
            .on_edit(|s: &mut Scene| &mut s.combo);
        if let Some(mode) = s.mode {
            combo = combo.auto_complete(mode);
        }
        let plain = text_input(&s.plain, TextStyle::new(14.0, 400.0))
            .single_line()
            .key("plain")
            .label("Plain")
            .size(200.0, 24.0)
            .on_edit(|s: &mut Scene| &mut s.plain);
        let search = text_input(&s.search, TextStyle::new(14.0, 400.0))
            .single_line()
            .key("search")
            .role("searchbox")
            .label("Search")
            .size(200.0, 24.0)
            .auto_complete(AutoComplete::Both)
            .on_edit(|s: &mut Scene| &mut s.search);
        let list = col()
            .key("list")
            .role("listbox")
            .label("Fruit")
            .child(el().key("apple").role("option").label("Apple").size(200.0, 20.0).child(text("Apple")));
        col().w_full().gap(6.0).child(combo).child(plain).child(search).child(list)
    }

    assert!(el().cold.is_empty(), "unset auto_complete allocates no cold storage");

    let mut rt = Runtime::new(
        Scene {
            combo: TextEdit::new("Ap"),
            plain: TextEdit::new(""),
            search: TextEdit::new(""),
            mode: None,
        },
        view,
        (320.0, 240.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let mut link = AccessibilityLink::new();
    link.activate();
    assert!(link.service(&mut rt, "Window").is_some());
    assert!(link.service(&mut rt, "Window").is_none(), "idle frame emits no update");

    // Unset omits autoComplete while searchbox carries Both.
    let tree = rt.accessibility_tree();
    let json = rt.accessibility_json();
    let (_, ak_fruit) = ak_node(&tree, "fruit");
    assert_eq!(ak_fruit.role(), Role::EditableComboBox);
    assert_eq!(ak_fruit.auto_complete(), None);
    assert!(find(&json, "fruit").unwrap().get("autoComplete").is_none());
    let (_, ak_plain) = ak_node(&tree, "plain");
    assert_eq!(ak_plain.auto_complete(), None);
    assert!(find(&json, "plain").unwrap().get("autoComplete").is_none());
    let (_, ak_search) = ak_node(&tree, "search");
    assert_eq!(ak_search.role(), Role::SearchInput);
    assert_eq!(ak_search.auto_complete(), Some(AkAutoComplete::Both));
    assert_eq!(find(&json, "search").unwrap()["autoComplete"], "both");

    // Transition None -> List -> Inline -> Both -> None: every step triggers incremental update.
    let cases = [
        (Some(AutoComplete::List), Some(AkAutoComplete::List), Some("list")),
        (Some(AutoComplete::Inline), Some(AkAutoComplete::Inline), Some("inline")),
        (Some(AutoComplete::Both), Some(AkAutoComplete::Both), Some("both")),
        (None, None, None),
    ];
    for (mode, expected_ak, expected_json) in cases {
        rt.update(|s, _| s.mode = mode);
        rt.frame(0.0);
        let update = link.service(&mut rt, "Window")
            .unwrap_or_else(|| panic!("changing auto_complete to {mode:?} must emit AccessKit update"));
        assert!(!update.nodes.is_empty());
        assert!(link.service(&mut rt, "Window").is_none(), "unchanged frame stays quiet");

        let tree = rt.accessibility_tree();
        let (_, ak_fruit) = ak_node(&tree, "fruit");
        assert_eq!(ak_fruit.auto_complete(), expected_ak);

        let json = rt.accessibility_json();
        let j_fruit = find(&json, "fruit").unwrap();
        assert_eq!(j_fruit.get("autoComplete").and_then(Value::as_str), expected_json);
    }
}

#[test]
fn editable_combobox_auto_complete_is_readable_closed_and_open_on_desktop_and_web_mirror() {
    struct Scene {
        fruit: TextEdit,
        open: bool,
        mode: Option<AutoComplete>,
    }

    fn view(s: &Scene, _: &mut Cx<Scene>) -> El {
        let mut combo = text_input(&s.fruit, TextStyle::new(14.0, 400.0))
            .single_line()
            .key("fruit")
            .role("combobox")
            .label("Fruit")
            .size(200.0, 28.0)
            .expanded(s.open)
            .has_popup(HasPopup::Listbox)
            .controls("fruit-list")
            .on_edit(|s: &mut Scene| &mut s.fruit);
        if let Some(mode) = s.mode {
            combo = combo.auto_complete(mode);
        }
        let other = el()
            .key("other")
            .role("button")
            .label("Other")
            .focusable(true)
            .size(80.0, 28.0)
            .on_press(|_: &mut Scene| {})
            .child(text("Other"));

        let mut root = col().w_full().h_full().gap(8.0).child(combo).child(other);
        if s.open {
            root = root.child(
                col()
                    .key("fruit-list")
                    .role("listbox")
                    .label("Fruit")
                    .transition(Transition::none())
                    .anchored_fit("fruit", Side::Bottom, Align::Start, 4.0)
                    .modal(true, |s: &mut Scene| s.open = false)
                    .child(
                        el()
                            .key("opt-apple")
                            .role("option")
                            .label("Apple")
                            .focusable(true)
                            .size(180.0, 24.0)
                            .on_press(|s: &mut Scene| s.open = false)
                            .child(text("Apple")),
                    ),
            );
        }
        root
    }

    let mut rt = Runtime::new(
        Scene {
            fruit: TextEdit::new("Ap"),
            open: false,
            mode: Some(AutoComplete::List),
        },
        view,
        (480.0, 320.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // 1. Popup CLOSED: readable in AccessKit, JSON, and web mirror (full + patch).
    let tree = rt.accessibility_tree();
    let (_, ak_closed) = ak_node(&tree, "fruit");
    assert_eq!(ak_closed.role(), Role::EditableComboBox);
    assert_eq!(ak_closed.auto_complete(), Some(AkAutoComplete::List));
    assert_eq!(ak_closed.has_popup(), Some(AkHasPopup::Listbox));
    assert_eq!(ak_closed.is_expanded(), Some(false));
    assert!(ak_closed.controls().is_empty());

    let json = rt.accessibility_json();
    let j_closed = find(&json, "fruit").expect("closed editable combobox in json");
    assert_eq!(j_closed["role"], "editable-combo-box");
    assert_eq!(j_closed["autoComplete"], "list");
    assert_eq!(j_closed["hasPopup"], "listbox");
    assert!(states(j_closed).contains(&"collapsed"));
    assert!(actions(j_closed).contains(&"focus"));

    let mut mirror = A11yMirror::default();
    let mut patch_mirror = A11yMirror::default();
    let closed_items: Vec<Value> = serde_json::from_str(rt.web_a11y(&mut mirror).expect("initial mirror")).unwrap();
    let w_closed = closed_items
        .iter()
        .find(|v| v["o"] == "combobox" && v["t"] == "Fruit")
        .unwrap_or_else(|| panic!("closed editable combobox missing from web mirror: {closed_items:?}"));
    assert_eq!(w_closed["ac"], "list");
    assert_eq!(w_closed["hp"], "listbox");
    assert_eq!(w_closed["x"], false);
    assert_eq!(w_closed["v"], "Ap");
    assert!(w_closed.get("ct").is_none());
    assert!(rt.core().web_a11y_patch(&mut patch_mirror).is_some());

    // Clicking the closed editable combobox in the web mirror focuses it and updates focus proxy.
    let closed_path = w_closed["p"].as_str().unwrap().to_owned();
    assert!(rt.web_a11y_action(&json!({ "kind": "a11y_press", "path": closed_path })));
    rt.frame(0.0);
    let focus_closed: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut mirror).expect("focus proxy")).unwrap();
    assert_eq!(focus_closed["o"], "combobox");
    assert_eq!(focus_closed["t"], "Fruit");
    assert_eq!(focus_closed["ac"], "list");
    assert_eq!(focus_closed["hp"], "listbox");
    assert_eq!(focus_closed["x"], false);

    // 2. Popup OPEN (anchored modal listbox): editable combobox trigger stays readable on desktop
    //    AND in the web mirror with autoComplete/ac, expanded, hasPopup, and controls, while
    //    background controls are pruned and actions on the kept trigger are blocked.
    rt.update(|s, _| s.open = true);
    rt.frame(1.0 / 60.0);
    rt.frame(2.0 / 60.0);

    let tree = rt.accessibility_tree();
    let (list_id, ak_list) = ak_node(&tree, "fruit-list");
    let (_, ak_open) = ak_node(&tree, "fruit");
    assert!(!ak_list.is_modal(), "listbox popup must not announce modal state");
    assert_eq!(ak_open.role(), Role::EditableComboBox);
    assert_eq!(ak_open.auto_complete(), Some(AkAutoComplete::List));
    assert_eq!(ak_open.has_popup(), Some(AkHasPopup::Listbox));
    assert_eq!(ak_open.is_expanded(), Some(true));
    assert_eq!(ak_open.controls(), &[list_id]);

    let json = rt.accessibility_json();
    let j_open = find(&json, "fruit").expect("open editable combobox kept in json");
    assert_eq!(j_open["role"], "editable-combo-box");
    assert_eq!(j_open["autoComplete"], "list");
    assert_eq!(j_open["hasPopup"], "listbox");
    assert_eq!(j_open["controls"], json!(["fruit-list"]));
    assert!(states(j_open).contains(&"expanded"));
    assert!(actions(j_open).is_empty(), "kept modal trigger is inert: {j_open}");
    assert!(find(&json, "other").is_none(), "background button pruned while modal popup open");

    let open_items: Vec<Value> = serde_json::from_str(rt.web_a11y(&mut mirror).expect("open mirror")).unwrap();
    let w_open = open_items
        .iter()
        .find(|v| v["o"] == "combobox" && v["t"] == "Fruit")
        .unwrap_or_else(|| panic!("open editable combobox missing from web mirror: {open_items:?}"));
    assert_eq!(w_open["ac"], "list");
    assert_eq!(w_open["hp"], "listbox");
    assert_eq!(w_open["ct"], "fruit-list");
    assert_eq!(w_open["x"], true);
    assert_eq!(w_open["v"], "Ap");
    assert!(open_items.iter().any(|v| v["o"] == "listbox" && v["i"] == "fruit-list"));
    assert!(!open_items.iter().any(|v| v["t"] == "Other"), "background button pruned in web mirror");
    assert!(!rt.web_a11y_action(&json!({ "kind": "a11y_press", "path": w_open["p"].as_str().unwrap() })));

    let open_patch: Value = serde_json::from_str(rt.core().web_a11y_patch(&mut patch_mirror).expect("open patch")).unwrap();
    let upserts = open_patch["u"].as_array().expect("upserts on open");
    let p_combo = upserts.iter().find(|v| v["o"] == "combobox" && v["t"] == "Fruit").expect("combo patched");
    assert_eq!((&p_combo["ac"], &p_combo["ct"], &p_combo["x"]), (&json!("list"), &json!("fruit-list"), &json!(true)));

    // 3. Escape closes popup, and changing mode patches "ac" cleanly to "both", "inline", and omitted.
    press_key(&mut rt, Key::Escape);
    assert!(!rt.state.open);
    let _ = rt.core().web_a11y_patch(&mut patch_mirror);

    for (next_mode, expected) in [
        (Some(AutoComplete::Both), Some("both")),
        (Some(AutoComplete::Inline), Some("inline")),
        (None, None),
    ] {
        rt.update(|s, _| s.mode = next_mode);
        rt.frame(0.0);
        let patch: Value = serde_json::from_str(
            rt.core().web_a11y_patch(&mut patch_mirror).expect("auto_complete change emits patch"),
        ).unwrap();
        let u = patch["u"].as_array().expect("upserts");
        let item = u.iter().find(|v| v["o"] == "combobox" && v["t"] == "Fruit").expect("combo updated");
        assert_eq!(item.get("ac").and_then(Value::as_str), expected);
    }
}

#[test]
fn web_focus_proxy_carries_focused_input_auto_complete_with_active_descendant() {
    struct Scene {
        query: TextEdit,
    }
    fn view(s: &Scene, _: &mut Cx<Scene>) -> El {
        let field = text_input(&s.query, TextStyle::new(14.0, 400.0))
            .single_line()
            .key("search")
            .label("Search fruit")
            .size(200.0, 28.0)
            .auto_complete(AutoComplete::List)
            .active_descendant("opt-apple")
            .on_edit(|s: &mut Scene| &mut s.query);
        let list = col()
            .key("list")
            .role("listbox")
            .label("Fruit")
            .child(el().key("opt-apple").role("option").label("Apple").size(200.0, 24.0).child(text("Apple")));
        col().w_full().child(field).child(list)
    }

    let mut query = TextEdit::new("Ap");
    query.focused = true;
    let mut rt = Runtime::new(Scene { query }, view, (320.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let mut mirror = A11yMirror::default();
    let focus: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut mirror).expect("focus json")).unwrap();
    assert_eq!(focus["o"], "option");
    assert_eq!(focus["t"], "Apple");
    assert_eq!(focus["f"], "Search fruit");
    assert_eq!(focus["fac"], "list");
}

#[test]
fn auto_complete_edges_secure_disabled_read_only_and_rtl() {
    struct Scene {
        secret: TextEdit,
        disabled: TextEdit,
        read_only: TextEdit,
        rtl: TextEdit,
    }
    fn view(s: &Scene, _: &mut Cx<Scene>) -> El {
        col().w_full().gap(6.0)
            .child(
                text_input(&s.secret, TextStyle::new(14.0, 400.0))
                    .single_line().key("sec").role("combobox").label("Secret").size(200.0, 24.0)
                    .auto_complete(AutoComplete::List)
                    .on_edit(|s: &mut Scene| &mut s.secret),
            )
            .child(
                text_input(&s.disabled, TextStyle::new(14.0, 400.0))
                    .single_line().key("dis").role("combobox").label("Disabled combo").size(200.0, 24.0)
                    .disabled(true).auto_complete(AutoComplete::Inline)
                    .on_edit(|s: &mut Scene| &mut s.disabled),
            )
            .child(
                text_input(&s.read_only, TextStyle::new(14.0, 400.0))
                    .single_line().key("ro").role("combobox").label("Read-only combo").size(200.0, 24.0)
                    .read_only(true).auto_complete(AutoComplete::Both)
                    .on_edit(|s: &mut Scene| &mut s.read_only),
            )
            .child(
                text_input(&s.rtl, TextStyle::new(14.0, 400.0))
                    .single_line().key("rtl").role("combobox").label("الفاكهة").size(200.0, 24.0)
                    .auto_complete(AutoComplete::List)
                    .on_edit(|s: &mut Scene| &mut s.rtl),
            )
    }

    let mut rt = Runtime::new(
        Scene {
            secret: TextEdit::new("hunter2").secure(),
            disabled: TextEdit::new("Locked"),
            read_only: TextEdit::new("Fixed"),
            rtl: TextEdit::new("تفاح"),
        },
        view,
        (360.0, 240.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let json = rt.accessibility_json();
    // Disabled editable combobox keeps autoComplete readable with no actions.
    let dis = find(&json, "dis").unwrap();
    assert_eq!(dis["autoComplete"], "inline");
    assert!(states(dis).contains(&"disabled"));
    assert!(actions(dis).is_empty());

    // Read-only editable combobox keeps autoComplete alongside read-only state and non-mutating actions.
    let ro = find(&json, "ro").unwrap();
    assert_eq!(ro["autoComplete"], "both");
    assert!(states(ro).contains(&"read-only"));
    assert!(actions(ro).contains(&"focus") && !actions(ro).contains(&"set-value"));

    // RTL editable combobox preserves label, value, and autoComplete verbatim.
    let rtl = find(&json, "rtl").unwrap();
    assert_eq!((&rtl["name"], &rtl["value"], &rtl["autoComplete"]), (&json!("الفاكهة"), &json!("تفاح"), &json!("list")));

    // Web mirror never emits secure editable comboboxes or leaks their value, while mirroring the rest.
    let a11y_str = rt.a11y_json();
    assert!(!a11y_str.contains("hunter2"), "secure combobox value must not leak: {a11y_str}");
    let items: Vec<Value> = serde_json::from_str(&a11y_str).unwrap();
    assert!(!items.iter().any(|v| v["t"] == "Secret"), "secure combobox omitted from web mirror");
    let w_dis = items.iter().find(|v| v["t"] == "Disabled combo").expect("disabled combo in web mirror");
    assert_eq!((&w_dis["ac"], &w_dis["dis"]), (&json!("inline"), &json!(true)));
    assert!(!rt.web_a11y_action(&json!({ "kind": "a11y_press", "path": w_dis["p"].as_str().unwrap() })));
    let w_ro = items.iter().find(|v| v["t"] == "Read-only combo").expect("read-only combo in web mirror");
    assert_eq!((&w_ro["ac"], &w_ro["v"]), (&json!("both"), &json!("Fixed")));
    let w_rtl = items.iter().find(|v| v["t"] == "الفاكهة").expect("rtl combo in web mirror");
    assert_eq!((&w_rtl["ac"], &w_rtl["v"]), (&json!("list"), &json!("تفاح")));
}
