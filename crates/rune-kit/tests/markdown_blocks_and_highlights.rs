#![cfg(feature = "markdown")]

use rune::{Color, Cx, El, Env, Runtime, ScrollSpec, col};
use rune_kit::markdown::{Block, Highlight, Style, parse};
use rune_kit::{Kit, Markdown, MarkdownStyle};

fn by_key<S>(rt: &Runtime<S>, key: &str) -> rune::runtime::ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

// These fixtures intentionally have only blank-line-separated top-level blocks.
fn fixture_keys<'a>(src: &str, keys: &'a [&'static str]) -> Vec<(usize, &'a str)> {
    std::iter::once(0).chain(src.match_indices("\n\n").map(|(at, _)| at + 2)).zip(keys.iter().copied()).collect()
}

#[test]
fn caller_block_keys_insert_remove_edit_and_retarget_mid_flight() {
    type DocState = (String, Vec<&'static str>);
    fn view((src, keys): &DocState, _: &mut Cx<DocState>) -> El {
        col().w(320.0).child(Markdown::new(src.clone()).block_keys(fixture_keys(src, keys)).build(&Kit::neutral(true)))
    }

    let mut rt = Runtime::new(("Alpha\n\nBeta".to_owned(), vec!["p-a", "p-b"]), view, (320.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let y_a0 = by_key(&rt, "p-a").rect[1];
    let y_b0 = by_key(&rt, "p-b").rect[1];
    assert_eq!(y_a0, 0.0);
    assert!(y_b0 > 20.0);

    // 1. Insert a block at the top: old blocks keep their keys and spring down; new block fades in.
    rt.state = ("Intro\n\nAlpha\n\nBeta".to_owned(), vec!["p-intro", "p-a", "p-b"]);
    rt.invalidate();
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let intro = by_key(&rt, "p-intro");
    let a_mid = by_key(&rt, "p-a");
    let b_mid = by_key(&rt, "p-b");
    assert!(intro.opacity > 0.0 && intro.opacity < 1.0, "inserted block enters on its fade: {}", intro.opacity);
    assert!(a_mid.moving && a_mid.rect[1] > y_a0 && a_mid.rect[1] < y_b0, "p-a springs from old y to new y: {a_mid:?}");
    assert!(b_mid.moving && b_mid.rect[1] > y_b0, "p-b springs down with p-a: {b_mid:?}");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let y_a_landed = by_key(&rt, "p-a").rect[1];
    let y_b_landed = by_key(&rt, "p-b").rect[1];
    assert!((by_key(&rt, "p-intro").opacity - 1.0).abs() < 1e-3);
    assert!((y_a_landed - y_b0).abs() < 0.5 && y_b_landed > y_a_landed);

    // 2. Remove the middle block: it stays mounted while exiting and the trailing sibling springs up.
    rt.state = ("Intro\n\nBeta".to_owned(), vec!["p-intro", "p-b"]);
    rt.invalidate();
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let a_exit = by_key(&rt, "p-a");
    let b_reflow = by_key(&rt, "p-b");
    assert!(a_exit.exiting && a_exit.opacity > 0.0 && a_exit.opacity < 1.0, "removed block stays mounted while exiting: {a_exit:?}");
    assert!(b_reflow.moving && b_reflow.rect[1] < y_b_landed && b_reflow.rect[1] > y_a_landed, "p-b springs up: {b_reflow:?}");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("p-a")), "exited block unmounts after transition");
    assert!((by_key(&rt, "p-b").rect[1] - y_a_landed).abs() < 0.5);

    // 3. Edit a block in place and retarget mid-flight: height and sibling y continue smoothly without jumping.
    let h0 = by_key(&rt, "p-intro").rect[3];
    let y0 = by_key(&rt, "p-b").rect[1];
    rt.state = ("Intro line one  \nline two  \nline three\n\nBeta".to_owned(), vec!["p-intro", "p-b"]);
    rt.invalidate();
    rt.frame(0.0);
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    let h_mid = by_key(&rt, "p-intro").rect[3];
    let y_mid = by_key(&rt, "p-b").rect[1];
    assert!(h_mid > h0 && y_mid > y0, "height and sibling y are mid-spring: {h_mid} > {h0}, {y_mid} > {y0}");
    assert!(h_mid < by_key(&rt, "p-intro").target[3] && y_mid < by_key(&rt, "p-b").target[1],
        "editing animates toward the larger height and sibling position, not straight to them");

    rt.state = ("Intro line one  \nline two  \nline three  \nline four  \nline five\n\nBeta".to_owned(), vec!["p-intro", "p-b"]);
    rt.invalidate();
    rt.frame(0.0);
    let h_retarget = by_key(&rt, "p-intro").rect[3];
    let y_retarget = by_key(&rt, "p-b").rect[1];
    assert!((h_retarget - h_mid).abs() < 0.5 && (y_retarget - y_mid).abs() < 0.5, "retarget commit does not jump: {h_retarget} vs {h_mid}, {y_retarget} vs {y_mid}");
    rt.frame(1.0 / 60.0);
    assert!(by_key(&rt, "p-intro").rect[3] > h_retarget && by_key(&rt, "p-b").rect[1] > y_retarget);
}

#[test]
fn block_keys_fallback_nested_and_selectable() {
    fn view(keys: &Vec<&'static str>, _: &mut Cx<Vec<&'static str>>) -> El {
        let src = "# Heading\n\n> Quoted one\n>\n> Quoted two\n\nParagraph three";
        col().w(320.0).child(Markdown::new(src).block_keys(fixture_keys(src, keys)).selectable(true).reveal(0.15).build(&Kit::neutral(true)))
    }

    // Fewer keys, empty key and duplicate key fall back to unused b{i} keys without panicking;
    // nested blocks inside the quote keep their own b0, b1 keys under the quote's caller key.
    let mut rt = Runtime::new(vec!["sec-head", "sec-quote"], view, (320.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let paths: Vec<_> = rt.elements().into_iter().map(|e| e.path).collect();
    assert!(paths.iter().any(|p| p.ends_with("/sec-head")), "{paths:?}");
    assert!(paths.iter().any(|p| p.contains("/sec-quote/") && p.ends_with("/b0")) && paths.iter().any(|p| p.contains("/sec-quote/") && p.ends_with("/b1")), "{paths:?}");
    assert!(paths.iter().any(|p| p.ends_with("/b2")), "missing 3rd key falls back to b2: {paths:?}");

    // Extra keys, empty keys and duplicates never collide or panic.
    rt.state = vec!["", "dup", "dup", "extra-1", "extra-2"];
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let top_keys: Vec<_> = rt.elements().into_iter().filter(|e| !e.exiting && e.path.matches('/').count() == 2).filter_map(|e| e.key).collect();
    assert_eq!(top_keys, ["b0", "dup", "b2"]);

    // Selection across caller-keyed blocks copies the whole document in order.
    rt.pointer_move(4.0, 6.0);
    rt.pointer_down(4.0, 6.0);
    rt.pointer_up(4.0, 6.0);
    rt.frame(1.0 / 60.0);
    let cmd = rune::Mods { cmd: true, ..Default::default() };
    rt.key(rune::KeyEvent { key: rune::Key::Command('a'), mods: cmd });
    rt.frame(1.0 / 60.0);
    rt.key(rune::KeyEvent { key: rune::Key::Command('c'), mods: cmd });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.copy_request.as_deref(), Some("Heading\nQuoted one\nQuoted two\nParagraph three"));
}

#[test]
fn source_keys_do_not_shift_when_parsers_disagree() {
    fn view(src: &String, _: &mut Cx<String>) -> El {
        // The caller splits a loose list into two blocks and keeps an HTML block that the
        // renderer omits. Neither boundary must steal the later paragraph's durable ID.
        let keys = [(0, "list"), (src.find("- second").unwrap(), "second-item"),
            (src.find("<div>").unwrap(), "html"), (src.find("After").unwrap(), "after")];
        col().w(320.0).child(Markdown::new(src.clone()).block_keys(keys).reveal(0.15)
            .selectable(true).streaming(true).build(&Kit::neutral(true)))
    }
    let mut rt = Runtime::new("- first\n\n- second\n\n<div>ignored</div>\n\nAfter".to_owned(), view,
        (320.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(by_key(&rt, "after").text.as_deref(), Some("After"));
    assert!(rt.elements().iter().all(|e| !matches!(e.key.as_deref(), Some("second-item" | "html"))));
    // Appending a partial block takes the incremental parse path; absolute offsets stay valid.
    rt.state.push_str(" grows\n\npartial");
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(by_key(&rt, "after").text.as_deref(), Some("After grows"));
    assert!(by_key(&rt, "after").revealing, "appended words reveal under the retained key");
    assert_eq!(by_key(&rt, "b2").text.as_deref(), Some("partial"));
}

#[test]
fn nested_tasks_report_exact_source_marker_offsets_and_tab_skips_read_only() {
    let src = "Préface\n\n- plain\n  - [ ] nested\n- [x] outer\n\nTail\n\n- [ ] last\n";
    let mut rt = Runtime::new(TaskDoc { src: src.into(), handled: true, ..Default::default() },
        task_view, (320.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Space without focus does nothing. Tab walks document order, including nested tasks.
    rt.key(rune::KeyEvent { key: rune::Key::Text(" ".into()), mods: Default::default() });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.toggled.is_empty());
    for (n, (marker, key, checked)) in [("[ ] nested", "b1", true), ("[x] outer", "b1", false), ("[ ] last", "b3", true)].into_iter().enumerate() {
        rt.key(rune::KeyEvent { key: rune::Key::Tab, mods: Default::default() });
        rt.frame(1.0 / 60.0);
        rt.key(rune::KeyEvent { key: rune::Key::Text(" ".into()), mods: Default::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.toggled.len(), n + 1);
        assert_eq!(rt.state.toggled[n], (key.to_owned(), src.find(marker).unwrap(), checked));
    }
    rt.state.read_only = true;
    rt.state.toggled.clear();
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    for _ in 0..4 {
        rt.key(rune::KeyEvent { key: rune::Key::Tab, mods: Default::default() });
        rt.key(rune::KeyEvent { key: rune::Key::Text(" ".into()), mods: Default::default() });
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.toggled.is_empty());
    assert!(rt.elements().iter().filter(|e| e.role == "checkbox").all(|e| !e.focusable));
}

#[test]
fn task_controls_match_checkbox_paint_in_both_themes() {
    fn view(&(dark, checked): &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
        let kit = Kit::neutral(dark);
        col().w(320.0)
            .child(rune_kit::Checkbox::new(checked).label("Task").build(&kit).key("standard"))
            .child(Markdown::new(if checked { "- [x] Task" } else { "- [ ] Task" }).build(&kit).key("markdown"))
    }
    for dark in [false, true] {
        for checked in [false, true] {
            let mut rt = Runtime::new((dark, checked), view, (320.0, 160.0), Env::new(dark, 1.0));
            rt.frame(0.0);
            let controls: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("control")).collect();
            assert_eq!(controls.len(), 2);
            assert_eq!((controls[0].fill, controls[0].radius, controls[0].rect[2..].to_vec()),
                (controls[1].fill, controls[1].radius, controls[1].rect[2..].to_vec()));
            let marks: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("mark")).collect();
            assert_eq!((marks[0].opacity, marks[0].text_color, marks[0].rect[2..].to_vec()),
                (marks[1].opacity, marks[1].text_color, marks[1].rect[2..].to_vec()));
        }
    }
}

#[derive(Default)]
struct TaskDoc {
    src: String,
    keys: Vec<&'static str>,
    handled: bool,
    read_only: bool,
    toggled: Vec<(String, usize, bool)>,
}

fn task_view(s: &TaskDoc, _: &mut Cx<TaskDoc>) -> El {
    let mut md = Markdown::new(s.src.clone());
    if !s.keys.is_empty() {
        md = md.block_keys(fixture_keys(&s.src, &s.keys));
    }
    if s.handled {
        md = md.on_task_tx(|s: &mut TaskDoc, key, item, checked, tx| {
            s.toggled.push((key.to_owned(), item, checked));
            tx.copy(format!("{key}:{item}:{checked}"));
        });
    }
    if s.read_only {
        md = md.tasks_read_only();
    }
    col().w(320.0).child(md.build(&Kit::neutral(true)))
}

#[test]
fn task_lists_render_checkboxes_press_space_and_respect_read_only() {
    let k = Kit::neutral(true);
    let mut rt = Runtime::new(
        TaskDoc {
            src: "Notes\n\n- [ ] a\n- [x] b\n".to_owned(),
            keys: vec!["intro", "checklist"],
            handled: true,
            read_only: false,
            toggled: Vec::new(),
        },
        task_view,
        (320.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let boxes: Vec<_> = rt.elements().into_iter().filter(|e| e.role == "checkbox").collect();
    assert_eq!(boxes.len(), 2, "two checkbox elements: {boxes:?}");
    assert_eq!((boxes[0].label.as_deref(), boxes[0].item_selected, boxes[0].focusable), (Some("a"), false, true));
    assert_eq!((boxes[1].label.as_deref(), boxes[1].item_selected, boxes[1].focusable), (Some("b"), true, true));

    let marks: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("mark")).collect();
    assert_eq!((marks[0].opacity, marks[1].opacity), (0.0, 1.0));
    let controls: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("control")).collect();
    assert_eq!((controls[0].fill, controls[1].fill), (Color::TRANSPARENT, k.c.surface_weak));

    let click = |rt: &mut Runtime<TaskDoc>, r: [f32; 4]| {
        let (x, y) = (r[0] + 12.0, r[1] + r[3] * 0.5);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    };

    // Each callback identifies the source marker, not an index among tasks or list items.
    click(&mut rt, boxes[0].rect);
    assert_eq!(rt.state.toggled, [("checklist".to_owned(), 9, true)]);
    assert_eq!(rt.copy_request.as_deref(), Some("checklist:9:true"));

    click(&mut rt, boxes[1].rect);
    assert_eq!(rt.state.toggled, [("checklist".to_owned(), 9, true), ("checklist".to_owned(), 17, false)]);
    assert_eq!(rt.copy_request.as_deref(), Some("checklist:17:false"));

    // Space on the focused task checkbox toggles it through the same handler.
    rt.key(rune::KeyEvent { key: rune::Key::Text(" ".into()), mods: Default::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.toggled.last(), Some(&("checklist".to_owned(), 17, false)));
    assert_eq!(rt.state.toggled.len(), 3, "Space fires once on the focused checkbox");
    rt.key(rune::KeyEvent { key: rune::Key::Enter, mods: Default::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.toggled.len(), 3, "checkboxes toggle with Space, not Enter");

    // Without caller block_keys, on_task receives the default b{index} key.
    rt.state.keys.clear();
    rt.state.toggled.clear();
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    click(&mut rt, boxes[0].rect);
    assert_eq!(rt.state.toggled, [("b1".to_owned(), 9, true)]);

    // `.tasks_read_only()` and missing `on_task` keep role/label/checked state but ignore presses and Space.
    for (handled, read_only) in [(true, true), (false, false)] {
        rt.state.handled = handled;
        rt.state.read_only = read_only;
        rt.state.toggled.clear();
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        let ro_boxes: Vec<_> = rt.elements().into_iter().filter(|e| !e.exiting && e.role == "checkbox").collect();
        assert_eq!(ro_boxes.len(), 2);
        assert!(!ro_boxes[0].focusable && !ro_boxes[1].focusable);
        assert_eq!((ro_boxes[0].item_selected, ro_boxes[1].item_selected), (false, true));
        click(&mut rt, ro_boxes[0].rect);
        rt.key(rune::KeyEvent { key: rune::Key::Text(" ".into()), mods: Default::default() });
        rt.frame(1.0 / 60.0);
        assert!(rt.state.toggled.is_empty(), "read-only tasks must not fire: handled={handled} read_only={read_only}");
    }
}


#[test]
fn strikethrough_parses_and_paints_in_light_and_dark_themes() {
    let src = "Keep ~~struck~~ and ~~**bold** `code`~~ in\n\n- ~~item~~\n\n| Col |\n|---|\n| ~~cell~~ |\n";
    let b = parse(src);
    let Block::Para(p) = &b[0] else { panic!("{b:?}") };
    assert_eq!(p.text, "Keep struck and bold \u{202f}\u{202f}code\u{202f}\u{202f} in");
    let struck: Vec<&str> = p
        .spans
        .iter()
        .filter(|s| matches!(s.2, Style::Strike))
        .map(|&(a, z, _)| &p.text[a..z])
        .collect();
    assert_eq!(struck, ["struck", "bold \u{202f}\u{202f}code\u{202f}\u{202f}"]);

    fn view(&(dark, strike): &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
        let k = Kit::neutral(dark);
        let doc = if strike {
            "Keep ~~struck~~ and ~~**bold** `code`~~\n\n- ~~item~~\n\n| Col |\n|---|\n| ~~cell~~ |\n"
        } else {
            "Keep struck and **bold** `code`\n\n- item\n\n| Col |\n|---|\n| cell |\n"
        };
        col().w(360.0).child(Markdown::new(doc).build(&k))
    }

    for dark in [false, true] {
        let mut plain = Runtime::new((dark, false), view, (360.0, 300.0), Env::new(dark, 1.0));
        plain.frame(0.0);
        let mut struck_rt = Runtime::new((dark, true), view, (360.0, 300.0), Env::new(dark, 1.0));
        struck_rt.frame(0.0);

        let thin_rects = |rt: &Runtime<(bool, bool)>| -> Vec<[f32; 4]> {
            rt.instances
                .iter()
                .filter(|i| i.params[1] == rune::render::KIND_RECT && i.rect[3] >= 0.9 && i.rect[3] <= 2.5 && i.rect[2] > 12.0)
                .map(|i| i.rect)
                .collect()
        };
        let extra = thin_rects(&struck_rt).len().saturating_sub(thin_rects(&plain).len());
        assert!(extra >= 4, "dark={dark}: strike lines drawn for paragraph, list item and table cell ({extra} new lines)");
    }
}

#[test]
fn bare_urls_autolink_with_gfm_punctuation_rules_and_respect_opt_out() {
    let src = concat!(
        "Visit https://example.org/docs?x=1#top, http://example.org:8080/a; ",
        "and www.example.org/path/to_file! ",
        "Also (see https://en.wikipedia.org/wiki/Rust_(programming_language)) ",
        "and https://example.org/entity&nbsp; ",
        "but not `https://in.code` or [explicit](https://explicit.example/x).\n\n",
        "```\nhttps://in.fence.example\n```\n",
    );
    let b = parse(src);
    let Block::Para(p) = &b[0] else { panic!("{b:?}") };
    let links: Vec<(&str, &str)> = p.links.iter().map(|(a, z, u)| (&p.text[*a..*z], u.as_str())).collect();
    assert_eq!(
        links,
        [
            ("https://example.org/docs?x=1#top", "https://example.org/docs?x=1#top"),
            ("http://example.org:8080/a", "http://example.org:8080/a"),
            ("www.example.org/path/to_file", "http://www.example.org/path/to_file"),
            (
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            ),
            ("https://example.org/entity", "https://example.org/entity"),
            ("explicit", "https://explicit.example/x"),
        ]
    );

    // Pressing a bare `www.` link sends its `http://` destination through `on_link`, and `.autolink(false)` leaves bare URLs plain.
    struct AutoState {
        autolink: bool,
        opened: Vec<String>,
    }
    fn auto_view(s: &AutoState, _: &mut Cx<AutoState>) -> El {
        col().w(480.0).child(
            Markdown::new("Try www.example.org/hello, or [named](https://named.example).")
                .autolink(s.autolink)
                .on_link(|s: &mut AutoState, url| s.opened.push(url.to_owned()))
                .build(&Kit::neutral(true)),
        )
    }
    let mut rt = Runtime::new(AutoState { autolink: true, opened: Vec::new() }, auto_view, (480.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("http://www.example.org/hello").expect("bare www link is findable by normalized URL");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["http://www.example.org/hello"]);

    rt.state.autolink = false;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert!(rt.find("http://www.example.org/hello").is_none(), "autolink(false) disables bare URL links");
    assert!(rt.find("https://named.example").is_some(), "explicit links remain active");
}

#[test]
fn highlights_map_source_bytes_across_formatting_blocks_and_syntax_without_reshaping() {
    let src = concat!(
        "# Heading Alpha\n\n",
        "Plain **bold token** and `inline_code` plus ~~struck_word~~ and [linked text](https://example.org/hidden).\n\n",
        "Second paragraph continues here.\n\n",
        "- list item one\n",
        "- [x] task needle done\n\n",
        "> quoted thoughts\n\n",
        "| Col A | Col B |\n",
        "|---|---|\n",
        "| cell_alpha | cell_beta |\n\n",
        "```rust\nfn compute(value: i32) -> i32 {\n    value + 1\n}\n```\n",
    );

    let range_of = |needle: &str| -> std::ops::Range<usize> {
        let start = src.find(needle).unwrap_or_else(|| panic!("no {needle:?}"));
        start..start + needle.len()
    };

    // Include:
    // 0: heading substring
    // 1: span across plain -> **bold token** -> plain ("Plain **bold")
    // 2: inside inline code ("inline_code")
    // 3: inside strikethrough ("struck")
    // 4: spanning across first and second paragraphs ("hidden).\n\nSecond")
    // 5: inside list item ("item one")
    // 6: inside task item ("needle")
    // 7: inside quote ("thoughts")
    // 8: inside table cell ("cell_beta")
    // 9: inside code block ("compute(value") - marked as current!
    // 10: syntax-only range ("**") - must produce no visible highlight
    // 11: link destination syntax ("(https://example.org/hidden)") - must produce no visible highlight
    let bold_stars = src.find("**bold").unwrap();
    let link_dest = src.find("(https://example.org/hidden)").unwrap();
    let highlights = vec![
        (range_of("Alpha"), Highlight::new()),
        (range_of("Plain **bold"), Highlight::new()),
        (range_of("inline_code"), Highlight::new()),
        (range_of("struck"), Highlight::new()),
        (range_of("linked text](https://example.org/hidden).\n\nSecond"), Highlight::new()),
        (range_of("item one"), Highlight::new()),
        (range_of("needle"), Highlight::new()),
        (range_of("thoughts"), Highlight::new()),
        (range_of("cell_beta"), Highlight::new()),
        (range_of("compute(value"), Highlight::current().key("active-match")),
        (bold_stars..bold_stars + 2, Highlight::new().key("syntax-only-stars")),
        (link_dest..link_dest + "(https://example.org/hidden)".len(), Highlight::new().key("syntax-only-url")),
    ];

    type HlState = (String, Vec<(std::ops::Range<usize>, Highlight)>);
    fn hl_view((src, hls): &HlState, _: &mut Cx<HlState>) -> El {
        let k = Kit::neutral(true);
        let style = MarkdownStyle::from_kit(&k)
            .highlight_bg(rune::color::hex("#eab30838").unwrap())
            .highlight_current_bg(rune::color::hex("#f59e0b66").unwrap())
            .highlight_current_ring(Some((rune::color::hex("#f59e0b").unwrap(), 1.5)));
        col().w(520.0).child(Markdown::new(src.clone()).style(style).highlights(hls).build(&k))
    }
    fn render_with(src: &str, hls: &[(std::ops::Range<usize>, Highlight)]) -> Runtime<HlState> {
        let mut rt = Runtime::new((src.to_owned(), hls.to_vec()), hl_view, (520.0, 900.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    }

    let plain_rt = render_with(src, &[]);
    let hl_rt = render_with(src, &highlights);

    // 1. Every glyph quad in `hl_rt` is in the exact same position and UV as in `plain_rt` (no text reshaping!).
    let glyphs = |rt: &Runtime<HlState>| -> Vec<([u32; 4], [u32; 4])> {
        rt.instances
            .iter()
            .filter(|i| i.params[1] == rune::render::KIND_GLYPH)
            .map(|i| (i.rect.map(f32::to_bits), i.uv.map(f32::to_bits)))
            .collect()
    };
    assert_eq!(glyphs(&plain_rt), glyphs(&hl_rt), "highlights must never reshape text or move glyphs");

    // 2. Check exposed mark elements and labels.
    let marks: Vec<_> = hl_rt
        .elements()
        .into_iter()
        .filter(|e| e.role == "mark")
        .map(|e| (e.key.unwrap_or_default(), e.label.unwrap_or_default()))
        .collect();
    assert!(marks.contains(&("md-mark-0".into(), "Alpha".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-1".into(), "Plain bold".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-2".into(), "inline_code".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-3".into(), "struck".into())), "{marks:?}");
    // Cross-block highlight (#4) produces two mark elements: "linked text." in para 1 and "Second" in para 2.
    let cross: Vec<&str> = marks.iter().filter(|(k, _)| k == "md-mark-4").map(|(_, l)| l.as_str()).collect();
    assert_eq!(cross, ["linked text.", "Second"]);
    assert!(marks.contains(&("md-mark-5".into(), "item one".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-6".into(), "needle".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-7".into(), "thoughts".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-8".into(), "cell_beta".into())), "{marks:?}");
    assert!(marks.contains(&("active-match".into(), "compute(value".into())), "{marks:?}");

    // Syntax-only ranges produce no mark elements at all.
    assert!(!marks.iter().any(|(k, _)| k.starts_with("syntax-only")), "syntax-only ranges must not highlight: {marks:?}");

    // 3. Current match draws its inset ring, while standard matches draw rounded bg rects without a ring.
    let want_ring = rune::color::hex("#f59e0b").unwrap().premul(1.0);
    let ringed: Vec<_> = hl_rt
        .instances
        .iter()
        .filter(|i| i.params[1] == rune::render::KIND_RECT && (i.params[0] - 1.5).abs() < 1e-3 && i.border == want_ring)
        .collect();
    assert_eq!(ringed.len(), 1, "current match gets its 1.5pt ring: {ringed:?}");
}

#[test]
fn scroll_into_view_scrolls_to_highlighted_match_in_tall_document_and_code_block() {
    let mut doc = String::from("# Searchable Document\n\n");
    for i in 0..25 {
        doc.push_str(&format!("Paragraph {i} with filler words to push later content below the fold.\n\n"));
    }
    doc.push_str("Paragraph with the first_target_match in body text.\n\n");
    doc.push_str("```rust\n");
    for i in 0..30 {
        doc.push_str(&format!("let line_{i} = {i};\n"));
    }
    doc.push_str("let deep_code_target = 42;\n```\n");

    let first_range = {
        let s = doc.find("first_target_match").unwrap();
        s..s + "first_target_match".len()
    };
    let code_range = {
        let s = doc.find("deep_code_target").unwrap();
        s..s + "deep_code_target".len()
    };

    struct SearchApp {
        doc: String,
        r1: std::ops::Range<usize>,
        r2: std::ops::Range<usize>,
    }

    fn search_view(s: &SearchApp, _: &mut Cx<SearchApp>) -> El {
        let hls = [
            (s.r1.clone(), Highlight::new()),
            (s.r2.clone(), Highlight::current().key("code-hit")),
        ];
        col()
            .key("doc-scroll")
            .w_full()
            .h(200.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(Markdown::new(s.doc.clone()).highlights(&hls).build(&Kit::neutral(true)))
    }

    let mut rt = Runtime::new(
        SearchApp { doc, r1: first_range, r2: code_range },
        search_view,
        (480.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Initially both targets are well below the 200pt viewport.
    let mark_y = |rt: &Runtime<SearchApp>, key: &str| -> f32 {
        by_key(rt, key).rect[1]
    };
    let y1_initial = mark_y(&rt, &Markdown::highlight_key(0));
    let y2_initial = mark_y(&rt, "code-hit");
    assert!(y1_initial > 400.0 && y2_initial > y1_initial + 300.0, "initially below fold: {y1_initial}, {y2_initial}");

    // Scroll to the first highlight using its default key `Markdown::highlight_key(0)`.
    rt.update(|_, tx| tx.scroll_into_view(Markdown::highlight_key(0)));
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let m1 = by_key(&rt, &Markdown::highlight_key(0)).rect;
    assert!(m1[1] >= 0.0 && m1[1] + m1[3] <= 200.5, "first highlight scrolled into viewport: {m1:?}");

    // Scroll to the deep code-block highlight using its custom key `"code-hit"`.
    rt.update(|_, tx| tx.scroll_into_view("code-hit"));
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let m2 = by_key(&rt, "code-hit").rect;
    assert!(m2[1] >= 0.0 && m2[1] + m2[3] <= 200.5, "deep code-block highlight scrolled into viewport: {m2:?}");
}

#[test]
fn review_highlight_mapping_edges_and_streaming() {
    type State = (String, Vec<(std::ops::Range<usize>, Highlight)>);
    fn view((src, marks): &State, _: &mut Cx<State>) -> El {
        col().w(480.0).child(Markdown::new(src.as_str()).highlights(marks).build(&Kit::neutral(true)))
    }
    for (src, needle, expected) in [
        ("before &amp; after", "amp", "&"),
        ("before \\* after", "*", "*"),
        ("before <b>after</b>", "after", "after"),
        ("one\r\ntwo", "two", "two"),
        ("one  \r\ntwo", "two", "two"),
        ("> - nested **needle**", "needle", "needle"),
        ("left שלום world right", "שלום world", "שלום world"),
        ("| a | b |\n|---|---|\n| a\\|needle | end |", "needle", "needle"),
        ("| a |\n|---|\n| `a\\|needle` |", "needle", "needle"),
        ("`a\r\nb needle`", "needle", "needle"),
        ("`a\r\nbb cc dd ee ff gg hh`", "bb", "bb"),
        ("` a\r\nbb cc dd ee ff gg hh `", "bb", "bb"),
        ("`a bb cc dd ee ff gg\r\nhh ii`", "bb", "bb"),
        ("`a\r\nbb cc dd ee ff gg hh`", "gg", "gg"),
        ("> `one\n> two three four five`", "three", "three"),
        ("- `one\n  two three four five`", "three", "three"),
        ("https://example.org/needle", "needle", "needle"),
        ("reference [^needle]\n\n[^needle]: text", "needle", "needle"),
    ] {
        let start = src.find(needle).unwrap();
        let mut rt = Runtime::new((src.into(), vec![(start..start + needle.len(), Highlight::new())]), view, (480.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let labels: Vec<_> = rt.elements().into_iter().filter(|e| e.role == "mark").filter_map(|e| e.label).collect();
        assert_eq!(labels, [expected], "source={src:?}");
    }
    let src = "αβ **needle**\n\nsecond https://example.org/path";
    let start = src.find("needle").unwrap();
    let marks = vec![(start..start + 6, Highlight::new()), (1..3, Highlight::current()), (usize::MAX - 1..usize::MAX, Highlight::new())];
    let mut rt = Runtime::new((String::new(), marks), view, (480.0, 200.0), Env::new(true, 1.0));
    for end in 0..=src.len() {
        if !src.is_char_boundary(end) { continue; }
        rt.state.0 = src[..end].into();
        rt.invalidate();
        rt.frame(0.0);
        for e in rt.elements().into_iter().filter(|e| e.role == "mark") {
            assert_ne!(e.key.as_deref(), Some("md-mark-2"));
        }
    }
    rt.state.1 = vec![(start..start + 6, Highlight::current().fg(rune::Color::WHITE))];
    rt.invalidate();
    rt.frame(0.0);
    assert_eq!(rt.stats.shaped, 0, "changing highlights including foreground must reuse the shape cache");
}

#[test]
fn review_marks_do_not_wrap_large_offsets_or_intercept_links() {
    struct State { opened: Vec<String> }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        let src = "~~https://example.org/path~~ and שלום world";
        col().w(220.0).child(Markdown::new(src).selectable(true)
            .highlights([(10..24, Highlight::new()), (2..26, Highlight::current().key("hit"))])
            .on_link(|s: &mut State, url| s.opened.push(url.into())).build(&Kit::neutral(true)))
            .child(rune::text("plain").text_highlight(rune::TextHighlight::new(
                (u32::MAX as usize + 1)..(u32::MAX as usize + 5), rune::Color::WHITE).key("outside")))
    }
    let mut rt = Runtime::new(State { opened: Vec::new() }, view, (220.0, 180.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.find("outside").is_none(), "large offsets must not wrap to the beginning of text");
    assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("outside")));
    let (x, y) = rt.find("hit").unwrap();
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.opened, ["https://example.org/path"]);
}

#[test]
fn review_autolinks_skip_images_emails_and_unsafe_schemes() {
    let src = "HTTP://example.org/a, ![https://image.example/alt](image.png) [https://label.example](https://dest.example) <https://angle.example> `https://code.example` foo@bar.com javascript:alert(1) data:text/plain,test";
    let blocks = parse(src);
    let Block::Para(p) = &blocks[0] else { panic!("{blocks:?}") };
    assert_eq!(p.links.iter().map(|(_, _, u)| u.as_str()).collect::<Vec<_>>(),
        ["HTTP://example.org/a", "https://dest.example", "https://angle.example"]);
}

#[test]
fn review_two_thousand_matches_add_no_layout_nodes_or_shapes() {
    type State = (String, bool);
    fn view((src, marked): &State, _: &mut Cx<State>) -> El {
        let ranges = if *marked { src.match_indices("needle").map(|(i, _)| (i..i + 6, Highlight::new())).collect() } else { Vec::new() };
        col().w(480.0).h(200.0).scroll(ScrollSpec { stick_bottom: false })
            .child(Markdown::new(src.as_str()).highlights(ranges).build(&Kit::neutral(true)))
    }
    let src = "needle needle needle needle with some fictional filler words.\n\n".repeat(500);
    let mut rt = Runtime::new((src, false), view, (480.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let nodes = rt.stats.layout_nodes;
    rt.state.1 = true;
    rt.invalidate();
    rt.frame(0.0);
    assert_eq!(rt.stats.shaped, 0);
    assert_eq!(rt.stats.layout_nodes, nodes);
    assert_eq!(rt.elements().iter().filter(|e| e.role == "mark").count(), 2000);
    assert!(rt.find("md-mark-1999").is_some());
}
