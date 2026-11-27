use super::*;

fn site(_: &(), cx: &mut Cx<()>) -> El {
    let wide = cx.layout_size("root").is_some_and(|s| s[0] >= 700.0);
    let cards = if wide { crate::row() } else { crate::col() };
    crate::col().key("root").w_full().role("main").fill(crate::color::hex("#0b0d12").unwrap()).padding(24.0).gap(16.0)
        .child(crate::text("Rune").key("title").role("heading").level(1).font(crate::tok::type_::TITLE).color(crate::Color::WHITE))
        .child(crate::rich("Read the <docs> & more.", vec![]).font(crate::tok::type_::BODY)
            .link(crate::Link::new(9..15, "https://example.com/docs?a=1&b=2")))
        .child(cards.key("views").role("region").label("Views").gap(12.0)
            .child(crate::col().key("a").grow(1.0).radius(12.0).border(1.0, crate::color::hex("#2a2f3a").unwrap()).child(crate::text("Plain Rust")))
            .child(crate::col().key("b").grow(1.0).child(crate::text("Tested")))
            .children(wide.then(|| crate::col().key("c").grow(1.0).child(crate::text("Wide only")))))
        .child(crate::row().key("gh").href("https://github.com/example/rune").child(crate::text("GitHub")))
        .child(crate::col().key("idle").opacity(0.0).child(crate::text("Hidden")))
}

fn taken(rt: &mut Runtime<()>, w: f32) -> Vec<ExportNode> {
    rt.set_viewport((w, 600.0), 2.0);
    rt.frame(0.0);
    rt.frame(0.0);
    rt.export_nodes()
}

#[test]
fn exports_landmarks_headings_links_and_text_with_their_css() {
    let mut rt = Runtime::new((), site, (400.0, 600.0), Env::default());
    rt.page_mode();
    let nodes = taken(&mut rt, 400.0);
    let tags: Vec<&str> = nodes.iter().map(|n| n.tag).collect();
    assert_eq!(tags[..2], ["main", "h1"]);
    assert!(nodes.iter().any(|n| n.tag == "section" && n.attrs.contains("id=\"views\"")), "{tags:?}");
    let link = nodes.iter().find(|n| n.tag == "a").expect("href element");
    assert!(link.attrs.contains("href=\"https://github.com/example/rune\"") && link.attrs.contains("id=\"gh\""));
    let lede = nodes.iter().find(|n| n.inner.contains("docs")).expect("rich text");
    assert_eq!(lede.inner, "Read the <a href=\"https://example.com/docs?a=1&amp;b=2\">&lt;docs&gt;</a> &amp; more.");
    assert!(!nodes.iter().any(|n| n.inner.contains("Hidden")), "an invisible subtree is left out");
    let root = &nodes[0].decls;
    assert!(root.contains(&("flex-direction", "column".into())) && root.contains(&("width", "100%".into())));
    assert!(root.contains(&("padding", "24px".into())) && root.contains(&("row-gap", "16px".into())));
    let card = nodes.iter().find(|n| n.path.ends_with("a") && n.decls.iter().any(|d| d.0 == "border-radius")).expect("card");
    assert!(card.decls.contains(&("border-radius", "12px".into())));
    assert!(card.decls.iter().any(|d| d.0 == "box-shadow" && d.1.starts_with("inset 0 0 0 1px")), "a paint-only border is an inset shadow");
    let title = &nodes[1];
    assert!(title.decls.contains(&("color", "#ffffff".into())) && title.decls.iter().any(|d| d.0 == "font-size"));
    assert!(!nodes.iter().any(|n| n.tag != "a" && n.decls.iter().any(|d| d.0 == "user-select")), "page text selects");
    assert!(link.decls.contains(&("user-select", "none".into())), "as the live mirror's, a link's text doesn't");
}

#[test]
fn folds_widths_into_media_rules_and_hides_what_a_width_lacks() {
    let mut rt = Runtime::new((), site, (400.0, 600.0), Env::default());
    rt.page_mode();
    let narrow = taken(&mut rt, 400.0);
    let wide = taken(&mut rt, 900.0);
    let (body, css) = page(&[(400.0, narrow), (900.0, wide.clone())]);
    assert!(body.starts_with("<main data-rune-static"));
    assert!(css.starts_with(page::BASE_CSS));
    let media = css.split_once("@media (min-width:900px){").expect("a wide block").1;
    assert!(media.contains("flex-direction:row"), "{media}");
    assert!(!body.contains("Wide only"), "the body is the narrowest tree: {body}");
    assert!(wide.iter().any(|n| n.inner == "Wide only"));
    assert!(css.contains(".t>*{display:inline"), "a text element's runs stay inline: {css}");
    assert!(body.contains("<p class=\"t"), "{body}");
    let opened = body.matches('<').count() - body.matches("</").count();
    assert_eq!(opened, body.matches("</").count(), "every element closes: {body}");
}

