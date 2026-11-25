//! Issue #2359: copying one whole Markdown list item keeps its marker (plain text, HTML and
//! `copy_source` agree), partial intra-item selections drop the marker and `<li>` wrapper,
//! `copy_source` slices whole selected list items inside one list block verbatim (ordered,
//! bullet, custom start/delimiter, loose, task, nested, soft-break), and dragging from a list
//! marker anchors at byte 0 of that item without making the marker selectable.

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use rune_kit::{Kit, Markdown};

#[derive(Clone)]
struct Doc {
    src: String,
    copy_source: bool,
    caret_browsing: bool,
}

fn view(s: &Doc, _: &mut Cx<Doc>) -> El {
    let k = Kit::neutral(true);
    let mut md = Markdown::new(&s.src)
        .selectable(true)
        .copy_source(s.copy_source)
        .on_link(|_: &mut Doc, _| {})
        .build(&k)
        .key("note");
    if s.caret_browsing {
        md = md.caret_browsing(true);
    }
    col().w_full().padding(10.0).child(md)
}

fn runtime(src: &str, copy_source: bool, caret_browsing: bool) -> (Runtime<Doc>, rune::clipboard::MemoryClipboard) {
    let mut rt = Runtime::new(
        Doc { src: src.to_owned(), copy_source, caret_browsing },
        view,
        (480.0, 600.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);
    let clip = rune::clipboard::memory();
    rt.use_clipboard(&clip);
    (rt, clip)
}

fn tick(rt: &mut Runtime<Doc>) { rt.frame(1.0 / 60.0); }

fn cmd_c(rt: &mut Runtime<Doc>, clip: &rune::clipboard::MemoryClipboard) -> (String, Option<String>) {
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    tick(rt);
    let content = clip.content();
    let plain = content.as_ref().map(|c| c.text().to_owned()).unwrap_or_default();
    let html = content.as_ref().and_then(|c| c.html()).map(str::to_owned);
    (plain, html)
}

fn elem(rt: &Runtime<Doc>, suffix: &str) -> rune::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing element ending with {suffix}"))
}

fn drag(rt: &mut Runtime<Doc>, from: (f32, f32), to: (f32, f32)) {
    rt.pointer_move(from.0, from.1);
    rt.pointer_down(from.0, from.1);
    tick(rt);
    for k in 1..=6 {
        let f = k as f32 / 6.0;
        rt.pointer_move(from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f);
        tick(rt);
    }
    rt.pointer_up(to.0, to.1);
    tick(rt);
}

#[test]
fn single_whole_list_item_keeps_marker_and_partial_drops_it_across_plain_html_and_source() {
    let cases = [
        ("1. First\n2. Second\n3. Third", "1. First", "1. First\n2. Second", "ol", "First", "Fir"),
        ("- Alpha\n- Beta\n- Gamma", "- Alpha", "- Alpha\n- Beta", "ul", "Alpha", "Alp"),
    ];
    for (src, whole_one, whole_two, tag, word, partial) in cases {
        for copy_source in [false, true] {
            let (mut rt, clip) = runtime(src, copy_source, false);
            let item0 = elem(&rt, "note/b0/i0/#1/b0").path;

            // 1. Single whole list item keeps its marker in plain text, copy_source, and HTML.
            rt.update(|_, tx| tx.select_all_text(item0.clone()));
            tick(&mut rt);
            let (plain, html) = cmd_c(&mut rt, &clip);
            assert_eq!(plain, whole_one, "src={src:?} copy_source={copy_source}");
            assert_eq!(html.as_deref(), Some(format!("<{tag}><li>{word}</li></{tag}>").as_str()), "html whole item copy_source={copy_source}");

            // 2. Partial selection inside one item drops its marker and <li> wrapper.
            rt.update(|_, tx| tx.select_text(item0.clone(), 0..3));
            tick(&mut rt);
            let (plain, html) = cmd_c(&mut rt, &clip);
            assert_eq!(plain, partial, "partial src={src:?} copy_source={copy_source}");
            assert_eq!(html.as_deref(), Some(format!("<p>{partial}</p>").as_str()), "html partial item copy_source={copy_source}");

            // 3. Two whole items (guard): keeps both markers and wraps in <ol>/<ul>.
            let r0 = elem(&rt, "note/b0/i0/#1/b0").rect;
            let r1 = elem(&rt, "note/b0/i1/#1/b0").rect;
            drag(&mut rt, (r0[0] + 0.5, r0[1] + r0[3] / 2.0), (r1[0] + r1[2] - 1.0, r1[1] + r1[3] / 2.0));
            let (plain, html) = cmd_c(&mut rt, &clip);
            assert_eq!(plain, whole_two, "two items src={src:?} copy_source={copy_source}");
            assert!(html.as_deref().is_some_and(|h| h.starts_with(&format!("<{tag}><li>")) && h.contains("</li><li>")), "html two items: {html:?}");

            // 4. Drag starting one character into item 1 through item 2: item 1 drops its marker, item 2 keeps its.
            let item0_len = word.len();
            drag(&mut rt, (r0[0] + 10.0, r0[1] + r0[3] / 2.0), (r1[0] + r1[2] - 1.0, r1[1] + r1[3] / 2.0));
            let (plain, _) = cmd_c(&mut rt, &clip);
            assert!(!plain.starts_with("1. ") && !plain.starts_with("- "), "partial first item must not prepend marker: {plain:?}");
            assert!(plain.len() > item0_len, "selected across both items: {plain:?}");
        }
    }
}

