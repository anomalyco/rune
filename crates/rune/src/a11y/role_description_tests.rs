//! Headless tests for `El::role_description` and `El::role_description_static` across AccessKit,
//! `accessibility_json()`, incremental updates, the web DOM mirror + focus proxy, and static HTML export.

use accesskit::Role;
use serde_json::Value;

use crate::runtime::web_a11y::A11yMirror;
use crate::{
    AccessibilityLink, Cx, El, Env, Runtime, ScrollSpec, TextEdit, TextStyle, col, el, row, text,
    text_input,
};

fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    if v["key"] == key { return Some(v); }
    v["children"].as_array()?.iter().find_map(|c| find(c, key))
}

fn ak_node<'a>(tree: &'a accesskit::TreeUpdate, key: &str) -> Option<&'a accesskit::Node> {
    tree.nodes.iter().find(|(_, n)| n.author_id() == Some(key)).map(|(_, n)| n)
}

#[test]
fn role_description_reaches_accesskit_json_and_ignores_roleless_or_empty_elements() {
    struct Scene {
        edit: TextEdit,
    }

    fn view(s: &Scene, _: &mut Cx<Scene>) -> El {
        col().w_full().gap(4.0)
            // Valid semantic roles.
            .child(
                el().key("card").role("article").label("Row one").focusable(true)
                    .role_description("card").size(200.0, 24.0)
                    .child(text("Row one")),
            )
            .child(
                row().key("chip").role("button").label("Today")
                    .role_description_static("divider")
                    .size(100.0, 24.0)
                    .on_press(|_: &mut Scene| {})
                    .child(text("Today")),
            )
            .child(
                row().key("implicit-btn").label("Jump")
                    .role_description("separator chip")
                    .size(100.0, 24.0)
                    .on_press(|_: &mut Scene| {})
                    .child(text("Jump")),
            )
            .child(
                col().key("explicit-group").role("group").label("Slide 1")
                    .role_description("slide").size(200.0, 24.0),
            )
            .child(
                col().key("unlabelled-group").role("group")
                    .role_description_static("unlabelled slide").size(200.0, 24.0),
            )
            .child(
                col().key("region-slide").role("region").label("Intro")
                    .role_description("slide").size(200.0, 24.0),
            )
            .child(
                text("Chapter 1").key("heading").role("heading").level(2)
                    .role_description("chapter title"),
            )
            .child(
                row().key("ext-link").href("https://example.com")
                    .role_description("external link").size(120.0, 20.0)
                    .child(text("Docs")),
            )
            .child(
                el().key("spark").role("image").label("CPU")
                    .role_description("sparkline").size(60.0, 20.0),
            )
            .child(
                el().key("progress").role("progressbar").label("Upload")
                    .range_value(0.5, 0.0, 1.0).role_description("step meter")
                    .size(100.0, 10.0),
            )
            .child(
                text_input(&s.edit, TextStyle::new(14.0, 400.0))
                    .single_line().key("cmd").label("Command")
                    .role_description("command bar").size(180.0, 24.0)
                    .on_edit(|s: &mut Scene| &mut s.edit),
            )
            // Ignored: elements with no accessible role, omitted roles, unknown roles, or blank text.
            .child(el().role_description("orphan").size(10.0, 10.0))
            .child(el().key("plain-keyed").role_description("ignored").size(10.0, 10.0))
            .child(el().key("plain-labelled").label("Box").role_description("ignored").size(10.0, 10.0))
            .child(el().key("plain-focusable").focusable(true).role_description("ignored").size(10.0, 10.0))
            .child(
                col().key("plain-scroll").scroll(ScrollSpec::default())
                    .role_description("ignored").size(100.0, 40.0)
                    .child(el().size(100.0, 80.0)),
            )
            .child(el().key("role-none").role("none").role_description("ignored").size(10.0, 10.0))
            .child(el().key("role-pres").role("presentation").role_description("ignored").size(10.0, 10.0))
            .child(el().key("role-empty").role("").role_description("ignored").size(10.0, 10.0))
            .child(el().key("role-unknown").role("made-up").role_description("ignored").size(10.0, 10.0))
            .child(el().key("blank-rd").role("article").role_description("   ").size(10.0, 10.0))
            .child(
                el().key("cleared-rd").role("article").role_description("card")
                    .role_description_static("").size(10.0, 10.0),
            )
    }

    assert!(el().cold.is_empty());
    assert!(el().role_description("").cold.is_empty(), "empty role_description allocates no cold storage");
    assert!(el().role_description_static("   ").cold.is_empty(), "whitespace role_description_static allocates no cold storage");

    let mut rt = Runtime::new(Scene { edit: TextEdit::default() }, view, (400.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let tree = rt.accessibility_tree();
    let json = rt.accessibility_json();

    let expected = [
        ("card", Role::Article, "card"),
        ("chip", Role::Button, "divider"),
        ("implicit-btn", Role::Button, "separator chip"),
        ("explicit-group", Role::Group, "slide"),
        ("unlabelled-group", Role::Group, "unlabelled slide"),
        ("region-slide", Role::Region, "slide"),
        ("heading", Role::Heading, "chapter title"),
        ("ext-link", Role::Link, "external link"),
        ("spark", Role::Image, "sparkline"),
        ("progress", Role::ProgressIndicator, "step meter"),
        ("cmd", Role::TextInput, "command bar"),
    ];
    for (key, role, rd) in expected {
        let ak = ak_node(&tree, key).unwrap_or_else(|| panic!("missing AccessKit node {key:?}"));
        assert_eq!(ak.role(), role, "role for {key}");
        assert_eq!(ak.role_description(), Some(rd), "AccessKit role_description for {key}");
        let j = find(&json, key).unwrap_or_else(|| panic!("missing JSON node {key:?}"));
        assert_eq!(j["roleDescription"], rd, "accessibility_json roleDescription for {key}");
    }

    // Roleless, unknown-role, and blank role descriptions are never exposed.
    for key in ["plain-keyed", "plain-labelled", "plain-focusable", "plain-scroll", "role-empty", "role-unknown", "blank-rd", "cleared-rd"] {
        let ak = ak_node(&tree, key).unwrap_or_else(|| panic!("expected node {key:?} in tree"));
        assert_eq!(ak.role_description(), None, "{key} must not expose role_description");
        let j = find(&json, key).unwrap_or_else(|| panic!("expected node {key:?} in JSON"));
        assert!(j.get("roleDescription").is_none(), "{key} must omit roleDescription in JSON: {j}");
    }
    assert!(ak_node(&tree, "role-none").is_none());
    assert!(ak_node(&tree, "role-pres").is_none());
    assert!(!json.to_string().contains("orphan"));
}

#[test]
fn role_description_updates_incrementally_in_accesskit_and_web_mirror_and_focus_proxy() {
    struct Scene {
        rd: Option<&'static str>,
        edit: TextEdit,
    }

    fn view(s: &Scene, _: &mut Cx<Scene>) -> El {
        let mut card = el()
            .key("row-1")
            .role("article")
            .label("Row one")
            .focusable(true)
            .size(200.0, 28.0)
            .child(text("Row one"));
        if let Some(rd) = s.rd {
            card = card.role_description_static(rd);
        }
        let chip = row()
            .key("chip")
            .role("button")
            .label("Today")
            .focusable(true)
            .role_description("divider")
            .size(120.0, 24.0)
            .on_press(|_: &mut Scene| {})
            .child(text("Today"));
        let plain = el()
            .key("plain")
            .label("Plain box")
            .focusable(true)
            .role_description("ignored")
            .size(120.0, 24.0);
        let field = text_input(&s.edit, TextStyle::new(14.0, 400.0))
            .single_line()
            .key("cmd")
            .label("Command")
            .role_description("command bar")
            .size(180.0, 24.0)
            .on_edit(|s: &mut Scene| &mut s.edit);
        col().w_full().gap(4.0).child(card).child(chip).child(plain).child(field)
    }

    let mut rt = Runtime::new(Scene { rd: None, edit: TextEdit::default() }, view, (320.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let mut link = AccessibilityLink::new();
    link.activate();
    assert!(link.service(&mut rt, "Window").is_some());
    assert!(link.service(&mut rt, "Window").is_none(), "idle frame emits no update");

    let mut mirror = A11yMirror::default();
    let initial_patch = rt.core().web_a11y_patch(&mut mirror).expect("initial mirror patch");
    assert!(!initial_patch.contains(r#""rd":"card""#));
    assert!(initial_patch.contains(r#""rd":"divider""#));
    assert!(!initial_patch.contains(r#""rd":"ignored""#));

    // None -> Some("card") -> Some("tile") -> None updates both AccessKit and web mirror incrementally.
    for next in [Some("card"), Some("tile"), None] {
        rt.update(|s, _| s.rd = next);
        rt.frame(0.0);
        let upd = link.service(&mut rt, "Window").expect("role_description change emits AccessKit update");
        let row_ak = ak_node(&upd, "row-1").expect("row-1 rebuilt in incremental update");
        assert_eq!(row_ak.role_description(), next);

        let patch_str = rt.core().web_a11y_patch(&mut mirror).expect("role_description change emits mirror patch");
        let patch: Value = serde_json::from_str(patch_str).unwrap();
        let upserts = patch["u"].as_array().expect("patch upserts");
        let row_patch = upserts.iter().find(|u| u["g"] == "article").expect("article upserted");
        assert_eq!(row_patch.get("rd").and_then(Value::as_str), next);

        let items: Vec<Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
        let row_item = items.iter().find(|it| it["g"] == "article").expect("article in a11y_json");
        assert_eq!(row_item.get("rd").and_then(Value::as_str), next);
        let plain_item = items.iter().find(|it| it["t"] == "Plain box").expect("labelled group in a11y_json");
        assert!(plain_item.get("rd").is_none(), "plain labelled container without explicit role omits rd: {plain_item}");
    }

    // Focus proxy carries role="article" + rd="card", button rd="divider", textbox rd="command bar", and omits ignored rd.
    rt.update(|s, tx| {
        s.rd = Some("card");
        tx.focus("row-1", true);
    });
    rt.frame(0.0);
    let focus: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut mirror).unwrap()).unwrap();
    assert_eq!(focus["o"], "article");
    assert_eq!(focus["rd"], "card");

    rt.update(|_, tx| tx.focus("chip", true));
    rt.frame(0.0);
    let focus: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut mirror).unwrap()).unwrap();
    assert_eq!(focus["o"], "button");
    assert_eq!(focus["rd"], "divider");

    rt.update(|_, tx| tx.focus("plain", true));
    rt.frame(0.0);
    let focus: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut mirror).unwrap()).unwrap();
    assert!(focus.get("rd").is_none(), "focus proxy omits rd on roleless element: {focus}");

    rt.update(|_, tx| tx.focus("cmd", true));
    rt.frame(0.0);
    let focus: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut mirror).unwrap()).unwrap();
    assert_eq!(focus["o"], "textbox");
    assert_eq!(focus["rd"], "command bar");
}

#[test]
fn role_description_exports_in_static_html_only_on_roled_elements() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().key("root").w_full().role("main")
            .child(
                col().key("card").role("article").label("Row one")
                    .role_description(r#"card & "chip""#)
                    .child(text("Body")),
            )
            .child(
                row().key("chip").role("button").label("Today")
                    .role_description_static("divider")
                    .child(text("Today")),
            )
            .child(
                col().key("slide").role("group").label("Slide 1")
                    .role_description("slide"),
            )
            .child(
                row().key("lnk").href("https://example.com")
                    .role_description("external link")
                    .child(text("Docs")),
            )
            .child(
                text("Chapter 1").key("h").role("heading").level(2)
                    .role_description("chapter"),
            )
            .child(col().key("plain").label("No role").role_description("ignored"))
            .child(col().key("none").role("none").role_description("ignored"))
            .child(col().key("hidden").role("article").a11y_hidden().role_description("ignored"))
    }

    let mut rt = Runtime::new((), view, (400.0, 600.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    let nodes = rt.core().export_nodes();

    let by_path = |suffix: &str| {
        nodes.iter().find(|n| n.path.ends_with(suffix)).unwrap_or_else(|| panic!("missing export node {suffix}"))
    };

    let card = by_path("card");
    assert_eq!(card.tag, "article");
    assert!(card.attrs.contains(r#"aria-roledescription="card &amp; &quot;chip&quot;""#), "{}", card.attrs);

    let chip = by_path("chip");
    assert!(chip.attrs.contains(r#"role="button""#), "{}", chip.attrs);
    assert!(chip.attrs.contains(r#"aria-roledescription="divider""#), "{}", chip.attrs);

    let slide = by_path("slide");
    assert!(slide.attrs.contains(r#"role="group""#), "{}", slide.attrs);
    assert!(slide.attrs.contains(r#"aria-roledescription="slide""#), "{}", slide.attrs);

    let lnk = by_path("lnk");
    assert_eq!(lnk.tag, "a");
    assert!(lnk.attrs.contains(r#"aria-roledescription="external link""#), "{}", lnk.attrs);

    let h = by_path("h");
    assert_eq!(h.tag, "h2");
    assert!(h.attrs.contains(r#"aria-roledescription="chapter""#), "{}", h.attrs);

    for key in ["plain", "none", "hidden"] {
        let n = by_path(key);
        assert!(!n.attrs.contains("aria-roledescription"), "{key} must omit aria-roledescription: {}", n.attrs);
    }
}
