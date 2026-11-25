use rune::{Cx, Env, Runtime};

use super::*;

fn rect_of<T: 'static>(rt: &Runtime<T>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element with key {key:?}"))
        .rect
}

fn by_path_suffix<T: 'static>(rt: &Runtime<T>, suffix: &str) -> rune::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing element ending with {suffix:?}"))
}

#[test]
fn prose_width_caps_top_level_prose_and_keeps_code_diffs_tables_rules_and_images_full_width() {
    struct Case {
        prose_width: Option<f32>,
        container_w: f32,
    }
    const DOC: &str = concat!(
        "# Editorial heading that wraps inside the prose measure\n\n",
        "A long top-level paragraph that stays readable at a narrower measure even when the column ",
        "has to be wide enough for code blocks, diffs and tables below.\n\n",
        "- List item prose stays inside the prose width\n\n  ```rust\n  let nested_in_list = 1;\n  ```\n\n",
        "> Block quote prose stays inside the prose width\n>\n> ```rust\n> let nested_in_quote = 2;\n> ```\n\n",
        "```rust\nlet top_level_code = \"keeps the full 800 pt column width\";\n```\n\n",
        "```diff\n@@ -1 +1 @@\n-old\n+new\n```\n\n",
        "| Column A | Column B |\n|---|---|\n| alpha | beta |\n\n",
        "---\n\n",
        "![Banner](https://example.invalid/banner.png)\n",
    );

    fn view(c: &Case, _: &mut Cx<Case>) -> El {
        let mut style = MarkdownStyle::neutral(true).table_full_width(true);
        if let Some(w) = c.prose_width {
            style = style.prose_width(w);
        }
        col().w(c.container_w).child(
            Markdown::new(DOC)
                .image_frame(|img, _| img.w_full())
                .render(&style)
                .key("md"),
        )
    }

    // With prose_width(480.0) in an 800 pt column:
    let mut rt = Runtime::new(
        Case { prose_width: Some(480.0), container_w: 800.0 },
        view,
        (800.0, 1400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Top-level prose blocks (heading, para, list, quote) cap at 480 pt and stay left-aligned.
    for (k, role) in [("b0", "heading"), ("b1", ""), ("b2", "list"), ("b3", "blockquote")] {
        let info = by_path_suffix(&rt, &format!("/{k}"));
        assert!((info.rect[0]).abs() < 0.5, "{k} should sit at x=0, got {:?}", info.rect);
        assert!(
            info.rect[2] <= 480.5 && (info.rect[2] - 480.0).abs() < 0.5,
            "{k} should cap at 480 pt, got {:?}",
            info.rect
        );
        if !role.is_empty() {
            assert_eq!(info.role, role, "{k} keeps its accessibility role");
        }
    }

    // Code nested inside a list item or blockquote stays inside its parent's 480 pt measure.
    let nested_list_code = by_path_suffix(&rt, "/b2/i0/#1/b1");
    assert!(
        nested_list_code.rect[0] + nested_list_code.rect[2] <= 480.5,
        "code inside list must stay within 480 pt, got {:?}",
        nested_list_code.rect
    );
    let nested_quote_code = by_path_suffix(&rt, "/b3/#1/b1");
    assert!(
        nested_quote_code.rect[0] + nested_quote_code.rect[2] <= 480.5,
        "code inside blockquote must stay within 480 pt, got {:?}",
        nested_quote_code.rect
    );

    // Top-level code, diff, full-width table, rule and image keep the full 800 pt column.
    for k in ["b4", "b5", "b6", "b7", "b8"] {
        let r = rect_of(&rt, k);
        assert!((r[2] - 800.0).abs() < 0.5, "{k} should span full 800 pt, got {r:?}");
    }

    // Default (prose_width unset): top-level prose spans the full 800 pt column.
    let mut def = Runtime::new(
        Case { prose_width: None, container_w: 800.0 },
        view,
        (800.0, 1400.0),
        Env::new(true, 1.0),
    );
    def.frame(0.0);
    for k in ["b0", "b1", "b2", "b3"] {
        let r = rect_of(&def, k);
        assert!((r[2] - 800.0).abs() < 0.5, "default {k} should span 800 pt, got {r:?}");
    }

    // Narrow container (300 pt < 480 pt): prose fits 300 pt without overflowing.
    let mut narrow = Runtime::new(
        Case { prose_width: Some(480.0), container_w: 300.0 },
        view,
        (300.0, 1400.0),
        Env::new(true, 1.0),
    );
    narrow.frame(0.0);
    for k in ["b0", "b1", "b2", "b3"] {
        let r = rect_of(&narrow, k);
        assert!((r[2] - 300.0).abs() < 0.5, "narrow {k} should fit 300 pt, got {r:?}");
    }

    // Non-positive and non-finite inputs clear the cap; huge values stay within the container.
    for bad in [0.0, -40.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let s = MarkdownStyle::neutral(true).prose_width(480.0).prose_width(bad);
        assert_eq!(s.prose_width, None, "prose_width({bad:?}) must clear the cap");
    }
    let mut huge = Runtime::new(
        Case { prose_width: Some(10_000.0), container_w: 800.0 },
        view,
        (800.0, 1400.0),
        Env::new(true, 1.0),
    );
    huge.frame(0.0);
    assert!((rect_of(&huge, "b1")[2] - 800.0).abs() < 0.5);
}

#[test]
fn prose_width_aligns_rtl_prose_to_right_edge_and_keeps_code_full_width() {
    const RTL_DOC: &str = concat!(
        "# عنوان عربي طويل يلتف داخل عرض النص\n\n",
        "فقرة عربية طويلة تختبر محاذاة النص من اليمين إلى اليسار مع تحديد عرض الفقرات.\n\n",
        "- عنصر قائمة باللغة العربية\n\n",
        "> اقتباس باللغة العربية\n\n",
        "```rust\nlet x = 42;\n```\n",
    );

    let mut rt = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| {
            let style = MarkdownStyle::neutral(true).prose_width(480.0);
            col().w(800.0).child(Markdown::new(RTL_DOC).render(&style).key("md"))
        },
        (800.0, 800.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    for k in ["b0", "b1", "b2", "b3"] {
        let r = rect_of(&rt, k);
        assert!((r[2] - 480.0).abs() < 0.5, "RTL {k} should cap at 480 pt, got {r:?}");
        assert!(
            (r[0] - 320.0).abs() < 0.5 && (r[0] + r[2] - 800.0).abs() < 0.5,
            "RTL {k} should align flush to the right edge (x=320..800), got {r:?}"
        );
    }
    let code_r = rect_of(&rt, "b4");
    assert!((code_r[0]).abs() < 0.5 && (code_r[2] - 800.0).abs() < 0.5, "code stays full width: {code_r:?}");
}

#[test]
fn prose_width_invalidates_settled_block_memo_and_separates_nested_blocks() {
    struct State {
        prose_width: Option<f32>,
        streaming: bool,
    }
    const DOC: &str = "Same paragraph text.\n\n> Same paragraph text.\n\nTail.";

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let base = MarkdownStyle::neutral(true);
        let mut style = base.clone().quote(Color::TRANSPARENT, 0.0, 0.0, base.text);
        if let Some(w) = s.prose_width {
            style = style.prose_width(w);
        }
        col().w(800.0).child(Markdown::new(DOC).streaming(s.streaming).render(&style).key("md"))
    }

    let mut rt = Runtime::new(
        State { prose_width: None, streaming: true },
        view,
        (800.0, 600.0),
        Env::new(true, 1.0),
    );
    reset_block_builds();
    rt.frame(0.0);
    assert!((rect_of(&rt, "b0")[2] - 800.0).abs() < 0.5);
    let builds_after_first = block_builds();
    assert!(builds_after_first > 0);

    // Enabling prose_width busts settled-block memoization and updates top-level b0.
    rt.update(|s, tx| { tx.instant(); s.prose_width = Some(480.0); });
    rt.frame(1.0 / 60.0);
    assert!(block_builds() > builds_after_first, "changing prose_width must invalidate settled-block memo");
    let r_b0 = rect_of(&rt, "b0");
    let r_b1 = rect_of(&rt, "b1");
    assert!((r_b0[2] - 480.0).abs() < 0.5, "expected b0 width 480, got {r_b0:?}");
    assert!((r_b1[2] - 480.0).abs() < 0.5, "expected b1 width 480, got {r_b1:?}");

    // Changing prose_width again busts memoization and updates to 360 pt.
    let builds_at_480 = block_builds();
    rt.update(|s, tx| { tx.instant(); s.prose_width = Some(360.0); });
    rt.frame(2.0 / 60.0);
    assert!(block_builds() > builds_at_480);
    assert!((rect_of(&rt, "b0")[2] - 360.0).abs() < 0.5);

    // Non-streaming outer cache also invalidates when prose_width changes.
    let mut rt2 = Runtime::new(
        State { prose_width: Some(480.0), streaming: false },
        view,
        (800.0, 600.0),
        Env::new(true, 1.0),
    );
    RENDERS.with(|n| n.set(0));
    rt2.frame(0.0);
    assert!((rect_of(&rt2, "b0")[2] - 480.0).abs() < 0.5);
    let r1 = RENDERS.with(Cell::get);
    rt2.update(|s, tx| { tx.instant(); s.prose_width = Some(400.0); });
    rt2.frame(1.0 / 60.0);
    assert!(RENDERS.with(Cell::get) > r1);
    assert!((rect_of(&rt2, "b0")[2] - 400.0).abs() < 0.5);
}

#[test]
fn inline_code_text_colours_plain_code_spans_and_preserves_links_and_code_links() {
    let ink = Color { r: 0.1, g: 0.6, b: 0.9, a: 1.0 };
    let link_ink = Color { r: 0.9, g: 0.3, b: 0.1, a: 1.0 };
    let default_style = MarkdownStyle::neutral(true);
    let styled = default_style.clone().inline_code_text(ink);

    assert_ne!(hash_of(&default_style), hash_of(&styled), "inline_code_text must affect MarkdownStyle hash");

    let make_renderer = |style: MarkdownStyle, sources: Rc<[Source]>, tasks: Rc<[Option<(bool, usize)>]>, code_link: Option<CodeLink>| Renderer {
        s: style,
        highlights: Vec::new(),
        reveal: 0.0,
        streaming: false,
        on_link: None,
        on_link_secondary: None,
        on_link_hover: None,
        link_menu: true,
        link_look: None,
        link_help: None,
        link_ring: None,
        code_link,
        on_task: None,
        tasks_read_only: false,
        selectable: false,
        scroll_wide: false,
        wrap_anywhere: false,
        hard_breaks: false,
        tentative: None,
        highlighter: Highlighter::builtin(),
        sources,
        next_source: Cell::new(0),
        next_diff: Cell::new(0),
        tasks,
        next_task: Cell::new(0),
        current_block_key: RefCell::new("b0".into()),
        #[cfg(feature = "mermaid")]
        diagram_frame: None,
        code_frame: None,
        diff_frame: None,
        quote_frame: None,
        link_favicon: None,
        emoji: None,
        images: None,
        on_image: None,
        flow: Cell::default(),
    };

    // 1. Plain inline code: default keeps color = None; inline_code_text sets color = Some(ink) and keeps chip bg.
    let (blocks, sources, tasks, _, _) = doc_cached("Run `cargo test` and **bold `inner`** and [link `in_link`](https://example.com).");
    let Block::Para(inl) = &blocks[0] else { panic!() };

    let r_def = make_renderer(default_style.clone(), sources.clone(), tasks.clone(), None);
    let spans_def = r_def.spans(inl, default_style.body);
    let plain_def = spans_def.iter().find(|sp| &inl.text[sp.start as usize..sp.end as usize] == "cargo test").unwrap();
    assert_eq!(plain_def.color, None);
    assert_eq!(plain_def.bg, Some(default_style.code_bg));

    let r_styled = make_renderer(styled.clone(), sources.clone(), tasks.clone(), None);
    let spans_styled = r_styled.spans(inl, styled.body);
    let plain_styled = spans_styled.iter().find(|sp| &inl.text[sp.start as usize..sp.end as usize] == "cargo test").unwrap();
    assert_eq!(plain_styled.color, Some(ink));
    assert_eq!(plain_styled.bg, Some(styled.code_bg));

    let bold_code = spans_styled.iter().find(|sp| &inl.text[sp.start as usize..sp.end as usize] == "inner").unwrap();
    assert_eq!(bold_code.color, Some(ink));

    // 2. Code inside a link keeps color = None and bg = None (inherits link colour).
    let in_link = spans_styled.iter().find(|sp| &inl.text[sp.start as usize..sp.end as usize] == "in_link").unwrap();
    assert_eq!(in_link.color, None);
    assert_eq!(in_link.bg, None);

    // 3. Markdown::code_link spans keep their own link colour, unaffected by inline_code_text.
    let cl: CodeLink = (Rc::new(|code: &str| (code == "cargo test").then(|| "cmd:test".into())), link_ink);
    let r_cl = make_renderer(styled.clone(), sources, tasks, Some(cl));
    let spans_cl = r_cl.spans(inl, styled.body);
    let cl_span = spans_cl.iter().find(|sp| &inl.text[sp.start as usize..sp.end as usize] == "cargo test").unwrap();
    assert_eq!(cl_span.color, Some(link_ink));
    assert_eq!(cl_span.bg, None);

    // 4. Changing inline_code_text invalidates Markdown's outer cache.
    struct State {
        ink: Option<Color>,
    }
    let mut rt = Runtime::new(
        State { ink: None },
        |s: &State, _: &mut Cx<State>| {
            let mut st = MarkdownStyle::neutral(true);
            if let Some(c) = s.ink {
                st = st.inline_code_text(c);
            }
            col().w(400.0).child(Markdown::new("Run `cargo test`.").render(&st).key("md"))
        },
        (400.0, 200.0),
        Env::new(true, 1.0),
    );
    RENDERS.with(|n| n.set(0));
    rt.frame(0.0);
    let before = RENDERS.with(Cell::get);
    rt.update(|s, _| s.ink = Some(ink));
    rt.frame(1.0 / 60.0);
    assert!(RENDERS.with(Cell::get) > before);
}

#[test]
fn paragraph_heading_and_image_margins_preserve_defaults_and_apply_overrides() {
    let base = MarkdownStyle::neutral(false);
    assert_eq!(base.paragraph_margin, [0.0, 12.0]);
    assert_eq!(base.heading_margin, [[28.0, 12.0], [24.0, 10.0], [20.0, 8.0], [20.0, 8.0]]);
    assert_eq!(base.image_margin, [24.0, 24.0]);

    // Level clamping and level-3 cascade to level 4+ (matching heading_font).
    let cascaded = base.clone().heading_margin(3, 12.0, 5.0);
    assert_eq!(cascaded.heading_margin[2], [12.0, 5.0]);
    assert_eq!(cascaded.heading_margin[3], [12.0, 5.0]);
    let split = cascaded.clone().heading_margin(4, 8.0, 2.0);
    assert_eq!(split.heading_margin[2], [12.0, 5.0]);
    assert_eq!(split.heading_margin[3], [8.0, 2.0]);
    let clamped = base.clone().heading_margin(0, 11.0, 3.0).heading_margin(6, 7.0, 1.0);
    assert_eq!(clamped.heading_margin[0], [11.0, 3.0]);
    assert_eq!(clamped.heading_margin[3], [7.0, 1.0]);

    // Non-finite and negative margins clamp to 0.0 via table::pts.
    let san = base
        .clone()
        .paragraph_margin(-5.0, f32::NAN)
        .heading_margin(1, f32::INFINITY, -2.0)
        .image_margin(f32::NEG_INFINITY, -1.0);
    assert_eq!(san.paragraph_margin, [0.0, 0.0]);
    assert_eq!(san.heading_margin[0], [0.0, 0.0]);
    assert_eq!(san.image_margin, [0.0, 0.0]);

    // Every margin builder changes the style hash.
    assert_ne!(hash_of(&base), hash_of(&base.clone().paragraph_margin(0.0, 20.0)));
    assert_ne!(hash_of(&base), hash_of(&base.clone().heading_margin(1, 10.0, 6.0)));
    assert_ne!(hash_of(&base), hash_of(&base.clone().heading_margin(2, 10.0, 6.0)));
    assert_ne!(hash_of(&base), hash_of(&base.clone().heading_margin(3, 10.0, 6.0)));
    assert_ne!(hash_of(&base), hash_of(&base.clone().heading_margin(4, 10.0, 6.0)));
    assert_ne!(hash_of(&base), hash_of(&base.clone().image_margin(10.0, 10.0)));

    // Headless layout: check exact block tops for default vs custom margins.
    const DOC: &str = concat!(
        "# Reader\n\n",
        "First paragraph.\n\n",
        "Second paragraph.\n\n",
        "![Blocked](https://example.invalid/never.png)\n\n",
        "## Section\n\n",
        "### Subsection\n\n",
        "#### Deep\n\n",
        "- Item para one\n\n  Item para two\n",
    );

    let run = |style: MarkdownStyle| -> Runtime<MarkdownStyle> {
        let mut rt = Runtime::new(
            style,
            |st: &MarkdownStyle, _: &mut Cx<MarkdownStyle>| {
                col().w(600.0).child(
                    Markdown::new(DOC)
                        .image_frame(|img, _| img)
                        .render(st)
                        .key("md"),
                )
            },
            (600.0, 1200.0),
            Env::new(false, 1.0),
        );
        rt.frame(0.0);
        rt
    };

    let rt_def = run(base.clone());
    let b0 = rect_of(&rt_def, "b0"); // H1
    let b1 = rect_of(&rt_def, "b1"); // Para 1
    let b2 = rect_of(&rt_def, "b2"); // Para 2
    let b3 = rect_of(&rt_def, "b3"); // Image
    let b4 = rect_of(&rt_def, "b4"); // H2
    let b5 = rect_of(&rt_def, "b5"); // H3
    let b6 = rect_of(&rt_def, "b6"); // H4
    assert!((b0[1]).abs() < 0.01, "first block top suppressed");
    assert!(((b1[1] - (b0[1] + b0[3])) - 12.0).abs() < 0.5, "default H1 bottom = 12");
    assert!(((b2[1] - (b1[1] + b1[3])) - 12.0).abs() < 0.5, "default Para->Para = max(0, 12) = 12");
    assert!(((b3[1] - (b2[1] + b2[3])) - 24.0).abs() < 0.5, "default Para->Image = max(12, 24) = 24");
    assert!(((b4[1] - (b3[1] + b3[3])) - 24.0).abs() < 0.5, "default Image->H2 = max(24, 24) = 24");
    assert!(((b5[1] - (b4[1] + b4[3])) - 20.0).abs() < 0.5, "default H2->H3 = max(10, 20) = 20");
    assert!(((b6[1] - (b5[1] + b5[3])) - 20.0).abs() < 0.5, "default H3->H4 = max(8, 20) = 20");

    // Custom margins: note paragraph_margin top (50.0) is ignored right after H1 (h1 + p rule),
    // applies to Para->Para (max(50, 16) = 50), and list-item paragraphs keep their own 8 pt gap.
    let custom = base
        .clone()
        .heading_margin(1, 40.0, 6.0)
        .paragraph_margin(50.0, 16.0)
        .image_margin(10.0, 14.0)
        .heading_margin(2, 18.0, 4.0)
        .heading_margin(3, 15.0, 5.0)
        .heading_margin(4, 9.0, 3.0);
    let rt_cust = run(custom);
    let c0 = rect_of(&rt_cust, "b0");
    let c1 = rect_of(&rt_cust, "b1");
    let c2 = rect_of(&rt_cust, "b2");
    let c3 = rect_of(&rt_cust, "b3");
    let c4 = rect_of(&rt_cust, "b4");
    let c5 = rect_of(&rt_cust, "b5");
    let c6 = rect_of(&rt_cust, "b6");
    assert!((c0[1]).abs() < 0.01, "first block top stays 0 even with heading_margin(1, 40, 6)");
    assert!(((c1[1] - (c0[1] + c0[3])) - 6.0).abs() < 0.5, "H1 + Para uses H1 bottom (6) alone, ignoring Para top (50)");
    assert!(((c2[1] - (c1[1] + c1[3])) - 50.0).abs() < 0.5, "Para->Para collapses max(50, 16) = 50");
    assert!(((c3[1] - (c2[1] + c2[3])) - 16.0).abs() < 0.5, "Para->Image collapses max(16, 10) = 16");
    assert!(((c4[1] - (c3[1] + c3[3])) - 18.0).abs() < 0.5, "Image->H2 collapses max(14, 18) = 18");
    assert!(((c5[1] - (c4[1] + c4[3])) - 15.0).abs() < 0.5, "H2->H3 collapses max(4, 15) = 15");
    assert!(((c6[1] - (c5[1] + c5[3])) - 9.0).abs() < 0.5, "H3->H4 collapses max(5, 9) = 9");

    // List-item paragraph spacing stays unchanged (8 pt between first and second paragraph inside li).
    let li_p0 = by_path_suffix(&rt_cust, "/b7/i0/#1/b0").rect;
    let li_p1 = by_path_suffix(&rt_cust, "/b7/i0/#1/b1").rect;
    assert!(
        ((li_p1[1] - (li_p0[1] + li_p0[3])) - 8.0).abs() < 0.5,
        "list-item paragraph gap must remain 8 pt, got {}",
        li_p1[1] - (li_p0[1] + li_p0[3])
    );

    // Changing margins invalidates Markdown's outer cache.
    let mut rt_cache = Runtime::new(
        12.0f32,
        |mb: &f32, _: &mut Cx<f32>| {
            let st = MarkdownStyle::neutral(false).paragraph_margin(0.0, *mb);
            col().w(400.0).child(Markdown::new("A.\n\nB.").render(&st).key("md"))
        },
        (400.0, 300.0),
        Env::new(false, 1.0),
    );
    RENDERS.with(|n| n.set(0));
    rt_cache.frame(0.0);
    let r0 = RENDERS.with(Cell::get);
    let y0 = rect_of(&rt_cache, "b1")[1];
    rt_cache.update(|mb, tx| { tx.instant(); *mb = 30.0; });
    rt_cache.frame(1.0 / 60.0);
    assert!(RENDERS.with(Cell::get) > r0);
    assert!(((rect_of(&rt_cache, "b1")[1] - y0) - 18.0).abs() < 0.5);
}

#[test]
fn code_block_radius_defaults_to_6_and_overrides_code_diff_and_deep_tail_cards() {
    const FILL: Color = Color { r: 0.13, g: 0.27, b: 0.41, a: 1.0 };
    const BORDER: Color = Color { r: 0.3, g: 0.3, b: 0.3, a: 1.0 };
    const DOC: &str = "```rust\nlet a = 1;\n```\n\n```diff\n+a\n```\n";

    let card_radii = |radius: Option<f32>, src: &str, viewport_h: f32| -> Vec<[f32; 4]> {
        let mut rt = Runtime::new(
            (radius, src.to_owned()),
            |(rad, doc): &(Option<f32>, String), _: &mut Cx<(Option<f32>, String)>| {
                let mut st = MarkdownStyle::neutral(false).code_block(FILL, BORDER);
                if let Some(r) = *rad {
                    st = st.code_block_radius(r);
                }
                col().w(500.0).child(Markdown::new(doc).render(&st).key("md"))
            },
            (500.0, viewport_h),
            Env::new(false, 1.0),
        );
        rt.set_srgb_blend(false);
        rt.frame(0.0);
        let want = FILL.premul(1.0);
        rt.instances
            .iter()
            .filter(|i| {
                i.params[1] == rune::render::KIND_RECT
                    && i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)
            })
            .map(|i| i.radii)
            .collect()
    };

    // Default keeps 6.0 pt on both fenced code and diff cards.
    assert_eq!(card_radii(None, DOC, 600.0), vec![[6.0; 4], [6.0; 4]]);

    // Square corners (0.0) and custom radius (12.0) apply to both cards.
    assert_eq!(card_radii(Some(0.0), DOC, 600.0), vec![[0.0; 4], [0.0; 4]]);
    assert_eq!(card_radii(Some(12.0), DOC, 600.0), vec![[12.0; 4], [12.0; 4]]);

    // Negative, NaN and infinite values clamp to 0.0.
    for bad in [-4.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(card_radii(Some(bad), DOC, 600.0), vec![[0.0; 4], [0.0; 4]], "bad radius {bad:?}");
    }

    // Deep-tail fallback card (over 511 nested levels + >64 blocks past the bound) also honours code_block_radius.
    let prefix = "> ".repeat(515);
    let deep: String = (0..75).map(|i| format!("{prefix}line {i}\n{prefix}\n")).collect();
    let deep_radii = |radius: Option<f32>| -> Vec<[f32; 4]> {
        let mut rt = Runtime::new(
            (radius, deep.clone()),
            |(rad, doc): &(Option<f32>, String), _: &mut Cx<(Option<f32>, String)>| {
                let mut st = MarkdownStyle::neutral(false)
                    .code_block(FILL, BORDER)
                    .quote(Color::TRANSPARENT, 0.0, 0.0, Color::BLACK);
                if let Some(r) = *rad {
                    st = st.code_block_radius(r);
                }
                col().w(500.0).child(Markdown::new(doc).render(&st).key("md"))
            },
            (500.0, 4000.0),
            Env::new(false, 1.0),
        );
        rt.set_srgb_blend(false);
        rt.frame(0.0);
        let want = FILL.premul(1.0);
        rt.instances
            .iter()
            .filter(|i| {
                i.params[1] == rune::render::KIND_RECT
                    && i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)
            })
            .map(|i| i.radii)
            .collect()
    };
    assert_eq!(deep_radii(None), vec![[6.0; 4]], "deep-tail card defaults to 6.0 pt radius");
    assert_eq!(deep_radii(Some(0.0)), vec![[0.0; 4]], "deep-tail card honours code_block_radius(0.0)");

    // Changing code_block_radius changes the style hash and invalidates Markdown's outer cache.
    let base = MarkdownStyle::neutral(false);
    assert_ne!(hash_of(&base), hash_of(&base.clone().code_block_radius(0.0)));
    let mut rt_cache = Runtime::new(
        6.0f32,
        |rad: &f32, _: &mut Cx<f32>| {
            let st = MarkdownStyle::neutral(false).code_block(FILL, BORDER).code_block_radius(*rad);
            col().w(500.0).child(Markdown::new(DOC).render(&st).key("md"))
        },
        (500.0, 600.0),
        Env::new(false, 1.0),
    );
    RENDERS.with(|n| n.set(0));
    rt_cache.frame(0.0);
    let before = RENDERS.with(Cell::get);
    rt_cache.update(|rad, _| *rad = 0.0);
    rt_cache.frame(1.0 / 60.0);
    assert!(RENDERS.with(Cell::get) > before);
}

