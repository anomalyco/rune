//! Headless tests for `Current` and `El::current` across AccessKit,
//! `accessibility_json()`, incremental updates, web DOM mirror/patches/focus proxy,
//! and static HTML export (`--html`).

use accesskit::AriaCurrent as AkAriaCurrent;
use serde_json::{Value, json};

use crate::edit::{Key, KeyEvent, Mods};
use crate::runtime::web_a11y::A11yMirror;
use crate::{AccessibilityLink, Current, Cx, El, Env, Runtime, col, el, row, text};

fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    if v["key"] == key { return Some(v); }
    v["children"].as_array()?.iter().find_map(|c| find(c, key))
}

fn ak_node<'a>(tree: &'a accesskit::TreeUpdate, key: &str) -> (accesskit::NodeId, &'a accesskit::Node) {
    tree.nodes
        .iter()
        .find(|(_, n)| n.author_id() == Some(key))
        .map(|(id, n)| (*id, n))
        .unwrap_or_else(|| panic!("missing AccessKit node keyed {key:?}"))
}

fn find_web<'a>(items: &'a [Value], key: &str) -> &'a Value {
    let suffix = format!("/{key}");
    items
        .iter()
        .find(|v| v["p"].as_str().is_some_and(|p| p == key || p.ends_with(&suffix)))
        .unwrap_or_else(|| panic!("missing web mirror item keyed {key:?}: {items:?}"))
}