#[test]
fn a_node_missing_at_a_wider_width_is_hidden_there() {
    let narrow = vec![ExportNode { path: "0".into(), depth: 0, tag: "div", attrs: String::new(), inner: String::new(), decls: vec![], width: 100.0, text: false },
        ExportNode { path: "0.1".into(), depth: 1, tag: "p", attrs: String::new(), inner: "x".into(), decls: vec![("color", "#fff".into())], width: 100.0, text: true }];
    let wide = vec![narrow[0].clone()];
    let (body, css) = page(&[(360.0, narrow), (800.0, wide)]);
    assert_eq!(body, "<div data-rune-static><p class=\"t a0 b0\">x</p></div>");
    assert!(css.ends_with(".a0{color:#fff;display:flex}@media (min-width:800px){.b0{display:none}}"), "{css}");
}

#[test]
fn radial_gradients_scale_with_the_box_and_soft_clips_become_masks() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let black = crate::Color::BLACK;
        crate::col().key("root").w_full().role("main")
            .child(crate::col().key("vignette").w(200.0).h(100.0).fill(black)
                .radial_gradient([0.5, 0.5], [100.0, 50.0], [(0.58, crate::Color::TRANSPARENT), (1.0, black)]))
            .child(crate::row().key("trace").w(200.0).h(20.0).clip_fade_linear(40.0, 0.0, 0.0, 0.0)
                .child(crate::text("x")))
    }
    let mut rt = Runtime::new((), view, (400.0, 600.0), Env::default());
    rt.page_mode();
    let nodes = taken(&mut rt, 400.0);
    let decl = |prop: &str| nodes.iter().flat_map(|n| &n.decls).find(|d| d.0 == prop).map(|d| d.1.clone()).unwrap_or_default();
    assert_eq!(decl("background"), "radial-gradient(50% 50% at 50% 50%,#00000000 58%,#000000 100%),#000000");
    assert_eq!(decl("mask-image"), "linear-gradient(to right,transparent 0,#000 40px,#000 100%)");
}

#[test]
fn text_that_wraps_anywhere_breaks_inside_a_long_word() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::row().key("root").w_full().role("main")
            .child(crate::text("crates/rune-motion/tests/spring.rs:124").key("path").wrap_anywhere().shrink())
            .child(crate::text("words that wrap at spaces").key("prose").wrap())
    }
    let mut rt = Runtime::new((), view, (400.0, 600.0), Env::default());
    rt.page_mode();
    let nodes = taken(&mut rt, 120.0);
    let decls = |text: &str| nodes.iter().find(|n| n.inner.starts_with(text)).map(|n| n.decls.clone()).unwrap_or_default();
    assert!(decls("crates/").contains(&("overflow-wrap", "anywhere".into())), "{:?}", decls("crates/"));
    assert!(!decls("words").iter().any(|d| d.0 == "overflow-wrap"));
}

#[test]
fn an_image_source_set_gets_sizes_from_every_width() {
    let img = |w: f32| ExportNode { path: "0.0".into(), depth: 1, tag: "img", attrs: " src=\"a-640.avif\" srcset=\"a-640.avif 640w, a-1280.avif 1280w\"".into(), inner: String::new(), decls: vec![], width: w, text: false };
    let root = ExportNode { path: "0".into(), depth: 0, tag: "div", attrs: String::new(), inner: String::new(), decls: vec![], width: 0.0, text: false };
    let (body, css) = page(&[(360.0, vec![root.clone(), img(328.0)]), (1000.0, vec![root, img(500.0)])]);
    assert!(body.contains(" sizes=\"(min-width:1000px) 50vw,92vw\">"), "{body}");
    // The `height` attribute is the size it was laid out at; at any other width its class's
    // `aspect-ratio` or height decides, so the reset sits under every class (`:where`).
    assert!(css.contains(":where([data-rune-static]) img{height:auto}"), "{css}");
}

