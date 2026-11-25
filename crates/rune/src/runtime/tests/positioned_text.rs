use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::el::{col, el, positioned_text, text};
use crate::render::{KIND_GLYPH, KIND_RECT};
use crate::{
    GrowthSpec, Key, KeyEvent, Link, Live, Mods, PhysicalInput, PositionedText, ScrollSpec, TextHighlight, TextMark, oklch,
};

fn push_line(s: &mut String, rects: &mut Vec<[f32; 4]>, line: &str, origin: [f32; 2], cell: [f32; 2], br: &str) {
    let ([mut cx, y], [cw, ch]) = (origin, cell);
    for c in line.chars() {
        s.push(c);
        rects.push([cx, y, cw, ch]);
        cx += cw;
    }
    for b in br.chars() {
        s.push(b);
        rects.push([0.0; 4]);
    }
}

/// Builds a two-column PDF-style page (600x400) with:
/// - Column 1 (`x = 20..260`): two lines with `"foo.bar"`, `"can't"`, and ligature `"fi"`,
///   plus a paragraph gap beside Column 2.
/// - Column 2 (`x = 320..560`): two lines sharing y-bands with Column 1 and its paragraph gap.
fn make_pdf_page(tag: &str) -> PositionedText {
    let mut s = String::new();
    let mut rects = Vec::new();
    push_line(&mut s, &mut rects, &format!("{tag} foo.bar can't"), [20.0, 20.0], [8.0, 14.0], "\n");
    let mut cx = 20.0;
    for c in "A ".chars() { s.push(c); rects.push([cx, 40.0, 8.0, 14.0]); cx += 8.0; }
    let lig_rect = [cx, 40.0, 12.0, 14.0];
    for c in "fi".chars() { s.push(c); rects.push(lig_rect); }
    cx += 12.0;
    for c in "ne day".chars() { s.push(c); rects.push([cx, 40.0, 8.0, 14.0]); cx += 8.0; }
    s.push('\n'); rects.push([0.0; 4]);
    push_line(&mut s, &mut rects, "Col1 after gap", [20.0, 84.0], [8.0, 14.0], "\n");
    push_line(&mut s, &mut rects, "Col2 top line", [320.0, 20.0], [8.0, 14.0], "\n");
    push_line(&mut s, &mut rects, "Col2 beside gap", [320.0, 62.0], [8.0, 14.0], "");
    PositionedText::new(s, rects).view_box(600.0, 400.0)
}

fn plain_key(key: Key) -> KeyEvent { KeyEvent { key, mods: Mods::default() } }
fn shift_key(key: Key) -> KeyEvent { KeyEvent { key, mods: Mods { shift: true, ..Mods::default() } } }
fn cmd_key(c: char) -> KeyEvent {
    let mods = if cfg!(target_os = "macos") {
        Mods { cmd: true, logo: true, ..Mods::default() }
    } else {
        Mods { cmd: true, ctrl: true, ..Mods::default() }
    };
    KeyEvent { key: Key::Command(c), mods }
}

fn press_at<S: 'static>(rt: &mut Runtime<S>, x: f32, y: f32, t: f64) {
    rt.frame(t);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.frame(t);
}

fn release_at<S: 'static>(rt: &mut Runtime<S>, x: f32, y: f32, t: f64) {
    rt.frame(t);
    rt.pointer_up(x, y);
    rt.frame(t);
}

fn drag<S: 'static>(rt: &mut Runtime<S>, a: (f32, f32), b: (f32, f32), t0: f64) {
    press_at(rt, a.0, a.1, t0);
    rt.pointer_move(b.0, b.1);
    rt.frame(t0 + 0.01);
    release_at(rt, b.0, b.1, t0 + 0.02);
}

fn glyph_quad_count<S: 'static>(rt: &Runtime<S>) -> usize {
    rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count()
}

