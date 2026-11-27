use super::*;
use serde_json::json;

#[test]
fn inline_relative_and_explicit_href_links_emit_real_hrefs_while_opaque_ids_stay_action_only() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let body = crate::rich("Go Chat Query Anchor Rel Up Named Action Bad", vec![])
            .key("links")
            .font(crate::tok::type_::BODY)
            .link(crate::Link::new(3..7, "/chat/123"))
            .link(crate::Link::new(8..13, "?tab=2"))
            .link(crate::Link::new(14..20, "#install"))
            .link(crate::Link::new(21..24, "./local"))
            .link(crate::Link::new(25..27, "../parent"))
            .link(crate::Link::new(28..33, "doc-42").href("notes/42"))
            .link(crate::Link::new(34..40, "mention-alice"))
            .link(crate::Link::new(41..44, "javascript:alert(1)"))
            .on_link(|_: &mut (), _, _| {});
        crate::col().w_full().child(body)
    }

    let mut rt = Runtime::new((), view, (480.0, 200.0), Env::default());
    rt.frame(0.0);
    let items = rt.web_a11y_items();
    let item = items.iter().find(|i| i.path.ends_with("links")).expect("links item");
    let link_runs: Vec<(&str, Option<&str>, Option<usize>)> = item
        .runs
        .iter()
        .filter(|r| !r.is_plain())
        .map(|r| (r.text.as_str(), r.href.as_deref(), r.link))
        .collect();
    assert_eq!(
        link_runs,
        [
            ("Chat", Some("/chat/123"), Some(0)),
            ("Query", Some("?tab=2"), Some(1)),
            ("Anchor", Some("#install"), Some(2)),
            ("Rel", Some("./local"), Some(3)),
            ("Up", Some("../parent"), Some(4)),
            ("Named", Some("notes/42"), Some(5)),
            ("Action", None, Some(6)),
        ]
    );

    // JSON omits "h" for opaque action links while preserving "p" and "i", and unsafe schemes stay plain text.
    let json_items: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    let runs = json_items[0]["u"].as_array().expect("runs array");
    let action = runs.iter().find(|r| r["t"] == "Action").expect("opaque link run");
    assert_eq!(action.get("h"), None, "opaque link id must not emit a relative href");
    assert_eq!(action["i"], 6);
    assert!(action["p"].as_str().is_some());
    assert!(!rt.a11y_json().contains("javascript:"));
    assert!(runs.iter().any(|r| r["t"].as_str().is_some_and(|t| t.contains("Bad") && r.get("i").is_none())));
}

#[test]
fn single_opaque_inline_link_is_not_collapsed_to_plain_text() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let solo = crate::rich("Retry", vec![])
            .key("solo")
            .font(crate::tok::type_::BODY)
            .link(crate::Link::new(0..5, "retry-action"))
            .on_link(|_: &mut (), _, _| {});
        crate::col().w_full().child(solo)
    }

    let mut rt = Runtime::new((), view, (320.0, 120.0), Env::default());
    rt.frame(0.0);
    let items = rt.web_a11y_items();
    let solo = items.iter().find(|i| i.path.ends_with("solo")).expect("solo item");
    assert!(solo.text.is_empty(), "single opaque link must stay in runs, not plain text");
    assert_eq!(solo.runs.len(), 1);
    assert_eq!(solo.runs[0].text, "Retry");
    assert_eq!(solo.runs[0].href, None);
    assert_eq!(solo.runs[0].link, Some(0));
}