#[test]
fn html_export_blocks_unsafe_hrefs_and_sanitizes_named_fonts() {
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
        let mut root = crate::col().key("root").w_full().role("main");
        for (k, u) in bad_urls {
            root = root.child(crate::row().key(k).href(u).child(crate::text(k).font(crate::tok::type_::BODY)));
        }
        for (k, u) in good_urls {
            root = root.child(crate::row().key(k).href(u).child(crate::text(k).font(crate::tok::type_::BODY)));
        }
        let inline_bad = crate::rich("a b c d e f", vec![]).key("inline-bad").font(crate::tok::type_::BODY)
            .link(crate::Link::new(0..1, "javascript:alert(1)"))
            .link(crate::Link::new(2..3, "data:text/html,hi"))
            .link(crate::Link::new(4..5, "VbScript:1"))
            .link(crate::Link::new(6..7, " \tJaVaScRiPt:1"))
            .link(crate::Link::new(8..9, "\x01javascript:1"))
            .link(crate::Link::new(10..11, "//evil.example/x"));
        let inline_good = crate::rich("one two three four five", vec![]).key("inline-good").font(crate::tok::type_::BODY)
            .link(crate::Link::new(0..3, "http://example.com/1"))
            .link(crate::Link::new(4..7, "HTTPS://example.com/2"))
            .link(crate::Link::new(8..13, "mailto:a@example.com"))
            .link(crate::Link::new(14..18, "/guide"))
            .link(crate::Link::new(19..23, "#top"));
        let mut bad_font = crate::tok::type_::BODY;
        bad_font.family = crate::tok::Family::Named("Bad\";\\}\nbody{color:red}</style>");
        let mut good_font = crate::tok::type_::BODY;
        good_font.family = crate::tok::Family::Named("Inter");
        root.child(inline_bad)
            .child(inline_good)
            .child(crate::text("bad font").key("bad-font").font(bad_font))
            .child(crate::text("good font").key("good-font").font(good_font))
    }

    let mut rt = Runtime::new((), view, (400.0, 600.0), Env::default());
    rt.page_mode();
    let nodes = taken(&mut rt, 400.0);
    let (body, css) = page(&[(400.0, nodes.clone())]);

    let lower_body = body.to_ascii_lowercase();
    for forbidden in ["javascript:", "data:", "vbscript:", "//evil.example"] {
        assert!(!lower_body.contains(forbidden), "exported HTML leaked {forbidden}: {body}");
    }
    for bad in ["bad-js", "bad-data", "bad-vbs", "bad-proto"] {
        let node = nodes.iter().find(|n| n.path.ends_with(bad)).expect(bad);
        assert_ne!(node.tag, "a", "refused href must not export as <a>: {node:?}");
        assert!(!node.attrs.contains("href="), "refused href must not write href attr: {node:?}");
    }
    let ibad = nodes.iter().find(|n| n.path.ends_with("inline-bad")).expect("inline-bad");
    assert_eq!(ibad.inner, "a b c d e f", "refused inline links export as plain text");

    for expected in [
        "href=\"http://example.com/a\"",
        "href=\"https://example.com/b\"",
        "href=\"mailto:a@example.com\"",
        "href=\"/docs\"",
        "href=\"#install\"",
        "href=\"notes/42\"",
        "<a href=\"http://example.com/1\">one</a>",
        "<a href=\"https://example.com/2\">two</a>",
        "<a href=\"mailto:a@example.com\">three</a>",
        "<a href=\"/guide\">four</a>",
        "<a href=\"#top\">five</a>",
    ] {
        assert!(body.contains(expected), "missing safe link {expected} in {body}");
    }

    assert!(css.contains("font-family:\"Inter\""), "normal named font preserved: {css}");
    assert!(!css.contains("</style>"), "CSS must not contain closing style tag: {css}");
    assert!(!css.contains('\n'), "CSS must not contain raw newlines: {css}");
    assert!(!css.contains("body{color:red}"), "CSS must not allow rule breakout: {css}");
}

#[test]
fn html_export_omits_secure_fields_labels_and_containers() {
    struct S {
        pw: crate::TextEdit,
        revealed: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        crate::col().key("root").w_full().role("main")
            .child(crate::text("Visible heading").key("hdr"))
            .child(crate::text_input(&s.pw, crate::tok::type_::BODY).key("pw").single_line().label("Password").w(200.0))
            .child(crate::text("export-label-secret").key("lbl").secure().revealed(s.revealed))
            .child(crate::col().key("box").secure().child(crate::text("export-boxed-secret")))
    }
    let pw = crate::TextEdit::new("export-field-secret").secure();
    let mut rt = Runtime::new(S { pw, revealed: false }, view, (400.0, 300.0), Env::default());
    rt.page_mode();
    for revealed in [false, true] {
        rt.update(|s, _| {
            s.revealed = revealed;
            s.pw.set_revealed(revealed);
        });
        rt.frame(0.0);
        let nodes = rt.export_nodes();
        let (body, css) = page(&[(400.0, nodes)]);
        assert!(body.contains("Visible heading"), "{body}");
        assert!(!body.contains('•'), "html export leaked mask: {body}");
        for secret in ["export-field-secret", "export-label-secret", "export-boxed-secret"] {
            assert!(!body.contains(secret) && !css.contains(secret), "html export leaked {secret}: {body}");
        }
    }
}