#[test]
fn text_spec_size_unchanged_and_zero_content_copy() {
    assert_eq!(std::mem::size_of::<crate::el::TextSpec>(), 224);
    let page = make_pdf_page("P1");
    let elem = positioned_text(&page);
    let spec = elem.text.as_deref().expect("positioned_text sets TextSpec");
    assert!(spec.content.is_empty() && spec.content.capacity() == 0, "TextSpec::content allocates 0 bytes");
    assert_eq!(spec.content_str(), page.text());
    assert_eq!(spec.source_text(), page.text());
}

#[test]
fn drag_forward_backward_double_triple_shift_and_select_all() {
    let page = make_pdf_page("P1");
    let board = crate::clipboard::memory();
    let mut rt = Runtime::new(
        page.clone(),
        |p: &PositionedText, _| {
            col().size(600.0, 400.0).child(
                positioned_text(p).key("pdf").w_full().h_full().selection_color(oklch(0.65, 0.16, 250.0).alpha(0.35)),
            )
        },
        (600.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&board);
    rt.frame(0.0);
    assert_eq!(glyph_quad_count(&rt), 0, "positioned_text emits zero glyph quads");

    // Forward drag over `"P1 foo"` on line 0 (`x = 20..68, y = 27`).
    drag(&mut rt, (21.0, 27.0), (67.0, 27.0), 0.1);
    assert_eq!(rt.selected_text().as_deref(), Some("P1 foo"));
    assert_eq!(glyph_quad_count(&rt), 0);
    assert!(rt.instances.iter().any(|i| i.params[1] == KIND_RECT), "selection highlight painted");

    // Backward drag over `"foo"` (`x = 67 -> 45`).
    drag(&mut rt, (67.0, 27.0), (45.0, 27.0), 1.0);
    assert_eq!(rt.selected_text().as_deref(), Some("foo"));
    assert!(rt.selection().unwrap().backward());

    // Double-click on `"foo"` in `"foo.bar"` selects `"foo"` (Unicode word boundary).
    // Note: `Runtime::frame(dt)` takes a frame delta in seconds.
    rt.frame(1.0);
    press_at(&mut rt, 46.0, 27.0, 0.01);
    release_at(&mut rt, 46.0, 27.0, 0.01);
    press_at(&mut rt, 46.0, 27.0, 0.02);
    release_at(&mut rt, 46.0, 27.0, 0.01);
    let word = rt.selected_text().unwrap();
    assert!(word == "foo" || word == "foo ", "double-click selected word: {word:?}");

    // Triple-click on line 0 selects only line 0 (never merges Col 2 on the same y-band!).
    press_at(&mut rt, 46.0, 27.0, 0.02);
    release_at(&mut rt, 46.0, 27.0, 0.01);
    assert_eq!(rt.selected_text().as_deref(), Some("P1 foo.bar can't\n"));

    // Shift+click extends selection into Column 2 (`Col2 top line` at `x = 345, y = 27`).
    rt.physical_input(PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    press_at(&mut rt, 345.0, 27.0, 3.0);
    release_at(&mut rt, 345.0, 27.0, 3.02);
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    let extended = rt.selected_text().unwrap();
    assert!(extended.starts_with("P1 foo.bar") && extended.contains("Col1 after gap") && extended.contains("Col"));

    // Cmd/Ctrl+A selects entire page; Cmd/Ctrl+C copies it.
    rt.key(cmd_key('a'));
    rt.frame(3.1);
    assert_eq!(rt.selected_text().as_deref(), Some(page.text()));
    rt.key(cmd_key('c'));
    rt.frame(3.2);
    assert_eq!(board.content().unwrap().text(), page.text());
}

#[test]
fn multi_page_selection_group_mixed_with_normal_text_plain_and_html_copy() {
    struct Doc { p1: PositionedText, p2: PositionedText }
    let doc = Doc { p1: make_pdf_page("Page1"), p2: make_pdf_page("Page2") };
    let board = crate::clipboard::memory();
    let mut rt = Runtime::new(
        doc,
        |d: &Doc, _| {
            col().size(600.0, 900.0).selection_group(true)
                .child(text("Document Title").key("hdr").selectable(true).copy_prefix("# "))
                .child(positioned_text(&d.p1).key("p1").size(600.0, 400.0))
                .child(positioned_text(&d.p2).key("p2").size(600.0, 400.0))
                .child(text("End Footer").key("ftr").selectable(true))
        },
        (600.0, 900.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&board);
    rt.frame(0.0);

    press_at(&mut rt, 30.0, 60.0, 0.1);
    release_at(&mut rt, 30.0, 60.0, 0.12);
    rt.key(cmd_key('a'));
    rt.frame(0.2);
    rt.key(cmd_key('c'));
    rt.frame(0.3);

    let clip = board.content().expect("Cmd/Ctrl+C copied mixed selection group");
    assert!(clip.text().starts_with("# Document Title\nPage1 foo.bar"));
    assert!(clip.text().contains("Page2 foo.bar"));
    assert!(clip.text().ends_with("End Footer"));
    let html = clip.html().expect("copy_html emitted HTML for mixed selection group");
    assert!(html.contains("Document Title") && html.contains("Page1 foo.bar") && html.contains("Page2 foo.bar") && html.contains("End Footer"));
}

#[test]
fn virtual_rows_offscreen_anchor_copy_and_rebind() {
    let pages: Vec<PositionedText> = (0..20).map(|i| make_pdf_page(&format!("Pg{i:02}"))).collect();
    let board = crate::clipboard::memory();
    let mut rt = Runtime::new(
        pages,
        |ps: &Vec<PositionedText>, _| {
            col().size(600.0, 260.0).selection_group(true).child(
                col().key("vp").w_full().h_full().scroll(ScrollSpec::default()).child(
                    col().key("pages").w_full().growth(GrowthSpec::default()).virtual_rows(
                        ps.len(),
                        120.0,
                        |_: &Vec<PositionedText>, i| Cow::Owned(format!("page-{i}")),
                        |s: &Vec<PositionedText>, i| {
                            el().size(600.0, 120.0).child(positioned_text(&s[i]).key(format!("pt-{i}")).w_full().h_full())
                        },
                    ),
                ),
            )
        },
        (600.0, 260.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&board);
    rt.frame(0.0);

    // Start a drag on Page 0 (mounted at top).
    drag(&mut rt, (22.0, 8.0), (60.0, 8.0), 0.1);
    assert!(rt.selected_text().unwrap().starts_with("Pg00"));

    // Scroll down far enough that Page 0 unmounts into pinned OffscreenText and Page 15 is mounted.
    rt.update(|_, tx| { tx.instant(); tx.scroll_to("vp", 0.0, 15.0 * 120.0); });
    rt.frame(0.016);
    rt.frame(0.016);
    let mounted: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).filter(|k| k.starts_with("pt-")).collect();
    assert!(!mounted.iter().any(|k| k == "pt-0"), "page 0 unmounted: {mounted:?}");
    assert!(mounted.iter().any(|k| k == "pt-15"), "page 15 mounted: {mounted:?}");

    // Shift+click on mounted Page 15 extends from offscreen Page 0 anchor through Page 15.
    let p15_drawn = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("pt-15")).unwrap().rect;
    rt.physical_input(PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    press_at(&mut rt, p15_drawn[0] + 60.0, p15_drawn[1] + 8.0, 0.3);
    release_at(&mut rt, p15_drawn[0] + 60.0, p15_drawn[1] + 8.0, 0.02);
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));

    // Copy spans pinned offscreen Page 0 + on-demand unbuilt Pages 1..14 + mounted Page 15.
    rt.key(cmd_key('c'));
    rt.frame(0.4);
    let copied = board.content().expect("virtual positioned_text copy").text().to_owned();
    assert!(copied.starts_with("Pg00 foo.bar"), "starts at pinned Page 0: {:?}", &copied[..40]);
    assert!(copied.contains("Pg07 foo.bar"), "includes unbuilt intermediate Page 7");
    assert!(copied.contains("Pg15"), "reaches mounted Page 15");

    // Scroll back to top: Page 0 remounts and selection rebinds cleanly.
    rt.update(|_, tx| { tx.instant(); tx.scroll_to("vp", 0.0, 0.0); });
    rt.frame(0.016);
    rt.frame(0.016);
    assert!(rt.selected_text().unwrap().starts_with("Pg00 foo.bar"));
}

#[test]
fn caret_browsing_across_lines_and_pages() {
    struct TwoPages { p1: PositionedText, p2: PositionedText }
    let state = TwoPages { p1: make_pdf_page("One"), p2: make_pdf_page("Two") };
    let board = crate::clipboard::memory();
    let mut rt = Runtime::new(
        state,
        |s: &TwoPages, _| {
            col().key("doc").size(600.0, 820.0).selection_group(true).caret_browsing(true)
                .child(positioned_text(&s.p1).key("p1").size(600.0, 400.0))
                .child(positioned_text(&s.p2).key("p2").size(600.0, 400.0))
        },
        (600.0, 820.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&board);
    rt.frame(0.0);

    rt.key(plain_key(Key::Tab));
    rt.frame(0.1);
    let p1_caret = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("p1")).unwrap().caret;
    assert!(p1_caret.is_some(), "collapsed caret visible on positioned_text");
    assert_eq!(glyph_quad_count(&rt), 0);
    assert!(rt.instances.iter().any(|i| i.params[1] == KIND_RECT), "caret bar painted");

    for k in 0..4 { rt.key(plain_key(Key::Right)); rt.frame(0.12 + k as f64 * 0.01); }
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(4..4));
    for k in 0..5 { rt.key(plain_key(Key::Down)); rt.frame(0.2 + k as f64 * 0.01); }
    let (sel_path, _) = rt.text_selection().expect("caret crossed onto Page 2");
    assert!(sel_path.ends_with("p2"), "expected caret on p2, got {sel_path}");

    // Line Home / Shift+Line End: Cmd+Left / Shift+Cmd+Right on macOS, Home / Shift+End elsewhere.
    let (line_home, shift_line_end) = if cfg!(target_os = "macos") {
        (
            KeyEvent { key: Key::Left, mods: Mods { cmd: true, logo: true, ..Mods::default() } },
            KeyEvent { key: Key::Right, mods: Mods { cmd: true, logo: true, shift: true, ..Mods::default() } },
        )
    } else {
        (plain_key(Key::Home), shift_key(Key::End))
    };
    rt.key(line_home); rt.frame(0.3);
    rt.key(shift_line_end); rt.frame(0.35);
    rt.key(cmd_key('c')); rt.frame(0.4);
    assert_eq!(board.content().unwrap().text(), "Two foo.bar can't");
}

