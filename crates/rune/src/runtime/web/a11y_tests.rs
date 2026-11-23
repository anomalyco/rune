use super::*;
use serde_json::json;

#[test]
fn mirrors_headings_inline_links_and_buttons_and_skips_idle_frames() {
    fn view(open: &(bool, usize), _: &mut Cx<(bool, usize)>) -> El {
        let mut body = crate::rich("Read spec for details.", vec![]).font(crate::tok::type_::BODY)
            .link(crate::Link::new(5..9, "https://example.com/spec"))
            .on_link(|s: &mut (bool, usize), _, _| s.1 += 1);
        body = body.selectable(true);
        let toggle = crate::row().key("fold").role("button").label(if open.0 { "Hide details" } else { "Show details" })
            .expanded(open.0)
            .on_press(|s: &mut (bool, usize)| s.0 = !s.0)
            .child(crate::text("Details"));
        crate::col().w_full().gap(8.0)
            .child(crate::text("Release notes").role("heading").level(1).font(crate::tok::type_::TITLE))
            .child(body)
            .child(toggle)
    }
    let mut rt = Runtime::new((false, 0usize), view, (480.0, 320.0), Env::default());
    rt.frame(0.0);
    let mut mirror = A11yMirror::default();
    let first = rt.web_a11y(&mut mirror).expect("initial tree").to_owned();
    assert!(rt.web_a11y(&mut mirror).is_none(), "unchanged frame emits nothing");

    let items = rt.web_a11y_items();
    assert_eq!(items.len(), 3);
    assert_eq!((items[0].kind, items[0].level, items[0].text.as_str()), ("heading", Some(1), "Release notes"));
    assert_eq!(items[1].kind, "text");
    assert_eq!(items[1].runs.len(), 3);
    assert_eq!((items[1].runs[1].text.as_str(), items[1].runs[1].href.as_deref()), ("spec", Some("https://example.com/spec")));
    assert_eq!((items[2].kind, items[2].text.as_str(), items[2].expanded), ("button", "Show details", Some(false)));
    assert!(items[1].font.is_some(), "text carries its font, so Find highlights land on the glyphs");
    assert!(!items[1].selectable, "outside a page the canvas selects text itself");

    let btn_path = items[2].path.clone();
    let link_path = items[1].path.clone();
    assert!(rt.web_a11y_action(&json!({"kind": "a11y_press", "path": btn_path})));
    rt.frame(0.0);
    assert!(rt.state.0);
    let updated = rt.web_a11y(&mut mirror).expect("button flip updates expanded state");
    assert_ne!(first, updated);
    assert!(updated.contains("Hide details"));

    assert!(rt.web_a11y_action(&json!({"kind": "a11y_link", "path": link_path, "link": 0})));
    rt.frame(0.0);
    assert_eq!(rt.state.1, 1);
}

fn page(_: &(), _: &mut Cx<()>) -> El {
    crate::col().w_full().role("main")
        .child(crate::col().key("intro").role("region").label("Intro").w_full().h(400.0)
            .child(crate::text("Rune").key("title").role("heading").level(1).font(crate::tok::type_::TITLE))
            .child(crate::text("A Rust UI framework.").key("lede").font(crate::tok::type_::BODY).selectable(true).padding(4.0)))
        .child(crate::col().key("links").role("navigation").w_full()
            .child(crate::row().key("gh").href("https://github.com/example/rune").child(crate::text("GitHub")))
            .child(crate::col().key("still").role("img").label("Liquid metal").w(200.0).h(100.0)))
}

