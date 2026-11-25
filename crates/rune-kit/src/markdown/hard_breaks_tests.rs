//! Tests for `Markdown::hard_breaks`: inline contexts, hard-break parity, parse/memo cache
//! separation, streaming incremental reparse counts, and live-stream toggling.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rune::{Cx, Env, Runtime};

use super::*;

fn blocks(src: &str, hard_breaks: bool) -> Vec<Block> {
    parse_doc_with(src, true, hard_breaks).0
}

fn find_by_suffix<T: 'static>(rt: &Runtime<T>, suffix: &str) -> rune::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing element ending with {suffix:?}"))
}

#[test]
fn soft_breaks_in_every_inline_context_match_hard_breaks_and_default_stays_commonmark() {
    // 1. Plain paragraph (LF and CRLF) matches CommonMark hard break (two trailing spaces or backslash).
    let hard_spaces = blocks("First line  \nSecond line", false);
    let hard_backslash = blocks("First line\\\nSecond line", false);
    assert_eq!(blocks("First line\nSecond line", true), hard_spaces);
    assert_eq!(blocks("First line\r\nSecond line", true), hard_spaces);
    assert_eq!(blocks("First line\nSecond line", true), hard_backslash);
    assert_eq!(
        blocks("First line\nSecond line", false),
        vec![Block::Para(Inline { text: "First line Second line".into(), ..Inline::default() })],
        "default follows CommonMark soft break as space"
    );

    // 2. Emphasis, strong, strikethrough.
    for (soft, hard, expected_off) in [
        ("*alpha\nbeta*", "*alpha  \nbeta*", "alpha beta"),
        ("**alpha\nbeta**", "**alpha  \nbeta**", "alpha beta"),
        ("~~alpha\nbeta~~", "~~alpha  \nbeta~~", "alpha beta"),
    ] {
        assert_eq!(blocks(soft, true), blocks(hard, false), "hard_breaks(true) parity for {soft:?}");
        let off_blocks = blocks(soft, false);
        let [Block::Para(inl_off)] = off_blocks.as_slice() else { panic!() };
        assert_eq!(inl_off.text, expected_off, "default for {soft:?}");
    }

    // 3. Link text and bare autolink across soft break.
    assert_eq!(
        blocks("[first\nsecond](https://example.com)", true),
        blocks("[first  \nsecond](https://example.com)", false),
        "link text soft break matches hard break"
    );
    let link_blocks = blocks("[first\nsecond](https://example.com)", false);
    let [Block::Para(link_off)] = link_blocks.as_slice() else { panic!() };
    assert_eq!(link_off.text, "first second");
    let auto_blocks = blocks("See:\nhttps://example.com/a\nnext", true);
    let [Block::Para(auto_on)] = auto_blocks.as_slice() else { panic!() };
    assert_eq!(auto_on.text, "See:\nhttps://example.com/a\nnext");
    assert_eq!(auto_on.links, vec![(5, 26, "https://example.com/a".into())]);

    // 4. List items (bullet, ordered, task, nested).
    for (soft, hard) in [
        ("- one\n  two\n- three", "- one  \n  two\n- three"),
        ("1. first\n   second", "1. first  \n   second"),
        ("- [ ] todo\n  detail", "- [ ] todo  \n  detail"),
        ("- parent\n  - child\n    cont", "- parent\n  - child  \n    cont"),
    ] {
        assert_eq!(blocks(soft, true), blocks(hard, false), "list parity for {soft:?}");
        assert_ne!(blocks(soft, false), blocks(soft, true), "default differs for {soft:?}");
    }

    // 5. Block quotes (prefixed and lazy continuation) and QuoteFrame::text.
    assert_eq!(blocks("> line 1\n> line 2", true), blocks("> line 1  \n> line 2", false));
    assert_eq!(blocks("> line 1\nline 2", true), blocks("> line 1  \nline 2", false));
    let q_blocks = blocks("> line 1\n> line 2", false);
    let [Block::Quote(q_off)] = q_blocks.as_slice() else { panic!() };
    assert_eq!(q_off, &[Block::Para(Inline { text: "line 1 line 2".into(), ..Inline::default() })]);

    let captured: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let mut rt = Runtime::new(
        captured.clone(),
        |cap: &Rc<RefCell<Vec<String>>>, _: &mut Cx<Rc<RefCell<Vec<String>>>>| {
            let cap = cap.clone();
            col().w(400.0).child(
                Markdown::new("> first\n> second")
                    .hard_breaks(true)
                    .quote_frame(move |el, q| {
                        cap.borrow_mut().push(q.text.to_owned());
                        el
                    })
                    .render(&MarkdownStyle::neutral(true)),
            )
        },
        (400.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert_eq!(captured.borrow().as_slice(), ["first\nsecond"]);

    // 6. Headings (Setext headings carry soft breaks; ATX heading ends at newline).
    assert_eq!(blocks("Line 1\nLine 2\n===", true), blocks("Line 1  \nLine 2\n===", false));
    assert_eq!(blocks("Line 1\nLine 2\n---", true), blocks("Line 1  \nLine 2\n---", false));
    let h_blocks = blocks("Line 1\nLine 2\n===", false);
    let [Block::Heading(1, h_off)] = h_blocks.as_slice() else { panic!() };
    assert_eq!(h_off.text, "Line 1 Line 2");
    assert_eq!(blocks("# Title\nNext", true), blocks("# Title\nNext", false));

    // 7. Table cells and tables beside soft-broken paragraphs.
    // GFM tables are row-per-line, so table structure is preserved; a hard break inside a cell
    // (or a table preceded by a soft-broken paragraph) matches across modes.
    let with_table = "Intro line 1\nIntro line 2\n\n| Col A | Col B |\n|---|---|\n| a | b |\n";
    let t_off = blocks(with_table, false);
    let t_on = blocks(with_table, true);
    assert_eq!(t_off.len(), 2);
    assert_eq!(t_on.len(), 2);
    assert_eq!(t_off[1], t_on[1], "table cells unchanged when no inline soft break");
    assert_eq!(t_on[0], Block::Para(Inline { text: "Intro line 1\nIntro line 2".into(), ..Inline::default() }));

    // A table cell whose Inline contains `\n` lays out on two lines (same path as hard break).
    let cell_table = Block::Table(Table {
        align: vec![Align::Start],
        head: vec![Inline { text: "H".into(), ..Inline::default() }],
        rows: vec![vec![Inline { text: "row 1\nrow 2".into(), ..Inline::default() }]],
        open: false,
    });
    let cell_single = Block::Table(Table {
        align: vec![Align::Start],
        head: vec![Inline { text: "H".into(), ..Inline::default() }],
        rows: vec![vec![Inline { text: "row 1 row 2".into(), ..Inline::default() }]],
        open: false,
    });
    let render_table = |blk: Block| {
        let mut rt = Runtime::new(
            blk,
            |b: &Block, _: &mut Cx<Block>| {
                let style = MarkdownStyle::neutral(true);
                let r = Renderer {
                    s: style, highlights: Vec::new(), reveal: -1.0, streaming: false,
                    on_link: None, on_link_secondary: None, on_link_hover: None, link_menu: true,
                    link_look: None, link_help: None, link_ring: None, code_link: None,
                    on_task: None, tasks_read_only: false, selectable: false, scroll_wide: false,
                    wrap_anywhere: false, hard_breaks: true, tentative: None,
                    highlighter: Highlighter::builtin(), sources: Rc::from([]),
                    next_source: Cell::new(0), next_diff: Cell::new(0),
                    tasks: Rc::from([]), next_task: Cell::new(0),
                    current_block_key: RefCell::new("b0".into()),
                    #[cfg(feature = "mermaid")]
                    diagram_frame: None,
                    code_frame: None, diff_frame: None, quote_frame: None, link_favicon: None,
                    emoji: None, images: None, on_image: None, flow: Cell::default(),
                };
                let blocks = std::slice::from_ref(b);
                let dirs = bidi::Dirs::new(blocks);
                col().w(400.0).child(r.blocks_with_dirs(blocks, None, &[BlockMap::None], &[], true, &dirs).key("md"))
            },
            (400.0, 300.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        find_by_suffix(&rt, "md/b0/r0/c0").rect[3]
    };
    let h_multi = render_table(cell_table);
    let h_single = render_table(cell_single);
    assert!(h_multi > h_single * 1.4, "multiline table cell should be taller: {h_multi} vs {h_single}");

    // 8. Code spans and code blocks are NEVER turned into newlines.
    assert_eq!(blocks("`first\nsecond`", true), blocks("`first\nsecond`", false));
    let code_blocks = blocks("`first\nsecond`", true);
    let [Block::Para(code_span)] = code_blocks.as_slice() else { panic!() };
    assert!(!code_span.text.contains('\n'), "code span must not contain newline: {:?}", code_span.text);
    assert_eq!(blocks("```rust\nlet a = 1;\nlet b = 2;\n```", true), blocks("```rust\nlet a = 1;\nlet b = 2;\n```", false));
    assert_eq!(blocks("    indented 1\n    indented 2\n", true), blocks("    indented 1\n    indented 2\n", false));

    // 9. Break immediately after a lifted block image is still skipped (no leading newline).
    let after_img = blocks("![alt](pic.png)\nCaption line\nSecond caption", true);
    assert_eq!(after_img.len(), 2);
    assert!(matches!(&after_img[0], Block::Image { src, alt, .. } if src == "pic.png" && alt == "alt"));
    assert_eq!(
        after_img[1],
        Block::Para(Inline { text: "Caption line\nSecond caption".into(), ..Inline::default() }),
        "break right after image is skipped; subsequent soft break becomes newline"
    );
    let around_img = blocks("Before\n![alt](pic.png)\nAfter", true);
    assert_eq!(around_img.len(), 3);
    assert_eq!(around_img[0], Block::Para(Inline { text: "Before".into(), ..Inline::default() }));
    assert!(matches!(&around_img[1], Block::Image { .. }));
    assert_eq!(around_img[2], Block::Para(Inline { text: "After".into(), ..Inline::default() }));
}

#[test]
fn parse_cache_and_memo_keep_both_modes_separate_and_invalidate_on_toggle() {
    const SRC: &str = "Unique cache key line 1\nUnique cache key line 2";
    let src = SRC;

    // Side-by-side in one commit: both modes stay in CACHE and neither shows the other's result.
    let mut both = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| {
            let style = MarkdownStyle::neutral(true);
            col().w(480.0)
                .child(Markdown::new(SRC).hard_breaks(false).render(&style).key("off"))
                .child(Markdown::new(SRC).hard_breaks(true).render(&style).key("on"))
        },
        (480.0, 300.0),
        Env::new(true, 1.0),
    );
    both.frame(0.0);
    let el_off = find_by_suffix(&both, "off/b0");
    let el_on = find_by_suffix(&both, "on/b0");
    assert_eq!(el_off.text.as_deref(), Some("Unique cache key line 1 Unique cache key line 2"));
    assert_eq!(el_on.text.as_deref(), Some("Unique cache key line 1\nUnique cache key line 2"));
    assert!(el_on.rect[3] > el_off.rect[3] * 1.5, "hard_breaks(true) lays out two lines: off={:?} on={:?}", el_off.rect, el_on.rect);
    CACHE.with_borrow(|c| {
        assert!(c.peek(src, false).is_some(), "off mode cached");
        assert!(c.peek(src, true).is_some(), "on mode cached");
    });

    // Toggling hard_breaks on a live non-streaming Runtime invalidates whole-doc memo (RENDERS).
    struct Toggle { doc: String, hard: bool, autolink: bool }
    fn toggle_view(s: &Toggle, _: &mut Cx<Toggle>) -> El {
        col().w(480.0).child(
            Markdown::new(s.doc.clone())
                .autolink(s.autolink)
                .hard_breaks(s.hard)
                .render(&MarkdownStyle::neutral(true))
                .key("md"),
        )
    }
    let mut rt = Runtime::new(
        Toggle { doc: "Toggle line 1\nToggle line 2".into(), hard: false, autolink: true },
        toggle_view,
        (480.0, 300.0),
        Env::new(true, 1.0),
    );
    RENDERS.with(|n| n.set(0));
    rt.frame(0.0);
    let r0 = RENDERS.with(Cell::get);
    assert_eq!(find_by_suffix(&rt, "md/b0").text.as_deref(), Some("Toggle line 1 Toggle line 2"));

    rt.update(|s, tx| { tx.instant(); s.hard = true; });
    rt.frame(1.0 / 60.0);
    assert!(RENDERS.with(Cell::get) > r0, "toggling hard_breaks must miss whole-doc memo");
    assert_eq!(find_by_suffix(&rt, "md/b0").text.as_deref(), Some("Toggle line 1\nToggle line 2"));

    // autolink(false) + hard_breaks(true) takes the uncached parse path and still renders breaks.
    rt.update(|s, tx| {
        tx.instant();
        s.autolink = false;
        s.doc = "Plain\nhttps://example.com".into();
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(
        find_by_suffix(&rt, "md/b0").text.as_deref().map(unpadded),
        Some("Plain\nhttps://example.com".into()),
    );
    assert!(rt.elements().iter().all(|e| e.role != "link"), "autolink(false) keeps URL plain");
}

#[test]
fn streaming_cache_reparses_same_blocks_on_append_and_fresh_parses_on_toggle() {
    // 1. Prefix-by-prefix streaming cache under hard_breaks(true) matches full streaming parse.
    let doc = "Intro **bold\nacross lines** and `code`.\n\n- item one\n  continued\n- item two\n\n> quoted\n> line\n\nlast *words\nstill open";
    for end in (1..=doc.len()).filter(|&i| doc.is_char_boundary(i)) {
        let src = &doc[..end];
        let ((blocks, sources, tasks, starts, maps), _) = stream_cached_with_meta(src, true);
        let (want, want_sources, want_tasks, _, want_starts, want_maps) = parse_streaming_with(src, true, true);
        assert_eq!(*blocks, want, "streaming hard_breaks(true) blocks for {src:?}");
        assert_eq!(
            (&*sources, &*tasks, &*starts, &*maps),
            (&want_sources[..], &want_tasks[..], &want_starts[..], &want_maps[..]),
            "streaming hard_breaks(true) metadata for {src:?}"
        );
    }

    // 2. Prove with PARSES, REPARSED_BLOCKS and block_builds() that appending a line to a
    // multi-block stream re-parses the exact same number of blocks with hard_breaks off and on
    // (1 trailing block; 0 settled earlier blocks rebuilt).
    let measure_append = |hard_breaks: bool, tag: &str| -> ((usize, usize), usize, usize) {
        let base = format!(
            "Block {tag} zero\nwith break.\n\nBlock {tag} one\nwith break.\n\n- list {tag} a\n  more\n\n> quote {tag} a\n> quote {tag} b\n\nTail {tag} line 1\n"
        );
        let grown = format!("{base}Tail {tag} line 2\n");

        struct S { src: String, hard: bool }
        let mut rt = Runtime::new(
            S { src: base, hard: hard_breaks },
            |s: &S, _: &mut Cx<S>| {
                col().w(480.0).child(
                    Markdown::new(s.src.clone())
                        .streaming(true)
                        .hard_breaks(s.hard)
                        .render(&MarkdownStyle::neutral(true))
                        .key("md"),
                )
            },
            (480.0, 600.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);

        let p0 = PARSES.get();
        let rb0 = REPARSED_BLOCKS.get();
        reset_block_builds();
        rt.update(|s, tx| { tx.instant(); s.src = grown.clone(); });
        rt.frame(1.0 / 60.0);
        let p1 = PARSES.get();
        let rb1 = REPARSED_BLOCKS.get();
        let builds = block_builds();

        let tail_text = find_by_suffix(&rt, "md/b4").text.unwrap_or_default();
        if hard_breaks {
            assert_eq!(tail_text, format!("Tail {tag} line 1\nTail {tag} line 2"));
        } else {
            assert_eq!(tail_text, format!("Tail {tag} line 1 Tail {tag} line 2"));
        }
        ((p1.0 - p0.0, p1.1 - p0.1), rb1 - rb0, builds)
    };

    let (parses_off, reparsed_off, builds_off) = measure_append(false, "off");
    let (parses_on, reparsed_on, builds_on) = measure_append(true, "on");
    assert_eq!(parses_off, (0, 1), "off: 0 whole parses, 1 incremental tail parse");
    assert_eq!(parses_on, (0, 1), "on: 0 whole parses, 1 incremental tail parse");
    assert_eq!(reparsed_off, 1, "off: only the trailing block is re-parsed");
    assert_eq!(reparsed_on, 1, "on: only the trailing block is re-parsed");
    assert_eq!(reparsed_on, reparsed_off, "re-parsed block count matches between off and on");
    assert_eq!(builds_off, 0, "off: earlier settled blocks hit memo cache");
    assert_eq!(builds_on, 0, "on: earlier settled blocks hit memo cache");

    // 3. Switching hard_breaks on a live stream does a fresh full parse (never reuses prefix
    // blocks parsed under the other mode) and updates earlier settled blocks immediately.
    struct Live { src: String, hard: bool }
    let mut live = Runtime::new(
        Live {
            src: "Live para one\nline two.\n\nLive tail line 1\n".into(),
            hard: false,
        },
        |s: &Live, _: &mut Cx<Live>| {
            col().w(480.0).child(
                Markdown::new(s.src.clone())
                    .streaming(true)
                    .hard_breaks(s.hard)
                    .render(&MarkdownStyle::neutral(true))
                    .key("md"),
            )
        },
        (480.0, 400.0),
        Env::new(true, 1.0),
    );
    live.frame(0.0);
    assert_eq!(find_by_suffix(&live, "md/b0").text.as_deref(), Some("Live para one line two."));

    // Flip hard_breaks ON while also appending text: must NOT extend the false-mode prefix!
    let p_before = PARSES.get();
    reset_block_builds();
    live.update(|s, tx| {
        tx.instant();
        s.hard = true;
        s.src.push_str("Live tail line 2\n");
    });
    live.frame(1.0 / 60.0);
    let p_after = PARSES.get();
    assert_eq!(p_after.0 - p_before.0, 1, "switching hard_breaks on a live stream triggers a fresh full parse");
    assert_eq!(p_after.1 - p_before.1, 0, "must not extend the previous mode's prefix blocks");
    assert!(block_builds() > 0, "earlier settled blocks rebuild under the new mode");
    assert_eq!(find_by_suffix(&live, "md/b0").text.as_deref(), Some("Live para one\nline two."));
    assert_eq!(find_by_suffix(&live, "md/b1").text.as_deref(), Some("Live tail line 1\nLive tail line 2"));

    STREAMS.with_borrow(|c| {
        assert!(c.peek("Live para one\nline two.\n\nLive tail line 1\nLive tail line 2\n", true).is_some());
    });
}