#[test]
fn diagram_max_items_defaults_to_400_clamps_hashes_and_renders_large_mermaid() {
    let base = MarkdownStyle::neutral(false);
    assert_eq!(base.diagram.max_items, 400);
    assert_eq!(base.clone().diagram_max_items(0).diagram.max_items, 1);
    assert_eq!(base.clone().diagram_max_items(100_000).diagram.max_items, 1_500);
    assert_eq!(hash_of(base.clone().diagram_max_items(0)), hash_of(base.clone().diagram_max_items(1)));
    assert_eq!(hash_of(base.clone().diagram_max_items(1_500)), hash_of(base.clone().diagram_max_items(100_000)));
    assert_ne!(hash_of(&base), hash_of(base.clone().diagram_max_items(1_000)));

    #[cfg(feature = "mermaid")]
    {
        let mut src = String::from("flowchart TD\n");
        for i in 0..209 {
            src.push_str(&format!("  N{i} --> N{}\n", i + 1));
        }
        let md = format!("```mermaid\n{src}```\n");
        let raised = base.clone().diagram_max_items(1_000);

        // Direct mermaid helpers honour diagram_max_items while default stays at 400.
        assert!(!crate::mermaid::supported(&src));
        assert!(crate::mermaid::supported_with(&src, &crate::mermaid::DiagramStyle::neutral(false).max_items(1_000)));
        assert!(crate::mermaid::render(&src, &base).is_none());
        assert!(crate::mermaid::render(&src, &raised).is_some());
        assert!(crate::mermaid::drawing(&src, &base, 680.0).is_none());
        assert!(crate::mermaid::drawing(&src, &raised, 680.0).is_some());
        assert!(crate::mermaid::prepare(&src, &base, 680.0).is_none());
        assert!(crate::mermaid::prepare(&src, &raised, 680.0).is_some());

        // Markdown rendering: default falls back to "source"; diagram_max_items(1_000) renders
        // "diagram" with figure role and Flowchart label, and toggling invalidates memoization.
        let mut rt = Runtime::new(
            (400usize, md),
            |(cap, doc): &(usize, String), _: &mut Cx<(usize, String)>| {
                let st = MarkdownStyle::neutral(false).diagram_max_items(*cap);
                col().w(680.0).child(Markdown::new(doc).render(&st).key("md"))
            },
            (680.0, 1200.0),
            Env::new(false, 1.0),
        );
        rt.frame(0.0);
        let keys0: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
        assert!(keys0.iter().any(|k| k == "source") && !keys0.iter().any(|k| k == "diagram"), "default 400 falls back: {keys0:?}");

        RENDERS.with(|n| n.set(0));
        let before = RENDERS.with(Cell::get);
        rt.update(|s, tx| {
            tx.instant();
            s.0 = 1_000;
        });
        rt.frame(1.0 / 60.0);
        assert!(RENDERS.with(Cell::get) > before, "changing diagram_max_items invalidates cache");
        let elems = rt.elements();
        assert!(elems.iter().any(|e| e.key.as_deref() == Some("diagram")), "raised cap renders diagram");
        assert!(!elems.iter().any(|e| e.key.as_deref() == Some("source")), "raised cap does not fall back to source");
        let fig = elems.iter().find(|e| e.role == "figure").expect("diagram has figure role");
        assert_eq!(fig.label.as_deref(), Some("Flowchart"));
    }
}