#[test]
fn a_page_mirrors_landmarks_ids_links_and_images_in_document_coordinates() {
    let mut rt = Runtime::new((), page, (480.0, 300.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    rt.set_page_scroll(120.0);
    rt.frame(0.0);
    let items = rt.web_a11y_items();
    let kinds: Vec<_> = items.iter().map(|i| (i.kind, i.tag, i.parent)).collect();
    assert_eq!(kinds, [
        ("group", Some("main"), None),
        ("group", Some("section"), Some(0)),
        ("heading", None, Some(1)),
        ("text", None, Some(1)),
        ("group", Some("nav"), Some(0)),
        ("link", None, Some(4)),
        ("image", None, Some(4)),
    ]);
    assert_eq!((items[1].id.as_deref(), items[1].text.as_str()), (Some("intro"), "Intro"));
    assert_eq!(items[2].id.as_deref(), Some("title"), "a heading is an anchor");
    assert_eq!(items[3].id, None);
    assert_eq!((items[5].href.as_deref(), items[5].text.as_str()), (Some("https://github.com/example/rune"), "GitHub"));
    assert_eq!(items[6].text, "Liquid metal");
    assert_eq!(items[4].rect[1], 400.0, "rects are in the document, whatever the scroll");

    let font = items[3].font.as_ref().expect("text in a page carries its font");
    assert_eq!((font.family.as_str(), font.align), ("sans", "left"));
    assert!(items[3].selectable);
    assert!(items[2].selectable, "a page's plain text selects without asking");
    assert_eq!(items[3].rect[0], 4.0, "selectable text sits at its text box, inside the padding");
}

/// Plain text beside text inside things that take the pointer: a pressable card (holding a button,
/// so its text mirrors on its own), a draggable row, a tab, and an opted-out header.
fn controls(_: &(), _: &mut Cx<()>) -> El {
    let body = crate::tok::type_::BODY;
    crate::col().w_full().role("main")
        .child(crate::text("Plain words").key("plain").font(body))
        .child(crate::col().key("card").on_press(|_: &mut ()| {})
            .child(crate::text("Card words").key("card-words").font(body))
            .child(crate::text("Pinned words").key("pinned").font(body).selectable(true))
            .child(crate::row().key("go").on_press(|_: &mut ()| {}).child(crate::text("Go"))))
        .child(crate::row().key("drag").on_drag(|_: &mut (), _, _| {}).child(crate::text("Drag words").font(body)))
        .child(crate::row().key("tab").role("tab").child(crate::text("Tab words").font(body)))
        .child(crate::col().key("brand").unselectable()
            .child(crate::text("Brand words").font(body))
            .child(crate::text("Kept words").font(body).selectable(true)))
}

fn selectable_texts<S: 'static>(rt: &Runtime<S>) -> Vec<(String, bool)> {
    rt.web_a11y_items().into_iter().filter(|i| i.kind == "text").map(|i| (i.text, i.selectable)).collect()
}

#[test]
fn a_page_selects_its_text_but_not_text_inside_controls() {
    let mut rt = Runtime::new((), controls, (480.0, 400.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    let texts = selectable_texts(&rt);
    let want = |s: &str, on: bool| (s.to_owned(), on);
    assert_eq!(texts, [
        want("Plain words", true),
        want("Card words", false),
        want("Pinned words", true),
        want("Drag words", false),
        want("Tab words", false),
        want("Brand words", false),
        want("Kept words", true),
    ]);
    let items = rt.web_a11y_items();
    assert!(items.iter().any(|i| i.kind == "button" && i.text == "Go"), "the card's button still mirrors");
    let plain = items.iter().find(|i| i.text == "Plain words").expect("plain text mirrors");
    assert_eq!(plain.to_json()["e"], json!(true), "the bridge makes it selectable");
}

#[test]
fn outside_a_page_only_selectable_text_is_marked() {
    let mut rt = Runtime::new((), controls, (480.0, 400.0), Env::default());
    rt.frame(0.0);
    assert!(selectable_texts(&rt).iter().all(|(_, on)| !on), "a full-window app's canvas selects text itself");
}

#[test]
fn a_page_mirror_does_not_change_while_the_page_scrolls() {
    let mut rt = Runtime::new((), page, (480.0, 300.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    let mut mirror = A11yMirror::default();
    assert!(rt.web_a11y(&mut mirror).is_some());
    rt.set_page_scroll(200.0);
    rt.frame(0.0);
    assert!(rt.web_a11y(&mut mirror).is_none(), "scrolling the page leaves the mirror alone");
}

/// A page whose only change between frames is a colour, at a document offset that sits on a
/// rounding boundary (100.05 points rounds the other way either side of it).
fn tinted(tone: &u32, _: &mut Cx<u32>) -> El {
    crate::col().w_full().role("main")
        .child(crate::col().key("intro").role("region").label("Intro").w_full().h(100.05)
            .child(crate::text("Rune").key("title").role("heading").level(1).font(crate::tok::type_::TITLE)))
        .child(crate::text("A Rust UI framework.").key("lede").font(crate::tok::type_::BODY).selectable(true)
            .fill(crate::Color::WHITE.alpha(f32::from(u8::try_from(*tone % 200).expect("byte")) / 400.0)))
}

#[test]
fn fractional_scroll_offsets_never_rewrite_a_page_mirror() {
    let mut rt = Runtime::new(0u32, tinted, (480.0, 300.0), Env::new(true, 3.0));
    rt.page_mode();
    rt.frame(0.0);
    let mut mirror = A11yMirror::default();
    assert!(rt.web_a11y(&mut mirror).is_some());
    // At `@3x` a scroll offset is a third of a pixel, so the band's offset and the document's
    // position never add back exactly in `f32`. A commit that only redraws the band used to round
    // an item's position the other way and rewrite the whole page's mirror.
    for i in 1..20u32 {
        let y = f32::from(u16::try_from(i).expect("small")) * 137.3333;
        assert!(rt.set_page_scroll(y), "each offset is a new page position");
        rt.state = i;
        rt.frame(0.0);
        assert!(rt.web_a11y(&mut mirror).is_none(), "the mirror changed at {y}");
    }
}

/// A page with a wide code snippet in a horizontal viewport, as a site puts one.
fn strip(_: &(), _: &mut Cx<()>) -> El {
    crate::col().w_full().role("main")
        .child(crate::row().key("scroll").w(200.0).h(40.0).scroll_x()
            .child(crate::text("fn main() { println!(\"hello\"); }").key("code").w(600.0).h(40.0)
                .font(crate::tok::type_::BODY).selectable(true)))
}

#[test]
fn a_page_mirrors_a_horizontal_viewport_as_a_scroller_the_browser_scrolls() {
    let mut rt = Runtime::new((), strip, (400.0, 300.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    let items = rt.web_a11y_items();
    let scroll = items.iter().find(|i| i.kind == "scroll_x").expect("the strip mirrors as a scroller");
    assert_eq!((scroll.tag, scroll.rect[0], scroll.rect[2]), (Some("div"), 0.0, 200.0));
    let code = items.iter().find(|i| i.text.contains("println")).expect("the code still mirrors");
    let placed = code.rect[0];
    let mut mirror = A11yMirror::default();
    assert!(rt.web_a11y(&mut mirror).is_some());

    let path = scroll.path.clone();
    assert!(rt.set_page_scroll_x(&path, 120.0), "the browser scrolled the strip");
    rt.frame(0.0);
    assert!(rt.web_a11y(&mut mirror).is_none(), "the container's own scroll leaves the mirror alone");
    let after = rt.web_a11y_items();
    let code = after.iter().find(|i| i.text.contains("println")).expect("the code still mirrors");
    assert_eq!(code.rect[0], placed, "the text keeps its place inside the scroller");
    assert!(!rt.set_page_scroll_x(&path, 120.0), "the same offset is no change");
}

/// Code drawn one text per token, as an editor that animates tokens does, beside a gutter
/// number; then words a small gap apart, and a label and value set far apart.
fn code(_: &(), _: &mut Cx<()>) -> El {
    let mono = crate::tok::type_::BODY.mono();
    let line = |n: usize, toks: &[&str]| {
        let mut row = crate::row().key("toks");
        for (k, t) in toks.iter().enumerate() {
            row = row.child(crate::text(*t).key(format!("t{k}")).font(mono).w(t.chars().count() as f32 * 8.0).h(20.0));
        }
        crate::row().key(format!("ln{n}")).h(20.0)
            .child(crate::row().key("num").w(32.0).child(crate::text(n.to_string()).font(mono)))
            .child(row)
    };
    crate::col().w_full()
        .child(line(1, &["type", " ", "Port", " = ", "Int"]))
        .child(line(2, &["listen", "(", "port", ")"]))
        .child(crate::row().key("words").gap(4.0).child(crate::text("Hello")).child(crate::text("world")))
        .child(crate::row().key("pair").w(300.0).justify_between().child(crate::text("Name")).child(crate::text("Kit")))
}

#[test]
fn a_row_of_token_texts_mirrors_as_one_line() {
    let mut rt = Runtime::new((), code, (480.0, 300.0), Env::default());
    rt.frame(0.0);
    let items = rt.web_a11y_items();
    let texts: Vec<_> = items.iter().map(|i| (i.kind, i.text.as_str(), i.runs.len())).collect();
    assert_eq!(texts, [
        ("text", "1", 0),
        ("text", "type Port = Int", 0),
        ("text", "2", 0),
        ("text", "listen(port)", 0),
        ("text", "Hello world", 0),
        ("text", "Name", 0),
        ("text", "Kit", 0),
    ]);
    let line = &items[1];
    assert_eq!((line.rect[0], line.rect[2]), (32.0, 15.0 * 8.0), "the line's box spans its tokens");
}

#[test]
fn href_presses_open_absolute_urls() {    let mut rt = Runtime::new((), page, (480.0, 300.0), Env::default());
    rt.frame(0.0);
    let gh = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("gh")).unwrap().path.clone();
    assert_eq!(rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("gh")).unwrap().role, "link");
    assert!(rt.web_a11y_action(&json!({"kind": "a11y_press", "path": gh})));
    rt.frame(0.0);
    assert_eq!(rt.take_url_requests(), ["https://github.com/example/rune"]);
}

#[test]
fn mac_meta_copy_and_select_all_work_on_selectable_text() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::col().key("doc").selection_group(true)
            .child(crate::text("First paragraph").key("p1").font(crate::tok::type_::BODY).selectable(true))
            .child(crate::text("Second paragraph").key("p2").font(crate::tok::type_::BODY).selectable(true))
    }
    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::default());
    let board = crate::clipboard::memory();
    rt.use_clipboard(&board);
    rt.frame(0.0);
    rt.pointer_down(12.0, 8.0);
    rt.pointer_up(12.0, 8.0);
    rt.frame(0.0);
    // Simulate macOS browser Meta+A (meta=true, ctrl=false) then Meta+C.
    rt.web_key(&json!({"kind":"key", "key":"a", "code":"KeyA", "meta":true, "ctrl":false, "shift":false, "alt":false}));
    rt.frame(0.0);
    assert_eq!(rt.copy_text().as_deref(), Some("First paragraph\nSecond paragraph"));
    rt.web_key(&json!({"kind":"key", "key":"c", "code":"KeyC", "meta":true, "ctrl":false, "shift":false, "alt":false}));
    rt.frame(0.0);
    crate::clipboard::flush_memory(rt.core_mut());
    assert_eq!(board.text().as_deref().map(String::as_str), Some("First paragraph\nSecond paragraph"));
}

#[test]
fn exiting_children_do_not_swallow_or_misparent_live_siblings() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        let mut btn = crate::row().key("btn").role("button").label("Action").on_press(|_: &mut bool| {})
            .size(100.0, 24.0);
        if *show {
            btn = btn.child(crate::el().key("badge").size(10.0, 10.0).transition(crate::Transition::opacity()));
        }
        let mut box_a = crate::col().key("box-a").role("region").label("Box A").size(200.0, 40.0);
        if *show {
            box_a = box_a.child(crate::el().key("old").size(10.0, 10.0).transition(crate::Transition::opacity()));
        }
        crate::col().w_full().role("main")
            .child(btn)
            .child(crate::text("Live sibling after button").key("after-btn").font(crate::tok::type_::BODY))
            .child(box_a)
            .child(crate::col().key("box-b").role("region").label("Box B").size(200.0, 40.0)
                .child(crate::text("Box B text").font(crate::tok::type_::BODY)))
    }
    let mut rt = Runtime::new(true, view, (480.0, 300.0), Env::default());
    rt.frame(0.0);
    rt.update(|s, _| *s = false);
    rt.frame(1.0 / 60.0);
    assert!(rt.tree.nodes.iter().enumerate().any(|(i, _)| rt.core().subtree_exiting(i)), "exiting tail must be active");

    let items = rt.web_a11y_items();
    let after = items.iter().find(|it| it.text == "Live sibling after button")
        .expect("live sibling after button with exiting child must remain mirrored");
    assert_eq!(after.parent, Some(0), "sibling stays directly under main");
    let box_b = items.iter().find(|it| it.text == "Box B")
        .expect("live sibling region after exiting-child region must remain mirrored");
    assert_eq!(box_b.parent, Some(0), "Box B must remain a child of main, not mis-parented under Box A");
}