#[test]
fn html_export_emits_fit_content_height_fraction_and_self_auto() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::col().key("root").w_full().h(200.0).role("main")
            .child(crate::row().key("pill").w_fit().child(crate::text("Badge")))
            .child(crate::col().key("chip").h_fit().child(crate::text("Chip")))
            .child(crate::el().key("frac").h_frac(0.4))
            .child(crate::el().key("fixed").size(20.0, 20.0).self_start())
            .child(crate::el().key("reset").size(20.0, 20.0).self_start().self_auto())
    }
    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::default());
    rt.page_mode();
    let nodes = taken(&mut rt, 400.0);
    let decls = |key: &str| nodes.iter().find(|n| n.path.ends_with(key)).expect(key).decls.clone();
    assert!(decls("pill").contains(&("width", "fit-content".into())), "{:?}", decls("pill"));
    assert!(!decls("pill").iter().any(|d| d.0 == "align-self"), "{:?}", decls("pill"));
    assert!(decls("chip").contains(&("height", "fit-content".into())), "{:?}", decls("chip"));
    assert!(decls("frac").contains(&("height", "40%".into())), "{:?}", decls("frac"));
    assert!(decls("fixed").contains(&("align-self", "flex-start".into())), "{:?}", decls("fixed"));
    assert!(!decls("reset").iter().any(|d| d.0 == "align-self"), "{:?}", decls("reset"));
}

#[test]
fn exports_compound_shape_fill_rules_to_svg() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let ring = crate::Path::new()
            .move_to([0.0, 0.0]).line_to([40.0, 0.0]).line_to([40.0, 40.0]).line_to([0.0, 40.0]).close()
            .move_to([10.0, 10.0]).line_to([30.0, 10.0]).line_to([30.0, 30.0]).line_to([10.0, 30.0]).close();
        let a = crate::Path::new()
            .move_to([0.0, 0.0]).line_to([20.0, 0.0]).line_to([20.0, 20.0]).line_to([0.0, 20.0]).close();
        let b = crate::Path::new()
            .move_to([10.0, 10.0]).line_to([30.0, 10.0]).line_to([30.0, 30.0]).line_to([10.0, 30.0]).close();
        let c = crate::Color::WHITE;
        crate::col().key("root").w_full().role("main")
            .child(crate::canvas(crate::Shape::new().fill_path(&ring, c)).key("nz").size(40.0, 40.0))
            .child(crate::canvas(crate::Shape::new().fill_path_with(&ring, crate::FillRule::EvenOdd, c)).key("eo").size(40.0, 40.0))
            .child(crate::canvas(crate::Shape::new().fill_path_with(&a, crate::FillRule::EvenOdd, c).fill_path_with(&b, crate::FillRule::EvenOdd, c)).key("two").size(40.0, 40.0))
            .child(crate::canvas(crate::Shape::new().fill_area(&crate::Area::new(&ring, crate::FillRule::EvenOdd), c)).key("area").size(40.0, 40.0))
    }
    let mut rt = Runtime::new((), view, (400.0, 300.0), Env::default());
    rt.page_mode();
    let nodes = taken(&mut rt, 400.0);
    let nz = nodes.iter().find(|n| n.path.ends_with("nz")).expect("nz");
    assert!(nz.inner.contains("<path fill=\"#ffffff\" d=\"M0,0L40,0L40,40L0,40ZM10,10L30,10L30,30L10,30Z\"/>"), "{}", nz.inner);
    assert!(!nz.inner.contains("fill-rule"), "default nonzero omits fill-rule: {}", nz.inner);
    let eo = nodes.iter().find(|n| n.path.ends_with("eo")).expect("eo");
    assert!(eo.inner.contains("<path fill=\"#ffffff\" fill-rule=\"evenodd\" d=\"M0,0L40,0L40,40L0,40ZM10,10L30,10L30,30L10,30Z\"/>"), "{}", eo.inner);
    let two = nodes.iter().find(|n| n.path.ends_with("two")).expect("two");
    assert_eq!(two.inner.matches("<path ").count(), 2, "separate compound fills must not merge into one path: {}", two.inner);
    let area = nodes.iter().find(|n| n.path.ends_with("area")).expect("area");
    assert!(area.inner.contains("<path fill=\"#ffffff\" fill-rule=\"evenodd\" d=\"M0,0L40,0L40,40L0,40ZM10,10L30,10L30,30L10,30Z\"/>"), "{}", area.inner);
}