#[test]
fn live_scale_zero_commit_and_zero_alloc_zoom_plus_view_box_layout_zoom() {
    struct ZoomState { page: PositionedText, zoom: Live<f32>, layout_zoom: f32, builds: Arc<AtomicUsize> }
    let builds = Arc::new(AtomicUsize::new(0));
    let zoom = Live::new(1.0f32);
    let state = ZoomState { page: make_pdf_page("Z1"), zoom: zoom.clone(), layout_zoom: 1.0, builds: builds.clone() };
    let mut rt = Runtime::new(
        state,
        |s: &ZoomState, _| {
            s.builds.fetch_add(1, Ordering::Relaxed);
            col().size(900.0, 600.0).child(
                el().key("page_box").size(600.0 * s.layout_zoom, 400.0 * s.layout_zoom).origin(0.0, 0.0).live_scale(&s.zoom)
                    .child(positioned_text(&s.page).key("pdf").w_full().h_full()),
            )
        },
        (900.0, 600.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.frame(0.016);

    // Steady idle frame allocates 0 bytes.
    let allocs_before_idle = crate::tooling::test_alloc::allocs();
    rt.frame(0.032);
    let idle_allocs = crate::tooling::test_alloc::allocs() - allocs_before_idle;
    assert_eq!(idle_allocs, 0, "steady idle frame must allocate nothing");

    // Warm both PaintKeep buffers once for live_scale, then drive a 10-frame zoom gesture
    // (1.05 -> 1.5): 0 commits, 0 view builds, 0 allocs!
    let commits_before = rt.commit.commits;
    let builds_before = builds.load(Ordering::Relaxed);
    zoom.set(1.02);
    rt.frame(1.0 / 60.0);
    zoom.set(1.05);
    rt.frame(1.0 / 60.0);
    let allocs_before_zoom = crate::tooling::test_alloc::allocs();
    for step in 1..=10 {
        zoom.set(1.05 + step as f32 * 0.045);
        rt.frame(1.0 / 60.0);
    }
    let zoom_allocs = crate::tooling::test_alloc::allocs() - allocs_before_zoom;
    assert_eq!(rt.commit.commits, commits_before, "live_scale zoom triggers 0 commits");
    assert_eq!(builds.load(Ordering::Relaxed), builds_before, "live_scale zoom triggers 0 view builds");
    assert_eq!(zoom_allocs, 0, "live_scale zoom allocates 0 bytes");
    assert_eq!(glyph_quad_count(&rt), 0);

    // Drag at 1.5x live_scale selects `"Z1 foo"` accurately and paints highlight with 0 glyphs.
    drag(&mut rt, (21.0 * 1.5, 27.0 * 1.5), (67.0 * 1.5, 27.0 * 1.5), 0.3);
    assert_eq!(rt.selected_text().as_deref(), Some("Z1 foo"));
    assert_eq!(glyph_quad_count(&rt), 0);

    // Hit-testing at 1.5x live_scale aligns with scaled boxes: source (21, 27) -> window (31.5, 40.5).
    assert_eq!(rt.text_offset_at("pdf", 21.0 * 1.5, 27.0 * 1.5), Some(0));
    assert_eq!(rt.text_offset_at("pdf", 27.0 * 1.5, 27.0 * 1.5), Some(1));

    // Layout-size zoom (view_box): reset live_scale to 1.0 and scale element size to 1.5x.
    zoom.set(1.0);
    rt.update(|s, _| s.layout_zoom = 1.5);
    rt.frame(0.6);
    assert_eq!(rt.text_offset_at("pdf", 21.0 * 1.5, 27.0 * 1.5), Some(0));
    assert_eq!(rt.text_offset_at("pdf", 27.0 * 1.5, 27.0 * 1.5), Some(1));
}

#[test]
fn selection_rects_tx_text_offset_at_highlights_and_marks() {
    struct S { page: PositionedText, seen_rects: Rc<RefCell<Vec<[f32; 4]>>>, tx_offset: Option<usize>, mark: Option<TextMark> }
    let seen_rects = Rc::new(RefCell::new(Vec::new()));
    let page = make_pdf_page("M1");
    let mut rt = Runtime::new(
        S { page, seen_rects: seen_rects.clone(), tx_offset: None, mark: None },
        |s: &S, cx: &mut Cx<S>| {
            *s.seen_rects.borrow_mut() = cx.selection_rects();
            if let Some(m) = &s.mark { cx.mark_texts([m.clone()]); }
            col().size(600.0, 400.0)
                .child(
                    positioned_text(&s.page)
                        .key("pdf")
                        .w_full()
                        .h_full()
                        .text_highlight(TextHighlight::new(0..2, oklch(0.85, 0.15, 95.0))),
                )
                .highlight_texts(|t| {
                    t.match_indices("foo").map(|(at, m)| TextHighlight::new(at..at + m.len(), oklch(0.8, 0.12, 140.0))).collect::<Vec<_>>()
                })
        },
        (600.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert_eq!(glyph_quad_count(&rt), 0);
    assert!(rt.instances.iter().filter(|i| i.params[1] == KIND_RECT).count() >= 2);

    // Select `"M1 foo"` and verify Cx::selection_rects + mark_texts.
    drag(&mut rt, (21.0, 27.0), (67.0, 27.0), 0.1);
    rt.frame(0.15);
    let boxes = seen_rects.borrow().clone();
    assert_eq!(boxes.len(), 1);
    assert!((boxes[0][0] - 20.0).abs() < 1.0 && (boxes[0][1] - 20.0).abs() < 1.0);

    let sel = rt.selection().unwrap();
    let mark = sel.texts()[0].mark();
    rt.update(|s, tx| { s.mark = Some(mark.clone()); tx.clear_text_selection(); });
    rt.frame(0.2);
    assert!(rt.instances.iter().filter(|i| i.params[1] == KIND_RECT).count() >= 3);

    // Verify Tx::text_offset_at resolves exact byte offset in `"M1 foo.bar"` (byte 3 = 'f').
    rt.update(|_, tx| tx.text_offset_at("pdf", 45.0, 27.0, |s: &mut S, off, _| s.tx_offset = off));
    rt.frame(0.3);
    assert_eq!(rt.state.tx_offset, Some(3));
}

#[test]
fn links_armed_drag_slop_menus_and_selectable_opt_outs() {
    struct LinkDemo { page: PositionedText, clicked: Vec<String>, allow: bool }
    let page = make_pdf_page("L1");
    let mut rt = Runtime::new(
        LinkDemo { page, clicked: Vec::new(), allow: true },
        |s: &LinkDemo, _| {
            col().size(600.0, 400.0).selection_enabled(s.allow).child(
                positioned_text(&s.page)
                    .key("pdf")
                    .w_full()
                    .h_full()
                    .link(Link::new(3..10, "https://example.com"))
                    .on_link(|s: &mut LinkDemo, id, _| s.clicked.push(id.to_owned())),
            )
        },
        (600.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // 1. Plain click on `"foo.bar"` (bytes 3..10, x = 44..100, y = 20..34) activates link.
    press_at(&mut rt, 52.0, 27.0, 0.1);
    release_at(&mut rt, 52.0, 27.0, 0.12);
    assert_eq!(rt.state.clicked, vec!["https://example.com"]);

    // 2. Drag past CLICK_SLOP starting inside the link cancels link activation and selects text.
    rt.state.clicked.clear();
    drag(&mut rt, (46.0, 27.0), (96.0, 27.0), 0.5);
    assert!(rt.state.clicked.is_empty(), "drag inside link must not fire on_link");
    assert_eq!(rt.selected_text().as_deref(), Some("foo.ba"));

    // 3. Secondary press on link queues default link_menu (Open Link / Copy Link).
    rt.pointer_move(52.0, 27.0);
    rt.secondary_down(52.0, 27.0);
    rt.frame(1.0);
    rt.secondary_up(52.0, 27.0);
    rt.frame(1.02);
    let link_menu = rt.core().host.popups.front().expect("link context menu queued");
    let ids: Vec<&str> = link_menu.menu.entries.iter().filter_map(|e| match e { crate::PopupEntry::Item(it) => Some(it.id.as_str()), _ => None }).collect();
    assert!(ids.contains(&"open-link") && ids.contains(&"copy-link"), "link_menu items: {ids:?}");
    rt.core_mut().host.popups = Default::default();

    // 4. Secondary press away from link queues selection_menu (Copy).
    rt.pointer_move(25.0, 27.0);
    rt.secondary_down(25.0, 27.0);
    rt.frame(1.2);
    rt.secondary_up(25.0, 27.0);
    rt.frame(1.22);
    let sel_menu = rt.core().host.popups.front().expect("selection context menu queued");
    let sids: Vec<&str> = sel_menu.menu.entries.iter().filter_map(|e| match e { crate::PopupEntry::Item(it) => Some(it.id.as_str()), _ => None }).collect();
    assert!(sids.contains(&"copy"), "selection_menu items: {sids:?}");
    rt.core_mut().host.popups = Default::default();

    // 5. `.selectable(false)` drops Cursor::Text, and `.selection_enabled(false)` turns off selection.
    let opt_out = positioned_text(&rt.state.page).selectable(false);
    assert_eq!(opt_out.handlers.cursor, None);
    rt.update(|s, _| s.allow = false);
    rt.frame(1.5);
    drag(&mut rt, (21.0, 27.0), (67.0, 27.0), 1.6);
    assert_eq!(rt.selected_text(), None);
}