#[test]
fn rtl_horizontal_scroller_mirror_undoes_rtl_shift() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::col().w_full().child(
            crate::row().key("strip").dir(crate::Direction::Rtl).size(200.0, 60.0).scroll_x()
                .child(crate::row().w(600.0).h(60.0)
                    .child(crate::text("مرحبا").key("word").font(crate::tok::type_::BODY))),
        )
    }
    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    let x0 = rt.web_a11y_items().into_iter().find(|it| it.text == "مرحبا").unwrap().rect[0];
    rt.wheel_xy(100.0, 30.0, 80.0, 0.0);
    rt.frame(0.0);
    let strip = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("strip")).unwrap().id;
    assert!((rt.scroll.scrolls[&strip].x.offset - 80.0).abs() < 0.5, "RTL horizontal scroll moved offset");
    let x1 = rt.web_a11y_items().into_iter().find(|it| it.text == "مرحبا").unwrap().rect[0];
    assert!((x1 - x0).abs() < 0.2, "mirror rect must stay in scroller content coordinates across RTL scroll: {x0} -> {x1}");
}

#[test]
fn a11y_mirror_strips_unsafe_hrefs_and_sanitizes_named_fonts() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let bad_urls = [
            ("bad-js", "javascript:alert(1)"),
            ("bad-js-case", "JaVaScRiPt:alert(1)"),
            ("bad-js-ws", "  \tjavascript:alert(1)"),
            ("bad-js-ctrl", "\x01javascript:alert(1)"),
            ("bad-data", "data:text/html,<script>alert(1)</script>"),
            ("bad-data-b64", "DaTa:text/plain;base64,SGk="),
            ("bad-vbs", "VbScript:msgbox(1)"),
            ("bad-proto", "//evil.example/x"),
            ("bad-ctrl-rel", "\x00/docs"),
        ];
        let good_urls = [
            ("good-http", "http://example.com/a"),
            ("good-https", "  HTTPS://example.com/b  "),
            ("good-mail", "MAILTO:a@example.com"),
            ("good-abs", "/docs"),
            ("good-frag", "#install"),
            ("good-rel", "notes/42"),
        ];
        let mut root = crate::col().w_full().role("main");
        for (k, u) in bad_urls {
            root = root.child(crate::row().key(k).href(u).child(crate::text(k).font(crate::tok::type_::BODY)));
        }
        for (k, u) in good_urls {
            root = root.child(crate::row().key(k).href(u).child(crate::text(k).font(crate::tok::type_::BODY)));
        }
        let mut inline_bad = crate::rich("a b c d e f", vec![]).key("inline-bad").font(crate::tok::type_::BODY);
        inline_bad = inline_bad
            .link(crate::Link::new(0..1, "javascript:alert(1)"))
            .link(crate::Link::new(2..3, "data:text/html,hi"))
            .link(crate::Link::new(4..5, "VbScript:1"))
            .link(crate::Link::new(6..7, " \tJaVaScRiPt:1"))
            .link(crate::Link::new(8..9, "\x01javascript:1"))
            .link(crate::Link::new(10..11, "//evil.example/x"));
        let mut inline_good = crate::rich("one two three four five", vec![]).key("inline-good").font(crate::tok::type_::BODY);
        inline_good = inline_good
            .link(crate::Link::new(0..3, "http://example.com/1"))
            .link(crate::Link::new(4..7, "HTTPS://example.com/2"))
            .link(crate::Link::new(8..13, "mailto:a@example.com"))
            .link(crate::Link::new(14..18, "/guide"))
            .link(crate::Link::new(19..23, "#top"));
        let mut bad_font = crate::tok::type_::BODY;
        bad_font.family = crate::tok::Family::Named("Bad\";\\}\n</style><script>");
        let mut good_font = crate::tok::type_::BODY;
        good_font.family = crate::tok::Family::Named("Inter");
        root.child(inline_bad)
            .child(inline_good)
            .child(crate::text("bad font").key("bad-font").font(bad_font))
            .child(crate::text("good font").key("good-font").font(good_font))
    }

    let mut rt = Runtime::new((), view, (480.0, 600.0), Env::default());
    rt.frame(0.0);
    let items = rt.web_a11y_items();
    let json = rt.a11y_json();
    let lower = json.to_ascii_lowercase();
    for forbidden in ["javascript:", "data:", "vbscript:", "//evil.example"] {
        assert!(!lower.contains(forbidden), "a11y JSON leaked {forbidden}: {json}");
    }
    let links: Vec<(&str, &str)> = items.iter()
        .filter(|i| i.kind == "link")
        .map(|i| (i.text.as_str(), i.href.as_deref().unwrap_or("")))
        .collect();
    assert_eq!(links, [
        ("good-http", "http://example.com/a"),
        ("good-https", "https://example.com/b"),
        ("good-mail", "mailto:a@example.com"),
        ("good-abs", "/docs"),
        ("good-frag", "#install"),
        ("good-rel", "notes/42"),
    ]);
    assert!(!items.iter().any(|i| i.kind == "button"), "refused hrefs must not become buttons");
    assert!(items.iter().any(|i| i.kind == "text" && i.text == "bad-js"), "refused link label still mirrors as text");

    let ibad = items.iter().find(|i| i.path.ends_with("inline-bad")).expect("inline-bad");
    assert!(ibad.runs.is_empty(), "refused inline links collapse to plain text: {:?}", ibad.runs);
    assert_eq!(ibad.text, "a b c d e f");

    let igood = items.iter().find(|i| i.path.ends_with("inline-good")).expect("inline-good");
    let run_hrefs: Vec<Option<&str>> = igood.runs.iter().filter_map(|r| r.href.as_deref().map(Some)).collect();
    assert_eq!(run_hrefs, [
        Some("http://example.com/1"),
        Some("https://example.com/2"),
        Some("mailto:a@example.com"),
        Some("/guide"),
        Some("#top"),
    ]);

    let bf = items.iter().find(|i| i.text == "bad font").unwrap().font.as_ref().unwrap();
    assert!(!bf.family.chars().any(|c| c.is_control() || matches!(c, '"' | '\'' | '\\' | ';' | '{' | '}' | '<' | '>')),
        "named font must be sanitized: {:?}", bf.family);
    let gf = items.iter().find(|i| i.text == "good font").unwrap().font.as_ref().unwrap();
    assert_eq!(gf.family, "Inter");
}