#[test]
fn copy_source_slices_whole_list_item_subsets_preserving_inline_markdown_and_source_markers() {
    // Ordered list starting at 3 with `)` delimiter and inline Markdown.
    let ord = "3) **Third**\n4) `Fourth`\n5) *Fifth*";
    let (mut rt, clip) = runtime(ord, true, false);
    let i0 = elem(&rt, "note/b0/i0/#1/b0").path;
    rt.update(|_, tx| tx.select_all_text(i0.clone()));
    tick(&mut rt);
    assert_eq!(cmd_c(&mut rt, &clip).0, "3) **Third**");
    let r0 = elem(&rt, "note/b0/i0/#1/b0").rect;
    let r1 = elem(&rt, "note/b0/i1/#1/b0").rect;
    drag(&mut rt, (r0[0] + 0.5, r0[1] + r0[3] / 2.0), (r1[0] + r1[2] - 1.0, r1[1] + r1[3] / 2.0));
    assert_eq!(cmd_c(&mut rt, &clip).0, "3) **Third**\n4) `Fourth`");
    // Partial slice inside item 0 falls back to drawn text without marker.
    rt.update(|_, tx| tx.select_text(i0, 0..3));
    tick(&mut rt);
    assert_eq!(cmd_c(&mut rt, &clip).0, "Thi");

    // Bullet list with `*` marker.
    let star = "* **Alpha**\n* `Beta`\n* Gamma";
    let (mut rt, clip) = runtime(star, true, false);
    let s0 = elem(&rt, "note/b0/i0/#1/b0").path;
    rt.update(|_, tx| tx.select_all_text(s0));
    tick(&mut rt);
    assert_eq!(cmd_c(&mut rt, &clip).0, "* **Alpha**");

    // Loose list with blank lines between items.
    let loose = "1. **First**\n\n2. `Second`\n\n3. Third";
    let (mut rt, clip) = runtime(loose, true, false);
    let l0 = elem(&rt, "note/b0/i0/#1/b0").rect;
    let l1 = elem(&rt, "note/b0/i1/#1/b0").rect;
    drag(&mut rt, (l0[0] + 0.5, l0[1] + l0[3] / 2.0), (l1[0] + l1[2] - 1.0, l1[1] + l1[3] / 2.0));
    assert_eq!(cmd_c(&mut rt, &clip).0, "1. **First**\n\n2. `Second`");

    // Task list preserving checkbox syntax.
    let tasks = "- [ ] **Todo**\n- [x] `Done`\n- [ ] Later";
    let (mut rt, clip) = runtime(tasks, true, false);
    let t1 = elem(&rt, "note/b0/i1/#1/b0").path;
    rt.update(|_, tx| tx.select_all_text(t1));
    tick(&mut rt);
    assert_eq!(cmd_c(&mut rt, &clip).0, "- [x] `Done`");

    // Nested list: whole outer item (including nested children) and nested subset alone.
    let nested = "- **Outer**\n  - *Sub one*\n  - `Sub two`\n- Next";
    let (mut rt, clip) = runtime(nested, true, false);
    let outer = elem(&rt, "note/b0/i0/#1/b0").rect;
    let sub1 = elem(&rt, "note/b0/i0/#1/b1/i1/#1/b0").rect;
    drag(&mut rt, (outer[0] + 0.5, outer[1] + outer[3] / 2.0), (sub1[0] + sub1[2] - 1.0, sub1[1] + sub1[3] / 2.0));
    assert_eq!(cmd_c(&mut rt, &clip).0, "- **Outer**\n  - *Sub one*\n  - `Sub two`");

    let sub0_path = elem(&rt, "note/b0/i0/#1/b1/i0/#1/b0").path;
    rt.update(|_, tx| tx.select_all_text(sub0_path));
    tick(&mut rt);
    assert_eq!(cmd_c(&mut rt, &clip).0, "  - *Sub one*");

    // List item with a soft break inside it.
    let soft = "1. First line\n   second line\n2. Next";
    let (mut rt, clip) = runtime(soft, true, false);
    let f0 = elem(&rt, "note/b0/i0/#1/b0").path;
    rt.update(|_, tx| tx.select_all_text(f0));
    tick(&mut rt);
    assert_eq!(cmd_c(&mut rt, &clip).0, "1. First line\n   second line");
}