#[test]
fn html_export_emits_pixelated_image_rendering_for_nearest_filter() {
    let cache = crate::ImageCache::new(std::sync::Arc::new(|| {}));
    let linear_img = cache.url("a.png", 32.0, 1.0);
    let nearest_img = cache.url("b.png", 32.0, 1.0);

    let mut rt = Runtime::new(
        (linear_img, nearest_img),
        |(a, b), _| {
            crate::col().key("root").w_full().role("main")
                .child(crate::el().key("lin").size(32.0, 32.0).pixels(a.pixels()))
                .child(crate::el().key("near").size(32.0, 32.0).pixels(b.pixels()).image_filter(crate::ImageFilter::Nearest))
        },
        (400.0, 200.0),
        Env::default(),
    );
    rt.page_mode();
    rt.set_viewport((400.0, 200.0), 2.0);
    rt.frame(0.0);
    rt.frame(0.0);
    let nodes = rt.export_nodes();
    let lin = nodes.iter().find(|n| n.path.ends_with("lin")).expect("lin");
    let near = nodes.iter().find(|n| n.path.ends_with("near")).expect("near");
    assert!(!lin.decls.iter().any(|d| d.0 == "image-rendering"), "{:?}", lin.decls);
    assert!(near.decls.contains(&("image-rendering", "pixelated".into())), "{:?}", near.decls);
}

#[test]
fn html_export_emits_background_for_pixels_tile_and_wrap() {
    let cache = crate::ImageCache::new(std::sync::Arc::new(|| {}));
    let tile_img = cache.url("tile.png", 24.0, 1.0);
    let photo_img = cache.url("photo.png", 64.0, 1.0);
    let plain_img = cache.url("plain.png", 32.0, 1.0);

    let mut rt = Runtime::new(
        (tile_img, photo_img, plain_img),
        |(t, p, pl), _| {
            crate::col().key("root").w_full().role("main")
                .child(crate::el().key("tiled").size(120.0, 80.0).pixels_tile(t.pixels(), [24.0, 32.0]).image_filter(crate::ImageFilter::Nearest))
                .child(crate::el().key("mirrored").size(120.0, 80.0).pixels_wrap(p.pixels(), [20.0, 10.0, 64.0, 60.0], crate::PixelsWrap::Mirror))
                .child(crate::el().key("plain").size(32.0, 32.0).pixels(pl.pixels()))
        },
        (400.0, 300.0),
        Env::default(),
    );
    rt.page_mode();
    rt.set_viewport((400.0, 300.0), 2.0);
    rt.frame(0.0);
    rt.frame(0.0);
    let nodes = rt.export_nodes();
    let tiled = nodes.iter().find(|n| n.path.ends_with("tiled")).expect("tiled");
    let mirrored = nodes.iter().find(|n| n.path.ends_with("mirrored")).expect("mirrored");
    let plain = nodes.iter().find(|n| n.path.ends_with("plain")).expect("plain");
    assert_eq!(tiled.tag, "div");
    assert!(tiled.decls.contains(&("background", "url(\"tile.png\") 0 0 / 24px 32px repeat".into())), "{:?}", tiled.decls);
    assert!(tiled.decls.contains(&("image-rendering", "pixelated".into())), "{:?}", tiled.decls);
    assert_eq!(mirrored.tag, "div");
    assert!(mirrored.decls.contains(&("background", "url(\"photo.png\") 20px 10px / 64px 60px no-repeat".into())), "{:?}", mirrored.decls);
    assert!(!mirrored.decls.iter().any(|d| d.0 == "image-rendering"), "{:?}", mirrored.decls);
    assert_eq!(plain.tag, "img");
}