#[test]
fn checked_state_maps_to_role_appropriate_aria_attributes() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        let on = *on;
        crate::col().w_full().gap(4.0)
            .child(crate::row().key("wrap").role("button").label("Wrap").checked(on)
                .focusable(true).on_press(|s: &mut bool| *s = !*s).size(80.0, 24.0))
            .child(crate::row().key("mixed-btn").role("button").label("Bold").checked_mixed()
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
            .child(crate::row().key("plain-btn").role("button").label("Run")
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
            .child(crate::row().key("check").role("checkbox").label("All").checked_mixed()
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
            .child(crate::row().key("switch").role("switch").label("Lights").checked(on)
                .focusable(true).on_press(|s: &mut bool| *s = !*s).size(80.0, 24.0))
            .child(crate::row().key("radio-checked").role("radio").label("Fast").checked(true)
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
            .child(crate::row().key("radio-sel").role("radio").label("Slow").focus_selected(true)
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
            .child(crate::row().key("menu-check").role("menuitemcheckbox").label("Minimap").checked(true)
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
            .child(crate::row().key("menu-radio").role("menuitemradio").label("Dark").checked(false)
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
            .child(crate::row().key("menu-plain").role("menuitem").label("Wrap item").checked(true)
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
            .child(crate::row().key("tab-checked").role("tab").label("Files").checked(true)
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
            .child(crate::row().key("tab-sel").role("tab").label("Search").focus_selected(true)
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
            .child(crate::row().key("opt-checked").role("option").label("Item A").checked(true)
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
            .child(crate::row().key("opt-off").role("option").label("Item B").checked(false)
                .focusable(true).on_press(|_: &mut bool| {}).size(80.0, 24.0))
    }

    let mut rt = Runtime::new(false, view, (480.0, 600.0), Env::default());
    rt.frame(0.0);
    let mut mirror = A11yMirror::default();
    let parse_items = |json: &str| -> Vec<serde_json::Value> { serde_json::from_str(json).unwrap() };
    let find = |items: &[serde_json::Value], label: &str| -> serde_json::Value {
        items.iter().find(|v| v["t"] == label).unwrap_or_else(|| panic!("missing {label}: {items:?}")).clone()
    };

    let items = parse_items(rt.web_a11y(&mut mirror).expect("initial mirror"));
    let wrap = find(&items, "Wrap");
    assert_eq!(wrap.get("o"), None);
    assert_eq!(wrap.get("pr"), Some(&json!("false")));
    assert_eq!(wrap.get("c"), None);
    assert_eq!(wrap.get("se"), None);

    let mixed_btn = find(&items, "Bold");
    assert_eq!(mixed_btn.get("pr"), Some(&json!("mixed")));
    assert_eq!(mixed_btn.get("c"), None);

    let plain_btn = find(&items, "Run");
    assert_eq!(plain_btn.get("pr"), None);
    assert_eq!(plain_btn.get("c"), None);
    assert_eq!(plain_btn.get("se"), None);

    for (label, role, val) in [
        ("All", "checkbox", "mixed"),
        ("Lights", "switch", "false"),
        ("Fast", "radio", "true"),
        ("Slow", "radio", "true"),
        ("Minimap", "menuitemcheckbox", "true"),
        ("Dark", "menuitemradio", "false"),
        ("Wrap item", "menuitem", "true"),
    ] {
        let it = find(&items, label);
        assert_eq!(it.get("o"), Some(&json!(role)), "{label} role");
        assert_eq!(it.get("c"), Some(&json!(val)), "{label} checked");
        assert_eq!(it.get("pr"), None, "{label} must not emit pressed");
    }

    for (label, role, val) in [
        ("Files", "tab", true),
        ("Search", "tab", true),
        ("Item A", "option", true),
        ("Item B", "option", false),
    ] {
        let it = find(&items, label);
        assert_eq!(it.get("o"), Some(&json!(role)), "{label} role");
        assert_eq!(it.get("se"), Some(&json!(val)), "{label} selected");
        assert_eq!(it.get("c"), None, "{label} must not emit checked");
        assert_eq!(it.get("pr"), None, "{label} must not emit pressed");
    }

    // Activating the toggle button flips both aria-pressed on the button and aria-checked on the switch.
    let wrap_path = wrap["p"].as_str().unwrap().to_owned();
    assert!(rt.web_a11y_action(&json!({"kind": "a11y_press", "path": wrap_path})));
    rt.frame(0.0);
    let after = parse_items(rt.web_a11y(&mut mirror).expect("toggle updates mirror"));
    assert_eq!(find(&after, "Wrap").get("pr"), Some(&json!("true")));
    assert_eq!(find(&after, "Wrap").get("c"), None);
    assert_eq!(find(&after, "Lights").get("c"), Some(&json!("true")));

    // The focus proxy uses the same role-specific attributes.
    let focus_json = |rt: &mut Runtime<bool>, mirror: &mut A11yMirror, key: &str| -> serde_json::Value {
        let key = key.to_owned();
        rt.update(move |_, tx| tx.focus(key, true));
        rt.frame(0.0);
        serde_json::from_str(rt.core().web_a11y_focus(mirror).expect("focus proxy update")).unwrap()
    };
    let f_wrap = focus_json(&mut rt, &mut mirror, "wrap");
    assert_eq!((f_wrap.get("o"), f_wrap.get("pr"), f_wrap.get("c")), (Some(&json!("button")), Some(&json!("true")), None));
    let f_mixed = focus_json(&mut rt, &mut mirror, "mixed-btn");
    assert_eq!((f_mixed.get("o"), f_mixed.get("pr"), f_mixed.get("c")), (Some(&json!("button")), Some(&json!("mixed")), None));
    let f_check = focus_json(&mut rt, &mut mirror, "check");
    assert_eq!((f_check.get("o"), f_check.get("c"), f_check.get("pr")), (Some(&json!("checkbox")), Some(&json!("mixed")), None));
    let f_tab = focus_json(&mut rt, &mut mirror, "tab-checked");
    assert_eq!((f_tab.get("o"), f_tab.get("s"), f_tab.get("c"), f_tab.get("pr")), (Some(&json!("tab")), Some(&json!(true)), None, None));
    let f_opt = focus_json(&mut rt, &mut mirror, "opt-checked");
    assert_eq!((f_opt.get("o"), f_opt.get("s"), f_opt.get("c"), f_opt.get("pr")), (Some(&json!("option")), Some(&json!(true)), None, None));
}

#[test]
fn tablist_and_closable_tabs_mirror_with_sibling_close_buttons() {
    #[derive(Default)]
    struct State {
        active: usize,
        closed: usize,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let tab_a = crate::row().key("t-a").role("tab").label("Alpha")
            .focus_selected(s.active == 0).position_in_set(1, 2).size(120.0, 28.0)
            .on_press(|s: &mut State| s.active = 0)
            .child(crate::text("Alpha").font(crate::tok::type_::BODY))
            .child(crate::row().key("close-a").role("button").label("Close Alpha").size(20.0, 20.0)
                .on_press(|s: &mut State| s.closed += 1));
        let tab_b = crate::row().key("t-b").role("tab")
            .focus_selected(s.active == 1).position_in_set(2, 2).size(120.0, 28.0)
            .on_press(|s: &mut State| s.active = 1)
            .child(crate::text("Beta").font(crate::tok::type_::BODY))
            .child(crate::row().key("close-b").role("button").label("Close Beta").size(20.0, 20.0)
                .on_press(|s: &mut State| s.closed += 10));
        crate::row().key("tabs").role("tablist").label("Tabs").focus_group().size(300.0, 28.0)
            .child(tab_a)
            .child(tab_b)
    }
    let mut rt = Runtime::new(State::default(), view, (400.0, 100.0), Env::default());
    rt.frame(0.0);

    let items = rt.web_a11y_items();
    let summary: Vec<_> = items.iter().map(|i| (i.kind, i.tag, i.role, i.text.as_str(), i.selected, i.set, i.parent)).collect();
    assert_eq!(summary, [
        ("group", Some("div"), Some("tablist"), "Tabs", None, None, None),
        ("button", None, Some("tab"), "Alpha", Some(true), Some((1, 2)), Some(0)),
        ("button", None, None, "Close Alpha", None, None, Some(0)),
        ("button", None, Some("tab"), "Beta", Some(false), Some((2, 2)), Some(0)),
        ("button", None, None, "Close Beta", None, None, Some(0)),
    ]);

    let tab_b_path = items[3].path.clone();
    let close_a_path = items[2].path.clone();
    assert!(rt.web_a11y_action(&json!({"kind": "a11y_press", "path": tab_b_path})));
    rt.frame(0.0);
    assert_eq!(rt.state.active, 1);
    let updated = rt.web_a11y_items();
    assert_eq!((updated[1].selected, updated[3].selected), (Some(false), Some(true)));

    assert!(rt.web_a11y_action(&json!({"kind": "a11y_press", "path": close_a_path})));
    rt.frame(0.0);
    assert_eq!(rt.state.closed, 1);
}

#[test]
fn web_a11y_mirror_omits_secure_fields_labels_and_containers() {
    struct S {
        pw: crate::TextEdit,
        revealed: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        crate::col().w_full().role("main")
            .child(crate::text_input(&s.pw, crate::tok::type_::BODY).key("pw").single_line().label("Password").w(200.0)
                .on_edit(|s: &mut S| &mut s.pw))
            .child(crate::text("mirror-label-secret").key("lbl").secure().revealed(s.revealed))
            .child(crate::col().key("box").secure().child(crate::text("mirror-boxed-secret")))
            .child(crate::row().key("btn").role("button").on_press(|_: &mut S| {})
                .child(crate::text("Reveal"))
                .child(crate::col().secure().child(crate::text("mirror-btn-secret"))))
    }
    let mut pw = crate::TextEdit::new("hunter2secret").secure();
    pw.focused = true;
    let mut rt = Runtime::new(S { pw, revealed: false }, view, (400.0, 300.0), Env::default());
    let mut mirror = A11yMirror::default();
    for revealed in [false, true] {
        rt.update(|s, _| {
            s.revealed = revealed;
            s.pw.set_revealed(revealed);
        });
        rt.frame(0.0);
        let a11y_json = rt.a11y_json();
        let focus_json = rt.core().web_a11y_focus(&mut mirror).map(str::to_owned).unwrap_or_default();
        for out in [&a11y_json, &focus_json] {
            assert!(!out.contains('•'), "web mirror leaked mask: {out}");
            for secret in ["hunter2secret", "mirror-label-secret", "mirror-boxed-secret", "mirror-btn-secret"] {
                assert!(!out.contains(secret), "web mirror leaked {secret}: {out}");
            }
        }
        let items = rt.web_a11y_items();
        let btn = items.iter().find(|it| it.kind == "button").expect("button present");
        assert_eq!(btn.text, "Reveal", "button label must omit secure descendant text");
    }
}