#[test]
fn list_quote_and_rule_margins_preserve_defaults_and_apply_overrides() {
    let base = MarkdownStyle::neutral(false);
    assert_eq!(base.block_margin, [[8.0, 12.0], [16.0, 16.0], [32.0, 32.0]]);
    let san = base.clone().list_margin(-1.0, f32::NAN).quote_margin(f32::INFINITY, -2.0).rule_margin(-3.0, 4.0);
    assert_eq!(san.block_margin, [[0.0, 0.0], [0.0, 0.0], [0.0, 4.0]]);
    assert_ne!(hash_of(&base), hash_of(&base.clone().list_margin(0.0, 6.0)));
    assert_ne!(hash_of(&base), hash_of(&base.clone().quote_margin(6.0, 6.0)));
    assert_ne!(hash_of(&base), hash_of(&base.clone().rule_margin(6.0, 6.0)));

    // Para, Para, List, Para, Quote, Para, Rule, Para: every gap is 6 once all margins say so.
    const DOC: &str = "One.\n\nTwo.\n\n- item\n\nThree.\n\n> quote\n\nFour.\n\n---\n\nFive.";
    let gaps = |style: MarkdownStyle| -> Vec<f32> {
        let mut rt = Runtime::new(
            style,
            |st: &MarkdownStyle, _: &mut Cx<MarkdownStyle>| col().w(600.0).child(Markdown::new(DOC).render(st).key("md")),
            (600.0, 1200.0),
            Env::new(false, 1.0),
        );
        rt.frame(0.0);
        let r: Vec<[f32; 4]> = (0..8).map(|i| rect_of(&rt, &format!("b{i}"))).collect();
        r.windows(2).map(|w| w[1][1] - (w[0][1] + w[0][3])).collect()
    };
    let def = gaps(base.clone().paragraph_margin(0.0, 6.0));
    for (got, want) in def.iter().zip([6.0, 8.0, 12.0, 16.0, 16.0, 32.0, 32.0]) {
        assert!((got - want).abs() < 0.5, "default gaps {def:?}");
    }
    let even = gaps(base.paragraph_margin(0.0, 6.0).list_margin(0.0, 6.0).quote_margin(0.0, 6.0).rule_margin(0.0, 6.0));
    assert!(even.iter().all(|g| (g - 6.0).abs() < 0.5), "even gaps {even:?}");
}