#[test]
fn exports_grid_template_columns_for_auto_fit_auto_fill_and_equal_columns() {
    let mut rt = Runtime::new(
        (),
        |_, _| {
            crate::col().key("root").w_full().role("main")
                .child(crate::el().key("fit").w_full().auto_fit_columns(240.0).gap(12.0).child(crate::el().h(40.0)))
                .child(crate::el().key("fill").w_full().auto_fill_columns(180.5).gap(8.0).child(crate::el().h(40.0)))
                .child(crate::el().key("eq").w_full().equal_columns(3).child(crate::el().h(40.0)))
        },
        (800.0, 400.0),
        Env::default(),
    );
    rt.page_mode();
    let nodes = taken(&mut rt, 800.0);
    let fit = nodes.iter().find(|n| n.path.ends_with("fit")).expect("fit");
    let fill = nodes.iter().find(|n| n.path.ends_with("fill")).expect("fill");
    let eq = nodes.iter().find(|n| n.path.ends_with("eq")).expect("eq");
    assert!(fit.decls.contains(&("display", "grid".into())), "{:?}", fit.decls);
    assert!(fit.decls.contains(&("grid-template-columns", "repeat(auto-fit,minmax(240px,1fr))".into())), "{:?}", fit.decls);
    assert!(fill.decls.contains(&("grid-template-columns", "repeat(auto-fill,minmax(180.5px,1fr))".into())), "{:?}", fill.decls);
    assert!(eq.decls.contains(&("grid-template-columns", "1fr 1fr 1fr".into())), "{:?}", eq.decls);
}

#[test]
fn html_export_keeps_numeric_leading_and_punctuation_anchor_ids_and_styles_them_via_classes() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::col().key("root").w_full().role("main")
            .child(
                crate::col().key("100-overview").role("region").label("Overview")
                    .child(
                        crate::text("3D Device View")
                            .key("3d-device-view")
                            .role("heading")
                            .level(2)
                            .font(crate::tok::type_::TITLE)
                            .color(crate::Color::WHITE),
                    )
                    .child(
                        crate::row()
                            .key("42-jump")
                            .href("#3d-device-view")
                            .child(crate::text("Jump to 3D")),
                    )
                    .child(crate::text("Zero").key("0").role("heading").level(3))
                    .child(crate::text("Dash").key("--html").role("heading").level(3))
                    .child(crate::text("Under").key("_intro").role("heading").level(3))
                    .child(crate::text("Bad space").key("3d view").role("heading").level(3))
                    .child(crate::text("Bad quote").key("3d\"view").role("heading").level(3))
                    .child(crate::text("Bad dup").key("3d#dup1").role("heading").level(3))
                    .child(crate::text("Empty key").key("").role("heading").level(3)),
            )
    }
    let mut rt = Runtime::new((), view, (480.0, 600.0), Env::default());
    rt.page_mode();
    let nodes = taken(&mut rt, 480.0);
    let (body, css) = page(&[(480.0, nodes.clone())]);

    for expected in [
        "id=\"100-overview\"",
        "id=\"3d-device-view\"",
        "id=\"42-jump\" href=\"#3d-device-view\"",
        "id=\"0\"",
        "id=\"--html\"",
        "id=\"_intro\"",
    ] {
        assert!(body.contains(expected), "missing {expected} in exported HTML: {body}");
    }
    // The <h2> with numeric-leading id is styled via generated class selector (not unescaped `#3d-device-view`).
    let h2_open = body
        .split("<h2 ")
        .nth(1)
        .and_then(|s| s.split('>').next())
        .expect("exported <h2>");
    assert!(h2_open.contains("id=\"3d-device-view\""), "<h2> attrs: {h2_open}");
    let h2_classes = h2_open
        .split("class=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("<h2> class attribute");
    let style_class = h2_classes
        .split_whitespace()
        .find(|c| *c != "t")
        .expect("generated style class for <h2>");
    assert!(
        css.contains(&format!(".{style_class}{{")),
        "stylesheet must target <h2> via class .{style_class}: {css}",
    );
    assert!(!css.contains("#3d-device-view"), "stylesheet must not emit unescaped numeric #id selectors: {css}");

    // Unsafe, whitespace, duplicate-suffix, and empty keys still emit no `id` attribute.
    for (label, key) in [
        ("Bad space", "3d view"),
        ("Bad quote", "3d\"view"),
        ("Bad dup", "3d#dup1"),
        ("Empty key", ""),
    ] {
        let node = nodes
            .iter()
            .find(|n| n.inner == label)
            .unwrap_or_else(|| panic!("missing exported node for {label} ({key:?})"));
        assert!(!node.attrs.contains(" id="), "invalid key {key:?} must not emit id: {}", node.attrs);
    }
}