#[test]
fn drag_from_list_marker_anchors_at_item_start_click_selects_nothing_and_supports_nested_and_rtl() {
    // 1. Ordered and bullet list: drag from marker of item 1 to end of item 3.
    for (src, want) in [
        ("1. First word\n2. Second\n3. Third", "1. First word\n2. Second\n3. Third"),
        ("- Alpha word\n- Beta\n- Gamma", "- Alpha word\n- Beta\n- Gamma"),
    ] {
        let (mut rt, clip) = runtime(src, true, false);
        let li0 = elem(&rt, "note/b0/i0").rect;
        let b0 = elem(&rt, "note/b0/i0/#1/b0").rect;
        let b2 = elem(&rt, "note/b0/i2/#1/b0").rect;
        // Marker sits in the gutter between li0 left edge and item text left edge.
        let marker_pt = (li0[0] + (b0[0] - li0[0]) * 0.5, b0[1] + b0[3] * 0.5);
        assert!(marker_pt.0 < b0[0] - 1.0, "marker point must be left of item text");

        // Bare click on marker selects nothing and leaves no collapsed selection.
        rt.pointer_move(marker_pt.0, marker_pt.1);
        rt.pointer_down(marker_pt.0, marker_pt.1);
        tick(&mut rt);
        rt.pointer_up(marker_pt.0, marker_pt.1);
        tick(&mut rt);
        assert_eq!(rt.selected_text(), None, "bare click on marker selects nothing");
        assert_eq!(rt.text_selection(), None, "bare click on marker places no collapsed selection");

        // Wait past double-click interval so the drag starts as click 1.
        rt.frame(1.0);
        drag(&mut rt, marker_pt, (b2[0] + b2[2] - 1.0, b2[1] + b2[3] * 0.5));
        assert_eq!(rt.selected_text().as_deref(), Some(want));
        assert_eq!(cmd_c(&mut rt, &clip).0, want);

        // Double-click on marker selects first word of item (never the marker itself).
        rt.frame(1.0);
        rt.pointer_move(marker_pt.0, marker_pt.1);
        rt.pointer_down(marker_pt.0, marker_pt.1);
        rt.pointer_up(marker_pt.0, marker_pt.1);
        rt.pointer_down(marker_pt.0, marker_pt.1);
        rt.pointer_up(marker_pt.0, marker_pt.1);
        tick(&mut rt);
        let dbl = rt.selected_text().expect("double-click on marker selects first word");
        assert!(dbl.trim() == "First" || dbl.trim() == "Alpha", "got {dbl:?}");

        // Triple-click on marker selects the whole paragraph of item 1 (marker kept on whole copy).
        rt.pointer_down(marker_pt.0, marker_pt.1);
        rt.pointer_up(marker_pt.0, marker_pt.1);
        tick(&mut rt);
        let tri = rt.selected_text().expect("triple-click on marker selects whole item paragraph");
        assert!(tri == "1. First word" || tri == "- Alpha word", "got {tri:?}");
    }

    // 2. Click on marker alone moves no keyboard caret in caret_browsing(true).
    let (mut rt, _) = runtime("1. First\n2. Second", false, true);
    rt.update(|_, tx| tx.focus("note", true));
    tick(&mut rt);
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    tick(&mut rt);
    let before_sel = rt.text_selection().map(|(p, r)| (p.to_owned(), r));
    assert!(before_sel.is_some(), "caret placed on item 2");
    let li0 = elem(&rt, "note/b0/i0").rect;
    let b0 = elem(&rt, "note/b0/i0/#1/b0").rect;
    let marker_pt = (li0[0] + (b0[0] - li0[0]) * 0.5, b0[1] + b0[3] * 0.5);
    rt.pointer_move(marker_pt.0, marker_pt.1);
    rt.pointer_down(marker_pt.0, marker_pt.1);
    tick(&mut rt);
    rt.pointer_up(marker_pt.0, marker_pt.1);
    tick(&mut rt);
    let after_sel = rt.text_selection().map(|(p, r)| (p.to_owned(), r));
    assert_eq!(after_sel, before_sel, "bare click on marker must not move keyboard caret");
    assert_eq!(rt.selected_text(), None);

    // 3. Nested list marker anchors at the nested item, not the outer item.
    let (mut rt, clip) = runtime("- Outer\n  - Sub one\n  - Sub two\n- Next", true, false);
    let nested_li = elem(&rt, "note/b0/i0/#1/b1/i0").rect;
    let nested_b0 = elem(&rt, "note/b0/i0/#1/b1/i0/#1/b0").rect;
    let nested_b1 = elem(&rt, "note/b0/i0/#1/b1/i1/#1/b0").rect;
    let nested_marker = (nested_li[0] + (nested_b0[0] - nested_li[0]) * 0.5, nested_b0[1] + nested_b0[3] * 0.5);
    drag(&mut rt, nested_marker, (nested_b1[0] + nested_b1[2] - 1.0, nested_b1[1] + nested_b1[3] * 0.5));
    assert_eq!(cmd_c(&mut rt, &clip).0, "  - Sub one\n  - Sub two");

    // 4. Right-to-left list: marker sits on the right side, dragging left/down selects from item 1.
    let (mut rt, clip) = runtime("1. \u{0645}\u{0631}\u{062d}\u{0628}\u{0627}\n2. \u{0639}\u{0627}\u{0644}\u{0645}", true, false);
    let rtl_li0 = elem(&rt, "note/b0/i0").rect;
    let rtl_b0 = elem(&rt, "note/b0/i0/#1/b0").rect;
    let rtl_b1 = elem(&rt, "note/b0/i1/#1/b0").rect;
    let rtl_right = rtl_li0[0] + rtl_li0[2];
    let text_right = rtl_b0[0] + rtl_b0[2];
    assert!(rtl_right > text_right + 1.0, "RTL marker sits on the right of item text: li={rtl_li0:?} text={rtl_b0:?}");
    let rtl_marker = (text_right + (rtl_right - text_right) * 0.5, rtl_b0[1] + rtl_b0[3] * 0.5);
    drag(&mut rt, rtl_marker, (rtl_b1[0] + 1.0, rtl_b1[1] + rtl_b1[3] * 0.5));
    assert_eq!(cmd_c(&mut rt, &clip).0, "1. \u{0645}\u{0631}\u{062d}\u{0628}\u{0627}\n2. \u{0639}\u{0627}\u{0644}\u{0645}");
}