#[test]
fn relative_el_href_routes_custom_on_press_and_has_scheme_distinguishes_colons_in_paths() {
    assert!(crate::open_url::has_scheme("https://example.com/a"));
    assert!(crate::open_url::has_scheme("mailto:a@example.com"));
    assert!(!crate::open_url::has_scheme("/chat/123"));
    assert!(!crate::open_url::has_scheme("/wiki/Category:Rust"));
    assert!(!crate::open_url::has_scheme("?time=12:30"));
    assert!(!crate::open_url::has_scheme("#section:1"));
    assert!(!crate::open_url::has_scheme("//evil.example/x"));

    fn view(routed: &usize, _: &mut Cx<usize>) -> El {
        let _ = routed;
        crate::col()
            .w_full()
            .child(
                crate::row()
                    .key("chat")
                    .href("/chat/123")
                    .on_press(|s: &mut usize| *s += 1)
                    .child(crate::text("Chat 123").font(crate::tok::type_::BODY)),
            )
            .child(
                crate::row()
                    .key("rel-default")
                    .href("/wiki/Category:Rust")
                    .child(crate::text("Wiki").font(crate::tok::type_::BODY)),
            )
    }

    let mut rt = Runtime::new(0usize, view, (320.0, 160.0), Env::default());
    rt.frame(0.0);
    let items = rt.web_a11y_items();
    let chat = items.iter().find(|i| i.path.ends_with("chat")).expect("chat link");
    assert_eq!(chat.kind, "link");
    assert_eq!(chat.href.as_deref(), Some("/chat/123"));
    let wiki = items.iter().find(|i| i.path.ends_with("rel-default")).expect("wiki link");
    assert_eq!(wiki.kind, "link");
    assert_eq!(wiki.href.as_deref(), Some("/wiki/Category:Rust"));

    assert!(rt.web_a11y_action(&json!({"kind": "a11y_press", "path": chat.path})));
    rt.frame(0.0);
    assert_eq!(rt.state, 1);

    // Default relative El::href press does not open an external URL natively.
    assert!(rt.web_a11y_action(&json!({"kind": "a11y_press", "path": wiki.path})));
    rt.frame(0.0);
    assert!(rt.take_url_requests().is_empty());
}

#[test]
fn anchor_id_accepts_html5_numeric_leading_keys_in_live_mirror_and_aria_relations() {
    for good in ["3d-device-view", "0", "404", "1_2-3", "--html", "-flag", "_intro", "views"] {
        assert_eq!(anchor_id(Some(good)).as_deref(), Some(good), "expected valid anchor_id for {good:?}");
    }
    for bad in ["", "3d view", "3d#dup1", "a\"b", "a<b", "a/b", "a.b", "a:b"] {
        assert_eq!(anchor_id(Some(bad)), None, "expected None for invalid key {bad:?}");
    }
    assert_eq!(anchor_id(None), None);

    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::col().w_full().role("main")
            .child(
                crate::col().key("100-section").role("region").label("3D Section").w_full()
                    .child(
                        crate::text("3D Device View")
                            .key("3d-device-view")
                            .role("heading")
                            .level(2)
                            .font(crate::tok::type_::TITLE),
                    )
                    .child(
                        crate::col()
                            .key("2d-label-heading")
                            .role("heading")
                            .level(3)
                            .label("2D Heading")
                            .size(120.0, 24.0),
                    )
                    .child(
                        crate::row()
                            .key("42-link")
                            .href("#3d-device-view")
                            .child(crate::text("Jump").font(crate::tok::type_::BODY)),
                    )
                    .child(
                        crate::row()
                            .key("toggle")
                            .role("button")
                            .label("Toggle")
                            .controls("0-panel")
                            .owns("0-panel")
                            .size(80.0, 24.0)
                            .on_press(|_: &mut ()| {}),
                    )
                    .child(
                        crate::col()
                            .key("0-panel")
                            .role("region")
                            .label("Zero Panel")
                            .size(200.0, 40.0),
                    ),
            )
    }

    let mut rt = Runtime::new((), view, (480.0, 400.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    let items = rt.web_a11y_items();
    let by_key = |k: &str| {
        let suffix = format!("/{k}");
        items
            .iter()
            .find(|i| i.path.ends_with(&suffix))
            .unwrap_or_else(|| panic!("missing mirrored item ending with {suffix}: {items:?}"))
    };
    assert_eq!(by_key("100-section").id.as_deref(), Some("100-section"));
    assert_eq!(by_key("3d-device-view").id.as_deref(), Some("3d-device-view"));
    assert_eq!(by_key("2d-label-heading").id.as_deref(), Some("2d-label-heading"));
    assert_eq!(by_key("42-link").id.as_deref(), Some("42-link"));
    assert_eq!(by_key("0-panel").id.as_deref(), Some("0-panel"));
    let toggle = by_key("toggle");
    assert_eq!(toggle.aria.controls.as_deref(), Some("0-panel"));
    assert_eq!(toggle.aria.owns.as_deref(), Some("0-panel"));
}