#[test]
fn current_variants_reach_accesskit_json_and_incremental_updates() {
    struct Scene {
        nav_current: Option<Current>,
    }

    fn view(s: &Scene, _: &mut Cx<Scene>) -> El {
        let mut home = el()
            .key("home")
            .role("link")
            .label("Home")
            .focusable(true)
            .size(120.0, 28.0)
            .on_press(|_: &mut Scene| {});
        if let Some(cur) = s.nav_current {
            home = home.current(cur);
        }
        let settings = el()
            .key("settings")
            .role("link")
            .label("Settings")
            .focusable(true)
            .size(120.0, 28.0)
            .on_press(|_: &mut Scene| {});
        col()
            .key("nav")
            .role("navigation")
            .label("Sections")
            .w_full()
            .gap(4.0)
            .child(home)
            .child(settings)
            .child(el().key("step").role("listitem").label("Step 2").current(Current::Step).size(120.0, 24.0))
            .child(el().key("loc").role("link").label("Docs").current(Current::Location).size(120.0, 24.0))
            .child(el().key("date").role("button").label("27").current(Current::Date).size(40.0, 24.0))
            .child(el().key("time").role("button").label("09:00").current(Current::Time).size(60.0, 24.0))
            .child(el().key("item").role("button").label("Active").current(Current::True).size(80.0, 24.0))
    }

    assert!(el().cold.is_empty(), "unset current allocates no cold storage");

    let mut rt = Runtime::new(Scene { nav_current: None }, view, (400.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let mut link = AccessibilityLink::new();
    link.activate();
    let initial = link.service(&mut rt, "Nav").expect("initial AccessKit tree");
    let (home_id, ak_home) = ak_node(&initial, "home");
    assert_eq!(ak_home.aria_current(), None);
    assert_eq!(ak_node(&initial, "settings").1.aria_current(), None);
    assert_eq!(ak_node(&initial, "step").1.aria_current(), Some(AkAriaCurrent::Step));
    assert_eq!(ak_node(&initial, "loc").1.aria_current(), Some(AkAriaCurrent::Location));
    assert_eq!(ak_node(&initial, "date").1.aria_current(), Some(AkAriaCurrent::Date));
    assert_eq!(ak_node(&initial, "time").1.aria_current(), Some(AkAriaCurrent::Time));
    assert_eq!(ak_node(&initial, "item").1.aria_current(), Some(AkAriaCurrent::True));

    let json_init = rt.accessibility_json();
    assert_eq!(find(&json_init, "home").unwrap().get("current"), None);
    assert_eq!(find(&json_init, "settings").unwrap().get("current"), None);
    assert_eq!(find(&json_init, "step").unwrap()["current"], json!("step"));
    assert_eq!(find(&json_init, "loc").unwrap()["current"], json!("location"));
    assert_eq!(find(&json_init, "date").unwrap()["current"], json!("date"));
    assert_eq!(find(&json_init, "time").unwrap()["current"], json!("time"));
    assert_eq!(find(&json_init, "item").unwrap()["current"], json!("true"));

    // Incremental updates: set each token in turn, then clear back to None.
    for (next, expected_ak, expected_json) in [
        (Some(Current::Page), Some(AkAriaCurrent::Page), Some("page")),
        (Some(Current::Step), Some(AkAriaCurrent::Step), Some("step")),
        (Some(Current::Location), Some(AkAriaCurrent::Location), Some("location")),
        (Some(Current::Date), Some(AkAriaCurrent::Date), Some("date")),
        (Some(Current::Time), Some(AkAriaCurrent::Time), Some("time")),
        (Some(Current::True), Some(AkAriaCurrent::True), Some("true")),
        (None, None, None),
    ] {
        rt.update(|s, _| s.nav_current = next);
        rt.frame(0.0);
        let update = link
            .service(&mut rt, "Nav")
            .unwrap_or_else(|| panic!("changing current to {next:?} must emit AccessKit update"));
        let (_, updated) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == home_id)
            .expect("home node must be included in incremental update");
        assert_eq!(updated.aria_current(), expected_ak);

        let tree_json = rt.accessibility_json();
        let entry = find(&tree_json, "home").expect("home in accessibility_json");
        assert_eq!(entry.get("current").and_then(Value::as_str), expected_json);
    }
}

#[test]
fn current_mirrors_to_web_a11y_json_patches_focus_proxy_and_static_html() {
    struct Scene {
        active_home: Option<Current>,
    }

    fn view(s: &Scene, _: &mut Cx<Scene>) -> El {
        let mut press_link = el()
            .key("nav-home")
            .role("link")
            .label("Home")
            .focusable(true)
            .size(120.0, 28.0)
            .on_press(|_: &mut Scene| {});
        if let Some(cur) = s.active_home {
            press_link = press_link.current(cur);
        }
        let mut href_link = row()
            .key("href-docs")
            .href("/docs")
            .size(120.0, 28.0)
            .child(text("Docs"));
        if let Some(cur) = s.active_home {
            href_link = href_link.current(cur);
        }
        col()
            .key("root")
            .role("navigation")
            .label("Sections")
            .w_full()
            .gap(4.0)
            .child(press_link)
            .child(href_link)
            .child(el().key("nav-other").role("link").label("Settings").focusable(true).size(120.0, 28.0).on_press(|_: &mut Scene| {}))
            .child(col().key("li-step").role("listitem").current(Current::Step).child(text("Step 2")))
            .child(text("Overview").key("h-cur").role("heading").level(2).current(Current::Page))
            .child(row().gap(4.0).child(text("Plain").key("t-plain")).child(text("Now").key("t-cur").current(Current::Time)))
            .child(el().key("pin").role("image").label("Here").current(Current::Location).size(24.0, 24.0))
            .child(el().key("hidden-cur").href("/secret").a11y_hidden().current(Current::Page).size(80.0, 20.0).child(text("Hidden")))
    }

    let mut rt = Runtime::new(Scene { active_home: Some(Current::Page) }, view, (480.0, 360.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // 1. Web accessibility mirror full snapshot (`a11y_json()`).
    let items: Vec<Value> = serde_json::from_str(&rt.a11y_json()).expect("valid a11y_json");
    assert_eq!(find_web(&items, "nav-home").get("cu"), Some(&json!("page")));
    assert_eq!(find_web(&items, "href-docs").get("cu"), Some(&json!("page")));
    assert_eq!(find_web(&items, "nav-other").get("cu"), None);
    assert_eq!(find_web(&items, "li-step").get("cu"), Some(&json!("step")));
    assert_eq!(find_web(&items, "h-cur").get("cu"), Some(&json!("page")));
    assert_eq!(find_web(&items, "t-plain").get("cu"), None, "plain sibling text stays unmerged and unset");
    assert_eq!(find_web(&items, "t-cur").get("cu"), Some(&json!("time")), "text with current is not merged into plain sibling");
    assert_eq!(find_web(&items, "pin").get("cu"), Some(&json!("location")));

    // 2. Incremental patch updates `cu` and clears it when removed.
    let mut mirror = A11yMirror::default();
    let _ = rt.core().web_a11y_patch(&mut mirror).expect("initial mirror patch");

    rt.update(|s, _| s.active_home = Some(Current::Location));
    rt.frame(0.0);
    let patch: Value = serde_json::from_str(
        rt.core().web_a11y_patch(&mut mirror).expect("changing current emits patch"),
    )
    .unwrap();
    let upserts = patch["u"].as_array().expect("patch upserts");
    assert_eq!(find_web(upserts, "nav-home").get("cu"), Some(&json!("location")));
    assert_eq!(find_web(upserts, "href-docs").get("cu"), Some(&json!("location")));

    rt.update(|s, _| s.active_home = None);
    rt.frame(0.0);
    let patch: Value = serde_json::from_str(
        rt.core().web_a11y_patch(&mut mirror).expect("clearing current emits patch"),
    )
    .unwrap();
    let upserts = patch["u"].as_array().expect("patch upserts");
    assert_eq!(find_web(upserts, "nav-home").get("cu"), None);
    assert_eq!(find_web(upserts, "href-docs").get("cu"), None);

    // 3. Web focus proxy carries `cu` on focused nav link and omits it on non-current sibling.
    rt.update(|s, _| s.active_home = Some(Current::Page));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    let focus_home: Value = serde_json::from_str(
        rt.core().web_a11y_focus(&mut mirror).expect("focus proxy for nav-home"),
    )
    .unwrap();
    assert_eq!(focus_home["o"], json!("link"));
    assert_eq!(focus_home["t"], json!("Home"));
    assert_eq!(focus_home.get("cu"), Some(&json!("page")));

    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    let focus_other: Value = serde_json::from_str(
        rt.core().web_a11y_focus(&mut mirror).expect("focus proxy for nav-other"),
    )
    .unwrap();
    assert_eq!(focus_other["t"], json!("Settings"));
    assert_eq!(focus_other.get("cu"), None);

    // 4. Static HTML export (`--html`) emits `aria-current="..."` and omits it when hidden/unset.
    let exported = rt.export_nodes();
    let by_key = |k: &str| {
        let suffix = format!("/{k}");
        exported
            .iter()
            .find(|n| n.path == k || n.path.ends_with(&suffix))
            .unwrap_or_else(|| panic!("missing exported node {k}"))
    };
    assert!(by_key("href-docs").attrs.contains("aria-current=\"page\""), "attrs: {}", by_key("href-docs").attrs);
    assert!(by_key("li-step").attrs.contains("aria-current=\"step\""), "attrs: {}", by_key("li-step").attrs);
    assert!(by_key("h-cur").attrs.contains("aria-current=\"page\""), "attrs: {}", by_key("h-cur").attrs);
    assert!(!by_key("nav-other").attrs.contains("aria-current"), "unset node must omit aria-current");
    assert!(!by_key("hidden-cur").attrs.contains("aria-current"), "a11y_hidden node must omit aria-current");
}
