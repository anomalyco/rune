//! Order-independence tests for `El::disabled` / `El::aria_disabled` / `El::focusable(false)`
//! combined with builders that enable focus (`El::link`, `El::on_step`, `El::focusable(true)`,
//! `El::dir_by_paragraph`).
use accesskit::Action as Ak;
use serde_json::{Value, json};

use super::*;
use crate::runtime::web_a11y::A11yMirror;
use crate::{Cursor, Key, KeyEvent, Link, Mods, TextStyle, rich, text};

fn press_key<S: 'static>(rt: &mut Runtime<S>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(0.0);
}

fn ak_node<'a>(tree: &'a accesskit::TreeUpdate, id: &str) -> &'a accesskit::Node {
    tree.nodes
        .iter()
        .find(|(_, n)| n.author_id() == Some(id))
        .map(|(_, n)| n)
        .unwrap_or_else(|| panic!("missing AccessKit node {id:?}"))
}

fn mirror_items<S>(rt: &Runtime<S>) -> Vec<Value> {
    let mut mirror = A11yMirror::default();
    serde_json::from_str(rt.web_a11y(&mut mirror).expect("web a11y mirror")).unwrap()
}

fn find_mirror_by_key<'a>(items: &'a [Value], key: &str) -> &'a Value {
    items
        .iter()
        .find(|v| v["p"].as_str().is_some_and(|p| p.ends_with(&format!("/{key}")) || p == key))
        .unwrap_or_else(|| panic!("missing mirror item for key {key:?}: {items:?}"))
}