#[test]
fn list_quote_rule_margins_first_last_nesting_collapsing_and_cache_isolation() {
    use super::cache::PARSES;
    let k = crate::Kit::neutral(false);
    let base = MarkdownStyle::from_kit(&k);

    // 1. Reporter's exact example: paragraphs 6 apart, list_margin(0, 6) makes all gaps even.
    const REPORTER_DOC: &str = "One.\n\nTwo.\n\n- item\n\nThree.";
    let measure_gaps = |doc: &str, n: usize, style: MarkdownStyle| -> (Vec<f32>, [f32; 4], [f32; 4], [f32; 4]) {
        let mut rt = Runtime::new(
            (doc.to_owned(), style),
            |(doc, st): &(String, MarkdownStyle), _: &mut Cx<(String, MarkdownStyle)>| col().w(600.0).child(Markdown::new(doc.as_str()).render(st).key("md")),
            (600.0, 1200.0),
            Env::new(false, 1.0),
        );
        rt.frame(0.0);
        let rects: Vec<[f32; 4]> = (0..n).map(|i| rect_of(&rt, &format!("b{i}"))).collect();
        let gaps = rects.windows(2).map(|w| w[1][1] - (w[0][1] + w[0][3])).collect();
        let md = rect_of(&rt, "md");
        (gaps, md, rects[0], *rects.last().unwrap())
    };
    let (def_gaps, _, _, _) = measure_gaps(REPORTER_DOC, 4, base.clone().paragraph_margin(0.0, 6.0));
    for (got, want) in def_gaps.iter().zip([6.0, 8.0, 12.0]) {
        assert!((got - want).abs() < 0.5, "default list gaps {def_gaps:?}");
    }
    let (even_gaps, _, _, _) = measure_gaps(REPORTER_DOC, 4, base.clone().paragraph_margin(0.0, 6.0).list_margin(0.0, 6.0));
    assert!(even_gaps.iter().all(|g| (g - 6.0).abs() < 0.5), "even list gaps {even_gaps:?}");

    // 2. First block top margin and last block bottom margin are suppressed for list, quote, rule.
    let big = base.clone().list_margin(40.0, 40.0).quote_margin(40.0, 40.0).rule_margin(40.0, 40.0);
    for doc in ["- only list", "> only quote", "---"] {
        let (_, md, first, last) = measure_gaps(doc, 1, big.clone());
        assert!(first[1].abs() < 0.01, "first block top suppressed for {doc:?}: {first:?}");
        assert!(((last[1] + last[3]) - (md[1] + md[3])).abs() < 0.5, "last block bottom suppressed for {doc:?}: last={last:?} md={md:?}");
    }

    // 3. Rule next to a heading collapses via max(rule_bottom, heading_top) (same CSS rule as #1403).
    let (rh_big, _, _, _) = measure_gaps("---\n\n## Next", 2, base.clone().rule_margin(10.0, 44.0).heading_margin(2, 24.0, 10.0));
    assert!((rh_big[0] - 44.0).abs() < 0.5, "rule(44) + h2(24) collapses to 44: {rh_big:?}");
    let (rh_small, _, _, _) = measure_gaps("---\n\n## Next", 2, base.clone().rule_margin(10.0, 6.0).heading_margin(2, 24.0, 10.0));
    assert!((rh_small[0] - 24.0).abs() < 0.5, "rule(6) + h2(24) collapses to 24: {rh_small:?}");

    // 4. Nested list inside a list item keeps its own 4 pt spacing regardless of list_margin.
    let mut rt_nested = Runtime::new(
        base.clone().list_margin(30.0, 30.0),
        |st: &MarkdownStyle, _: &mut Cx<MarkdownStyle>| {
            col().w(600.0).child(Markdown::new("- parent\n  - nested").render(st).key("md"))
        },
        (600.0, 400.0),
        Env::new(false, 1.0),
    );
    rt_nested.frame(0.0);
    let parent_p = by_path_suffix(&rt_nested, "/b0/i0/#1/b0").rect;
    let nested_list = by_path_suffix(&rt_nested, "/b0/i0/#1/b1").rect;
    let nested_gap = nested_list[1] - (parent_p[1] + parent_p[3]);
    assert!((nested_gap - 4.0).abs() < 0.5, "nested list keeps 4 pt top margin, got {nested_gap}");

    // 5. List inside a quote collapses through the quote (max(quote_bottom, list_bottom)).
    let (ql_gaps, _, _, _) = measure_gaps(
        "> - quoted item\n\nAfter.",
        2,
        base.clone().quote_margin(4.0, 4.0).list_margin(4.0, 26.0).paragraph_margin(0.0, 4.0),
    );
    assert!((ql_gaps[0] - 26.0).abs() < 0.5, "list inside quote collapses out to 26: {ql_gaps:?}");

    // 6. Changing list/quote/rule margins re-lays out (RENDERS increases) and does NOT re-parse (PARSES == (0, 0)).
    const CACHE_DOC: &str = "Unique-2365-cache-proof.\n\n- item\n\n> quote\n\n---\n\nEnd.";
    let mut rt_cache = Runtime::new(
        6.0f32,
        |m: &f32, _: &mut Cx<f32>| {
            let st = MarkdownStyle::neutral(false)
                .paragraph_margin(0.0, 6.0)
                .list_margin(*m, *m)
                .quote_margin(*m, *m)
                .rule_margin(*m, *m);
            col().w(500.0).child(Markdown::new(CACHE_DOC).render(&st).key("md"))
        },
        (500.0, 800.0),
        Env::new(false, 1.0),
    );
    rt_cache.frame(0.0);
    let y_end_before = rect_of(&rt_cache, "b4")[1];
    RENDERS.with(|n| n.set(0));
    PARSES.with(|p| p.set((0, 0)));
    rt_cache.update(|m, tx| { tx.instant(); *m = 28.0; });
    rt_cache.frame(1.0 / 60.0);
    assert!(RENDERS.with(Cell::get) > 0, "margin change must re-render/re-layout");
    assert_eq!(PARSES.with(Cell::get), (0, 0), "margin change must not re-parse cached source");
    let y_end_after = rect_of(&rt_cache, "b4")[1];
    assert!(y_end_after > y_end_before + 60.0, "blocks moved further apart: before={y_end_before} after={y_end_after}");

    // 7. Same source rendered with two styles in one frame keeps independent layouts in the memo.
    const SHARED_DOC: &str = "Shared-2365.\n\n- item\n\nAfter.";
    let mut rt_two = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| {
            let tight = MarkdownStyle::neutral(false).paragraph_margin(0.0, 6.0).list_margin(6.0, 6.0);
            let loose = MarkdownStyle::neutral(false).paragraph_margin(0.0, 6.0).list_margin(30.0, 30.0);
            col().w(500.0)
                .child(Markdown::new(SHARED_DOC).render(&tight).key("tight"))
                .child(Markdown::new(SHARED_DOC).render(&loose).key("loose"))
        },
        (500.0, 800.0),
        Env::new(false, 1.0),
    );
    rt_two.frame(0.0);
    let tight_h = rect_of(&rt_two, "tight")[3];
    let loose_h = rect_of(&rt_two, "loose")[3];
    assert!(((loose_h - tight_h) - 48.0).abs() < 1.0, "two styles on same source must not collide in memo: tight={tight_h} loose={loose_h}");
}