#[test]
fn link_context_menu_keyboard_selection_and_interactive_task_checkbox_stay_intact() {
    // 1. Right-clicking a link inside a list item still opens the link context menu.
    let (mut rt, clip) = runtime("1. [Rune](https://example.com/rune) here\n2. Next", true, false);
    let link = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("https://example.com/rune")).expect("link element");
    let link_pt = (link.rect[0] + link.rect[2] * 0.5, link.rect[1] + link.rect[3] * 0.5);
    rt.secondary_down(link_pt.0, link_pt.1);
    rt.secondary_up(link_pt.0, link_pt.1);
    tick(&mut rt);
    let menu = rt.popup_menu().expect("link context menu inside list item");
    assert!(menu.find("copy-link").is_some(), "Copy Link item present");
    rt.resolve_popup(Some("copy-link")).unwrap();
    tick(&mut rt);
    assert_eq!(clip.text().as_deref().map(String::as_str), Some("https://example.com/rune"));

    // 2. Keyboard selection in caret_browsing(true) still selects across list items as before.
    let (mut rt, _) = runtime("- Alpha\n- Beta", false, true);
    rt.update(|_, tx| tx.focus("note", true));
    tick(&mut rt);
    rt.key(KeyEvent { key: Key::Right, mods: Mods { shift: true, ..Mods::default() } });
    rt.key(KeyEvent { key: Key::Right, mods: Mods { shift: true, ..Mods::default() } });
    tick(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("Al"));

    // 3. Interactive task list checkbox keeps toggling on click (never hijacked by marker drag).
    #[derive(Default)]
    struct TaskState { toggled: Vec<(usize, bool)> }
    fn task_view(_: &TaskState, _: &mut Cx<TaskState>) -> El {
        let k = Kit::neutral(true);
        col().w_full().padding(10.0).child(
            Markdown::new("- [ ] Check me\n- [x] Done")
                .selectable(true)
                .on_task(|s: &mut TaskState, _, idx, checked| s.toggled.push((idx, checked)))
                .build(&k)
                .key("note"),
        )
    }
    let mut trt = Runtime::new(TaskState::default(), task_view, (480.0, 300.0), Env::new(true, 2.0));
    trt.frame(0.0);
    let ctrl = trt.elements().into_iter().find(|e| e.path.ends_with("note/b0/i0/control")).expect("checkbox control").rect;
    let pt = (ctrl[0] + ctrl[2] * 0.5, ctrl[1] + ctrl[3] * 0.5);
    trt.pointer_move(pt.0, pt.1);
    trt.pointer_down(pt.0, pt.1);
    trt.pointer_up(pt.0, pt.1);
    trt.frame(1.0 / 60.0);
    assert_eq!(trt.state.toggled, [(2, true)], "clicking interactive checkbox toggles it at source offset 2");
}