#[test]
fn disabled_before_or_after_link_keeps_link_unfocusable_inactive_and_disabled_in_a11y() {
    #[derive(Default)]
    struct State {
        opened: Vec<String>,
        secondary: Vec<String>,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        let st = TextStyle::new(14.0, 400.0).lh(20.0);
        let mk_link = |url: &'static str| Link::new(4..8, url);
        col()
            .size(320.0, 320.0)
            .gap(6.0)
            .child(el().key("before-btn").role("button").label("Before").size(100.0, 24.0).focusable(true))
            .child(
                rich("See docs", Vec::new())
                    .key("dis-then-link")
                    .font(st)
                    .w(200.0)
                    .disabled(true)
                    .link(mk_link("https://before.example"))
                    .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned()))
                    .on_link_secondary(|s: &mut State, id, _, _| s.secondary.push(id.to_owned())),
            )
            .child(
                rich("See docs", Vec::new())
                    .key("link-then-dis")
                    .font(st)
                    .w(200.0)
                    .link(mk_link("https://after.example"))
                    .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned()))
                    .on_link_secondary(|s: &mut State, id, _, _| s.secondary.push(id.to_owned()))
                    .disabled(true),
            )
            .child(
                col().key("dis-anc").disabled(true).child(
                    rich("See docs", Vec::new())
                        .key("under-dis-anc")
                        .font(st)
                        .w(200.0)
                        .link(mk_link("https://anc.example"))
                        .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned())),
                ),
            )
            .child(
                rich("See docs", Vec::new())
                    .key("aria-then-link")
                    .font(st)
                    .w(200.0)
                    .aria_disabled(true)
                    .link(mk_link("https://aria-before.example"))
                    .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned()))
                    .on_link_secondary(|s: &mut State, id, _, _| s.secondary.push(id.to_owned())),
            )
            .child(
                rich("See docs", Vec::new())
                    .key("link-then-aria")
                    .font(st)
                    .w(200.0)
                    .link(mk_link("https://aria-after.example"))
                    .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned()))
                    .on_link_secondary(|s: &mut State, id, _, _| s.secondary.push(id.to_owned()))
                    .aria_disabled(true),
            )
            .child(
                rich("See docs", Vec::new())
                    .key("enabled-link")
                    .font(st)
                    .w(200.0)
                    .link(mk_link("https://ok.example"))
                    .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned())),
            )
            .child(el().key("after-btn").role("button").label("After").size(100.0, 24.0).focusable(true))
    }

    let mut rt = Runtime::new(State::default(), view, (320.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // 1. Tab skips disabled links in either builder order and under a disabled ancestor,
    //    while keeping aria_disabled and enabled links in Tab order.
    let mut visited = Vec::new();
    for _ in 0..8 {
        press_key(&mut rt, Key::Tab);
        let k = rt.focused_key().unwrap_or_default();
        if visited.first() == Some(&k) { break; }
        visited.push(k);
    }
    assert_eq!(
        visited,
        [
            "before-btn",
            "https://aria-before.example",
            "https://aria-after.example",
            "https://ok.example",
            "after-btn",
        ],
        "disabled links in either builder order must stay out of Tab order"
    );

    // 2. Pressing Enter/Space/ContextMenu on focused aria_disabled links must not activate them;
    //    web focus proxy reports dis:true.
    for id in ["https://aria-before.example", "https://aria-after.example"] {
        rt.update(move |_, tx| tx.focus(id, true));
        rt.frame(0.0);
        assert_eq!(rt.focused_key().as_deref(), Some(id), "aria_disabled link stays focusable");
        let mut mirror = A11yMirror::default();
        let focus_json = rt.core().web_a11y_focus(&mut mirror).expect("focus proxy");
        assert!(focus_json.contains(r#""dis":true"#), "focus proxy reports disabled for {id}: {focus_json}");
        press_key(&mut rt, Key::Enter);
        press_key(&mut rt, Key::Text(" ".into()));
        press_key(&mut rt, Key::ContextMenu);
    }
    assert!(rt.state.opened.is_empty(), "aria_disabled links must not activate: {:?}", rt.state.opened);
    assert!(rt.state.secondary.is_empty(), "aria_disabled links must not open link menu: {:?}", rt.state.secondary);

    // 3. Tx::focus refuses hard-disabled links in either order or under a disabled ancestor.
    for id in ["https://before.example", "https://after.example", "https://anc.example"] {
        rt.update(move |_, tx| tx.focus(id, true));
        rt.frame(0.0);
        assert_ne!(rt.focused_key().as_deref(), Some(id), "Tx::focus must refuse disabled link {id}");
    }

    // 4. Pointer click, right-click, and hover cursor: disabled and aria_disabled links ignore
    //    clicks and show Cursor::Default; enabled link keeps Cursor::Pointer and activates.
    for id in [
        "https://before.example",
        "https://after.example",
        "https://anc.example",
        "https://aria-before.example",
        "https://aria-after.example",
    ] {
        let (x, y) = rt.find(id).expect(id);
        rt.pointer_move(x, y);
        assert_eq!(rt.cursor(), Cursor::Default, "{id} must show default cursor");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(0.0);
    }
    assert!(rt.state.opened.is_empty(), "disabled links must not activate on click: {:?}", rt.state.opened);
    assert!(rt.state.secondary.is_empty(), "disabled links must not run secondary on click: {:?}", rt.state.secondary);

    // Enabled link works by both Enter and pointer click.
    rt.update(|_, tx| tx.focus("https://ok.example", true));
    rt.frame(0.0);
    assert_eq!(rt.focused_key().as_deref(), Some("https://ok.example"));
    press_key(&mut rt, Key::Enter);
    assert_eq!(rt.state.opened, ["https://ok.example"]);
    let (ox, oy) = rt.find("https://ok.example").unwrap();
    rt.pointer_move(ox, oy);
    assert_eq!(rt.cursor(), Cursor::Pointer, "enabled link keeps pointer cursor");
    rt.pointer_down(ox, oy);
    rt.pointer_up(ox, oy);
    rt.frame(0.0);
    assert_eq!(rt.state.opened, ["https://ok.example", "https://ok.example"]);

    // 5. AccessKit tree agrees: disabled links expose is_disabled() and omit Focus/Click/ShowContextMenu;
    //    aria_disabled links expose is_disabled() + Focus and omit Click/ShowContextMenu.
    let tree = rt.accessibility_tree();
    for id in ["https://before.example", "https://after.example", "https://anc.example"] {
        let n = ak_node(&tree, id);
        assert!(n.is_disabled(), "{id} must be disabled in AccessKit");
        assert!(!n.supports_action(Ak::Focus), "{id} must not advertise Focus in AccessKit");
        assert!(!n.supports_action(Ak::Click), "{id} must not advertise Click in AccessKit");
        assert!(!n.supports_action(Ak::ShowContextMenu), "{id} must not advertise ShowContextMenu in AccessKit");
    }
    for id in ["https://aria-before.example", "https://aria-after.example"] {
        let n = ak_node(&tree, id);
        assert!(n.is_disabled(), "{id} must be disabled in AccessKit");
        assert!(n.supports_action(Ak::Focus), "{id} must keep Focus in AccessKit");
        assert!(!n.supports_action(Ak::Click), "{id} must not advertise Click in AccessKit");
        assert!(!n.supports_action(Ak::ShowContextMenu), "{id} must not advertise ShowContextMenu in AccessKit");
    }
    let ok_ak = ak_node(&tree, "https://ok.example");
    assert!(!ok_ak.is_disabled());
    assert!(ok_ak.supports_action(Ak::Focus) && ok_ak.supports_action(Ak::Click));

    // 6. Web mirror agrees: disabled links carry dis:true and refuse a11y_link; enabled omits dis and accepts a11y_link.
    let items = mirror_items(&rt);
    for key in ["dis-then-link", "link-then-dis", "under-dis-anc", "aria-then-link", "link-then-aria"] {
        let item = find_mirror_by_key(&items, key);
        assert_eq!(item.get("dis"), Some(&json!(true)), "{key} must expose dis:true in web mirror: {item}");
        let path = item["p"].as_str().unwrap().to_owned();
        assert!(!rt.web_a11y_action(&json!({"kind": "a11y_link", "path": path, "link": 0})), "{key} must refuse a11y_link");
    }
    let ok_item = find_mirror_by_key(&items, "enabled-link");
    assert_eq!(ok_item.get("dis"), None, "enabled link omits dis: {ok_item}");
    let ok_path = ok_item["p"].as_str().unwrap().to_owned();
    assert!(rt.web_a11y_action(&json!({"kind": "a11y_link", "path": ok_path, "link": 0})));
    rt.frame(0.0);
    assert_eq!(rt.state.opened.len(), 3);
}

#[test]
fn disabled_before_or_after_other_focus_builders_is_order_independent() {
    #[derive(Default)]
    struct State {
        steps: Vec<&'static str>,
        opened: Vec<String>,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        let st = TextStyle::new(14.0, 400.0).lh(20.0);
        let mixed = "Hello\nمرحبا docs";
        let docs_start = mixed.find("docs").unwrap();
        let docs_range = docs_start..docs_start + 4;
        col()
            .size(320.0, 420.0)
            .gap(4.0)
            .child(el().key("start").role("button").label("Start").size(80.0, 20.0).focusable(true))
            .child(
                el().key("dis-then-step").role("slider").label("DS").size(100.0, 20.0)
                    .disabled(true)
                    .on_step(|s: &mut State, _, _| s.steps.push("dis-then-step")),
            )
            .child(
                el().key("step-then-dis").role("slider").label("SD").size(100.0, 20.0)
                    .on_step(|s: &mut State, _, _| s.steps.push("step-then-dis"))
                    .disabled(true),
            )
            .child(
                el().key("aria-then-step").role("slider").label("AS").size(100.0, 20.0)
                    .aria_disabled(true)
                    .on_step(|s: &mut State, _, _| s.steps.push("aria-then-step")),
            )
            .child(
                el().key("step-then-aria").role("slider").label("SA").size(100.0, 20.0)
                    .on_step(|s: &mut State, _, _| s.steps.push("step-then-aria"))
                    .aria_disabled(true),
            )
            .child(
                el().key("dis-then-focusable").role("button").label("DF").size(100.0, 20.0)
                    .disabled(true)
                    .focusable(true),
            )
            .child(
                el().key("focusable-then-dis").role("button").label("FD").size(100.0, 20.0)
                    .focusable(true)
                    .disabled(true),
            )
            .child(
                rich("See docs", Vec::new()).key("optout-then-link").font(st).w(200.0)
                    .focusable(false)
                    .link(Link::new(4..8, "https://optout-before.example"))
                    .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned())),
            )
            .child(
                rich("See docs", Vec::new()).key("link-then-optout").font(st).w(200.0)
                    .link(Link::new(4..8, "https://optout-after.example"))
                    .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned()))
                    .focusable(false),
            )
            .child(
                el().key("optout-then-step").role("slider").label("OS").size(100.0, 20.0)
                    .focusable(false)
                    .on_step(|s: &mut State, _, _| s.steps.push("optout-then-step")),
            )
            .child(
                el().key("step-then-optout").role("slider").label("SO").size(100.0, 20.0)
                    .on_step(|s: &mut State, _, _| s.steps.push("step-then-optout"))
                    .focusable(false),
            )
            .child(
                text(mixed).key("dis-then-para").font(st).w(200.0)
                    .disabled(true)
                    .link(Link::new(docs_range.clone(), "https://para-before.example"))
                    .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned()))
                    .dir_by_paragraph(),
            )
            .child(
                text(mixed).key("para-then-dis").font(st).w(200.0)
                    .link(Link::new(docs_range, "https://para-after.example"))
                    .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned()))
                    .dir_by_paragraph()
                    .disabled(true),
            )
            .child(el().key("end").role("button").label("End").size(80.0, 20.0).focusable(true))
    }

    let mut rt = Runtime::new(State::default(), view, (320.0, 420.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Tab only visits start, the two aria_disabled sliders, and end.
    let mut visited = Vec::new();
    for _ in 0..8 {
        press_key(&mut rt, Key::Tab);
        let k = rt.focused_key().unwrap_or_default();
        if visited.first() == Some(&k) { break; }
        visited.push(k);
    }
    assert_eq!(visited, ["start", "aria-then-step", "step-then-aria", "end"]);

    // Focused aria_disabled sliders must not step on arrows.
    for key in ["aria-then-step", "step-then-aria"] {
        rt.update(move |_, tx| tx.focus(key, true));
        rt.frame(0.0);
        assert_eq!(rt.focused_key().as_deref(), Some(key));
        press_key(&mut rt, Key::Right);
        press_key(&mut rt, Key::Up);
    }
    assert!(rt.state.steps.is_empty(), "aria_disabled sliders must not step: {:?}", rt.state.steps);

    // Hard-disabled and opt-out controls reject Tx::focus and AccessKit Focus.
    let tree = rt.accessibility_tree();
    for key in [
        "dis-then-step",
        "step-then-dis",
        "dis-then-focusable",
        "focusable-then-dis",
        "https://para-before.example",
        "https://para-after.example",
    ] {
        rt.update(move |_, tx| tx.focus(key, true));
        rt.frame(0.0);
        assert_ne!(rt.focused_key().as_deref(), Some(key), "Tx::focus must refuse {key}");
        let n = ak_node(&tree, key);
        assert!(n.is_disabled(), "{key} must be disabled in AccessKit");
        assert!(!n.supports_action(Ak::Focus), "{key} must not advertise Focus in AccessKit");
    }

    // Enabled links that opted out of Tab via focusable(false) remain clickable with the pointer.
    for id in ["https://optout-before.example", "https://optout-after.example"] {
        let (x, y) = rt.find(id).expect(id);
        rt.pointer_move(x, y);
        assert_eq!(rt.cursor(), Cursor::Pointer, "enabled opt-out link {id} keeps pointer cursor");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
    }
    assert_eq!(rt.state.opened, ["https://optout-before.example", "https://optout-after.example"]);
}
